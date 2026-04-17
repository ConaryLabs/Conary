// conary-core/src/bootstrap/tier2.rs

//! Phase 6: Tier-2 packages (BLFS + Conary self-hosting)
//!
//! After the base LFS system is complete and bootable, this phase installs
//! additional packages from Beyond Linux From Scratch (BLFS) that are needed
//! for Conary to function: PAM, OpenSSH, CA certificates, curl, sudo, nano,
//! Rust, and Conary itself. Once this phase completes, the system can manage
//! its own packages.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{debug, info, warn};

use super::build_runner::{ChecksumPolicy, PackageBuildRunner};
use super::chroot_env::ChrootEnv;
use super::config::BootstrapConfig;
use super::toolchain::Toolchain;
use crate::recipe::parse_recipe_file;

/// Tier-2 package build order (BLFS + Conary).
///
/// Currently unused -- `build_all()` returns `NotImplemented` until the
/// recipe-driven build pipeline is wired end-to-end.
#[allow(dead_code)]
const TIER2_ORDER: &[&str] = &[
    "linux-pam",
    "openssh",
    "make-ca",
    "curl",
    "sudo",
    "nano",
    "rust",
    "conary",
];

/// Errors specific to the Tier-2 build phase.
#[derive(Debug, thiserror::Error)]
pub enum Tier2Error {
    /// A package build step failed.
    #[error("Tier-2 build failed for {package}: {reason}")]
    BuildFailed { package: String, reason: String },

    /// The base system is not ready.
    #[error("Base system not ready: {0}")]
    BaseNotReady(String),

    /// Tier-2-specific preflight validation failed.
    #[error("Tier-2 preflight failed: {0}")]
    Preflight(String),

    /// SSH configuration failed.
    #[error("SSH configuration failed: {0}")]
    SshConfig(String),

    /// The staged Conary workspace input is missing or invalid.
    #[error("Staged conary source invalid: {0}")]
    StagedSource(String),

    /// Feature not yet implemented.
    #[error("not implemented: {0}")]
    NotImplemented(String),

    /// I/O error during the build.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Error from the shared build runner.
    #[error(transparent)]
    BuildRunner(#[from] super::build_runner::BuildRunnerError),
}

/// Builder for Phase 6 Tier-2 packages.
///
/// Builds BLFS packages and Conary itself, completing the self-hosting
/// bootstrap.
pub struct Tier2Builder {
    /// Working directory for build artifacts.
    #[allow(dead_code)] // Used once recipe-driven pipeline is wired
    work_dir: PathBuf,
    /// Root of the installed system.
    system_root: PathBuf,
    /// Bootstrap configuration.
    #[allow(dead_code)] // Used once recipe-driven pipeline is wired
    config: BootstrapConfig,
    /// System toolchain.
    #[allow(dead_code)] // Used once recipe-driven pipeline is wired
    toolchain: Toolchain,
    /// Shared build runner for source fetching and verification.
    runner: PackageBuildRunner,
}

impl Tier2Builder {
    /// Create a new Tier-2 builder.
    ///
    /// # Arguments
    ///
    /// * `work_dir` - scratch space for downloads and build trees
    /// * `system_root` - root of the installed LFS system
    /// * `config` - bootstrap configuration
    /// * `toolchain` - system toolchain from the completed LFS build
    ///
    /// # Errors
    ///
    /// Returns `Tier2Error::BaseNotReady` if `system_root` does not contain
    /// a usable system (missing `/usr/bin/gcc`).
    pub fn new(
        work_dir: &Path,
        system_root: &Path,
        config: BootstrapConfig,
        toolchain: Toolchain,
    ) -> Result<Self, Tier2Error> {
        let gcc = system_root.join("usr").join("bin").join("gcc");
        if !gcc.exists() {
            return Err(Tier2Error::BaseNotReady(format!(
                "GCC not found at {}, complete Phase 3 first",
                gcc.display()
            )));
        }

        let sources_dir = work_dir.join("sources");
        std::fs::create_dir_all(&sources_dir)?;

        let runner = PackageBuildRunner::new(&sources_dir, &config)
            .with_checksum_policy(ChecksumPolicy::StrictSha256);

        Ok(Self {
            work_dir: work_dir.to_path_buf(),
            system_root: system_root.to_path_buf(),
            config,
            toolchain,
            runner,
        })
    }

    /// Build all Tier-2 packages in order.
    pub fn build_all(&self) -> Result<(), Tier2Error> {
        info!(
            "Building Tier-2 packages into sysroot at {}",
            self.system_root.display()
        );

        let mut chroot_env = ChrootEnv::new(&self.system_root);
        chroot_env.setup().map_err(|e| {
            Tier2Error::Preflight(format!("failed to set up chroot environment: {e}"))
        })?;

        for package in TIER2_ORDER {
            info!("Building Tier-2 package: {package}");
            self.build_package(package)?;
        }

        info!("Tier-2 package build complete");
        Ok(())
    }

    fn vm_selfhost_inputs_dir(&self) -> PathBuf {
        self.work_dir.join("vm-selfhost").join("inputs")
    }

    fn staged_conary_bundle_paths(&self) -> (PathBuf, PathBuf) {
        let bundle = self
            .vm_selfhost_inputs_dir()
            .join("conary-workspace.tar.gz");
        let sidecar = bundle.with_file_name(format!(
            "{}.sha256",
            bundle
                .file_name()
                .expect("conary workspace bundle path must have a filename")
                .to_string_lossy()
        ));
        (bundle, sidecar)
    }

    fn parse_sha256_sidecar(&self, sidecar_path: &Path) -> Result<String, Tier2Error> {
        let content = fs::read_to_string(sidecar_path).map_err(|e| {
            Tier2Error::StagedSource(format!(
                "failed to read staged sha256 sidecar {}: {e}",
                sidecar_path.display()
            ))
        })?;

        let token = content.split_whitespace().next().ok_or_else(|| {
            Tier2Error::StagedSource(format!(
                "staged sha256 sidecar {} is empty",
                sidecar_path.display()
            ))
        })?;

        let digest = if let Some((algo, hash)) = token.split_once(':') {
            if algo != "sha256" {
                return Err(Tier2Error::StagedSource(format!(
                    "staged sha256 sidecar {} must use sha256, found {algo}",
                    sidecar_path.display()
                )));
            }
            hash
        } else {
            token
        };

        if digest.len() != 64 || !digest.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(Tier2Error::StagedSource(format!(
                "staged sha256 sidecar {} does not contain a valid sha256 digest",
                sidecar_path.display()
            )));
        }

        Ok(digest.to_string())
    }

    fn validate_staged_conary_source(&self) -> Result<PathBuf, Tier2Error> {
        let (bundle_path, sidecar_path) = self.staged_conary_bundle_paths();

        if !bundle_path.exists() {
            return Err(Tier2Error::StagedSource(format!(
                "missing staged conary workspace bundle at {}",
                bundle_path.display()
            )));
        }
        if !sidecar_path.exists() {
            return Err(Tier2Error::StagedSource(format!(
                "missing sha256 sidecar for staged conary workspace bundle at {}",
                sidecar_path.display()
            )));
        }

        let expected_sha256 = self.parse_sha256_sidecar(&sidecar_path)?;
        self.runner
            .verify_checksum("conary", &format!("sha256:{expected_sha256}"), &bundle_path)
            .map_err(|e| {
                Tier2Error::StagedSource(format!(
                    "checksum mismatch for staged conary workspace bundle: {e}"
                ))
            })?;

        Ok(bundle_path)
    }

    fn ensure_sqlite_prereq(&self) -> Result<(), Tier2Error> {
        let candidate_dirs = [
            self.system_root.join("usr/lib"),
            self.system_root.join("usr/lib64"),
            self.system_root.join("usr/lib/x86_64-conary-linux-gnu"),
            self.system_root.join("usr/lib/x86_64-linux-gnu"),
        ];

        for dir in candidate_dirs {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };

            for entry in entries.flatten() {
                let name = entry.file_name();
                if name.to_string_lossy().starts_with("libsqlite3.so") {
                    return Ok(());
                }
            }
        }

        Err(Tier2Error::Preflight(format!(
            "sqlite prerequisite missing from sysroot {}; install sqlite during Phase 3 before building conary",
            self.system_root.display()
        )))
    }

    fn tier2_build_root(&self) -> PathBuf {
        self.system_root.join("var/tmp/conary-bootstrap/tier2")
    }

    fn prepare_build_dirs(&self, package: &str) -> Result<(PathBuf, PathBuf), Tier2Error> {
        let package_root = self.tier2_build_root().join(package);
        let src_dir = package_root.join("src");
        let build_dir = package_root.join("build");

        if package_root.exists() {
            fs::remove_dir_all(&package_root)?;
        }
        fs::create_dir_all(&src_dir)?;
        fs::create_dir_all(&build_dir)?;

        Ok((src_dir, build_dir))
    }

    fn chroot_env_vars(&self) -> Vec<(String, String)> {
        vec![
            ("PATH".into(), "/usr/bin:/usr/sbin".into()),
            ("HOME".into(), "/root".into()),
            ("TERM".into(), "xterm".into()),
            ("LC_ALL".into(), "C".into()),
            ("TZ".into(), "UTC".into()),
            ("SOURCE_DATE_EPOCH".into(), "0".into()),
            ("MAKEFLAGS".into(), format!("-j{}", self.config.jobs)),
        ]
    }

    fn path_in_chroot(&self, host_path: &Path) -> Result<String, Tier2Error> {
        let relative =
            host_path
                .strip_prefix(&self.system_root)
                .map_err(|_| Tier2Error::BuildFailed {
                    package: "tier2".to_string(),
                    reason: format!(
                        "path {} is not inside sysroot {}",
                        host_path.display(),
                        self.system_root.display()
                    ),
                })?;

        Ok(format!("/{}", relative.display()))
    }

    fn build_package(&self, package: &str) -> Result<(), Tier2Error> {
        let recipe_path = Path::new("recipes/tier2").join(format!("{package}.toml"));
        let recipe = parse_recipe_file(&recipe_path).map_err(|e| Tier2Error::BuildFailed {
            package: package.to_string(),
            reason: format!("failed to parse recipe: {e}"),
        })?;

        if package == "conary" {
            self.ensure_sqlite_prereq()?;
        }

        let source_archive = if package == "conary" {
            self.validate_staged_conary_source()?
        } else {
            self.runner.fetch_source(package, &recipe)?
        };

        let (src_dir, _build_dir) = self.prepare_build_dirs(package)?;
        self.runner
            .extract_source_strip(&source_archive, &src_dir)?;
        self.runner
            .fetch_additional_sources(package, &recipe, &src_dir)?;

        let src_dir_in_chroot = self.path_in_chroot(&src_dir)?;
        let script = format!(
            "set -e\ncd {}\n{}",
            src_dir_in_chroot,
            super::assemble_build_script(&recipe, "/")
        );

        let output = Command::new("chroot")
            .arg(&self.system_root)
            .arg("/bin/sh")
            .arg("-c")
            .arg(&script)
            .env_clear()
            .envs(
                self.chroot_env_vars()
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.as_str())),
            )
            .output()
            .map_err(|e| Tier2Error::BuildFailed {
                package: package.to_string(),
                reason: format!("failed to execute chroot: {e}"),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Tier2Error::BuildFailed {
                package: package.to_string(),
                reason: format!("build failed in chroot:\n{stderr}"),
            });
        }

        info!("  [OK] {package} built successfully");
        Ok(())
    }

    /// Configure SSH inside the sysroot for bootstrap/test access.
    ///
    /// 1. Writes `/etc/ssh/sshd_config` with permissive settings suitable for
    ///    testing (root login, pubkey auth, password auth, empty passwords).
    /// 2. Enables `sshd.service` via a systemd symlink.
    /// 3. Generates SSH host keys using the sysroot's own `ssh-keygen` binary.
    ///    Produces a hard error if the binary is not found (we must never use
    ///    the host's `ssh-keygen`).
    /// 4. Generates an Ed25519 test keypair and installs the public key to
    ///    `/root/.ssh/authorized_keys`.
    ///
    /// # Errors
    ///
    /// Returns `Tier2Error::SshConfig` if `ssh-keygen` is not found in the
    /// sysroot or if key generation fails.
    pub fn add_ssh_config(&self) -> Result<(), Tier2Error> {
        info!(
            "Configuring SSH in sysroot at {}",
            self.system_root.display()
        );

        let root = &self.system_root;

        // ---- 1. Write sshd_config ----
        let ssh_dir = root.join("etc/ssh");
        std::fs::create_dir_all(&ssh_dir)?;

        let sshd_config = ssh_dir.join("sshd_config");
        std::fs::write(
            &sshd_config,
            "\
# conaryOS sshd_config -- bootstrap defaults
# Regenerate with stricter settings for production use.

PermitRootLogin yes
PubkeyAuthentication yes
PasswordAuthentication yes
PermitEmptyPasswords yes
UsePAM no

# Logging
SyslogFacility AUTH
LogLevel INFO

# Host keys
HostKey /etc/ssh/ssh_host_rsa_key
HostKey /etc/ssh/ssh_host_ecdsa_key
HostKey /etc/ssh/ssh_host_ed25519_key

# Subsystem
Subsystem sftp /usr/libexec/sftp-server
",
        )?;
        info!("Wrote {}", sshd_config.display());

        // ---- 2. Enable sshd.service via symlink ----
        let wants_dir = root.join("etc/systemd/system/multi-user.target.wants");
        std::fs::create_dir_all(&wants_dir)?;

        let service_link = wants_dir.join("sshd.service");
        let service_target = Path::new("/usr/lib/systemd/system/sshd.service");

        // Remove existing symlink if present, then create.
        if service_link.exists() || service_link.symlink_metadata().is_ok() {
            std::fs::remove_file(&service_link)?;
        }

        #[cfg(unix)]
        std::os::unix::fs::symlink(service_target, &service_link)?;
        #[cfg(not(unix))]
        std::fs::write(&service_link, service_target.display().to_string())?;

        info!("Enabled sshd.service via symlink");

        // ---- 3. Generate SSH host keys using the sysroot's ssh-keygen ----
        let ssh_keygen = root.join("usr/bin/ssh-keygen");
        if !ssh_keygen.exists() {
            return Err(Tier2Error::SshConfig(format!(
                "ssh-keygen not found at {} -- openssh must be installed in the \
                 sysroot before generating host keys",
                ssh_keygen.display()
            )));
        }

        for (key_type, key_file) in &[
            ("rsa", "ssh_host_rsa_key"),
            ("ecdsa", "ssh_host_ecdsa_key"),
            ("ed25519", "ssh_host_ed25519_key"),
        ] {
            let key_path = ssh_dir.join(key_file);
            if key_path.exists() {
                debug!("Host key {} already exists, skipping", key_path.display());
                continue;
            }

            info!("Generating {} host key", key_type);
            let status = std::process::Command::new(&ssh_keygen)
                .args(["-t", key_type, "-f"])
                .arg(&key_path)
                .args(["-N", "", "-q"])
                .status()
                .map_err(|e| {
                    Tier2Error::SshConfig(format!("failed to run ssh-keygen for {key_type}: {e}"))
                })?;

            if !status.success() {
                return Err(Tier2Error::SshConfig(format!(
                    "ssh-keygen failed for {key_type} (exit {})",
                    status.code().unwrap_or(-1)
                )));
            }
        }

        // ---- 4. Generate test keypair and install authorized_keys ----
        let dot_ssh = root.join("root/.ssh");
        std::fs::create_dir_all(&dot_ssh)?;

        let test_key = dot_ssh.join("id_ed25519");
        if !test_key.exists() {
            info!("Generating test SSH keypair");
            let status = std::process::Command::new(&ssh_keygen)
                .args(["-t", "ed25519", "-f"])
                .arg(&test_key)
                .args(["-N", "", "-q", "-C", "conary-bootstrap-test"])
                .status()
                .map_err(|e| {
                    Tier2Error::SshConfig(format!("failed to generate test keypair: {e}"))
                })?;

            if !status.success() {
                return Err(Tier2Error::SshConfig(format!(
                    "test keypair generation failed (exit {})",
                    status.code().unwrap_or(-1)
                )));
            }
        }

        // Install public key to authorized_keys
        let pub_key_path = dot_ssh.join("id_ed25519.pub");
        let auth_keys = dot_ssh.join("authorized_keys");

        if pub_key_path.exists() {
            let pub_key = std::fs::read_to_string(&pub_key_path)?;
            std::fs::write(&auth_keys, &pub_key)?;

            // Set permissions: 600 for authorized_keys, 700 for .ssh dir
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&auth_keys, std::fs::Permissions::from_mode(0o600))?;
                std::fs::set_permissions(&dot_ssh, std::fs::Permissions::from_mode(0o700))?;
            }

            info!("Installed test public key to {}", auth_keys.display());
        } else {
            warn!(
                "Public key {} not found after generation -- skipping authorized_keys",
                pub_key_path.display()
            );
        }

        info!("SSH configuration complete");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap::toolchain::ToolchainKind;

    fn make_toolchain(root: &Path) -> Toolchain {
        Toolchain {
            kind: ToolchainKind::System,
            path: root.join("usr"),
            target: "x86_64-conary-linux-gnu".to_string(),
            gcc_version: None,
            glibc_version: None,
            binutils_version: None,
            is_static: false,
        }
    }

    #[test]
    fn test_tier2_order_count() {
        assert_eq!(TIER2_ORDER.len(), 8);
    }

    #[test]
    fn test_tier2_order_starts_with_pam() {
        assert_eq!(TIER2_ORDER[0], "linux-pam");
    }

    #[test]
    fn test_tier2_order_ends_with_conary() {
        assert_eq!(TIER2_ORDER[TIER2_ORDER.len() - 1], "conary");
    }

    #[test]
    fn test_tier2_includes_openssh() {
        assert!(TIER2_ORDER.contains(&"openssh"));
    }

    #[test]
    fn test_tier2_includes_conary() {
        assert!(TIER2_ORDER.contains(&"conary"));
    }

    #[test]
    fn test_tier2_includes_rust() {
        assert!(TIER2_ORDER.contains(&"rust"));
    }

    #[test]
    fn test_new_requires_gcc() {
        let work = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let config = BootstrapConfig::new();
        let tc = make_toolchain(root.path());

        let result = Tier2Builder::new(work.path(), root.path(), config, tc);
        assert!(result.is_err());
    }

    #[test]
    fn test_new_succeeds_with_gcc() {
        let work = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let gcc_path = root.path().join("usr/bin");
        std::fs::create_dir_all(&gcc_path).unwrap();
        std::fs::write(gcc_path.join("gcc"), b"").unwrap();

        let config = BootstrapConfig::new();
        let tc = make_toolchain(root.path());

        let builder = Tier2Builder::new(work.path(), root.path(), config, tc);
        assert!(builder.is_ok());
    }

    #[test]
    fn test_conary_bundle_paths_resolve_under_vm_selfhost_inputs() {
        let work = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let gcc_path = root.path().join("usr/bin");
        std::fs::create_dir_all(&gcc_path).unwrap();
        std::fs::write(gcc_path.join("gcc"), b"").unwrap();

        let config = BootstrapConfig::new();
        let tc = make_toolchain(root.path());

        let builder = Tier2Builder::new(work.path(), root.path(), config, tc).unwrap();
        let (bundle_path, sidecar_path) = builder.staged_conary_bundle_paths();

        assert_eq!(
            bundle_path,
            work.path()
                .join("vm-selfhost/inputs/conary-workspace.tar.gz"),
            "conary staged source bundle path should live under vm-selfhost/inputs"
        );
        assert_eq!(
            sidecar_path,
            work.path()
                .join("vm-selfhost/inputs/conary-workspace.tar.gz.sha256"),
            "conary staged source checksum sidecar should live next to the bundle"
        );
    }

    #[test]
    fn test_validate_staged_conary_source_rejects_missing_sidecar() {
        let work = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let gcc_path = root.path().join("usr/bin");
        std::fs::create_dir_all(&gcc_path).unwrap();
        std::fs::write(gcc_path.join("gcc"), b"").unwrap();

        let config = BootstrapConfig::new();
        let tc = make_toolchain(root.path());

        let builder = Tier2Builder::new(work.path(), root.path(), config, tc).unwrap();
        let (bundle_path, _) = builder.staged_conary_bundle_paths();
        std::fs::create_dir_all(bundle_path.parent().unwrap()).unwrap();
        std::fs::write(&bundle_path, b"fake bundle").unwrap();

        let result = builder.validate_staged_conary_source();
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("sha256 sidecar"),
            "missing sidecar error should mention the staged checksum file"
        );
    }

    #[test]
    fn test_validate_staged_conary_source_rejects_mismatched_sha256() {
        let work = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let gcc_path = root.path().join("usr/bin");
        std::fs::create_dir_all(&gcc_path).unwrap();
        std::fs::write(gcc_path.join("gcc"), b"").unwrap();

        let config = BootstrapConfig::new();
        let tc = make_toolchain(root.path());

        let builder = Tier2Builder::new(work.path(), root.path(), config, tc).unwrap();
        let (bundle_path, sidecar_path) = builder.staged_conary_bundle_paths();
        std::fs::create_dir_all(bundle_path.parent().unwrap()).unwrap();
        std::fs::write(&bundle_path, b"fake bundle").unwrap();
        std::fs::write(
            &sidecar_path,
            "sha256:deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef\n",
        )
        .unwrap();

        let result = builder.validate_staged_conary_source();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("checksum mismatch"),
            "mismatched staged source should fail closed on checksum drift"
        );
    }

    #[test]
    fn test_tier2_preflight_rejects_missing_sqlite_for_conary() {
        let work = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let gcc_path = root.path().join("usr/bin");
        std::fs::create_dir_all(&gcc_path).unwrap();
        std::fs::write(gcc_path.join("gcc"), b"").unwrap();

        let config = BootstrapConfig::new();
        let tc = make_toolchain(root.path());

        let builder = Tier2Builder::new(work.path(), root.path(), config, tc).unwrap();
        let result = builder.ensure_sqlite_prereq();
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("sqlite"),
            "sqlite preflight failure should mention the missing prerequisite"
        );
    }

    #[test]
    fn test_add_ssh_config_fails_without_ssh_keygen() {
        let work = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let gcc_path = root.path().join("usr/bin");
        std::fs::create_dir_all(&gcc_path).unwrap();
        std::fs::write(gcc_path.join("gcc"), b"").unwrap();

        let config = BootstrapConfig::new();
        let tc = make_toolchain(root.path());

        let builder = Tier2Builder::new(work.path(), root.path(), config, tc).unwrap();
        let result = builder.add_ssh_config();
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert!(
            matches!(err, Tier2Error::SshConfig(_)),
            "Expected SshConfig error, got: {err}"
        );
        assert!(
            err.to_string().contains("ssh-keygen not found"),
            "Error message should mention ssh-keygen: {err}"
        );
    }

    #[test]
    fn test_add_ssh_config_writes_sshd_config() {
        // Even though add_ssh_config will fail (no real ssh-keygen), we can
        // verify the sshd_config file is written before the failure point.
        let work = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let gcc_path = root.path().join("usr/bin");
        std::fs::create_dir_all(&gcc_path).unwrap();
        std::fs::write(gcc_path.join("gcc"), b"").unwrap();

        let config = BootstrapConfig::new();
        let tc = make_toolchain(root.path());

        let builder = Tier2Builder::new(work.path(), root.path(), config, tc).unwrap();
        let _ = builder.add_ssh_config(); // expected to fail at ssh-keygen

        let sshd_config = root.path().join("etc/ssh/sshd_config");
        assert!(sshd_config.exists(), "sshd_config should be written");

        let content = std::fs::read_to_string(&sshd_config).unwrap();
        assert!(content.contains("PermitRootLogin yes"));
        assert!(content.contains("PubkeyAuthentication yes"));
        assert!(content.contains("PasswordAuthentication yes"));
        assert!(content.contains("PermitEmptyPasswords yes"));
        assert!(content.contains("UsePAM no"));
    }

    #[test]
    fn test_add_ssh_config_creates_systemd_symlink() {
        let work = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let gcc_path = root.path().join("usr/bin");
        std::fs::create_dir_all(&gcc_path).unwrap();
        std::fs::write(gcc_path.join("gcc"), b"").unwrap();

        let config = BootstrapConfig::new();
        let tc = make_toolchain(root.path());

        let builder = Tier2Builder::new(work.path(), root.path(), config, tc).unwrap();
        let _ = builder.add_ssh_config(); // expected to fail at ssh-keygen

        let wants_dir = root
            .path()
            .join("etc/systemd/system/multi-user.target.wants");
        assert!(wants_dir.exists(), "systemd wants directory should exist");

        let link = wants_dir.join("sshd.service");
        assert!(
            link.symlink_metadata().is_ok(),
            "sshd.service symlink should exist"
        );
    }

    #[test]
    fn test_tier2_error_display() {
        let err = Tier2Error::BuildFailed {
            package: "curl".to_string(),
            reason: "configure failed".to_string(),
        };
        assert!(err.to_string().contains("curl"));
        assert!(err.to_string().contains("configure failed"));

        let err = Tier2Error::SshConfig("test error".to_string());
        assert!(err.to_string().contains("test error"));
    }
}
