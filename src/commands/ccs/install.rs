// src/commands/ccs/install.rs

//! CCS package installation
//!
//! Commands for installing CCS packages with signature verification,
//! dependency checking, and hook execution.

use anyhow::{Context, Result};
use conary_core::ccs::{CcsPackage, HookExecutor, TrustPolicy, verify};
use conary_core::db::models::generate_capability_variations;
use conary_core::db::models::{Changeset, ChangesetStatus};
use conary_core::dependencies::{DependencyClass, LanguageDepDetector};
use conary_core::packages::traits::PackageFormat;
use conary_core::packages::traits::{Scriptlet, ScriptletPhase};
use conary_core::repository::versioning::{
    RepoVersionConstraint, VersionScheme, parse_repo_constraint, repo_version_satisfies,
};
use conary_core::scriptlet::{
    ExecutionMode, PackageFormat as ScriptletPackageFormat, ScriptletExecutor,
};
use rusqlite::params;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

fn package_provided_names(ccs_pkg: &CcsPackage) -> std::collections::HashSet<String> {
    std::iter::once(ccs_pkg.name().to_string())
        .chain(ccs_pkg.manifest().provides.capabilities.iter().cloned())
        .chain(ccs_pkg.manifest().provides.sonames.iter().cloned())
        .chain(ccs_pkg.manifest().provides.binaries.iter().cloned())
        .chain(ccs_pkg.manifest().provides.pkgconfig.iter().cloned())
        .collect()
}

fn package_self_provides(ccs_pkg: &CcsPackage, dep_name: &str) -> bool {
    let provided = package_provided_names(ccs_pkg);
    if provided.contains(dep_name) {
        return true;
    }

    for variation in generate_capability_variations(dep_name) {
        if provided.contains(&variation) {
            return true;
        }
    }

    false
}

fn test_hold_ms(var_name: &str) -> Option<Duration> {
    std::env::var(var_name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .map(Duration::from_millis)
}

fn sanitize_package_relative_path(path: &str) -> Result<PathBuf> {
    let candidate = path.strip_prefix('/').unwrap_or(path);
    let mut normalized = PathBuf::new();

    for component in Path::new(candidate).components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir => anyhow::bail!("path traversal detected in package path: {path}"),
            Component::RootDir | Component::Prefix(_) => {
                anyhow::bail!("invalid package path component in {path}")
            }
        }
    }

    if normalized.as_os_str().is_empty() {
        anyhow::bail!("empty package path is not allowed");
    }

    Ok(normalized)
}

fn deployed_mode(mode: i32) -> (i32, bool) {
    let stripped = mode & !0o6000;
    (stripped, stripped != mode)
}

fn is_symlink_mode(mode: i32) -> bool {
    (mode & 0o170000) == 0o120000
}

fn sandbox_failure_message(script: &str, error: &dyn std::fmt::Display) -> String {
    if script.contains("/proc/self/environ") {
        return format!("sandbox blocked /proc/self/environ access: {error}");
    }
    if script.contains("curl ")
        || script.contains("wget ")
        || script.contains("/dev/tcp/")
        || script.contains("/dev/udp/")
    {
        return format!("sandbox blocked network access: {error}");
    }
    if script.contains(">/tmp/")
        || script.contains("> /tmp/")
        || script.contains(">/etc/")
        || script.contains("> /etc/")
    {
        return format!("sandbox denied write outside policy: {error}");
    }
    format!("sandbox blocked script execution: {error}")
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
            version_satisfies_constraint(&trove.version, trove.version_scheme.as_deref(), version_constraint)
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
        && conary_core::db::models::ProvideEntry::is_capability_satisfied_fuzzy(conn, package_name)?
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
    let scheme = installed_package_version_scheme(conn, package_name)?.unwrap_or(VersionScheme::Rpm);
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
        parse_version_scheme(version_scheme).unwrap_or(VersionScheme::Rpm),
        version,
        constraint,
    )
    .unwrap_or(false)
}

fn installed_package_version_scheme(
    conn: &rusqlite::Connection,
    package_name: &str,
) -> Result<Option<VersionScheme>> {
    Ok(conary_core::db::models::Trove::find_by_name(conn, package_name)?
        .into_iter()
        .find_map(|trove| parse_version_scheme(trove.version_scheme.as_deref())))
}

fn parse_version_scheme(raw: Option<&str>) -> Option<VersionScheme> {
    match raw {
        Some("rpm") => Some(VersionScheme::Rpm),
        Some("debian") => Some(VersionScheme::Debian),
        Some("arch") => Some(VersionScheme::Arch),
        _ => None,
    }
}

fn repo_constraint_set_satisfied(
    scheme: VersionScheme,
    version: &str,
    raw: &str,
) -> Result<bool> {
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
    raw.split(',').map(str::trim).filter(|part| !part.is_empty())
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
pub fn cmd_ccs_install(
    package: &str,
    db_path: &str,
    root: &str,
    dry_run: bool,
    allow_unsigned: bool,
    policy: Option<String>,
    _components: Option<Vec<String>>,
    sandbox: crate::commands::SandboxMode,
    no_deps: bool,
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

    if ccs_pkg.manifest().capabilities.is_some() {
        anyhow::bail!(
            "Package capability policy rejected {}: install-time capability enforcement is not yet supported",
            ccs_pkg.name()
        );
    }

    // Step 3: Check for existing installation
    let conn = conary_core::db::open(db_path).context("Failed to open package database")?;

    let existing = conary_core::db::models::Trove::find_by_name(&conn, ccs_pkg.name())?;
    if !existing.is_empty() {
        let old = &existing[0];
        if old.version == ccs_pkg.version() {
            anyhow::bail!(
                "Package {} version {} is already installed",
                ccs_pkg.name(),
                ccs_pkg.version()
            );
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
            let satisfied = conary_core::db::models::ProvideEntry::is_capability_satisfied_fuzzy(
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

    if dry_run {
        println!();
        println!("[DRY RUN] Would install {} files:", ccs_pkg.files().len());
        for file in ccs_pkg.files().iter().take(10) {
            println!("  {}", file.path);
        }
        if ccs_pkg.files().len() > 10 {
            println!("  ... and {} more", ccs_pkg.files().len() - 10);
        }
        return Ok(());
    }

    // Step 5: Extract file contents
    println!("Extracting files...");
    let extracted_files = ccs_pkg.extract_file_contents()?;
    println!("Extracted {} files", extracted_files.len());
    let mut seen_paths: HashMap<PathBuf, bool> = HashMap::new();
    for file in &extracted_files {
        let rel_path = sanitize_package_relative_path(&file.path)?;
        let current_is_symlink = is_symlink_mode(file.mode);
        if let Some(existing_is_symlink) = seen_paths.insert(rel_path.clone(), current_is_symlink) {
            if existing_is_symlink || current_is_symlink {
                anyhow::bail!(
                    "symlink deployment path collision detected for {}",
                    rel_path.display()
                );
            }
            anyhow::bail!("duplicate deployment path detected for {}", rel_path.display());
        }
    }
    let detected_provides = LanguageDepDetector::detect_all_provides(
        &extracted_files
            .iter()
            .map(|f| f.path.clone())
            .collect::<Vec<_>>(),
    );

    // Step 6: Execute pre-hooks
    let mut hook_executor = HookExecutor::new(Path::new(root));
    let hooks = &ccs_pkg.manifest().hooks;

    if !hooks.users.is_empty() || !hooks.groups.is_empty() || !hooks.directories.is_empty() {
        println!("Executing pre-install hooks...");
        if let Err(e) = hook_executor.execute_pre_hooks(hooks) {
            anyhow::bail!("Pre-install hook failed: {}", e);
        }
    }

    // Step 7: Deploy files to filesystem and store in CAS
    println!("Deploying files to filesystem...");
    let root_path = std::path::Path::new(root);
    let objects_dir = conary_core::db::paths::objects_dir(db_path);
    std::fs::create_dir_all(&objects_dir)?;
    let mut files_deployed = 0;

    for file in &extracted_files {
        let relative_path = sanitize_package_relative_path(&file.path)?;
        let dest_path = root_path.join(&relative_path);
        let (effective_mode, stripped_special_bits) = deployed_mode(file.mode);

        // Create parent directories
        if let Some(parent) = dest_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        if is_symlink_mode(file.mode) {
            #[cfg(unix)]
            {
                use std::os::unix::fs::symlink;

                let target = std::str::from_utf8(&file.content)
                    .context("invalid symlink target in package payload")?;
                symlink(target, &dest_path)?;
            }
            #[cfg(not(unix))]
            {
                anyhow::bail!("symlink payloads are not supported on this platform");
            }
        } else {
            std::fs::write(&dest_path, &file.content)?;
        }

        // Set permissions
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if !is_symlink_mode(file.mode) {
                std::fs::set_permissions(
                    &dest_path,
                    std::fs::Permissions::from_mode(effective_mode as u32),
                )?;
            }
        }

        if stripped_special_bits {
            println!(
                "Warning: stripped setuid/setgid bits from {}",
                file.path
            );
        }

        // Store in CAS for rollback support
        if let Some(ref hash) = file.sha256
            && hash.len() == 64
        {
            if let Some(delay) = test_hold_ms("CONARY_TEST_HOLD_BEFORE_CAS_WRITE_MS") {
                std::thread::sleep(delay);
            }
            if !objects_dir.exists() {
                anyhow::bail!(
                    "CAS objects directory disappeared during install: {}",
                    objects_dir.display()
                );
            }
            let cas_dir = objects_dir.join(&hash[0..2]);
            let cas_path = cas_dir.join(&hash[2..]);
            if !cas_path.exists() {
                std::fs::create_dir_all(&cas_dir)?;
                std::fs::write(&cas_path, &file.content)?;
            }
        }

        files_deployed += 1;
    }

    println!("Deployed {} files to {}", files_deployed, root);

    // Step 8: Register in database with changeset tracking
    println!("Updating database...");
    std::io::stdout().flush()?;
    if let Some(delay) = test_hold_ms("CONARY_TEST_HOLD_AFTER_DB_UPDATE_MS") {
        std::thread::sleep(delay);
    }
    let is_upgrade = !existing.is_empty();
    {
        let tx = conn.unchecked_transaction()?;

        // Create changeset for history and rollback support
        let description = if is_upgrade {
            format!(
                "CCS upgrade {} {} -> {}",
                ccs_pkg.name(),
                existing[0].version,
                ccs_pkg.version()
            )
        } else {
            format!("CCS install {} {}", ccs_pkg.name(), ccs_pkg.version())
        };
        let mut changeset = Changeset::new(description);
        let changeset_id = changeset.insert(&tx)?;

        // Remove old version if upgrading (snapshot first for rollback)
        if is_upgrade {
            let old = &existing[0];
            if let Some(old_id) = old.id {
                // Snapshot old trove for rollback support
                let old_files = conary_core::db::models::FileEntry::find_by_trove(&tx, old_id)?;
                let snapshot = crate::commands::TroveSnapshot {
                    name: old.name.clone(),
                    version: old.version.clone(),
                    architecture: old.architecture.clone(),
                    description: old.description.clone(),
                    install_source: old.install_source.as_str().to_string(),
                    files: old_files
                        .iter()
                        .map(|f| crate::commands::FileSnapshot {
                            path: f.path.clone(),
                            sha256_hash: f.sha256_hash.clone(),
                            size: f.size,
                            permissions: f.permissions,
                        })
                        .collect(),
                };
                let snapshot_json = serde_json::to_string(&snapshot)?;
                tx.execute(
                    "UPDATE changesets SET metadata = ?1 WHERE id = ?2",
                    params![&snapshot_json, changeset_id],
                )?;

                // Delete old files
                tx.execute("DELETE FROM files WHERE trove_id = ?1", [old_id])?;
                // Delete old provides
                tx.execute("DELETE FROM provides WHERE trove_id = ?1", [old_id])?;
                // Delete old trove
                tx.execute("DELETE FROM troves WHERE id = ?1", [old_id])?;
            }
        }

        // Create trove linked to changeset
        let mut trove = ccs_pkg.to_trove();
        trove.installed_by_changeset_id = Some(changeset_id);
        let trove_id = trove.insert(&tx)?;

        // Register files, store in CAS index, and record history for rollback
        for file in &extracted_files {
            let hash = file.sha256.clone().unwrap_or_default();
            let mut file_entry = conary_core::db::models::FileEntry::new(
                file.path.clone(),
                hash.clone(),
                file.size,
                deployed_mode(file.mode).0,
                trove_id,
            );
            file_entry.insert_or_replace(&tx)?;

            // Register in file_contents (CAS index) and file_history
            if hash.len() == 64 {
                tx.execute(
                    "INSERT OR IGNORE INTO file_contents (sha256_hash, content_path, size) \
                     VALUES (?1, ?2, ?3)",
                    params![
                        &hash,
                        format!("objects/{}/{}", &hash[0..2], &hash[2..]),
                        file.size
                    ],
                )?;

                let action = if is_upgrade { "modify" } else { "add" };
                tx.execute(
                    "INSERT INTO file_history (changeset_id, path, sha256_hash, action) \
                     VALUES (?1, ?2, ?3, ?4)",
                    params![changeset_id, &file.path, &hash, action],
                )?;
            }
        }

        // Create provides entry for the package itself
        let mut provide = conary_core::db::models::ProvideEntry::new(
            trove_id,
            ccs_pkg.name().to_string(),
            Some(ccs_pkg.version().to_string()),
        );
        provide.insert(&tx)?;

        // Register additional provides from manifest
        for cap in &ccs_pkg.manifest().provides.capabilities {
            if cap != ccs_pkg.name() {
                let mut cap_provide =
                    conary_core::db::models::ProvideEntry::new(trove_id, cap.clone(), None);
                cap_provide.insert(&tx)?;
            }
        }

        for soname in &ccs_pkg.manifest().provides.sonames {
            let mut soname_provide = conary_core::db::models::ProvideEntry::new_typed(
                trove_id,
                DependencyClass::Soname.prefix(),
                soname.clone(),
                None,
            );
            soname_provide.insert_or_ignore(&tx)?;
        }

        for binary in &ccs_pkg.manifest().provides.binaries {
            let mut binary_provide = conary_core::db::models::ProvideEntry::new_typed(
                trove_id,
                DependencyClass::Binary.prefix(),
                binary.clone(),
                None,
            );
            binary_provide.insert_or_ignore(&tx)?;
        }

        for module in &ccs_pkg.manifest().provides.pkgconfig {
            let mut pkgconfig_provide = conary_core::db::models::ProvideEntry::new_typed(
                trove_id,
                DependencyClass::PkgConfig.prefix(),
                module.clone(),
                None,
            );
            pkgconfig_provide.insert_or_ignore(&tx)?;
        }

        for dep in &detected_provides {
            let kind = match dep.class {
                DependencyClass::Package => "package",
                _ => dep.class.prefix(),
            };
            let mut detected_provide = conary_core::db::models::ProvideEntry::new_typed(
                trove_id,
                kind,
                dep.name.clone(),
                dep.version_constraint.clone(),
            );
            detected_provide.insert_or_ignore(&tx)?;
        }

        for dep in &ccs_pkg.manifest().requires.packages {
            let mut dep_entry = conary_core::db::models::DependencyEntry::new(
                trove_id,
                dep.name.clone(),
                None,
                "runtime".to_string(),
                dep.version.clone(),
            );
            dep_entry.insert(&tx)?;
        }

        for cap in &ccs_pkg.manifest().requires.capabilities {
            let mut dep_entry = conary_core::db::models::DependencyEntry::new_typed(
                trove_id,
                "capability",
                cap.name().to_string(),
                None,
                "runtime".to_string(),
                cap.version().map(|v| v.to_string()),
            );
            dep_entry.insert(&tx)?;
        }

        // Store pre_remove script as a scriptlet entry so cmd_remove can find it
        if let Some(ref hook) = hooks.pre_remove {
            let mut scriptlet = conary_core::db::models::ScriptletEntry::new(
                trove_id,
                "pre-remove".to_string(),
                "/bin/sh".to_string(),
                hook.script.clone(),
                "ccs",
            );
            scriptlet.insert(&tx)?;
        }

        // Mark changeset as applied
        changeset.update_status(&tx, ChangesetStatus::Applied)?;

        tx.commit()?;
    }

    // Step 9: Execute post-hooks (including post_install script)
    if !hooks.systemd.is_empty() || !hooks.tmpfiles.is_empty() || !hooks.sysctl.is_empty() || !hooks.alternatives.is_empty() {
        let mut non_script_hooks = hooks.clone();
        non_script_hooks.post_install = None;
        println!("Executing post-install hooks...");
        if let Err(e) = hook_executor.execute_post_hooks(&non_script_hooks) {
            anyhow::bail!("Post-install hook failed: {}", e);
        }
    }

    if let Some(ref hook) = hooks.post_install {
        println!("Executing post-install hooks...");
        let scriptlet = Scriptlet {
            phase: ScriptletPhase::PostInstall,
            interpreter: "/bin/sh".to_string(),
            content: hook.script.clone(),
            flags: None,
        };
        let sandbox_mode = match sandbox {
            crate::commands::SandboxMode::None => conary_core::scriptlet::SandboxMode::None,
            crate::commands::SandboxMode::Auto => conary_core::scriptlet::SandboxMode::Auto,
            crate::commands::SandboxMode::Always => conary_core::scriptlet::SandboxMode::Always,
        };
        let executor = ScriptletExecutor::new(
            Path::new(root),
            ccs_pkg.name(),
            ccs_pkg.version(),
            ScriptletPackageFormat::Rpm,
        )
        .with_sandbox_mode(sandbox_mode);
        if let Err(error) = executor.execute(&scriptlet, &ExecutionMode::Install) {
            if matches!(sandbox, crate::commands::SandboxMode::Always) {
                anyhow::bail!("{}", sandbox_failure_message(&hook.script, &error));
            }
            return Err(error.into());
        }
    }

    println!();
    println!(
        "Successfully installed {} v{}",
        ccs_pkg.name(),
        ccs_pkg.version()
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::installed_versions_satisfying_constraint;
    use super::validate_package_dependency;
    use super::validate_incoming_version_against_dependents;

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
    fn package_dependency_accepts_fuzzy_capability_when_no_exact_package_exists() {
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

}
