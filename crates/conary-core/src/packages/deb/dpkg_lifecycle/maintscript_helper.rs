// conary-core/src/packages/deb/dpkg_lifecycle/maintscript_helper.rs

use super::common::{
    DpkgLifecycleGrammarError, MAINTSCRIPT_HELPER, private_internal_command, require_absolute,
    require_no_nul, require_nonempty, required,
};

/// The four public operations reported by `supports` in dpkg 1.23.7.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaintscriptHelperAction {
    RemoveConffile,
    MoveConffile,
    SymlinkToDirectory,
    DirectoryToSymlink,
}

impl MaintscriptHelperAction {
    pub const ALL: [Self; 4] = [
        Self::RemoveConffile,
        Self::MoveConffile,
        Self::SymlinkToDirectory,
        Self::DirectoryToSymlink,
    ];

    pub const fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::RemoveConffile => b"rm_conffile",
            Self::MoveConffile => b"mv_conffile",
            Self::SymlinkToDirectory => b"symlink_to_dir",
            Self::DirectoryToSymlink => b"dir_to_symlink",
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::RemoveConffile => "rm_conffile",
            Self::MoveConffile => "mv_conffile",
            Self::SymlinkToDirectory => "symlink_to_dir",
            Self::DirectoryToSymlink => "dir_to_symlink",
        }
    }

    fn parse(value: &[u8]) -> Result<Self, DpkgLifecycleGrammarError> {
        match value {
            b"rm_conffile" => Ok(Self::RemoveConffile),
            b"mv_conffile" => Ok(Self::MoveConffile),
            b"symlink_to_dir" => Ok(Self::SymlinkToDirectory),
            b"dir_to_symlink" => Ok(Self::DirectoryToSymlink),
            _ => Err(DpkgLifecycleGrammarError::UnknownAction {
                tool: MAINTSCRIPT_HELPER,
                action: value.to_vec(),
            }),
        }
    }
}

/// Replacement context used by the documented prerm/postinst phase forms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaintainerReplacement {
    pub in_favour_package: Vec<u8>,
    pub in_favour_version: Vec<u8>,
    pub removing: Option<(Vec<u8>, Vec<u8>)>,
}

impl MaintainerReplacement {
    fn parse(
        action: &'static str,
        values: &[Vec<u8>],
        permit_removing: bool,
    ) -> Result<Self, DpkgLifecycleGrammarError> {
        let expected = if permit_removing {
            &[3, 6][..]
        } else {
            &[3][..]
        };
        if !expected.contains(&values.len()) {
            if values.len() < 3 {
                return Err(DpkgLifecycleGrammarError::MissingOperand {
                    tool: MAINTSCRIPT_HELPER,
                    action,
                    operand: "in-favour package and version",
                });
            }
            return Err(DpkgLifecycleGrammarError::UnexpectedOperand {
                tool: MAINTSCRIPT_HELPER,
                action,
                operand: values[3.min(values.len() - 1)].clone(),
            });
        }
        if values[0] != b"in-favour" {
            return Err(DpkgLifecycleGrammarError::InvalidOperand {
                tool: MAINTSCRIPT_HELPER,
                action,
                operand: "replacement marker",
                value: values[0].clone(),
                reason: "must be the literal 'in-favour'",
            });
        }
        require_nonempty(
            MAINTSCRIPT_HELPER,
            action,
            "replacement package",
            &values[1],
        )?;
        require_no_nul(
            MAINTSCRIPT_HELPER,
            action,
            "replacement version",
            &values[2],
        )?;
        let removing = if values.len() == 6 {
            if values[3] != b"removing" {
                return Err(DpkgLifecycleGrammarError::InvalidOperand {
                    tool: MAINTSCRIPT_HELPER,
                    action,
                    operand: "removal marker",
                    value: values[3].clone(),
                    reason: "must be the literal 'removing'",
                });
            }
            require_nonempty(MAINTSCRIPT_HELPER, action, "removed package", &values[4])?;
            require_no_nul(MAINTSCRIPT_HELPER, action, "removed version", &values[5])?;
            Some((values[4].clone(), values[5].clone()))
        } else {
            None
        };
        Ok(Self {
            in_favour_package: values[1].clone(),
            in_favour_version: values[2].clone(),
            removing,
        })
    }

    fn argv(&self) -> Vec<Vec<u8>> {
        let mut argv = vec![
            b"in-favour".to_vec(),
            self.in_favour_package.clone(),
            self.in_favour_version.clone(),
        ];
        if let Some((package, version)) = &self.removing {
            argv.push(b"removing".to_vec());
            argv.push(package.clone());
            argv.push(version.clone());
        }
        argv
    }
}

/// Union of the current maintainer-script argv forms documented by dpkg.
///
/// The script kind is supplied through `DPKG_MAINTSCRIPT_NAME`, not argv, so
/// this enum deliberately models the documented union without guessing which
/// script generated a shared action such as `upgrade`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MaintainerScriptPhase {
    Install {
        old_and_new_version: Option<(Vec<u8>, Vec<u8>)>,
    },
    Upgrade {
        old_version: Option<Vec<u8>>,
        new_version: Vec<u8>,
    },
    AbortUpgrade {
        old_version: Option<Vec<u8>>,
        new_version: Vec<u8>,
    },
    AbortInstall {
        old_and_new_version: Option<(Vec<u8>, Vec<u8>)>,
    },
    Configure {
        old_version: Vec<u8>,
    },
    Triggered {
        trigger_names: Vec<u8>,
    },
    AbortRemove {
        replacement: Option<MaintainerReplacement>,
    },
    AbortDeconfigure {
        replacement: MaintainerReplacement,
    },
    Remove {
        replacement: Option<MaintainerReplacement>,
    },
    FailedUpgrade {
        old_version: Vec<u8>,
        new_version: Vec<u8>,
    },
    Deconfigure {
        replacement: MaintainerReplacement,
    },
    Purge,
    Disappear {
        overwriter_package: Vec<u8>,
        overwriter_version: Vec<u8>,
    },
}

impl MaintainerScriptPhase {
    pub fn parse(argv: &[Vec<u8>]) -> Result<Self, DpkgLifecycleGrammarError> {
        for value in argv {
            require_no_nul(
                MAINTSCRIPT_HELPER,
                "maintainer phase",
                "phase argument",
                value,
            )?;
        }
        let action = required(
            MAINTSCRIPT_HELPER,
            "maintainer phase",
            "phase action",
            argv.first(),
        )?;
        let rest = &argv[1..];
        match action {
            b"install" => Self::parse_install(rest),
            b"upgrade" => Self::parse_version_pair("upgrade", rest, false),
            b"abort-upgrade" => Self::parse_version_pair("abort-upgrade", rest, true),
            b"abort-install" => Self::parse_abort_install(rest),
            b"configure" => {
                exact_count("configure", rest, 1, "old version")?;
                Ok(Self::Configure {
                    old_version: rest[0].clone(),
                })
            }
            b"triggered" => {
                exact_count("triggered", rest, 1, "trigger names")?;
                require_nonempty(MAINTSCRIPT_HELPER, "triggered", "trigger names", &rest[0])?;
                Ok(Self::Triggered {
                    trigger_names: rest[0].clone(),
                })
            }
            b"abort-remove" => parse_optional_replacement("abort-remove", rest)
                .map(|replacement| Self::AbortRemove { replacement }),
            b"abort-deconfigure" => MaintainerReplacement::parse("abort-deconfigure", rest, true)
                .map(|replacement| Self::AbortDeconfigure { replacement }),
            b"remove" => parse_optional_replacement("remove", rest)
                .map(|replacement| Self::Remove { replacement }),
            b"failed-upgrade" => {
                exact_count("failed-upgrade", rest, 2, "old and new versions")?;
                Ok(Self::FailedUpgrade {
                    old_version: rest[0].clone(),
                    new_version: rest[1].clone(),
                })
            }
            b"deconfigure" => MaintainerReplacement::parse("deconfigure", rest, true)
                .map(|replacement| Self::Deconfigure { replacement }),
            b"purge" => {
                exact_count("purge", rest, 0, "argument")?;
                Ok(Self::Purge)
            }
            b"disappear" => {
                exact_count("disappear", rest, 2, "overwriter package and version")?;
                require_nonempty(
                    MAINTSCRIPT_HELPER,
                    "disappear",
                    "overwriter package",
                    &rest[0],
                )?;
                Ok(Self::Disappear {
                    overwriter_package: rest[0].clone(),
                    overwriter_version: rest[1].clone(),
                })
            }
            _ => Err(DpkgLifecycleGrammarError::UnknownAction {
                tool: MAINTSCRIPT_HELPER,
                action: action.to_vec(),
            }),
        }
    }

    fn parse_install(rest: &[Vec<u8>]) -> Result<Self, DpkgLifecycleGrammarError> {
        match rest {
            [] => Ok(Self::Install {
                old_and_new_version: None,
            }),
            [old_version, new_version] => Ok(Self::Install {
                old_and_new_version: Some((old_version.clone(), new_version.clone())),
            }),
            [_] => Err(DpkgLifecycleGrammarError::MissingOperand {
                tool: MAINTSCRIPT_HELPER,
                action: "install",
                operand: "new version",
            }),
            [_, _, unexpected, ..] => Err(DpkgLifecycleGrammarError::UnexpectedOperand {
                tool: MAINTSCRIPT_HELPER,
                action: "install",
                operand: unexpected.clone(),
            }),
        }
    }

    fn parse_abort_install(rest: &[Vec<u8>]) -> Result<Self, DpkgLifecycleGrammarError> {
        match rest {
            [] => Ok(Self::AbortInstall {
                old_and_new_version: None,
            }),
            [old_version, new_version] => Ok(Self::AbortInstall {
                old_and_new_version: Some((old_version.clone(), new_version.clone())),
            }),
            [_] => Err(DpkgLifecycleGrammarError::MissingOperand {
                tool: MAINTSCRIPT_HELPER,
                action: "abort-install",
                operand: "new version",
            }),
            [_, _, unexpected, ..] => Err(DpkgLifecycleGrammarError::UnexpectedOperand {
                tool: MAINTSCRIPT_HELPER,
                action: "abort-install",
                operand: unexpected.clone(),
            }),
        }
    }

    fn parse_version_pair(
        action: &'static str,
        rest: &[Vec<u8>],
        abort: bool,
    ) -> Result<Self, DpkgLifecycleGrammarError> {
        let (old_version, new_version) = match rest {
            [new_version] => (None, new_version.clone()),
            [old_version, new_version] => (Some(old_version.clone()), new_version.clone()),
            [] => {
                return Err(DpkgLifecycleGrammarError::MissingOperand {
                    tool: MAINTSCRIPT_HELPER,
                    action,
                    operand: "new version",
                });
            }
            [_, _, unexpected, ..] => {
                return Err(DpkgLifecycleGrammarError::UnexpectedOperand {
                    tool: MAINTSCRIPT_HELPER,
                    action,
                    operand: unexpected.clone(),
                });
            }
        };
        if abort {
            Ok(Self::AbortUpgrade {
                old_version,
                new_version,
            })
        } else {
            Ok(Self::Upgrade {
                old_version,
                new_version,
            })
        }
    }

    pub fn argv(&self) -> Vec<Vec<u8>> {
        match self {
            Self::Install {
                old_and_new_version,
            } => {
                let mut argv = vec![b"install".to_vec()];
                if let Some((old, new)) = old_and_new_version {
                    argv.push(old.clone());
                    argv.push(new.clone());
                }
                argv
            }
            Self::Upgrade {
                old_version,
                new_version,
            }
            | Self::AbortUpgrade {
                old_version,
                new_version,
            } => {
                let action = if matches!(self, Self::Upgrade { .. }) {
                    b"upgrade".as_slice()
                } else {
                    b"abort-upgrade".as_slice()
                };
                let mut argv = vec![action.to_vec()];
                if let Some(old) = old_version {
                    argv.push(old.clone());
                }
                argv.push(new_version.clone());
                argv
            }
            Self::AbortInstall {
                old_and_new_version,
            } => {
                let mut argv = vec![b"abort-install".to_vec()];
                if let Some((old, new)) = old_and_new_version {
                    argv.push(old.clone());
                    argv.push(new.clone());
                }
                argv
            }
            Self::Configure { old_version } => {
                vec![b"configure".to_vec(), old_version.clone()]
            }
            Self::Triggered { trigger_names } => {
                vec![b"triggered".to_vec(), trigger_names.clone()]
            }
            Self::AbortRemove { replacement } | Self::Remove { replacement } => {
                let action = if matches!(self, Self::AbortRemove { .. }) {
                    b"abort-remove".as_slice()
                } else {
                    b"remove".as_slice()
                };
                let mut argv = vec![action.to_vec()];
                if let Some(replacement) = replacement {
                    argv.extend(replacement.argv());
                }
                argv
            }
            Self::AbortDeconfigure { replacement } | Self::Deconfigure { replacement } => {
                let action = if matches!(self, Self::AbortDeconfigure { .. }) {
                    b"abort-deconfigure".as_slice()
                } else {
                    b"deconfigure".as_slice()
                };
                let mut argv = vec![action.to_vec()];
                argv.extend(replacement.argv());
                argv
            }
            Self::FailedUpgrade {
                old_version,
                new_version,
            } => vec![
                b"failed-upgrade".to_vec(),
                old_version.clone(),
                new_version.clone(),
            ],
            Self::Purge => vec![b"purge".to_vec()],
            Self::Disappear {
                overwriter_package,
                overwriter_version,
            } => vec![
                b"disappear".to_vec(),
                overwriter_package.clone(),
                overwriter_version.clone(),
            ],
        }
    }
}

fn exact_count(
    action: &'static str,
    values: &[Vec<u8>],
    count: usize,
    missing: &'static str,
) -> Result<(), DpkgLifecycleGrammarError> {
    if values.len() < count {
        return Err(DpkgLifecycleGrammarError::MissingOperand {
            tool: MAINTSCRIPT_HELPER,
            action,
            operand: missing,
        });
    }
    if values.len() > count {
        return Err(DpkgLifecycleGrammarError::UnexpectedOperand {
            tool: MAINTSCRIPT_HELPER,
            action,
            operand: values[count].clone(),
        });
    }
    for value in values {
        require_no_nul(MAINTSCRIPT_HELPER, action, "phase argument", value)?;
    }
    Ok(())
}

fn parse_optional_replacement(
    action: &'static str,
    values: &[Vec<u8>],
) -> Result<Option<MaintainerReplacement>, DpkgLifecycleGrammarError> {
    if values.is_empty() {
        Ok(None)
    } else {
        MaintainerReplacement::parse(action, values, false).map(Some)
    }
}

/// Typed public operation arguments, separated from maintainer phase argv.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MaintscriptHelperOperation {
    RemoveConffile {
        conffile: Vec<u8>,
        prior_version: Option<Vec<u8>>,
        package: Option<Vec<u8>>,
        phase: MaintainerScriptPhase,
    },
    MoveConffile {
        old_conffile: Vec<u8>,
        new_conffile: Vec<u8>,
        prior_version: Option<Vec<u8>>,
        package: Option<Vec<u8>>,
        phase: MaintainerScriptPhase,
    },
    SymlinkToDirectory {
        pathname: Vec<u8>,
        old_target: Vec<u8>,
        prior_version: Option<Vec<u8>>,
        package: Option<Vec<u8>>,
        phase: MaintainerScriptPhase,
    },
    DirectoryToSymlink {
        pathname: Vec<u8>,
        new_target: Vec<u8>,
        prior_version: Option<Vec<u8>>,
        package: Option<Vec<u8>>,
        phase: MaintainerScriptPhase,
    },
}

impl MaintscriptHelperOperation {
    pub const fn action(&self) -> MaintscriptHelperAction {
        match self {
            Self::RemoveConffile { .. } => MaintscriptHelperAction::RemoveConffile,
            Self::MoveConffile { .. } => MaintscriptHelperAction::MoveConffile,
            Self::SymlinkToDirectory { .. } => MaintscriptHelperAction::SymlinkToDirectory,
            Self::DirectoryToSymlink { .. } => MaintscriptHelperAction::DirectoryToSymlink,
        }
    }

    pub const fn phase(&self) -> &MaintainerScriptPhase {
        match self {
            Self::RemoveConffile { phase, .. }
            | Self::MoveConffile { phase, .. }
            | Self::SymlinkToDirectory { phase, .. }
            | Self::DirectoryToSymlink { phase, .. } => phase,
        }
    }

    fn parse(
        action: MaintscriptHelperAction,
        operation: &[Vec<u8>],
        phase: MaintainerScriptPhase,
    ) -> Result<Self, DpkgLifecycleGrammarError> {
        let required_paths = if action == MaintscriptHelperAction::RemoveConffile {
            1
        } else {
            2
        };
        if operation.len() < required_paths {
            return Err(DpkgLifecycleGrammarError::MissingOperand {
                tool: MAINTSCRIPT_HELPER,
                action: action.name(),
                operand: "pathname",
            });
        }
        if operation.len() > required_paths + 2 {
            return Err(DpkgLifecycleGrammarError::UnexpectedOperand {
                tool: MAINTSCRIPT_HELPER,
                action: action.name(),
                operand: operation[required_paths + 2].clone(),
            });
        }
        let first = operation[0].clone();
        require_absolute(MAINTSCRIPT_HELPER, action.name(), "pathname", &first)?;
        let second = operation.get(1).cloned();
        if let Some(second) = &second {
            match action {
                MaintscriptHelperAction::MoveConffile => {
                    require_absolute(MAINTSCRIPT_HELPER, action.name(), "new conffile", second)?
                }
                MaintscriptHelperAction::SymlinkToDirectory
                | MaintscriptHelperAction::DirectoryToSymlink => {
                    require_nonempty(MAINTSCRIPT_HELPER, action.name(), "symlink target", second)?
                }
                MaintscriptHelperAction::RemoveConffile => {}
            }
        }
        let prior_version = operation.get(required_paths).cloned();
        let package = operation.get(required_paths + 1).cloned();
        if let Some(value) = &prior_version {
            require_no_nul(MAINTSCRIPT_HELPER, action.name(), "prior version", value)?;
        }
        if let Some(value) = &package {
            require_no_nul(MAINTSCRIPT_HELPER, action.name(), "package", value)?;
        }
        match action {
            MaintscriptHelperAction::RemoveConffile => Ok(Self::RemoveConffile {
                conffile: first,
                prior_version,
                package,
                phase,
            }),
            MaintscriptHelperAction::MoveConffile => Ok(Self::MoveConffile {
                old_conffile: first,
                new_conffile: second.expect("validated second path"),
                prior_version,
                package,
                phase,
            }),
            MaintscriptHelperAction::SymlinkToDirectory => Ok(Self::SymlinkToDirectory {
                pathname: first,
                old_target: second.expect("validated symlink target"),
                prior_version,
                package,
                phase,
            }),
            MaintscriptHelperAction::DirectoryToSymlink => {
                let mut pathname = first;
                if pathname.len() > 1 && pathname.ends_with(b"/") {
                    pathname.pop();
                }
                Ok(Self::DirectoryToSymlink {
                    pathname,
                    new_target: second.expect("validated symlink target"),
                    prior_version,
                    package,
                    phase,
                })
            }
        }
    }

    fn argv(&self) -> Vec<Vec<u8>> {
        let (mut argv, prior_version, package, phase) = match self {
            Self::RemoveConffile {
                conffile,
                prior_version,
                package,
                phase,
            } => (
                vec![b"rm_conffile".to_vec(), conffile.clone()],
                prior_version,
                package,
                phase,
            ),
            Self::MoveConffile {
                old_conffile,
                new_conffile,
                prior_version,
                package,
                phase,
            } => (
                vec![
                    b"mv_conffile".to_vec(),
                    old_conffile.clone(),
                    new_conffile.clone(),
                ],
                prior_version,
                package,
                phase,
            ),
            Self::SymlinkToDirectory {
                pathname,
                old_target,
                prior_version,
                package,
                phase,
            } => (
                vec![
                    b"symlink_to_dir".to_vec(),
                    pathname.clone(),
                    old_target.clone(),
                ],
                prior_version,
                package,
                phase,
            ),
            Self::DirectoryToSymlink {
                pathname,
                new_target,
                prior_version,
                package,
                phase,
            } => (
                vec![
                    b"dir_to_symlink".to_vec(),
                    pathname.clone(),
                    new_target.clone(),
                ],
                prior_version,
                package,
                phase,
            ),
        };
        if let Some(prior_version) = prior_version {
            argv.push(prior_version.clone());
        } else if package.is_some() {
            argv.push(Vec::new());
        }
        if let Some(package) = package {
            argv.push(package.clone());
        }
        argv.push(b"--".to_vec());
        argv.extend(phase.argv());
        argv
    }
}

/// Complete public dpkg-maintscript-helper invocation grammar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DpkgMaintscriptHelperInvocation {
    Supports(MaintscriptHelperAction),
    Operation(Box<MaintscriptHelperOperation>),
    Help,
    Version,
}

impl DpkgMaintscriptHelperInvocation {
    pub fn parse(argv: &[Vec<u8>]) -> Result<Self, DpkgLifecycleGrammarError> {
        let command = argv
            .first()
            .ok_or(DpkgLifecycleGrammarError::MissingAction {
                tool: MAINTSCRIPT_HELPER,
            })?;
        match command.as_slice() {
            b"supports" => {
                exact_count("supports", &argv[1..], 1, "supported command")?;
                MaintscriptHelperAction::parse(&argv[1]).map(Self::Supports)
            }
            b"help" | b"--help" | b"-?" => {
                exact_count("help", &argv[1..], 0, "argument")?;
                Ok(Self::Help)
            }
            b"--version" => {
                exact_count("version", &argv[1..], 0, "argument")?;
                Ok(Self::Version)
            }
            b"_internal_pkg_must_own_file" => Err(private_internal_command(command)),
            _ => {
                let action = MaintscriptHelperAction::parse(command)?;
                let delimiter = argv
                    .iter()
                    .enumerate()
                    .skip(1)
                    .filter_map(|(index, value)| (value == b"--").then_some(index))
                    .collect::<Vec<_>>();
                let delimiter = match delimiter.as_slice() {
                    [] => {
                        return Err(DpkgLifecycleGrammarError::MissingDelimiter {
                            tool: MAINTSCRIPT_HELPER,
                            action: action.name(),
                        });
                    }
                    [delimiter] => *delimiter,
                    _ => {
                        return Err(DpkgLifecycleGrammarError::MultipleDelimiters {
                            tool: MAINTSCRIPT_HELPER,
                        });
                    }
                };
                let phase = MaintainerScriptPhase::parse(&argv[delimiter + 1..])?;
                MaintscriptHelperOperation::parse(action, &argv[1..delimiter], phase)
                    .map(Box::new)
                    .map(Self::Operation)
            }
        }
    }

    pub fn argv(&self) -> Vec<Vec<u8>> {
        match self {
            Self::Supports(action) => vec![b"supports".to_vec(), action.as_bytes().to_vec()],
            Self::Operation(operation) => operation.argv(),
            Self::Help => vec![b"--help".to_vec()],
            Self::Version => vec![b"--version".to_vec()],
        }
    }
}

/// Source-documented result contract for `supports`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaintscriptHelperSupportStatus {
    Supported,
    Unsupported,
}

impl MaintscriptHelperSupportStatus {
    pub fn from_exit_code(status: u8) -> Result<Self, DpkgLifecycleGrammarError> {
        match status {
            0 => Ok(Self::Supported),
            1 => Ok(Self::Unsupported),
            status => Err(DpkgLifecycleGrammarError::UnsupportedExitStatus {
                tool: MAINTSCRIPT_HELPER,
                status,
            }),
        }
    }

    pub const fn exit_code(self) -> u8 {
        match self {
            Self::Supported => 0,
            Self::Unsupported => 1,
        }
    }
}
