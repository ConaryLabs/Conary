// conary-core/src/recipe/hermetic/ecosystem.rs

use super::evidence::{EcosystemDependencyIdentity, EcosystemPolicyReport, PolicyStatus};
use crate::error::{Error, Result};
use crate::hash;
use crate::recipe::BuildSystem;
use std::fs;
use std::path::{Component, Path, PathBuf};
use walkdir::WalkDir;

pub fn evaluate_ecosystem_policy(
    build_system: BuildSystem,
    source_root: &Path,
    command_text: &str,
) -> Result<EcosystemPolicyReport> {
    match build_system {
        BuildSystem::Cargo => evaluate_cargo_policy(source_root, command_text),
        BuildSystem::Npm => evaluate_npm_policy(source_root, command_text),
        BuildSystem::Python => Ok(blocked_report(
            "python",
            Vec::new(),
            vec![
                "Python hermetic publish support requires a lockfile and wheelhouse policy; M2 release publish fails closed for Python until that policy lands"
                    .to_string(),
            ],
        )),
        BuildSystem::Go => evaluate_go_policy(source_root, command_text),
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
    let locked = command_has_locked_flag(command_text);

    let mut identities = vec![lock_identity];
    if config.should_record_identity()
        && let Some(identity) = config.identity.clone()
    {
        identities.push(identity);
    }

    let mut diagnostics = Vec::new();
    if !offline {
        diagnostics.push(
            "Cargo builds must use explicit --offline or .cargo/config.toml [net].offline = true"
                .to_string(),
        );
    }
    if !locked {
        diagnostics.push(
            "Cargo builds must use --locked so Cargo.lock cannot be mutated during hermetic builds"
                .to_string(),
        );
    }

    let lock_sources = cargo_lock_source_values(&lock_contents);
    let has_registry_dependencies = lock_sources
        .iter()
        .any(|source| source.starts_with("registry+"));
    for source in lock_sources
        .iter()
        .filter(|source| !source.starts_with("registry+"))
    {
        diagnostics.push(format!(
            "unsupported Cargo.lock source {source:?}; M2a only accepts registry+ sources with pinned replacement evidence"
        ));
    }
    if has_registry_dependencies {
        if let Some(replacement) = config.replacement.clone() {
            identities.push(replacement.identity);
        } else if !config.exists {
            diagnostics.push(
                "Cargo.lock has registry dependencies; .cargo/config.toml must replace crates-io with vendor/ or a pinned Cargo cache"
                    .to_string(),
            );
        } else if !config.source_replacement_configured {
            diagnostics.push(
                ".cargo/config.toml must replace crates-io with vendor/ or a pinned Cargo cache"
                    .to_string(),
            );
        } else if !config.replaces_crates_io_with_vendor {
            diagnostics.push(
                ".cargo/config.toml must replace crates-io with a valid in-tree source directory or local registry"
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

fn evaluate_go_policy(source_root: &Path, command_text: &str) -> Result<EcosystemPolicyReport> {
    let mut identities = Vec::new();
    let mut diagnostics = Vec::new();
    let go_sum = source_root.join("go.sum");
    if go_sum.is_file() {
        identities.push(file_identity("go", "go.sum", &go_sum)?);
    } else {
        diagnostics.push("go.sum is required for hermetic Go builds".to_string());
    }
    let vendor = source_root.join("vendor");
    if vendor.is_dir() {
        identities.push(directory_identity("go", "vendor", &vendor)?);
    } else {
        diagnostics.push("vendor/ is required for accepted M2 Go hermetic publish".to_string());
    }
    if !command_text.contains("-mod=vendor") && !command_text.contains("GOFLAGS=-mod=vendor") {
        diagnostics.push("Go builds must use -mod=vendor or GOFLAGS=-mod=vendor".to_string());
    }
    if diagnostics.is_empty() {
        Ok(EcosystemPolicyReport {
            ecosystem: "go".to_string(),
            status: PolicyStatus::Clean,
            identities,
            diagnostics,
        })
    } else {
        Ok(blocked_report("go", identities, diagnostics))
    }
}

fn evaluate_npm_policy(source_root: &Path, command_text: &str) -> Result<EcosystemPolicyReport> {
    let mut identities = Vec::new();
    let mut diagnostics = Vec::new();
    let lockfile = [
        "package-lock.json",
        "npm-shrinkwrap.json",
        "pnpm-lock.yaml",
        "yarn.lock",
    ]
    .into_iter()
    .find_map(|name| {
        let path = source_root.join(name);
        path.is_file().then_some((name, path))
    });
    if let Some((name, path)) = lockfile {
        identities.push(file_identity("npm", name, &path)?);
    } else {
        diagnostics.push(
            "npm hermetic publish requires package-lock.json, npm-shrinkwrap.json, pnpm-lock.yaml, or yarn.lock"
                .to_string(),
        );
    }
    let node_modules = source_root.join("node_modules");
    let npm_cache = source_root.join(".npm-cache");
    if node_modules.is_dir() {
        identities.push(directory_identity("npm", "node_modules", &node_modules)?);
    } else if npm_cache.is_dir()
        && (command_text.contains("--cache .npm-cache")
            || command_text.contains("--cache=.npm-cache"))
    {
        identities.push(directory_identity("npm", ".npm-cache", &npm_cache)?);
    } else {
        diagnostics.push(
            "npm hermetic publish requires node_modules/ or .npm-cache recorded in BuildInputIdentity"
                .to_string(),
        );
    }
    if !command_text.contains("--offline") {
        diagnostics
            .push("npm hermetic publish requires explicit --offline command evidence".to_string());
    }
    if diagnostics.is_empty() {
        Ok(EcosystemPolicyReport {
            ecosystem: "npm".to_string(),
            status: PolicyStatus::Clean,
            identities,
            diagnostics,
        })
    } else {
        Ok(blocked_report("npm", identities, diagnostics))
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

#[derive(Debug, Clone)]
struct CargoConfigEvidence {
    exists: bool,
    offline: bool,
    replaces_crates_io_with_vendor: bool,
    source_replacement_configured: bool,
    replacement: Option<CargoSourceReplacement>,
    identity: Option<EcosystemDependencyIdentity>,
    diagnostics: Vec<String>,
}

#[derive(Debug, Clone)]
struct CargoSourceReplacement {
    identity: EcosystemDependencyIdentity,
}

impl CargoConfigEvidence {
    fn read(source_root: &Path) -> Result<Self> {
        let path = source_root.join(".cargo/config.toml");
        if !path.is_file() {
            return Ok(Self {
                exists: false,
                offline: false,
                replaces_crates_io_with_vendor: false,
                source_replacement_configured: false,
                replacement: None,
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
                source_replacement_configured: false,
                replacement: None,
                identity: Some(identity),
                diagnostics: vec![
                    ".cargo/config.toml must be valid TOML to provide Cargo offline policy evidence"
                        .to_string(),
                ],
            });
        };

        let source_replacement = cargo_config_source_replacement(source_root, &table)?;
        Ok(Self {
            exists: true,
            offline: cargo_config_offline(&table),
            replaces_crates_io_with_vendor: source_replacement.replaces_crates_io_with_vendor,
            source_replacement_configured: source_replacement.configured,
            replacement: source_replacement.replacement,
            identity: Some(identity),
            diagnostics: source_replacement.diagnostics,
        })
    }

    fn should_record_identity(&self) -> bool {
        self.offline
            || self.source_replacement_configured
            || self.replacement.is_some()
            || !self.diagnostics.is_empty()
    }
}

#[derive(Debug, Clone, Default)]
struct CargoSourceReplacementEvidence {
    configured: bool,
    replaces_crates_io_with_vendor: bool,
    replacement: Option<CargoSourceReplacement>,
    diagnostics: Vec<String>,
}

fn cargo_config_offline(table: &toml::Table) -> bool {
    table
        .get("net")
        .and_then(toml::Value::as_table)
        .and_then(|net| net.get("offline"))
        .and_then(toml::Value::as_bool)
        .unwrap_or(false)
}

fn cargo_config_source_replacement(
    source_root: &Path,
    table: &toml::Table,
) -> Result<CargoSourceReplacementEvidence> {
    let Some(source) = table.get("source").and_then(toml::Value::as_table) else {
        return Ok(CargoSourceReplacementEvidence::default());
    };
    let Some(replacement_name) = source
        .get("crates-io")
        .and_then(toml::Value::as_table)
        .and_then(|crates_io| crates_io.get("replace-with"))
        .and_then(toml::Value::as_str)
    else {
        return Ok(CargoSourceReplacementEvidence::default());
    };

    let mut evidence = CargoSourceReplacementEvidence {
        configured: true,
        ..CargoSourceReplacementEvidence::default()
    };
    let Some(replacement) = source.get(replacement_name).and_then(toml::Value::as_table) else {
        evidence.diagnostics.push(format!(
            ".cargo/config.toml source replacement {replacement_name:?} must define directory or local-registry"
        ));
        return Ok(evidence);
    };

    let Some(configured_path) = replacement
        .get("directory")
        .or_else(|| replacement.get("local-registry"))
        .and_then(toml::Value::as_str)
    else {
        evidence.diagnostics.push(format!(
            ".cargo/config.toml source replacement {replacement_name:?} must define directory or local-registry"
        ));
        return Ok(evidence);
    };

    let Ok(relative_path) = normalize_pinned_replacement_path(configured_path) else {
        evidence.diagnostics.push(format!(
            ".cargo/config.toml pinned cache/source replacement {configured_path:?} must be relative and must not contain parent traversal"
        ));
        return Ok(evidence);
    };

    let replacement_path = source_root.join(&relative_path);
    if !replacement_path.is_dir() {
        evidence.diagnostics.push(format!(
            ".cargo/config.toml pinned cache/source replacement {} is missing or is not a directory",
            normalized_path_display(&relative_path)
        ));
        return Ok(evidence);
    }

    let source_root = fs::canonicalize(source_root)?;
    let replacement_path = fs::canonicalize(&replacement_path)?;
    if !replacement_path.starts_with(&source_root) {
        evidence.diagnostics.push(format!(
            ".cargo/config.toml pinned cache/source replacement {} must stay inside source_root",
            normalized_path_display(&relative_path)
        ));
        return Ok(evidence);
    }

    let evidence_path = normalized_path_display(&relative_path);
    evidence.replaces_crates_io_with_vendor = evidence_path == "vendor";
    match replacement_identity(&evidence_path, &replacement_path) {
        Ok(identity) => {
            evidence.replacement = Some(CargoSourceReplacement { identity });
        }
        Err(error) => {
            evidence.diagnostics.push(error.to_string());
        }
    }
    Ok(evidence)
}

fn normalize_pinned_replacement_path(configured_path: &str) -> std::result::Result<PathBuf, ()> {
    let mut normalized = PathBuf::new();
    for component in Path::new(configured_path).components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return Err(()),
        }
    }

    if normalized.as_os_str().is_empty() {
        Err(())
    } else {
        Ok(normalized)
    }
}

fn command_has_offline_flag(command_text: &str) -> bool {
    command_has_flag(command_text, "--offline")
}

fn command_has_locked_flag(command_text: &str) -> bool {
    command_has_flag(command_text, "--locked")
}

fn command_has_flag(command_text: &str, flag: &str) -> bool {
    command_text
        .split_whitespace()
        .any(|argument| argument == flag)
}

fn cargo_lock_source_values(content: &str) -> Vec<String> {
    let Ok(table) = content.parse::<toml::Table>() else {
        return Vec::new();
    };
    let Some(packages) = table.get("package").and_then(toml::Value::as_array) else {
        return Vec::new();
    };

    packages
        .iter()
        .filter_map(toml::Value::as_table)
        .filter_map(|package| package.get("source").and_then(toml::Value::as_str))
        .map(str::to_string)
        .collect()
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

fn replacement_identity(evidence_path: &str, path: &Path) -> Result<EcosystemDependencyIdentity> {
    Ok(EcosystemDependencyIdentity {
        ecosystem: "cargo".to_string(),
        evidence_path: evidence_path.to_string(),
        evidence_hash: directory_tree_hash(path)?,
    })
}

fn directory_identity(
    ecosystem: &str,
    evidence_path: &str,
    path: &Path,
) -> Result<EcosystemDependencyIdentity> {
    Ok(EcosystemDependencyIdentity {
        ecosystem: ecosystem.to_string(),
        evidence_path: evidence_path.to_string(),
        evidence_hash: directory_tree_hash(path)?,
    })
}

#[derive(Debug, Clone)]
struct DirectoryTreeEntry {
    relative_path: PathBuf,
    hash: String,
    kind: DirectoryTreeEntryKind,
    mode: Option<u32>,
    symlink_target: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy)]
enum DirectoryTreeEntryKind {
    Regular,
    Symlink,
}

fn directory_tree_hash(root: &Path) -> Result<String> {
    let mut entries = directory_tree_entries(root)?;
    entries.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));

    let mut hasher = hash::Hasher::new(hash::HashAlgorithm::Sha256);
    for entry in entries {
        hasher.update(directory_tree_entry_kind_label(entry.kind).as_bytes());
        hasher.update(b"\0");
        hasher.update(&path_bytes(&entry.relative_path));
        hasher.update(b"\0");
        hasher.update(entry.hash.as_bytes());
        hasher.update(b"\0");
        if let Some(mode) = entry.mode {
            hasher.update(format!("{mode:o}").as_bytes());
        }
        hasher.update(b"\0");
        if let Some(target) = &entry.symlink_target {
            hasher.update(&path_bytes(target));
        }
        hasher.update(b"\n");
    }

    let hash = hasher.finalize();
    Ok(format!("sha256:{}", hash.value))
}

fn directory_tree_entries(root: &Path) -> Result<Vec<DirectoryTreeEntry>> {
    let mut entries = Vec::new();

    for entry in WalkDir::new(root).follow_links(false) {
        let entry = entry
            .map_err(|e| Error::IoError(format!("Failed to walk directory identity tree: {e}")))?;
        if entry.depth() == 0 {
            continue;
        }

        let metadata = fs::symlink_metadata(entry.path())?;
        let file_type = metadata.file_type();
        let kind = if file_type.is_file() {
            DirectoryTreeEntryKind::Regular
        } else if file_type.is_symlink() {
            DirectoryTreeEntryKind::Symlink
        } else {
            continue;
        };
        let relative_path = entry
            .path()
            .strip_prefix(root)
            .map_err(|e| {
                Error::ConfigError(format!(
                    "Failed to relativize directory identity tree entry: {e}"
                ))
            })?
            .to_path_buf();
        let (hash, symlink_target, mode) =
            directory_tree_entry_identity(root, entry.path(), kind, &metadata)?;

        entries.push(DirectoryTreeEntry {
            relative_path,
            hash,
            kind,
            mode,
            symlink_target,
        });
    }

    Ok(entries)
}

fn directory_tree_entry_identity(
    root: &Path,
    path: &Path,
    kind: DirectoryTreeEntryKind,
    metadata: &fs::Metadata,
) -> Result<(String, Option<PathBuf>, Option<u32>)> {
    match kind {
        DirectoryTreeEntryKind::Regular => {
            let mut file = fs::File::open(path)?;
            let hash = hash::sha256_reader_hex(&mut file)?;
            Ok((format!("sha256:{hash}"), None, mode_bits(metadata)))
        }
        DirectoryTreeEntryKind::Symlink => {
            let target = fs::read_link(path)?;
            validate_directory_tree_symlink_target(root, path, &target)?;
            let hash = hash::sha256_prefixed(&path_bytes(&target));
            Ok((hash, Some(target), None))
        }
    }
}

fn validate_directory_tree_symlink_target(
    root: &Path,
    link_path: &Path,
    target: &Path,
) -> Result<()> {
    let resolved_target = if target.is_absolute() {
        target.to_path_buf()
    } else {
        link_path.parent().unwrap_or(root).join(target)
    };
    let canonical_target = fs::canonicalize(&resolved_target).map_err(|e| {
        Error::ConfigError(format!(
            "directory identity tree symlink {} target {} could not be resolved and must stay inside directory identity tree: {e}",
            link_path.display(),
            target.display()
        ))
    })?;

    if !canonical_target.starts_with(root) {
        return Err(Error::ConfigError(format!(
            "directory identity tree symlink {} escapes directory identity tree; symlink targets must stay inside directory identity tree",
            link_path.display()
        )));
    }

    Ok(())
}

fn directory_tree_entry_kind_label(kind: DirectoryTreeEntryKind) -> &'static str {
    match kind {
        DirectoryTreeEntryKind::Regular => "regular",
        DirectoryTreeEntryKind::Symlink => "symlink",
    }
}

fn normalized_path_display(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(unix)]
fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(not(unix))]
fn path_bytes(path: &Path) -> Vec<u8> {
    path.to_string_lossy().as_bytes().to_vec()
}

#[cfg(unix)]
fn mode_bits(metadata: &fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(metadata.permissions().mode() & 0o7777)
}

#[cfg(not(unix))]
fn mode_bits(_metadata: &fs::Metadata) -> Option<u32> {
    None
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
    use std::process::Command;

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

    fn cargo_lock_with_git_dependency() -> &'static str {
        r#"# This file is automatically @generated by Cargo.
version = 3

[[package]]
name = "git-dep"
version = "0.1.0"
source = "git+https://example.invalid/repo"
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

    fn cargo_local_registry_config(path: &str) -> String {
        format!(
            r#"[net]
offline = true

[source.crates-io]
replace-with = "local-cache"

[source.local-cache]
local-registry = "{path}"
"#
        )
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

    fn git(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "git {:?} failed\nstdout:\n{}\nstderr:\n{}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
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
    fn cargo_lock_without_locked_flag_is_blocked() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("Cargo.lock"),
            cargo_lock_without_registry_dependencies(),
        );

        let report = evaluate_ecosystem_policy(
            BuildSystem::Cargo,
            dir.path(),
            "cargo build --release --offline",
        )
        .unwrap();

        assert_eq!(report.status, PolicyStatus::Blocked);
        assert!(
            diagnostics(&report).contains("--locked"),
            "expected --locked diagnostic, got {:?}",
            report.diagnostics
        );
    }

    #[test]
    fn cargo_git_lock_source_is_blocked_even_when_offline() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("Cargo.lock"),
            cargo_lock_with_git_dependency(),
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
            diagnostics.contains("git+https://example.invalid/repo"),
            "expected git source value in diagnostic, got {diagnostics:?}"
        );
        assert!(
            diagnostics.contains("unsupported"),
            "expected unsupported source diagnostic, got {diagnostics:?}"
        );
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
    fn cargo_registry_dependency_with_pinned_cache_identity_is_recorded() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("Cargo.lock"),
            cargo_lock_with_registry_dependency(),
        );
        write(
            &dir.path().join(".cargo/local-registry/index/serde"),
            "serde 1.0.0\n",
        );
        write(
            &dir.path().join(".cargo/config.toml"),
            &cargo_local_registry_config(".cargo/local-registry"),
        );

        let report = evaluate_ecosystem_policy(
            BuildSystem::Cargo,
            dir.path(),
            "cargo build --release --locked",
        )
        .unwrap();

        assert_eq!(report.status, PolicyStatus::Clean);
        for evidence_path in ["Cargo.lock", ".cargo/config.toml", ".cargo/local-registry"] {
            let evidence_hash =
                identity_hash_for(&report, evidence_path).expect("expected ecosystem identity");
            assert!(
                evidence_hash.starts_with("sha256:"),
                "expected sha256 hash for {evidence_path}, got {evidence_hash}"
            );
        }
    }

    #[test]
    fn cargo_pinned_cache_identity_hashes_ignored_git_worktree_contents() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init"]);
        write(&dir.path().join(".gitignore"), ".cargo/local-registry/\n");
        write(
            &dir.path().join("Cargo.lock"),
            cargo_lock_with_registry_dependency(),
        );
        write(
            &dir.path().join(".cargo/config.toml"),
            &cargo_local_registry_config(".cargo/local-registry"),
        );
        write(
            &dir.path().join(".cargo/local-registry/index/serde"),
            "serde 1.0.0\n",
        );

        let first = evaluate_ecosystem_policy(
            BuildSystem::Cargo,
            dir.path(),
            "cargo build --release --locked",
        )
        .unwrap();
        assert_eq!(first.status, PolicyStatus::Clean);
        let first_hash = identity_hash_for(&first, ".cargo/local-registry")
            .expect("expected pinned cache identity");

        write(
            &dir.path().join(".cargo/local-registry/index/serde"),
            "serde 1.0.1\n",
        );
        let second = evaluate_ecosystem_policy(
            BuildSystem::Cargo,
            dir.path(),
            "cargo build --release --locked",
        )
        .unwrap();
        assert_eq!(second.status, PolicyStatus::Clean);
        let second_hash = identity_hash_for(&second, ".cargo/local-registry")
            .expect("expected pinned cache identity");

        assert_ne!(
            first_hash, second_hash,
            "ignored cache content changes must change the recorded replacement identity"
        );
    }

    #[cfg(unix)]
    #[test]
    fn cargo_pinned_cache_with_symlink_escape_is_blocked() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("Cargo.lock"),
            cargo_lock_with_registry_dependency(),
        );
        write(
            &dir.path().join(".cargo/config.toml"),
            &cargo_local_registry_config(".cargo/local-registry"),
        );
        write(&dir.path().join("outside-cache.txt"), "outside\n");
        fs::create_dir_all(dir.path().join(".cargo/local-registry")).unwrap();
        symlink(
            "../../outside-cache.txt",
            dir.path().join(".cargo/local-registry/escape"),
        )
        .unwrap();

        let report = evaluate_ecosystem_policy(
            BuildSystem::Cargo,
            dir.path(),
            "cargo build --release --locked",
        )
        .unwrap();

        assert_eq!(report.status, PolicyStatus::Blocked);
        let diagnostics = diagnostics(&report);
        assert!(
            diagnostics.contains("symlink"),
            "expected symlink diagnostic, got {diagnostics:?}"
        );
        assert!(
            diagnostics.contains("directory identity tree"),
            "expected directory identity tree diagnostic, got {diagnostics:?}"
        );
    }

    #[test]
    fn cargo_registry_dependency_blocks_invalid_pinned_replacement_paths() {
        let cases = [
            ("/tmp/conary-cache", "relative"),
            ("../outside-cache", "parent"),
        ];

        for (replacement_path, expected_diagnostic) in cases {
            let dir = tempfile::tempdir().unwrap();
            write(
                &dir.path().join("Cargo.lock"),
                cargo_lock_with_registry_dependency(),
            );
            write(
                &dir.path().join(".cargo/config.toml"),
                &cargo_local_registry_config(replacement_path),
            );

            let report = evaluate_ecosystem_policy(
                BuildSystem::Cargo,
                dir.path(),
                "cargo build --release --locked",
            )
            .unwrap();

            assert_eq!(report.status, PolicyStatus::Blocked);
            let diagnostics = diagnostics(&report);
            assert!(
                diagnostics.contains(replacement_path),
                "expected replacement path in diagnostic, got {diagnostics:?}"
            );
            assert!(
                diagnostics.contains(expected_diagnostic),
                "expected {expected_diagnostic:?} diagnostic, got {diagnostics:?}"
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
    fn go_policy_accepts_go_sum_with_vendor_mode() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("go.sum"),
            "example.invalid/mod v1.0.0 h1:test\n",
        )
        .unwrap();
        std::fs::create_dir_all(temp.path().join("vendor")).unwrap();

        let report =
            evaluate_ecosystem_policy(BuildSystem::Go, temp.path(), "go build -mod=vendor ./...")
                .unwrap();

        assert_eq!(report.status, PolicyStatus::Clean);
        assert!(
            report
                .identities
                .iter()
                .any(|identity| identity.evidence_path == "go.sum")
        );
        assert!(
            report
                .identities
                .iter()
                .any(|identity| identity.evidence_path == "vendor")
        );
    }

    #[test]
    fn npm_policy_accepts_lockfile_with_offline_cache() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("package-lock.json"), "{}").unwrap();
        std::fs::create_dir_all(temp.path().join(".npm-cache")).unwrap();

        let report = evaluate_ecosystem_policy(
            BuildSystem::Npm,
            temp.path(),
            "npm ci --offline --cache .npm-cache",
        )
        .unwrap();

        assert_eq!(report.status, PolicyStatus::Clean);
        assert!(
            report
                .identities
                .iter()
                .any(|identity| identity.evidence_path == "package-lock.json")
        );
        assert!(
            report
                .identities
                .iter()
                .any(|identity| identity.evidence_path == ".npm-cache")
        );
    }

    #[test]
    fn python_policy_remains_fail_closed_until_wheelhouse_policy_lands() {
        let temp = tempfile::tempdir().unwrap();

        let report = evaluate_ecosystem_policy(
            BuildSystem::Python,
            temp.path(),
            "python -m pip install -r requirements.txt",
        )
        .unwrap();

        assert_eq!(report.status, PolicyStatus::Blocked);
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic.contains(
                "Python hermetic publish support requires a lockfile and wheelhouse policy",
            )
        }));
    }

    #[test]
    fn native_build_systems_are_clean_without_ecosystem_policy() {
        let dir = tempfile::tempdir().unwrap();

        for (build_system, ecosystem) in [
            (BuildSystem::CMake, "cmake"),
            (BuildSystem::Meson, "meson"),
            (BuildSystem::Autotools, "autotools"),
        ] {
            let report = evaluate_ecosystem_policy(build_system, dir.path(), "").unwrap();

            assert_eq!(report.ecosystem, ecosystem);
            assert_eq!(report.status, PolicyStatus::Clean);
            assert!(report.diagnostics.is_empty());
        }
    }
}
