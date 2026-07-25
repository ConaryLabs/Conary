// conary-core/src/activation/security_policy/selinux.rs

//! Closed SELinux helper grammars backed by current upstream policycoreutils.

use super::{
    ActivationExecutableIdentity, SecurityPolicyActivationInvocation,
    SecurityPolicyInvocationDisposition, SecurityPolicyParseError, nonempty_value,
    safe_absolute_path, unsupported,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelinuxBooleanAssignment {
    pub name: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum SelinuxActivationAction {
    BooleanSet {
        assignments: Vec<SelinuxBooleanAssignment>,
    },
    ModuleInstall {
        module_path: String,
        priority: Option<u16>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelinuxActivationInvocation {
    pub executable: ActivationExecutableIdentity,
    pub action: SelinuxActivationAction,
    /// Exact arguments observed after argv[0].
    pub arguments: Vec<String>,
}

impl SelinuxActivationInvocation {
    pub fn validate(&self) -> Result<(), SecurityPolicyParseError> {
        self.executable.validate()?;
        let program = std::path::Path::new(&self.executable.invoked_path)
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| unsupported("selinux", "executable path has no UTF-8 basename"))?;
        let disposition = parse(program, &self.arguments, self.executable.clone())?;
        let SecurityPolicyInvocationDisposition::Generation(
            SecurityPolicyActivationInvocation::Selinux(reparsed),
        ) = disposition
        else {
            return Err(unsupported(
                program,
                "persisted invocation no longer parses as live SELinux work",
            ));
        };
        if reparsed != *self {
            return Err(unsupported(
                program,
                "persisted typed SELinux action disagrees with exact argv",
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
        "restorecon" => {
            parse_restorecon(arguments)?;
            Ok(SecurityPolicyInvocationDisposition::SelectedRoot)
        }
        "semanage" => {
            parse_semanage_fcontext(arguments)?;
            Ok(SecurityPolicyInvocationDisposition::SelectedRoot)
        }
        "setsebool" => parse_setsebool(arguments, executable),
        "semodule" => parse_semodule(arguments, executable),
        _ => Err(unsupported(program, "unknown SELinux helper")),
    }
}

fn parse_restorecon(arguments: &[String]) -> Result<(), SecurityPolicyParseError> {
    let mut paths = Vec::new();
    for argument in arguments {
        if matches!(
            argument.as_str(),
            "-R" | "-r" | "-v" | "-F" | "-n" | "-D" | "--recursive" | "--verbose" | "--force"
        ) || short_flag_chars_are(argument, &['R', 'r', 'v', 'F', 'n', 'D'])
        {
            continue;
        }
        if argument.starts_with('-') {
            return Err(unsupported(
                "restorecon",
                format!("unsupported option '{argument}'"),
            ));
        }
        if !safe_policy_path(argument) {
            return Err(unsupported(
                "restorecon",
                format!("path '{argument}' is not a bounded selected-root path"),
            ));
        }
        paths.push(argument);
    }
    if paths.is_empty() {
        return Err(unsupported(
            "restorecon",
            "at least one bounded absolute path is required",
        ));
    }
    Ok(())
}

fn parse_semanage_fcontext(arguments: &[String]) -> Result<(), SecurityPolicyParseError> {
    if arguments.first().map(String::as_str) != Some("fcontext") {
        return Err(unsupported(
            "semanage",
            "only selected-root fcontext operations are modeled",
        ));
    }
    let mut action = None;
    let mut selinux_type = None;
    let mut expressions = Vec::new();
    let mut index = 1;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "-N" | "--noreload" => {}
            "-a" | "--add" => set_once(&mut action, "add", "semanage action")?,
            "-m" | "--modify" => set_once(&mut action, "modify", "semanage action")?,
            "-d" | "--delete" => set_once(&mut action, "delete", "semanage action")?,
            "-t" | "--type" => {
                index += 1;
                let value = arguments
                    .get(index)
                    .ok_or_else(|| unsupported("semanage", "type option requires a value"))?;
                if !is_selinux_identifier(value) {
                    return Err(unsupported("semanage", "invalid SELinux type"));
                }
                selinux_type = Some(value.as_str());
            }
            "-f" | "--ftype" | "-s" | "--seuser" | "-r" | "--range" => {
                index += 1;
                arguments
                    .get(index)
                    .ok_or_else(|| unsupported("semanage", "fcontext option requires a value"))?;
            }
            value if value.starts_with('-') => {
                return Err(unsupported(
                    "semanage",
                    format!("unsupported fcontext option '{value}'"),
                ));
            }
            value => expressions.push(value),
        }
        index += 1;
    }
    let action = action.ok_or_else(|| unsupported("semanage", "fcontext action is required"))?;
    if matches!(action, "add" | "modify") && selinux_type.is_none() {
        return Err(unsupported(
            "semanage",
            "fcontext add/modify requires an exact SELinux type",
        ));
    }
    if expressions.len() != 1 || fcontext_payload_root(expressions[0]).is_none() {
        return Err(unsupported(
            "semanage",
            "fcontext requires one bounded absolute expression",
        ));
    }
    Ok(())
}

fn parse_setsebool(
    arguments: &[String],
    executable: ActivationExecutableIdentity,
) -> Result<SecurityPolicyInvocationDisposition, SecurityPolicyParseError> {
    let mut persistent = false;
    let mut positional = Vec::new();
    let mut options_done = false;
    for argument in arguments {
        if !options_done && argument == "--" {
            options_done = true;
            continue;
        }
        if !options_done && argument.starts_with('-') {
            if argument.len() < 2
                || argument.starts_with("--")
                || !argument[1..]
                    .chars()
                    .all(|flag| matches!(flag, 'P' | 'N' | 'V'))
            {
                return Err(unsupported(
                    "setsebool",
                    format!("unsupported option '{argument}'"),
                ));
            }
            persistent |= argument[1..].contains('P');
            continue;
        }
        positional.push(argument.as_str());
    }
    let assignments = if positional.first().is_some_and(|value| value.contains('=')) {
        positional
            .iter()
            .map(|assignment| parse_boolean_assignment(assignment))
            .collect::<Result<Vec<_>, _>>()?
    } else {
        if positional.len() != 2 {
            return Err(unsupported(
                "setsebool",
                "boolean/value form requires exactly two operands",
            ));
        }
        vec![boolean_assignment(positional[0], positional[1])?]
    };
    if assignments.is_empty() {
        return Err(unsupported("setsebool", "at least one boolean is required"));
    }
    if !persistent {
        return Err(SecurityPolicyParseError::UnsupportedLiveOperation {
            program: "setsebool".to_string(),
            reason: "non-persistent booleans would be lost when the same generation reboots after its one-shot request was applied; use -P"
                .to_string(),
        });
    }
    Ok(SecurityPolicyInvocationDisposition::Generation(
        SecurityPolicyActivationInvocation::Selinux(SelinuxActivationInvocation {
            executable,
            action: SelinuxActivationAction::BooleanSet { assignments },
            arguments: arguments.to_vec(),
        }),
    ))
}

fn parse_semodule(
    arguments: &[String],
    executable: ActivationExecutableIdentity,
) -> Result<SecurityPolicyInvocationDisposition, SecurityPolicyParseError> {
    let mut module_path = None;
    let mut priority = None;
    let mut no_reload = false;
    let mut index = 0;
    while index < arguments.len() {
        let argument = &arguments[index];
        match argument.as_str() {
            "-i" | "--install" => {
                index += 1;
                let path = arguments
                    .get(index)
                    .ok_or_else(|| unsupported("semodule", "install requires a module path"))?;
                set_owned_once(&mut module_path, path, "semodule install path")?;
            }
            "-N" | "-n" | "--noreload" => no_reload = true,
            "-v" | "--verbose" | "-P" | "--preserve_tunables" | "-C" | "--ignore-module-cache" => {}
            "-X" | "--priority" => {
                index += 1;
                let value = arguments
                    .get(index)
                    .ok_or_else(|| unsupported("semodule", "priority requires a value"))?;
                priority = Some(parse_priority(value)?);
            }
            value if value.starts_with("--install=") => {
                set_owned_once(
                    &mut module_path,
                    value.trim_start_matches("--install="),
                    "semodule install path",
                )?;
            }
            value if value.starts_with("--priority=") => {
                priority = Some(parse_priority(value.trim_start_matches("--priority="))?);
            }
            value => {
                return Err(unsupported(
                    "semodule",
                    format!("unsupported module-install argument '{value}'"),
                ));
            }
        }
        index += 1;
    }
    let module_path =
        module_path.ok_or_else(|| unsupported("semodule", "one install path is required"))?;
    if !safe_policy_path(&module_path)
        || !matches!(
            std::path::Path::new(&module_path)
                .extension()
                .and_then(|extension| extension.to_str()),
            Some("pp" | "cil")
        )
    {
        return Err(unsupported(
            "semodule",
            "module path must be a bounded absolute .pp or .cil path",
        ));
    }
    if no_reload {
        return Ok(SecurityPolicyInvocationDisposition::SelectedRoot);
    }
    Ok(SecurityPolicyInvocationDisposition::Generation(
        SecurityPolicyActivationInvocation::Selinux(SelinuxActivationInvocation {
            executable,
            action: SelinuxActivationAction::ModuleInstall {
                module_path,
                priority,
            },
            arguments: arguments.to_vec(),
        }),
    ))
}

fn parse_boolean_assignment(
    expression: &str,
) -> Result<SelinuxBooleanAssignment, SecurityPolicyParseError> {
    let (name, value) = expression
        .split_once('=')
        .ok_or_else(|| unsupported("setsebool", "assignment requires name=value"))?;
    boolean_assignment(name, value)
}

fn boolean_assignment(
    name: &str,
    value: &str,
) -> Result<SelinuxBooleanAssignment, SecurityPolicyParseError> {
    if !is_selinux_identifier(name) {
        return Err(unsupported(
            "setsebool",
            format!("invalid boolean name '{name}'"),
        ));
    }
    let enabled = match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "on" => true,
        "0" | "false" | "off" => false,
        _ => {
            return Err(unsupported(
                "setsebool",
                format!("invalid boolean value '{value}'"),
            ));
        }
    };
    Ok(SelinuxBooleanAssignment {
        name: name.to_string(),
        enabled,
    })
}

fn parse_priority(value: &str) -> Result<u16, SecurityPolicyParseError> {
    let priority = value
        .parse::<u16>()
        .map_err(|_| unsupported("semodule", format!("invalid priority '{value}'")))?;
    if !(1..=999).contains(&priority) {
        return Err(unsupported(
            "semodule",
            format!("priority '{value}' is outside 1..=999"),
        ));
    }
    Ok(priority)
}

fn fcontext_payload_root(expression: &str) -> Option<String> {
    let expression = expression.strip_prefix('^').unwrap_or(expression);
    let expression = expression.strip_suffix('$').unwrap_or(expression);
    let root = expression
        .strip_suffix("(/.*)?")
        .or_else(|| expression.strip_suffix("(/.*)"))
        .unwrap_or(expression);
    safe_policy_path(root).then(|| root.to_string())
}

fn safe_policy_path(path: &str) -> bool {
    safe_absolute_path(path)
        && !matches!(
            path.trim_end_matches('/'),
            "" | "/"
                | "/bin"
                | "/boot"
                | "/etc"
                | "/lib"
                | "/lib64"
                | "/opt"
                | "/sbin"
                | "/usr"
                | "/usr/bin"
                | "/usr/lib"
                | "/usr/lib64"
                | "/usr/sbin"
                | "/usr/share"
                | "/var"
        )
}

fn is_selinux_identifier(value: &str) -> bool {
    nonempty_value(value)
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

fn short_flag_chars_are(argument: &str, allowed: &[char]) -> bool {
    argument.starts_with('-')
        && !argument.starts_with("--")
        && argument.len() > 2
        && argument[1..].chars().all(|flag| allowed.contains(&flag))
}

fn set_once<'a>(
    slot: &mut Option<&'a str>,
    value: &'a str,
    label: &str,
) -> Result<(), SecurityPolicyParseError> {
    if slot.replace(value).is_some() {
        return Err(unsupported("selinux", format!("{label} was repeated")));
    }
    Ok(())
}

fn set_owned_once(
    slot: &mut Option<String>,
    value: impl AsRef<str>,
    label: &str,
) -> Result<(), SecurityPolicyParseError> {
    if slot.replace(value.as_ref().to_string()).is_some() {
        return Err(unsupported("selinux", format!("{label} was repeated")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn executable(program: &str) -> ActivationExecutableIdentity {
        ActivationExecutableIdentity {
            invoked_path: format!("/usr/sbin/{program}"),
            canonical_path: format!("/usr/sbin/{program}"),
            sha256: format!("sha256:{}", "a".repeat(64)),
        }
    }

    #[test]
    fn persistent_boolean_forms_are_typed_and_nonpersistent_form_fails() {
        let args = strings(&["-PN", "demo_can_network=on", "demo_debug=0"]);
        let SecurityPolicyInvocationDisposition::Generation(
            SecurityPolicyActivationInvocation::Selinux(invocation),
        ) = parse("setsebool", &args, executable("setsebool")).unwrap()
        else {
            panic!("expected generation activation");
        };
        let SelinuxActivationAction::BooleanSet { assignments } = invocation.action else {
            panic!("expected boolean action");
        };
        assert_eq!(assignments.len(), 2);
        assert!(assignments[0].enabled);
        assert!(!assignments[1].enabled);

        let error = parse(
            "setsebool",
            &strings(&["demo_can_network", "on"]),
            executable("setsebool"),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            SecurityPolicyParseError::UnsupportedLiveOperation { .. }
        ));
    }

    #[test]
    fn semodule_install_splits_no_reload_from_live_reload() {
        let args = strings(&["-X", "300", "-i", "/usr/share/selinux/demo.pp"]);
        let live = parse("semodule", &args, executable("semodule")).unwrap();
        assert!(matches!(
            live,
            SecurityPolicyInvocationDisposition::Generation(_)
        ));
        let offline = parse(
            "semodule",
            &strings(&["-N", "-i", "/usr/share/selinux/demo.pp"]),
            executable("semodule"),
        )
        .unwrap();
        assert_eq!(offline, SecurityPolicyInvocationDisposition::SelectedRoot);
    }

    #[test]
    fn restorecon_and_fcontext_are_selected_root_only() {
        assert_eq!(
            parse(
                "restorecon",
                &strings(&["-Rv", "/usr/share/demo"]),
                executable("restorecon")
            )
            .unwrap(),
            SecurityPolicyInvocationDisposition::SelectedRoot
        );
        assert_eq!(
            parse(
                "semanage",
                &strings(&[
                    "fcontext",
                    "-N",
                    "-a",
                    "-t",
                    "demo_exec_t",
                    "/usr/lib/demo(/.*)?"
                ]),
                executable("semanage")
            )
            .unwrap(),
            SecurityPolicyInvocationDisposition::SelectedRoot
        );
    }

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }
}
