// src/commands/adopt/convert.rs

//! Parallel batch CCS conversion pipeline for adopted packages
//!
//! Converts adopted system packages to native CCS format using rayon
//! for parallel builds. Files are copied from the live filesystem,
//! packaged via CcsBuilder, and tracked via ConvertedPackage records.

use super::super::create_state_snapshot;
use super::super::open_db;
use super::super::progress::AdoptProgress;
use super::checkpoint::write_db_checkpoint;
use anyhow::{Context, Result};
use conary_core::ccs::builder::{CcsBuilder, write_signed_current_ccs_package};
use conary_core::ccs::manifest::{CcsManifest, Platform};
use conary_core::ccs::signing::SigningKeyPair;
use conary_core::ccs::verify::{TrustPolicy, verify_package};
use conary_core::db::backup::CheckpointReason;
use conary_core::db::models::{
    Changeset, ChangesetStatus, ConvertedPackage, FileEntry, InstalledRequirementGroup,
    ProvideEntry, Trove,
};
use rayon::prelude::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::{debug, warn};

/// Collected data for one adopted trove ready for parallel conversion
struct AdoptedTroveBundle {
    trove: Trove,
    files: Vec<FileEntry>,
    requirements: Vec<InstalledRequirementGroup>,
    provides: Vec<ProvideEntry>,
}

/// Successful conversion data for one package
struct ConversionSuccess {
    trove_id: i64,
    converted: ConvertedPackage,
}

/// Per-package conversion result returned from parallel workers
enum PackageConversionResult {
    Success(Box<ConversionSuccess>),
    Failed { name: String, error: String },
}

/// Query adopted troves that lack a ConvertedPackage record.
///
/// Uses a LEFT JOIN to find trove IDs with adopted install sources
/// that have not yet been converted to CCS format, then loads each
/// trove via `Trove::find_by_id`.
fn query_unconverted_adopted(conn: &rusqlite::Connection) -> Result<Vec<Trove>> {
    let mut stmt = conn.prepare(
        "SELECT t.id FROM troves t \
         LEFT JOIN converted_packages cp ON cp.trove_id = t.id \
         WHERE t.install_source IN ('adopted-track', 'adopted-full') \
         AND cp.id IS NULL \
         ORDER BY t.name",
    )?;

    let ids: Vec<i64> = stmt
        .query_map([], |row| row.get(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    let mut troves = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(trove) = Trove::find_by_id(conn, id)? {
            troves.push(trove);
        }
    }

    Ok(troves)
}

/// Convert all unconverted adopted packages to CCS format.
///
/// This command:
/// 1. Queries adopted troves missing a `ConvertedPackage` record
/// 2. Collects DB data (files, deps, provides) sequentially
/// 3. Converts each package to CCS in parallel via rayon
/// 4. Inserts conversion records in a single DB transaction
/// 5. Creates a state snapshot for rollback safety
pub async fn cmd_adopt_convert(
    db_path: &str,
    jobs: Option<usize>,
    no_chunking: bool,
    dry_run: bool,
    signing_key_path: Option<&Path>,
) -> Result<()> {
    let mut conn = open_db(db_path)?;

    // 1. Query unconverted adopted troves
    let troves = query_unconverted_adopted(&conn)?;

    if troves.is_empty() {
        println!("No unconverted adopted packages found.");
        return Ok(());
    }

    println!(
        "Found {} adopted packages to convert to CCS format.",
        troves.len()
    );

    if dry_run {
        for t in &troves {
            println!("  {} {}", t.name, t.version);
        }
        println!("\nDry run: no packages converted.");
        return Ok(());
    }

    let signing_key_path = signing_key_path.context(
        "adopted-package CCS conversion requires --key <private-key>; no default signing authority exists",
    )?;
    let signing_key = Arc::new(
        SigningKeyPair::load_from_file(signing_key_path).with_context(|| {
            format!(
                "load adopted-package CCS signing key {}",
                signing_key_path.display()
            )
        })?,
    );

    write_db_checkpoint(db_path, CheckpointReason::PreMutation)?;

    // 2. Collect DB data sequentially (SQLite is single-threaded)
    let bundles: Vec<AdoptedTroveBundle> = troves
        .into_iter()
        .map(|trove| {
            let trove_id = trove.id.ok_or_else(|| {
                anyhow::anyhow!("Adopted trove '{}' has no database id", trove.name)
            })?;
            let files = FileEntry::find_by_trove(&conn, trove_id)?;
            let requirements = InstalledRequirementGroup::find_by_trove(&conn, trove_id)?;
            let provides = ProvideEntry::find_by_trove(&conn, trove_id)?;
            Ok(AdoptedTroveBundle {
                trove,
                files,
                requirements,
                provides,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    // 3. Determine output directory (sibling to DB's objects/)
    let db_parent = Path::new(db_path)
        .parent()
        .unwrap_or(Path::new("/var/lib/conary"));
    let output_dir = db_parent.join("packages");
    fs::create_dir_all(&output_dir)?;

    // 4. Configure rayon thread pool
    if let Some(j) = jobs
        && let Err(e) = rayon::ThreadPoolBuilder::new()
            .num_threads(j)
            .build_global()
    {
        warn!("Could not set thread pool to {j} threads (already initialized): {e}");
    }

    // 5. Progress tracking
    let progress = AdoptProgress::new(bundles.len() as u64, "Converting to CCS");
    let completed = Arc::new(AtomicU64::new(0));

    // 6. Parallel conversion via rayon
    let enable_chunking = !no_chunking;
    let completed_ref = Arc::clone(&completed);
    let signing_key_ref = Arc::clone(&signing_key);
    let bundle_count = bundles.len() as u64;
    let results: Vec<PackageConversionResult> = bundles
        .par_iter()
        .map(|bundle| {
            let result = convert_single_package(
                bundle,
                &output_dir,
                enable_chunking,
                signing_key_ref.as_ref(),
            );
            let done = completed_ref.fetch_add(1, Ordering::Relaxed) + 1;
            debug!("Converted {}/{}: {}", done, bundle_count, bundle.trove.name);
            result
        })
        .collect();

    progress.finish("Conversion complete");

    // 7. Single DB transaction for all conversion record inserts
    let mut converted_count: u64 = 0;
    let mut failed_count: u64 = 0;
    let mut failed_names: Vec<String> = Vec::new();

    let changeset_id = conary_core::db::transaction(&mut conn, |tx| {
        let mut changeset = Changeset::new("Batch CCS conversion of adopted packages".to_string());
        let cs_id = changeset.insert(tx)?;

        for result in results {
            match result {
                PackageConversionResult::Success(success) => {
                    let ConversionSuccess {
                        trove_id,
                        mut converted,
                    } = *success;
                    if let Err(e) = converted.insert(tx) {
                        warn!(
                            "Failed to insert ConvertedPackage for trove {}: {}",
                            trove_id, e
                        );
                        failed_count += 1;
                    } else {
                        converted_count += 1;
                    }
                }
                PackageConversionResult::Failed { name, error } => {
                    let row = format!("{name}: {error}");
                    eprintln!("{}", crate::ui::row_line(crate::ui::Status::Fail, &[&row]));
                    failed_names.push(name);
                    failed_count += 1;
                }
            }
        }

        changeset.update_status(tx, ChangesetStatus::Applied)?;
        Ok(cs_id)
    })?;

    // 8. State snapshot for rollback safety
    if converted_count > 0 {
        create_state_snapshot(
            &conn,
            changeset_id,
            &format!("CCS conversion: {} packages", converted_count),
        )?;
    }
    write_db_checkpoint(db_path, CheckpointReason::PostSuccess)?;

    // 9. Summary
    println!("\nConversion summary:");
    println!("  Converted: {}", converted_count);
    if failed_count > 0 {
        println!("  Failed:    {}", failed_count);
        for name in &failed_names {
            println!("    - {}", name);
        }
    }
    println!("  Output:    {}", output_dir.display());

    Ok(())
}

/// Convert a single package, wrapping errors into the result enum.
fn convert_single_package(
    bundle: &AdoptedTroveBundle,
    output_dir: &Path,
    enable_chunking: bool,
    signing_key: &SigningKeyPair,
) -> PackageConversionResult {
    match convert_single_package_inner(bundle, output_dir, enable_chunking, signing_key) {
        Ok(converted) => PackageConversionResult::Success(Box::new(ConversionSuccess {
            trove_id: match bundle.trove.id {
                Some(id) => id,
                None => {
                    return PackageConversionResult::Failed {
                        name: bundle.trove.name.clone(),
                        error: "Trove has no database id".to_string(),
                    };
                }
            },
            converted,
        })),
        Err(e) => PackageConversionResult::Failed {
            name: bundle.trove.name.clone(),
            error: e.to_string(),
        },
    }
}

/// Inner conversion logic for a single adopted package.
///
/// Builds a CCS manifest from trove metadata, copies files to a temp
/// directory, runs the CCS builder, and writes the `.ccs` output file.
fn convert_single_package_inner(
    bundle: &AdoptedTroveBundle,
    output_dir: &Path,
    enable_chunking: bool,
    signing_key: &SigningKeyPair,
) -> Result<ConvertedPackage> {
    let trove = &bundle.trove;

    // Build manifest from trove metadata + deps + provides
    let manifest = build_manifest_from_adopted(bundle)?;

    // Create temp dir and copy files from the live filesystem
    let temp_dir = tempfile::tempdir()?;
    copy_files_to_temp(&bundle.files, temp_dir.path())?;

    // Build CCS package
    let mut builder = CcsBuilder::new(manifest, temp_dir.path());
    if enable_chunking {
        builder = builder.with_chunking();
    }
    let build_result = builder.build()?;

    // Write .ccs file to output directory (include arch to avoid multi-arch collisions)
    let arch_suffix = trove.architecture.as_deref().unwrap_or("noarch");
    let output_filename = format!("{}-{}-{}.ccs", trove.name, trove.version, arch_suffix);
    let ccs_path = output_dir.join(&output_filename);
    write_signed_current_ccs_package(&build_result, &ccs_path, signing_key, false)?;
    verify_package(
        &ccs_path,
        &TrustPolicy::strict(vec![signing_key.public_key_base64()]),
    )
    .with_context(|| {
        format!(
            "verify newly converted adopted CCS package {}",
            ccs_path.display()
        )
    })?;

    // Build ConvertedPackage record (include arch in dedup key)
    let format_str = if trove.install_source.is_adopted() {
        "adopted"
    } else {
        "unknown"
    };
    let original_checksum = format!(
        "adopted:{}:{}:{}:{}",
        format_str, trove.name, trove.version, arch_suffix
    );

    let trove_id = trove
        .id
        .ok_or_else(|| anyhow::anyhow!("Trove has no database id"))?;
    let converted =
        ConvertedPackage::new_installed(trove_id, format_str.to_string(), original_checksum);

    Ok(converted)
}

/// Build a CCS manifest from an adopted trove's metadata.
///
/// Populates description, platform, exact requirements, and provides sections
/// from the database records collected for this trove.
fn build_manifest_from_adopted(bundle: &AdoptedTroveBundle) -> Result<CcsManifest> {
    let trove = &bundle.trove;
    let mut manifest = CcsManifest::new_minimal(&trove.name, &trove.version);

    // Set description if available
    if let Some(ref desc) = trove.description {
        manifest.package.description = desc.clone();
    }

    // Set platform from architecture
    if let Some(ref arch) = trove.architecture {
        manifest.package.platform = Some(Platform {
            os: "linux".to_string(),
            arch: Some(arch.clone()),
            libc: "gnu".to_string(),
            abi: None,
        });
    }

    for stored in &bundle.requirements {
        if stored.kind.is_negative_relation() {
            manifest.relations.push(stored.requirement.clone());
        } else {
            manifest.requirements.push(stored.requirement.clone());
        }
    }

    // Convert provides to CCS format
    manifest.provides.capabilities = bundle
        .provides
        .iter()
        .map(|p| p.capability.clone())
        .collect();

    Ok(manifest)
}

/// Copy package files from the live filesystem to a temp staging directory.
///
/// Preserves directory structure, handles symlinks, and sets permissions.
/// Files that no longer exist on disk are silently skipped.
fn copy_files_to_temp(files: &[FileEntry], temp_dir: &Path) -> Result<()> {
    for file in files {
        let rel_path = file.path.strip_prefix('/').unwrap_or(&file.path);
        let dest = temp_dir.join(rel_path);

        // Guard against path traversal (e.g., "../../etc/shadow")
        if !dest.starts_with(temp_dir) {
            warn!(
                "Skipping file with path traversal: {} -> {}",
                file.path,
                dest.display()
            );
            continue;
        }

        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }

        let source = Path::new(&file.path);
        if source.is_symlink() {
            let target = fs::read_link(source)?;
            std::os::unix::fs::symlink(&target, &dest)?;
        } else if source.is_file() {
            fs::copy(source, &dest)?;
            // Preserve permissions from the DB record
            let perms = fs::Permissions::from_mode(file.node.source.mode & 0o7777);
            fs::set_permissions(&dest, perms)?;
        }
        // Skip files that no longer exist on disk (may have been removed)
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db::models::{InstallSource, TroveType};
    use conary_core::packages::InstalledPackageIdentity;
    use conary_core::repository::dependency_model::RepositoryRequirementKind;

    /// Helper to create a minimal trove for testing
    fn make_test_trove(name: &str, version: &str) -> Trove {
        let mut t = Trove::new_with_source(
            name.to_string(),
            version.to_string(),
            TroveType::Package,
            InstallSource::AdoptedTrack,
            conary_core::repository::versioning::VersionScheme::Conary,
        );
        t.id = Some(42);
        t.architecture = Some("x86_64".to_string());
        t.description = Some("A test package".to_string());
        t.native_package_identity =
            Some(InstalledPackageIdentity::pacman(name, name, version, "x86_64").unwrap());
        t
    }

    fn stored_requirement(
        kind: RepositoryRequirementKind,
        native: &str,
    ) -> InstalledRequirementGroup {
        let requirement = conary_core::repository::requirement::parse_native_requirement(
            kind,
            conary_core::repository::versioning::VersionScheme::Conary,
            native,
        )
        .unwrap();
        InstalledRequirementGroup::from_group(
            42,
            conary_core::repository::versioning::VersionScheme::Conary,
            requirement,
        )
        .unwrap()
    }

    #[test]
    fn test_build_manifest_from_adopted() {
        let trove = make_test_trove("nginx", "1.24.0");

        let requirements = vec![
            stored_requirement(RepositoryRequirementKind::Depends, "openssl >= 3.1.0"),
            stored_requirement(RepositoryRequirementKind::Depends, "pcre2"),
            stored_requirement(RepositoryRequirementKind::Build, "gcc"),
        ];

        let provides = vec![
            ProvideEntry::new(42, "nginx".to_string(), Some("1.24.0".to_string())),
            ProvideEntry::new(42, "webserver".to_string(), None),
        ];

        let bundle = AdoptedTroveBundle {
            trove,
            files: Vec::new(),
            requirements,
            provides,
        };

        let manifest = build_manifest_from_adopted(&bundle).unwrap();

        // Package metadata
        assert_eq!(manifest.package.name, "nginx");
        assert_eq!(manifest.package.version, "1.24.0");
        assert_eq!(manifest.package.description, "A test package");

        // Platform
        let platform = manifest.package.platform.unwrap();
        assert_eq!(platform.arch, Some("x86_64".to_string()));
        assert_eq!(platform.os, "linux");

        // Exact requirements retain runtime constraints and build ownership.
        assert_eq!(manifest.requirements.len(), 3);
        assert_eq!(manifest.requirements[0].alternatives[0].name, "openssl");
        assert_eq!(
            manifest.requirements[0].alternatives[0]
                .version_constraint
                .as_deref(),
            Some(">= 3.1.0")
        );
        assert_eq!(manifest.requirements[1].alternatives[0].name, "pcre2");
        assert_eq!(
            manifest.requirements[2].kind,
            RepositoryRequirementKind::Build
        );

        // Provides
        assert_eq!(manifest.provides.capabilities.len(), 2);
        assert!(
            manifest
                .provides
                .capabilities
                .contains(&"nginx".to_string())
        );
        assert!(
            manifest
                .provides
                .capabilities
                .contains(&"webserver".to_string())
        );
    }

    #[test]
    fn test_build_manifest_empty_deps() {
        let trove = make_test_trove("simple-pkg", "0.1.0");

        let bundle = AdoptedTroveBundle {
            trove,
            files: Vec::new(),
            requirements: Vec::new(),
            provides: Vec::new(),
        };

        let manifest = build_manifest_from_adopted(&bundle).unwrap();

        assert_eq!(manifest.package.name, "simple-pkg");
        assert_eq!(manifest.package.version, "0.1.0");
        assert!(manifest.requirements.is_empty());
        assert!(manifest.provides.capabilities.is_empty());
    }

    #[test]
    fn test_dedup_key_format() {
        // Verify the original_checksum format used as the dedup key (includes arch)
        let trove = make_test_trove("bash", "5.2.21");

        let format_str = if trove.install_source.is_adopted() {
            "adopted"
        } else {
            "unknown"
        };
        let arch_suffix = trove.architecture.as_deref().unwrap_or("noarch");
        let key = format!(
            "adopted:{}:{}:{}:{}",
            format_str, trove.name, trove.version, arch_suffix
        );

        assert_eq!(key, "adopted:adopted:bash:5.2.21:x86_64");

        // Non-adopted source should produce "unknown"
        let mut non_adopted = Trove::new(
            "foo".to_string(),
            "1.0".to_string(),
            TroveType::Package,
            conary_core::repository::versioning::VersionScheme::Conary,
        );
        non_adopted.id = Some(99);
        let format_str2 = if non_adopted.install_source.is_adopted() {
            "adopted"
        } else {
            "unknown"
        };
        let arch2 = non_adopted.architecture.as_deref().unwrap_or("noarch");
        let key2 = format!(
            "adopted:{}:{}:{}:{}",
            format_str2, non_adopted.name, non_adopted.version, arch2
        );
        assert_eq!(key2, "adopted:unknown:foo:1.0:noarch");
    }

    #[test]
    fn adopted_conversion_emits_verified_signed_v2() {
        let temp = tempfile::tempdir().unwrap();
        let bundle = AdoptedTroveBundle {
            trove: make_test_trove("signed-adopted", "1.0.0"),
            files: Vec::new(),
            requirements: Vec::new(),
            provides: Vec::new(),
        };
        let signer = SigningKeyPair::generate().with_key_id("adopt-test");

        convert_single_package_inner(&bundle, temp.path(), false, &signer).unwrap();

        let package_path = temp.path().join("signed-adopted-1.0.0-x86_64.ccs");
        let verified = verify_package(
            &package_path,
            &TrustPolicy::strict(vec![signer.public_key_base64()]),
        )
        .unwrap();
        let package = conary_core::ccs::CcsPackage::from_verified_archive(
            &package_path.to_string_lossy(),
            &verified,
        )
        .unwrap();
        assert_eq!(package.manifest().package.name, "signed-adopted");
    }
}
