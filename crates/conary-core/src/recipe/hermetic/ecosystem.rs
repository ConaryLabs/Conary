// conary-core/src/recipe/hermetic/ecosystem.rs

use super::evidence::{EcosystemDependencyIdentity, EcosystemPolicyReport, PolicyStatus};
use super::source_identity::{CiMode, local_tree_identity};
use crate::error::Result;
use crate::hash;
use crate::recipe::BuildSystem;
use std::ffi::OsStr;
use std::fs;
use std::path::Component;
use std::path::Path;

pub fn evaluate_ecosystem_policy(
    build_system: BuildSystem,
    source_root: &Path,
    command_text: &str,
) -> Result<EcosystemPolicyReport> {
    match build_system {
        BuildSystem::Cargo => evaluate_cargo_policy(source_root, command_text),
        BuildSystem::Npm => Ok(unsupported_ecosystem_report("npm")),
        BuildSystem::Python => Ok(unsupported_ecosystem_report("python")),
        BuildSystem::Go => Ok(unsupported_ecosystem_report("go")),
        // M2a only evaluates language package managers with lockfile/network
        // resolution evidence. Native build-system risk remains owned by the
        // command-risk and Kitchen policy layers.
        BuildSystem::CMake | BuildSystem::Meson | BuildSystem::Autotools => {
            Ok(EcosystemPolicyReport::clean(ecosystem_name(build_system)))
        }
    }
}

fn evaluate_cargo_policy(source_root: &Path, command_text: &str) -> Result<EcosystemPolicyReport> {
    let lock_path = source_root.join("Cargo.lock");
    if !lock_path.is_file() {
        return Ok(blocked_report(
            "cargo",
            Vec::new(),
            vec!["Cargo.lock is required for hermetic Cargo builds in M2a".to_string()],
        ));
    }

    let lock_contents = fs::read_to_string(&lock_path)?;
    let lock_identity = file_identity("cargo", "Cargo.lock", &lock_path)?;
    let config = CargoConfigEvidence::read(source_root)?;
    let offline = command_has_offline_flag(command_text) || config.offline;

    let mut identities = vec![lock_identity];
    if config.should_record_identity() {
        if let Some(identity) = config.identity.clone() {
            identities.push(identity);
        }
    }

    let mut diagnostics = Vec::new();
    if !offline {
        diagnostics.push(
            "Cargo builds must use explicit --offline or .cargo/config.toml [net].offline = true"
                .to_string(),
        );
    }

    let has_registry_dependencies = cargo_lock_has_registry_dependencies(&lock_contents);
    if has_registry_dependencies {
        let vendor_path = source_root.join("vendor");
        if vendor_path.is_dir() {
            identities.push(vendor_identity(&vendor_path)?);
        } else {
            diagnostics.push(
                "Cargo.lock has registry dependencies; vendor/ is required for M2a hermetic policy"
                    .to_string(),
            );
        }

        if !config.exists {
            diagnostics.push(
                "Cargo.lock has registry dependencies; .cargo/config.toml must replace crates-io with vendor/"
                    .to_string(),
            );
        } else if !config.replaces_crates_io_with_vendor {
            diagnostics.push(
                ".cargo/config.toml must replace crates-io with a source directory pinned to vendor/"
                    .to_string(),
            );
        }

        diagnostics.extend(config.diagnostics.clone());
    }

    if diagnostics.is_empty() {
        Ok(EcosystemPolicyReport {
            ecosystem: "cargo".to_string(),
            status: PolicyStatus::Clean,
            identities,
            diagnostics,
        })
    } else {
        Ok(blocked_report("cargo", identities, diagnostics))
    }
}

fn blocked_report(
    ecosystem: impl Into<String>,
    identities: Vec<EcosystemDependencyIdentity>,
    diagnostics: Vec<String>,
) -> EcosystemPolicyReport {
    EcosystemPolicyReport {
        ecosystem: ecosystem.into(),
        status: PolicyStatus::Blocked,
        identities,
        diagnostics,
    }
}

fn unsupported_ecosystem_report(ecosystem: &str) -> EcosystemPolicyReport {
    blocked_report(
        ecosystem,
        Vec::new(),
        vec![format!(
            "{ecosystem} ecosystem: M2a has no accepted hermetic policy for it yet"
        )],
    )
}

#[derive(Debug, Clone)]
struct CargoConfigEvidence {
    exists: bool,
    offline: bool,
    replaces_crates_io_with_vendor: bool,
    identity: Option<EcosystemDependencyIdentity>,
    diagnostics: Vec<String>,
}

impl CargoConfigEvidence {
    fn read(source_root: &Path) -> Result<Self> {
        let path = source_root.join(".cargo/config.toml");
        if !path.is_file() {
            return Ok(Self {
                exists: false,
                offline: false,
                replaces_crates_io_with_vendor: false,
                identity: None,
                diagnostics: Vec::new(),
            });
        }

        let content = fs::read_to_string(&path)?;
        let identity = file_identity("cargo", ".cargo/config.toml", &path)?;
        let Ok(table) = content.parse::<toml::Table>() else {
            return Ok(Self {
                exists: true,
                offline: false,
                replaces_crates_io_with_vendor: false,
                identity: Some(identity),
                diagnostics: vec![
                    ".cargo/config.toml must be valid TOML to provide Cargo offline policy evidence"
                        .to_string(),
                ],
            });
        };

        Ok(Self {
            exists: true,
            offline: cargo_config_offline(&table),
            replaces_crates_io_with_vendor: cargo_config_replaces_crates_io_with_vendor(&table),
            identity: Some(identity),
            diagnostics: Vec::new(),
        })
    }

    fn should_record_identity(&self) -> bool {
        self.offline || self.replaces_crates_io_with_vendor || !self.diagnostics.is_empty()
    }
}

fn cargo_config_offline(table: &toml::Table) -> bool {
    table
        .get("net")
        .and_then(toml::Value::as_table)
        .and_then(|net| net.get("offline"))
        .and_then(toml::Value::as_bool)
        .unwrap_or(false)
}

fn cargo_config_replaces_crates_io_with_vendor(table: &toml::Table) -> bool {
    let Some(source) = table.get("source").and_then(toml::Value::as_table) else {
        return false;
    };
    let Some(replacement_name) = source
        .get("crates-io")
        .and_then(toml::Value::as_table)
        .and_then(|crates_io| crates_io.get("replace-with"))
        .and_then(toml::Value::as_str)
    else {
        return false;
    };
    let Some(directory) = source
        .get(replacement_name)
        .and_then(toml::Value::as_table)
        .and_then(|replacement| replacement.get("directory"))
        .and_then(toml::Value::as_str)
    else {
        return false;
    };

    cargo_source_directory_is_vendor(directory)
}

fn cargo_source_directory_is_vendor(directory: &str) -> bool {
    let components = Path::new(directory)
        .components()
        .filter(|component| !matches!(component, Component::CurDir))
        .collect::<Vec<_>>();

    matches!(
        components.as_slice(),
        [Component::Normal(component)] if *component == OsStr::new("vendor")
    )
}

fn command_has_offline_flag(command_text: &str) -> bool {
    command_text
        .split_whitespace()
        .any(|argument| argument == "--offline")
}

fn cargo_lock_has_registry_dependencies(content: &str) -> bool {
    content
        .lines()
        .map(str::trim)
        .any(|line| line.starts_with("source = \"registry+"))
}

fn file_identity(
    ecosystem: &str,
    evidence_path: &str,
    path: &Path,
) -> Result<EcosystemDependencyIdentity> {
    let mut file = fs::File::open(path)?;
    let hash = hash::sha256_reader_hex(&mut file)?;
    Ok(EcosystemDependencyIdentity {
        ecosystem: ecosystem.to_string(),
        evidence_path: evidence_path.to_string(),
        evidence_hash: format!("sha256:{hash}"),
    })
}

fn vendor_identity(path: &Path) -> Result<EcosystemDependencyIdentity> {
    let identity = local_tree_identity(path, CiMode::Off)?;
    Ok(EcosystemDependencyIdentity {
        ecosystem: "cargo".to_string(),
        evidence_path: "vendor".to_string(),
        evidence_hash: identity.tree_hash,
    })
}

fn ecosystem_name(build_system: BuildSystem) -> &'static str {
    match build_system {
        BuildSystem::Cargo => "cargo",
        BuildSystem::Npm => "npm",
        BuildSystem::Python => "python",
        BuildSystem::Go => "go",
        BuildSystem::CMake => "cmake",
        BuildSystem::Meson => "meson",
        BuildSystem::Autotools => "autotools",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::hermetic::PolicyStatus;
    use std::fs;
    use std::path::Path;

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    fn cargo_lock_without_registry_dependencies() -> &'static str {
        r#"# This file is automatically @generated by Cargo.
version = 3

[[package]]
name = "local-only"
version = "0.1.0"
"#
    }

    fn cargo_lock_with_registry_dependency() -> &'static str {
        r#"# This file is automatically @generated by Cargo.
version = 3

[[package]]
name = "serde"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "0000000000000000000000000000000000000000000000000000000000000000"
"#
    }

    fn cargo_vendor_config() -> &'static str {
        r#"[net]
offline = true

[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
"#
    }

    fn diagnostics(report: &EcosystemPolicyReport) -> String {
        report.diagnostics.join("\n")
    }

    fn identity_hash_for(report: &EcosystemPolicyReport, path: &str) -> Option<String> {
        report
            .identities
            .iter()
            .find(|identity| identity.evidence_path == path)
            .map(|identity| identity.evidence_hash.clone())
    }

    #[test]
    fn cargo_without_lock_is_blocked() {
        let dir = tempfile::tempdir().unwrap();

        let report =
            evaluate_ecosystem_policy(BuildSystem::Cargo, dir.path(), "cargo build --offline")
                .unwrap();

        assert_eq!(report.ecosystem, "cargo");
        assert_eq!(report.status, PolicyStatus::Blocked);
        assert!(
            diagnostics(&report).contains("Cargo.lock"),
            "expected Cargo.lock diagnostic, got {:?}",
            report.diagnostics
        );
    }

    #[test]
    fn cargo_lock_with_no_registry_dependencies_is_clean_when_offline_flag_present() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("Cargo.lock"),
            cargo_lock_without_registry_dependencies(),
        );

        let report = evaluate_ecosystem_policy(
            BuildSystem::Cargo,
            dir.path(),
            "cargo build --release --locked --offline",
        )
        .unwrap();

        assert_eq!(report.status, PolicyStatus::Clean);
        let lock_hash = identity_hash_for(&report, "Cargo.lock").expect("Cargo.lock identity");
        assert!(
            lock_hash.starts_with("sha256:"),
            "expected sha256 evidence hash, got {lock_hash}"
        );
        assert_eq!(report.identities.len(), 1);
    }

    #[test]
    fn cargo_registry_dependency_without_vendor_identity_is_blocked() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("Cargo.lock"),
            cargo_lock_with_registry_dependency(),
        );

        let report = evaluate_ecosystem_policy(
            BuildSystem::Cargo,
            dir.path(),
            "cargo build --release --locked --offline",
        )
        .unwrap();

        assert_eq!(report.status, PolicyStatus::Blocked);
        let diagnostics = diagnostics(&report);
        assert!(
            diagnostics.contains("vendor"),
            "expected vendor diagnostic, got {diagnostics:?}"
        );
        assert!(
            diagnostics.contains(".cargo/config.toml"),
            "expected .cargo/config.toml diagnostic, got {diagnostics:?}"
        );
    }

    #[test]
    fn cargo_registry_dependency_with_vendor_identity_is_recorded() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("Cargo.lock"),
            cargo_lock_with_registry_dependency(),
        );
        write(
            &dir.path().join("vendor/serde/src/lib.rs"),
            "pub fn serde() {}\n",
        );
        write(
            &dir.path().join(".cargo/config.toml"),
            cargo_vendor_config(),
        );

        let report = evaluate_ecosystem_policy(
            BuildSystem::Cargo,
            dir.path(),
            "cargo build --release --locked",
        )
        .unwrap();

        assert_eq!(report.status, PolicyStatus::Clean);
        for evidence_path in ["Cargo.lock", "vendor", ".cargo/config.toml"] {
            let evidence_hash =
                identity_hash_for(&report, evidence_path).expect("expected ecosystem identity");
            assert!(
                evidence_hash.starts_with("sha256:"),
                "expected sha256 hash for {evidence_path}, got {evidence_hash}"
            );
        }
    }

    #[test]
    fn cargo_lock_without_offline_flag_is_blocked() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("Cargo.lock"),
            cargo_lock_without_registry_dependencies(),
        );

        let report = evaluate_ecosystem_policy(
            BuildSystem::Cargo,
            dir.path(),
            "cargo build --release --locked",
        )
        .unwrap();

        assert_eq!(report.status, PolicyStatus::Blocked);
        assert!(
            diagnostics(&report).contains("--offline"),
            "expected --offline diagnostic, got {:?}",
            report.diagnostics
        );
    }

    #[test]
    fn npm_python_and_go_are_fail_closed_until_policy_is_explicit() {
        let dir = tempfile::tempdir().unwrap();

        for (build_system, ecosystem) in [
            (BuildSystem::Npm, "npm"),
            (BuildSystem::Python, "python"),
            (BuildSystem::Go, "go"),
        ] {
            let report = evaluate_ecosystem_policy(build_system, dir.path(), "").unwrap();

            assert_eq!(report.ecosystem, ecosystem);
            assert_eq!(report.status, PolicyStatus::Blocked);
            let diagnostics = diagnostics(&report);
            assert!(
                diagnostics.contains(ecosystem),
                "expected ecosystem name in diagnostic, got {diagnostics:?}"
            );
            assert!(
                diagnostics.contains("M2a has no accepted hermetic policy"),
                "expected M2a policy diagnostic, got {diagnostics:?}"
            );
        }
    }
}
