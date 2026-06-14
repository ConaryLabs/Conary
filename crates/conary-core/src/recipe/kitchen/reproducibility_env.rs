// conary-core/src/recipe/kitchen/reproducibility_env.rs

//! Hermetic command-local reproducibility environment validation.

use crate::ccs::convert::command_evidence::extract_invocations_from_shell_text;
use crate::error::{Error, Result};
use crate::recipe::hermetic::ReproducibilityConfig;

pub(super) fn validate_command_local_reproducibility_env(
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

fn nested_shell_stdin_error(phase: &str, shell: &str) -> Error {
    Error::ConfigError(format!(
        "hermetic reproducibility rejects nested shell {shell} reading script from stdin in {phase} phase"
    ))
}

fn nested_shell_script_error(phase: &str, shell: &str, script: &str) -> Error {
    Error::ConfigError(format!(
        "hermetic reproducibility rejects nested shell {shell} script operand {script} in {phase} phase"
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
        if let Some(identifier_start) = start.take()
            && &expression[identifier_start..index] == name
        {
            return true;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

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
            ("coproc SOURCE_DATE_EPOCH=999 make -s; wait", "coproc"),
            ("coproc make -s SOURCE_DATE_EPOCH=999; wait", "coproc"),
            ("sh -c 'SOURCE_DATE_EPOCH=999 make'", "-c"),
            ("/bin/sh -c 'env -u SOURCE_DATE_EPOCH make'", "-c"),
            ("bash -ec 'SOURCE_DATE_EPOCH=999 make'", "-ec"),
            (
                "printf %s \"export SOURCE_DATE_EPOCH=999; make -s\" | sh",
                "stdin",
            ),
            (
                "echo \"export SOURCE_DATE_EPOCH=999; make -s\" | sh",
                "stdin",
            ),
            (
                "printf %s \"export SOURCE_DATE_EPOCH=999; make -s\" | bash",
                "stdin",
            ),
            ("sh < build.sh", "stdin"),
            ("bash -s < build.sh", "stdin"),
            ("sh build.sh", "script operand"),
            ("bash ./build.sh", "script operand"),
            ("busybox sh build.sh", "script operand"),
            (
                "printf %s \"export SOURCE_DATE_EPOCH=999; make -s\" > build.sh; sh build.sh",
                "script operand",
            ),
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
}
