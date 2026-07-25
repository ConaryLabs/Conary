// conary-core/src/packages/deb/dpkg_lifecycle/dpkg/options.rs

use super::{DpkgForceMode, DpkgForceThing, DpkgOption};
use crate::packages::deb::dpkg_lifecycle::common::{
    DPKG, DpkgLifecycleGrammarError, long_value, option_value, require_no_nul,
};

pub(super) fn parse_option(
    argv: &[Vec<u8>],
    index: &mut usize,
    argument: &[u8],
) -> Result<Option<DpkgOption>, DpkgLifecycleGrammarError> {
    let fixed = match argument {
        b"-B" | b"--auto-deconfigure" => Some(DpkgOption::AutoDeconfigure),
        b"--no-act" | b"--dry-run" | b"--simulate" => Some(DpkgOption::NoAct),
        b"-R" | b"--recursive" => Some(DpkgOption::Recursive),
        b"-G" => Some(DpkgOption::RefuseDowngrade),
        b"-O" | b"--selected-only" => Some(DpkgOption::SelectedOnly),
        b"-E" | b"--skip-same-version" => Some(DpkgOption::SkipSameVersion),
        b"--robot" => Some(DpkgOption::Robot),
        b"--no-pager" => Some(DpkgOption::NoPager),
        b"--no-debsig" => Some(DpkgOption::NoDebsig),
        b"--triggers" => Some(DpkgOption::TriggerProcessing(true)),
        b"--no-triggers" => Some(DpkgOption::TriggerProcessing(false)),
        b"-a" | b"--pending" => Some(DpkgOption::Pending),
        _ => None,
    };
    if fixed.is_some() {
        return Ok(fixed);
    }
    if argument == b"--debug" || argument == b"-D" {
        let value = option_value(argv, index, DPKG, "--debug", None)?;
        return parse_debug(&value).map(|value| Some(DpkgOption::Debug(value)));
    }
    if let Some(value) = argument
        .strip_prefix(b"-D")
        .filter(|value| !value.is_empty())
    {
        return parse_debug(value).map(|value| Some(DpkgOption::Debug(value)));
    }
    if let Some(value) = long_value(argument, b"--debug") {
        return parse_debug(value).map(|value| Some(DpkgOption::Debug(value)));
    }
    if let Some((mode, value)) = force_value(argv, index, argument)? {
        return parse_force(mode, &value).map(Some);
    }
    for (name, kind) in value_options() {
        if argument == name {
            let value = option_value(argv, index, DPKG, kind.label(), None)?;
            return kind.build(value).map(Some);
        }
        if let Some(value) = long_value(argument, name) {
            let value = option_value(argv, index, DPKG, kind.label(), Some(value))?;
            return kind.build(value).map(Some);
        }
    }
    Ok(None)
}

#[derive(Clone, Copy)]
enum ValueOption {
    AbortAfter,
    IgnoreDepends,
    Admin,
    Install,
    Root,
    PreInvoke,
    PostInvoke,
    PathExclude,
    PathInclude,
    VerifyFormat,
    StatusFd,
    StatusLogger,
    Log,
}

impl ValueOption {
    const fn label(self) -> &'static str {
        match self {
            Self::AbortAfter => "--abort-after",
            Self::IgnoreDepends => "--ignore-depends",
            Self::Admin => "--admindir",
            Self::Install => "--instdir",
            Self::Root => "--root",
            Self::PreInvoke => "--pre-invoke",
            Self::PostInvoke => "--post-invoke",
            Self::PathExclude => "--path-exclude",
            Self::PathInclude => "--path-include",
            Self::VerifyFormat => "--verify-format",
            Self::StatusFd => "--status-fd",
            Self::StatusLogger => "--status-logger",
            Self::Log => "--log",
        }
    }

    fn build(self, value: Vec<u8>) -> Result<DpkgOption, DpkgLifecycleGrammarError> {
        Ok(match self {
            Self::AbortAfter => DpkgOption::AbortAfter(parse_u32(self.label(), &value, 10)?),
            Self::IgnoreDepends => {
                let values = split_nonempty(self.label(), &value, b',')?;
                DpkgOption::IgnoreDepends(values)
            }
            Self::Admin => DpkgOption::AdministrativeDirectory(value),
            Self::Install => DpkgOption::InstallationDirectory(value),
            Self::Root => DpkgOption::Root(value),
            Self::PreInvoke => DpkgOption::PreInvoke(value),
            Self::PostInvoke => DpkgOption::PostInvoke(value),
            Self::PathExclude => DpkgOption::PathExclude(value),
            Self::PathInclude => DpkgOption::PathInclude(value),
            Self::VerifyFormat => DpkgOption::VerifyFormat(value),
            Self::StatusFd => DpkgOption::StatusFd(parse_u32(self.label(), &value, 10)?),
            Self::StatusLogger => DpkgOption::StatusLogger(value),
            Self::Log => DpkgOption::Log(value),
        })
    }
}

fn value_options() -> [(&'static [u8], ValueOption); 13] {
    [
        (b"--abort-after", ValueOption::AbortAfter),
        (b"--ignore-depends", ValueOption::IgnoreDepends),
        (b"--admindir", ValueOption::Admin),
        (b"--instdir", ValueOption::Install),
        (b"--root", ValueOption::Root),
        (b"--pre-invoke", ValueOption::PreInvoke),
        (b"--post-invoke", ValueOption::PostInvoke),
        (b"--path-exclude", ValueOption::PathExclude),
        (b"--path-include", ValueOption::PathInclude),
        (b"--verify-format", ValueOption::VerifyFormat),
        (b"--status-fd", ValueOption::StatusFd),
        (b"--status-logger", ValueOption::StatusLogger),
        (b"--log", ValueOption::Log),
    ]
}

fn force_value(
    argv: &[Vec<u8>],
    index: &mut usize,
    argument: &[u8],
) -> Result<Option<(DpkgForceMode, Vec<u8>)>, DpkgLifecycleGrammarError> {
    for (name, mode) in [
        (b"--force".as_slice(), DpkgForceMode::Force),
        (b"--refuse".as_slice(), DpkgForceMode::Refuse),
        (b"--no-force".as_slice(), DpkgForceMode::Refuse),
    ] {
        if argument == name {
            let value = option_value(argv, index, DPKG, "--force", None)?;
            return Ok(Some((mode, value)));
        }
        let mut prefix = name.to_vec();
        prefix.push(b'-');
        if let Some(value) = argument.strip_prefix(prefix.as_slice()) {
            return Ok(Some((mode, value.to_vec())));
        }
    }
    Ok(None)
}

fn parse_force(mode: DpkgForceMode, value: &[u8]) -> Result<DpkgOption, DpkgLifecycleGrammarError> {
    let values = split_nonempty("--force", value, b',')?;
    let mut things = Vec::with_capacity(values.len());
    for value in values {
        things.push(DpkgForceThing::parse(&value).ok_or_else(|| {
            DpkgLifecycleGrammarError::InvalidOperand {
                tool: DPKG,
                action: "--force",
                operand: "force thing",
                value,
                reason: "is not documented by dpkg 1.23.7",
            }
        })?);
    }
    Ok(DpkgOption::Force { mode, things })
}

fn parse_debug(value: &[u8]) -> Result<u32, DpkgLifecycleGrammarError> {
    parse_u32("--debug", value, 8)
}

fn parse_u32(
    action: &'static str,
    value: &[u8],
    radix: u32,
) -> Result<u32, DpkgLifecycleGrammarError> {
    let text = std::str::from_utf8(value).map_err(|_| invalid_number(action, value))?;
    u32::from_str_radix(text, radix).map_err(|_| invalid_number(action, value))
}

fn invalid_number(action: &'static str, value: &[u8]) -> DpkgLifecycleGrammarError {
    DpkgLifecycleGrammarError::InvalidOperand {
        tool: DPKG,
        action,
        operand: "numeric value",
        value: value.to_vec(),
        reason: "has invalid syntax or is out of range",
    }
}

fn split_nonempty(
    action: &'static str,
    value: &[u8],
    separator: u8,
) -> Result<Vec<Vec<u8>>, DpkgLifecycleGrammarError> {
    let values = value.split(|byte| *byte == separator).collect::<Vec<_>>();
    if values.iter().any(|value| value.is_empty()) {
        return Err(DpkgLifecycleGrammarError::InvalidOperand {
            tool: DPKG,
            action,
            operand: "list value",
            value: value.to_vec(),
            reason: "contains an empty member",
        });
    }
    for value in &values {
        require_no_nul(DPKG, action, "list member", value)?;
    }
    Ok(values.into_iter().map(<[u8]>::to_vec).collect())
}
