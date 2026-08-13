// conary-core/src/scriptlet/arguments.rs

use super::{ExecutionMode, PackageFormat, ScriptletExecutor};
use crate::error::{Error, Result, ScriptletFailureKind};

impl ScriptletExecutor {
    /// Get arguments based on distro and execution mode
    ///
    /// Each distro has different argument semantics:
    /// - RPM: Integer count of packages remaining after operation
    /// - DEB: Action word + optional version string (per Debian Policy)
    /// - Arch: Version string(s)
    pub(super) fn get_args(&self, mode: &ExecutionMode, phase: &str) -> Result<Vec<String>> {
        let unsupported = || {
            Error::scriptlet(
                ScriptletFailureKind::ContractViolation,
                format!(
                    "unsupported {} lifecycle phase '{phase}' for {}",
                    package_format_name(self.package_format),
                    execution_mode_name(mode)
                ),
            )
        };
        match self.package_format {
            PackageFormat::Conary => match (mode, phase) {
                (ExecutionMode::Remove, "pre-remove") => Ok(Vec::new()),
                _ => Err(unsupported()),
            },
            PackageFormat::Rpm => {
                // RPM uses integer arguments (count of packages remaining):
                // Install: $1 = 1
                // Upgrade (new pkg): $1 = 2
                // Upgrade (old pkg removal): $1 = 1 (NOT 0! another version remains)
                // Remove: $1 = 0
                match mode {
                    ExecutionMode::Install if matches!(phase, "pre-install" | "post-install") => {
                        Ok(vec!["1".to_string()])
                    }
                    ExecutionMode::Remove if matches!(phase, "pre-remove" | "post-remove") => {
                        Ok(vec!["0".to_string()])
                    }
                    ExecutionMode::Upgrade { .. }
                        if matches!(phase, "pre-install" | "post-install") =>
                    {
                        Ok(vec!["2".to_string()])
                    }
                    ExecutionMode::UpgradeRemoval { .. }
                        if matches!(phase, "pre-remove" | "post-remove") =>
                    {
                        Ok(vec!["1".to_string()])
                    }
                    ExecutionMode::Install
                    | ExecutionMode::Remove
                    | ExecutionMode::Upgrade { .. }
                    | ExecutionMode::UpgradeRemoval { .. } => Err(unsupported()),
                }
            }
            PackageFormat::Deb => {
                // DEB uses action words + version strings (per Debian Policy):
                // preinst: install | upgrade <old-version>
                // postinst: configure <most-recently-configured-version>
                // prerm: remove | upgrade <new-version>
                // postrm: remove | upgrade <new-version>
                match mode {
                    ExecutionMode::Install => match phase {
                        "pre-install" => Ok(vec!["install".to_string()]),
                        "post-install" => Ok(vec!["configure".to_string()]),
                        _ => Err(unsupported()),
                    },
                    ExecutionMode::Remove => match phase {
                        "pre-remove" | "post-remove" => Ok(vec!["remove".to_string()]),
                        _ => Err(unsupported()),
                    },
                    ExecutionMode::Upgrade { old_version } => {
                        // For NEW package scripts during upgrade
                        match phase {
                            "pre-install" => Ok(vec!["upgrade".to_string(), old_version.clone()]),
                            "post-install" => {
                                Ok(vec!["configure".to_string(), old_version.clone()])
                            }
                            _ => Err(unsupported()),
                        }
                    }
                    ExecutionMode::UpgradeRemoval { new_version } => {
                        // For OLD package scripts during upgrade
                        // prerm/postrm get "upgrade <new_version>"
                        match phase {
                            "pre-remove" | "post-remove" => {
                                Ok(vec!["upgrade".to_string(), new_version.clone()])
                            }
                            _ => Err(unsupported()),
                        }
                    }
                }
            }
            PackageFormat::Arch => {
                // Arch uses version strings:
                // Install: $1 = new_version
                // Remove: $1 = old_version
                // Upgrade: $1 = new_version, $2 = old_version
                // UpgradeRemoval: Should NOT be called for Arch!
                match mode {
                    ExecutionMode::Install if matches!(phase, "pre-install" | "post-install") => {
                        Ok(vec![self.package_version.clone()])
                    }
                    ExecutionMode::Remove if matches!(phase, "pre-remove" | "post-remove") => {
                        Ok(vec![self.package_version.clone()])
                    }
                    ExecutionMode::Upgrade { old_version }
                        if matches!(phase, "pre-upgrade" | "post-upgrade") =>
                    {
                        Ok(vec![self.package_version.clone(), old_version.clone()])
                    }
                    ExecutionMode::Install
                    | ExecutionMode::Remove
                    | ExecutionMode::Upgrade { .. }
                    | ExecutionMode::UpgradeRemoval { .. } => Err(unsupported()),
                }
            }
            PackageFormat::Eopkg => Err(unsupported()),
        }
    }
}

fn package_format_name(package_format: PackageFormat) -> &'static str {
    match package_format {
        PackageFormat::Conary => "conary",
        PackageFormat::Rpm => "rpm",
        PackageFormat::Deb => "deb",
        PackageFormat::Arch => "arch",
        PackageFormat::Eopkg => "eopkg",
    }
}

fn execution_mode_name(mode: &ExecutionMode) -> &'static str {
    match mode {
        ExecutionMode::Install => "install",
        ExecutionMode::Remove => "remove",
        ExecutionMode::Upgrade { .. } => "upgrade",
        ExecutionMode::UpgradeRemoval { .. } => "upgrade-removal",
    }
}

#[cfg(test)]
mod tests {
    use super::super::{ExecutionMode, PackageFormat, ScriptletExecutor};
    use std::path::Path;

    #[test]
    fn test_rpm_args() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm);

        assert_eq!(
            executor
                .get_args(&ExecutionMode::Install, "pre-install")
                .unwrap(),
            vec!["1".to_string()]
        );
        assert_eq!(
            executor
                .get_args(&ExecutionMode::Remove, "pre-remove")
                .unwrap(),
            vec!["0".to_string()]
        );
        assert_eq!(
            executor
                .get_args(
                    &ExecutionMode::Upgrade {
                        old_version: "0.9.0".to_string()
                    },
                    "pre-install"
                )
                .unwrap(),
            vec!["2".to_string()]
        );
        // UpgradeRemoval: old package scripts get $1=1 (NOT 0!)
        assert_eq!(
            executor
                .get_args(
                    &ExecutionMode::UpgradeRemoval {
                        new_version: "1.0.0".to_string()
                    },
                    "pre-remove"
                )
                .unwrap(),
            vec!["1".to_string()]
        );
    }

    #[test]
    fn test_deb_args() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Deb);

        // Fresh install
        assert_eq!(
            executor
                .get_args(&ExecutionMode::Install, "pre-install")
                .unwrap(),
            vec!["install".to_string()]
        );
        assert_eq!(
            executor
                .get_args(&ExecutionMode::Install, "post-install")
                .unwrap(),
            vec!["configure".to_string()]
        );

        // Remove
        assert_eq!(
            executor
                .get_args(&ExecutionMode::Remove, "pre-remove")
                .unwrap(),
            vec!["remove".to_string()]
        );
        assert_eq!(
            executor
                .get_args(&ExecutionMode::Remove, "post-remove")
                .unwrap(),
            vec!["remove".to_string()]
        );

        // Upgrade
        assert_eq!(
            executor
                .get_args(
                    &ExecutionMode::Upgrade {
                        old_version: "0.9.0".to_string()
                    },
                    "pre-install"
                )
                .unwrap(),
            vec!["upgrade".to_string(), "0.9.0".to_string()]
        );
        assert_eq!(
            executor
                .get_args(
                    &ExecutionMode::Upgrade {
                        old_version: "0.9.0".to_string()
                    },
                    "post-install"
                )
                .unwrap(),
            vec!["configure".to_string(), "0.9.0".to_string()]
        );
        // UpgradeRemoval: OLD package scripts get "upgrade <new_version>"
        assert_eq!(
            executor
                .get_args(
                    &ExecutionMode::UpgradeRemoval {
                        new_version: "1.0.0".to_string()
                    },
                    "pre-remove"
                )
                .unwrap(),
            vec!["upgrade".to_string(), "1.0.0".to_string()]
        );
        assert_eq!(
            executor
                .get_args(
                    &ExecutionMode::UpgradeRemoval {
                        new_version: "1.0.0".to_string()
                    },
                    "post-remove"
                )
                .unwrap(),
            vec!["upgrade".to_string(), "1.0.0".to_string()]
        );
    }

    #[test]
    fn test_arch_args() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Arch);

        assert_eq!(
            executor
                .get_args(&ExecutionMode::Install, "post-install")
                .unwrap(),
            vec!["1.0.0".to_string()]
        );
        assert_eq!(
            executor
                .get_args(&ExecutionMode::Remove, "pre-remove")
                .unwrap(),
            vec!["1.0.0".to_string()]
        );
        assert_eq!(
            executor
                .get_args(
                    &ExecutionMode::Upgrade {
                        old_version: "0.9.0".to_string()
                    },
                    "post-upgrade"
                )
                .unwrap(),
            vec!["1.0.0".to_string(), "0.9.0".to_string()]
        );
    }

    #[test]
    fn guessed_lifecycle_arguments_are_rejected() {
        let conary =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Conary);
        assert!(
            conary
                .get_args(&ExecutionMode::Install, "post-install")
                .unwrap_err()
                .to_string()
                .contains("unsupported conary lifecycle phase 'post-install' for install")
        );

        let rpm = ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm);
        assert!(
            rpm.get_args(&ExecutionMode::Install, "trigger")
                .unwrap_err()
                .to_string()
                .contains("unsupported rpm lifecycle phase 'trigger' for install")
        );

        let deb = ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Deb);
        assert!(
            deb.get_args(&ExecutionMode::Install, "trigger")
                .unwrap_err()
                .to_string()
                .contains("unsupported deb lifecycle phase 'trigger' for install")
        );

        let arch = ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Arch);
        assert!(
            arch.get_args(
                &ExecutionMode::UpgradeRemoval {
                    new_version: "2.0.0".to_string(),
                },
                "pre-remove",
            )
            .unwrap_err()
            .to_string()
            .contains("unsupported arch lifecycle phase 'pre-remove' for upgrade-removal")
        );
    }
}
