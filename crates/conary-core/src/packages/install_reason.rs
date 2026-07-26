// conary-core/src/packages/install_reason.rs

//! Exact native package-manager install-reason query support.

use std::collections::HashSet;
use std::process::Command;

/// Failure to obtain exact install-reason authority from a native package
/// manager.
#[derive(Debug, thiserror::Error)]
pub enum InstallReasonAuthorityError {
    #[error(
        "{authority} install-reason authority is unavailable because command '{command}' was not found"
    )]
    CommandUnavailable {
        authority: &'static str,
        command: String,
    },

    #[error("{authority} install-reason authority could not execute command '{command}': {detail}")]
    CommandIo {
        authority: &'static str,
        command: String,
        detail: String,
    },

    #[error(
        "{authority} install-reason command '{command}' failed with status {status:?}: {stderr}"
    )]
    CommandFailed {
        authority: &'static str,
        command: String,
        status: Option<i32>,
        stderr: String,
    },

    #[error("{authority} install-reason command '{command}' emitted invalid UTF-8: {detail}")]
    InvalidUtf8 {
        authority: &'static str,
        command: String,
        detail: String,
    },

    #[error(
        "{authority} install-reason authority emitted malformed package name at line {line}: {detail}"
    )]
    MalformedOutput {
        authority: &'static str,
        line: usize,
        detail: String,
    },
}

impl InstallReasonAuthorityError {
    pub(crate) fn is_command_unavailable(&self) -> bool {
        matches!(self, Self::CommandUnavailable { .. })
    }
}

pub(crate) fn query_package_names(
    authority: &'static str,
    command: &str,
    args: &[&str],
) -> Result<HashSet<String>, InstallReasonAuthorityError> {
    let display_command = std::iter::once(command)
        .chain(args.iter().copied())
        .collect::<Vec<_>>()
        .join(" ");
    let output = Command::new(command)
        .args(args)
        .env("LC_ALL", "C")
        .output()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                InstallReasonAuthorityError::CommandUnavailable {
                    authority,
                    command: display_command.clone(),
                }
            } else {
                InstallReasonAuthorityError::CommandIo {
                    authority,
                    command: display_command.clone(),
                    detail: error.to_string(),
                }
            }
        })?;

    if !output.status.success() {
        return Err(InstallReasonAuthorityError::CommandFailed {
            authority,
            command: display_command,
            status: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }

    let stdout = String::from_utf8(output.stdout).map_err(|error| {
        InstallReasonAuthorityError::InvalidUtf8 {
            authority,
            command: display_command,
            detail: error.to_string(),
        }
    })?;
    parse_package_name_lines(authority, &stdout)
}

pub(crate) fn parse_package_name_lines(
    authority: &'static str,
    output: &str,
) -> Result<HashSet<String>, InstallReasonAuthorityError> {
    let mut packages = HashSet::new();
    for (index, name) in output.lines().enumerate() {
        let line = index + 1;
        if name.is_empty() {
            return Err(InstallReasonAuthorityError::MalformedOutput {
                authority,
                line,
                detail: "blank records are not part of the documented output grammar".to_string(),
            });
        }
        if name.trim() != name || name.chars().any(char::is_whitespace) {
            return Err(InstallReasonAuthorityError::MalformedOutput {
                authority,
                line,
                detail: format!("expected one exact package name, got {name:?}"),
            });
        }
        packages.insert(name.to_string());
    }
    Ok(packages)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_name_output_is_exact_and_deduplicated() {
        let parsed = parse_package_name_lines("test", "alpha\nbeta:amd64\nalpha\n").unwrap();
        assert_eq!(parsed.len(), 2);
        assert!(parsed.contains("alpha"));
        assert!(parsed.contains("beta:amd64"));
    }

    #[test]
    fn malformed_package_name_output_is_typed() {
        let error = parse_package_name_lines("test", "alpha\nbad package\n").unwrap_err();
        assert!(matches!(
            error,
            InstallReasonAuthorityError::MalformedOutput { line: 2, .. }
        ));
    }

    #[test]
    fn missing_authority_command_is_typed() {
        let error = query_package_names(
            "test",
            "/conary/definitely-missing-install-reason-authority",
            &[],
        )
        .unwrap_err();
        assert!(matches!(
            error,
            InstallReasonAuthorityError::CommandUnavailable { .. }
        ));
    }
}
