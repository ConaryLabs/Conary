// conary-core/src/recipe/kitchen/cook.rs

//! Cook: the actual build execution for a single recipe

use crate::ccs::builder::{CcsBuilder, write_ccs_package};
use crate::ccs::convert::command_evidence::extract_invocations_from_shell_text;
use crate::ccs::manifest::{CcsManifest, ManifestProvenance, PackageDep};
use crate::container::{BindMount, ContainerConfig, Sandbox};
use crate::error::{Error, Result};
use crate::recipe::format::{Recipe, SourceSection, is_remote_url};
use crate::recipe::hermetic::ReproducibilityConfig;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;
use tracing::{debug, info};

use super::Kitchen;
use super::archive::{apply_patch, extract_archive};
use super::local_source::{copy_dir_contents, materialize_local_source_from_file_list};
use super::provenance_capture::ProvenanceCapture;

const DANGEROUS_BUILD_ENV_VARS: &[&str] =
    &["LD_PRELOAD", "LD_LIBRARY_PATH", "LD_AUDIT", "LD_BIND_NOT"];

fn is_dangerous_build_env_var(key: &str) -> bool {
    DANGEROUS_BUILD_ENV_VARS.contains(&key)
}

fn filtered_build_env(env: &[(String, String)]) -> impl Iterator<Item = (&str, &str)> {
    env.iter()
        .filter(|(key, _)| !is_dangerous_build_env_var(key))
        .map(|(key, value)| (key.as_str(), value.as_str()))
}

fn apply_direct_build_env(cmd: &mut Command, env: &[(String, String)]) {
    cmd.env_clear()
        .env("HOME", "/root")
        .env("TERM", "xterm")
        .env("LC_ALL", "C")
        .env("SHELL", "/bin/sh");

    if !env.iter().any(|(key, _)| key == "PATH") {
        cmd.env("PATH", "/usr/bin:/usr/sbin:/bin:/sbin:/tools/bin");
    }

    for (key, value) in filtered_build_env(env) {
        cmd.env(key, value);
    }
}

fn chroot_env_args(env: &[(String, String)], jobs: u32) -> Vec<String> {
    let mut env_args = vec!["env".to_string(), "-i".to_string()];
    for (key, value) in filtered_build_env(env) {
        env_args.push(format!("{key}={value}"));
    }
    env_args.push("PATH=/usr/bin:/usr/sbin:/bin:/sbin:/tools/bin".to_string());
    env_args.push("HOME=/root".to_string());
    env_args.push("TERM=xterm".to_string());
    env_args.push("LC_ALL=C".to_string());
    env_args.push(format!("MAKEFLAGS=-j{jobs}"));
    env_args
}

fn translate_path_for_chroot(path: &Path, sysroot: &Path) -> PathBuf {
    match path.strip_prefix(sysroot) {
        Ok(relative) => Path::new("/").join(relative),
        Err(_) => path.to_path_buf(),
    }
}

fn translate_env_for_chroot(env: &[(String, String)], sysroot: &Path) -> Vec<(String, String)> {
    env.iter()
        .map(|(key, value)| {
            let translated = if Path::new(value).is_absolute() {
                translate_path_for_chroot(Path::new(value), sysroot)
                    .to_string_lossy()
                    .to_string()
            } else {
                value.clone()
            };
            (key.clone(), translated)
        })
        .collect()
}

fn translate_command_for_chroot(command: &str, sysroot: &Path) -> String {
    let prefix = sysroot.to_string_lossy();
    let prefix = prefix.trim_end_matches('/');
    if prefix.is_empty() {
        return command.to_string();
    }
    command.replace(prefix, "")
}

fn configure_provenance_from_kitchen(
    kitchen: &Kitchen,
    provenance: &mut ProvenanceCapture,
) -> Result<()> {
    provenance.origin_class = kitchen.config.origin_class_override.clone();
    provenance.source_provenance = kitchen.config.source_provenance_override.clone();

    if let Some(evidence) = &kitchen.config.hermetic_evidence {
        if !kitchen.config.pristine_mode {
            return Err(Error::ConfigError(
                "hermetic evidence requires pristine mode before build execution".to_string(),
            ));
        }
        provenance.hermetic_evidence = Some(evidence.clone());
        provenance.hardening_level_override = Some("hermetic".to_string());
    }

    Ok(())
}

fn validate_command_local_reproducibility_env(
    config: &ReproducibilityConfig,
    phase: &str,
    command: &str,
) -> Result<()> {
    validate_shell_env_mutations(config, phase, command)?;

    for invocation in extract_invocations_from_shell_text(phase, command, Some(phase)) {
        for fact in invocation.environment {
            if !ReproducibilityConfig::controlled_env_keys().contains(&fact.name.as_str()) {
                continue;
            }
            let value = fact.value.as_deref().unwrap_or_default();
            if !config.command_local_assignment_allowed(&fact.name, value) {
                return Err(Error::ConfigError(format!(
                    "hermetic reproducibility rejects command-local {} assignment in {} phase",
                    fact.name, phase
                )));
            }
        }
    }

    Ok(())
}

fn validate_shell_env_mutations(
    config: &ReproducibilityConfig,
    phase: &str,
    command: &str,
) -> Result<()> {
    for line in command.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        validate_no_command_substitution(phase, line)?;
        for segment in split_shell_env_segments(line) {
            validate_shell_env_mutation_segment(config, phase, &segment)?;
        }
    }

    Ok(())
}

fn validate_shell_env_mutation_segment(
    config: &ReproducibilityConfig,
    phase: &str,
    segment: &str,
) -> Result<()> {
    let tokens: Vec<String> = segment.split_whitespace().map(clean_shell_token).collect();
    let mut index = 0;

    loop {
        index = validate_leading_shell_assignments(config, phase, &tokens, index)?;
        index = peel_shell_env_wrappers(phase, &tokens, index)?;
        let Some(command_token) = tokens.get(index).map(String::as_str) else {
            return Ok(());
        };
        validate_no_shell_expansion(phase, command_token, "command")?;

        match command_basename(command_token) {
            "export" | "readonly" => {
                return validate_export_env_mutations(config, phase, &tokens[index + 1..]);
            }
            "declare" | "typeset" | "local" => {
                return validate_declare_env_mutations(
                    config,
                    phase,
                    command_basename(command_token),
                    &tokens[index + 1..],
                );
            }
            "read" => return validate_read_env_mutations(phase, &tokens[index + 1..]),
            "mapfile" | "readarray" => {
                return validate_mapfile_env_mutations(
                    phase,
                    command_basename(command_token),
                    &tokens[index + 1..],
                );
            }
            "printf" => return validate_printf_env_mutations(phase, &tokens[index + 1..]),
            "let" => return validate_let_env_mutations(phase, &tokens[index + 1..]),
            "getopts" => return validate_getopts_env_mutations(phase, &tokens[index + 1..]),
            "set" => return validate_set_env_mutations(phase, &tokens[index + 1..]),
            "alias" | "unalias" | "shopt" => {
                return Err(shell_alias_expansion_error(
                    phase,
                    command_basename(command_token),
                ));
            }
            "eval" | "source" | "." | "trap" => {
                return Err(Error::ConfigError(format!(
                    "hermetic reproducibility does not support {command_token} in {phase} phase"
                )));
            }
            "unset" => return validate_unset_env_mutations(phase, &tokens[index + 1..]),
            "env" => {
                return validate_env_wrapper_mutations(config, phase, &tokens[index + 1..]);
            }
            "make" | "gmake" => {
                return validate_make_command_args(
                    phase,
                    command_basename(command_token),
                    &tokens[index + 1..],
                );
            }
            _ => {}
        }
        if validate_shell_like_invocation(
            phase,
            command_basename(command_token),
            &tokens[index + 1..],
        )? {
            return Ok(());
        }

        if let Some(next_index) = peel_shell_control_word(phase, &tokens, index)? {
            index = next_index;
            continue;
        }

        return Ok(());
    }
}

fn validate_leading_shell_assignments(
    config: &ReproducibilityConfig,
    phase: &str,
    tokens: &[String],
    mut index: usize,
) -> Result<usize> {
    while let Some(token) = tokens.get(index) {
        if let Some((key, _)) = shell_append_assignment(token) {
            validate_shell_append_assignment(phase, &key)?;
            index += 1;
            continue;
        }
        let Some((key, value, is_array_target)) = shell_assignment(token) else {
            break;
        };
        validate_shell_assignment(config, phase, &key, &value, is_array_target)?;
        index += 1;
    }
    Ok(index)
}

fn peel_shell_env_wrappers(phase: &str, tokens: &[String], mut index: usize) -> Result<usize> {
    while let Some(command_token) = tokens.get(index).map(String::as_str) {
        match command_basename(command_token) {
            "command" => index = peel_command_wrapper(phase, tokens, index)?,
            "exec" => index = peel_exec_wrapper(phase, tokens, index)?,
            "builtin" => index = peel_builtin_wrapper(phase, tokens, index)?,
            _ => break,
        }
    }
    Ok(index)
}

fn is_shell_interpreter_command(command: &str) -> bool {
    matches!(
        command,
        "sh" | "bash" | "dash" | "zsh" | "ksh" | "mksh" | "ash"
    )
}

fn validate_shell_like_invocation(phase: &str, command: &str, args: &[String]) -> Result<bool> {
    if is_shell_interpreter_command(command) {
        validate_shell_interpreter_invocation(phase, command, args)?;
        return Ok(true);
    }
    if command == "busybox" {
        let Some(applet) = args.first().map(String::as_str).map(command_basename) else {
            return Ok(false);
        };
        if is_shell_interpreter_command(applet) {
            validate_shell_interpreter_invocation(phase, applet, &args[1..])?;
            return Ok(true);
        }
    }
    Ok(false)
}

fn validate_shell_interpreter_invocation(phase: &str, shell: &str, args: &[String]) -> Result<()> {
    for arg in args {
        if arg == "--" {
            break;
        }
        if shell_option_invokes_command_string(arg) {
            return Err(Error::ConfigError(format!(
                "hermetic reproducibility rejects nested shell {shell} {arg} invocation in {phase} phase"
            )));
        }
    }
    Ok(())
}

fn shell_option_invokes_command_string(arg: &str) -> bool {
    arg.starts_with('-') && !arg.starts_with("--") && arg[1..].chars().any(|ch| ch == 'c')
}

fn peel_shell_control_word(phase: &str, tokens: &[String], index: usize) -> Result<Option<usize>> {
    let Some(token) = tokens.get(index).map(String::as_str) else {
        return Ok(None);
    };
    match token {
        "!" | "if" | "while" | "until" | "then" | "do" | "else" | "elif" | "{" => {
            Ok(Some(index + 1))
        }
        "time" => Ok(Some(peel_time_control_word(phase, tokens, index)?)),
        "for" | "case" | "select" | "function" => Err(Error::ConfigError(format!(
            "hermetic reproducibility does not support shell control word {token} in {phase} phase"
        ))),
        _ => Ok(None),
    }
}

fn peel_time_control_word(phase: &str, tokens: &[String], index: usize) -> Result<usize> {
    let next_index = index + 1;
    match tokens.get(next_index).map(String::as_str) {
        Some("-p") => Ok(next_index + 1),
        Some(token) if token.starts_with('-') => Err(Error::ConfigError(format!(
            "hermetic reproducibility does not support time option {token} in {phase} phase"
        ))),
        _ => Ok(next_index),
    }
}

fn peel_command_wrapper(phase: &str, tokens: &[String], index: usize) -> Result<usize> {
    let mut next_index = index + 1;
    while let Some(token) = tokens.get(next_index).map(String::as_str) {
        if token == "--" {
            return Ok(next_index + 1);
        }
        if token == "-p" {
            next_index += 1;
            continue;
        }
        if token.starts_with('-') {
            return Err(Error::ConfigError(format!(
                "hermetic reproducibility does not support command option {token} in {phase} phase"
            )));
        }
        break;
    }
    Ok(next_index)
}

fn peel_exec_wrapper(phase: &str, tokens: &[String], index: usize) -> Result<usize> {
    let mut next_index = index + 1;
    while let Some(token) = tokens.get(next_index).map(String::as_str) {
        if token == "--" {
            return Ok(next_index + 1);
        }
        if token == "-c" || is_combined_exec_clear_option(token) {
            return Err(command_local_env_clear_error(phase));
        }
        if token == "-a" {
            shell_wrapper_operand(phase, tokens, next_index, "exec", token)?;
            next_index += 2;
            continue;
        }
        if token == "-l" {
            next_index += 1;
            continue;
        }
        if token.starts_with('-') {
            return Err(Error::ConfigError(format!(
                "hermetic reproducibility does not support exec option {token} in {phase} phase"
            )));
        }
        break;
    }
    Ok(next_index)
}

fn peel_builtin_wrapper(phase: &str, tokens: &[String], index: usize) -> Result<usize> {
    let next_index = index + 1;
    match tokens.get(next_index).map(String::as_str) {
        Some("--") => Ok(next_index + 1),
        Some(token) if token.starts_with('-') => Err(Error::ConfigError(format!(
            "hermetic reproducibility does not support builtin option {token} in {phase} phase"
        ))),
        _ => Ok(next_index),
    }
}

fn is_combined_exec_clear_option(token: &str) -> bool {
    token.starts_with('-') && !token.starts_with("--") && token[1..].chars().any(|ch| ch == 'c')
}

fn shell_wrapper_operand<'a>(
    phase: &str,
    tokens: &'a [String],
    index: usize,
    wrapper: &str,
    option: &str,
) -> Result<&'a str> {
    let operand = tokens.get(index + 1).map(String::as_str).ok_or_else(|| {
        Error::ConfigError(format!(
            "hermetic reproducibility rejects {wrapper} {option} without an operand in {phase} phase"
        ))
    })?;
    validate_no_shell_expansion(phase, operand, &format!("{wrapper} {option} operand"))?;
    Ok(operand)
}

fn validate_export_env_mutations(
    config: &ReproducibilityConfig,
    phase: &str,
    tokens: &[String],
) -> Result<()> {
    for token in tokens {
        validate_no_shell_expansion(phase, token, "export")?;
        if token.starts_with('-') {
            continue;
        }
        if let Some((key, _)) = shell_append_assignment(token) {
            validate_shell_append_assignment(phase, &key)?;
            continue;
        }
        if let Some((key, value, is_array_target)) = shell_assignment(token) {
            validate_shell_assignment(config, phase, &key, &value, is_array_target)?;
            continue;
        }
        if let Some(key) = controlled_reproducibility_target(token) {
            return Err(command_local_env_error(phase, key));
        }
    }
    Ok(())
}

fn validate_declare_env_mutations(
    config: &ReproducibilityConfig,
    phase: &str,
    builtin: &str,
    tokens: &[String],
) -> Result<()> {
    for token in tokens {
        validate_no_shell_expansion(phase, token, builtin)?;
        if declare_option_enables_nameref(token) {
            return Err(Error::ConfigError(format!(
                "hermetic reproducibility does not support {builtin} nameref option {token} in {phase} phase"
            )));
        }
        if token.starts_with('-') || token.starts_with('+') {
            continue;
        }
        if let Some((key, _)) = shell_append_assignment(token) {
            validate_shell_append_assignment(phase, &key)?;
            continue;
        }
        if let Some((key, value, is_array_target)) = shell_assignment(token) {
            validate_shell_assignment(config, phase, &key, &value, is_array_target)?;
            continue;
        }
        if let Some(key) = controlled_reproducibility_target(token) {
            return Err(command_local_env_error(phase, key));
        }
    }
    Ok(())
}

fn declare_option_enables_nameref(token: &str) -> bool {
    if token == "--" || token.starts_with("--") {
        return false;
    }
    let Some(options) = token.strip_prefix('-').or_else(|| token.strip_prefix('+')) else {
        return false;
    };
    options.chars().any(|option| option == 'n')
}

fn validate_unset_env_mutations(phase: &str, tokens: &[String]) -> Result<()> {
    for token in tokens {
        validate_no_shell_expansion(phase, token, "unset")?;
        if token.starts_with('-') {
            continue;
        }
        if let Some(key) = controlled_reproducibility_target(token) {
            return Err(command_local_env_error(phase, key));
        }
    }
    Ok(())
}

fn validate_read_env_mutations(phase: &str, tokens: &[String]) -> Result<()> {
    let mut index = 0;
    while let Some(token) = tokens.get(index).map(String::as_str) {
        validate_no_shell_expansion(phase, token, "read")?;
        if token == "--" {
            index += 1;
            break;
        }
        if !token.starts_with('-') {
            break;
        }
        match token {
            "-e" | "-r" | "-s" => index += 1,
            "-a" => {
                let key = shell_wrapper_operand(phase, tokens, index, "read", token)?;
                validate_no_shell_expansion(phase, key, "read")?;
                if let Some(key) = controlled_reproducibility_target(key) {
                    return Err(command_local_env_error(phase, key));
                }
                index += 2;
            }
            "-d" | "-i" | "-n" | "-N" | "-p" | "-t" | "-u" => {
                shell_wrapper_operand(phase, tokens, index, "read", token)?;
                index += 2;
            }
            _ => {
                return Err(Error::ConfigError(format!(
                    "hermetic reproducibility does not support read option {token} in {phase} phase"
                )));
            }
        }
    }

    for token in &tokens[index..] {
        validate_no_shell_expansion(phase, token, "read")?;
        if token.starts_with('<') {
            return Err(Error::ConfigError(format!(
                "hermetic reproducibility does not support read redirection in {phase} phase"
            )));
        }
        if let Some(key) = controlled_reproducibility_target(token) {
            return Err(command_local_env_error(phase, key));
        }
    }
    Ok(())
}

fn validate_mapfile_env_mutations(phase: &str, builtin: &str, tokens: &[String]) -> Result<()> {
    let mut index = 0;
    while let Some(token) = tokens.get(index).map(String::as_str) {
        validate_no_shell_expansion(phase, token, builtin)?;
        if token == "--" {
            index += 1;
            break;
        }
        if token.starts_with('<') {
            return Err(Error::ConfigError(format!(
                "hermetic reproducibility does not support {builtin} redirection in {phase} phase"
            )));
        }
        if !token.starts_with('-') {
            break;
        }
        match token {
            "-t" => index += 1,
            "-C" | "-c" | "-d" | "-n" | "-O" | "-s" | "-u" => {
                shell_wrapper_operand(phase, tokens, index, builtin, token)?;
                index += 2;
            }
            _ => {
                return Err(Error::ConfigError(format!(
                    "hermetic reproducibility does not support {builtin} option {token} in {phase} phase"
                )));
            }
        }
    }

    let Some(key) = tokens.get(index).map(String::as_str) else {
        return Ok(());
    };
    validate_no_shell_expansion(phase, key, builtin)?;
    if key.starts_with('<') {
        return Err(Error::ConfigError(format!(
            "hermetic reproducibility does not support {builtin} redirection in {phase} phase"
        )));
    }
    if let Some(key) = controlled_reproducibility_target(key) {
        return Err(command_local_env_error(phase, key));
    }
    Ok(())
}

fn validate_printf_env_mutations(phase: &str, tokens: &[String]) -> Result<()> {
    let mut index = 0;
    while let Some(token) = tokens.get(index).map(String::as_str) {
        validate_no_shell_expansion(phase, token, "printf")?;
        if token == "--" {
            return Ok(());
        }
        if token == "-v" {
            let key = shell_wrapper_operand(phase, tokens, index, "printf", token)?;
            validate_no_shell_expansion(phase, key, "printf")?;
            if let Some(key) = controlled_reproducibility_target(key) {
                return Err(command_local_env_error(phase, key));
            }
            index += 2;
            continue;
        }
        if let Some(key) = token.strip_prefix("-v").filter(|key| !key.is_empty()) {
            validate_no_shell_expansion(phase, key, "printf")?;
            if let Some(key) = controlled_reproducibility_target(key) {
                return Err(command_local_env_error(phase, key));
            }
            index += 1;
            continue;
        }
        if token.starts_with('-') {
            return Err(Error::ConfigError(format!(
                "hermetic reproducibility does not support printf option {token} in {phase} phase"
            )));
        }
        break;
    }
    Ok(())
}

fn validate_let_env_mutations(phase: &str, tokens: &[String]) -> Result<()> {
    for token in tokens {
        validate_no_shell_expansion(phase, token, "let")?;
        if let Some(key) = controlled_key_mentioned_in_expression(token) {
            return Err(command_local_env_error(phase, key));
        }
    }
    Ok(())
}

fn validate_getopts_env_mutations(phase: &str, tokens: &[String]) -> Result<()> {
    if let Some(key) = tokens.get(1).map(String::as_str) {
        validate_no_shell_expansion(phase, key, "getopts")?;
        if let Some(key) = controlled_reproducibility_target(key) {
            return Err(command_local_env_error(phase, key));
        }
    }
    Ok(())
}

fn validate_set_env_mutations(phase: &str, tokens: &[String]) -> Result<()> {
    let mut index = 0;
    while let Some(token) = tokens.get(index).map(String::as_str) {
        if token == "--" {
            return Ok(());
        }
        if token == "-o" {
            let option = shell_wrapper_operand(phase, tokens, index, "set", token)?;
            if option == "keyword" {
                return Err(shell_keyword_mode_error(phase, "set -o keyword"));
            }
            index += 2;
            continue;
        }
        if token.starts_with('-') && !token.starts_with("--") {
            if token[1..].chars().any(|option| option == 'k') {
                return Err(shell_keyword_mode_error(phase, token));
            }
            index += 1;
            continue;
        }
        if token.starts_with("--") {
            return Err(Error::ConfigError(format!(
                "hermetic reproducibility does not support set option {token} in {phase} phase"
            )));
        }
        break;
    }
    Ok(())
}

fn validate_env_wrapper_mutations(
    config: &ReproducibilityConfig,
    phase: &str,
    tokens: &[String],
) -> Result<()> {
    let mut index = 0;
    while let Some(token) = tokens.get(index) {
        validate_no_shell_expansion(phase, token, "env")?;
        if let Some(next_index) = validate_env_option(phase, tokens, index)? {
            index = next_index;
            continue;
        }

        if let Some((key, _)) = token.split_once('=') {
            if ReproducibilityConfig::is_forbidden_shell_environment_key(key) {
                return Err(command_local_env_error(phase, key));
            }
            ReproducibilityConfig::validate_make_environment_value(key, &token[key.len() + 1..])?;
        }

        if let Some((key, _)) = shell_append_assignment(token) {
            validate_shell_append_assignment(phase, &key)?;
            index += 1;
            continue;
        }
        let Some((key, value, is_array_target)) = shell_assignment(token) else {
            break;
        };
        validate_shell_assignment(config, phase, &key, &value, is_array_target)?;
        index += 1;
    }

    let Some(command_token) = tokens.get(index).map(String::as_str) else {
        return Ok(());
    };
    let command = command_basename(command_token);
    if command == "env" {
        return validate_env_wrapper_mutations(config, phase, &tokens[index + 1..]);
    }
    if is_make_command(command) {
        return validate_make_command_args(phase, command, &tokens[index + 1..]);
    }
    if validate_shell_like_invocation(phase, command, &tokens[index + 1..])? {
        return Ok(());
    }

    Ok(())
}

fn validate_env_option(phase: &str, tokens: &[String], index: usize) -> Result<Option<usize>> {
    let token = &tokens[index];
    if token == "--" {
        return Ok(Some(index + 1));
    }
    if token == "-" || token == "--ignore-environment" {
        return Err(command_local_env_clear_error(phase));
    }
    if let Some(key) = token.strip_prefix("--unset=") {
        validate_env_unset_key(phase, key)?;
        return Ok(Some(index + 1));
    }
    if token == "--unset" || token == "-u" {
        let key = env_option_operand(phase, tokens, index, token)?;
        validate_env_unset_key(phase, key)?;
        return Ok(Some(index + 2));
    }
    if token == "--debug" {
        return Ok(Some(index + 1));
    }
    if token == "-S" || token.starts_with("-S") {
        return Err(Error::ConfigError(format!(
            "hermetic reproducibility does not support env -S/--split-string in {phase} phase"
        )));
    }
    if token == "--split-string" || token.starts_with("--split-string=") {
        return Err(Error::ConfigError(format!(
            "hermetic reproducibility does not support env --split-string in {phase} phase"
        )));
    }
    if token == "-C" || token == "-a" {
        env_option_operand(phase, tokens, index, token)?;
        return Ok(Some(index + 2));
    }
    for option in ["--chdir", "--argv0"] {
        if token == option {
            env_option_operand(phase, tokens, index, token)?;
            return Ok(Some(index + 2));
        }
        if let Some(operand) = token
            .strip_prefix(option)
            .and_then(|rest| rest.strip_prefix('='))
        {
            validate_no_shell_expansion(phase, operand, token)?;
            return Ok(Some(index + 1));
        }
    }
    if !token.starts_with('-') {
        return Ok(None);
    }
    if token.starts_with("--") {
        return Err(Error::ConfigError(format!(
            "hermetic reproducibility does not support env option {token} in {phase} phase"
        )));
    }

    let mut chars = token[1..].char_indices().peekable();
    while let Some((offset, option)) = chars.next() {
        match option {
            'i' => return Err(command_local_env_clear_error(phase)),
            'u' => {
                let key_start = offset + option.len_utf8() + 1;
                let key = if key_start < token.len() {
                    &token[key_start..]
                } else {
                    tokens.get(index + 1).map(String::as_str).ok_or_else(|| {
                        Error::ConfigError(format!(
                            "hermetic reproducibility rejects env -u without a key in {phase} phase"
                        ))
                    })?
                };
                validate_env_unset_key(phase, key)?;
                let next_index = if key_start < token.len() {
                    index + 1
                } else {
                    index + 2
                };
                return Ok(Some(next_index));
            }
            _ => {
                if chars.peek().is_none() {
                    return Err(Error::ConfigError(format!(
                        "hermetic reproducibility does not support env option -{option} in {phase} phase"
                    )));
                }
            }
        }
    }

    Ok(None)
}

fn env_option_operand<'a>(
    phase: &str,
    tokens: &'a [String],
    index: usize,
    option: &str,
) -> Result<&'a str> {
    let operand = tokens.get(index + 1).map(String::as_str).ok_or_else(|| {
        Error::ConfigError(format!(
            "hermetic reproducibility rejects env {option} without an operand in {phase} phase"
        ))
    })?;
    validate_no_shell_expansion(phase, operand, &format!("env {option} operand"))?;
    Ok(operand)
}

fn validate_env_unset_key(phase: &str, key: &str) -> Result<()> {
    validate_no_shell_expansion(phase, key, "env unset")?;
    if is_controlled_reproducibility_key(key) {
        return Err(command_local_env_error(phase, key));
    }
    Ok(())
}

fn validate_shell_assignment(
    config: &ReproducibilityConfig,
    phase: &str,
    key: &str,
    value: &str,
    is_array_target: bool,
) -> Result<()> {
    ReproducibilityConfig::validate_make_environment_value(key, value)?;
    if !is_controlled_reproducibility_key(key) {
        return Ok(());
    }
    if is_array_target {
        return Err(command_local_env_error(phase, key));
    }
    if config.command_local_assignment_allowed(key, value) {
        return Ok(());
    }
    Err(command_local_env_error(phase, key))
}

fn validate_shell_append_assignment(phase: &str, key: &str) -> Result<()> {
    if ReproducibilityConfig::is_make_environment_key(key) {
        return Err(command_local_env_error(phase, key));
    }
    if is_controlled_reproducibility_key(key) {
        return Err(command_local_env_error(phase, key));
    }
    Ok(())
}

fn validate_make_command_args(phase: &str, make: &str, args: &[String]) -> Result<()> {
    for token in args {
        validate_no_shell_expansion(phase, token, make)?;
        if ReproducibilityConfig::is_make_eval_option(token) {
            return Err(Error::ConfigError(format!(
                "hermetic reproducibility does not support {make} eval option {token} in {phase} phase"
            )));
        }
        if ReproducibilityConfig::is_makefile_import_option(token) {
            return Err(Error::ConfigError(format!(
                "hermetic reproducibility does not support {make} makefile import option {token} in {phase} phase"
            )));
        }
        if let Some(key) = ReproducibilityConfig::controlled_make_assignment_key(token) {
            return Err(command_local_env_error(phase, key));
        }
    }
    Ok(())
}

fn validate_no_shell_expansion(phase: &str, token: &str, context: &str) -> Result<()> {
    if has_dynamic_shell_expansion(token) {
        return Err(Error::ConfigError(format!(
            "hermetic reproducibility does not support dynamic shell expansion in {context} token {token} in {phase} phase"
        )));
    }
    Ok(())
}

fn has_dynamic_shell_expansion(token: &str) -> bool {
    for ch in token.chars() {
        if matches!(ch, '$' | '{' | '}' | '*' | '?' | '[' | '\\' | '"' | '\'') {
            return true;
        }
    }
    false
}

fn validate_no_command_substitution(phase: &str, line: &str) -> Result<()> {
    if line.contains("$(") || line.contains('`') {
        return Err(Error::ConfigError(format!(
            "hermetic reproducibility does not support command substitution in {phase} phase"
        )));
    }
    if line.contains("<(") || line.contains(">(") {
        return Err(Error::ConfigError(format!(
            "hermetic reproducibility does not support process substitution in {phase} phase"
        )));
    }
    Ok(())
}

fn is_make_command(command: &str) -> bool {
    matches!(command, "make" | "gmake")
}

fn command_local_env_error(phase: &str, key: &str) -> Error {
    Error::ConfigError(format!(
        "hermetic reproducibility rejects command-local {key} assignment in {phase} phase"
    ))
}

fn command_local_env_clear_error(phase: &str) -> Error {
    Error::ConfigError(format!(
        "hermetic reproducibility rejects command-local environment clearing in {phase} phase"
    ))
}

fn shell_keyword_mode_error(phase: &str, option: &str) -> Error {
    Error::ConfigError(format!(
        "hermetic reproducibility rejects shell keyword-mode {option} in {phase} phase"
    ))
}

fn shell_alias_expansion_error(phase: &str, surface: &str) -> Error {
    Error::ConfigError(format!(
        "hermetic reproducibility rejects shell alias expansion surface {surface} in {phase} phase"
    ))
}

fn is_controlled_reproducibility_key(key: &str) -> bool {
    controlled_reproducibility_target(key).is_some()
}

fn controlled_reproducibility_target(target: &str) -> Option<&'static str> {
    let (base, _) = shell_variable_base(target)?;
    ReproducibilityConfig::controlled_env_keys()
        .iter()
        .copied()
        .find(|key| *key == base)
}

fn controlled_key_mentioned_in_expression(expression: &str) -> Option<&'static str> {
    ReproducibilityConfig::controlled_env_keys()
        .iter()
        .copied()
        .find(|key| shell_identifier_present(expression, key))
}

fn shell_identifier_present(expression: &str, name: &str) -> bool {
    let mut start = None;
    for (index, ch) in expression.char_indices() {
        if is_shell_identifier_char(ch) {
            start.get_or_insert(index);
            continue;
        }
        if let Some(identifier_start) = start.take() {
            if &expression[identifier_start..index] == name {
                return true;
            }
        }
    }
    start
        .map(|identifier_start| &expression[identifier_start..] == name)
        .unwrap_or(false)
}

fn is_shell_identifier_char(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

fn shell_assignment(token: &str) -> Option<(String, String, bool)> {
    let (key, value) = token.split_once('=')?;
    let (key, is_array_target) = shell_variable_base(key)?;
    Some((key.to_string(), value.to_string(), is_array_target))
}

fn shell_append_assignment(token: &str) -> Option<(String, bool)> {
    let (key, _) = token.split_once("+=")?;
    let (key, is_array_target) = shell_variable_base(key)?;
    Some((key.to_string(), is_array_target))
}

fn shell_variable_base(target: &str) -> Option<(&str, bool)> {
    if is_shell_env_name(target) {
        return Some((target, false));
    }
    let (base, subscript) = target.split_once('[')?;
    if subscript.ends_with(']') && is_shell_env_name(base) {
        return Some((base, true));
    }
    None
}

fn is_shell_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return false;
    }
    chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn command_basename(command: &str) -> &str {
    command.rsplit('/').next().unwrap_or(command)
}

fn clean_shell_token(token: &str) -> String {
    token
        .trim_matches(|ch: char| matches!(ch, '"' | '\''))
        .to_string()
}

fn split_shell_env_segments(line: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut chars = line.chars().peekable();
    let mut quote = None;
    let mut escaped = false;

    while let Some(ch) = chars.next() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' {
            current.push(ch);
            escaped = true;
            continue;
        }
        if let Some(quote_ch) = quote {
            current.push(ch);
            if ch == quote_ch {
                quote = None;
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            current.push(ch);
            quote = Some(ch);
            continue;
        }

        match ch {
            '&' if chars.peek() == Some(&'&') => {
                chars.next();
                push_shell_env_segment(&mut segments, &mut current);
            }
            '|' => {
                if chars.peek() == Some(&'|') {
                    chars.next();
                }
                push_shell_env_segment(&mut segments, &mut current);
            }
            ';' | '(' | ')' | '`' => push_shell_env_segment(&mut segments, &mut current),
            '$' if chars.peek() == Some(&'(') => {
                chars.next();
                push_shell_env_segment(&mut segments, &mut current);
            }
            _ => current.push(ch),
        }
    }

    push_shell_env_segment(&mut segments, &mut current);
    segments
}

fn push_shell_env_segment(segments: &mut Vec<String>, current: &mut String) {
    let segment = current.trim();
    if !segment.is_empty() {
        segments.push(segment.to_string());
    }
    current.clear();
}

/// A single cook operation
pub struct Cook<'a> {
    pub(super) kitchen: &'a Kitchen,
    pub(super) recipe: &'a Recipe,
    /// Owner of the temporary build directory (None when an external dest_dir is provided)
    pub(super) _build_dir_owner: Option<TempDir>,
    /// Build directory path
    pub(super) build_dir: PathBuf,
    /// Source directory within build_dir
    pub(crate) source_dir: PathBuf,
    /// Destination directory (where files get installed)
    pub(super) dest_dir: PathBuf,
    /// Build log accumulator
    pub(super) log: String,
    /// Warnings
    pub(super) warnings: Vec<String>,
    /// Provenance capture for this build
    pub(super) provenance: ProvenanceCapture,
}

impl<'a> Cook<'a> {
    pub(super) fn new(kitchen: &'a Kitchen, recipe: &'a Recipe) -> Result<Self> {
        let build_dir = TempDir::new()
            .map_err(|e| Error::IoError(format!("Failed to create build directory: {}", e)))?;

        let build_path = build_dir.path().to_path_buf();
        let source_dir = build_path.join("source");
        let dest_dir = build_path.join("destdir");

        fs::create_dir_all(&source_dir)?;
        fs::create_dir_all(&dest_dir)?;

        let mut provenance = ProvenanceCapture::new();
        configure_provenance_from_kitchen(kitchen, &mut provenance)?;

        // Record build dependencies from recipe
        for dep in &recipe.build.makedepends {
            // TODO: Look up actual versions from installed packages
            provenance.add_build_dep(dep, "unknown", None);
        }

        Ok(Self {
            kitchen,
            recipe,
            _build_dir_owner: Some(build_dir),
            build_dir: build_path,
            source_dir,
            dest_dir,
            log: String::new(),
            warnings: Vec::new(),
            provenance,
        })
    }

    /// Create a Cook with a caller-provided destination directory.
    ///
    /// Used by bootstrap builds where files install directly to $LFS
    /// instead of a temporary staging area.
    pub(crate) fn new_with_dest(
        kitchen: &'a Kitchen,
        recipe: &'a Recipe,
        dest_dir: &Path,
    ) -> Result<Self> {
        let build_dir = if let Some(sysroot) = &kitchen.config.sysroot {
            let parent = sysroot.join("var/tmp/conary-derivation-build");
            fs::create_dir_all(&parent)?;
            TempDir::new_in(&parent).map_err(|e| {
                Error::IoError(format!(
                    "Failed to create build directory in {}: {}",
                    parent.display(),
                    e
                ))
            })?
        } else {
            TempDir::new()
                .map_err(|e| Error::IoError(format!("Failed to create build directory: {}", e)))?
        };
        let build_path = build_dir.path().to_path_buf();
        let source_dir = build_path.join("source");

        fs::create_dir_all(&source_dir)?;
        fs::create_dir_all(dest_dir)?;

        let mut provenance = ProvenanceCapture::new();
        configure_provenance_from_kitchen(kitchen, &mut provenance)?;
        for dep in &recipe.build.makedepends {
            provenance.add_build_dep(dep, "unknown", None);
        }

        Ok(Self {
            kitchen,
            recipe,
            _build_dir_owner: Some(build_dir),
            build_dir: build_path,
            source_dir,
            dest_dir: dest_dir.to_path_buf(),
            log: String::new(),
            warnings: Vec::new(),
            provenance,
        })
    }

    /// Access the accumulated build log.
    pub(crate) fn build_log(&self) -> &str {
        &self.log
    }

    /// Phase 1: Prep - fetch all sources
    pub(crate) fn prep(&mut self) -> Result<()> {
        let source = match &self.recipe.source {
            SourceSection::Remote(source) => source,
            SourceSection::Local(source) => {
                let resolved = self.kitchen.resolve_local_source(source)?;
                let metadata = fs::metadata(&resolved).map_err(|e| {
                    Error::NotFound(format!(
                        "Local source path not found: {} ({e})",
                        resolved.display()
                    ))
                })?;
                if !metadata.is_dir() {
                    return Err(Error::ConfigError(format!(
                        "Local source path must be a directory: {}",
                        resolved.display()
                    )));
                }

                if self.provenance.source_provenance.is_none() {
                    self.provenance.upstream_url =
                        Some(format!("local:{}", source.path.to_string_lossy()));
                    self.provenance.upstream_hash = None;
                }

                if !self.kitchen.config.use_isolation {
                    self.source_dir = resolved;
                    self.log_line(&format!(
                        "Using local source: {}",
                        self.source_dir.display()
                    ));
                    return Ok(());
                }

                if let Some(files) = self.kitchen.config.hermetic_local_files.as_deref() {
                    materialize_local_source_from_file_list(&resolved, &self.source_dir, files)?;
                } else {
                    copy_dir_contents(&resolved, &resolved, &self.source_dir)?;
                }
                self.log_line(&format!("Prepared local source: {}", resolved.display()));
                return Ok(());
            }
        };

        // Fetch main source archive
        let archive_url = self.recipe.archive_url();
        let archive_path = self.kitchen.fetch_source(&archive_url, &source.checksum)?;

        // Record source fetch for provenance
        self.provenance
            .record_source_fetch(&archive_url, &source.checksum);

        // Copy to build directory
        let local_archive = self
            .build_dir
            .as_path()
            .join(self.recipe.archive_filename());
        fs::copy(&archive_path, &local_archive)?;

        self.log_line(&format!("Fetched source: {}", archive_url));

        // Fetch additional sources (with variable substitution)
        for additional in &source.additional {
            let url = self.recipe.substitute(&additional.url, "");
            let path = self.kitchen.fetch_source(&url, &additional.checksum)?;
            let filename = url.split('/').next_back().unwrap_or("additional.tar.gz");
            let local_path = self.source_dir.join(filename);
            fs::copy(&path, &local_path)?;
            self.log_line(&format!("Fetched additional source: {}", url));
        }

        // Fetch patches -- all remote patches MUST have checksums
        if let Some(patches) = &self.recipe.patches {
            for patch in &patches.files {
                let patch_file = self.recipe.substitute(&patch.file, "");
                if is_remote_url(&patch_file) {
                    let filename = patch_file.split('/').next_back().unwrap_or("patch.diff");
                    let local_path = self.build_dir.as_path().join("patches").join(filename);
                    fs::create_dir_all(local_path.parent().unwrap())?;

                    let checksum = patch.checksum.as_ref().ok_or_else(|| {
                        Error::ConfigError(format!(
                            "Remote patch '{}' has no checksum. \
                             All remote patches must include a sha256 checksum \
                             to prevent MITM or compromised-server attacks. \
                             Add a 'checksum' field to the patch entry in your recipe.",
                            patch.file
                        ))
                    })?;
                    let path = self.kitchen.fetch_source(&patch_file, checksum)?;
                    fs::copy(&path, &local_path)?;

                    self.log_line(&format!("Fetched patch: {}", patch_file));
                }
            }
        }

        Ok(())
    }

    /// Phase 2a: Unpack sources
    pub(crate) fn unpack(&mut self) -> Result<()> {
        let source = match &self.recipe.source {
            SourceSection::Remote(source) => source,
            SourceSection::Local(_) => {
                self.log_line(&format!(
                    "Using local source at {}",
                    self.source_dir.display()
                ));
                return Ok(());
            }
        };

        // Remember the staging dir where prep() placed additional archives.
        // source_dir may be rewritten below (single top-level dir detection),
        // but the staged files live in the original location.
        let staging_dir = self.source_dir.clone();

        let archive_path = self
            .build_dir
            .as_path()
            .join(self.recipe.archive_filename());

        // Detect archive type and extract
        extract_archive(&archive_path, &self.source_dir)?;
        self.log_line(&format!(
            "Extracted source to {}",
            self.source_dir.display()
        ));

        // Find the actual source directory (often archives have a top-level dir).
        // Only count directories — additional source tarballs placed here by prep()
        // should not interfere with the single-directory detection.
        let dir_entries: Vec<_> = fs::read_dir(&self.source_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .collect();

        if dir_entries.len() == 1 {
            // Single directory - this is the actual source
            self.source_dir = dir_entries[0].path();
            debug!("Source directory: {}", self.source_dir.display());
        }

        // Override with explicit extract_dir if specified
        if let Some(extract_dir) = &source.extract_dir {
            self.source_dir = self.build_dir.as_path().join("source").join(extract_dir);
        }

        // Extract additional source archives, honoring extract_to.
        // Archives were staged by prep() into the original staging_dir,
        // not the (possibly rewritten) source_dir.
        // Use the same substitution as prep() so templated filenames match.
        for additional in &source.additional {
            let substituted_url = self.recipe.substitute(&additional.url, "");
            let filename = substituted_url
                .split('/')
                .next_back()
                .unwrap_or("additional.tar.gz");
            let additional_archive = staging_dir.join(filename);

            if additional.extract && additional_archive.exists() {
                let dest = if let Some(extract_to) = &additional.extract_to {
                    let target = self.source_dir.join(extract_to);
                    fs::create_dir_all(&target)?;
                    target
                } else {
                    self.source_dir.clone()
                };

                extract_archive(&additional_archive, &dest)?;
                self.log_line(&format!(
                    "Extracted additional source {} to {}",
                    filename,
                    dest.display()
                ));
            }
        }

        Ok(())
    }

    /// Phase 2b: Apply patches
    pub(crate) fn patch(&mut self) -> Result<()> {
        let patches = match &self.recipe.patches {
            Some(p) => &p.files,
            None => return Ok(()),
        };

        for patch_info in patches {
            let patch_file = self.recipe.substitute(&patch_info.file, "");
            let patch_path = if is_remote_url(&patch_file) {
                let filename = patch_file.split('/').next_back().unwrap_or("patch.diff");
                self.build_dir.as_path().join("patches").join(filename)
            } else {
                resolve_local_patch_path(
                    self.kitchen.config.recipe_source_base_dir.as_deref(),
                    &patch_file,
                    self.kitchen.config.hermetic_evidence.is_some(),
                )?
            };

            if !patch_path.exists() {
                return Err(Error::NotFound(format!(
                    "Patch file not found: {}",
                    patch_path.display()
                )));
            }

            // Read patch content for provenance hashing
            let patch_content = fs::read(&patch_path).unwrap_or_default();

            info!("Applying patch: {}", patch_file);
            apply_patch(&self.source_dir, &patch_path, patch_info.strip)?;
            self.log_line(&format!("Applied patch: {}", patch_file));

            // Record patch for provenance
            self.provenance.record_patch(
                &patch_file,
                &patch_content,
                patch_info.strip,
                None, // Author not typically in recipe
                None, // Description not in current recipe format
            );
        }

        Ok(())
    }

    /// Phase 3: Simmer - run the build
    pub(crate) fn simmer(&mut self) -> Result<()> {
        // Mark build start for provenance
        self.provenance.start_build();
        self.provenance
            .record_isolation(self.kitchen.config.use_isolation);

        let build = &self.recipe.build;

        // Determine working directory
        let workdir = if let Some(wd) = &build.workdir {
            self.source_dir.join(wd)
        } else {
            self.source_dir.clone()
        };

        // Set up environment
        let mut env: Vec<(String, String)> = vec![
            (
                "DESTDIR".to_string(),
                self.dest_dir.to_string_lossy().to_string(),
            ),
            (
                "MAKEFLAGS".to_string(),
                format!("-j{}", build.jobs.unwrap_or(self.kitchen.config.jobs)),
            ),
        ];

        // Inject caller-supplied env vars (e.g. LFS, LFS_TGT, PATH for bootstrap
        // builds) without touching the process-wide environment.
        for (key, value) in &self.kitchen.config.extra_env {
            env.push((key.clone(), value.clone()));
        }

        for (key, value) in &build.environment {
            env.push((key.clone(), value.clone()));
        }

        // Run setup if specified
        if let Some(setup) = &build.setup {
            self.run_build_step("setup", setup, &workdir, &env)?;
        }

        // Run configure
        if let Some(configure) = &build.configure {
            let cmd = self
                .recipe
                .substitute(configure, &self.dest_dir.to_string_lossy());
            self.run_build_step("configure", &cmd, &workdir, &env)?;
        }

        // Run make
        if let Some(make) = &build.make {
            let cmd = self
                .recipe
                .substitute(make, &self.dest_dir.to_string_lossy());
            self.run_build_step("make", &cmd, &workdir, &env)?;
        }

        // Run check if specified
        if let Some(check) = &build.check {
            match self.run_build_step("check", check, &workdir, &env) {
                Ok(_) => {}
                Err(e) if self.hermetic_controls_active() => return Err(e),
                Err(e) => {
                    self.warnings.push(format!("Tests failed: {}", e));
                }
            }
        }

        // Run install
        if let Some(install) = &build.install {
            let cmd = self
                .recipe
                .substitute(install, &self.dest_dir.to_string_lossy());
            self.run_build_step("install", &cmd, &workdir, &env)?;
        }

        // Run post_install if specified
        if let Some(post_install) = &build.post_install {
            self.run_build_step("post_install", post_install, &workdir, &env)?;
        }

        Ok(())
    }

    /// Run a build step
    fn run_build_step(
        &mut self,
        phase: &str,
        command: &str,
        workdir: &Path,
        env: &[(String, String)],
    ) -> Result<()> {
        info!("Running {} phase", phase);
        debug!("Command: {}", command);

        let final_env;
        let env = if let Some(config) = self.reproducibility_config_for_execution() {
            final_env = config.merge_env(env.to_vec())?;
            config.validate_final_env(&final_env)?;
            validate_command_local_reproducibility_env(&config, phase, command)?;
            final_env.as_slice()
        } else {
            env
        };

        if self.kitchen.config.use_isolation {
            self.run_build_step_isolated(phase, command, workdir, env)
        } else {
            self.run_build_step_direct(phase, command, workdir, env)
        }
    }

    /// Run a build step with container isolation
    fn run_build_step_isolated(
        &mut self,
        phase: &str,
        command: &str,
        workdir: &Path,
        env: &[(String, String)],
    ) -> Result<()> {
        // Configure container based on pristine mode
        let mut container_config = if self.kitchen.config.pristine_mode {
            // Pristine mode: no host system mounts
            // This is critical for bootstrap builds to avoid toolchain contamination
            let config = if let Some(sysroot) = &self.kitchen.config.sysroot {
                ContainerConfig::pristine_for_bootstrap(
                    sysroot,
                    &self.source_dir,
                    self.build_dir.as_path(),
                    &self.dest_dir,
                )
            } else {
                ContainerConfig::pristine()
            };
            info!(
                "Using pristine container (no host mounts) for {} phase",
                phase
            );
            config
        } else {
            // Normal mode: mount host system directories
            ContainerConfig::default()
        };

        // Set resource limits from kitchen config
        container_config.memory_limit = self.kitchen.config.memory_limit;
        container_config.cpu_time_limit = self.kitchen.config.cpu_time_limit;
        container_config.timeout = self.kitchen.config.timeout;
        container_config.hostname = "conary-build".to_string();
        container_config.workdir = workdir.to_path_buf();

        // Network isolation is on by default - only allow if explicitly configured
        if self.kitchen.config.allow_network {
            container_config.allow_network();
        }

        // For non-pristine mode, set up bind mounts manually
        if !self.kitchen.config.pristine_mode {
            // Clear default mounts and add build-specific ones
            container_config.bind_mounts.clear();

            // Essential system directories (read-only)
            for path in &["/usr", "/lib", "/lib64", "/bin", "/sbin"] {
                if Path::new(path).exists() {
                    container_config
                        .bind_mounts
                        .push(BindMount::readonly(*path, *path));
                }
            }

            // Config files that build tools might need (no resolv.conf - network is isolated)
            for path in &["/etc/passwd", "/etc/group", "/etc/hosts"] {
                if Path::new(path).exists() {
                    container_config
                        .bind_mounts
                        .push(BindMount::readonly(*path, *path));
                }
            }

            // Only mount resolv.conf if network is allowed
            if self.kitchen.config.allow_network && Path::new("/etc/resolv.conf").exists() {
                container_config
                    .bind_mounts
                    .push(BindMount::readonly("/etc/resolv.conf", "/etc/resolv.conf"));
            }

            // Source directory (read-only - we shouldn't modify sources)
            container_config
                .bind_mounts
                .push(BindMount::readonly(&self.source_dir, &self.source_dir));

            // Destination directory (writable - where install goes)
            container_config
                .bind_mounts
                .push(BindMount::writable(&self.dest_dir, &self.dest_dir));

            // Build directory (writable - for build artifacts)
            container_config.bind_mounts.push(BindMount::writable(
                self.build_dir.as_path(),
                self.build_dir.as_path(),
            ));
        }

        let mut sandbox = Sandbox::new(container_config);

        // Convert env to the format expected by Sandbox
        let env_refs: Vec<(&str, &str)> =
            env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();

        // Shell-escape the workdir to prevent injection from paths with
        // spaces or special characters. Single-quote the path, escaping
        // any embedded single-quotes with '\'' .
        let workdir_str = workdir.to_string_lossy();
        let escaped_workdir = format!("'{}'", workdir_str.replace('\'', "'\\''"));
        let (exit_code, stdout, stderr) = sandbox.execute(
            "/bin/sh",
            &format!("cd {} && {}", escaped_workdir, command),
            &[],
            &env_refs,
        )?;

        self.log_build_output(phase, true, &stdout, &stderr);

        if exit_code != 0 {
            return Err(Error::IoError(format!(
                "{} phase failed with exit code {}\nstderr: {}",
                phase, exit_code, stderr
            )));
        }

        Ok(())
    }

    /// Run a build step directly (no isolation)
    fn run_build_step_direct(
        &mut self,
        phase: &str,
        command: &str,
        workdir: &Path,
        env: &[(String, String)],
    ) -> Result<()> {
        // When a sysroot is configured (bootstrap builds), run inside the
        // sysroot as a chroot. This matches LFS's build model: all packages
        // build inside the chroot where only self-built tools are visible.
        // Without chroot, the host gcc/glibc/headers are used, causing
        // compatibility issues (e.g., Python 3.14 + host GCC 15 -Werror).
        let output = if let Some(sysroot) = &self.kitchen.config.sysroot {
            // Convert workdir to be relative to the sysroot
            let chroot_workdir = translate_path_for_chroot(workdir, sysroot);

            // Build env string for chroot (env -i clears host env)
            let chroot_env = translate_env_for_chroot(env, sysroot);
            let env_args = chroot_env_args(
                &chroot_env,
                self.recipe.build.jobs.unwrap_or(self.kitchen.config.jobs),
            );
            let command = translate_command_for_chroot(command, sysroot);

            // Shell-escape the chroot workdir to prevent injection from
            // paths with spaces or special characters, matching the
            // escaping used in run_build_step_isolated.
            let workdir_str = chroot_workdir.to_string_lossy();
            let escaped_workdir = format!("'{}'", workdir_str.replace('\'', "'\\''"));
            let script = format!("cd {} && {}", escaped_workdir, command);

            info!("Running {} phase in chroot {}", phase, sysroot.display());

            Command::new("chroot")
                .arg(sysroot)
                .args(&env_args)
                .arg("/bin/sh")
                .arg("-c")
                .arg(&script)
                .output()
                .map_err(|e| Error::IoError(format!("Failed to chroot {} phase: {}", phase, e)))?
        } else {
            let mut cmd = Command::new("sh");
            cmd.arg("-c").arg(command).current_dir(workdir);
            apply_direct_build_env(&mut cmd, env);
            cmd.output()
                .map_err(|e| Error::IoError(format!("Failed to run {} phase: {}", phase, e)))?
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        self.log_build_output(phase, false, &stdout, &stderr);

        if !output.status.success() {
            return Err(Error::IoError(format!(
                "{} phase failed with exit code {:?}\nstderr: {}",
                phase,
                output.status.code(),
                stderr
            )));
        }

        Ok(())
    }

    fn reproducibility_config_for_execution(&self) -> Option<ReproducibilityConfig> {
        self.kitchen.config.reproducibility.as_ref().map(|config| {
            if let Some(sysroot) = &self.kitchen.config.sysroot
                && !self.kitchen.config.use_isolation
            {
                return config.with_roots(
                    &translate_path_for_chroot(&self.source_dir, sysroot),
                    &translate_path_for_chroot(self.build_dir.as_path(), sysroot),
                );
            }
            config.with_roots(&self.source_dir, self.build_dir.as_path())
        })
    }

    fn hermetic_controls_active(&self) -> bool {
        self.kitchen.config.reproducibility.is_some()
            || self.kitchen.config.hermetic_evidence.is_some()
    }

    /// Phase 4: Plate - package the result as CCS
    pub(super) fn plate(&mut self, output_dir: &Path) -> Result<(PathBuf, ManifestProvenance)> {
        // Check that destdir has files
        if fs::read_dir(&self.dest_dir)?.count() == 0 {
            return Err(Error::IoError(
                "No files installed to destdir - install phase may have failed".to_string(),
            ));
        }

        // Create CCS manifest from recipe metadata
        let mut manifest =
            CcsManifest::new_minimal(&self.recipe.package.name, &self.recipe.package.version);

        // Copy over additional metadata from recipe
        if let Some(desc) = &self.recipe.package.description {
            manifest.package.description = desc.clone();
        } else if let Some(summary) = &self.recipe.package.summary {
            manifest.package.description = summary.clone();
        }
        manifest.package.license = self.recipe.package.license.clone();
        manifest.package.homepage = self.recipe.package.homepage.clone();

        // Add build dependencies as requires (for reference)
        for dep in &self.recipe.build.requires {
            manifest.requires.packages.push(PackageDep {
                name: dep.clone(),
                version: None,
            });
        }

        // Build CCS package from destdir
        let builder = CcsBuilder::new(manifest, &self.dest_dir);
        let mut build_result = builder
            .build()
            .map_err(|e| Error::IoError(format!("CCS build failed: {e}")))?;

        // Record file hashes for provenance merkle root
        for file in &build_result.files {
            self.provenance.record_file_hash(&file.path, &file.hash);
        }

        // Compute merkle root from all file hashes
        self.provenance.compute_merkle_root();

        // Convert provenance capture to manifest format
        let provenance = self.provenance.to_manifest_provenance();

        // Attach provenance to the existing build result's manifest
        // (avoids a full rebuild just to embed provenance metadata)
        build_result.manifest.provenance = Some(provenance.clone());

        // Write CCS package
        let package_name = format!(
            "{}-{}-{}.ccs",
            self.recipe.package.name, self.recipe.package.version, self.recipe.package.release
        );
        let package_path = output_dir.join(&package_name);

        write_ccs_package(&build_result, &package_path)
            .map_err(|e| Error::IoError(format!("Failed to write CCS package: {e}")))?;

        self.log_line(&format!(
            "Created CCS package: {} ({} files, {} blobs)",
            package_path.display(),
            build_result.files.len(),
            build_result.blobs.len()
        ));
        info!(
            "Cooked: {} ({} files, DNA: {})",
            package_path.display(),
            build_result.files.len(),
            provenance.dna_hash.as_deref().unwrap_or("unknown")
        );

        Ok((package_path, provenance))
    }

    fn log_line(&mut self, line: &str) {
        self.log.push_str(line);
        self.log.push('\n');
    }

    /// Log build step output (stdout/stderr) with a phase header
    fn log_build_output(&mut self, phase: &str, isolated: bool, stdout: &str, stderr: &str) {
        let header = if isolated {
            format!("=== {} (isolated) ===", phase)
        } else {
            format!("=== {} ===", phase)
        };
        self.log_line(&header);
        if !stdout.is_empty() {
            self.log.push_str(stdout);
            self.log.push('\n');
        }
        if !stderr.is_empty() {
            self.log.push_str(stderr);
            self.log.push('\n');
        }
    }
}

fn resolve_local_patch_path(
    recipe_source_base_dir: Option<&Path>,
    patch_file: &str,
    require_recipe_source_base_dir: bool,
) -> Result<PathBuf> {
    let relative_patch = clean_relative_local_patch_path(patch_file)?;
    let Some(recipe_source_base_dir) = recipe_source_base_dir else {
        if require_recipe_source_base_dir {
            return Err(Error::ConfigError(
                "hermetic local patch application requires recipe source base dir (KitchenConfig.recipe_source_base_dir)"
                    .to_string(),
            ));
        }
        return Ok(relative_patch);
    };

    let canonical_recipe_dir = fs::canonicalize(recipe_source_base_dir).map_err(|error| {
        Error::ConfigError(format!(
            "Recipe source base dir not found for local patch resolution: {} ({error})",
            recipe_source_base_dir.display()
        ))
    })?;
    let patch_path = canonical_recipe_dir.join(relative_patch);
    let canonical_patch = fs::canonicalize(&patch_path).map_err(|error| {
        Error::NotFound(format!(
            "Patch file not found: {} ({error})",
            patch_path.display()
        ))
    })?;

    if !canonical_patch.starts_with(&canonical_recipe_dir) {
        return Err(Error::ConfigError(format!(
            "Local patch path must stay within the recipe directory: {patch_file}"
        )));
    }

    Ok(canonical_patch)
}

fn clean_relative_local_patch_path(patch_file: &str) -> Result<PathBuf> {
    let path = Path::new(patch_file);
    if path.as_os_str().is_empty() {
        return Err(Error::ConfigError(
            "Local patch path cannot be empty".to_string(),
        ));
    }
    if path.is_absolute() {
        return Err(Error::ConfigError(format!(
            "Local patch path must be relative to the recipe directory: {patch_file}"
        )));
    }

    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => clean.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(Error::ConfigError(format!(
                    "Local patch path must stay within the recipe directory: {patch_file}"
                )));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(Error::ConfigError(format!(
                    "Local patch path must be relative to the recipe directory: {patch_file}"
                )));
            }
        }
    }

    if clean.as_os_str().is_empty() {
        return Err(Error::ConfigError(
            "Local patch path cannot be empty".to_string(),
        ));
    }

    Ok(clean)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::format::{
        BuildSection, LocalSourceSection, PackageSection, PatchInfo, PatchSection, Recipe,
        RemoteSourceSection, SourceSection,
    };
    use crate::recipe::hermetic::source_identity::{CiMode, canonical_local_file_list};
    use crate::recipe::hermetic::{
        BuildCommandRiskReport, BuildInputIdentity, BuilderEnvironmentIdentity,
        BuilderEnvironmentKind, DependencyLock, EcosystemPolicyReport, HERMETIC_EVIDENCE_SCHEMA_V1,
        HermeticBuildEvidence, RecipeIdentity, ReproducibilityConfig, ReproducibilityRecord,
        SourceIdentity,
    };
    use crate::recipe::kitchen::KitchenConfig;
    use std::collections::HashMap;
    use std::sync::{LazyLock, Mutex};

    static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    fn minimal_recipe() -> Recipe {
        Recipe {
            package: PackageSection {
                name: "test-pkg".to_string(),
                version: "1.0.0".to_string(),
                release: "1".to_string(),
                summary: None,
                description: None,
                license: None,
                homepage: None,
            },
            source: SourceSection::Remote(RemoteSourceSection {
                archive: "https://example.invalid/test.tar.gz".to_string(),
                checksum: "sha256:test".to_string(),
                signature: None,
                additional: Vec::new(),
                extract_dir: None,
            }),
            build: BuildSection {
                requires: Vec::new(),
                makedepends: Vec::new(),
                configure: None,
                make: None,
                install: None,
                check: None,
                setup: None,
                post_install: None,
                environment: HashMap::new(),
                workdir: None,
                script_file: None,
                jobs: None,
                stage: None,
            },
            cross: None,
            patches: None,
            components: None,
            variables: HashMap::new(),
        }
    }

    #[test]
    fn test_run_build_step_direct_clears_host_environment() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var("CONARY_KITCHEN_LEAK", "host-secret");
        }

        let kitchen = Kitchen::new(KitchenConfig {
            use_isolation: false,
            ..KitchenConfig::default()
        });
        let recipe = minimal_recipe();
        let mut cook = Cook::new(&kitchen, &recipe).unwrap();
        let workdir = cook.build_dir.clone();

        let result = cook.run_build_step_direct(
            "configure",
            "test -z \"$CONARY_KITCHEN_LEAK\"",
            &workdir,
            &[],
        );

        unsafe {
            std::env::remove_var("CONARY_KITCHEN_LEAK");
        }

        assert!(
            result.is_ok(),
            "direct kitchen build steps should not inherit host environment variables: {result:?}"
        );
    }

    #[test]
    fn test_apply_direct_build_env_filters_dangerous_loader_variables() {
        let mut cmd = Command::new("env");
        apply_direct_build_env(
            &mut cmd,
            &[
                ("LD_PRELOAD".to_string(), "/tmp/malicious.so".to_string()),
                ("SAFE_FLAG".to_string(), "1".to_string()),
            ],
        );

        let envs: HashMap<String, Option<String>> = cmd
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect();

        assert!(!envs.contains_key("LD_PRELOAD"));
        assert_eq!(envs.get("SAFE_FLAG"), Some(&Some("1".to_string())));
    }

    #[test]
    fn test_chroot_env_args_filter_dangerous_loader_variables() {
        let args = chroot_env_args(
            &[
                ("LD_LIBRARY_PATH".to_string(), "/tmp/evil".to_string()),
                ("CUSTOM".to_string(), "value".to_string()),
            ],
            8,
        );

        assert!(!args.iter().any(|arg| arg.starts_with("LD_LIBRARY_PATH=")));
        assert!(args.iter().any(|arg| arg == "CUSTOM=value"));
        assert!(args.iter().any(|arg| arg == "MAKEFLAGS=-j8"));
    }

    #[test]
    fn test_chroot_path_translation_maps_sysroot_paths_inside_chroot() {
        let sysroot = Path::new("/tmp/conary-seed/sysroot");
        assert_eq!(
            translate_path_for_chroot(Path::new("/tmp/conary-seed/sysroot/var/tmp/build"), sysroot),
            PathBuf::from("/var/tmp/build")
        );
        assert_eq!(
            translate_path_for_chroot(Path::new("/outside/build"), sysroot),
            PathBuf::from("/outside/build")
        );
    }

    #[test]
    fn test_chroot_reproducibility_config_uses_compiler_visible_roots() {
        let dir = tempfile::tempdir().unwrap();
        let sysroot = dir.path().join("sysroot");
        let dest = sysroot.join("dest");
        let kitchen = Kitchen::new(KitchenConfig {
            sysroot: Some(sysroot.clone()),
            reproducibility: Some(ReproducibilityConfig::default()),
            use_isolation: false,
            ..KitchenConfig::default()
        });
        let recipe = minimal_recipe();
        let cook = Cook::new_with_dest(&kitchen, &recipe, &dest).unwrap();

        let config = cook.reproducibility_config_for_execution().unwrap();
        let env = config.env_vars();
        let rustflags = env
            .iter()
            .find(|(key, _)| key == "RUSTFLAGS")
            .unwrap()
            .1
            .as_str();
        let cflags = env
            .iter()
            .find(|(key, _)| key == "CFLAGS")
            .unwrap()
            .1
            .as_str();
        let sysroot_text = sysroot.to_string_lossy();

        assert!(!rustflags.contains(sysroot_text.as_ref()));
        assert!(!cflags.contains(sysroot_text.as_ref()));
        assert!(rustflags.contains("--remap-path-prefix=/var/tmp/conary-derivation-build/"));
        assert!(cflags.contains("-ffile-prefix-map=/var/tmp/conary-derivation-build/"));
    }

    #[test]
    fn test_prep_host_local_path_source_uses_workspace_as_source_root() {
        let dir = tempfile::tempdir().unwrap();
        let recipe_dir = dir.path().join("recipe");
        let workspace = recipe_dir.join("src");
        let cache = dir.path().join("cache");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("marker.txt"), "local workspace").unwrap();

        let kitchen = Kitchen::new(KitchenConfig {
            source_cache: cache.clone(),
            recipe_source_base_dir: Some(recipe_dir),
            use_isolation: false,
            ..KitchenConfig::default()
        });
        let mut recipe = minimal_recipe();
        recipe.source = SourceSection::Local(LocalSourceSection {
            path: PathBuf::from("./src"),
        });

        let mut cook = Cook::new(&kitchen, &recipe).unwrap();
        cook.prep().unwrap();
        cook.unpack().unwrap();

        assert_eq!(cook.source_dir, workspace.canonicalize().unwrap());
        assert!(!cook.source_dir.starts_with(&cook.build_dir));
        assert_eq!(
            std::fs::read_to_string(cook.source_dir.join("marker.txt")).unwrap(),
            "local workspace"
        );
        assert!(
            !cache.exists() || std::fs::read_dir(&cache).unwrap().next().is_none(),
            "local path source prep should not fetch or cache an archive"
        );
    }

    #[test]
    fn test_prep_local_path_source_requires_recipe_source_base_dir() {
        let mut recipe = minimal_recipe();
        recipe.source = SourceSection::Local(LocalSourceSection {
            path: PathBuf::from("./src"),
        });

        let kitchen = Kitchen::new(KitchenConfig {
            use_isolation: false,
            ..KitchenConfig::default()
        });
        let mut cook = Cook::new(&kitchen, &recipe).unwrap();

        let error = cook.prep().unwrap_err();

        assert!(
            error.to_string().contains("recipe source base dir"),
            "expected missing base dir error, got: {error}"
        );
    }

    #[test]
    fn test_prep_isolated_local_path_source_copies_workspace_into_build_root() {
        let dir = tempfile::tempdir().unwrap();
        let recipe_dir = dir.path().join("recipe");
        let workspace = recipe_dir.join("src");
        std::fs::create_dir_all(workspace.join("nested")).unwrap();
        std::fs::write(workspace.join("nested/marker.txt"), "isolated copy").unwrap();

        let kitchen = Kitchen::new(KitchenConfig {
            recipe_source_base_dir: Some(recipe_dir),
            use_isolation: true,
            ..KitchenConfig::default()
        });
        let mut recipe = minimal_recipe();
        recipe.source = SourceSection::Local(LocalSourceSection {
            path: PathBuf::from("./src"),
        });

        let mut cook = Cook::new(&kitchen, &recipe).unwrap();
        cook.prep().unwrap();
        cook.unpack().unwrap();

        assert!(cook.source_dir.starts_with(&cook.build_dir));
        assert_eq!(
            std::fs::read_to_string(cook.source_dir.join("nested/marker.txt")).unwrap(),
            "isolated copy"
        );
    }

    #[test]
    fn test_prep_isolated_local_path_source_uses_hermetic_file_list_when_present() {
        let dir = tempfile::tempdir().unwrap();
        let recipe_dir = dir.path().join("recipe");
        let workspace = recipe_dir.join("src");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("included.txt"), "included\n").unwrap();
        std::fs::write(workspace.join("excluded.txt"), "excluded\n").unwrap();
        let mut hermetic_files = canonical_local_file_list(&workspace, CiMode::Off).unwrap();
        hermetic_files.retain(|file| file.relative_path == PathBuf::from("included.txt"));

        let kitchen = Kitchen::new(KitchenConfig {
            recipe_source_base_dir: Some(recipe_dir),
            use_isolation: true,
            hermetic_local_files: Some(hermetic_files),
            ..KitchenConfig::default()
        });
        let mut recipe = minimal_recipe();
        recipe.source = SourceSection::Local(LocalSourceSection {
            path: PathBuf::from("./src"),
        });

        let mut cook = Cook::new(&kitchen, &recipe).unwrap();
        cook.prep().unwrap();
        cook.unpack().unwrap();

        assert_eq!(
            std::fs::read_to_string(cook.source_dir.join("included.txt")).unwrap(),
            "included\n"
        );
        assert!(!cook.source_dir.join("excluded.txt").exists());
    }

    #[test]
    fn test_prep_local_path_source_records_local_provenance_marker() {
        let dir = tempfile::tempdir().unwrap();
        let recipe_dir = dir.path().join("recipe");
        let workspace = recipe_dir.join("src");
        std::fs::create_dir_all(&workspace).unwrap();

        let kitchen = Kitchen::new(KitchenConfig {
            recipe_source_base_dir: Some(recipe_dir),
            use_isolation: false,
            ..KitchenConfig::default()
        });
        let mut recipe = minimal_recipe();
        recipe.source = SourceSection::Local(LocalSourceSection {
            path: PathBuf::from("./src"),
        });

        let mut cook = Cook::new(&kitchen, &recipe).unwrap();
        cook.prep().unwrap();

        assert_eq!(cook.provenance.upstream_url.as_deref(), Some("local:./src"));
        assert!(
            cook.provenance.upstream_hash.is_none(),
            "local source provenance should leave upstream_hash unset until tree hashing exists"
        );
    }

    #[test]
    fn test_patch_local_path_resolves_relative_to_recipe_source_base_dir() {
        let dir = tempfile::tempdir().unwrap();
        let recipe_dir = dir.path().join("recipe");
        let patch_dir = recipe_dir.join("patches");
        std::fs::create_dir_all(&patch_dir).unwrap();
        std::fs::write(
            patch_dir.join("fix.patch"),
            r#"--- file.txt
+++ file.txt
@@ -1 +1 @@
-old
+new
"#,
        )
        .unwrap();

        let kitchen = Kitchen::new(KitchenConfig {
            recipe_source_base_dir: Some(recipe_dir),
            use_isolation: false,
            ..KitchenConfig::default()
        });
        let mut recipe = minimal_recipe();
        recipe.patches = Some(PatchSection {
            files: vec![PatchInfo {
                file: "patches/fix.patch".to_string(),
                checksum: None,
                strip: 0,
                condition: None,
            }],
        });

        let mut cook = Cook::new(&kitchen, &recipe).unwrap();
        std::fs::write(cook.source_dir.join("file.txt"), "old\n").unwrap();

        cook.patch().unwrap();

        assert_eq!(
            std::fs::read_to_string(cook.source_dir.join("file.txt")).unwrap(),
            "new\n"
        );
    }

    #[test]
    fn test_patch_local_path_substitutes_recipe_variables() {
        let dir = tempfile::tempdir().unwrap();
        let recipe_dir = dir.path().join("recipe");
        let patch_dir = recipe_dir.join("patches");
        std::fs::create_dir_all(&patch_dir).unwrap();
        std::fs::write(
            patch_dir.join("1.0.0.patch"),
            r#"--- file.txt
+++ file.txt
@@ -1 +1 @@
-old
+new
"#,
        )
        .unwrap();

        let kitchen = Kitchen::new(KitchenConfig {
            recipe_source_base_dir: Some(recipe_dir),
            use_isolation: false,
            ..KitchenConfig::default()
        });
        let mut recipe = minimal_recipe();
        recipe.patches = Some(PatchSection {
            files: vec![PatchInfo {
                file: "patches/%(version)s.patch".to_string(),
                checksum: None,
                strip: 0,
                condition: None,
            }],
        });

        let mut cook = Cook::new(&kitchen, &recipe).unwrap();
        std::fs::write(cook.source_dir.join("file.txt"), "old\n").unwrap();

        cook.patch().unwrap();

        assert_eq!(
            std::fs::read_to_string(cook.source_dir.join("file.txt")).unwrap(),
            "new\n"
        );
    }

    #[test]
    fn test_hermetic_local_patch_requires_recipe_source_base_dir() {
        let kitchen = Kitchen::new(KitchenConfig {
            hermetic_evidence: Some(dummy_hermetic_evidence()),
            pristine_mode: true,
            use_isolation: false,
            ..KitchenConfig::default()
        });
        let mut recipe = minimal_recipe();
        recipe.patches = Some(PatchSection {
            files: vec![PatchInfo {
                file: "patches/fix.patch".to_string(),
                checksum: None,
                strip: 0,
                condition: None,
            }],
        });
        let mut cook = Cook::new(&kitchen, &recipe).unwrap();

        let error = cook.patch().unwrap_err();

        assert!(error.to_string().contains("hermetic"));
        assert!(error.to_string().contains("recipe source base dir"));
    }

    #[test]
    fn test_cook_new_rejects_hermetic_evidence_without_pristine_mode() {
        let kitchen = Kitchen::new(KitchenConfig {
            hermetic_evidence: Some(dummy_hermetic_evidence()),
            pristine_mode: false,
            ..KitchenConfig::default()
        });
        let recipe = minimal_recipe();

        let error = match Cook::new(&kitchen, &recipe) {
            Ok(_) => panic!("expected hermetic evidence without pristine mode to be rejected"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("hermetic evidence"));
        assert!(error.to_string().contains("pristine mode"));
    }

    #[test]
    fn test_simmer_rejects_command_local_source_date_epoch_override_in_hermetic_mode() {
        let kitchen = Kitchen::new(KitchenConfig {
            hermetic_evidence: Some(dummy_hermetic_evidence()),
            reproducibility: Some(ReproducibilityConfig::default()),
            pristine_mode: true,
            use_isolation: false,
            ..KitchenConfig::default()
        });
        let mut recipe = minimal_recipe();
        recipe.build.make = Some("SOURCE_DATE_EPOCH=999 true".to_string());
        let mut cook = Cook::new(&kitchen, &recipe).unwrap();

        let error = cook.simmer().unwrap_err();

        assert!(error.to_string().contains("SOURCE_DATE_EPOCH"));
        assert!(error.to_string().contains("command-local"));
    }

    #[test]
    fn test_simmer_rejects_shell_startup_env_in_hermetic_mode() {
        let cases = [("SHELLOPTS", "keyword"), ("BASHOPTS", "expand_aliases")];

        for (key, value) in cases {
            let kitchen = Kitchen::new(KitchenConfig {
                hermetic_evidence: Some(dummy_hermetic_evidence()),
                reproducibility: Some(ReproducibilityConfig::default()),
                pristine_mode: true,
                use_isolation: false,
                ..KitchenConfig::default()
            });
            let mut recipe = minimal_recipe();
            recipe
                .build
                .environment
                .insert(key.to_string(), value.to_string());
            recipe.build.make = Some("make SOURCE_DATE_EPOCH=999".to_string());
            let mut cook = Cook::new(&kitchen, &recipe).unwrap();

            let error = cook.simmer().unwrap_err();

            assert!(
                error.to_string().contains(key),
                "expected {key} rejection, got: {error}"
            );
        }
    }

    #[test]
    fn test_simmer_rejects_make_override_env_in_hermetic_mode() {
        for (key, value, expected) in [
            ("MAKEOVERRIDES", "CFLAGS=bad", "CFLAGS"),
            ("MAKEFILES", "evil.mk", "MAKEFILES"),
        ] {
            let kitchen = Kitchen::new(KitchenConfig {
                hermetic_evidence: Some(dummy_hermetic_evidence()),
                reproducibility: Some(ReproducibilityConfig::default()),
                pristine_mode: true,
                use_isolation: false,
                ..KitchenConfig::default()
            });
            let mut recipe = minimal_recipe();
            recipe
                .build
                .environment
                .insert(key.to_string(), value.to_string());
            recipe.build.make = Some("true".to_string());
            let mut cook = Cook::new(&kitchen, &recipe).unwrap();

            let error = cook.simmer().unwrap_err();

            assert!(error.to_string().contains(key));
            assert!(error.to_string().contains(expected));
        }
    }

    #[test]
    fn test_hermetic_command_validation_rejects_shell_env_mutation_forms() {
        let config = ReproducibilityConfig::new(0, Path::new("/src"), Path::new("/build"));
        let cases = [
            ("SOURCE_DATE_EPOCH=999; make", "SOURCE_DATE_EPOCH"),
            ("export SOURCE_DATE_EPOCH=999; make", "SOURCE_DATE_EPOCH"),
            ("unset SOURCE_DATE_EPOCH; make", "SOURCE_DATE_EPOCH"),
            (
                "/usr/bin/env SOURCE_DATE_EPOCH=999 make",
                "SOURCE_DATE_EPOCH",
            ),
            ("env -i make", "environment"),
            ("env -u SOURCE_DATE_EPOCH make", "SOURCE_DATE_EPOCH"),
            ("env --unset=RUSTFLAGS make", "RUSTFLAGS"),
            ("/usr/bin/env -u CFLAGS make", "CFLAGS"),
            ("env -iu SOURCE_DATE_EPOCH make", "environment"),
            ("env - make", "environment"),
            (
                "env -C /tmp SOURCE_DATE_EPOCH=999 make",
                "SOURCE_DATE_EPOCH",
            ),
            (
                "env --chdir=/tmp SOURCE_DATE_EPOCH=999 make",
                "SOURCE_DATE_EPOCH",
            ),
            (
                "env -a custom SOURCE_DATE_EPOCH=999 make",
                "SOURCE_DATE_EPOCH",
            ),
            (
                "env --argv0=custom SOURCE_DATE_EPOCH=999 make",
                "SOURCE_DATE_EPOCH",
            ),
            (
                "env --debug SOURCE_DATE_EPOCH=999 make",
                "SOURCE_DATE_EPOCH",
            ),
            ("env -- SOURCE_DATE_EPOCH=999 make", "SOURCE_DATE_EPOCH"),
            (
                "env --block-signal SOURCE_DATE_EPOCH=999 make",
                "--block-signal",
            ),
            ("env -S 'SOURCE_DATE_EPOCH=999 make'", "-S"),
            (
                "env --split-string='SOURCE_DATE_EPOCH=999 make'",
                "split-string",
            ),
            (
                "env 'BASH_FUNC_make%%=() { SOURCE_DATE_EPOCH=999 make; }' ./build.sh",
                "BASH_FUNC_make%%",
            ),
            ("make SOURCE_DATE_EPOCH=999", "SOURCE_DATE_EPOCH"),
            ("gmake RUSTFLAGS+=bad", "RUSTFLAGS"),
            ("make MAKEFLAGS=SOURCE_DATE_EPOCH=999", "MAKEFLAGS"),
            ("MAKEFLAGS=SOURCE_DATE_EPOCH=999 make", "MAKEFLAGS"),
            ("MAKEFLAGS+=SOURCE_DATE_EPOCH=999 make", "MAKEFLAGS"),
            ("env GNUMAKEFLAGS=RUSTFLAGS=bad make", "GNUMAKEFLAGS"),
            ("make --eval 'export SOURCE_DATE_EPOCH=999'", "--eval"),
            ("MAKEFILES=evil.mk make", "MAKEFILES"),
            ("env MAKEFILES=evil.mk make", "MAKEFILES"),
            ("make -f evil.mk", "-f"),
            ("make --file=evil.mk", "--file"),
            ("MAKEFLAGS=--file=evil.mk make", "MAKEFLAGS"),
            ("make -rfevil.mk", "-rfevil.mk"),
            ("MAKEFLAGS=-rfevil.mk make", "MAKEFLAGS"),
            ("GNUMAKEFLAGS=-rfevil.mk make", "GNUMAKEFLAGS"),
            ("make -rEexport SOURCE_DATE_EPOCH=999", "-rEexport"),
            (
                "MAKEFLAGS='-rEexport SOURCE_DATE_EPOCH=999' make",
                "MAKEFLAGS",
            ),
            ("make --ev=export SOURCE_DATE_EPOCH=999", "--ev"),
            ("MAKEFLAGS='--ev=export CFLAGS=bad' make", "MAKEFLAGS"),
            ("make --fi=evil.mk", "--fi"),
            ("GNUMAKEFLAGS=--fi=evil.mk make", "GNUMAKEFLAGS"),
            ("make --mak=evil.mk", "--mak"),
            ("make -I evil", "-I"),
            ("MAKEFLAGS=-Ievil make", "MAKEFLAGS"),
            ("GNUMAKEFLAGS=--include-dir=evil make", "GNUMAKEFLAGS"),
            ("make --inc=evil", "--inc"),
            (
                "name=SOURCE_DATE_EPOCH; export $name=999; make",
                "shell expansion",
            ),
            ("export${IFS}SOURCE_DATE_EPOCH=999; make", "shell expansion"),
            ("ARGS=SOURCE_DATE_EPOCH=999; make $ARGS", "shell expansion"),
            ("OPT=--inc=evil; make $OPT", "shell expansion"),
            (
                "BAD=SOURCE_DATE_EPOCH=999; MAKEFLAGS=$BAD make",
                "MAKEFLAGS",
            ),
            ("env -u $KEY make", "shell expansion"),
            ("printf -v $name 999; make", "shell expansion"),
            (
                "export $(printf SOURCE_DATE_EPOCH)=999; make",
                "command substitution",
            ),
            (
                "export `printf SOURCE_DATE_EPOCH`=999; make",
                "command substitution",
            ),
            (
                "make $(printf SOURCE_DATE_EPOCH=999)",
                "command substitution",
            ),
            (
                "MAKEFLAGS=$(printf SOURCE_DATE_EPOCH=999) make",
                "command substitution",
            ),
            (
                "make `printf -- --include-dir=evil`",
                "command substitution",
            ),
            (
                "sh <(printf %s \"export SOURCE_DATE_EPOCH=999; make -s\")",
                "process substitution",
            ),
            (
                "bash <(printf %s \"export SOURCE_DATE_EPOCH=999; make -s\")",
                "process substitution",
            ),
            ("export SOURCE_DATE_EPOCH{,}=999; make", "shell expansion"),
            ("make SOURCE_DATE_EPOCH{,}=999", "shell expansion"),
            ("make --inc{,}=evil", "shell expansion"),
            ("export *; make", "shell expansion"),
            ("env SOURCE* make", "shell expansion"),
            ("make all *", "shell expansion"),
            ("make all --include*", "shell expansion"),
            ("export SOURCE_DATE_EPOCH\\=999; make", "shell expansion"),
            ("e\\xport SOURCE_DATE_EPOCH=777; make", "shell expansion"),
            ("ex\"\"port SOURCE_DATE_EPOCH=333; make", "shell expansion"),
            ("make SOURCE_DATE_EPOCH\\=999", "shell expansion"),
            ("ma\\ke SOURCE_DATE_EPOCH=888", "shell expansion"),
            ("ma\"\"ke SOURCE_DATE_EPOCH=777", "shell expansion"),
            ("env SOURCE_DATE_EPOCH\\=999 make", "shell expansion"),
            ("MAKEFLAGS=SOURCE_DATE_EPOCH\\=999 make", "MAKEFLAGS"),
            (
                "ARGS=x,SOURCE_DATE_EPOCH=999; IFS=,; env -a $ARGS make -s",
                "shell expansion",
            ),
            (
                "ARGS=x,SOURCE_DATE_EPOCH=999; IFS=,; env --argv0 $ARGS make -s",
                "shell expansion",
            ),
            (
                "ARGS=dir,SOURCE_DATE_EPOCH=999; IFS=,; env -C $ARGS make -s",
                "shell expansion",
            ),
            (
                "ARGS=x,env,SOURCE_DATE_EPOCH=999; IFS=,; exec -a $ARGS make -s",
                "shell expansion",
            ),
            (
                "ARGS=x,SOURCE_DATE_EPOCH=999; IFS=,; env --argv0=$ARGS make -s",
                "shell expansion",
            ),
            (
                "ARGS=dir,SOURCE_DATE_EPOCH=999; IFS=,; env --chdir=$ARGS make -s",
                "shell expansion",
            ),
            (
                "command env SOURCE_DATE_EPOCH=999 make",
                "SOURCE_DATE_EPOCH",
            ),
            (
                "command -p env SOURCE_DATE_EPOCH=999 make",
                "SOURCE_DATE_EPOCH",
            ),
            ("exec env -u SOURCE_DATE_EPOCH make", "SOURCE_DATE_EPOCH"),
            (
                "command export SOURCE_DATE_EPOCH=999; make",
                "SOURCE_DATE_EPOCH",
            ),
            ("command unset SOURCE_DATE_EPOCH; make", "SOURCE_DATE_EPOCH"),
            ("exec -c make", "environment"),
            ("readonly SOURCE_DATE_EPOCH=999; make", "SOURCE_DATE_EPOCH"),
            ("readonly SOURCE_DATE_EPOCH; make", "SOURCE_DATE_EPOCH"),
            (
                "command readonly SOURCE_DATE_EPOCH=999; make",
                "SOURCE_DATE_EPOCH",
            ),
            (
                "readonly RUSTFLAGS=--remap-path-prefix=/src=/build/source-old; make",
                "RUSTFLAGS",
            ),
            ("time SOURCE_DATE_EPOCH=999 make", "SOURCE_DATE_EPOCH"),
            ("! env SOURCE_DATE_EPOCH=999 make", "SOURCE_DATE_EPOCH"),
            (
                "if SOURCE_DATE_EPOCH=999 make; then :; fi",
                "SOURCE_DATE_EPOCH",
            ),
            ("sh -c 'SOURCE_DATE_EPOCH=999 make'", "-c"),
            ("/bin/sh -c 'env -u SOURCE_DATE_EPOCH make'", "-c"),
            ("bash -ec 'SOURCE_DATE_EPOCH=999 make'", "-ec"),
            ("ash -c 'SOURCE_DATE_EPOCH=999 make'", "-c"),
            ("busybox sh -c 'SOURCE_DATE_EPOCH=999 make'", "-c"),
            ("busybox ash -c 'env -u SOURCE_DATE_EPOCH make'", "-c"),
            ("env sh -c 'SOURCE_DATE_EPOCH=999 make'", "-c"),
            ("env busybox ash -c 'SOURCE_DATE_EPOCH=999 make'", "-c"),
            (
                "/usr/bin/env /bin/sh -c 'env -u SOURCE_DATE_EPOCH make'",
                "-c",
            ),
            ("env env SOURCE_DATE_EPOCH=999 make", "SOURCE_DATE_EPOCH"),
            (
                "/usr/bin/env /usr/bin/env -u SOURCE_DATE_EPOCH make",
                "SOURCE_DATE_EPOCH",
            ),
            (
                "builtin export SOURCE_DATE_EPOCH=999; make",
                "SOURCE_DATE_EPOCH",
            ),
            ("SOURCE_DATE_EPOCH+=999 env", "SOURCE_DATE_EPOCH"),
            ("SOURCE_DATE_EPOCH[0]=999 env", "SOURCE_DATE_EPOCH"),
            ("env SOURCE_DATE_EPOCH[0]=999 make", "SOURCE_DATE_EPOCH"),
            ("export SOURCE_DATE_EPOCH+=999; make", "SOURCE_DATE_EPOCH"),
            ("declare SOURCE_DATE_EPOCH=999; make", "SOURCE_DATE_EPOCH"),
            ("declare SOURCE_DATE_EPOCH+=999; make", "SOURCE_DATE_EPOCH"),
            ("declare -n ref=SOURCE_DATE_EPOCH; ref=999; make", "nameref"),
            (
                "declare -n ref=SOURCE_DATE_EPOCH; ref+=999; make",
                "nameref",
            ),
            ("typeset CFLAGS=bad; make", "CFLAGS"),
            ("typeset CFLAGS+=bad; make", "CFLAGS"),
            ("typeset -n ref=RUSTFLAGS; ref=bad; make", "nameref"),
            (
                "f(){ local SOURCE_DATE_EPOCH=999; make; }; f",
                "shell expansion",
            ),
            (
                "function f { local -n ref=RUSTFLAGS; ref=bad; make; }; f",
                "function",
            ),
            (
                "f(){ local SOURCE_DATE_EPOCH[0]=999; make; }; f",
                "shell expansion",
            ),
            ("readonly SOURCE_DATE_EPOCH+=999; make", "SOURCE_DATE_EPOCH"),
            (
                "declare SOURCE_DATE_EPOCH[0]=999; make",
                "SOURCE_DATE_EPOCH",
            ),
            ("readonly RUSTFLAGS[0]+=bad; make", "RUSTFLAGS"),
            (
                "read SOURCE_DATE_EPOCH <<EOF\n999\nEOF\nmake",
                "SOURCE_DATE_EPOCH",
            ),
            (
                "read SOURCE_DATE_EPOCH[0] <<< 999; make",
                "SOURCE_DATE_EPOCH",
            ),
            ("read < file SOURCE_DATE_EPOCH; make", "read redirection"),
            ("mapfile SOURCE_DATE_EPOCH; make", "SOURCE_DATE_EPOCH"),
            ("readarray CFLAGS; make", "CFLAGS"),
            ("printf -v SOURCE_DATE_EPOCH 999; make", "SOURCE_DATE_EPOCH"),
            (
                "printf -v SOURCE_DATE_EPOCH[0] 999; make",
                "SOURCE_DATE_EPOCH",
            ),
            ("printf -v RUSTFLAGS[0] bad; make", "RUSTFLAGS"),
            ("let SOURCE_DATE_EPOCH=999; make", "SOURCE_DATE_EPOCH"),
            ("getopts ab SOURCE_DATE_EPOCH; make", "SOURCE_DATE_EPOCH"),
            ("eval 'SOURCE_DATE_EPOCH=999 make'", "eval"),
            ("source ./env-file; make", "source"),
            (". ./env-file; make", "."),
            (
                "export RUSTFLAGS=--remap-path-prefix=/src=/build/source-old; make",
                "RUSTFLAGS",
            ),
        ];

        for (command, key) in cases {
            let error =
                validate_command_local_reproducibility_env(&config, "make", command).unwrap_err();
            assert!(
                error.to_string().contains(key),
                "expected {key} rejection for {command}, got: {error}"
            );
        }
    }

    #[test]
    fn test_env_wrapper_scanner_validates_nested_env_command() {
        let config = ReproducibilityConfig::new(0, Path::new("/src"), Path::new("/build"));
        let cases = [
            (
                vec![
                    "sh".to_string(),
                    "-c".to_string(),
                    "SOURCE_DATE_EPOCH=999 make".to_string(),
                ],
                "-c",
            ),
            (
                vec![
                    "/bin/sh".to_string(),
                    "-c".to_string(),
                    "env -u SOURCE_DATE_EPOCH make".to_string(),
                ],
                "-c",
            ),
            (
                vec![
                    "env".to_string(),
                    "SOURCE_DATE_EPOCH=999".to_string(),
                    "make".to_string(),
                ],
                "SOURCE_DATE_EPOCH",
            ),
            (
                vec![
                    "/usr/bin/env".to_string(),
                    "-u".to_string(),
                    "SOURCE_DATE_EPOCH".to_string(),
                    "make".to_string(),
                ],
                "SOURCE_DATE_EPOCH",
            ),
        ];

        for (tokens, expected) in cases {
            let error = validate_env_wrapper_mutations(&config, "make", &tokens).unwrap_err();

            assert!(
                error.to_string().contains(expected),
                "expected {expected} rejection, got: {error}"
            );
        }
    }

    #[test]
    fn test_shell_env_scanner_rejects_nested_shell_c_invocations() {
        let config = ReproducibilityConfig::new(0, Path::new("/src"), Path::new("/build"));
        let cases = [
            ("sh -c 'SOURCE_DATE_EPOCH=999 make'", "-c"),
            ("/bin/sh -c 'env -u SOURCE_DATE_EPOCH make'", "-c"),
            ("bash -ec 'SOURCE_DATE_EPOCH=999 make'", "-ec"),
            ("dash -e -c 'SOURCE_DATE_EPOCH=999 make'", "-c"),
            ("zsh -ce 'SOURCE_DATE_EPOCH=999 make'", "-ce"),
            ("ksh -c 'SOURCE_DATE_EPOCH=999 make'", "-c"),
            ("mksh -c 'SOURCE_DATE_EPOCH=999 make'", "-c"),
            ("ash -c 'SOURCE_DATE_EPOCH=999 make'", "-c"),
            ("busybox sh -c 'SOURCE_DATE_EPOCH=999 make'", "-c"),
            ("busybox ash -c 'env -u SOURCE_DATE_EPOCH make'", "-c"),
        ];

        for (segment, expected) in cases {
            let error = validate_shell_env_mutation_segment(&config, "make", segment).unwrap_err();

            assert!(
                error.to_string().contains(expected),
                "expected {expected} rejection for {segment}, got: {error}"
            );
        }
    }

    #[test]
    fn test_env_wrapper_scanner_rejects_busybox_shell_applets() {
        let config = ReproducibilityConfig::new(0, Path::new("/src"), Path::new("/build"));
        let tokens = vec![
            "busybox".to_string(),
            "ash".to_string(),
            "-c".to_string(),
            "SOURCE_DATE_EPOCH=999 make".to_string(),
        ];

        let error = validate_env_wrapper_mutations(&config, "make", &tokens).unwrap_err();

        assert!(error.to_string().contains("-c"));
    }

    #[test]
    fn test_shell_env_scanner_rejects_control_word_hidden_env_mutations() {
        let config = ReproducibilityConfig::new(0, Path::new("/src"), Path::new("/build"));
        let cases = [
            ("time SOURCE_DATE_EPOCH=999 make", "SOURCE_DATE_EPOCH"),
            ("time -p SOURCE_DATE_EPOCH=999 make", "SOURCE_DATE_EPOCH"),
            ("! env SOURCE_DATE_EPOCH=999 make", "SOURCE_DATE_EPOCH"),
            ("if SOURCE_DATE_EPOCH=999 make", "SOURCE_DATE_EPOCH"),
            ("while SOURCE_DATE_EPOCH=999 make", "SOURCE_DATE_EPOCH"),
            ("until SOURCE_DATE_EPOCH=999 make", "SOURCE_DATE_EPOCH"),
        ];

        for (segment, expected) in cases {
            let error = validate_shell_env_mutation_segment(&config, "make", segment).unwrap_err();

            assert!(
                error.to_string().contains(expected),
                "expected {expected} rejection for {segment}, got: {error}"
            );
        }
    }

    #[test]
    fn test_shell_env_scanner_rejects_keyword_mode_set() {
        let config = ReproducibilityConfig::new(0, Path::new("/src"), Path::new("/build"));
        let cases = [
            ("set -k; make SOURCE_DATE_EPOCH=999", "-k"),
            ("set -ak; make RUSTFLAGS=bad", "-ak"),
            ("set -o keyword; make CFLAGS=bad", "keyword"),
        ];

        for (command, expected) in cases {
            let error = validate_shell_env_mutations(&config, "make", command).unwrap_err();

            assert!(
                error.to_string().contains(expected),
                "expected {expected} rejection for {command}, got: {error}"
            );
        }
    }

    #[test]
    fn test_shell_env_scanner_rejects_alias_expansion_surfaces() {
        let config = ReproducibilityConfig::new(0, Path::new("/src"), Path::new("/build"));
        let cases = [
            (
                "shopt -s expand_aliases\nalias m='SOURCE_DATE_EPOCH=999 make'\nm",
                "shopt",
            ),
            (
                "shopt -s expand_aliases\nalias m='export SOURCE_DATE_EPOCH=999'\nm\nmake",
                "shopt",
            ),
            ("alias m='SOURCE_DATE_EPOCH=999 make'", "alias"),
            ("unalias m", "unalias"),
        ];

        for (command, expected) in cases {
            let error = validate_shell_env_mutations(&config, "make", command).unwrap_err();

            assert!(
                error.to_string().contains(expected),
                "expected {expected} rejection for {command}, got: {error}"
            );
        }
    }

    #[test]
    fn test_shell_env_scanner_rejects_trap_env_bypasses() {
        let config = ReproducibilityConfig::new(0, Path::new("/src"), Path::new("/build"));
        let cases = [
            "trap 'export SOURCE_DATE_EPOCH=999' DEBUG; make",
            "trap 'SOURCE_DATE_EPOCH=999' DEBUG; make",
        ];

        for command in cases {
            let error = validate_shell_env_mutations(&config, "make", command).unwrap_err();

            assert!(
                error.to_string().contains("trap"),
                "expected trap rejection for {command}, got: {error}"
            );
        }
    }

    #[test]
    fn test_shell_env_scanner_fails_closed_on_unsupported_control_words() {
        let config = ReproducibilityConfig::new(0, Path::new("/src"), Path::new("/build"));
        let cases = [("time -v make", "-v"), ("for item in values", "for")];

        for (segment, expected) in cases {
            let error = validate_shell_env_mutation_segment(&config, "make", segment).unwrap_err();

            assert!(
                error.to_string().contains(expected),
                "expected {expected} rejection for {segment}, got: {error}"
            );
        }
    }

    #[test]
    fn test_shell_env_scanner_rejects_readonly_controlled_vars() {
        let config = ReproducibilityConfig::new(0, Path::new("/src"), Path::new("/build"));
        let cases = [
            ("readonly SOURCE_DATE_EPOCH=999", "SOURCE_DATE_EPOCH"),
            ("readonly SOURCE_DATE_EPOCH", "SOURCE_DATE_EPOCH"),
            (
                "command readonly SOURCE_DATE_EPOCH=999",
                "SOURCE_DATE_EPOCH",
            ),
            (
                "readonly RUSTFLAGS=--remap-path-prefix=/src=/build/source-old",
                "RUSTFLAGS",
            ),
        ];

        for (segment, expected) in cases {
            let error = validate_shell_env_mutation_segment(&config, "make", segment).unwrap_err();

            assert!(
                error.to_string().contains(expected),
                "expected {expected} rejection for {segment}, got: {error}"
            );
        }
    }

    #[test]
    fn test_shell_env_scanner_rejects_append_assignments() {
        let config = ReproducibilityConfig::new(0, Path::new("/src"), Path::new("/build"));
        let cases = [
            ("SOURCE_DATE_EPOCH+=999 env", "SOURCE_DATE_EPOCH"),
            ("SOURCE_DATE_EPOCH[0]=999 env", "SOURCE_DATE_EPOCH"),
            ("export SOURCE_DATE_EPOCH+=999", "SOURCE_DATE_EPOCH"),
            ("declare SOURCE_DATE_EPOCH+=999", "SOURCE_DATE_EPOCH"),
            ("typeset CFLAGS+=bad", "CFLAGS"),
            ("readonly SOURCE_DATE_EPOCH+=999", "SOURCE_DATE_EPOCH"),
            ("readonly SOURCE_DATE_EPOCH[0]+=999", "SOURCE_DATE_EPOCH"),
        ];

        for (segment, expected) in cases {
            let error = validate_shell_env_mutation_segment(&config, "make", segment).unwrap_err();

            assert!(
                error.to_string().contains(expected),
                "expected {expected} rejection for {segment}, got: {error}"
            );
        }
    }

    #[test]
    fn test_shell_env_scanner_rejects_assignment_capable_builtins() {
        let config = ReproducibilityConfig::new(0, Path::new("/src"), Path::new("/build"));
        let cases = [
            ("builtin export SOURCE_DATE_EPOCH=999", "SOURCE_DATE_EPOCH"),
            (
                "builtin printf -v SOURCE_DATE_EPOCH 999",
                "SOURCE_DATE_EPOCH",
            ),
            ("builtin -x export SOURCE_DATE_EPOCH=999", "-x"),
            ("declare SOURCE_DATE_EPOCH=999", "SOURCE_DATE_EPOCH"),
            ("declare -x SOURCE_DATE_EPOCH", "SOURCE_DATE_EPOCH"),
            ("declare -n ref=SOURCE_DATE_EPOCH", "nameref"),
            ("declare -gn ref=SOURCE_DATE_EPOCH", "nameref"),
            ("declare +n ref=SOURCE_DATE_EPOCH", "nameref"),
            ("declare -n ref=SOURCE_DATE_EPOCH; ref+=999", "nameref"),
            ("typeset CFLAGS=bad", "CFLAGS"),
            ("typeset -n ref=RUSTFLAGS", "nameref"),
            ("local SOURCE_DATE_EPOCH=999", "SOURCE_DATE_EPOCH"),
            ("local -n ref=RUSTFLAGS", "nameref"),
            ("local SOURCE_DATE_EPOCH[0]=999", "SOURCE_DATE_EPOCH"),
            ("function f { local SOURCE_DATE_EPOCH=999", "function"),
            ("declare SOURCE_DATE_EPOCH[0]=999", "SOURCE_DATE_EPOCH"),
            ("typeset CFLAGS[0]=bad", "CFLAGS"),
            ("read -r SOURCE_DATE_EPOCH", "SOURCE_DATE_EPOCH"),
            ("read -a SOURCE_DATE_EPOCH", "SOURCE_DATE_EPOCH"),
            ("read SOURCE_DATE_EPOCH[0] <<< 999", "SOURCE_DATE_EPOCH"),
            ("read < file SOURCE_DATE_EPOCH", "read redirection"),
            ("mapfile SOURCE_DATE_EPOCH", "SOURCE_DATE_EPOCH"),
            ("mapfile SOURCE_DATE_EPOCH[0]", "SOURCE_DATE_EPOCH"),
            ("readarray CFLAGS", "CFLAGS"),
            ("readarray CFLAGS[0]", "CFLAGS"),
            ("printf -v SOURCE_DATE_EPOCH 999", "SOURCE_DATE_EPOCH"),
            ("printf -v SOURCE_DATE_EPOCH[0] 999", "SOURCE_DATE_EPOCH"),
            ("printf -v RUSTFLAGS[0] bad", "RUSTFLAGS"),
            ("let SOURCE_DATE_EPOCH=999", "SOURCE_DATE_EPOCH"),
            ("let count=SOURCE_DATE_EPOCH+1", "SOURCE_DATE_EPOCH"),
            ("getopts ab SOURCE_DATE_EPOCH", "SOURCE_DATE_EPOCH"),
            ("eval 'SOURCE_DATE_EPOCH=999 make'", "eval"),
            ("source ./env-file", "source"),
            (". ./env-file", "."),
        ];

        for (segment, expected) in cases {
            let error = validate_shell_env_mutation_segment(&config, "make", segment).unwrap_err();

            assert!(
                error.to_string().contains(expected),
                "expected {expected} rejection for {segment}, got: {error}"
            );
        }
    }

    #[test]
    fn test_shell_env_scanner_peels_command_and_exec_wrappers() {
        let config = ReproducibilityConfig::new(0, Path::new("/src"), Path::new("/build"));
        let cases = [
            (
                "command env SOURCE_DATE_EPOCH=999 make",
                "SOURCE_DATE_EPOCH",
            ),
            (
                "command -p env SOURCE_DATE_EPOCH=999 make",
                "SOURCE_DATE_EPOCH",
            ),
            ("exec env -u SOURCE_DATE_EPOCH make", "SOURCE_DATE_EPOCH"),
            ("command export SOURCE_DATE_EPOCH=999", "SOURCE_DATE_EPOCH"),
            ("command unset SOURCE_DATE_EPOCH", "SOURCE_DATE_EPOCH"),
            ("exec -c make", "environment"),
        ];

        for (segment, expected) in cases {
            let error = validate_shell_env_mutation_segment(&config, "make", segment).unwrap_err();

            assert!(
                error.to_string().contains(expected),
                "expected {expected} rejection for {segment}, got: {error}"
            );
        }
    }

    #[test]
    fn test_env_wrapper_scanner_keeps_scanning_after_option_delimiter() {
        let config = ReproducibilityConfig::new(0, Path::new("/src"), Path::new("/build"));
        let tokens = vec![
            "--".to_string(),
            "SOURCE_DATE_EPOCH=999".to_string(),
            "make".to_string(),
        ];

        let error = validate_env_wrapper_mutations(&config, "make", &tokens).unwrap_err();

        assert!(error.to_string().contains("SOURCE_DATE_EPOCH"));
    }

    #[test]
    fn test_env_wrapper_scanner_rejects_unsupported_long_options() {
        let config = ReproducibilityConfig::new(0, Path::new("/src"), Path::new("/build"));
        let tokens = vec![
            "--block-signal".to_string(),
            "SOURCE_DATE_EPOCH=999".to_string(),
            "make".to_string(),
        ];

        let error = validate_env_wrapper_mutations(&config, "make", &tokens).unwrap_err();

        assert!(error.to_string().contains("--block-signal"));
    }

    #[test]
    fn test_env_wrapper_scanner_rejects_split_string_options() {
        let config = ReproducibilityConfig::new(0, Path::new("/src"), Path::new("/build"));
        for (tokens, expected) in [
            (
                vec!["-S".to_string(), "SOURCE_DATE_EPOCH=999 make".to_string()],
                "-S",
            ),
            (
                vec!["--split-string=SOURCE_DATE_EPOCH=999 make".to_string()],
                "split-string",
            ),
        ] {
            let error = validate_env_wrapper_mutations(&config, "make", &tokens).unwrap_err();

            assert!(
                error.to_string().contains(expected),
                "expected {expected} rejection, got: {error}"
            );
        }
    }

    #[test]
    fn test_hermetic_check_phase_env_guard_fails_closed() {
        let kitchen = Kitchen::new(KitchenConfig {
            hermetic_evidence: Some(dummy_hermetic_evidence()),
            reproducibility: Some(ReproducibilityConfig::default()),
            pristine_mode: true,
            use_isolation: false,
            ..KitchenConfig::default()
        });
        let mut recipe = minimal_recipe();
        recipe.build.check = Some("SOURCE_DATE_EPOCH=999 true".to_string());
        let mut cook = Cook::new(&kitchen, &recipe).unwrap();

        let error = cook.simmer().unwrap_err();

        assert!(error.to_string().contains("SOURCE_DATE_EPOCH"));
        assert!(error.to_string().contains("command-local"));
    }

    #[cfg(unix)]
    #[test]
    fn test_prep_isolated_local_path_source_rejects_nested_relative_symlink_escape() {
        let dir = tempfile::tempdir().unwrap();
        let recipe_dir = dir.path().join("recipe");
        let workspace = recipe_dir.join("src");
        let outside = recipe_dir.join("outside");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("marker.txt"), "escaped").unwrap();
        std::os::unix::fs::symlink("../outside/marker.txt", workspace.join("escape.txt")).unwrap();

        let kitchen = Kitchen::new(KitchenConfig {
            recipe_source_base_dir: Some(recipe_dir),
            use_isolation: true,
            ..KitchenConfig::default()
        });
        let mut recipe = minimal_recipe();
        recipe.source = SourceSection::Local(LocalSourceSection {
            path: PathBuf::from("./src"),
        });

        let mut cook = Cook::new(&kitchen, &recipe).unwrap();
        let error = cook.prep().unwrap_err();

        assert!(
            error
                .to_string()
                .contains("Local source symlink must stay within the source directory"),
            "expected nested symlink escape rejection, got: {error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_prep_isolated_local_path_source_rejects_absolute_symlink_escape() {
        let dir = tempfile::tempdir().unwrap();
        let recipe_dir = dir.path().join("recipe");
        let workspace = recipe_dir.join("src");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let escaped = outside.join("marker.txt");
        std::fs::write(&escaped, "escaped").unwrap();
        std::os::unix::fs::symlink(&escaped, workspace.join("escape.txt")).unwrap();

        let kitchen = Kitchen::new(KitchenConfig {
            recipe_source_base_dir: Some(recipe_dir),
            use_isolation: true,
            ..KitchenConfig::default()
        });
        let mut recipe = minimal_recipe();
        recipe.source = SourceSection::Local(LocalSourceSection {
            path: PathBuf::from("./src"),
        });

        let mut cook = Cook::new(&kitchen, &recipe).unwrap();
        let error = cook.prep().unwrap_err();

        assert!(
            error
                .to_string()
                .contains("Local source symlink must stay within the source directory"),
            "expected absolute symlink escape rejection, got: {error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_prep_local_path_source_rejects_symlink_escape() {
        let dir = tempfile::tempdir().unwrap();
        let recipe_dir = dir.path().join("recipe");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&recipe_dir).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("marker.txt"), "escaped").unwrap();
        std::os::unix::fs::symlink(&outside, recipe_dir.join("src")).unwrap();

        let kitchen = Kitchen::new(KitchenConfig {
            recipe_source_base_dir: Some(recipe_dir),
            use_isolation: false,
            ..KitchenConfig::default()
        });
        let mut recipe = minimal_recipe();
        recipe.source = SourceSection::Local(LocalSourceSection {
            path: PathBuf::from("./src"),
        });

        let mut cook = Cook::new(&kitchen, &recipe).unwrap();
        let error = cook.prep().unwrap_err();

        assert!(
            error
                .to_string()
                .contains("must stay within the recipe directory"),
            "expected symlink escape rejection, got: {error}"
        );
    }

    #[test]
    fn test_chroot_command_translation_maps_destdir_substitutions() {
        let sysroot = Path::new("/tmp/conary-seed/sysroot");
        let command = "mkdir -p /tmp/conary-seed/sysroot/var/tmp/dest && touch /tmp/conary-seed/sysroot/var/tmp/dest/ok";

        assert_eq!(
            translate_command_for_chroot(command, sysroot),
            "mkdir -p /var/tmp/dest && touch /var/tmp/dest/ok"
        );
    }

    fn dummy_hermetic_evidence() -> HermeticBuildEvidence {
        HermeticBuildEvidence {
            schema_version: HERMETIC_EVIDENCE_SCHEMA_V1,
            build_input: BuildInputIdentity {
                recipe: RecipeIdentity::ExplicitRecipe {
                    path: "recipe.toml".to_string(),
                    hash: "sha256:recipe".to_string(),
                },
                source: SourceIdentity::Archive {
                    url: "https://example.invalid/test.tar.gz".to_string(),
                    checksum: "sha256:source".to_string(),
                },
                additional_sources: Vec::new(),
                patches: Vec::new(),
                local_tree: None,
                ecosystem_dependencies: Vec::new(),
                builder_environment: BuilderEnvironmentIdentity {
                    kind: BuilderEnvironmentKind::Pristine,
                    sysroot_hash: Some(
                        "sha256:1111111111111111111111111111111111111111111111111111111111111111"
                            .to_string(),
                    ),
                    toolchain_hash: None,
                    diagnostics: Vec::new(),
                },
            },
            dependency_lock: DependencyLock::default(),
            ecosystem_policy: EcosystemPolicyReport::clean("unknown"),
            command_risk: BuildCommandRiskReport::clean(),
            reproducibility: ReproducibilityRecord {
                source_date_epoch: Some(0),
                path_remap_count: 2,
                env_keys: vec![
                    "CFLAGS".to_string(),
                    "CXXFLAGS".to_string(),
                    "RUSTFLAGS".to_string(),
                    "SOURCE_DATE_EPOCH".to_string(),
                ],
            },
            diagnostics: Vec::new(),
        }
    }
}
