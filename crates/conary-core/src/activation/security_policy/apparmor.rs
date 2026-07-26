// conary-core/src/activation/security_policy/apparmor.rs

//! Closed AppArmor parser and mode-helper grammars from current upstream tools.

use super::{
    ActivationExecutableIdentity, SecurityPolicyActivationInvocation,
    SecurityPolicyInvocationDisposition, SecurityPolicyParseError, nonempty_value,
    safe_absolute_path, unsupported,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApparmorProgram {
    Parser,
    Enforce,
    Complain,
    Disable,
}

impl ApparmorProgram {
    pub const fn executable_name(self) -> &'static str {
        match self {
            Self::Parser => "apparmor_parser",
            Self::Enforce => "aa-enforce",
            Self::Complain => "aa-complain",
            Self::Disable => "aa-disable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum ApparmorActivationAction {
    ProfileReload { profiles: Vec<String> },
    ModeEnforce { targets: Vec<String> },
    ModeComplain { targets: Vec<String> },
    ProfileDisable { targets: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApparmorActivationInvocation {
    pub executable: ActivationExecutableIdentity,
    pub program: ApparmorProgram,
    pub action: ApparmorActivationAction,
    /// Exact arguments observed after argv[0].
    pub arguments: Vec<String>,
}

impl ApparmorActivationInvocation {
    pub fn validate(&self) -> Result<(), SecurityPolicyParseError> {
        self.executable.validate()?;
        let disposition = parse(
            self.program.executable_name(),
            &self.arguments,
            self.executable.clone(),
        )?;
        let SecurityPolicyInvocationDisposition::Generation(
            SecurityPolicyActivationInvocation::Apparmor(reparsed),
        ) = disposition
        else {
            return Err(unsupported(
                self.program.executable_name(),
                "persisted invocation no longer parses as live AppArmor work",
            ));
        };
        if reparsed != *self {
            return Err(unsupported(
                self.program.executable_name(),
                "persisted typed AppArmor action disagrees with exact argv",
            ));
        }
        Ok(())
    }
}

pub(super) fn parse(
    program: &str,
    arguments: &[String],
    executable: ActivationExecutableIdentity,
) -> Result<SecurityPolicyInvocationDisposition, SecurityPolicyParseError> {
    executable.validate()?;
    match program {
        "apparmor_parser" => parse_parser(arguments, executable),
        "aa-enforce" => parse_mode(ApparmorProgram::Enforce, arguments, executable),
        "aa-complain" => parse_mode(ApparmorProgram::Complain, arguments, executable),
        "aa-disable" => parse_mode(ApparmorProgram::Disable, arguments, executable),
        _ => Err(unsupported(program, "unknown AppArmor helper")),
    }
}

fn parse_parser(
    arguments: &[String],
    executable: ActivationExecutableIdentity,
) -> Result<SecurityPolicyInvocationDisposition, SecurityPolicyParseError> {
    let mut replace = false;
    let mut skip_kernel_load = false;
    let mut profiles = Vec::new();
    let mut positional_only = false;
    for argument in arguments {
        if !positional_only && argument == "--" {
            positional_only = true;
            continue;
        }
        if !positional_only {
            match argument.as_str() {
                "-r" | "--replace" => {
                    if replace {
                        return Err(unsupported("apparmor_parser", "replace option repeated"));
                    }
                    replace = true;
                    continue;
                }
                "-Q" | "--skip-kernel-load" => {
                    skip_kernel_load = true;
                    continue;
                }
                "-T" | "--skip-read-cache" | "-K" | "--skip-cache" | "-W" | "--write-cache"
                | "-v" | "--verbose" => continue,
                value if value.starts_with('-') => {
                    return Err(unsupported(
                        "apparmor_parser",
                        format!("unsupported option '{value}'"),
                    ));
                }
                _ => {}
            }
        }
        if !apparmor_profile_path(argument) {
            return Err(unsupported(
                "apparmor_parser",
                format!("profile '{argument}' is not a bounded AppArmor profile path"),
            ));
        }
        profiles.push(argument.clone());
    }
    if !replace || profiles.is_empty() {
        return Err(unsupported(
            "apparmor_parser",
            "replace plus at least one exact profile path are required",
        ));
    }
    if skip_kernel_load {
        return Ok(SecurityPolicyInvocationDisposition::SelectedRoot);
    }
    Ok(SecurityPolicyInvocationDisposition::Generation(
        SecurityPolicyActivationInvocation::Apparmor(ApparmorActivationInvocation {
            executable,
            program: ApparmorProgram::Parser,
            action: ApparmorActivationAction::ProfileReload { profiles },
            arguments: arguments.to_vec(),
        }),
    ))
}

fn parse_mode(
    program: ApparmorProgram,
    arguments: &[String],
    executable: ActivationExecutableIdentity,
) -> Result<SecurityPolicyInvocationDisposition, SecurityPolicyParseError> {
    let mut no_reload = false;
    let mut targets = Vec::new();
    let mut selector = None;
    let mut positional_only = false;
    let mut index = 0;
    while index < arguments.len() {
        let argument = &arguments[index];
        if !positional_only && argument == "--" {
            positional_only = true;
            index += 1;
            continue;
        }
        if !positional_only {
            match argument.as_str() {
                "--no-reload" => {
                    no_reload = true;
                    index += 1;
                    continue;
                }
                "-d" | "--dir" => {
                    index += 1;
                    let directory = arguments.get(index).ok_or_else(|| {
                        unsupported(
                            program.executable_name(),
                            "profile directory requires a value",
                        )
                    })?;
                    if !safe_absolute_path(directory) {
                        return Err(unsupported(
                            program.executable_name(),
                            "profile directory must be an absolute selected-root path",
                        ));
                    }
                    index += 1;
                    continue;
                }
                "--profile" | "--profile-file" | "--executable-file" => {
                    if selector.is_some() || !targets.is_empty() {
                        return Err(unsupported(
                            program.executable_name(),
                            "exactly one selector form may be used",
                        ));
                    }
                    index += 1;
                    let value = arguments.get(index).ok_or_else(|| {
                        unsupported(program.executable_name(), "selector requires a value")
                    })?;
                    validate_mode_target(value, program)?;
                    selector = Some(argument.as_str());
                    targets.push(value.clone());
                    index += 1;
                    continue;
                }
                "--force" if !matches!(program, ApparmorProgram::Disable) => {
                    index += 1;
                    continue;
                }
                value if value.starts_with('-') => {
                    return Err(unsupported(
                        program.executable_name(),
                        format!("unsupported option '{value}'"),
                    ));
                }
                _ => {}
            }
        }
        if selector.is_some() {
            return Err(unsupported(
                program.executable_name(),
                "selector option cannot be combined with positional targets",
            ));
        }
        validate_mode_target(argument, program)?;
        targets.push(argument.clone());
        index += 1;
    }
    if targets.is_empty() {
        return Err(unsupported(
            program.executable_name(),
            "at least one exact profile target is required",
        ));
    }
    if no_reload {
        return Ok(SecurityPolicyInvocationDisposition::SelectedRoot);
    }
    let action = match program {
        ApparmorProgram::Enforce => ApparmorActivationAction::ModeEnforce { targets },
        ApparmorProgram::Complain => ApparmorActivationAction::ModeComplain { targets },
        ApparmorProgram::Disable => ApparmorActivationAction::ProfileDisable { targets },
        ApparmorProgram::Parser => unreachable!("parser has a separate grammar"),
    };
    Ok(SecurityPolicyInvocationDisposition::Generation(
        SecurityPolicyActivationInvocation::Apparmor(ApparmorActivationInvocation {
            executable,
            program,
            action,
            arguments: arguments.to_vec(),
        }),
    ))
}

fn validate_mode_target(
    value: &str,
    program: ApparmorProgram,
) -> Result<(), SecurityPolicyParseError> {
    if !nonempty_value(value) || value == "-" {
        return Err(unsupported(
            program.executable_name(),
            format!("invalid profile target '{value}'"),
        ));
    }
    Ok(())
}

fn apparmor_profile_path(path: &str) -> bool {
    safe_absolute_path(path)
        && path
            .strip_prefix("/etc/apparmor.d/")
            .is_some_and(|suffix| !suffix.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn executable(program: &str) -> ActivationExecutableIdentity {
        ActivationExecutableIdentity {
            invoked_path: format!("/usr/sbin/{program}"),
            canonical_path: format!("/usr/sbin/{program}"),
            sha256: format!("sha256:{}", "b".repeat(64)),
        }
    }

    #[test]
    fn parser_replace_splits_skip_kernel_load_from_generation_reload() {
        let live = parse(
            "apparmor_parser",
            &strings(&["-r", "-T", "/etc/apparmor.d/usr.bin.demo"]),
            executable("apparmor_parser"),
        )
        .unwrap();
        assert!(matches!(
            live,
            SecurityPolicyInvocationDisposition::Generation(_)
        ));
        let offline = parse(
            "apparmor_parser",
            &strings(&["-Q", "-r", "/etc/apparmor.d/usr.bin.demo"]),
            executable("apparmor_parser"),
        )
        .unwrap();
        assert_eq!(offline, SecurityPolicyInvocationDisposition::SelectedRoot);
    }

    #[test]
    fn current_mode_helpers_are_typed_and_no_reload_is_selected_root_only() {
        for program in ["aa-enforce", "aa-complain", "aa-disable"] {
            let live = parse(
                program,
                &strings(&["/etc/apparmor.d/usr.bin.demo"]),
                executable(program),
            )
            .unwrap();
            let SecurityPolicyInvocationDisposition::Generation(
                SecurityPolicyActivationInvocation::Apparmor(invocation),
            ) = live
            else {
                panic!("expected generation action");
            };
            assert!(matches!(
                (&invocation.action, program),
                (ApparmorActivationAction::ModeEnforce { .. }, "aa-enforce")
                    | (ApparmorActivationAction::ModeComplain { .. }, "aa-complain")
                    | (
                        ApparmorActivationAction::ProfileDisable { .. },
                        "aa-disable"
                    )
            ));

            let offline = parse(
                program,
                &strings(&["--no-reload", "/etc/apparmor.d/usr.bin.demo"]),
                executable(program),
            )
            .unwrap();
            assert_eq!(offline, SecurityPolicyInvocationDisposition::SelectedRoot);
        }
    }

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }
}
