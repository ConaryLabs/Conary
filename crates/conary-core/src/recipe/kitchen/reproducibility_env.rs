// conary-core/src/recipe/kitchen/reproducibility_env.rs

//! Hermetic command-local reproducibility environment validation.

mod shell_syntax;

use shell_syntax::*;

use crate::ccs::convert::command_evidence::extract_invocations_from_shell_text;
use crate::error::{Error, Result};
use crate::recipe::hermetic::ReproducibilityConfig;

pub(super) fn validate_command_local_reproducibility_env(
    config: &ReproducibilityConfig,
    phase: &str,
    command: &str,
) -> Result<()> {
    validate_shell_env_mutations(config, phase, command)?;

    let invocations =
        extract_invocations_from_shell_text(phase, command, Some(phase)).map_err(|error| {
            Error::ConfigError(format!(
                "hermetic reproducibility could not parse {phase} phase shell: {error}"
            ))
        })?;
    for invocation in invocations {
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
    let mut end_options = false;

    for arg in args {
        validate_no_shell_expansion(phase, arg, shell)?;
        if arg.starts_with('<') {
            return Err(nested_shell_stdin_error(phase, shell));
        }
        if arg == "--" {
            end_options = true;
            continue;
        }
        if !end_options && shell_option_invokes_command_string(arg) {
            return Err(Error::ConfigError(format!(
                "hermetic reproducibility rejects nested shell {shell} {arg} invocation in {phase} phase"
            )));
        }
        if !end_options && shell_option_reads_stdin(arg) {
            return Err(nested_shell_stdin_error(phase, shell));
        }
        if !end_options && arg.starts_with('-') {
            continue;
        }
        return Err(nested_shell_script_error(phase, shell, arg));
    }

    Err(nested_shell_stdin_error(phase, shell))
}

fn shell_option_invokes_command_string(arg: &str) -> bool {
    arg.starts_with('-') && !arg.starts_with("--") && arg[1..].chars().any(|ch| ch == 'c')
}

fn shell_option_reads_stdin(arg: &str) -> bool {
    arg.starts_with('-') && !arg.starts_with("--") && arg[1..].chars().any(|ch| ch == 's')
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
        "for" | "case" | "select" | "function" | "coproc" => Err(Error::ConfigError(format!(
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

#[cfg(test)]
mod tests;
