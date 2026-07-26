// src/commands/update/collection.rs

//! Collection update orchestration for `conary update @collection`.

use super::super::install::OwnershipMode;
use super::super::{SandboxMode, open_db};
use super::adopted_authority::{
    AdoptedUpdateDecision, adopted_update_decision, native_manager_for_trove,
};
use super::cmd_update;
use super::selection::{
    SecurityMetadataUnavailable, UpdateCandidateSelection, print_security_metadata_unavailable,
    security_metadata_unavailable_error, select_update_candidate,
};
use anyhow::Result;
use conary_core::db::models::{CollectionMember, Trove, TroveType};
use conary_core::repository::resolution_policy::RequestScope;
use tracing::info;

#[derive(Debug, Clone, PartialEq, Eq)]
struct CollectionUpdateTarget {
    name: String,
    version: String,
    architecture: Option<String>,
}

impl CollectionUpdateTarget {
    fn from_trove(trove: &Trove) -> Self {
        Self {
            name: trove.name.clone(),
            version: trove.version.clone(),
            architecture: trove.architecture.clone(),
        }
    }

    fn display(&self) -> String {
        match self.architecture.as_deref() {
            Some(arch) => format!("{} {} [{}]", self.name, self.version, arch),
            None => format!("{} {}", self.name, self.version),
        }
    }
}

/// Update all members of a collection/group (best-effort, per-package)
///
/// This updates all installed packages that are members of the specified collection.
/// Updates are applied one package at a time; earlier members remain updated even if
/// a later one fails.  Returns an error if any member fails to update.
/// If `security_only` is true, only applies security updates.
#[allow(clippy::too_many_arguments)]
pub async fn cmd_update_group(
    name: &str,
    db_path: &str,
    root: &str,
    security_only: bool,
    dry_run: bool,
    sandbox_mode: SandboxMode,
    ownership: Option<OwnershipMode>,
    yes: bool,
) -> Result<()> {
    info!("Updating collection: {}", name);
    let requested_ownership = ownership;
    let effective_ownership = requested_ownership.unwrap_or_default();
    let conn = open_db(db_path)?;
    let effective_source_policy =
        conary_core::repository::load_effective_policy(&conn, RequestScope::Any)?;
    let policy = effective_source_policy.resolution;

    let troves = Trove::find_by_name(&conn, name)?;
    let collection = troves
        .iter()
        .find(|t| t.trove_type == TroveType::Collection)
        .ok_or_else(|| anyhow::anyhow!("Collection '{}' not found", name))?;

    let collection_id = collection
        .id
        .ok_or_else(|| anyhow::anyhow!("Collection has no ID"))?;
    let members = CollectionMember::find_by_collection(&conn, collection_id)?;

    if members.is_empty() {
        println!("Collection '{}' has no members.", name);
        return Ok(());
    }

    // Find installed members that need updates
    let mut updates_to_apply: Vec<CollectionUpdateTarget> = Vec::new();
    let mut not_installed: Vec<String> = Vec::new();
    let mut adopted_updates_skipped = false;
    let mut security_metadata_unavailable: Vec<SecurityMetadataUnavailable> = Vec::new();

    for member in &members {
        let installed = Trove::find_by_name(&conn, &member.member_name)?
            .into_iter()
            .filter(|trove| trove.trove_type == TroveType::Package)
            .collect::<Vec<_>>();
        if installed.is_empty() {
            not_installed.push(member.member_name.clone());
            continue;
        }

        for trove in &installed {
            if trove.pinned {
                println!(
                    "  {} is pinned, skipping",
                    CollectionUpdateTarget::from_trove(trove).display()
                );
                continue;
            }

            let adopted_decision = if trove.install_source.is_adopted() {
                Some(adopted_update_decision(
                    effective_ownership,
                    requested_ownership,
                ))
            } else {
                None
            };

            if trove.install_source.is_adopted() {
                let native_manager = native_manager_for_trove(trove);
                match adopted_decision.expect("adopted trove must have an update decision") {
                    AdoptedUpdateDecision::QueueTakeover => {}
                    AdoptedUpdateDecision::SkipNativeAuthority => {
                        let guidance = native_manager.map_or_else(
                            || "the recorded external owner".to_string(),
                            |manager| manager.update_command(&trove.name),
                        );
                        println!(
                            "  {} is adopted; external authority owns updates: {}",
                            CollectionUpdateTarget::from_trove(trove).display(),
                            guidance
                        );
                        adopted_updates_skipped = true;
                        continue;
                    }
                }
            }

            let enforce_security_metadata = security_only
                && !matches!(
                    adopted_decision,
                    Some(AdoptedUpdateDecision::SkipNativeAuthority)
                );
            match select_update_candidate(&conn, trove, enforce_security_metadata, &policy)? {
                UpdateCandidateSelection::Selected(_) => {
                    updates_to_apply.push(CollectionUpdateTarget::from_trove(trove));
                }
                UpdateCandidateSelection::NoEligibleUpdate => {}
                UpdateCandidateSelection::SecurityMetadataUnavailable(unavailable) => {
                    security_metadata_unavailable.push(unavailable);
                }
            }
        }
    }

    drop(conn);

    if !security_metadata_unavailable.is_empty() {
        print_security_metadata_unavailable(&security_metadata_unavailable);
        anyhow::bail!(security_metadata_unavailable_error(
            security_metadata_unavailable.len()
        ));
    }

    if !not_installed.is_empty() {
        println!(
            "Note: {} member(s) not installed: {}",
            not_installed.len(),
            not_installed.join(", ")
        );
    }

    if updates_to_apply.is_empty() {
        if adopted_updates_skipped {
            println!(
                "No Conary-managed updates available for collection '{}'; adopted package updates remain under native package-manager authority",
                name
            );
            println!(
                "Run 'conary system adopt --refresh' after native package-manager changes before retrying Conary workflows."
            );
        } else if security_only {
            println!("No security updates available for collection '{}'", name);
        } else {
            println!("All members of collection '{}' are up to date", name);
        }
        return Ok(());
    }

    println!(
        "Updating {} package(s) from collection '{}':",
        updates_to_apply.len(),
        name
    );
    for target in &updates_to_apply {
        println!("  {}", target.display());
    }

    // Update each package
    let mut updated_count = 0;
    let mut failed_count = 0;

    for target in &updates_to_apply {
        println!("\nUpdating {}...", target.display());
        match cmd_update(
            Some(target.name.clone()),
            db_path,
            root,
            security_only,
            dry_run,
            sandbox_mode,
            requested_ownership,
            yes,
            Some(target.version.clone()),
            target.architecture.clone(),
        )
        .await
        {
            Ok(()) => updated_count += 1,
            Err(e) => {
                eprintln!("  Failed to update {}: {}", target.display(), e);
                failed_count += 1;
            }
        }
    }

    println!("\nCollection update complete:");
    println!("  Updated: {} package(s)", updated_count);
    if failed_count > 0 {
        println!("  Failed: {} package(s)", failed_count);
        return Err(anyhow::anyhow!(
            "{} of {} package(s) in collection '{}' failed to update",
            failed_count,
            updates_to_apply.len(),
            name
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::SandboxMode;
    use crate::commands::test_helpers::create_test_db;
    use conary_core::db::models::{
        CollectionMember, InstallSource, Repository, RepositoryPackage, Trove, TroveType,
    };
    use rusqlite::Connection;

    #[tokio::test]
    async fn collection_update_preserves_member_variant_selector() {
        let (_temp, db_path) = create_test_db();
        let conn = Connection::open(&db_path).unwrap();

        let mut repo = Repository::new(
            "variant-repo".to_string(),
            "https://example.test/variant".to_string(),
        );
        repo.source_profile = Some("fedora-44".to_string());
        let repo_id = repo.insert(&conn).unwrap();

        let mut collection = Trove::new(
            "base".to_string(),
            "1.0.0".to_string(),
            TroveType::Collection,
            conary_core::repository::versioning::VersionScheme::Conary,
        );
        let collection_id = collection.insert(&conn).unwrap();
        CollectionMember::new(collection_id, "demo".to_string())
            .insert(&conn)
            .unwrap();

        for arch in ["x86_64", "aarch64"] {
            let mut installed = Trove::new_with_source(
                "demo".to_string(),
                "1.0.0".to_string(),
                TroveType::Package,
                InstallSource::Repository,
                conary_core::repository::versioning::VersionScheme::Rpm,
            );
            installed.architecture = Some(arch.to_string());
            installed.source_profile = Some("fedora-44".to_string());
            installed.installed_from_repository_id = Some(repo_id);
            installed.insert(&conn).unwrap();

            let mut candidate = RepositoryPackage::new(
                repo_id,
                "demo".to_string(),
                "1.0.1".to_string(),
                conary_core::repository::versioning::VersionScheme::Rpm,
                format!("sha256:demo-{arch}"),
                123,
                format!("https://example.test/variant/demo-1.0.1-{arch}.ccs"),
            );
            candidate.architecture = Some(arch.to_string());
            candidate.source_profile = Some("fedora-44".to_string());
            candidate.insert(&conn).unwrap();
        }
        drop(conn);

        let result = cmd_update_group(
            "base",
            &db_path,
            "/",
            false,
            true,
            SandboxMode::Always,
            None,
            true,
        )
        .await;

        assert!(
            result.is_ok(),
            "collection update should preserve member variant selectors: {:?}",
            result
        );
    }
}
