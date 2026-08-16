// conary-core/src/ccs/native_export/mod.rs
//! Native package format exporters.

pub mod arch;
pub mod deb;
pub mod rpm;

use crate::ccs::builder::{BuildResult, FileEntry};
use crate::ccs::manifest::Hooks;
use anyhow::Context;
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

/// Copy one source-backed regular file while verifying its signed authority.
///
/// The returned value is the MD5 digest required by Debian package metadata.
pub fn copy_file_content(
    result: &BuildResult,
    file: &FileEntry,
    destination: &Path,
) -> anyhow::Result<String> {
    let authority = file.content.as_ref().with_context(|| {
        format!(
            "regular payload node {} has no content authority",
            file.path
        )
    })?;
    let mut reader = result.open_file(file)?;
    let mut writer = File::create(destination)?;
    let mut sha256 = crate::hash::Hasher::new(crate::hash::HashAlgorithm::Sha256);
    let mut md5 = crate::hash::Hasher::new(crate::hash::HashAlgorithm::Md5);
    let mut size = 0_u64;
    let mut buffer = [0_u8; crate::packages::payload::PAYLOAD_IO_BUFFER_SIZE];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        writer.write_all(&buffer[..count])?;
        sha256.update(&buffer[..count]);
        md5.update(&buffer[..count]);
        size = size
            .checked_add(count as u64)
            .context("native export payload size overflow")?;
    }
    writer.flush()?;
    let actual_sha256 = sha256.finalize().value;
    if size != authority.size || actual_sha256 != authority.sha256 {
        anyhow::bail!("payload source does not match authority for {}", file.path);
    }
    Ok(md5.finalize().value)
}

/// Information that may be lost when exporting to a native package format.
#[derive(Debug, Default)]
pub struct LossReport {
    pub unsupported_features: Vec<String>,
    pub hook_notes: Vec<String>,
    pub dependency_notes: Vec<String>,
}

impl LossReport {
    pub fn is_empty(&self) -> bool {
        self.unsupported_features.is_empty()
            && self.hook_notes.is_empty()
            && self.dependency_notes.is_empty()
    }

    pub fn add_unsupported(&mut self, feature: &str) {
        self.unsupported_features.push(feature.to_string());
    }

    pub fn add_hook_note(&mut self, note: &str) {
        self.hook_notes.push(note.to_string());
    }

    pub fn add_dependency_note(&mut self, note: &str) {
        self.dependency_notes.push(note.to_string());
    }

    pub fn print_summary(&self, format_name: &str) {
        if self.is_empty() {
            return;
        }
        println!("  Conversion notes for {format_name}:");
        for note in &self.unsupported_features {
            println!("    [UNSUPPORTED] {note}");
        }
        for note in &self.hook_notes {
            println!("    [HOOK] {note}");
        }
        for note in &self.dependency_notes {
            println!("    [DEPENDENCY] {note}");
        }
    }
}

#[derive(Debug)]
pub struct GenerationResult {
    pub size: u64,
    pub loss_report: LossReport,
}

pub trait HookConverter {
    fn pre_install(&self, hooks: &Hooks) -> Option<String>;
    fn post_install(&self, hooks: &Hooks) -> Option<String>;
    fn pre_remove(&self, hooks: &Hooks) -> Option<String>;
    fn post_remove(&self, hooks: &Hooks) -> Option<String>;
}

pub struct CommonHookGenerator;

impl CommonHookGenerator {
    pub fn user_creation_commands(hooks: &Hooks) -> Vec<String> {
        let mut commands = Vec::new();
        for group in &hooks.groups {
            let flags = if group.system { "--system " } else { "" };
            let name = shell_escape(&group.name);
            commands.push(format!(
                "getent group {name} >/dev/null || groupadd {flags}{name}"
            ));
        }
        for user in &hooks.users {
            let mut flags = Vec::new();
            if user.system {
                flags.push("--system".to_string());
            }
            if let Some(home) = &user.home {
                flags.push(format!("--home-dir {}", shell_escape(home)));
            }
            if let Some(shell) = &user.shell {
                flags.push(format!("--shell {}", shell_escape(shell)));
            } else if user.system {
                flags.push("--shell /usr/sbin/nologin".to_string());
            }
            if let Some(group) = &user.group {
                flags.push(format!("--gid {}", shell_escape(group)));
            }
            let flags = if flags.is_empty() {
                String::new()
            } else {
                format!("{} ", flags.join(" "))
            };
            let name = shell_escape(&user.name);
            commands.push(format!(
                "getent passwd {name} >/dev/null || useradd {flags}{name}"
            ));
        }
        commands
    }

    pub fn directory_commands(hooks: &Hooks) -> Vec<String> {
        hooks
            .directories
            .iter()
            .map(|directory| {
                format!(
                    "install -d -m {} -o {} -g {} {}",
                    shell_escape(&directory.mode),
                    shell_escape(&directory.owner),
                    shell_escape(&directory.group),
                    shell_escape(&directory.path),
                )
            })
            .collect()
    }

    pub fn systemd_commands(hooks: &Hooks, enable: bool) -> Vec<String> {
        let mut commands = Vec::new();
        for unit in &hooks.systemd {
            if unit.enable && enable {
                let unit_name = shell_escape(&unit.unit);
                commands.push(format!(
                    "if command -v systemctl >/dev/null 2>&1; then systemctl daemon-reload; systemctl enable {unit_name}; fi"
                ));
            } else if !enable {
                let unit_name = shell_escape(&unit.unit);
                commands.push(format!(
                    "if command -v systemctl >/dev/null 2>&1; then systemctl stop {unit_name} 2>/dev/null || true; fi"
                ));
            }
        }
        commands
    }

    pub fn tmpfiles_commands(hooks: &Hooks) -> Vec<String> {
        if hooks.tmpfiles.is_empty() {
            Vec::new()
        } else {
            vec![
                "if command -v systemd-tmpfiles >/dev/null 2>&1; then systemd-tmpfiles --create; fi"
                    .to_string(),
            ]
        }
    }

    pub fn sysctl_commands(hooks: &Hooks) -> Vec<String> {
        hooks
            .sysctl
            .iter()
            .map(|sysctl| {
                let safe = |value: &str| {
                    !value.is_empty()
                        && value.bytes().all(|byte| {
                            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')
                        })
                };
                if safe(&sysctl.key) && safe(&sysctl.value) {
                    format!("sysctl -w {}={}", sysctl.key, sysctl.value)
                } else {
                    format!(
                        "sysctl -w {}={}",
                        shell_escape(&sysctl.key),
                        shell_escape(&sysctl.value)
                    )
                }
            })
            .collect()
    }
}

pub fn shell_escape(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Resolve a CCS architecture through the exact target-format contract.
pub fn arch_for_format(arch: Option<&str>, format: &str) -> anyhow::Result<String> {
    let arch = arch.ok_or_else(|| anyhow::anyhow!("CCS package has no target architecture"))?;
    let target = match format {
        "deb" => match arch {
            "x86_64" | "amd64" => "amd64",
            "aarch64" | "arm64" => "arm64",
            "i686" | "i386" => "i386",
            "armv7l" | "armhf" => "armhf",
            "all" => "all",
            other => anyhow::bail!("unsupported Debian target architecture '{other}'"),
        },
        "rpm" => match arch {
            "x86_64" | "amd64" => "x86_64",
            "aarch64" | "arm64" => "aarch64",
            "i686" => "i686",
            "i386" => "i386",
            "armv7l" | "armv7hl" => "armv7hl",
            "noarch" => "noarch",
            other => anyhow::bail!("unsupported RPM target architecture '{other}'"),
        },
        "arch" => match arch {
            "x86_64" | "amd64" => "x86_64",
            "any" => "any",
            other => anyhow::bail!("unsupported ALPM target architecture '{other}'"),
        },
        other => anyhow::bail!("unsupported native package format '{other}'"),
    };
    Ok(target.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packages::PackageFormat;
    use crate::payload::PayloadNodeKind;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn source_backed_copy_preserves_bytes_and_computes_debian_md5() {
        let result = crate::ccs::builder::test_support::minimal_file_build_result(
            "hello",
            "1.0.0",
            b"hello world",
        );
        let output = tempfile::NamedTempFile::new().unwrap();
        let digest = copy_file_content(&result, &result.files[0], output.path()).unwrap();

        assert_eq!(std::fs::read(output.path()).unwrap(), b"hello world");
        assert_eq!(digest, "5eb63bbbe01eeed093cb22bb8f5acdc3");
    }

    #[test]
    fn source_backed_copy_rejects_content_changed_after_authoring() {
        let source = tempfile::tempdir().unwrap();
        let source_file = source.path().join("payload");
        std::fs::write(&source_file, b"signed bytes").unwrap();
        let result = crate::ccs::builder::CcsBuilder::new(
            crate::ccs::manifest::CcsManifest::new_minimal("changed", "1.0.0"),
            source.path(),
        )
        .unwrap()
        .build()
        .unwrap();
        std::fs::write(source_file, b"changed bytes").unwrap();
        let output = tempfile::NamedTempFile::new().unwrap();

        let error = copy_file_content(&result, &result.files[0], output.path()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("payload source does not match authority")
        );
    }

    #[test]
    fn shell_escape_handles_quotes() {
        assert_eq!(shell_escape("it's"), "'it'\\''s'");
    }

    #[test]
    fn architecture_mapping_remains_format_specific() {
        assert_eq!(arch_for_format(Some("x86_64"), "deb").unwrap(), "amd64");
        assert_eq!(arch_for_format(Some("aarch64"), "deb").unwrap(), "arm64");
        assert_eq!(arch_for_format(Some("amd64"), "rpm").unwrap(), "x86_64");
        assert!(arch_for_format(Some("all"), "rpm").is_err());
        assert!(arch_for_format(Some("any"), "rpm").is_err());
        assert!(arch_for_format(Some("noarch"), "deb").is_err());
        assert!(arch_for_format(None, "arch").is_err());
        assert!(arch_for_format(Some("riscv64"), "arch").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn native_exporters_round_trip_explicit_directory_and_symlink_topology() {
        let source = tempfile::tempdir().unwrap();
        let state_dir = source.path().join("usr/lib/topology/state");
        let bin_dir = source.path().join("usr/bin");
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::create_dir_all(&bin_dir).unwrap();
        std::fs::set_permissions(&state_dir, std::fs::Permissions::from_mode(0o750)).unwrap();
        std::fs::write(bin_dir.join("topology-tool"), b"topology\n").unwrap();
        std::os::unix::fs::symlink("topology-tool", bin_dir.join("topology-link")).unwrap();

        let mut manifest = crate::ccs::manifest::CcsManifest::new_minimal("topology", "1.0.0");
        manifest.package.license = Some("MIT".to_string());
        manifest.package.homepage = Some("https://example.invalid/topology".to_string());
        manifest.package.authors = Some(crate::ccs::manifest::Authors {
            maintainers: vec!["Topology Test <topology@example.invalid>".to_string()],
            upstream: None,
        });
        manifest.package.platform = Some(crate::ccs::manifest::Platform {
            arch: Some("x86_64".to_string()),
            ..Default::default()
        });
        let result = crate::ccs::builder::CcsBuilder::new(manifest, source.path())
            .unwrap()
            .build()
            .unwrap();
        let output = tempfile::tempdir().unwrap();

        let rpm_path = output.path().join("topology.rpm");
        rpm::generate(&result, &rpm_path).unwrap();
        let rpm = crate::packages::rpm::RpmPackage::parse(rpm_path.to_str().unwrap()).unwrap();
        assert_topology("rpm", &rpm);

        let deb_path = output.path().join("topology.deb");
        deb::generate(&result, &deb_path).unwrap();
        let deb = crate::packages::deb::DebPackage::parse(deb_path.to_str().unwrap()).unwrap();
        assert_topology("deb", &deb);

        let arch_path = output.path().join("topology.pkg.tar.zst");
        arch::generate(&result, &arch_path).unwrap();
        let arch = crate::packages::arch::ArchPackage::parse(arch_path.to_str().unwrap()).unwrap();
        assert_topology("arch", &arch);
    }

    fn assert_topology(format: &str, package: &impl PackageFormat) {
        let payload = package.package_payload().unwrap();
        let directory = payload
            .files()
            .iter()
            .find(|file| file.path == "/usr/lib/topology/state")
            .expect("explicit topology directory");
        assert!(matches!(directory.node.kind, PayloadNodeKind::Directory));
        assert_eq!(directory.node.mode & 0o7777, 0o750);

        if format == "rpm" {
            assert!(
                payload.files().iter().all(|file| file.path != "/usr/bin"),
                "rpm must not claim the target filesystem package's default parent directory"
            );
            assert!(
                payload
                    .files()
                    .iter()
                    .all(|file| file.path != "/usr/lib/topology"),
                "rpm must leave default parent directories implicit"
            );
        }

        let symlink = payload
            .files()
            .iter()
            .find(|file| file.path == "/usr/bin/topology-link")
            .expect("topology symlink");
        let PayloadNodeKind::Symlink { target } = &symlink.node.kind else {
            panic!("{format} topology link is {:?}", symlink.node.kind);
        };
        assert_eq!(target, "topology-tool", "{format} symlink target");
    }

    #[test]
    fn loss_report_tracks_unsupported_features() {
        let mut report = LossReport::default();
        assert!(report.is_empty());

        report.add_unsupported("merkle tree verification");
        assert!(!report.is_empty());
        assert_eq!(report.unsupported_features.len(), 1);
    }
}
