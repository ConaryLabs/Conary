// conary-core/src/db/models/mod.rs

//! Data models for Conary database entities
//!
//! This module defines Rust structs that correspond to database tables
//! and provides methods for creating, reading, updating, and deleting records.
//!
//! ## Model patterns
//!
//! Two patterns coexist across these models:
//!
//! - **Struct methods** (e.g., `Trove::find_by_name`, `FileEntry::insert`): Used by most
//!   models. CRUD operations are associated functions or methods on the model struct itself.
//!
//! - **Free functions** (e.g., `add_to_path`, `get_label_path`, `remove_from_path` in
//!   `label.rs`): Used when the operation doesn't map cleanly to a single struct, or when
//!   the function operates on a concept (like label paths) rather than a single row.
//!
//! Both patterns are intentional. Struct methods are preferred for standard CRUD on a
//! single table. Free functions are used for cross-table helpers or operations that don't
//! return the model struct. New models should prefer struct methods unless there is a
//! clear reason to use free functions.

mod appstream_cache;
mod canonical;
mod changeset;
mod chunk_access;
mod collection;
mod component;
mod component_dependency;
mod config;
mod converted;
mod debian_debconf_state;
mod delta;
mod derived;
mod download_stats;
mod file_entry;
mod flavor;
mod generation_activation;
mod generation_publication;
mod installed_ccs_remove_hook;
mod installed_file_capability;
mod installed_native_lifecycle_bundle;
mod installed_requirement_atom;
mod installed_requirement_group;
mod label;
mod lifecycle_event;
mod metadata;
mod native_lifecycle_residual_state;
mod native_publication;
mod package_payload_ownership;
mod package_transaction_staging;
mod payload_claim;
mod persisted_value;
mod provenance;
mod provide_entry;
mod redirect;
mod remi_catalog;
mod remote_collection;
mod repology_cache;
mod repository;
mod repository_capability;
mod repository_package_key;
mod repository_requirement;
mod resolution;
mod state;
mod subpackage;
mod system_affinity;
mod trigger;
mod trigger_engine;
mod trove;
mod try_session;

pub mod admin_token;
pub mod audit_log;
pub mod federation_peer;
pub mod settings;

pub use appstream_cache::AppstreamCacheEntry;
pub use canonical::{CanonicalMappingAuthority, CanonicalPackage, PackageImplementation};
pub use changeset::{Changeset, ChangesetKind, ChangesetStatus};
pub use chunk_access::{ChunkAccess, ChunkStats};
pub use collection::CollectionMember;
pub use component::Component;
pub use component_dependency::{ComponentDepType, ComponentDependency, ComponentProvide};
pub use config::{ConfigBackup, ConfigFile, ConfigSource, ConfigStatus};
pub use converted::{
    CONVERSION_VERSION, ChunkConversionState, ConvertedArtifactKind, ConvertedPackage,
    EMPTY_REPOSITORY_PROVIDES_DIGEST, RepositoryConvertedArtifact,
};
pub use debian_debconf_state::DebianDebconfState;
pub use delta::{DeltaStats, PackageDelta};
pub use derived::{DerivedOverride, DerivedPackage, DerivedPatch, DerivedStatus, VersionPolicy};
pub use download_stats::{DownloadCount, DownloadStat, GlobalDownloadStats};
pub use file_entry::{ExistingDirectoryMaterialization, FileEntry};
pub use flavor::Flavor;
pub use generation_activation::{
    ActivationRequest, ActivationRequestSourceKind, GenerationActivationIntent,
    GenerationActivationIntentStatus, NewActivationRequest,
};
pub use generation_publication::{
    GenerationPublication, GenerationPublicationPhase, GenerationPublicationStatus,
};
pub use installed_ccs_remove_hook::InstalledCcsRemoveHook;
pub use installed_file_capability::InstalledFileCapability;
pub use installed_native_lifecycle_bundle::InstalledNativeLifecycleBundle;
pub use installed_requirement_atom::InstalledRequirementAtom;
pub use installed_requirement_group::InstalledRequirementGroup;
pub use label::{LabelEntry, LabelPathEntry, add_to_path, get_label_path, remove_from_path};
pub use lifecycle_event::{LifecycleEvent, NewLifecycleEvent};
pub use metadata::{MetadataTable, get_metadata, set_metadata};
pub use native_lifecycle_residual_state::NativeLifecycleResidualState;
pub use native_publication::{
    NATIVE_NOARCH, NativePackagePublication, NativePublicationStatus, normalize_native_architecture,
};
pub use package_payload_ownership::{PackagePayloadEntry, PackagePayloadOwnership};
pub use package_transaction_staging::{
    PackageTransactionSqlWork, PackageTransactionStaging, StagedAnchorDisposition, StagedConfigRow,
    StagedHistoryAction, StagedHistoryRow, StagedPayloadOutcome, StagedPayloadRow,
};
pub use payload_claim::{PayloadClaim, PayloadClaimAnchorPolicy};
pub use persisted_value::{InvalidPersistedValue, PersistedValueCorruption};
pub use provenance::Provenance;
pub use provide_entry::ProvideEntry;
pub use redirect::{Redirect, RedirectType, ResolveResult};
pub use remi_catalog::{
    RemiActiveProfileRevision, RemiCatalogResource, RemiCatalogResourceKind,
    RemiProfileActivationOutcome, RemiProfileRevisionActivation, RemiProfileRevisionMember,
    RemiProfileRevisionPin, RemiRevisionPinKind, activate_profile_revision,
    register_profile_catalog_revision,
};
pub use remote_collection::{DEFAULT_CACHE_TTL_SECS, RemoteCollection};
pub use repology_cache::RepologyCacheEntry;
pub(crate) use repository::version_scheme_from_row;
pub use repository::{
    AuthenticatedSnapshotIdentity, NativeSourceEcosystem, NativeSourceStream, Repository,
    RepositoryOwnership, RepositoryPackage, RepositoryPolicyScope, RepositorySourcePolicy,
    RepositoryUpdateMode, SecurityAdvisorySupport,
};
pub use repository_capability::RepositoryProvide;
pub use repository_package_key::{RepositoryPackageKey, RepositoryPackageKeyStatus};
pub use repository_requirement::{RepositoryRequirement, RepositoryRequirementGroup};
pub use resolution::{CacheTier, PackageResolution, PrimaryStrategy, ResolutionStrategy};
pub use state::{RestorePlan, StateDiff, StateEngine, StateMember, SystemState};
pub use subpackage::{RelatedPackages, SubpackageRelationship, show_subpackage_guidance};
pub use system_affinity::SystemAffinity;
pub use trigger::{ChangesetTrigger, Trigger, TriggerDependency, TriggerStatus};
pub use trigger_engine::TriggerEngine;
pub use trove::{InstallReason, InstallSource, Trove, TroveType};
pub use try_session::{CreateTrySession, TrySession, TrySessionMode, TrySessionStatus};

/// Return the connection's authoritative SQLite bind-variable limit as a
/// non-zero Rust batch size.
///
/// Multi-package model queries chunk against the runtime connection limit
/// instead of assuming the compile-time default used by one SQLite build.
pub(super) fn sqlite_variable_batch_size(
    conn: &rusqlite::Connection,
) -> crate::error::Result<usize> {
    let limit = conn.limit(rusqlite::limits::Limit::SQLITE_LIMIT_VARIABLE_NUMBER)?;
    usize::try_from(limit)
        .ok()
        .filter(|limit| *limit > 0)
        .ok_or_else(|| {
            crate::error::Error::InitError(
                "SQLite reported a non-positive bind-variable limit".to_string(),
            )
        })
}

/// Format a byte count as a human-readable size string.
///
/// Delegates to [`crate::util::format_size`].
pub fn format_size(bytes: i64) -> String {
    crate::util::format_size(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::testing::create_test_db;
    use crate::payload::{PayloadContentAuthority, PayloadNode, ResolvedPayloadNode};

    fn regular_file(path: &str, size: u64, mode: u32, trove_id: i64) -> FileEntry {
        let node =
            ResolvedPayloadNode::from_numeric_source(PayloadNode::regular(mode & 0o7777)).unwrap();
        FileEntry::new(
            path.to_string(),
            node,
            Some(PayloadContentAuthority {
                sha256: crate::hash::sha256(b"test payload"),
                size,
            }),
            trove_id,
        )
    }

    #[test]
    fn test_trove_crud() {
        let (_temp, conn) = create_test_db();

        // Create a trove
        let mut trove = Trove::new(
            "test-package".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            crate::repository::versioning::VersionScheme::Conary,
        );
        trove.architecture = Some("x86_64".to_string());
        trove.description = Some("A test package".to_string());

        let id = trove.insert(&conn).unwrap();
        assert!(id > 0);
        assert_eq!(trove.id, Some(id));

        // Find by ID
        let found = Trove::find_by_id(&conn, id).unwrap().unwrap();
        assert_eq!(found.name, "test-package");
        assert_eq!(found.version, "1.0.0");
        assert_eq!(found.trove_type, TroveType::Package);

        // Find by name
        let by_name = Trove::find_by_name(&conn, "test-package").unwrap();
        assert_eq!(by_name.len(), 1);

        // List all
        let all = Trove::list_all(&conn).unwrap();
        assert_eq!(all.len(), 1);

        // Delete
        Trove::delete(&conn, id).unwrap();
        let deleted = Trove::find_by_id(&conn, id).unwrap();
        assert!(deleted.is_none());
    }

    #[test]
    fn test_changeset_crud() {
        let (_temp, conn) = create_test_db();

        // Create a changeset
        let mut changeset = Changeset::new("Install test-package".to_string());
        let id = changeset.insert(&conn).unwrap();
        assert!(id > 0);
        assert_eq!(changeset.status, ChangesetStatus::Pending);

        // Find by ID
        let found = Changeset::find_by_id(&conn, id).unwrap().unwrap();
        assert_eq!(found.description, "Install test-package");
        assert_eq!(found.status, ChangesetStatus::Pending);

        // Update status
        changeset
            .update_status(&conn, ChangesetStatus::Applied)
            .unwrap();
        let updated = Changeset::find_by_id(&conn, id).unwrap().unwrap();
        assert_eq!(updated.status, ChangesetStatus::Applied);
        assert!(updated.applied_at.is_some());

        // List all
        let all = Changeset::list_all(&conn).unwrap();
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn test_file_crud() {
        let (_temp, conn) = create_test_db();

        // Create a trove first (foreign key requirement)
        let mut trove = Trove::new(
            "test-package".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            crate::repository::versioning::VersionScheme::Conary,
        );
        let trove_id = trove.insert(&conn).unwrap();

        // Create a file
        let mut file = regular_file("/usr/bin/test", 1024, 0o755, trove_id);

        let id = file.insert(&conn).unwrap();
        assert!(id > 0);

        // Find by path
        let found = FileEntry::find_by_path(&conn, "/usr/bin/test")
            .unwrap()
            .unwrap();
        assert_eq!(
            found.content.as_ref().unwrap().sha256,
            crate::hash::sha256(b"test payload")
        );
        assert_eq!(found.content.as_ref().unwrap().size, 1024);

        // Find by trove
        let files = FileEntry::find_by_trove(&conn, trove_id).unwrap();
        assert_eq!(files.len(), 1);

        // Delete
        FileEntry::delete(&conn, "/usr/bin/test").unwrap();
        let deleted = FileEntry::find_by_path(&conn, "/usr/bin/test").unwrap();
        assert!(deleted.is_none());
    }

    #[test]
    fn test_cascade_delete() {
        let (_temp, conn) = create_test_db();

        // Create a trove with a file
        let mut trove = Trove::new(
            "test-package".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            crate::repository::versioning::VersionScheme::Conary,
        );
        let trove_id = trove.insert(&conn).unwrap();

        let mut file = regular_file("/usr/bin/test", 1024, 0o755, trove_id);
        file.insert(&conn).unwrap();

        // Delete the trove - file should be cascade deleted
        Trove::delete(&conn, trove_id).unwrap();

        // Verify file is gone
        let file_exists = FileEntry::find_by_path(&conn, "/usr/bin/test").unwrap();
        assert!(file_exists.is_none());
    }

    #[test]
    fn test_flavor_crud() {
        let (_temp, conn) = create_test_db();

        // Create a trove first
        let mut trove = Trove::new(
            "nginx".to_string(),
            "1.21.0".to_string(),
            TroveType::Package,
            crate::repository::versioning::VersionScheme::Conary,
        );
        let trove_id = trove.insert(&conn).unwrap();

        // Create flavors
        let mut flavor1 = Flavor::new(trove_id, "ssl".to_string(), "enabled".to_string());
        let id1 = flavor1.insert(&conn).unwrap();
        assert!(id1 > 0);

        let mut flavor2 = Flavor::new(trove_id, "http3".to_string(), "enabled".to_string());
        flavor2.insert(&conn).unwrap();

        // Find by trove
        let flavors = Flavor::find_by_trove(&conn, trove_id).unwrap();
        assert_eq!(flavors.len(), 2);
        assert_eq!(flavors[0].key, "http3"); // Ordered by key
        assert_eq!(flavors[1].key, "ssl");

        // Find by key
        let ssl_flavors = Flavor::find_by_key(&conn, "ssl").unwrap();
        assert_eq!(ssl_flavors.len(), 1);
        assert_eq!(ssl_flavors[0].value, "enabled");

        // Delete
        Flavor::delete(&conn, id1).unwrap();
        let remaining = Flavor::find_by_trove(&conn, trove_id).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].key, "http3");
    }

    #[test]
    fn test_provenance_crud() {
        let (_temp, conn) = create_test_db();

        // Create a trove first
        let mut trove = Trove::new(
            "nginx".to_string(),
            "1.21.0".to_string(),
            TroveType::Package,
            crate::repository::versioning::VersionScheme::Conary,
        );
        let trove_id = trove.insert(&conn).unwrap();

        // Create provenance
        let mut prov = Provenance::new(trove_id);
        prov.source_url = Some("https://github.com/nginx/nginx".to_string());
        prov.source_branch = Some("main".to_string());
        prov.source_commit = Some("abc123def456".to_string());
        prov.build_host = Some("builder01.example.com".to_string());
        prov.builder = Some("builder-bot".to_string());

        let id = prov.insert(&conn).unwrap();
        assert!(id > 0);

        // Find by trove
        let found = Provenance::find_by_trove(&conn, trove_id).unwrap().unwrap();
        assert_eq!(
            found.source_url,
            Some("https://github.com/nginx/nginx".to_string())
        );
        assert_eq!(found.source_commit, Some("abc123def456".to_string()));
        assert_eq!(found.builder, Some("builder-bot".to_string()));

        // Update
        let mut updated_prov = found.clone();
        updated_prov.source_commit = Some("new_commit_hash".to_string());
        updated_prov.update(&conn).unwrap();

        let reloaded = Provenance::find_by_trove(&conn, trove_id).unwrap().unwrap();
        assert_eq!(reloaded.source_commit, Some("new_commit_hash".to_string()));

        // Delete
        Provenance::delete(&conn, trove_id).unwrap();
        let deleted = Provenance::find_by_trove(&conn, trove_id).unwrap();
        assert!(deleted.is_none());
    }

    #[test]
    fn test_flavor_cascade_delete() {
        let (_temp, conn) = create_test_db();

        // Create a trove with flavors
        let mut trove = Trove::new(
            "test-pkg".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            crate::repository::versioning::VersionScheme::Conary,
        );
        let trove_id = trove.insert(&conn).unwrap();

        let mut flavor = Flavor::new(trove_id, "feature".to_string(), "enabled".to_string());
        flavor.insert(&conn).unwrap();

        // Delete the trove - flavors should be cascade deleted
        Trove::delete(&conn, trove_id).unwrap();

        // Verify flavors are gone
        let flavors = Flavor::find_by_trove(&conn, trove_id).unwrap();
        assert_eq!(flavors.len(), 0);
    }

    #[test]
    fn test_provenance_cascade_delete() {
        let (_temp, conn) = create_test_db();

        // Create a trove with provenance
        let mut trove = Trove::new(
            "test-pkg".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            crate::repository::versioning::VersionScheme::Conary,
        );
        let trove_id = trove.insert(&conn).unwrap();

        let mut prov = Provenance::new(trove_id);
        prov.source_url = Some("https://example.com".to_string());
        prov.insert(&conn).unwrap();

        // Delete the trove - provenance should be cascade deleted
        Trove::delete(&conn, trove_id).unwrap();

        // Verify provenance is gone
        let prov_exists = Provenance::find_by_trove(&conn, trove_id).unwrap();
        assert!(prov_exists.is_none());
    }
}
