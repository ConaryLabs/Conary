// conary-core/src/activation/openrc.rs

//! Closed parser and invocation contract for OpenRC runtime service work.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OpenRcActivationAction {
    Start,
    Stop,
    Reload,
    Restart,
}

impl OpenRcActivationAction {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Reload => "reload",
            Self::Restart => "restart",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "start" => Some(Self::Start),
            "stop" => Some(Self::Stop),
            "reload" => Some(Self::Reload),
            "restart" => Some(Self::Restart),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OpenRcActivationCondition {
    IfCrashed,
    IfExists,
    IfInactive,
    IfNotStarted,
    IfStarted,
    IfStopped,
}

impl OpenRcActivationCondition {
    const fn as_arg(self) -> &'static str {
        match self {
            Self::IfCrashed => "--ifcrashed",
            Self::IfExists => "--ifexists",
            Self::IfInactive => "--ifinactive",
            Self::IfNotStarted => "--ifnotstarted",
            Self::IfStarted => "--ifstarted",
            Self::IfStopped => "--ifstopped",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenRcActivationInvocation {
    pub action: OpenRcActivationAction,
    pub service: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<OpenRcActivationCondition>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arguments: Vec<String>,
    #[serde(default)]
    pub debug: bool,
    #[serde(default)]
    pub nodeps: bool,
    #[serde(default)]
    pub nocolor: bool,
    #[serde(default)]
    pub verbose: bool,
    #[serde(default)]
    pub quiet: u8,
}

impl OpenRcActivationInvocation {
    pub fn simple(action: OpenRcActivationAction, service: impl Into<String>) -> Self {
        Self {
            action,
            service: service.into(),
            conditions: Vec::new(),
            arguments: Vec::new(),
            debug: false,
            nodeps: false,
            nocolor: false,
            verbose: false,
            quiet: 0,
        }
    }

    pub fn try_restart(service: impl Into<String>) -> Self {
        let mut invocation = Self::simple(OpenRcActivationAction::Restart, service);
        invocation
            .conditions
            .push(OpenRcActivationCondition::IfStarted);
        invocation
    }

    pub fn rc_service_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        if self.debug {
            args.push("--debug".to_string());
        }
        if self.nodeps {
            args.push("--nodeps".to_string());
        }
        if self.nocolor {
            args.push("--nocolor".to_string());
        }
        if self.verbose {
            args.push("--verbose".to_string());
        }
        args.extend(std::iter::repeat_n(
            "--quiet".to_string(),
            self.quiet as usize,
        ));
        args.extend(
            self.conditions
                .iter()
                .map(|condition| condition.as_arg().to_string()),
        );
        args.push("--".to_string());
        args.push(self.service.clone());
        args.push(self.action.as_str().to_string());
        args.extend(self.arguments.iter().cloned());
        args
    }

    pub fn validate(&self) -> Result<(), OpenRcActivationParseError> {
        validate_service(&self.service)?;
        if self
            .arguments
            .iter()
            .any(|argument| argument.contains('\0'))
        {
            return Err(OpenRcActivationParseError::NulArgument);
        }
        let mut conditions = self.conditions.clone();
        conditions.sort_by_key(|condition| *condition as u8);
        conditions.dedup();
        if conditions.len() != self.conditions.len() {
            return Err(OpenRcActivationParseError::DuplicateCondition);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OpenRcActivationParseError {
    #[error("rc-service activation argv is empty")]
    EmptyArgv,
    #[error("rc-service option '{option}' is outside the generation activation contract")]
    UnsupportedOption { option: String },
    #[error("rc-service activation requires one service and command")]
    MissingServiceOrCommand,
    #[error("rc-service option '{option}' requires a value")]
    MissingOptionValue { option: String },
    #[error("OpenRC service must be a pathless, nonempty name of at most 255 bytes")]
    InvalidService,
    #[error("rc-service activation argument contains a NUL byte")]
    NulArgument,
    #[error("rc-service activation repeats one conditional option")]
    DuplicateCondition,
}

#[derive(Default)]
struct ParsedOptions {
    conditions: Vec<OpenRcActivationCondition>,
    debug: bool,
    nodeps: bool,
    nocolor: bool,
    verbose: bool,
    quiet: u8,
    dry_run: bool,
    unsupported: Option<String>,
}

/// Parse argv observed at an actual `rc-service` process boundary.
///
/// `Ok(None)` denotes an OpenRC query/utility mode, an unknown service command,
/// or an explicit dry run. A recognized runtime action with an unsupported
/// option fails closed.
pub fn parse_openrc_activation_invocation(
    argv: &[String],
) -> Result<Option<OpenRcActivationInvocation>, OpenRcActivationParseError> {
    if argv.is_empty() {
        return Err(OpenRcActivationParseError::EmptyArgv);
    }
    let mut options = ParsedOptions::default();
    let mut positional = Vec::new();
    let mut positional_only = false;
    let mut index = 0;
    while index < argv.len() {
        let token = &argv[index];
        if positional_only || !token.starts_with('-') || token == "-" {
            positional.push(token.clone());
            index += 1;
            continue;
        }
        if token == "--" {
            positional_only = true;
            index += 1;
            continue;
        }
        match token.as_str() {
            "--help" | "--list" => return Ok(None),
            "--version" if argv.len() == 1 => return Ok(None),
            "--version" => {}
            value if value.starts_with("--exists=") || value.starts_with("--resolve=") => {
                return Ok(None);
            }
            "--exists" | "--resolve" => {
                if argv.get(index + 1).is_none() {
                    return Err(OpenRcActivationParseError::MissingOptionValue {
                        option: token.clone(),
                    });
                }
                return Ok(None);
            }
            "--debug" => options.debug = true,
            "--nodeps" => options.nodeps = true,
            "--ifcrashed" => options
                .conditions
                .push(OpenRcActivationCondition::IfCrashed),
            "--ifexists" => options.conditions.push(OpenRcActivationCondition::IfExists),
            "--ifinactive" => options
                .conditions
                .push(OpenRcActivationCondition::IfInactive),
            "--ifnotstarted" => options
                .conditions
                .push(OpenRcActivationCondition::IfNotStarted),
            "--ifstarted" => options
                .conditions
                .push(OpenRcActivationCondition::IfStarted),
            "--ifstopped" => options
                .conditions
                .push(OpenRcActivationCondition::IfStopped),
            "--dry-run" => options.dry_run = true,
            "--nocolor" => options.nocolor = true,
            "--verbose" => options.verbose = true,
            "--quiet" => options.quiet = options.quiet.saturating_add(1),
            "--user" => options.unsupported = Some(token.clone()),
            _ if token.starts_with("--") => {
                return Err(OpenRcActivationParseError::UnsupportedOption {
                    option: token.clone(),
                });
            }
            _ if parse_short_options(
                token,
                &mut options,
                argv.len() == 1,
                index + 1 < argv.len(),
            )? =>
            {
                return Ok(None);
            }
            _ => {}
        }
        index += 1;
    }

    let Some(service) = positional.first() else {
        return Err(OpenRcActivationParseError::MissingServiceOrCommand);
    };
    let Some(command) = positional.get(1) else {
        return Err(OpenRcActivationParseError::MissingServiceOrCommand);
    };
    let Some(action) = OpenRcActivationAction::parse(command) else {
        return Ok(None);
    };
    if options.dry_run {
        return Ok(None);
    }
    if let Some(option) = options.unsupported {
        return Err(OpenRcActivationParseError::UnsupportedOption { option });
    }
    let mut conditions = Vec::new();
    for condition in options.conditions {
        if !conditions.contains(&condition) {
            conditions.push(condition);
        }
    }
    let invocation = OpenRcActivationInvocation {
        action,
        service: service.clone(),
        conditions,
        arguments: positional[2..].to_vec(),
        debug: options.debug,
        nodeps: options.nodeps,
        nocolor: options.nocolor,
        verbose: options.verbose,
        quiet: options.quiet,
    };
    invocation.validate()?;
    Ok(Some(invocation))
}

fn parse_short_options(
    token: &str,
    options: &mut ParsedOptions,
    version_only: bool,
    has_next_argument: bool,
) -> Result<bool, OpenRcActivationParseError> {
    let mut characters = token
        .strip_prefix('-')
        .unwrap_or_default()
        .chars()
        .peekable();
    while let Some(option) = characters.next() {
        match option {
            'h' | 'l' => return Ok(true),
            'V' if version_only => return Ok(true),
            'V' => {}
            'e' | 'r' => {
                if characters.peek().is_none() && !has_next_argument {
                    return Err(OpenRcActivationParseError::MissingOptionValue {
                        option: format!("-{option}"),
                    });
                }
                return Ok(true);
            }
            'c' => options
                .conditions
                .push(OpenRcActivationCondition::IfCrashed),
            'd' => options.debug = true,
            'D' => options.nodeps = true,
            'i' => options.conditions.push(OpenRcActivationCondition::IfExists),
            'I' => options
                .conditions
                .push(OpenRcActivationCondition::IfInactive),
            'N' => options
                .conditions
                .push(OpenRcActivationCondition::IfNotStarted),
            's' => options
                .conditions
                .push(OpenRcActivationCondition::IfStarted),
            'S' => options
                .conditions
                .push(OpenRcActivationCondition::IfStopped),
            'Z' => options.dry_run = true,
            'C' => options.nocolor = true,
            'v' => options.verbose = true,
            'q' => options.quiet = options.quiet.saturating_add(1),
            'U' => options.unsupported = Some(format!("-{option}")),
            _ => {
                return Err(OpenRcActivationParseError::UnsupportedOption {
                    option: format!("-{option}"),
                });
            }
        }
    }
    Ok(false)
}

fn validate_service(service: &str) -> Result<(), OpenRcActivationParseError> {
    if service.is_empty()
        || service.len() > 255
        || service.contains(['/', '\\', '\0'])
        || matches!(service, "." | "..")
    {
        return Err(OpenRcActivationParseError::InvalidService);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conditional_restart_is_preserved_as_exact_rc_service_argv() {
        let invocation = parse_openrc_activation_invocation(
            &["--nodeps", "--ifstarted", "sshd", "restart", "graceful"].map(str::to_string),
        )
        .unwrap()
        .unwrap();

        assert_eq!(
            invocation.rc_service_args(),
            [
                "--nodeps",
                "--ifstarted",
                "--",
                "sshd",
                "restart",
                "graceful"
            ]
            .map(str::to_string)
        );
    }

    #[test]
    fn query_and_explicit_dry_run_do_not_create_activation_work() {
        for argv in [
            vec!["--list"],
            vec!["sshd", "status"],
            vec!["--dry-run", "sshd", "restart"],
        ] {
            assert!(
                parse_openrc_activation_invocation(
                    &argv.into_iter().map(str::to_string).collect::<Vec<_>>()
                )
                .unwrap()
                .is_none()
            );
        }
    }

    #[test]
    fn getopt_queries_and_version_follow_upstream_exit_order() {
        for argv in [
            vec!["-de", "sshd"],
            vec!["-rnetworking"],
            vec!["-V"],
            vec!["--version"],
        ] {
            assert!(
                parse_openrc_activation_invocation(
                    &argv.into_iter().map(str::to_string).collect::<Vec<_>>()
                )
                .unwrap()
                .is_none()
            );
        }

        let version_with_action =
            parse_openrc_activation_invocation(&["-V", "sshd", "restart"].map(str::to_string))
                .unwrap()
                .unwrap();
        assert_eq!(version_with_action.action, OpenRcActivationAction::Restart);

        let error =
            parse_openrc_activation_invocation(&["-xe", "sshd"].map(str::to_string)).unwrap_err();
        assert!(matches!(
            error,
            OpenRcActivationParseError::UnsupportedOption { ref option } if option == "-x"
        ));
    }

    #[test]
    fn repeated_boolean_conditions_are_canonicalized() {
        let invocation = parse_openrc_activation_invocation(
            &["--ifstarted", "-ss", "sshd", "restart"].map(str::to_string),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            invocation.conditions,
            vec![OpenRcActivationCondition::IfStarted]
        );
    }

    #[test]
    fn openrc_invocation_round_trips_as_a_closed_runtime_variant() {
        let invocation = crate::activation::RuntimeActivationInvocation::OpenRc(
            OpenRcActivationInvocation::simple(OpenRcActivationAction::Reload, "networking"),
        );
        let encoded = serde_json::to_string(&invocation).unwrap();

        assert_eq!(
            serde_json::from_str::<crate::activation::RuntimeActivationInvocation>(&encoded)
                .unwrap(),
            invocation
        );
        assert!(encoded.contains("\"kind\":\"open-rc\""));
    }

    #[test]
    fn openrc_service_operand_cannot_be_a_path() {
        assert!(
            OpenRcActivationInvocation::simple(OpenRcActivationAction::Start, "../sshd")
                .validate()
                .is_err()
        );
    }
}
