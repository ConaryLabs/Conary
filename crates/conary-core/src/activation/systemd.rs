// conary-core/src/activation/systemd.rs

//! Closed parser for systemd unit activation operations.
//!
//! The action set is the `unit_actions` table in systemd's
//! `src/systemctl/systemctl-start-unit.c`. Legacy spellings are accepted only
//! where that table declares them as aliases, then normalized to the method's
//! canonical operation. This is a grammar adapter, not token classification.

use serde::{Deserialize, Serialize};
use std::path::Path;

mod grammar;

pub(crate) use grammar::systemctl_proxy_shell_scan;
use grammar::{ParsedVerb, SystemctlOptionGrammar, exact_option, prefixed_option};

/// Canonical systemd unit operation deferred until a generation boots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SystemdActivationAction {
    Start,
    Stop,
    Reload,
    Restart,
    TryRestart,
    ReloadOrRestart,
    ReloadOrTryRestart,
}

impl SystemdActivationAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Reload => "reload",
            Self::Restart => "restart",
            Self::TryRestart => "try-restart",
            Self::ReloadOrRestart => "reload-or-restart",
            Self::ReloadOrTryRestart => "reload-or-try-restart",
        }
    }
}

/// Unit-file operations whose documented `--now` half becomes runtime work.
///
/// This mapping is owned by systemd's `verb_enable()` implementation:
/// enable starts, disable/mask stop, and reenable try-restarts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SystemdUnitFileNowAction {
    Enable,
    Disable,
    Reenable,
    Mask,
}

impl SystemdUnitFileNowAction {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Enable => "enable",
            Self::Disable => "disable",
            Self::Reenable => "reenable",
            Self::Mask => "mask",
        }
    }

    const fn runtime_action(self) -> SystemdActivationAction {
        match self {
            Self::Enable => SystemdActivationAction::Start,
            Self::Disable | Self::Mask => SystemdActivationAction::Stop,
            Self::Reenable => SystemdActivationAction::TryRestart,
        }
    }

    const fn accepts_path(self) -> bool {
        matches!(self, Self::Enable | Self::Disable | Self::Reenable)
    }
}

/// Exact systemd job transaction mode preserved from `--job-mode=`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SystemdJobMode {
    Fail,
    Replace,
    ReplaceIrreversibly,
    Isolate,
    IgnoreDependencies,
    IgnoreRequirements,
    Flush,
    Trigger,
    RestartDependencies,
    ReplaceDependencies,
}

impl SystemdJobMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fail => "fail",
            Self::Replace => "replace",
            Self::ReplaceIrreversibly => "replace-irreversibly",
            Self::Isolate => "isolate",
            Self::IgnoreDependencies => "ignore-dependencies",
            Self::IgnoreRequirements => "ignore-requirements",
            Self::Flush => "flush",
            Self::Trigger => "trigger",
            Self::RestartDependencies => "restart-dependencies",
            Self::ReplaceDependencies => "replace-dependencies",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "fail" => Some(Self::Fail),
            "replace" => Some(Self::Replace),
            "replace-irreversibly" => Some(Self::ReplaceIrreversibly),
            "isolate" => Some(Self::Isolate),
            "ignore-dependencies" => Some(Self::IgnoreDependencies),
            "ignore-requirements" => Some(Self::IgnoreRequirements),
            "flush" => Some(Self::Flush),
            "trigger" => Some(Self::Trigger),
            "restart-dependencies" => Some(Self::RestartDependencies),
            "replace-dependencies" => Some(Self::ReplaceDependencies),
            _ => None,
        }
    }
}

/// One exact system-scope invocation of systemd's unit activation grammar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemdActivationInvocation {
    pub action: SystemdActivationAction,
    pub units: Vec<String>,
    pub job_mode: Option<SystemdJobMode>,
    pub all: bool,
    pub no_block: bool,
    pub no_ask_password: bool,
    pub quiet: bool,
    pub no_warn: bool,
    pub wait: bool,
    pub show_transaction: bool,
}

impl SystemdActivationInvocation {
    /// Canonical argv for the booted generation's system `systemctl`.
    pub fn systemctl_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        args.push("--system".to_string());
        if let Some(mode) = self.job_mode {
            args.push(format!("--job-mode={}", mode.as_str()));
        }
        if self.all {
            args.push("--all".to_string());
        }
        if self.no_block {
            args.push("--no-block".to_string());
        }
        if self.no_ask_password {
            args.push("--no-ask-password".to_string());
        }
        if self.quiet {
            args.push("--quiet".to_string());
        }
        if self.no_warn {
            args.push("--no-warn".to_string());
        }
        if self.wait {
            args.push("--wait".to_string());
        }
        if self.show_transaction {
            args.push("--show-transaction".to_string());
        }
        args.push(self.action.as_str().to_string());
        args.push("--".to_string());
        args.extend(self.units.iter().cloned());
        args
    }

    pub fn validate(&self) -> Result<(), SystemdActivationParseError> {
        if self.units.is_empty() {
            return Err(SystemdActivationParseError::MissingUnit {
                action: self.action,
            });
        }
        if self.wait && self.no_block {
            return Err(SystemdActivationParseError::WaitWithNoBlock);
        }
        if self.wait
            && !matches!(
                self.action,
                SystemdActivationAction::Start | SystemdActivationAction::Restart
            )
        {
            return Err(SystemdActivationParseError::WaitUnsupported {
                action: self.action,
            });
        }
        for unit in &self.units {
            validate_exact_unit(unit)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SystemdActivationParseError {
    #[error("systemctl activation argv is empty")]
    EmptyArgv,
    #[error("systemctl option '{option}' is outside the generation activation contract")]
    UnsupportedOption { option: String },
    #[error("systemctl --job-mode requires a value")]
    MissingJobMode,
    #[error("systemctl option '{option}' requires a value")]
    MissingOptionValue { option: String },
    #[error("systemctl job mode '{mode}' is outside the documented job-mode grammar")]
    UnsupportedJobMode { mode: String },
    #[error("systemctl action {action:?} requires at least one exact unit")]
    MissingUnit { action: SystemdActivationAction },
    #[error("systemctl --wait is not valid for deferred {action:?}")]
    WaitUnsupported { action: SystemdActivationAction },
    #[error("systemctl --wait cannot be combined with --no-block")]
    WaitWithNoBlock,
    #[error("systemctl activation unit is empty")]
    EmptyUnit,
    #[error("systemctl activation unit contains a NUL byte")]
    NulUnit,
    #[error("systemctl {action} --now path '{unit}' has no unit-file name")]
    InvalidUnitFilePath { action: String, unit: String },
}

#[derive(Debug, Default)]
struct ParsedOptions {
    job_mode: Option<SystemdJobMode>,
    all: bool,
    no_block: bool,
    no_ask_password: bool,
    quiet: bool,
    no_warn: bool,
    wait: bool,
    show_transaction: bool,
    dry_run: bool,
    now: bool,
    unsupported_target: Option<String>,
}

/// Parse raw arguments captured at the actual `systemctl` execution boundary.
///
/// `Ok(None)` means the invocation is not one of systemd's unit activation
/// verbs, or it explicitly used `--dry-run`. Unsupported options on an
/// activation verb are errors rather than silently changing its semantics.
pub fn parse_systemctl_activation_invocation(
    argv: &[String],
) -> Result<Option<SystemdActivationInvocation>, SystemdActivationParseError> {
    if argv.is_empty() {
        return Err(SystemdActivationParseError::EmptyArgv);
    }

    let mut verb = None;
    let mut options = ParsedOptions::default();
    let mut units = Vec::new();
    let mut positional_only = false;
    let mut index = 0;

    while index < argv.len() {
        let token = &argv[index];
        if verb.is_none() && positional_only {
            let Some(parsed) = ParsedVerb::parse(token) else {
                return Ok(None);
            };
            verb = Some(parsed);
            index += 1;
            continue;
        }
        if verb.is_some() && positional_only {
            units.push(token.clone());
            index += 1;
            continue;
        }
        if token == "--" {
            positional_only = true;
            index += 1;
            continue;
        }
        if !token.starts_with('-') {
            if verb.is_none() {
                let Some(parsed) = ParsedVerb::parse(token) else {
                    return Ok(None);
                };
                verb = Some(parsed);
            } else {
                units.push(token.clone());
            }
            index += 1;
            continue;
        }

        if let Some(option) = exact_option(token) {
            match option {
                SystemctlOptionGrammar::System | SystemctlOptionGrammar::AcceptedNoEffect => {}
                SystemctlOptionGrammar::All => options.all = true,
                SystemctlOptionGrammar::NoBlock => options.no_block = true,
                SystemctlOptionGrammar::NoAskPassword => options.no_ask_password = true,
                SystemctlOptionGrammar::Quiet => options.quiet = true,
                SystemctlOptionGrammar::NoWarn => options.no_warn = true,
                SystemctlOptionGrammar::Wait => options.wait = true,
                SystemctlOptionGrammar::ShowTransaction => options.show_transaction = true,
                SystemctlOptionGrammar::DryRun => options.dry_run = true,
                SystemctlOptionGrammar::Now => options.now = true,
                SystemctlOptionGrammar::Fail => {
                    options.job_mode = Some(SystemdJobMode::Fail);
                }
                SystemctlOptionGrammar::Irreversible => {
                    options.job_mode = Some(SystemdJobMode::ReplaceIrreversibly);
                }
                SystemctlOptionGrammar::IgnoreDependencies => {
                    options.job_mode = Some(SystemdJobMode::IgnoreDependencies);
                }
                SystemctlOptionGrammar::JobModeValue => {
                    index += 1;
                    let value = argv
                        .get(index)
                        .ok_or(SystemdActivationParseError::MissingJobMode)?;
                    options.job_mode = Some(SystemdJobMode::parse(value).ok_or_else(|| {
                        SystemdActivationParseError::UnsupportedJobMode {
                            mode: value.clone(),
                        }
                    })?);
                }
                SystemctlOptionGrammar::UnsupportedTarget => {
                    options.unsupported_target = Some(token.clone());
                }
                SystemctlOptionGrammar::UnsupportedTargetValue => {
                    index += 1;
                    if argv.get(index).is_none() {
                        return Err(SystemdActivationParseError::MissingOptionValue {
                            option: token.clone(),
                        });
                    }
                    options.unsupported_target = Some(token.clone());
                }
            }
            index += 1;
            continue;
        }
        if let Some((option, value)) = prefixed_option(token) {
            match option {
                SystemctlOptionGrammar::JobModeValue => {
                    options.job_mode = Some(SystemdJobMode::parse(value).ok_or_else(|| {
                        SystemdActivationParseError::UnsupportedJobMode {
                            mode: value.to_string(),
                        }
                    })?);
                }
                SystemctlOptionGrammar::UnsupportedTarget => {
                    options.unsupported_target = Some(token.clone());
                }
                _ => unreachable!("prefixed systemctl grammar has a closed semantic set"),
            }
            index += 1;
            continue;
        }
        if verb.is_none() {
            // Without a documented option grammar, a later token cannot be
            // trusted as the command. The generated proxy suppresses this
            // ambiguous call and the parent fails the transaction here.
            return Err(SystemdActivationParseError::UnsupportedOption {
                option: token.clone(),
            });
        }
        return Err(SystemdActivationParseError::UnsupportedOption {
            option: token.clone(),
        });
    }

    if options.dry_run {
        return Ok(None);
    }
    let Some(verb) = verb else {
        return Ok(None);
    };
    if let Some(option) = options.unsupported_target {
        return Err(SystemdActivationParseError::UnsupportedOption { option });
    }
    let action = match verb {
        ParsedVerb::Runtime(action) => action,
        ParsedVerb::UnitFile(_) if !options.now => return Ok(None),
        ParsedVerb::UnitFile(action) => {
            units = normalize_unit_file_now_units(action, units)?;
            if units.is_empty() {
                return Ok(None);
            }
            action.runtime_action()
        }
    };
    let invocation = SystemdActivationInvocation {
        action,
        units,
        job_mode: options.job_mode,
        all: options.all,
        no_block: options.no_block,
        no_ask_password: options.no_ask_password,
        quiet: options.quiet,
        no_warn: options.no_warn,
        wait: options.wait,
        show_transaction: options.show_transaction,
    };
    invocation.validate()?;
    Ok(Some(invocation))
}

fn validate_exact_unit(unit: &str) -> Result<(), SystemdActivationParseError> {
    if unit.is_empty() {
        return Err(SystemdActivationParseError::EmptyUnit);
    }
    if unit.contains('\0') {
        return Err(SystemdActivationParseError::NulUnit);
    }
    Ok(())
}

fn normalize_unit_file_now_units(
    action: SystemdUnitFileNowAction,
    units: Vec<String>,
) -> Result<Vec<String>, SystemdActivationParseError> {
    let mut normalized = Vec::with_capacity(units.len());
    for unit in units {
        let unit = if action.accepts_path() && unit.contains('/') {
            Path::new(&unit)
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
                .ok_or_else(|| SystemdActivationParseError::InvalidUnitFilePath {
                    action: action.as_str().to_string(),
                    unit: unit.clone(),
                })?
        } else if unit.contains('/') {
            return Err(SystemdActivationParseError::InvalidUnitFilePath {
                action: action.as_str().to_string(),
                unit,
            });
        } else {
            unit
        };
        if let Some(template) = systemd_template_runtime_operand(&unit) {
            if action == SystemdUnitFileNowAction::Enable {
                // systemd enables DefaultInstance= but intentionally skips the
                // runtime start for template names.
                continue;
            }
            normalized.push(template);
        } else {
            validate_exact_unit(&unit)?;
            normalized.push(unit);
        }
    }
    Ok(normalized)
}

fn systemd_template_runtime_operand(unit: &str) -> Option<String> {
    let (stem, suffix) = unit.rsplit_once('@')?;
    if !suffix.starts_with('.') || suffix.len() == 1 {
        return None;
    }
    Some(format!("{stem}@*{suffix}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn exact_current_systemd_verbs_normalize_only_documented_aliases() {
        let cases = [
            ("start", SystemdActivationAction::Start),
            ("stop", SystemdActivationAction::Stop),
            ("condstop", SystemdActivationAction::Stop),
            ("reload", SystemdActivationAction::Reload),
            ("restart", SystemdActivationAction::Restart),
            ("try-restart", SystemdActivationAction::TryRestart),
            ("condrestart", SystemdActivationAction::TryRestart),
            (
                "reload-or-restart",
                SystemdActivationAction::ReloadOrRestart,
            ),
            (
                "try-reload-or-restart",
                SystemdActivationAction::ReloadOrTryRestart,
            ),
            (
                "reload-or-try-restart",
                SystemdActivationAction::ReloadOrTryRestart,
            ),
            ("condreload", SystemdActivationAction::ReloadOrTryRestart),
            ("force-reload", SystemdActivationAction::ReloadOrTryRestart),
        ];
        for (verb, expected) in cases {
            let parsed =
                parse_systemctl_activation_invocation(&args(&["--system", verb, "demo.service"]))
                    .unwrap()
                    .unwrap();
            assert_eq!(parsed.action, expected, "{verb}");
        }
    }

    #[test]
    fn typed_options_and_multi_unit_transaction_round_trip_to_canonical_argv() {
        let parsed = parse_systemctl_activation_invocation(&args(&[
            "--no-ask-password",
            "--job-mode=replace",
            "--all",
            "restart",
            "--no-block",
            "--",
            "api.service",
            "worker.service",
        ]))
        .unwrap()
        .unwrap();

        assert_eq!(
            parsed.systemctl_args(),
            args(&[
                "--system",
                "--job-mode=replace",
                "--all",
                "--no-block",
                "--no-ask-password",
                "restart",
                "--",
                "api.service",
                "worker.service",
            ])
        );
    }

    #[test]
    fn captured_runtime_operands_remain_exact_systemctl_authority() {
        let parsed = parse_systemctl_activation_invocation(&args(&[
            "restart",
            "../literal-systemctl-operand.service",
        ]))
        .unwrap()
        .unwrap();

        assert_eq!(parsed.units, vec!["../literal-systemctl-operand.service"]);
        assert_eq!(
            parsed.systemctl_args(),
            args(&[
                "--system",
                "restart",
                "--",
                "../literal-systemctl-operand.service",
            ])
        );
    }

    #[test]
    fn dry_run_and_non_activation_commands_do_not_create_intents() {
        assert!(
            parse_systemctl_activation_invocation(&args(&["--dry-run", "restart", "demo.service"]))
                .unwrap()
                .is_none()
        );
        assert!(
            parse_systemctl_activation_invocation(&args(&["daemon-reload"]))
                .unwrap()
                .is_none()
        );
        assert!(
            parse_systemctl_activation_invocation(&args(&["is-active", "restart"]))
                .unwrap()
                .is_none(),
            "a non-activation command operand must not become a verb"
        );
        let ambiguous = parse_systemctl_activation_invocation(&args(&[
            "--property",
            "restart",
            "show",
            "demo.service",
        ]))
        .unwrap_err();
        assert!(matches!(
            ambiguous,
            SystemdActivationParseError::UnsupportedOption { .. }
        ));
    }

    #[test]
    fn unsupported_scope_is_typed_only_for_activation_commands() {
        assert!(matches!(
            parse_systemctl_activation_invocation(&args(&["--host"])),
            Err(SystemdActivationParseError::MissingOptionValue { .. })
        ));
        let scope =
            parse_systemctl_activation_invocation(&args(&["--user", "restart", "demo.service"]))
                .unwrap_err();
        assert!(matches!(
            scope,
            SystemdActivationParseError::UnsupportedOption { .. }
        ));
        assert!(
            parse_systemctl_activation_invocation(&args(&["--user", "is-active", "demo.service"]))
                .unwrap()
                .is_none()
        );
        let glob = parse_systemctl_activation_invocation(&args(&["restart", "demo@*.service"]))
            .unwrap()
            .unwrap();
        assert_eq!(glob.units, vec!["demo@*.service"]);
    }

    #[test]
    fn documented_unit_file_now_actions_emit_their_exact_runtime_halves() {
        let cases = [
            ("enable", SystemdActivationAction::Start),
            ("disable", SystemdActivationAction::Stop),
            ("mask", SystemdActivationAction::Stop),
            ("reenable", SystemdActivationAction::TryRestart),
        ];
        for (verb, expected) in cases {
            let parsed = parse_systemctl_activation_invocation(&args(&[
                "--no-reload",
                verb,
                "--now",
                "demo.service",
            ]))
            .unwrap()
            .unwrap();
            assert_eq!(parsed.action, expected, "{verb}");
            assert_eq!(parsed.units, vec!["demo.service"], "{verb}");
        }
    }

    #[test]
    fn unit_file_now_preserves_systemd_path_and_template_normalization() {
        let from_path = parse_systemctl_activation_invocation(&args(&[
            "enable",
            "--now",
            "/usr/lib/systemd/system/demo.service",
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(from_path.units, vec!["demo.service"]);

        assert!(
            parse_systemctl_activation_invocation(&args(&["enable", "--now", "worker@.service"]))
                .unwrap()
                .is_none(),
            "systemd intentionally skips starting an enabled template"
        );
        let reenabled_template =
            parse_systemctl_activation_invocation(&args(&["reenable", "--now", "worker@.service"]))
                .unwrap()
                .unwrap();
        assert_eq!(reenabled_template.units, vec!["worker@*.service"]);
    }

    #[test]
    fn missing_unit_and_invalid_wait_action_fail_closed() {
        assert!(matches!(
            parse_systemctl_activation_invocation(&args(&["restart"])).unwrap_err(),
            SystemdActivationParseError::MissingUnit { .. }
        ));
        assert!(matches!(
            parse_systemctl_activation_invocation(&args(&["--wait", "reload", "demo.service"]))
                .unwrap_err(),
            SystemdActivationParseError::WaitUnsupported { .. }
        ));
        assert!(matches!(
            parse_systemctl_activation_invocation(&args(&[
                "--wait",
                "--no-block",
                "restart",
                "demo.service"
            ]))
            .unwrap_err(),
            SystemdActivationParseError::WaitWithNoBlock
        ));
    }
}
