// conary-core/src/scriptlet/arguments.rs

use super::{ExecutionMode, PackageFormat, ScriptletExecutor};
use tracing::warn;

impl ScriptletExecutor {
    /// Get arguments based on distro and execution mode
    ///
    /// Each distro has different argument semantics:
    /// - RPM: Integer count of packages remaining after operation
    /// - DEB: Action word + optional version string (per Debian Policy)
    /// - Arch: Version string(s)
    pub(super) fn get_args(&self, mode: &ExecutionMode, phase: &str) -> Vec<String> {
        match self.package_format {
            PackageFormat::Rpm => {
                // RPM uses integer arguments (count of packages remaining):
                // Install: $1 = 1
                // Upgrade (new pkg): $1 = 2
                // Upgrade (old pkg removal): $1 = 1 (NOT 0! another version remains)
                // Remove: $1 = 0
                match mode {
                    ExecutionMode::Install => vec!["1".to_string()],
                    ExecutionMode::Remove => vec!["0".to_string()],
                    ExecutionMode::Upgrade { .. } => vec!["2".to_string()],
                    ExecutionMode::UpgradeRemoval { .. } => vec!["1".to_string()],
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
                        "pre-install" => vec!["install".to_string()],
                        "post-install" => vec!["configure".to_string()],
                        _ => vec!["install".to_string()],
                    },
                    ExecutionMode::Remove => {
                        vec!["remove".to_string()]
                    }
                    ExecutionMode::Upgrade { old_version } => {
                        // For NEW package scripts during upgrade
                        match phase {
                            "pre-install" => vec!["upgrade".to_string(), old_version.clone()],
                            "post-install" => vec!["configure".to_string(), old_version.clone()],
                            _ => vec!["upgrade".to_string(), old_version.clone()],
                        }
                    }
                    ExecutionMode::UpgradeRemoval { new_version } => {
                        // For OLD package scripts during upgrade
                        // prerm/postrm get "upgrade <new_version>"
                        vec!["upgrade".to_string(), new_version.clone()]
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
                    ExecutionMode::Install => vec![self.package_version.clone()],
                    ExecutionMode::Remove => vec![self.package_version.clone()],
                    ExecutionMode::Upgrade { old_version } => {
                        vec![self.package_version.clone(), old_version.clone()]
                    }
                    ExecutionMode::UpgradeRemoval { .. } => {
                        // This should never be called for Arch - log warning
                        // Arch does NOT run old package scripts during upgrade
                        warn!("UpgradeRemoval mode called for Arch package - this is a bug!");
                        vec![self.package_version.clone()]
                    }
                }
            }
        }
    }

    /// Generate wrapper script for Arch .INSTALL function libraries
    ///
    /// Arch .INSTALL files define functions like post_install(), pre_upgrade(), etc.
    /// but don't call them. We need to source the file and call the appropriate function.
    pub(super) fn prepare_arch_wrapper(&self, content: &str, phase: &str) -> String {
        // Map phase to Arch function name
        let function_name = match phase {
            "pre-install" => "pre_install",
            "post-install" => "post_install",
            "pre-remove" => "pre_remove",
            "post-remove" => "post_remove",
            "pre-upgrade" => "pre_upgrade",
            "post-upgrade" => "post_upgrade",
            _ => "post_install", // Fallback
        };

        format!(
            "#!/bin/bash\nset -e\n\n# Arch .INSTALL content:\n{}\n\n# Call the function if it exists\nif declare -f {} > /dev/null; then\n    {} \"$@\"\nfi\n",
            content, function_name, function_name
        )
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
            executor.get_args(&ExecutionMode::Install, "pre-install"),
            vec!["1"]
        );
        assert_eq!(
            executor.get_args(&ExecutionMode::Remove, "pre-remove"),
            vec!["0"]
        );
        assert_eq!(
            executor.get_args(
                &ExecutionMode::Upgrade {
                    old_version: "0.9.0".to_string()
                },
                "pre-install"
            ),
            vec!["2"]
        );
        // UpgradeRemoval: old package scripts get $1=1 (NOT 0!)
        assert_eq!(
            executor.get_args(
                &ExecutionMode::UpgradeRemoval {
                    new_version: "1.0.0".to_string()
                },
                "pre-remove"
            ),
            vec!["1"]
        );
    }

    #[test]
    fn test_deb_args() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Deb);

        // Fresh install
        assert_eq!(
            executor.get_args(&ExecutionMode::Install, "pre-install"),
            vec!["install"]
        );
        assert_eq!(
            executor.get_args(&ExecutionMode::Install, "post-install"),
            vec!["configure"]
        );

        // Remove
        assert_eq!(
            executor.get_args(&ExecutionMode::Remove, "pre-remove"),
            vec!["remove"]
        );
        assert_eq!(
            executor.get_args(&ExecutionMode::Remove, "post-remove"),
            vec!["remove"]
        );

        // Upgrade
        assert_eq!(
            executor.get_args(
                &ExecutionMode::Upgrade {
                    old_version: "0.9.0".to_string()
                },
                "pre-install"
            ),
            vec!["upgrade", "0.9.0"]
        );
        assert_eq!(
            executor.get_args(
                &ExecutionMode::Upgrade {
                    old_version: "0.9.0".to_string()
                },
                "post-install"
            ),
            vec!["configure", "0.9.0"]
        );
        // UpgradeRemoval: OLD package scripts get "upgrade <new_version>"
        assert_eq!(
            executor.get_args(
                &ExecutionMode::UpgradeRemoval {
                    new_version: "1.0.0".to_string()
                },
                "pre-remove"
            ),
            vec!["upgrade", "1.0.0"]
        );
        assert_eq!(
            executor.get_args(
                &ExecutionMode::UpgradeRemoval {
                    new_version: "1.0.0".to_string()
                },
                "post-remove"
            ),
            vec!["upgrade", "1.0.0"]
        );
    }

    #[test]
    fn test_arch_args() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Arch);

        assert_eq!(
            executor.get_args(&ExecutionMode::Install, "post-install"),
            vec!["1.0.0"]
        );
        assert_eq!(
            executor.get_args(&ExecutionMode::Remove, "pre-remove"),
            vec!["1.0.0"]
        );
        assert_eq!(
            executor.get_args(
                &ExecutionMode::Upgrade {
                    old_version: "0.9.0".to_string()
                },
                "post-upgrade"
            ),
            vec!["1.0.0", "0.9.0"]
        );
    }

    #[test]
    fn test_arch_wrapper_generation() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Arch);

        let content = "post_install() {\n    echo \"Hello\"\n}";
        let wrapper = executor.prepare_arch_wrapper(content, "post-install");

        assert!(wrapper.contains("#!/bin/bash"));
        assert!(wrapper.contains("set -e"));
        assert!(wrapper.contains(content));
        assert!(wrapper.contains("post_install \"$@\""));
    }
}
