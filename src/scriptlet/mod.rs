// src/scriptlet/mod.rs

//! Scriptlet execution for package install/remove hooks
//!
//! This module handles executing package scriptlets with cross-distro support
//! for RPM, DEB, and Arch packages. Key features:
//!
//! - Distro-specific argument handling:
//!   - RPM: Integer count ($1=1 install, $1=2 upgrade, $1=0 remove)
//!   - DEB: Action words per Debian Policy ($1=install/configure/remove/upgrade)
//!   - Arch: Version strings ($1=new_version, $2=old_version for upgrades)
//! - Arch .INSTALL function wrapper generation
//! - Timeout protection (60 seconds)
//! - stdin nullification to prevent hangs
//! - Non-root install safety (skip scriptlets if root != "/")
//! - Optional container isolation for untrusted scripts

use crate::container::{BindMount, ContainerConfig, Sandbox, ScriptRisk, analyze_script};
use crate::db::models::ScriptletEntry;
use crate::error::{Error, Result};
use crate::packages::traits::{Scriptlet, ScriptletPhase};
use std::fs::{self, File};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;
use tempfile::TempDir;
use tracing::{debug, info, warn};
use wait_timeout::ChildExt;

/// Sandbox mode for scriptlet execution
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SandboxMode {
    /// No sandboxing - direct execution (default for compatibility)
    #[default]
    None,
    /// Automatic - sandbox based on script risk analysis
    Auto,
    /// Always sandbox all scripts
    Always,
}

impl SandboxMode {
    /// Parse sandbox mode from string (auto, always, never)
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "never" | "none" | "off" | "false" => Some(Self::None),
            "auto" => Some(Self::Auto),
            "always" | "on" | "true" => Some(Self::Always),
            _ => None,
        }
    }
}

/// Default timeout for scriptlet execution (60 seconds)
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// Package format types for argument handling
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageFormat {
    Rpm,
    Deb,
    Arch,
}

impl PackageFormat {
    /// Parse from string representation
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "rpm" => Some(Self::Rpm),
            "deb" => Some(Self::Deb),
            "arch" => Some(Self::Arch),
            _ => None,
        }
    }

    /// Convert to string representation
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Rpm => "rpm",
            Self::Deb => "deb",
            Self::Arch => "arch",
        }
    }
}

/// Execution mode determines arguments passed to scriptlets
#[derive(Debug, Clone)]
pub enum ExecutionMode {
    /// Fresh install
    Install,
    /// Package removal
    Remove,
    /// Upgrade from old version (for NEW package scriptlets)
    Upgrade { old_version: String },
    /// Upgrade removal (for OLD package scriptlets during upgrade)
    /// RPM: $1=1 (not 0, signaling "another version remains")
    /// DEB: "upgrade <new_version>" (not "remove")
    /// Arch: Should NOT be used - Arch skips old package scripts during upgrade
    UpgradeRemoval { new_version: String },
}

/// Scriptlet executor with cross-distro support
pub struct ScriptletExecutor {
    root: PathBuf,
    package_name: String,
    package_version: String,
    package_format: PackageFormat,
    timeout: Duration,
    sandbox_mode: SandboxMode,
}

impl ScriptletExecutor {
    /// Create a new executor
    pub fn new(
        root: &Path,
        name: &str,
        version: &str,
        format: PackageFormat,
    ) -> Self {
        Self {
            root: root.to_path_buf(),
            package_name: name.to_string(),
            package_version: version.to_string(),
            package_format: format,
            timeout: DEFAULT_TIMEOUT,
            sandbox_mode: SandboxMode::default(),
        }
    }

    /// Set custom timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set sandbox mode for scriptlet execution
    pub fn with_sandbox_mode(mut self, mode: SandboxMode) -> Self {
        self.sandbox_mode = mode;
        self
    }

    /// Execute a scriptlet from package parsing
    pub fn execute(&self, scriptlet: &Scriptlet, mode: &ExecutionMode) -> Result<()> {
        self.execute_impl(
            &scriptlet.phase.to_string(),
            &scriptlet.interpreter,
            &scriptlet.content,
            scriptlet.flags.as_deref(),
            mode,
        )
    }

    /// Execute a scriptlet from database entry
    pub fn execute_entry(&self, entry: &ScriptletEntry, mode: &ExecutionMode) -> Result<()> {
        self.execute_impl(
            &entry.phase,
            &entry.interpreter,
            &entry.content,
            entry.flags.as_deref(),
            mode,
        )
    }

    /// Core execution implementation
    fn execute_impl(
        &self,
        phase: &str,
        interpreter: &str,
        content: &str,
        _flags: Option<&str>,
        mode: &ExecutionMode,
    ) -> Result<()> {
        // Safety check: Don't run scriptlets on non-root installs
        // They would affect the host system, not the target root
        if self.root != Path::new("/") {
            warn!(
                "Skipping {} scriptlet: execution in non-root install paths ({}) is not yet supported",
                phase,
                self.root.display()
            );
            return Ok(());
        }

        // Prepare script content (Arch needs wrapper generation)
        let script_content = if self.package_format == PackageFormat::Arch {
            self.prepare_arch_wrapper(content, phase)
        } else {
            content.to_string()
        };

        // Analyze script for dangerous patterns
        let analysis = analyze_script(&script_content);

        // Determine if we should sandbox based on mode and risk
        let use_sandbox = match self.sandbox_mode {
            SandboxMode::None => false,
            SandboxMode::Always => true,
            SandboxMode::Auto => {
                // Sandbox if risk is Medium or higher
                analysis.risk >= ScriptRisk::Medium
            }
        };

        if !analysis.patterns.is_empty() {
            info!(
                "{} scriptlet risk analysis: {} - {:?}",
                phase,
                analysis.risk.as_str(),
                analysis.patterns
            );
        }

        info!(
            "Executing {} scriptlet for {} v{} (sandbox: {})",
            phase, self.package_name, self.package_version, use_sandbox
        );

        // Resolve interpreter (Arch always uses bash for wrapper)
        let interpreter_path = if self.package_format == PackageFormat::Arch {
            "/bin/bash".to_string()
        } else {
            interpreter.to_string()
        };

        // Validate interpreter exists - NO FALLBACK
        if !Path::new(&interpreter_path).exists() {
            return Err(Error::ScriptletError(format!(
                "Interpreter not found: {}. Cannot execute {} scriptlet.",
                interpreter_path,
                phase
            )));
        }

        // Prepare arguments based on distro, mode, and phase
        let args = self.get_args(mode, phase);

        // Build environment variables
        let env = [
            ("CONARY_PACKAGE_NAME", self.package_name.as_str()),
            ("CONARY_PACKAGE_VERSION", self.package_version.as_str()),
            ("CONARY_ROOT", "/"),
            ("CONARY_PHASE", phase),
        ];

        if use_sandbox {
            // Execute in sandbox with custom timeout and writable bind mounts
            let config = ContainerConfig {
                timeout: self.timeout,
                bind_mounts: {
                    let mut mounts = ContainerConfig::default().bind_mounts;
                    // Add writable access to common scriptlet targets
                    mounts.push(BindMount::writable("/var", "/var"));
                    mounts.push(BindMount::writable("/etc", "/etc"));
                    mounts
                },
                ..ContainerConfig::default()
            };

            let mut sandbox = Sandbox::new(config);
            let (code, stdout, stderr) = sandbox.execute(
                &interpreter_path,
                &script_content,
                &args,
                &env,
            )?;

            // Log output
            if !stdout.is_empty() {
                for line in stdout.lines() {
                    info!("[{}] {}", phase, line);
                }
            }
            if !stderr.is_empty() {
                for line in stderr.lines() {
                    warn!("[{}] {}", phase, line);
                }
            }

            if code == 0 {
                info!("{} scriptlet completed successfully (sandboxed)", phase);
                Ok(())
            } else {
                Err(Error::ScriptletError(format!(
                    "{} scriptlet failed with exit code {} (sandboxed)",
                    phase, code
                )))
            }
        } else {
            // Execute directly (legacy behavior)
            self.execute_direct(phase, &interpreter_path, &script_content, &args, &env)
        }
    }

    /// Execute scriptlet directly without sandbox
    fn execute_direct(
        &self,
        phase: &str,
        interpreter: &str,
        content: &str,
        args: &[String],
        env: &[(&str, &str)],
    ) -> Result<()> {
        // Create temp directory for script
        let temp_dir = TempDir::new()?;
        let script_path = temp_dir.path().join("scriptlet.sh");

        {
            let mut file = File::create(&script_path)?;
            file.write_all(content.as_bytes())?;
            let mut perms = fs::metadata(&script_path)?.permissions();
            perms.set_mode(0o700);
            fs::set_permissions(&script_path, perms)?;
        }

        debug!(
            "Executing script: {} {} {:?}",
            interpreter,
            script_path.display(),
            args
        );

        // Execute with timeout and stdin nullification
        let mut cmd = Command::new(interpreter);
        cmd.arg(&script_path)
            .args(args)
            .stdin(Stdio::null()) // CRITICAL: Prevent stdin hangs
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        for (key, value) in env {
            cmd.env(*key, *value);
        }

        let mut child = cmd.spawn()
            .map_err(|e| Error::ScriptletError(format!("Failed to spawn scriptlet: {}", e)))?;

        // Wait with timeout
        match child.wait_timeout(self.timeout)? {
            Some(status) => {
                // Capture output for logging
                let output = child.wait_with_output()?;
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);

                if !stdout.is_empty() {
                    for line in stdout.lines() {
                        info!("[{}] {}", phase, line);
                    }
                }
                if !stderr.is_empty() {
                    for line in stderr.lines() {
                        warn!("[{}] {}", phase, line);
                    }
                }

                if status.success() {
                    info!("{} scriptlet completed successfully", phase);
                    Ok(())
                } else {
                    let code = status.code().unwrap_or(-1);
                    Err(Error::ScriptletError(format!(
                        "{} scriptlet failed with exit code {}",
                        phase, code
                    )))
                }
            }
            None => {
                // Timeout - kill the process
                let _ = child.kill();
                Err(Error::ScriptletError(format!(
                    "{} scriptlet timed out after {} seconds",
                    phase,
                    self.timeout.as_secs()
                )))
            }
        }
    }

    /// Get arguments based on distro and execution mode
    ///
    /// Each distro has different argument semantics:
    /// - RPM: Integer count of packages remaining after operation
    /// - DEB: Action word + optional version string (per Debian Policy)
    /// - Arch: Version string(s)
    fn get_args(&self, mode: &ExecutionMode, phase: &str) -> Vec<String> {
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
                    ExecutionMode::Install => {
                        match phase {
                            "pre-install" => vec!["install".to_string()],
                            "post-install" => vec!["configure".to_string()],
                            _ => vec!["install".to_string()],
                        }
                    }
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
    fn prepare_arch_wrapper(&self, content: &str, phase: &str) -> String {
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

/// Convert ScriptletPhase to string for database storage
pub fn phase_to_string(phase: ScriptletPhase) -> String {
    phase.to_string()
}

/// Parse phase string back to ScriptletPhase
pub fn phase_from_string(s: &str) -> Option<ScriptletPhase> {
    match s {
        "pre-install" => Some(ScriptletPhase::PreInstall),
        "post-install" => Some(ScriptletPhase::PostInstall),
        "pre-remove" => Some(ScriptletPhase::PreRemove),
        "post-remove" => Some(ScriptletPhase::PostRemove),
        "pre-upgrade" => Some(ScriptletPhase::PreUpgrade),
        "post-upgrade" => Some(ScriptletPhase::PostUpgrade),
        "pre-transaction" => Some(ScriptletPhase::PreTransaction),
        "post-transaction" => Some(ScriptletPhase::PostTransaction),
        "trigger" => Some(ScriptletPhase::Trigger),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_package_format_from_str() {
        assert_eq!(PackageFormat::parse("rpm"), Some(PackageFormat::Rpm));
        assert_eq!(PackageFormat::parse("deb"), Some(PackageFormat::Deb));
        assert_eq!(PackageFormat::parse("arch"), Some(PackageFormat::Arch));
        assert_eq!(PackageFormat::parse("unknown"), None);
    }

    #[test]
    fn test_rpm_args() {
        let executor = ScriptletExecutor::new(
            Path::new("/"),
            "test-pkg",
            "1.0.0",
            PackageFormat::Rpm,
        );

        assert_eq!(executor.get_args(&ExecutionMode::Install, "pre-install"), vec!["1"]);
        assert_eq!(executor.get_args(&ExecutionMode::Remove, "pre-remove"), vec!["0"]);
        assert_eq!(
            executor.get_args(&ExecutionMode::Upgrade {
                old_version: "0.9.0".to_string()
            }, "pre-install"),
            vec!["2"]
        );
        // UpgradeRemoval: old package scripts get $1=1 (NOT 0!)
        assert_eq!(
            executor.get_args(&ExecutionMode::UpgradeRemoval {
                new_version: "1.0.0".to_string()
            }, "pre-remove"),
            vec!["1"]
        );
    }

    #[test]
    fn test_deb_args() {
        let executor = ScriptletExecutor::new(
            Path::new("/"),
            "test-pkg",
            "1.0.0",
            PackageFormat::Deb,
        );

        // Fresh install
        assert_eq!(executor.get_args(&ExecutionMode::Install, "pre-install"), vec!["install"]);
        assert_eq!(executor.get_args(&ExecutionMode::Install, "post-install"), vec!["configure"]);

        // Remove
        assert_eq!(executor.get_args(&ExecutionMode::Remove, "pre-remove"), vec!["remove"]);
        assert_eq!(executor.get_args(&ExecutionMode::Remove, "post-remove"), vec!["remove"]);

        // Upgrade
        assert_eq!(
            executor.get_args(&ExecutionMode::Upgrade {
                old_version: "0.9.0".to_string()
            }, "pre-install"),
            vec!["upgrade", "0.9.0"]
        );
        assert_eq!(
            executor.get_args(&ExecutionMode::Upgrade {
                old_version: "0.9.0".to_string()
            }, "post-install"),
            vec!["configure", "0.9.0"]
        );
        // UpgradeRemoval: OLD package scripts get "upgrade <new_version>"
        assert_eq!(
            executor.get_args(&ExecutionMode::UpgradeRemoval {
                new_version: "1.0.0".to_string()
            }, "pre-remove"),
            vec!["upgrade", "1.0.0"]
        );
        assert_eq!(
            executor.get_args(&ExecutionMode::UpgradeRemoval {
                new_version: "1.0.0".to_string()
            }, "post-remove"),
            vec!["upgrade", "1.0.0"]
        );
    }

    #[test]
    fn test_arch_args() {
        let executor = ScriptletExecutor::new(
            Path::new("/"),
            "test-pkg",
            "1.0.0",
            PackageFormat::Arch,
        );

        assert_eq!(executor.get_args(&ExecutionMode::Install, "post-install"), vec!["1.0.0"]);
        assert_eq!(executor.get_args(&ExecutionMode::Remove, "pre-remove"), vec!["1.0.0"]);
        assert_eq!(
            executor.get_args(&ExecutionMode::Upgrade {
                old_version: "0.9.0".to_string()
            }, "post-upgrade"),
            vec!["1.0.0", "0.9.0"]
        );
    }

    #[test]
    fn test_arch_wrapper_generation() {
        let executor = ScriptletExecutor::new(
            Path::new("/"),
            "test-pkg",
            "1.0.0",
            PackageFormat::Arch,
        );

        let content = "post_install() {\n    echo \"Hello\"\n}";
        let wrapper = executor.prepare_arch_wrapper(content, "post-install");

        assert!(wrapper.contains("#!/bin/bash"));
        assert!(wrapper.contains("set -e"));
        assert!(wrapper.contains(content));
        assert!(wrapper.contains("post_install \"$@\""));
    }

    #[test]
    fn test_phase_conversion() {
        assert_eq!(phase_to_string(ScriptletPhase::PreInstall), "pre-install");
        assert_eq!(phase_from_string("pre-install"), Some(ScriptletPhase::PreInstall));
        assert_eq!(phase_from_string("invalid"), None);
    }
}
