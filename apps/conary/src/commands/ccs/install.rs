// src/commands/ccs/install.rs

//! CCS package installation
//!
//! Commands for installing CCS packages with signature verification,
//! dependency checking, and hook execution.

use super::super::open_db;
use super::payload_paths::validate_ccs_payload_paths;
use anyhow::{Context, Result};
use conary_core::ccs::{CcsPackage, TrustPolicy, verify};
use conary_core::components::ComponentType;
use conary_core::packages::traits::PackageFormat;
use conary_core::repository::versioning::{
    RepoVersionConstraint, VersionScheme, parse_repo_constraint, repo_version_satisfies,
};
use std::path::Path;

fn package_provided_names(ccs_pkg: &CcsPackage) -> std::collections::HashSet<String> {
    let mut provided = std::collections::HashSet::new();
    provided.insert(ccs_pkg.name().to_string());
    provided.extend(ccs_pkg.manifest().provides.capabilities.iter().cloned());
    for soname in &ccs_pkg.manifest().provides.sonames {
        provided.insert(soname.clone());
        provided.insert(format!("soname({soname})"));
    }
    for binary in &ccs_pkg.manifest().provides.binaries {
        provided.insert(binary.clone());
        provided.insert(format!("binary({binary})"));
    }
    for pkgconfig in &ccs_pkg.manifest().provides.pkgconfig {
        provided.insert(pkgconfig.clone());
        provided.insert(format!("pkgconfig({pkgconfig})"));
    }
    provided
}

fn package_self_provides(ccs_pkg: &CcsPackage, dep_name: &str) -> bool {
    package_provided_names(ccs_pkg).contains(dep_name)
}

pub(crate) fn enforce_ccs_capability_policy(
    ccs_pkg: &CcsPackage,
    allow_capabilities: bool,
    capability_policy: Option<&str>,
) -> Result<()> {
    let Some(cap_decl) = ccs_pkg.manifest().capabilities.as_ref() else {
        return Ok(());
    };

    use conary_core::capability::policy::{
        CapabilityPolicy, PolicyDecision, infer_linux_capabilities,
    };

    let cap_policy = CapabilityPolicy::load(capability_policy)?;
    let required_caps = infer_linux_capabilities(cap_decl);

    // Evaluate all caps, checking denied first so a denied capability is not
    // masked by an earlier prompted capability bailing first.
    for cap in &required_caps {
        if let PolicyDecision::Denied(msg) = cap_policy.evaluate(cap) {
            anyhow::bail!(
                "Package {} capability policy rejected: {} -- {}",
                ccs_pkg.name(),
                cap,
                msg,
            );
        }
    }

    for cap in &required_caps {
        match cap_policy.evaluate(cap) {
            PolicyDecision::Allowed | PolicyDecision::Denied(_) => {}
            PolicyDecision::Prompt(msg) => {
                if allow_capabilities {
                    println!("Capability {cap} approved via --allow-capabilities");
                } else {
                    anyhow::bail!(
                        "Package {} requires capability {}: {}. \
                         Use --allow-capabilities to approve.",
                        ccs_pkg.name(),
                        cap,
                        msg,
                    );
                }
            }
        }
    }

    Ok(())
}

fn installed_versions_satisfying_constraint(
    conn: &rusqlite::Connection,
    package_name: &str,
    version_constraint: Option<&str>,
) -> Result<Vec<String>> {
    let installed = conary_core::db::models::Trove::find_by_name(conn, package_name)?;
    if installed.is_empty() {
        return Ok(Vec::new());
    }

    let Some(version_constraint) = version_constraint.filter(|v| !v.trim().is_empty()) else {
        return Ok(installed.into_iter().map(|trove| trove.version).collect());
    };

    let matches = installed
        .into_iter()
        .filter_map(|trove| {
            version_satisfies_constraint(
                &trove.version,
                trove.version_scheme.as_deref(),
                version_constraint,
            )
            .then_some(trove.version)
        })
        .collect();

    Ok(matches)
}

fn validate_package_dependency(
    conn: &rusqlite::Connection,
    package_name: &str,
    version_constraint: Option<&str>,
    dry_run: bool,
) -> Result<()> {
    let matching_versions =
        installed_versions_satisfying_constraint(conn, package_name, version_constraint)?;
    if !matching_versions.is_empty() {
        return Ok(());
    }

    let installed_versions = conary_core::db::models::Trove::find_by_name(conn, package_name)?
        .into_iter()
        .map(|trove| trove.version)
        .collect::<Vec<_>>();
    if installed_versions.is_empty()
        && conary_core::db::models::ProvideEntry::is_declared_capability_satisfied(
            conn,
            package_name,
        )?
    {
        return Ok(());
    }

    if dry_run {
        println!("  Missing dependency: {package_name} (would fail)");
        return Ok(());
    }

    if installed_versions.is_empty() {
        anyhow::bail!(
            "Missing dependency: {}{}",
            package_name,
            version_constraint
                .map(|v| format!(" {v}"))
                .unwrap_or_default()
        );
    }

    anyhow::bail!(
        "dependency version mismatch: {} requires {} but installed versions are {}",
        package_name,
        version_constraint.unwrap_or("*"),
        installed_versions.join(", ")
    );
}

fn validate_incoming_version_against_dependents(
    conn: &rusqlite::Connection,
    package_name: &str,
    incoming_version: &str,
) -> Result<()> {
    let scheme =
        installed_package_version_scheme(conn, package_name)?.unwrap_or(VersionScheme::Rpm);
    let dependents = conary_core::db::models::DependencyEntry::find_dependents(conn, package_name)?;
    let mut violations = Vec::new();

    for dep in dependents {
        let Some(constraint_str) = dep.version_constraint.as_deref() else {
            continue;
        };
        if repo_constraint_set_satisfied(scheme, incoming_version, constraint_str)? {
            continue;
        }
        let dependent_name = conary_core::db::models::Trove::find_by_id(conn, dep.trove_id)?
            .map(|trove| trove.name)
            .unwrap_or_else(|| format!("trove-{}", dep.trove_id));
        violations.push(format!("{dependent_name} requires {constraint_str}"));
    }

    if violations.is_empty() {
        return Ok(());
    }

    anyhow::bail!(
        "dependency version mismatch: {} {} would break {}",
        package_name,
        incoming_version,
        violations.join(", ")
    );
}

fn version_satisfies_constraint(
    version: &str,
    version_scheme: Option<&str>,
    constraint: &str,
) -> bool {
    repo_constraint_set_satisfied(
        conary_core::repository::distro::version_scheme_or_rpm(version_scheme),
        version,
        constraint,
    )
    .unwrap_or(false)
}

fn installed_package_version_scheme(
    conn: &rusqlite::Connection,
    package_name: &str,
) -> Result<Option<VersionScheme>> {
    Ok(
        conary_core::db::models::Trove::find_by_name(conn, package_name)?
            .into_iter()
            .find_map(|trove| {
                conary_core::repository::distro::version_scheme_from_db(
                    trove.version_scheme.as_deref(),
                )
            }),
    )
}

#[derive(Debug, Clone)]
struct SelectedCcsComponents {
    names: Vec<String>,
    recognized_types: Vec<ComponentType>,
}

impl SelectedCcsComponents {
    fn to_install_component_selection(
        &self,
        available_names: &[String],
    ) -> super::super::install::ComponentSelection {
        if self.names.len() == available_names.len()
            && available_names
                .iter()
                .all(|available| self.names.iter().any(|name| name == available))
        {
            return super::super::install::ComponentSelection::All;
        }

        if self.recognized_types.is_empty() {
            return super::super::install::ComponentSelection::All;
        }

        super::super::install::ComponentSelection::Specific(self.recognized_types.clone())
    }
}

fn sorted_available_component_names(ccs_pkg: &CcsPackage) -> Vec<String> {
    let mut names: Vec<String> = ccs_pkg.components().keys().cloned().collect();
    names.sort();
    names
}

fn select_ccs_components(
    ccs_pkg: &CcsPackage,
    requested: Option<Vec<String>>,
) -> Result<SelectedCcsComponents> {
    let available = sorted_available_component_names(ccs_pkg);
    if available.is_empty() {
        if ccs_pkg.file_entries().is_empty() {
            return Ok(SelectedCcsComponents {
                names: Vec::new(),
                recognized_types: Vec::new(),
            });
        }
        anyhow::bail!(
            "Package {} does not contain any installable components",
            ccs_pkg.name()
        );
    }

    let names = if let Some(requested_components) = requested {
        let mut selected = Vec::new();
        let mut select_all = false;

        for raw in requested_components {
            let component = raw.trim().to_ascii_lowercase();
            if component.is_empty() {
                continue;
            }

            if component == "all" {
                select_all = true;
                break;
            }

            if !available
                .iter()
                .any(|available_name| available_name == &component)
            {
                anyhow::bail!(
                    "Unknown component '{}'. Available components: {}",
                    raw,
                    available.join(", ")
                );
            }

            if !selected.iter().any(|name| name == &component) {
                selected.push(component);
            }
        }

        if select_all {
            available.clone()
        } else if selected.is_empty() {
            anyhow::bail!(
                "No components selected. Available components: {}",
                available.join(", ")
            );
        } else {
            selected
        }
    } else {
        let mut defaults = Vec::new();
        for component in &ccs_pkg.manifest().components.default {
            let normalized = component.trim().to_ascii_lowercase();
            if available
                .iter()
                .any(|available_name| available_name == &normalized)
                && !defaults.iter().any(|name| name == &normalized)
            {
                defaults.push(normalized);
            }
        }

        if defaults.is_empty() {
            available.clone()
        } else {
            defaults
        }
    };

    let recognized_types = names
        .iter()
        .filter_map(|name| ComponentType::parse(name))
        .collect();

    Ok(SelectedCcsComponents {
        names,
        recognized_types,
    })
}

fn repo_constraint_set_satisfied(scheme: VersionScheme, version: &str, raw: &str) -> Result<bool> {
    for part in split_constraint_parts(raw) {
        let constraint = parse_repo_constraint(scheme, part)
            .ok_or_else(|| anyhow::anyhow!("invalid version constraint: {raw}"))?;
        if !repo_constraint_satisfies(scheme, version, &constraint) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn split_constraint_parts(raw: &str) -> impl Iterator<Item = &str> {
    raw.split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
}

fn repo_constraint_satisfies(
    scheme: VersionScheme,
    version: &str,
    constraint: &RepoVersionConstraint,
) -> bool {
    repo_version_satisfies(scheme, version, constraint)
}

/// Install a CCS package
///
/// This is a minimal implementation that validates and extracts the package.
/// Full transaction support will be added in a future iteration.
#[allow(clippy::too_many_arguments)]
pub async fn cmd_ccs_install(
    package: &str,
    db_path: &str,
    root: &str,
    dry_run: bool,
    allow_unsigned: bool,
    policy: Option<String>,
    components: Option<Vec<String>>,
    sandbox: crate::commands::SandboxMode,
    no_deps: bool,
    reinstall: bool,
    allow_capabilities: bool,
    capability_policy: Option<String>,
) -> Result<()> {
    cmd_ccs_install_with_replay_options(
        package,
        db_path,
        root,
        dry_run,
        allow_unsigned,
        policy,
        components,
        sandbox,
        no_deps,
        false,
        reinstall,
        allow_capabilities,
        capability_policy,
        super::super::install::LegacyReplayOptions::default(),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn cmd_ccs_install_with_replay_options(
    package: &str,
    db_path: &str,
    root: &str,
    dry_run: bool,
    allow_unsigned: bool,
    policy: Option<String>,
    components: Option<Vec<String>>,
    sandbox: crate::commands::SandboxMode,
    no_deps: bool,
    no_scripts: bool,
    reinstall: bool,
    allow_capabilities: bool,
    capability_policy: Option<String>,
    legacy_replay: super::super::install::LegacyReplayOptions,
) -> Result<()> {
    let package_path = Path::new(package);

    if !package_path.exists() {
        anyhow::bail!("Package not found: {}", package);
    }

    println!("Installing CCS package: {}", package_path.display());

    // Step 1: Verify signature (unless --allow-unsigned)
    if !allow_unsigned {
        let trust_policy = if let Some(policy_path) = &policy {
            TrustPolicy::from_file(Path::new(policy_path)).context("Failed to load trust policy")?
        } else {
            TrustPolicy::default()
        };

        let result = match verify::verify_package(package_path, &trust_policy) {
            Ok(result) => result,
            Err(err)
                if matches!(
                    err.downcast_ref::<conary_core::ccs::verify::VerifyError>(),
                    Some(conary_core::ccs::verify::VerifyError::NotSigned)
                ) =>
            {
                anyhow::bail!("Package is not signed. Use --allow-unsigned to install anyway.");
            }
            Err(err) => return Err(err).context("Package verification failed"),
        };
        if let Some(expired_warning) = result
            .warnings
            .iter()
            .find(|warning| warning.contains("seconds old"))
        {
            anyhow::bail!("Package signature expired: {expired_warning}");
        }
        if !result.valid {
            if trust_policy.allow_unsigned {
                println!(
                    "Warning: Package signature verification failed, but continuing (allow_unsigned policy)"
                );
                for warning in &result.warnings {
                    println!("  - {}", warning);
                }
            } else {
                anyhow::bail!(
                    "Package signature verification failed. Use --allow-unsigned to install anyway.\n  Signature: {:?}\n  Content: {:?}",
                    result.signature_status,
                    result.content_status
                );
            }
        } else {
            println!("Signature verified: {:?}", result.signature_status);
        }
    } else {
        println!("Warning: Skipping signature verification (--allow-unsigned)");
    }

    // Step 2: Parse the package
    println!("Parsing package...");
    let ccs_pkg = CcsPackage::parse(package)?;

    println!(
        "Package: {} v{} ({} files)",
        ccs_pkg.name(),
        ccs_pkg.version(),
        ccs_pkg.files().len()
    );

    let selected_components = select_ccs_components(&ccs_pkg, components)?;
    if selected_components.names.is_empty() {
        println!("Installing metadata-only package (no file components)");
    } else {
        println!(
            "Installing components: {}",
            selected_components.names.join(", ")
        );
    }

    enforce_ccs_capability_policy(&ccs_pkg, allow_capabilities, capability_policy.as_deref())?;

    // Step 3: Check for existing installation
    let mut conn = open_db(db_path)?;

    let existing = conary_core::db::models::Trove::find_by_name(&conn, ccs_pkg.name())?;
    if !existing.is_empty() {
        let old = &existing[0];
        if old.version == ccs_pkg.version() {
            if reinstall {
                println!(
                    "Reinstalling {} {} (--reinstall)",
                    ccs_pkg.name(),
                    ccs_pkg.version()
                );
            } else {
                anyhow::bail!(
                    "Package {} version {} is already installed",
                    ccs_pkg.name(),
                    ccs_pkg.version()
                );
            }
        }
        println!(
            "Upgrading {} from {} to {}",
            ccs_pkg.name(),
            old.version,
            ccs_pkg.version()
        );
    }
    validate_incoming_version_against_dependents(&conn, ccs_pkg.name(), ccs_pkg.version())?;

    // Step 4: Check dependencies
    if no_deps {
        println!("Skipping dependency check (--no-deps)");
    } else {
        println!("Checking dependencies...");
        for dep in &ccs_pkg.manifest().requires.packages {
            if package_self_provides(&ccs_pkg, &dep.name) {
                continue;
            }
            validate_package_dependency(&conn, &dep.name, dep.version.as_deref(), dry_run)?;
        }
        for cap in &ccs_pkg.manifest().requires.capabilities {
            let capability_name = cap.name();
            if package_self_provides(&ccs_pkg, capability_name) {
                continue;
            }
            let satisfied =
                conary_core::db::models::ProvideEntry::is_declared_capability_satisfied(
                    &conn,
                    capability_name,
                )?;
            if !satisfied {
                if dry_run {
                    println!("  Missing dependency: {capability_name} (would fail)");
                } else {
                    anyhow::bail!(
                        "Missing dependency: {}{}",
                        capability_name,
                        cap.version().map(|v| format!(" {v}")).unwrap_or_default()
                    );
                }
            }
        }
        println!("Dependencies satisfied.");
    }

    let available_components = sorted_available_component_names(&ccs_pkg);
    let component_selection =
        selected_components.to_install_component_selection(&available_components);

    if dry_run {
        super::super::install::install_ccs_package_transactionally(
            &mut conn,
            &ccs_pkg,
            super::super::install::CcsTransactionInstallOptions {
                db_path,
                root,
                dry_run,
                defer_generation: false,
                no_scripts,
                sandbox_mode: match sandbox {
                    crate::commands::SandboxMode::None => conary_core::scriptlet::SandboxMode::None,
                    crate::commands::SandboxMode::Auto => conary_core::scriptlet::SandboxMode::Auto,
                    crate::commands::SandboxMode::Always => {
                        conary_core::scriptlet::SandboxMode::Always
                    }
                },
                allow_downgrade: false,
                reinstall,
                selection_reason: None,
                component_selection,
                selected_manifest_components: Some(selected_components.names.clone()),
                repository_provenance: None,
                legacy_replay,
            },
        )?;
        return Ok(());
    }

    validate_ccs_payload_paths(Path::new(root), &ccs_pkg, &selected_components.names)?;

    let tx_result = super::super::install::install_ccs_package_transactionally(
        &mut conn,
        &ccs_pkg,
        super::super::install::CcsTransactionInstallOptions {
            db_path,
            root,
            dry_run,
            defer_generation: false,
            no_scripts,
            sandbox_mode: match sandbox {
                crate::commands::SandboxMode::None => conary_core::scriptlet::SandboxMode::None,
                crate::commands::SandboxMode::Auto => conary_core::scriptlet::SandboxMode::Auto,
                crate::commands::SandboxMode::Always => conary_core::scriptlet::SandboxMode::Always,
            },
            allow_downgrade: false,
            reinstall,
            selection_reason: None,
            component_selection,
            selected_manifest_components: Some(selected_components.names.clone()),
            repository_provenance: None,
            legacy_replay,
        },
    )?;
    let _changeset_id = tx_result.changeset_id;
    let post_commit_warnings = tx_result.post_commit_warnings;

    println!();
    if post_commit_warnings.is_empty() {
        println!(
            "Successfully installed {} v{}",
            ccs_pkg.name(),
            ccs_pkg.version()
        );
    } else {
        println!(
            "Installed {} v{} with warnings",
            ccs_pkg.name(),
            ccs_pkg.version()
        );
        for warning in &post_commit_warnings {
            println!("  - {warning}");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::installed_versions_satisfying_constraint;
    use super::validate_incoming_version_against_dependents;
    use super::validate_package_dependency;

    fn stage_test_boot_assets(root: &std::path::Path) {
        let kernel_version =
            conary_core::generation::builder::detect_kernel_version_from_troves(&[])
                .unwrap_or_else(|| "test-kernel".to_string());
        let boot_root = root.join("boot");
        std::fs::create_dir_all(boot_root.join("EFI/BOOT")).unwrap();
        std::fs::write(
            boot_root.join(format!("vmlinuz-{kernel_version}")),
            b"test-kernel",
        )
        .unwrap();
        std::fs::write(
            boot_root.join(format!("initramfs-{kernel_version}.img")),
            b"test-initramfs",
        )
        .unwrap();
        std::fs::write(boot_root.join("EFI/BOOT/BOOTX64.EFI"), b"test-efi").unwrap();
    }

    fn seed_test_init_trove(db_path: &str, db_dir: &std::path::Path) {
        use conary_core::db::models::{
            Changeset, ChangesetStatus, Component, FileEntry, ProvideEntry, Trove, TroveType,
        };

        let cas = conary_core::filesystem::CasStore::new(db_dir.join("objects")).unwrap();
        let init_content = b"#!/bin/sh\nexec true\n";
        let init_hash = cas.store(init_content).unwrap();
        let init_size = i64::try_from(init_content.len()).unwrap();
        let mut conn = conary_core::db::open(db_path).unwrap();

        conary_core::db::transaction(&mut conn, |tx| {
            let mut changeset = Changeset::new("Install test-init-1.0.0".to_string());
            let changeset_id = changeset.insert(tx)?;

            let mut trove = Trove::new(
                "test-init".to_string(),
                "1.0.0".to_string(),
                TroveType::Package,
            );
            trove.installed_by_changeset_id = Some(changeset_id);
            let trove_id = trove.insert(tx)?;

            let mut component = Component::new(trove_id, "runtime".to_string());
            let component_id = component.insert(tx)?;

            tx.execute(
                "INSERT OR IGNORE INTO file_contents (sha256_hash, content_path, size) VALUES (?1, ?2, ?3)",
                rusqlite::params![
                    &init_hash,
                    format!("objects/{}/{}", &init_hash[0..2], &init_hash[2..]),
                    init_size
                ],
            )?;

            let mut init = FileEntry::new(
                "/usr/sbin/init".to_string(),
                init_hash,
                init_size,
                0o755,
                trove_id,
            );
            init.component_id = Some(component_id);
            init.insert(tx)?;

            let mut provide = ProvideEntry::new(trove_id, "test-init".to_string(), Some("1.0.0".to_string()));
            provide.insert(tx)?;
            changeset.update_status(tx, ChangesetStatus::Applied)?;

            Ok(())
        })
        .unwrap();
    }

    fn ccs_init_file() -> (conary_core::ccs::FileEntry, Vec<u8>, String) {
        use conary_core::ccs::{FileEntry, FileType};

        let init_content = b"#!/bin/sh\nexec true\n".to_vec();
        let init_hash = conary_core::hash::sha256(&init_content);
        (
            FileEntry {
                path: "/usr/sbin/init".to_string(),
                hash: init_hash.clone(),
                size: init_content.len() as u64,
                mode: 0o100755,
                component: "runtime".to_string(),
                file_type: FileType::Regular,
                target: None,
                chunks: None,
            },
            init_content,
            init_hash,
        )
    }

    #[test]
    fn installed_versions_respect_version_constraints() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("conary.db");
        let db_path_str = db_path.to_str().unwrap();
        conary_core::db::init(db_path_str).unwrap();
        let conn = conary_core::db::open(db_path_str).unwrap();

        let mut trove_v1 = conary_core::db::models::Trove::new(
            "dep-base".to_string(),
            "1.0.0".to_string(),
            conary_core::db::models::TroveType::Package,
        );
        trove_v1.insert(&conn).unwrap();

        let mut trove_v2 = conary_core::db::models::Trove::new(
            "dep-base".to_string(),
            "2.0.0".to_string(),
            conary_core::db::models::TroveType::Package,
        );
        trove_v2.insert(&conn).unwrap();

        let matching =
            installed_versions_satisfying_constraint(&conn, "dep-base", Some(">=1.0, <2.0"))
                .unwrap();
        assert_eq!(matching, vec!["1.0.0".to_string()]);

        let not_matching =
            installed_versions_satisfying_constraint(&conn, "dep-base", Some(">=3.0")).unwrap();
        assert!(not_matching.is_empty());
    }

    #[test]
    fn installed_versions_respect_debian_version_constraints() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("conary.db");
        let db_path_str = db_path.to_str().unwrap();
        conary_core::db::init(db_path_str).unwrap();
        let conn = conary_core::db::open(db_path_str).unwrap();

        let mut prerelease = conary_core::db::models::Trove::new(
            "dep-base".to_string(),
            "1.0~beta1".to_string(),
            conary_core::db::models::TroveType::Package,
        );
        prerelease.version_scheme = Some("debian".to_string());
        prerelease.insert(&conn).unwrap();

        let mut stable = conary_core::db::models::Trove::new(
            "dep-base".to_string(),
            "1.0".to_string(),
            conary_core::db::models::TroveType::Package,
        );
        stable.version_scheme = Some("debian".to_string());
        stable.insert(&conn).unwrap();

        let matching =
            installed_versions_satisfying_constraint(&conn, "dep-base", Some(">= 1.0")).unwrap();
        assert_eq!(matching, vec!["1.0".to_string()]);
    }

    #[test]
    fn incoming_version_uses_arch_constraints_for_dependents() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("conary.db");
        let db_path_str = db_path.to_str().unwrap();
        conary_core::db::init(db_path_str).unwrap();
        let conn = conary_core::db::open(db_path_str).unwrap();

        let mut liba = conary_core::db::models::Trove::new(
            "dep-liba".to_string(),
            "1.0-1".to_string(),
            conary_core::db::models::TroveType::Package,
        );
        liba.version_scheme = Some("arch".to_string());
        liba.insert(&conn).unwrap();

        let mut app = conary_core::db::models::Trove::new(
            "dep-app".to_string(),
            "1.0-1".to_string(),
            conary_core::db::models::TroveType::Package,
        );
        app.version_scheme = Some("arch".to_string());
        let app_id = app.insert(&conn).unwrap();

        let mut dep = conary_core::db::models::DependencyEntry::new(
            app_id,
            "dep-liba".to_string(),
            None,
            "runtime".to_string(),
            Some(">= 1.0-2".to_string()),
        );
        dep.insert(&conn).unwrap();

        let error =
            validate_incoming_version_against_dependents(&conn, "dep-liba", "1.0-1").unwrap_err();
        let error_text = error.to_string();
        assert!(error_text.contains("dependency version mismatch"));
        assert!(error_text.contains("dep-app requires >= 1.0-2"));

        validate_incoming_version_against_dependents(&conn, "dep-liba", "1.0-2").unwrap();
    }

    #[test]
    fn incoming_version_cannot_break_installed_dependents() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("conary.db");
        let db_path_str = db_path.to_str().unwrap();
        conary_core::db::init(db_path_str).unwrap();
        let conn = conary_core::db::open(db_path_str).unwrap();

        let mut liba = conary_core::db::models::Trove::new(
            "dep-liba".to_string(),
            "1.0.0".to_string(),
            conary_core::db::models::TroveType::Package,
        );
        liba.insert(&conn).unwrap();

        let mut app = conary_core::db::models::Trove::new(
            "dep-app".to_string(),
            "1.0.0".to_string(),
            conary_core::db::models::TroveType::Package,
        );
        let app_id = app.insert(&conn).unwrap();

        let mut dep = conary_core::db::models::DependencyEntry::new(
            app_id,
            "dep-liba".to_string(),
            None,
            "runtime".to_string(),
            Some(">=1.0, <2.0".to_string()),
        );
        dep.insert(&conn).unwrap();

        let error =
            validate_incoming_version_against_dependents(&conn, "dep-liba", "2.0.0").unwrap_err();
        let error_text = error.to_string();
        assert!(error_text.contains("dependency version mismatch"));
        assert!(error_text.contains("dep-app requires >=1.0, <2.0"));

        validate_incoming_version_against_dependents(&conn, "dep-liba", "1.5.0").unwrap();
    }

    #[test]
    fn package_dependency_rejects_undeclared_capability_guess() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("conary.db");
        let db_path_str = db_path.to_str().unwrap();
        conary_core::db::init(db_path_str).unwrap();
        let conn = conary_core::db::open(db_path_str).unwrap();

        let mut glibc = conary_core::db::models::Trove::new(
            "glibc".to_string(),
            "2.41.0".to_string(),
            conary_core::db::models::TroveType::Package,
        );
        let trove_id = glibc.insert(&conn).unwrap();

        let mut provide = conary_core::db::models::ProvideEntry::new_typed(
            trove_id,
            "soname",
            "libc.so.6(GLIBC_2.41)(64bit)".to_string(),
            None,
        );
        provide.insert(&conn).unwrap();

        let err = validate_package_dependency(&conn, "libc.so.6", None, false).unwrap_err();
        assert!(err.to_string().contains("Missing dependency: libc.so.6"));
    }

    #[test]
    fn package_dependency_accepts_declared_capability_when_no_exact_package_exists() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("conary.db");
        let db_path_str = db_path.to_str().unwrap();
        conary_core::db::init(db_path_str).unwrap();
        let conn = conary_core::db::open(db_path_str).unwrap();

        let mut glibc = conary_core::db::models::Trove::new(
            "glibc".to_string(),
            "2.41.0".to_string(),
            conary_core::db::models::TroveType::Package,
        );
        let trove_id = glibc.insert(&conn).unwrap();

        let mut provide = conary_core::db::models::ProvideEntry::new_typed(
            trove_id,
            "soname",
            "libc.so.6".to_string(),
            None,
        );
        provide.insert(&conn).unwrap();

        validate_package_dependency(&conn, "libc.so.6", None, false).unwrap();
    }

    #[test]
    fn package_dependency_does_not_hide_exact_package_version_mismatch() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("conary.db");
        let db_path_str = db_path.to_str().unwrap();
        conary_core::db::init(db_path_str).unwrap();
        let conn = conary_core::db::open(db_path_str).unwrap();

        let mut package = conary_core::db::models::Trove::new(
            "dep-base".to_string(),
            "1.0.0".to_string(),
            conary_core::db::models::TroveType::Package,
        );
        let trove_id = package.insert(&conn).unwrap();

        let mut provide = conary_core::db::models::ProvideEntry::new_typed(
            trove_id,
            "soname",
            "dep-base.so.1".to_string(),
            None,
        );
        provide.insert(&conn).unwrap();

        let error =
            validate_package_dependency(&conn, "dep-base", Some(">=2.0"), false).unwrap_err();
        assert!(error.to_string().contains("dependency version mismatch"));
    }

    #[tokio::test]
    async fn ccs_install_records_payload_without_direct_live_root_write() {
        use conary_core::ccs::builder::write_ccs_package;
        use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
        use conary_core::hash;

        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        let temp_dir = tempfile::tempdir().unwrap();
        let install_root = temp_dir.path().join("root");
        let package_path = temp_dir.path().join("composefs-only.ccs");
        let db_path = temp_dir.path().join("conary.db");
        let db_path_str = db_path.to_str().unwrap();

        std::fs::create_dir_all(&install_root).unwrap();
        conary_core::db::init(db_path_str).unwrap();
        stage_test_boot_assets(temp_dir.path());

        let content = b"from ccs".to_vec();
        let file_hash = hash::sha256(&content);
        let init_content = b"#!/bin/sh\nexec true\n".to_vec();
        let init_hash = hash::sha256(&init_content);
        let total_size = (content.len() + init_content.len()) as u64;
        let files = vec![
            FileEntry {
                path: "/usr/bin/from-ccs".to_string(),
                hash: file_hash.clone(),
                size: content.len() as u64,
                mode: 0o100755,
                component: "runtime".to_string(),
                file_type: FileType::Regular,
                target: None,
                chunks: None,
            },
            FileEntry {
                path: "/usr/sbin/init".to_string(),
                hash: init_hash.clone(),
                size: init_content.len() as u64,
                mode: 0o100755,
                component: "runtime".to_string(),
                file_type: FileType::Regular,
                target: None,
                chunks: None,
            },
        ];
        let result = BuildResult {
            manifest: CcsManifest::new_minimal("composefs-only", "1.0.0"),
            components: HashMap::from([(
                "runtime".to_string(),
                ComponentData {
                    name: "runtime".to_string(),
                    files: files.clone(),
                    hash: "runtime".to_string(),
                    size: total_size,
                },
            )]),
            files,
            blobs: HashMap::from([
                (file_hash.clone(), content.clone()),
                (init_hash, init_content),
            ]),
            total_size,
            chunked: false,
            chunk_stats: None,
        };
        write_ccs_package(&result, &package_path).unwrap();

        super::cmd_ccs_install(
            package_path.to_str().unwrap(),
            db_path_str,
            install_root.to_str().unwrap(),
            false,
            true,
            None,
            None,
            crate::commands::SandboxMode::None,
            true,
            false,
            false,
            None,
        )
        .await
        .unwrap();

        assert!(
            !install_root.join("usr/bin/from-ccs").exists(),
            "CCS install must not deploy package payloads directly into the live root"
        );

        let conn = conary_core::db::open(db_path_str).unwrap();
        let stored_path: String = conn
            .query_row(
                "SELECT path FROM files WHERE path = '/usr/bin/from-ccs'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored_path, "/usr/bin/from-ccs");

        let current = std::fs::read_link(temp_dir.path().join("current"));
        assert!(
            current.is_ok(),
            "test-mode composefs apply must still publish an active generation pointer"
        );
    }

    #[tokio::test]
    async fn ccs_install_strips_special_permission_bits_from_db_metadata() {
        use conary_core::ccs::builder::write_ccs_package;
        use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
        use conary_core::hash;

        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        let temp_dir = tempfile::tempdir().unwrap();
        let install_root = temp_dir.path().join("root");
        let package_path = temp_dir.path().join("special-mode.ccs");
        let db_path = temp_dir.path().join("conary.db");
        let db_path_str = db_path.to_str().unwrap();

        std::fs::create_dir_all(&install_root).unwrap();
        conary_core::db::init(db_path_str).unwrap();
        stage_test_boot_assets(temp_dir.path());

        let content = b"setid tool".to_vec();
        let file_hash = hash::sha256(&content);
        let init_content = b"#!/bin/sh\nexec true\n".to_vec();
        let init_hash = hash::sha256(&init_content);
        let total_size = (content.len() + init_content.len()) as u64;
        let files = vec![
            FileEntry {
                path: "/usr/bin/setid-tool".to_string(),
                hash: file_hash.clone(),
                size: content.len() as u64,
                mode: 0o106755,
                component: "runtime".to_string(),
                file_type: FileType::Regular,
                target: None,
                chunks: None,
            },
            FileEntry {
                path: "/usr/sbin/init".to_string(),
                hash: init_hash.clone(),
                size: init_content.len() as u64,
                mode: 0o100755,
                component: "runtime".to_string(),
                file_type: FileType::Regular,
                target: None,
                chunks: None,
            },
        ];
        let result = BuildResult {
            manifest: CcsManifest::new_minimal("special-mode", "1.0.0"),
            components: HashMap::from([(
                "runtime".to_string(),
                ComponentData {
                    name: "runtime".to_string(),
                    files: files.clone(),
                    hash: "runtime".to_string(),
                    size: total_size,
                },
            )]),
            files,
            blobs: HashMap::from([(file_hash, content), (init_hash, init_content)]),
            total_size,
            chunked: false,
            chunk_stats: None,
        };
        write_ccs_package(&result, &package_path).unwrap();

        super::cmd_ccs_install(
            package_path.to_str().unwrap(),
            db_path_str,
            install_root.to_str().unwrap(),
            false,
            true,
            None,
            None,
            crate::commands::SandboxMode::None,
            true,
            false,
            false,
            None,
        )
        .await
        .unwrap();

        let conn = conary_core::db::open(db_path_str).unwrap();
        let permissions: i32 = conn
            .query_row(
                "SELECT permissions FROM files WHERE path = '/usr/bin/setid-tool'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(permissions, 0o100755);
        assert_eq!(permissions & 0o6000, 0);
    }

    #[tokio::test]
    async fn ccs_install_persists_capability_declarations() {
        use conary_core::capability::{
            CapabilityDeclaration, FilesystemCapabilities, NetworkCapabilities,
            SyscallCapabilities, load_capabilities_by_name,
        };
        use conary_core::ccs::builder::write_ccs_package;
        use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
        use conary_core::hash;

        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        let temp_dir = tempfile::tempdir().unwrap();
        let install_root = temp_dir.path().join("root");
        let package_path = temp_dir.path().join("declared-capabilities.ccs");
        let db_path = temp_dir.path().join("conary.db");
        let db_path_str = db_path.to_str().unwrap();

        std::fs::create_dir_all(&install_root).unwrap();
        conary_core::db::init(db_path_str).unwrap();
        stage_test_boot_assets(temp_dir.path());

        let content = b"declared capabilities".to_vec();
        let file_hash = hash::sha256(&content);
        let init_content = b"#!/bin/sh\nexec true\n".to_vec();
        let init_hash = hash::sha256(&init_content);
        let total_size = (content.len() + init_content.len()) as u64;
        let files = vec![
            FileEntry {
                path: "/usr/bin/cap-decl".to_string(),
                hash: file_hash.clone(),
                size: content.len() as u64,
                mode: 0o100755,
                component: "runtime".to_string(),
                file_type: FileType::Regular,
                target: None,
                chunks: None,
            },
            FileEntry {
                path: "/usr/sbin/init".to_string(),
                hash: init_hash.clone(),
                size: init_content.len() as u64,
                mode: 0o100755,
                component: "runtime".to_string(),
                file_type: FileType::Regular,
                target: None,
                chunks: None,
            },
        ];
        let mut manifest = CcsManifest::new_minimal("declared-capabilities", "1.0.0");
        manifest.capabilities = Some(CapabilityDeclaration {
            version: 1,
            rationale: Some("needs outbound TLS and read access".to_string()),
            network: NetworkCapabilities {
                outbound: vec!["443".to_string()],
                listen: Vec::new(),
                none: false,
            },
            filesystem: FilesystemCapabilities {
                read: vec!["/etc/ssl/certs".to_string()],
                write: Vec::new(),
                execute: vec!["/usr/bin".to_string()],
                deny: Vec::new(),
            },
            syscalls: SyscallCapabilities::default(),
        });
        let result = BuildResult {
            manifest,
            components: HashMap::from([(
                "runtime".to_string(),
                ComponentData {
                    name: "runtime".to_string(),
                    files: files.clone(),
                    hash: "runtime".to_string(),
                    size: total_size,
                },
            )]),
            files,
            blobs: HashMap::from([(file_hash, content), (init_hash, init_content)]),
            total_size,
            chunked: false,
            chunk_stats: None,
        };
        write_ccs_package(&result, &package_path).unwrap();

        super::cmd_ccs_install(
            package_path.to_str().unwrap(),
            db_path_str,
            install_root.to_str().unwrap(),
            false,
            true,
            None,
            None,
            crate::commands::SandboxMode::None,
            true,
            false,
            false,
            None,
        )
        .await
        .unwrap();

        let conn = conary_core::db::open(db_path_str).unwrap();
        let stored = load_capabilities_by_name(&conn, "declared-capabilities")
            .unwrap()
            .expect("declared CCS capabilities should be stored");
        assert_eq!(
            stored.rationale.as_deref(),
            Some("needs outbound TLS and read access")
        );
        assert_eq!(stored.network.outbound, vec!["443"]);
        assert_eq!(stored.filesystem.read, vec!["/etc/ssl/certs"]);
        assert_eq!(stored.filesystem.execute, vec!["/usr/bin"]);
    }

    #[tokio::test]
    async fn ccs_install_rejects_scriptlet_capabilities_without_enforcement_before_mutation() {
        use conary_core::ccs::builder::write_ccs_package;
        use conary_core::ccs::manifest::ScriptletCapabilityDeclaration;
        use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
        use conary_core::hash;

        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        let temp_dir = tempfile::tempdir().unwrap();
        let install_root = temp_dir.path().join("root");
        let package_path = temp_dir.path().join("scriptlet-capability.ccs");
        let db_path = temp_dir.path().join("conary.db");
        let db_path_str = db_path.to_str().unwrap();

        std::fs::create_dir_all(&install_root).unwrap();
        conary_core::db::init(db_path_str).unwrap();
        stage_test_boot_assets(temp_dir.path());

        let content = b"scriptlet capability".to_vec();
        let file_hash = hash::sha256(&content);
        let (init_file, init_content, init_hash) = ccs_init_file();
        let files = vec![
            FileEntry {
                path: "/usr/bin/scriptlet-capability".to_string(),
                hash: file_hash.clone(),
                size: content.len() as u64,
                mode: 0o100755,
                component: "runtime".to_string(),
                file_type: FileType::Regular,
                target: None,
                chunks: None,
            },
            init_file,
        ];
        let total_size = (content.len() + init_content.len()) as u64;
        let mut manifest = CcsManifest::new_minimal("scriptlet-capability", "1.0.0");
        manifest
            .scriptlets
            .capabilities
            .push(ScriptletCapabilityDeclaration {
                name: "systemd-service-registration".to_string(),
                paths: vec!["/etc/systemd/system".to_string()],
            });
        let result = BuildResult {
            manifest,
            components: HashMap::from([(
                "runtime".to_string(),
                ComponentData {
                    name: "runtime".to_string(),
                    files: files.clone(),
                    hash: "runtime".to_string(),
                    size: total_size,
                },
            )]),
            files,
            blobs: HashMap::from([(file_hash, content), (init_hash, init_content)]),
            total_size,
            chunked: false,
            chunk_stats: None,
        };
        write_ccs_package(&result, &package_path).unwrap();

        let err = super::cmd_ccs_install(
            package_path.to_str().unwrap(),
            db_path_str,
            install_root.to_str().unwrap(),
            false,
            true,
            None,
            None,
            crate::commands::SandboxMode::Always,
            true,
            false,
            false,
            None,
        )
        .await
        .unwrap_err();

        let message = err.to_string();
        assert!(
            message.contains(
                "scriptlet capability declarations are present but enforcement is not available"
            ),
            "unexpected error: {message}"
        );
        let conn = conary_core::db::open(db_path_str).unwrap();
        assert!(
            conary_core::db::models::Trove::find_by_name(&conn, "scriptlet-capability")
                .unwrap()
                .is_empty(),
            "scriptlet capability gate must fail before DB mutation"
        );
    }

    #[tokio::test]
    async fn ccs_install_respects_manifest_component_selection() {
        use conary_core::ccs::builder::write_ccs_package;
        use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
        use conary_core::hash;

        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        let temp_dir = tempfile::tempdir().unwrap();
        let install_root = temp_dir.path().join("root");
        let package_path = temp_dir.path().join("custom-components.ccs");
        let db_path = temp_dir.path().join("conary.db");
        let db_path_str = db_path.to_str().unwrap();

        std::fs::create_dir_all(&install_root).unwrap();
        conary_core::db::init(db_path_str).unwrap();
        stage_test_boot_assets(temp_dir.path());

        let chosen_content = b"chosen component".to_vec();
        let chosen_hash = hash::sha256(&chosen_content);
        let skipped_content = b"skipped component".to_vec();
        let skipped_hash = hash::sha256(&skipped_content);
        let init_content = b"#!/bin/sh\nexec true\n".to_vec();
        let init_hash = hash::sha256(&init_content);
        let chosen_files = vec![
            FileEntry {
                path: "/usr/bin/chosen-custom".to_string(),
                hash: chosen_hash.clone(),
                size: chosen_content.len() as u64,
                mode: 0o100755,
                component: "chosen".to_string(),
                file_type: FileType::Regular,
                target: None,
                chunks: None,
            },
            FileEntry {
                path: "/usr/sbin/init".to_string(),
                hash: init_hash.clone(),
                size: init_content.len() as u64,
                mode: 0o100755,
                component: "chosen".to_string(),
                file_type: FileType::Regular,
                target: None,
                chunks: None,
            },
        ];
        let skipped_files = vec![FileEntry {
            path: "/usr/bin/skipped-custom".to_string(),
            hash: skipped_hash.clone(),
            size: skipped_content.len() as u64,
            mode: 0o100755,
            component: "skipped".to_string(),
            file_type: FileType::Regular,
            target: None,
            chunks: None,
        }];
        let mut files = chosen_files.clone();
        files.extend(skipped_files.clone());
        let result = BuildResult {
            manifest: CcsManifest::new_minimal("custom-components", "1.0.0"),
            components: HashMap::from([
                (
                    "chosen".to_string(),
                    ComponentData {
                        name: "chosen".to_string(),
                        files: chosen_files,
                        hash: "chosen".to_string(),
                        size: (chosen_content.len() + init_content.len()) as u64,
                    },
                ),
                (
                    "skipped".to_string(),
                    ComponentData {
                        name: "skipped".to_string(),
                        files: skipped_files,
                        hash: "skipped".to_string(),
                        size: skipped_content.len() as u64,
                    },
                ),
            ]),
            files,
            blobs: HashMap::from([
                (chosen_hash, chosen_content),
                (skipped_hash, skipped_content),
                (init_hash, init_content),
            ]),
            total_size: 0,
            chunked: false,
            chunk_stats: None,
        };
        write_ccs_package(&result, &package_path).unwrap();

        super::cmd_ccs_install(
            package_path.to_str().unwrap(),
            db_path_str,
            install_root.to_str().unwrap(),
            false,
            true,
            None,
            Some(vec!["chosen".to_string()]),
            crate::commands::SandboxMode::None,
            true,
            false,
            false,
            None,
        )
        .await
        .unwrap();

        assert!(!install_root.join("usr/bin/chosen-custom").exists());
        assert!(!install_root.join("usr/bin/skipped-custom").exists());

        let conn = conary_core::db::open(db_path_str).unwrap();
        let chosen_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM files WHERE path = '/usr/bin/chosen-custom'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let skipped_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM files WHERE path = '/usr/bin/skipped-custom'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(chosen_count, 1);
        assert_eq!(
            skipped_count, 0,
            "CCS install must honor selected manifest components before path classification"
        );
        let chosen_component_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM components WHERE name = 'chosen'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let skipped_component_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM components WHERE name = 'skipped'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(chosen_component_count, 1);
        assert_eq!(skipped_component_count, 0);
    }

    #[tokio::test]
    async fn ccs_install_persists_pre_remove_hook() {
        use conary_core::ccs::builder::write_ccs_package;
        use conary_core::ccs::manifest::ScriptHook;
        use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
        use conary_core::hash;

        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        let temp_dir = tempfile::tempdir().unwrap();
        let install_root = temp_dir.path().join("root");
        let package_path = temp_dir.path().join("pre-remove.ccs");
        let db_path = temp_dir.path().join("conary.db");
        let db_path_str = db_path.to_str().unwrap();

        std::fs::create_dir_all(&install_root).unwrap();
        conary_core::db::init(db_path_str).unwrap();
        stage_test_boot_assets(temp_dir.path());

        let content = b"hooked payload".to_vec();
        let file_hash = hash::sha256(&content);
        let init_content = b"#!/bin/sh\nexec true\n".to_vec();
        let init_hash = hash::sha256(&init_content);
        let files = vec![
            FileEntry {
                path: "/usr/bin/hooked".to_string(),
                hash: file_hash.clone(),
                size: content.len() as u64,
                mode: 0o100755,
                component: "runtime".to_string(),
                file_type: FileType::Regular,
                target: None,
                chunks: None,
            },
            FileEntry {
                path: "/usr/sbin/init".to_string(),
                hash: init_hash.clone(),
                size: init_content.len() as u64,
                mode: 0o100755,
                component: "runtime".to_string(),
                file_type: FileType::Regular,
                target: None,
                chunks: None,
            },
        ];
        let mut manifest = CcsManifest::new_minimal("pre-remove", "1.0.0");
        manifest.hooks.pre_remove = Some(ScriptHook {
            script: "echo removing pre-remove".to_string(),
        });
        let result = BuildResult {
            manifest,
            components: HashMap::from([(
                "runtime".to_string(),
                ComponentData {
                    name: "runtime".to_string(),
                    files: files.clone(),
                    hash: "runtime".to_string(),
                    size: (content.len() + init_content.len()) as u64,
                },
            )]),
            files,
            blobs: HashMap::from([(file_hash, content), (init_hash, init_content)]),
            total_size: 0,
            chunked: false,
            chunk_stats: None,
        };
        write_ccs_package(&result, &package_path).unwrap();

        super::cmd_ccs_install(
            package_path.to_str().unwrap(),
            db_path_str,
            install_root.to_str().unwrap(),
            false,
            true,
            None,
            None,
            crate::commands::SandboxMode::None,
            true,
            false,
            false,
            None,
        )
        .await
        .unwrap();

        let conn = conary_core::db::open(db_path_str).unwrap();
        let (phase, content, package_format): (String, String, String) = conn
            .query_row(
                "SELECT phase, content, package_format FROM scriptlets LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(phase, "pre-remove");
        assert_eq!(content, "echo removing pre-remove");
        assert_eq!(package_format, "ccs");
    }

    #[tokio::test]
    async fn ccs_install_persists_manifest_provides() {
        use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
        use conary_core::hash;
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use tar::Builder;

        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        let temp_dir = tempfile::tempdir().unwrap();
        let install_root = temp_dir.path().join("root");
        let package_path = temp_dir.path().join("manifest-provides.ccs");
        let db_path = temp_dir.path().join("conary.db");
        let db_path_str = db_path.to_str().unwrap();

        std::fs::create_dir_all(&install_root).unwrap();
        conary_core::db::init(db_path_str).unwrap();
        stage_test_boot_assets(temp_dir.path());

        let init_content = b"#!/bin/sh\nexec true\n".to_vec();
        let init_hash = hash::sha256(&init_content);
        let files = vec![FileEntry {
            path: "/usr/sbin/init".to_string(),
            hash: init_hash.clone(),
            size: init_content.len() as u64,
            mode: 0o100755,
            component: "runtime".to_string(),
            file_type: FileType::Regular,
            target: None,
            chunks: None,
        }];

        let mut manifest = CcsManifest::new_minimal("manifest-provides", "1.0.0");
        manifest.provides.capabilities = vec!["virtual-web-server".to_string()];
        manifest.provides.sonames = vec!["libmanifest.so.1".to_string()];
        manifest.provides.binaries = vec!["manifestctl".to_string()];
        manifest.provides.pkgconfig = vec!["manifest".to_string()];

        let result = BuildResult {
            manifest,
            components: HashMap::from([(
                "runtime".to_string(),
                ComponentData {
                    name: "runtime".to_string(),
                    files: files.clone(),
                    hash: "runtime".to_string(),
                    size: init_content.len() as u64,
                },
            )]),
            files,
            blobs: HashMap::from([(init_hash.clone(), init_content.clone())]),
            total_size: 0,
            chunked: false,
            chunk_stats: None,
        };

        let package_root = temp_dir.path().join("package-root");
        let components_dir = package_root.join("components");
        let object_path = package_root
            .join("objects")
            .join(&init_hash[..2])
            .join(&init_hash[2..]);
        std::fs::create_dir_all(&components_dir).unwrap();
        std::fs::create_dir_all(object_path.parent().unwrap()).unwrap();
        std::fs::write(
            package_root.join("MANIFEST.toml"),
            result.manifest.to_toml().unwrap(),
        )
        .unwrap();
        std::fs::write(
            components_dir.join("runtime.json"),
            serde_json::to_string_pretty(result.components.get("runtime").unwrap()).unwrap(),
        )
        .unwrap();
        std::fs::write(object_path, &init_content).unwrap();

        let output = std::fs::File::create(&package_path).unwrap();
        let encoder = GzEncoder::new(output, Compression::default());
        let mut archive = Builder::new(encoder);
        archive.append_dir_all(".", &package_root).unwrap();
        let encoder = archive.into_inner().unwrap();
        encoder.finish().unwrap();

        super::cmd_ccs_install(
            package_path.to_str().unwrap(),
            db_path_str,
            install_root.to_str().unwrap(),
            false,
            true,
            None,
            None,
            crate::commands::SandboxMode::None,
            true,
            false,
            false,
            None,
        )
        .await
        .unwrap();

        let conn = conary_core::db::open(db_path_str).unwrap();
        let rows: Vec<(String, String)> = {
            let mut stmt = conn
                .prepare("SELECT kind, capability FROM provides ORDER BY kind, capability")
                .unwrap();
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };

        assert!(
            rows.contains(&("package".to_string(), "virtual-web-server".to_string())),
            "manifest capability provides must be persisted"
        );
        assert!(
            rows.contains(&("soname".to_string(), "libmanifest.so.1".to_string())),
            "manifest soname provides must be persisted"
        );
        assert!(
            rows.contains(&("binary".to_string(), "manifestctl".to_string())),
            "manifest binary provides must be persisted"
        );
        assert!(
            rows.contains(&("pkgconfig".to_string(), "manifest".to_string())),
            "manifest pkgconfig provides must be persisted"
        );
    }

    #[tokio::test]
    async fn ccs_install_persists_typed_provide_when_name_collides() {
        use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
        use conary_core::hash;
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use tar::Builder;

        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        let temp_dir = tempfile::tempdir().unwrap();
        let install_root = temp_dir.path().join("root");
        let package_path = temp_dir.path().join("collision-tool.ccs");
        let db_path = temp_dir.path().join("conary.db");
        let db_path_str = db_path.to_str().unwrap();

        std::fs::create_dir_all(&install_root).unwrap();
        conary_core::db::init(db_path_str).unwrap();
        stage_test_boot_assets(temp_dir.path());

        let init_content = b"#!/bin/sh\nexec true\n".to_vec();
        let init_hash = hash::sha256(&init_content);
        let files = vec![FileEntry {
            path: "/usr/sbin/init".to_string(),
            hash: init_hash.clone(),
            size: init_content.len() as u64,
            mode: 0o100755,
            component: "runtime".to_string(),
            file_type: FileType::Regular,
            target: None,
            chunks: None,
        }];

        let mut manifest = CcsManifest::new_minimal("collision-tool", "1.0.0");
        manifest.provides.binaries = vec!["collision-tool".to_string()];

        let result = BuildResult {
            manifest,
            components: HashMap::from([(
                "runtime".to_string(),
                ComponentData {
                    name: "runtime".to_string(),
                    files: files.clone(),
                    hash: "runtime".to_string(),
                    size: init_content.len() as u64,
                },
            )]),
            files,
            blobs: HashMap::from([(init_hash.clone(), init_content.clone())]),
            total_size: 0,
            chunked: false,
            chunk_stats: None,
        };

        let package_root = temp_dir.path().join("package-root");
        let components_dir = package_root.join("components");
        let object_path = package_root
            .join("objects")
            .join(&init_hash[..2])
            .join(&init_hash[2..]);
        std::fs::create_dir_all(&components_dir).unwrap();
        std::fs::create_dir_all(object_path.parent().unwrap()).unwrap();
        std::fs::write(
            package_root.join("MANIFEST.toml"),
            result.manifest.to_toml().unwrap(),
        )
        .unwrap();
        std::fs::write(
            components_dir.join("runtime.json"),
            serde_json::to_string_pretty(result.components.get("runtime").unwrap()).unwrap(),
        )
        .unwrap();
        std::fs::write(object_path, &init_content).unwrap();

        let output = std::fs::File::create(&package_path).unwrap();
        let encoder = GzEncoder::new(output, Compression::default());
        let mut archive = Builder::new(encoder);
        archive.append_dir_all(".", &package_root).unwrap();
        let encoder = archive.into_inner().unwrap();
        encoder.finish().unwrap();

        super::cmd_ccs_install(
            package_path.to_str().unwrap(),
            db_path_str,
            install_root.to_str().unwrap(),
            false,
            true,
            None,
            None,
            crate::commands::SandboxMode::None,
            true,
            false,
            false,
            None,
        )
        .await
        .unwrap();

        let conn = conary_core::db::open(db_path_str).unwrap();
        let typed =
            conary_core::db::models::ProvideEntry::find_typed(&conn, "binary", "collision-tool")
                .unwrap();
        assert!(
            typed.is_some(),
            "typed manifest provide must remain resolvable when its raw capability equals the package name"
        );
    }

    #[tokio::test]
    async fn ccs_install_reinstall_dry_run_does_not_mutate_db() {
        use conary_core::ccs::builder::write_ccs_package;
        use conary_core::ccs::{BuildResult, CcsManifest};

        let temp_dir = tempfile::tempdir().unwrap();
        let install_root = temp_dir.path().join("root");
        let package_path = temp_dir.path().join("reinstall-dry-run.ccs");
        let db_path = temp_dir.path().join("conary.db");
        let db_path_str = db_path.to_str().unwrap();

        std::fs::create_dir_all(&install_root).unwrap();
        conary_core::db::init(db_path_str).unwrap();
        let conn = conary_core::db::open(db_path_str).unwrap();
        let mut existing = conary_core::db::models::Trove::new(
            "reinstall-dry-run".to_string(),
            "1.0.0".to_string(),
            conary_core::db::models::TroveType::Package,
        );
        let existing_id = existing.insert(&conn).unwrap();
        drop(conn);

        let result = BuildResult {
            manifest: CcsManifest::new_minimal("reinstall-dry-run", "1.0.0"),
            components: HashMap::new(),
            files: Vec::new(),
            blobs: HashMap::new(),
            total_size: 0,
            chunked: false,
            chunk_stats: None,
        };
        write_ccs_package(&result, &package_path).unwrap();

        super::cmd_ccs_install(
            package_path.to_str().unwrap(),
            db_path_str,
            install_root.to_str().unwrap(),
            true,
            true,
            None,
            None,
            crate::commands::SandboxMode::None,
            true,
            true,
            false,
            None,
        )
        .await
        .unwrap();

        let conn = conary_core::db::open(db_path_str).unwrap();
        let (trove_count, retained_id): (i64, i64) = conn
            .query_row(
                "SELECT COUNT(*), COALESCE(MAX(id), -1) FROM troves WHERE name = 'reinstall-dry-run'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(trove_count, 1);
        assert_eq!(
            retained_id, existing_id,
            "dry-run reinstall must not delete the existing installed trove"
        );
    }

    #[tokio::test]
    async fn ccs_install_rejects_child_write_beneath_package_symlink() {
        use conary_core::ccs::builder::write_ccs_package;
        use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
        use conary_core::filesystem::CasStore;
        use conary_core::hash;

        let temp_dir = tempfile::tempdir().unwrap();
        let install_root = temp_dir.path().join("root");
        let outside_root = temp_dir.path().join("outside");
        let package_path = temp_dir.path().join("symlink-escape.ccs");
        let db_path = temp_dir.path().join("conary.db");
        let db_path_str = db_path.to_str().unwrap();

        std::fs::create_dir_all(&install_root).unwrap();
        std::fs::create_dir_all(&outside_root).unwrap();
        conary_core::db::init(db_path_str).unwrap();

        let symlink_target = outside_root.to_string_lossy().to_string();
        let symlink_hash = CasStore::compute_symlink_hash(&symlink_target);
        let child_path = "/usr/lib/link/cron.d/persist".to_string();
        let child_content = b"persist".to_vec();
        let child_hash = hash::sha256(&child_content);

        let files = vec![
            FileEntry {
                path: "/usr/lib/link".to_string(),
                hash: symlink_hash.clone(),
                size: symlink_target.len() as u64,
                mode: 0o120777,
                component: "runtime".to_string(),
                file_type: FileType::Symlink,
                target: Some(symlink_target.clone()),
                chunks: None,
            },
            FileEntry {
                path: child_path.clone(),
                hash: child_hash.clone(),
                size: child_content.len() as u64,
                mode: 0o100644,
                component: "runtime".to_string(),
                file_type: FileType::Regular,
                target: None,
                chunks: None,
            },
        ];

        let result = BuildResult {
            manifest: CcsManifest::new_minimal("symlink-escape", "1.0.0"),
            components: HashMap::from([(
                "runtime".to_string(),
                ComponentData {
                    name: "runtime".to_string(),
                    files: files.clone(),
                    hash: "test-runtime".to_string(),
                    size: (symlink_target.len() + child_content.len()) as u64,
                },
            )]),
            files,
            blobs: HashMap::from([
                (symlink_hash, symlink_target.as_bytes().to_vec()),
                (child_hash, child_content.clone()),
            ]),
            total_size: (symlink_target.len() + child_content.len()) as u64,
            chunked: false,
            chunk_stats: None,
        };
        write_ccs_package(&result, &package_path).unwrap();

        let err = super::cmd_ccs_install(
            package_path.to_str().unwrap(),
            db_path_str,
            install_root.to_str().unwrap(),
            false,
            true,
            None,
            None,
            crate::commands::SandboxMode::None,
            true,
            false,
            false,
            None,
        )
        .await
        .unwrap_err();

        assert!(
            err.to_string().contains("path traversal") || err.to_string().contains("symlink"),
            "unexpected error: {err:#}"
        );
        assert!(!outside_root.join("cron.d/persist").exists());
    }

    #[tokio::test]
    async fn ccs_install_rejects_child_before_package_symlink() {
        use conary_core::ccs::builder::write_ccs_package;
        use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
        use conary_core::filesystem::CasStore;
        use conary_core::hash;

        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        let temp_dir = tempfile::tempdir().unwrap();
        let install_root = temp_dir.path().join("root");
        let outside_root = temp_dir.path().join("outside");
        let package_path = temp_dir.path().join("reversed-symlink-escape.ccs");
        let db_path = temp_dir.path().join("conary.db");
        let db_path_str = db_path.to_str().unwrap();

        std::fs::create_dir_all(&install_root).unwrap();
        std::fs::create_dir_all(&outside_root).unwrap();
        conary_core::db::init(db_path_str).unwrap();
        stage_test_boot_assets(temp_dir.path());

        let symlink_target = outside_root.to_string_lossy().to_string();
        let symlink_hash = CasStore::compute_symlink_hash(&symlink_target);
        let child_path = "/usr/lib/link/cron.d/persist".to_string();
        let child_content = b"persist".to_vec();
        let child_hash = hash::sha256(&child_content);
        let init_content = b"#!/bin/sh\nexec true\n".to_vec();
        let init_hash = hash::sha256(&init_content);

        let files = vec![
            FileEntry {
                path: child_path.clone(),
                hash: child_hash.clone(),
                size: child_content.len() as u64,
                mode: 0o100644,
                component: "runtime".to_string(),
                file_type: FileType::Regular,
                target: None,
                chunks: None,
            },
            FileEntry {
                path: "/usr/lib/link".to_string(),
                hash: symlink_hash.clone(),
                size: symlink_target.len() as u64,
                mode: 0o120777,
                component: "runtime".to_string(),
                file_type: FileType::Symlink,
                target: Some(symlink_target.clone()),
                chunks: None,
            },
            FileEntry {
                path: "/usr/sbin/init".to_string(),
                hash: init_hash.clone(),
                size: init_content.len() as u64,
                mode: 0o100755,
                component: "runtime".to_string(),
                file_type: FileType::Regular,
                target: None,
                chunks: None,
            },
        ];

        let result = BuildResult {
            manifest: CcsManifest::new_minimal("reversed-symlink-escape", "1.0.0"),
            components: HashMap::from([(
                "runtime".to_string(),
                ComponentData {
                    name: "runtime".to_string(),
                    files: files.clone(),
                    hash: "test-runtime".to_string(),
                    size: (symlink_target.len() + child_content.len() + init_content.len()) as u64,
                },
            )]),
            files,
            blobs: HashMap::from([
                (child_hash, child_content.clone()),
                (symlink_hash, symlink_target.as_bytes().to_vec()),
                (init_hash, init_content),
            ]),
            total_size: (symlink_target.len() + child_content.len()) as u64,
            chunked: false,
            chunk_stats: None,
        };
        write_ccs_package(&result, &package_path).unwrap();

        let err = super::cmd_ccs_install(
            package_path.to_str().unwrap(),
            db_path_str,
            install_root.to_str().unwrap(),
            false,
            true,
            None,
            None,
            crate::commands::SandboxMode::None,
            true,
            false,
            false,
            None,
        )
        .await
        .unwrap_err();

        assert!(
            err.to_string().contains("path traversal") || err.to_string().contains("symlink"),
            "unexpected error: {err:#}"
        );
        let conn = conary_core::db::open(db_path_str).unwrap();
        let persisted: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM files WHERE path = ?1",
                [&child_path],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(persisted, 0);
    }

    #[tokio::test]
    async fn ccs_install_persists_usrmerge_payload_under_usr_path() {
        use conary_core::ccs::builder::write_ccs_package;
        use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
        use conary_core::hash;

        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        let temp_dir = tempfile::tempdir().unwrap();
        let install_root = temp_dir.path().join("root");
        let package_path = temp_dir.path().join("usrmerge.ccs");
        let db_path = temp_dir.path().join("conary.db");
        let db_path_str = db_path.to_str().unwrap();

        std::fs::create_dir_all(&install_root).unwrap();
        conary_core::db::init(db_path_str).unwrap();
        stage_test_boot_assets(temp_dir.path());

        let content = b"chkconfig".to_vec();
        let file_hash = hash::sha256(&content);
        let init_content = b"#!/bin/sh\nexec true\n".to_vec();
        let init_hash = hash::sha256(&init_content);
        let total_size = (content.len() + init_content.len()) as u64;
        let files = vec![
            FileEntry {
                path: "bin/chkconfig".to_string(),
                hash: file_hash.clone(),
                size: content.len() as u64,
                mode: 0o100755,
                component: "runtime".to_string(),
                file_type: FileType::Regular,
                target: None,
                chunks: None,
            },
            FileEntry {
                path: "/usr/sbin/init".to_string(),
                hash: init_hash.clone(),
                size: init_content.len() as u64,
                mode: 0o100755,
                component: "runtime".to_string(),
                file_type: FileType::Regular,
                target: None,
                chunks: None,
            },
        ];

        let result = BuildResult {
            manifest: CcsManifest::new_minimal("usrmerge", "1.0.0"),
            components: HashMap::from([(
                "runtime".to_string(),
                ComponentData {
                    name: "runtime".to_string(),
                    files: files.clone(),
                    hash: "runtime".to_string(),
                    size: total_size,
                },
            )]),
            files,
            blobs: HashMap::from([(file_hash, content.clone()), (init_hash, init_content)]),
            total_size,
            chunked: false,
            chunk_stats: None,
        };
        write_ccs_package(&result, &package_path).unwrap();

        super::cmd_ccs_install(
            package_path.to_str().unwrap(),
            db_path_str,
            install_root.to_str().unwrap(),
            false,
            true,
            None,
            None,
            crate::commands::SandboxMode::None,
            true,
            false,
            false,
            None,
        )
        .await
        .unwrap();

        assert!(
            !install_root.join("usr/bin/chkconfig").exists(),
            "usr-merge package payload must be recorded for generation build, not written live"
        );
        let conn = conary_core::db::open(db_path_str).unwrap();
        let stored_path: String = conn
            .query_row(
                "SELECT path FROM files WHERE path = '/usr/bin/chkconfig'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored_path, "/usr/bin/chkconfig");
        let legacy_path_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM files WHERE path = 'bin/chkconfig'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(legacy_path_count, 0);
        assert!(std::fs::read_link(temp_dir.path().join("current")).is_ok());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ccs_install_allows_identical_existing_symlink_destination() {
        use conary_core::ccs::builder::write_ccs_package;
        use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
        use conary_core::filesystem::CasStore;
        use std::path::PathBuf;

        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        let temp_dir = tempfile::tempdir().unwrap();
        let install_root = temp_dir.path().join("root");
        let package_path = temp_dir.path().join("bash-link.ccs");
        let db_path = temp_dir.path().join("conary.db");
        let db_path_str = db_path.to_str().unwrap();

        std::fs::create_dir_all(install_root.join("usr/bin")).unwrap();
        std::os::unix::fs::symlink("bash", install_root.join("usr/bin/sh")).unwrap();
        conary_core::db::init(db_path_str).unwrap();
        stage_test_boot_assets(temp_dir.path());

        let target = "bash".to_string();
        let symlink_hash = CasStore::compute_symlink_hash(&target);
        let init_content = b"#!/bin/sh\nexec true\n".to_vec();
        let init_hash = conary_core::hash::sha256(&init_content);
        let files = vec![
            FileEntry {
                path: "/usr/bin/sh".to_string(),
                hash: symlink_hash.clone(),
                size: target.len() as u64,
                mode: 0o120777,
                component: "runtime".to_string(),
                file_type: FileType::Symlink,
                target: Some(target.clone()),
                chunks: None,
            },
            FileEntry {
                path: "/usr/sbin/init".to_string(),
                hash: init_hash.clone(),
                size: init_content.len() as u64,
                mode: 0o100755,
                component: "runtime".to_string(),
                file_type: FileType::Regular,
                target: None,
                chunks: None,
            },
        ];
        let result = BuildResult {
            manifest: CcsManifest::new_minimal("bash-link", "1.0.0"),
            components: HashMap::from([(
                "runtime".to_string(),
                ComponentData {
                    name: "runtime".to_string(),
                    files: files.clone(),
                    hash: "runtime".to_string(),
                    size: (target.len() + init_content.len()) as u64,
                },
            )]),
            files,
            blobs: HashMap::from([
                (symlink_hash, target.as_bytes().to_vec()),
                (init_hash, init_content),
            ]),
            total_size: 0,
            chunked: false,
            chunk_stats: None,
        };
        write_ccs_package(&result, &package_path).unwrap();

        super::cmd_ccs_install(
            package_path.to_str().unwrap(),
            db_path_str,
            install_root.to_str().unwrap(),
            false,
            true,
            None,
            None,
            crate::commands::SandboxMode::None,
            true,
            false,
            false,
            None,
        )
        .await
        .unwrap();

        assert_eq!(
            std::fs::read_link(install_root.join("usr/bin/sh")).unwrap(),
            PathBuf::from("bash")
        );
        let conn = conary_core::db::open(db_path_str).unwrap();
        let symlink_target: String = conn
            .query_row(
                "SELECT symlink_target FROM files WHERE path = '/usr/bin/sh'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(symlink_target, "bash");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ccs_install_replaces_existing_leaf_symlink_destination() {
        use conary_core::ccs::builder::write_ccs_package;
        use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
        use conary_core::filesystem::CasStore;
        use std::path::PathBuf;

        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        let temp_dir = tempfile::tempdir().unwrap();
        let install_root = temp_dir.path().join("root");
        let package_path = temp_dir.path().join("library-link.ccs");
        let db_path = temp_dir.path().join("conary.db");
        let db_path_str = db_path.to_str().unwrap();

        std::fs::create_dir_all(install_root.join("usr/lib64")).unwrap();
        std::os::unix::fs::symlink(
            "libtasn1.so.6.6.4",
            install_root.join("usr/lib64/libtasn1.so.6"),
        )
        .unwrap();
        conary_core::db::init(db_path_str).unwrap();
        stage_test_boot_assets(temp_dir.path());

        let target = "libtasn1.so.6.6.5".to_string();
        let symlink_hash = CasStore::compute_symlink_hash(&target);
        let init_content = b"#!/bin/sh\nexec true\n".to_vec();
        let init_hash = conary_core::hash::sha256(&init_content);
        let files = vec![
            FileEntry {
                path: "/usr/lib64/libtasn1.so.6".to_string(),
                hash: symlink_hash.clone(),
                size: target.len() as u64,
                mode: 0o120777,
                component: "runtime".to_string(),
                file_type: FileType::Symlink,
                target: Some(target.clone()),
                chunks: None,
            },
            FileEntry {
                path: "/usr/sbin/init".to_string(),
                hash: init_hash.clone(),
                size: init_content.len() as u64,
                mode: 0o100755,
                component: "runtime".to_string(),
                file_type: FileType::Regular,
                target: None,
                chunks: None,
            },
        ];
        let result = BuildResult {
            manifest: CcsManifest::new_minimal("library-link", "1.0.0"),
            components: HashMap::from([(
                "runtime".to_string(),
                ComponentData {
                    name: "runtime".to_string(),
                    files: files.clone(),
                    hash: "runtime".to_string(),
                    size: (target.len() + init_content.len()) as u64,
                },
            )]),
            files,
            blobs: HashMap::from([
                (symlink_hash, target.as_bytes().to_vec()),
                (init_hash, init_content),
            ]),
            total_size: 0,
            chunked: false,
            chunk_stats: None,
        };
        write_ccs_package(&result, &package_path).unwrap();

        super::cmd_ccs_install(
            package_path.to_str().unwrap(),
            db_path_str,
            install_root.to_str().unwrap(),
            false,
            true,
            None,
            None,
            crate::commands::SandboxMode::None,
            true,
            false,
            false,
            None,
        )
        .await
        .unwrap();

        assert_eq!(
            std::fs::read_link(install_root.join("usr/lib64/libtasn1.so.6")).unwrap(),
            PathBuf::from("libtasn1.so.6.6.4")
        );
        let conn = conary_core::db::open(db_path_str).unwrap();
        let symlink_target: String = conn
            .query_row(
                "SELECT symlink_target FROM files WHERE path = '/usr/lib64/libtasn1.so.6'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(symlink_target, "libtasn1.so.6.6.5");
    }

    #[tokio::test]
    async fn ccs_install_coalesces_identical_usrmerge_duplicate_files() {
        use conary_core::ccs::builder::write_ccs_package;
        use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
        use conary_core::hash;

        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        let temp_dir = tempfile::tempdir().unwrap();
        let install_root = temp_dir.path().join("root");
        let package_path = temp_dir.path().join("usrmerge-duplicate.ccs");
        let db_path = temp_dir.path().join("conary.db");
        let db_path_str = db_path.to_str().unwrap();

        std::fs::create_dir_all(install_root.join("usr/bin")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("usr/bin", install_root.join("bin")).unwrap();
        conary_core::db::init(db_path_str).unwrap();
        stage_test_boot_assets(temp_dir.path());

        let content = b"chkconfig".to_vec();
        let file_hash = hash::sha256(&content);
        let init_content = b"#!/bin/sh\nexec true\n".to_vec();
        let init_hash = hash::sha256(&init_content);
        let files = vec![
            FileEntry {
                path: "bin/chkconfig".to_string(),
                hash: file_hash.clone(),
                size: content.len() as u64,
                mode: 0o100755,
                component: "runtime".to_string(),
                file_type: FileType::Regular,
                target: None,
                chunks: None,
            },
            FileEntry {
                path: "usr/bin/chkconfig".to_string(),
                hash: file_hash.clone(),
                size: content.len() as u64,
                mode: 0o100755,
                component: "runtime".to_string(),
                file_type: FileType::Regular,
                target: None,
                chunks: None,
            },
            FileEntry {
                path: "/usr/sbin/init".to_string(),
                hash: init_hash.clone(),
                size: init_content.len() as u64,
                mode: 0o100755,
                component: "runtime".to_string(),
                file_type: FileType::Regular,
                target: None,
                chunks: None,
            },
        ];

        let result = BuildResult {
            manifest: CcsManifest::new_minimal("usrmerge-duplicate", "1.0.0"),
            components: HashMap::from([(
                "runtime".to_string(),
                ComponentData {
                    name: "runtime".to_string(),
                    files: files.clone(),
                    hash: "runtime".to_string(),
                    size: content.len() as u64 * 2 + init_content.len() as u64,
                },
            )]),
            files,
            blobs: HashMap::from([(file_hash, content.clone()), (init_hash, init_content)]),
            total_size: content.len() as u64 * 2,
            chunked: false,
            chunk_stats: None,
        };
        write_ccs_package(&result, &package_path).unwrap();

        super::cmd_ccs_install(
            package_path.to_str().unwrap(),
            db_path_str,
            install_root.to_str().unwrap(),
            false,
            true,
            None,
            None,
            crate::commands::SandboxMode::None,
            true,
            false,
            false,
            None,
        )
        .await
        .unwrap();

        let conn = conary_core::db::open(db_path_str).unwrap();
        let chkconfig_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM files WHERE path = '/usr/bin/chkconfig'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(chkconfig_count, 1);
        let legacy_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM files WHERE path IN ('bin/chkconfig', 'usr/bin/chkconfig')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(legacy_count, 0);
    }

    #[tokio::test]
    async fn ccs_install_rejects_conflicting_usrmerge_duplicate_files() {
        use conary_core::ccs::builder::write_ccs_package;
        use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
        use conary_core::hash;

        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        let temp_dir = tempfile::tempdir().unwrap();
        let install_root = temp_dir.path().join("root");
        let package_path = temp_dir.path().join("usrmerge-conflict.ccs");
        let db_path = temp_dir.path().join("conary.db");
        let db_path_str = db_path.to_str().unwrap();

        std::fs::create_dir_all(&install_root).unwrap();
        conary_core::db::init(db_path_str).unwrap();
        stage_test_boot_assets(temp_dir.path());

        let bin_content = b"from-bin".to_vec();
        let bin_hash = hash::sha256(&bin_content);
        let usr_content = b"from-usr".to_vec();
        let usr_hash = hash::sha256(&usr_content);
        let init_content = b"#!/bin/sh\nexec true\n".to_vec();
        let init_hash = hash::sha256(&init_content);
        let files = vec![
            FileEntry {
                path: "bin/chkconfig".to_string(),
                hash: bin_hash.clone(),
                size: bin_content.len() as u64,
                mode: 0o100755,
                component: "runtime".to_string(),
                file_type: FileType::Regular,
                target: None,
                chunks: None,
            },
            FileEntry {
                path: "usr/bin/chkconfig".to_string(),
                hash: usr_hash.clone(),
                size: usr_content.len() as u64,
                mode: 0o100755,
                component: "runtime".to_string(),
                file_type: FileType::Regular,
                target: None,
                chunks: None,
            },
            FileEntry {
                path: "/usr/sbin/init".to_string(),
                hash: init_hash.clone(),
                size: init_content.len() as u64,
                mode: 0o100755,
                component: "runtime".to_string(),
                file_type: FileType::Regular,
                target: None,
                chunks: None,
            },
        ];

        let result = BuildResult {
            manifest: CcsManifest::new_minimal("usrmerge-conflict", "1.0.0"),
            components: HashMap::from([(
                "runtime".to_string(),
                ComponentData {
                    name: "runtime".to_string(),
                    files: files.clone(),
                    hash: "runtime".to_string(),
                    size: (bin_content.len() + usr_content.len() + init_content.len()) as u64,
                },
            )]),
            files,
            blobs: HashMap::from([
                (bin_hash, bin_content),
                (usr_hash, usr_content),
                (init_hash, init_content),
            ]),
            total_size: 0,
            chunked: false,
            chunk_stats: None,
        };
        write_ccs_package(&result, &package_path).unwrap();

        let err = super::cmd_ccs_install(
            package_path.to_str().unwrap(),
            db_path_str,
            install_root.to_str().unwrap(),
            false,
            true,
            None,
            None,
            crate::commands::SandboxMode::None,
            true,
            false,
            false,
            None,
        )
        .await
        .unwrap_err();

        assert!(
            err.to_string().contains("duplicate deployment path"),
            "unexpected error: {err:#}"
        );
    }

    #[tokio::test]
    async fn ccs_install_registers_metadata_only_package_without_files() {
        use conary_core::ccs::builder::write_ccs_package;
        use conary_core::ccs::{BuildResult, CcsManifest};

        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        let temp_dir = tempfile::tempdir().unwrap();
        let install_root = temp_dir.path().join("root");
        let package_path = temp_dir.path().join("metadata-only.ccs");
        let db_path = temp_dir.path().join("conary.db");
        let db_path_str = db_path.to_str().unwrap();

        std::fs::create_dir_all(&install_root).unwrap();
        conary_core::db::init(db_path_str).unwrap();
        stage_test_boot_assets(temp_dir.path());
        seed_test_init_trove(db_path_str, temp_dir.path());

        let result = BuildResult {
            manifest: CcsManifest::new_minimal("metadata-only", "1.0.0"),
            components: HashMap::new(),
            files: Vec::new(),
            blobs: HashMap::new(),
            total_size: 0,
            chunked: false,
            chunk_stats: None,
        };
        write_ccs_package(&result, &package_path).unwrap();

        super::cmd_ccs_install(
            package_path.to_str().unwrap(),
            db_path_str,
            install_root.to_str().unwrap(),
            false,
            true,
            None,
            None,
            crate::commands::SandboxMode::None,
            true,
            false,
            false,
            None,
        )
        .await
        .unwrap();

        let conn = conary_core::db::open(db_path_str).unwrap();
        let trove_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM troves WHERE name = 'metadata-only'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let file_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) \
                 FROM files f \
                 JOIN troves t ON t.id = f.trove_id \
                 WHERE t.name = 'metadata-only'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(trove_count, 1);
        assert_eq!(file_count, 0);
    }

    #[tokio::test]
    async fn ccs_install_records_ldconfig_trigger_for_shared_libraries() {
        use conary_core::ccs::builder::write_ccs_package;
        use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
        use conary_core::hash;

        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        let temp_dir = tempfile::tempdir().unwrap();
        let install_root = temp_dir.path().join("root");
        let package_path = temp_dir.path().join("shared-lib.ccs");
        let db_path = temp_dir.path().join("conary.db");
        let db_path_str = db_path.to_str().unwrap();

        std::fs::create_dir_all(&install_root).unwrap();
        conary_core::db::init(db_path_str).unwrap();
        stage_test_boot_assets(temp_dir.path());

        let content = b"not a real elf; trigger matching is path-based".to_vec();
        let file_hash = hash::sha256(&content);
        let (init_file, init_content, init_hash) = ccs_init_file();
        let lib_file = FileEntry {
            path: "/usr/lib64/libtrigger-test.so.1".to_string(),
            hash: file_hash.clone(),
            size: content.len() as u64,
            mode: 0o100644,
            component: "lib".to_string(),
            file_type: FileType::Regular,
            target: None,
            chunks: None,
        };
        let files = vec![lib_file.clone(), init_file.clone()];

        let result = BuildResult {
            manifest: CcsManifest::new_minimal("shared-lib", "1.0.0"),
            components: HashMap::from([
                (
                    "lib".to_string(),
                    ComponentData {
                        name: "lib".to_string(),
                        files: vec![lib_file],
                        hash: "lib".to_string(),
                        size: content.len() as u64,
                    },
                ),
                (
                    "runtime".to_string(),
                    ComponentData {
                        name: "runtime".to_string(),
                        files: vec![init_file],
                        hash: "runtime".to_string(),
                        size: init_content.len() as u64,
                    },
                ),
            ]),
            files,
            blobs: HashMap::from([(file_hash, content), (init_hash, init_content)]),
            total_size: 0,
            chunked: false,
            chunk_stats: None,
        };
        write_ccs_package(&result, &package_path).unwrap();

        super::cmd_ccs_install(
            package_path.to_str().unwrap(),
            db_path_str,
            install_root.to_str().unwrap(),
            false,
            true,
            None,
            None,
            crate::commands::SandboxMode::None,
            true,
            false,
            false,
            None,
        )
        .await
        .unwrap();

        let conn = conary_core::db::open(db_path_str).unwrap();
        let (status, matched_files): (String, i64) = conn
            .query_row(
                "SELECT ct.status, ct.matched_files \
                 FROM changeset_triggers ct \
                 JOIN triggers t ON t.id = ct.trigger_id \
                 WHERE t.name = 'ldconfig'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(matched_files, 1);
        assert_eq!(status, "completed");
    }

    #[tokio::test]
    async fn ccs_install_marks_changeset_post_hooks_failed_after_post_install_error() {
        use conary_core::ccs::builder::write_ccs_package;
        use conary_core::ccs::manifest::ScriptHook;
        use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
        use conary_core::hash;

        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        let temp_dir = tempfile::tempdir().unwrap();
        let install_root = temp_dir.path().join("root");
        let package_path = temp_dir.path().join("post-hook-fails.ccs");
        let db_path = temp_dir.path().join("conary.db");
        let db_path_str = db_path.to_str().unwrap();

        std::fs::create_dir_all(&install_root).unwrap();
        conary_core::db::init(db_path_str).unwrap();
        stage_test_boot_assets(temp_dir.path());

        let content = b"hello".to_vec();
        let hash = hash::sha256(&content);
        let (init_file, init_content, init_hash) = ccs_init_file();
        let payload_file = FileEntry {
            path: "/usr/bin/post-hook-fails".to_string(),
            hash: hash.clone(),
            size: content.len() as u64,
            mode: 0o100755,
            component: "runtime".to_string(),
            file_type: FileType::Regular,
            target: None,
            chunks: None,
        };
        let files = vec![payload_file.clone(), init_file.clone()];

        let mut manifest = CcsManifest::new_minimal("post-hook-fails", "1.0.0");
        manifest.hooks.post_install = Some(ScriptHook {
            script: "exit 23".to_string(),
        });

        let result = BuildResult {
            manifest,
            components: HashMap::from([(
                "runtime".to_string(),
                ComponentData {
                    name: "runtime".to_string(),
                    files: files.clone(),
                    hash: "runtime".to_string(),
                    size: (content.len() + init_content.len()) as u64,
                },
            )]),
            files,
            blobs: HashMap::from([(hash, content), (init_hash, init_content)]),
            total_size: 5 + init_file.size,
            chunked: false,
            chunk_stats: None,
        };
        write_ccs_package(&result, &package_path).unwrap();

        super::cmd_ccs_install(
            package_path.to_str().unwrap(),
            db_path_str,
            install_root.to_str().unwrap(),
            false,
            true,
            None,
            None,
            crate::commands::SandboxMode::None,
            true,
            false,
            false,
            None,
        )
        .await
        .unwrap();

        let conn = conary_core::db::open(db_path_str).unwrap();
        let (status, description): (String, String) = conn
            .query_row(
                "SELECT status, description FROM changesets ORDER BY id DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "post_hooks_failed");
        assert!(!description.contains("[post-hooks failed]"));
    }

    #[tokio::test]
    async fn ccs_install_reverts_pre_hook_directories_when_deploy_fails() {
        use conary_core::ccs::builder::write_ccs_package;
        use conary_core::ccs::manifest::DirectoryHook;
        use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
        use conary_core::hash;

        let temp_dir = tempfile::tempdir().unwrap();
        let install_root = temp_dir.path().join("root");
        let outside_root = temp_dir.path().join("outside");
        let package_path = temp_dir.path().join("revert-pre-hooks.ccs");
        let db_path = temp_dir.path().join("conary.db");
        let db_path_str = db_path.to_str().unwrap();

        std::fs::create_dir_all(&install_root).unwrap();
        std::fs::create_dir_all(&outside_root).unwrap();
        conary_core::db::init(db_path_str).unwrap();

        let file_content = b"blocked".to_vec();
        let file_hash = hash::sha256(&file_content);

        let files = vec![FileEntry {
            path: "/usr/lib/link/cron.d/persist".to_string(),
            hash: file_hash.clone(),
            size: file_content.len() as u64,
            mode: 0o100644,
            component: "runtime".to_string(),
            file_type: FileType::Regular,
            target: None,
            chunks: None,
        }];

        let mut manifest = CcsManifest::new_minimal("revert-pre-hooks", "1.0.0");
        manifest.hooks.directories.push(DirectoryHook {
            path: "/var/lib/revert-pre-hooks".to_string(),
            mode: "0755".to_string(),
            owner: "root".to_string(),
            group: "root".to_string(),
            cleanup: None,
        });

        let result = BuildResult {
            manifest,
            components: HashMap::from([(
                "runtime".to_string(),
                ComponentData {
                    name: "runtime".to_string(),
                    files: files.clone(),
                    hash: "runtime".to_string(),
                    size: file_content.len() as u64,
                },
            )]),
            files,
            blobs: HashMap::from([(file_hash, file_content)]),
            total_size: 7,
            chunked: false,
            chunk_stats: None,
        };
        write_ccs_package(&result, &package_path).unwrap();
        std::fs::create_dir_all(install_root.join("usr/lib")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside_root, install_root.join("usr/lib/link")).unwrap();

        let err = super::cmd_ccs_install(
            package_path.to_str().unwrap(),
            db_path_str,
            install_root.to_str().unwrap(),
            false,
            true,
            None,
            None,
            crate::commands::SandboxMode::None,
            true,
            false,
            false,
            None,
        )
        .await
        .unwrap_err();

        assert!(
            err.to_string().contains("path traversal") || err.to_string().contains("symlink"),
            "unexpected error: {err:#}"
        );
        assert!(
            !install_root.join("var/lib/revert-pre-hooks").exists(),
            "pre-hook directory should be reverted on failure"
        );
    }

    #[tokio::test]
    async fn ccs_install_skips_post_install_hook_for_devel_only_component_selection() {
        use conary_core::ccs::builder::write_ccs_package;
        use conary_core::ccs::manifest::ScriptHook;
        use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
        use conary_core::hash;

        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        let temp_dir = tempfile::tempdir().unwrap();
        let install_root = temp_dir.path().join("root");
        let package_path = temp_dir.path().join("devel-only.ccs");
        let db_path = temp_dir.path().join("conary.db");
        let db_path_str = db_path.to_str().unwrap();
        let hook_marker = install_root.join("var/lib/devel-only/post-install-ran");

        std::fs::create_dir_all(&install_root).unwrap();
        conary_core::db::init(db_path_str).unwrap();
        stage_test_boot_assets(temp_dir.path());
        seed_test_init_trove(db_path_str, temp_dir.path());

        let runtime_content = b"#!/bin/sh\necho runtime\n".to_vec();
        let runtime_hash = hash::sha256(&runtime_content);
        let devel_content = b"#pragma once\n".to_vec();
        let devel_hash = hash::sha256(&devel_content);

        let runtime_file = FileEntry {
            path: "/usr/bin/devel-only".to_string(),
            hash: runtime_hash.clone(),
            size: runtime_content.len() as u64,
            mode: 0o100755,
            component: "runtime".to_string(),
            file_type: FileType::Regular,
            target: None,
            chunks: None,
        };
        let devel_file = FileEntry {
            path: "/usr/include/devel-only/api.h".to_string(),
            hash: devel_hash.clone(),
            size: devel_content.len() as u64,
            mode: 0o100644,
            component: "devel".to_string(),
            file_type: FileType::Regular,
            target: None,
            chunks: None,
        };

        let mut manifest = CcsManifest::new_minimal("devel-only", "1.0.0");
        manifest.hooks.post_install = Some(ScriptHook {
            script: format!(
                "mkdir -p '{}' && touch '{}'",
                hook_marker.parent().unwrap().display(),
                hook_marker.display()
            ),
        });

        let result = BuildResult {
            manifest,
            components: HashMap::from([
                (
                    "runtime".to_string(),
                    ComponentData {
                        name: "runtime".to_string(),
                        files: vec![runtime_file.clone()],
                        hash: "runtime".to_string(),
                        size: runtime_content.len() as u64,
                    },
                ),
                (
                    "devel".to_string(),
                    ComponentData {
                        name: "devel".to_string(),
                        files: vec![devel_file.clone()],
                        hash: "devel".to_string(),
                        size: devel_content.len() as u64,
                    },
                ),
            ]),
            files: vec![runtime_file, devel_file],
            blobs: HashMap::from([(runtime_hash, runtime_content), (devel_hash, devel_content)]),
            total_size: 0,
            chunked: false,
            chunk_stats: None,
        };
        write_ccs_package(&result, &package_path).unwrap();

        super::cmd_ccs_install(
            package_path.to_str().unwrap(),
            db_path_str,
            install_root.to_str().unwrap(),
            false,
            true,
            None,
            Some(vec!["devel".to_string()]),
            crate::commands::SandboxMode::None,
            true,
            false,
            false,
            None,
        )
        .await
        .unwrap();

        assert!(
            !hook_marker.exists(),
            "post-install hook should be skipped when only :devel is installed"
        );
    }
}
