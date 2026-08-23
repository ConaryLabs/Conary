// remi/src/server/readiness.rs

//! Evidence-bearing readiness evaluation for Remi serving.
//!
//! Readiness answers one question: can this server actually answer requests?
//! A probe that cannot establish its fact reports that as a failure, never as
//! success.
//!
//! Deploy verification and the operator health script consume
//! `/health/ready`; `/health` remains a separate liveness-only probe.
//!
//! The evaluation here is pure over its inputs so every failure mode is
//! provable in a focused test. The route handler stays thin.

use conary_core::db::schema::{SCHEMA_VERSION, SchemaCompatibility};
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::catalog_authority::CatalogAuthority;

/// Free space a serving root must have before Remi reports ready.
pub const DEFAULT_MIN_FREE_BYTES: u64 = 10 * 1024 * 1024 * 1024;

/// Latest usable outcome for one startup publication phase.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PublicationPhaseState {
    #[default]
    Pending,
    Complete,
    Partial,
    Failed,
    Unavailable,
}

impl PublicationPhaseState {
    fn is_usable(self) -> bool {
        matches!(self, Self::Complete | Self::Partial)
    }
}

/// Typed startup evidence shared by the scheduler and readiness route.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct PublicationReadiness {
    pub repository: PublicationPhaseState,
    pub canonical: PublicationPhaseState,
}

impl PublicationReadiness {
    pub(crate) fn is_ready(&self) -> bool {
        self.repository.is_usable() && self.canonical.is_usable()
    }

    /// Preserve a previously usable publication through a later failed
    /// refresh; a failed candidate must not retire the active state.
    pub(crate) fn record_repository(&mut self, outcome: PublicationPhaseState) {
        if outcome.is_usable() || !self.repository.is_usable() {
            self.repository = outcome;
        }
    }

    /// Preserve a previously usable canonical publication through a later
    /// failed derived cycle for the same reason.
    pub(crate) fn record_canonical(&mut self, outcome: PublicationPhaseState) {
        if outcome.is_usable() || !self.canonical.is_usable() {
            self.canonical = outcome;
        }
    }
}

/// Outcome of a single readiness probe.
///
/// `Unavailable` is deliberately distinct from `NotReady`: the first means the
/// probe could not run and the fact is unknown, the second means the probe ran
/// and the resource is genuinely unfit. They need different operator responses,
/// and collapsing them is what allowed a failed disk probe to read as success.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ProbeOutcome {
    Ready,
    NotReady { reason: String },
    Unavailable { reason: String },
}

impl ProbeOutcome {
    pub fn is_ready(&self) -> bool {
        matches!(self, ProbeOutcome::Ready)
    }

    fn not_ready(reason: impl Into<String>) -> Self {
        ProbeOutcome::NotReady {
            reason: reason.into(),
        }
    }

    fn unavailable(reason: impl Into<String>) -> Self {
        ProbeOutcome::Unavailable {
            reason: reason.into(),
        }
    }
}

/// Inputs the readiness evaluation needs, gathered from server configuration.
#[derive(Clone)]
pub struct ReadinessInputs {
    pub db_path: PathBuf,
    pub chunk_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub min_free_bytes: u64,
    pub required_source_profiles: Vec<String>,
    pub publication: PublicationReadiness,
    pub(crate) catalog_authority: CatalogAuthority,
}

/// Complete readiness report.
///
/// Field names state exactly what each probe established. `database` is not
/// "the file is present"; it is "the database opened, answered a query, and
/// carries the expected schema revision".
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReadinessReport {
    pub ready: bool,
    pub publication: PublicationReadiness,
    pub database: ProbeOutcome,
    pub source_profiles: ProbeOutcome,
    pub chunk_dir: ProbeOutcome,
    pub cache_dir: ProbeOutcome,
    pub free_space: ProbeOutcome,
    pub expected_schema_revision: i32,
}

impl ReadinessReport {
    fn from_probes(
        publication: PublicationReadiness,
        database: ProbeOutcome,
        source_profiles: ProbeOutcome,
        chunk_dir: ProbeOutcome,
        cache_dir: ProbeOutcome,
        free_space: ProbeOutcome,
    ) -> Self {
        let ready = publication.is_ready()
            && database.is_ready()
            && source_profiles.is_ready()
            && chunk_dir.is_ready()
            && cache_dir.is_ready()
            && free_space.is_ready();
        Self {
            ready,
            publication,
            database,
            source_profiles,
            chunk_dir,
            cache_dir,
            free_space,
            expected_schema_revision: SCHEMA_VERSION,
        }
    }
}

/// Evaluate readiness. Blocking: callers must run this off the async runtime.
pub fn evaluate(inputs: &ReadinessInputs) -> ReadinessReport {
    ReadinessReport::from_probes(
        inputs.publication.clone(),
        probe_database(&inputs.db_path),
        probe_source_profiles(
            &inputs.db_path,
            &inputs.catalog_authority,
            &inputs.required_source_profiles,
        ),
        probe_directory(&inputs.chunk_dir, "chunk directory"),
        probe_directory(&inputs.cache_dir, "cache directory"),
        probe_free_space(&inputs.chunk_dir, inputs.min_free_bytes),
    )
}

/// Require at least one package in every enabled profile's active catalog.
///
/// Operational SQLite owns which exact profiles are enabled. The verified,
/// pinned immutable catalog alone owns whether an activated profile contains
/// packages; mutable package projections are deliberately irrelevant here.
fn probe_source_profiles(
    db_path: &Path,
    catalog_authority: &CatalogAuthority,
    required_profiles: &[String],
) -> ProbeOutcome {
    let flags =
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = match rusqlite::Connection::open_with_flags(db_path, flags) {
        Ok(conn) => conn,
        Err(error) => {
            return ProbeOutcome::unavailable(format!(
                "could not inspect source-profile population in {}: {error}",
                db_path.display()
            ));
        }
    };

    let mut statement = match conn.prepare(
        "SELECT DISTINCT source_profile
         FROM repositories
         WHERE enabled = 1 AND source_profile IS NOT NULL
         ORDER BY source_profile",
    ) {
        Ok(statement) => statement,
        Err(error) => {
            return ProbeOutcome::unavailable(format!(
                "could not prepare source-profile population query in {}: {error}",
                db_path.display()
            ));
        }
    };
    let configured = match statement.query_map([], |row| row.get::<_, String>(0)) {
        Ok(rows) => match rows.collect::<Result<Vec<_>, _>>() {
            Ok(configured) => configured.into_iter().collect::<BTreeSet<_>>(),
            Err(error) => {
                return ProbeOutcome::unavailable(format!(
                    "could not read source-profile population in {}: {error}",
                    db_path.display()
                ));
            }
        },
        Err(error) => {
            return ProbeOutcome::unavailable(format!(
                "could not query source-profile population in {}: {error}",
                db_path.display()
            ));
        }
    };

    if configured.is_empty() {
        return ProbeOutcome::not_ready("no enabled exact source profiles are configured");
    }

    let required = if required_profiles.is_empty() {
        configured.clone()
    } else {
        required_profiles.iter().cloned().collect::<BTreeSet<_>>()
    };
    let missing_configuration = required
        .difference(&configured)
        .map(String::as_str)
        .collect::<Vec<_>>();
    if !missing_configuration.is_empty() {
        return ProbeOutcome::not_ready(format!(
            "required source profiles are not enabled: {}",
            missing_configuration.join(", ")
        ));
    }

    let mut missing = Vec::new();
    for profile in required {
        match active_profile_is_populated(catalog_authority, &profile) {
            Ok(true) => {}
            Ok(false) => missing.push(profile),
            Err(error) => {
                return ProbeOutcome::unavailable(format!(
                    "could not verify active immutable catalog for source profile '{profile}': {error}"
                ));
            }
        }
    }

    if missing.is_empty() {
        ProbeOutcome::Ready
    } else {
        ProbeOutcome::not_ready(format!(
            "enabled source profiles have no packages in their active immutable catalogs: {}",
            missing.join(", ")
        ))
    }
}

fn active_profile_is_populated(
    catalog_authority: &CatalogAuthority,
    profile: &str,
) -> anyhow::Result<bool> {
    let inspection = catalog_authority.inspect_active_profile(profile)?;
    Ok(inspection.manifest.counts.packages > 0)
}

/// Check one exact public profile without deriving authority from its route.
pub(crate) fn source_profile_is_populated(
    catalog_authority: &CatalogAuthority,
    profile: &str,
) -> Result<bool, String> {
    active_profile_is_populated(catalog_authority, profile).map_err(|error| format!("{error:#}"))
}

/// Open the database read-only, run a query, and require the exact schema epoch.
///
/// A missing database reports `Fresh`, which is not ready: a server with no
/// database cannot serve, regardless of whether its parent directory exists.
fn probe_database(db_path: &Path) -> ProbeOutcome {
    match conary_core::db::schema::inspect(db_path) {
        Ok(SchemaCompatibility::Current) => ProbeOutcome::Ready,
        Ok(SchemaCompatibility::Fresh) => {
            ProbeOutcome::not_ready(format!("no initialized database at {}", db_path.display()))
        }
        Ok(SchemaCompatibility::RebuildRequired { observed }) => ProbeOutcome::not_ready(format!(
            "database requires rebuild: expected epoch revision {SCHEMA_VERSION}, observed {observed}"
        )),
        Err(error) => {
            ProbeOutcome::unavailable(format!("could not inspect {}: {error}", db_path.display()))
        }
    }
}

fn probe_directory(path: &Path, label: &str) -> ProbeOutcome {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => ProbeOutcome::Ready,
        Ok(_) => ProbeOutcome::not_ready(format!("{label} {} is not a directory", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            ProbeOutcome::not_ready(format!("{label} {} does not exist", path.display()))
        }
        Err(error) => ProbeOutcome::unavailable(format!(
            "could not stat {label} {}: {error}",
            path.display()
        )),
    }
}

/// Report available free space beneath `path`.
///
/// A probe that cannot execute reports `Unavailable`. Returning success on
/// `statvfs` failure is what let a broken disk probe pass readiness.
fn probe_free_space(path: &Path, min_free_bytes: u64) -> ProbeOutcome {
    match available_bytes(path) {
        Ok(free) if free >= min_free_bytes => ProbeOutcome::Ready,
        Ok(free) => ProbeOutcome::not_ready(format!(
            "{} has {free} bytes free, below the required {min_free_bytes}",
            path.display()
        )),
        Err(error) => ProbeOutcome::unavailable(format!(
            "could not measure free space at {}: {error}",
            path.display()
        )),
    }
}

#[cfg(unix)]
fn available_bytes(path: &Path) -> Result<u64, String> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let path_cstr = CString::new(path.as_os_str().as_bytes())
        .map_err(|error| format!("path is not representable for statvfs: {error}"))?;

    // SAFETY: `stat` is zeroed and only read after statvfs reports success;
    // `path_cstr` outlives the call and is NUL-terminated by construction.
    let stat = unsafe {
        let mut stat: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(path_cstr.as_ptr(), &mut stat) != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        stat
    };

    #[allow(clippy::unnecessary_cast)]
    let available = (stat.f_bavail as u64).checked_mul(stat.f_bsize as u64);
    available.ok_or_else(|| "free-space calculation overflowed".to_string())
}

#[cfg(not(unix))]
fn available_bytes(_path: &Path) -> Result<u64, String> {
    Err("free-space measurement is not implemented on this platform".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db::schema;

    fn inputs_for(dir: &Path) -> ReadinessInputs {
        let db_path = dir.join("metadata/conary.db");
        std::fs::create_dir_all(db_path.parent().expect("db parent")).expect("create metadata dir");
        let chunk_dir = dir.join("chunks");
        let cache_dir = dir.join("cache");
        let catalog_dir = dir.join("catalogs");
        std::fs::create_dir_all(&chunk_dir).expect("create chunk dir");
        std::fs::create_dir_all(&cache_dir).expect("create cache dir");
        std::fs::create_dir_all(&catalog_dir).expect("create catalog dir");
        let database_writer = crate::server::database_writer::DatabaseWriter::default();
        let catalog_authority =
            CatalogAuthority::from_paths(db_path.clone(), catalog_dir, database_writer);
        ReadinessInputs {
            db_path,
            chunk_dir,
            cache_dir,
            min_free_bytes: 0,
            required_source_profiles: Vec::new(),
            publication: PublicationReadiness {
                repository: PublicationPhaseState::Complete,
                canonical: PublicationPhaseState::Complete,
            },
            catalog_authority,
        }
    }

    fn initialize_database(db_path: &Path) {
        let conn = rusqlite::Connection::open(db_path).expect("open database");
        schema::ensure_current(&conn).expect("initialize current schema");
        conary_core::db::models::RemiRuntimeSession::begin(&conn, 1)
            .expect("install readiness runtime session");
    }

    fn configure_profile(db_path: &Path, profile: &str) -> i64 {
        use conary_core::db::models::Repository;

        let conn = conary_core::db::open_fast(db_path).expect("open database");
        let mut repository = Repository::new(
            format!("{profile}-readiness"),
            format!("https://example.invalid/{profile}"),
        );
        repository.source_profile = Some(profile.to_string());
        repository.insert(&conn).expect("insert repository")
    }

    fn insert_operational_package(db_path: &Path, repository_id: i64, profile: &str) {
        use conary_core::db::models::RepositoryPackage;
        use conary_core::repository::versioning::VersionScheme;

        let conn = conary_core::db::open_fast(db_path).expect("open database");
        let mut package = RepositoryPackage::new(
            repository_id,
            "stale-operational-package".to_string(),
            "9.9-1".to_string(),
            VersionScheme::Rpm,
            conary_core::hash::sha256(b"stale-operational-package"),
            1,
            "https://example.invalid/stale.rpm".to_string(),
        );
        package.source_profile = Some(profile.to_string());
        package
            .insert(&conn)
            .expect("insert stale operational package");
    }

    fn activate_profile_catalog(inputs: &ReadinessInputs, profile: &str, populated: bool) {
        use conary_core::db::models::{
            RemiCatalogResource, RemiCatalogResourceKind, RemiProfileRevisionMember,
        };
        use conary_core::repository::catalog::{
            CATALOG_FILE_NAME, CatalogContentV1, CatalogPackageOriginV1, CatalogPackageRecordV1,
            CatalogScopeV1, CatalogSourceEvidenceV1, PROFILE_REVISION_SCHEMA_V1, ProfileRevisionV1,
            ProfileSourceMemberV1, SourceStreamKindV1, SourceStreamV1,
            publish_profile_catalog_bundle, write_catalog_candidate,
            write_profile_catalog_manifest,
        };
        use conary_core::repository::versioning::VersionScheme;

        let source_identity = format!("source-{profile}");
        let repository_identity = format!("repository-{profile}");
        let source_manifest_json = String::from_utf8(
            conary_core::json::canonical_json(&serde_json::json!({
                "fixture": "readiness-source-snapshot",
                "profile": profile,
            }))
            .expect("serialize readiness source resource"),
        )
        .expect("source manifest JSON is UTF-8");
        let source_snapshot_sha256 = conary_core::hash::sha256(source_manifest_json.as_bytes());
        let origin = CatalogPackageOriginV1::Profile {
            member_ordinal: 0,
            source_identity: source_identity.clone(),
            repository_identity: repository_identity.clone(),
            source_snapshot_sha256: source_snapshot_sha256.clone(),
        };
        let packages = populated
            .then(|| CatalogPackageRecordV1 {
                package_key_sha256: String::new(),
                origin,
                source_profile: profile.to_string(),
                name: "catalog-readiness-probe".to_string(),
                version: "1.0-1".to_string(),
                package_release: "1".to_string(),
                architecture: Some("x86_64".to_string()),
                debian_multi_arch: None,
                description: None,
                checksum: conary_core::hash::sha256(b"catalog-readiness-probe"),
                size: 1,
                download_url: format!("https://example.invalid/{profile}/package.rpm"),
                metadata: None,
                is_security_update: false,
                severity: None,
                cve_ids: None,
                advisory_id: None,
                advisory_url: None,
                version_scheme: VersionScheme::Rpm,
                provides: Vec::new(),
                requirement_groups: Vec::new(),
            })
            .into_iter()
            .collect();
        let content = CatalogContentV1::new(
            CatalogScopeV1::Profile {
                profile: profile.to_string(),
            },
            vec![CatalogSourceEvidenceV1::SourceSnapshot {
                member_ordinal: 0,
                source_identity: source_identity.clone(),
                repository_identity: repository_identity.clone(),
                source_snapshot_sha256: source_snapshot_sha256.clone(),
            }],
            packages,
        )
        .expect("build readiness profile catalog");
        let root = inputs
            .db_path
            .parent()
            .and_then(Path::parent)
            .expect("fixture storage root");
        let candidate_dir = root.join(format!("candidate-{profile}"));
        std::fs::create_dir_all(&candidate_dir).expect("create catalog candidate");
        let binding = write_catalog_candidate(candidate_dir.join(CATALOG_FILE_NAME), &content)
            .expect("write readiness catalog candidate");
        let manifest = ProfileRevisionV1 {
            schema_version: PROFILE_REVISION_SCHEMA_V1,
            profile: profile.to_string(),
            projection_version: 1,
            members: vec![ProfileSourceMemberV1 {
                ordinal: 0,
                source_identity,
                repository_identity,
                stream: SourceStreamV1 {
                    kind: SourceStreamKindV1::Release,
                    identity: "stable".to_string(),
                },
                priority: 0,
                required: true,
                source_snapshot_sha256: source_snapshot_sha256.clone(),
            }],
            catalog: binding.artifact.clone(),
            logical_digest_sha256: binding.logical_digest_sha256.clone(),
            counts: binding.counts,
        };
        write_profile_catalog_manifest(&candidate_dir, &manifest)
            .expect("write readiness profile manifest");
        publish_profile_catalog_bundle(&candidate_dir, root.join("catalogs"), &manifest)
            .expect("publish readiness profile catalog");
        let digest = manifest.manifest_sha256().expect("hash readiness revision");
        let manifest_json = String::from_utf8(
            conary_core::json::canonical_json(&manifest).expect("serialize readiness revision"),
        )
        .expect("manifest JSON is UTF-8");
        let conn = conary_core::db::open_fast(&inputs.db_path).expect("open readiness database");
        RemiCatalogResource {
            resource_sha256: source_snapshot_sha256.clone(),
            kind: RemiCatalogResourceKind::SourceSnapshot,
            source_profile: profile.to_string(),
            artifact_sha256: conary_core::hash::sha256(
                format!("readiness-source-artifact-{profile}").as_bytes(),
            ),
            artifact_size: 1,
            logical_digest_sha256: conary_core::hash::sha256(
                format!("readiness-source-logical-{profile}").as_bytes(),
            ),
            manifest_json: source_manifest_json,
            durable: true,
            created_at: 1,
        }
        .insert(&conn)
        .expect("insert readiness source resource");
        RemiCatalogResource {
            resource_sha256: digest.clone(),
            kind: RemiCatalogResourceKind::ProfileRevision,
            source_profile: profile.to_string(),
            artifact_sha256: manifest.catalog.sha256.clone(),
            artifact_size: i64::try_from(manifest.catalog.size).expect("artifact size fits"),
            logical_digest_sha256: manifest.logical_digest_sha256.clone(),
            manifest_json,
            durable: true,
            created_at: 1,
        }
        .insert(&conn)
        .expect("insert readiness profile resource");
        RemiProfileRevisionMember {
            profile_revision_sha256: digest.clone(),
            ordinal: 0,
            source_snapshot_sha256,
            source_identity: manifest.members[0].source_identity.clone(),
            repository_identity: manifest.members[0].repository_identity.clone(),
            stream_kind: "release".to_string(),
            stream_identity: "stable".to_string(),
            priority: 0,
            required: true,
        }
        .insert(&conn)
        .expect("insert readiness profile member");
        let run_id = uuid::Uuid::new_v4().to_string();
        let owner_instance_uuid = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO repository_sync_runs (
                 run_id, source_profile, owner_instance_uuid, fencing_epoch,
                 input_profile_digest, candidate_profile_digest, state,
                 started_at, heartbeat_at, lease_expires_at, finished_at
             ) VALUES (?1, ?2, ?3, 1, NULL, ?4, 'published', 1, 1, 1, 1)",
            rusqlite::params![run_id, profile, owner_instance_uuid, digest],
        )
        .expect("insert readiness activation run");
        conn.execute(
            "INSERT INTO remi_active_profile_revisions (
                 source_profile, profile_revision_sha256, fencing_epoch,
                 activation_run_id, owner_instance_uuid, activated_at
             ) VALUES (?1, ?2, 1, ?3, ?4, 1)",
            rusqlite::params![profile, digest, run_id, owner_instance_uuid,],
        )
        .expect("activate readiness profile catalog");
    }

    #[test]
    fn ready_when_database_directories_and_space_all_satisfy_their_probes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let inputs = inputs_for(dir.path());
        initialize_database(&inputs.db_path);
        configure_profile(&inputs.db_path, "fedora-44");
        activate_profile_catalog(&inputs, "fedora-44", true);

        let report = evaluate(&inputs);

        assert!(report.ready, "expected ready, got {report:?}");
        assert_eq!(report.database, ProbeOutcome::Ready);
        assert_eq!(report.expected_schema_revision, SCHEMA_VERSION);
    }

    #[test]
    fn readiness_does_not_claim_database_write_authority() {
        let dir = tempfile::tempdir().expect("tempdir");
        let inputs = inputs_for(dir.path());
        initialize_database(&inputs.db_path);
        configure_profile(&inputs.db_path, "fedora-44");
        activate_profile_catalog(&inputs, "fedora-44", true);
        let database_writer = inputs.catalog_authority.database_writer_for_test();
        let writer_guard = database_writer.hold_for_test();
        let (sender, receiver) = std::sync::mpsc::channel();
        let probe = std::thread::spawn(move || {
            sender.send(evaluate(&inputs)).expect("send report");
        });
        let prompt_report = receiver.recv_timeout(std::time::Duration::from_secs(1));
        drop(writer_guard);
        probe.join().expect("join readiness probe");
        let report = prompt_report.expect("readiness must not wait for the process SQLite writer");
        assert!(report.ready, "expected ready, got {report:?}");
    }

    #[test]
    fn not_ready_when_no_exact_source_profile_is_configured() {
        let dir = tempfile::tempdir().expect("tempdir");
        let inputs = inputs_for(dir.path());
        initialize_database(&inputs.db_path);

        let report = evaluate(&inputs);

        assert!(!report.ready);
        assert!(matches!(
            report.source_profiles,
            ProbeOutcome::NotReady { .. }
        ));
    }

    #[test]
    fn not_ready_while_initial_publication_is_pending() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut inputs = inputs_for(dir.path());
        initialize_database(&inputs.db_path);
        configure_profile(&inputs.db_path, "fedora-44");
        activate_profile_catalog(&inputs, "fedora-44", true);
        inputs.publication.canonical = PublicationPhaseState::Pending;

        let report = evaluate(&inputs);

        assert!(!report.ready);
        assert_eq!(report.publication.canonical, PublicationPhaseState::Pending);
    }

    #[test]
    fn failed_candidate_does_not_retire_usable_publication() {
        let mut publication = PublicationReadiness::default();
        publication.record_repository(PublicationPhaseState::Complete);
        publication.record_canonical(PublicationPhaseState::Partial);

        publication.record_repository(PublicationPhaseState::Failed);
        publication.record_canonical(PublicationPhaseState::Unavailable);

        assert_eq!(publication.repository, PublicationPhaseState::Complete);
        assert_eq!(publication.canonical, PublicationPhaseState::Partial);
        assert!(publication.is_ready());
    }

    #[test]
    fn not_ready_when_a_required_profile_has_zero_packages() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut inputs = inputs_for(dir.path());
        inputs.required_source_profiles = vec!["fedora-44".to_string()];
        initialize_database(&inputs.db_path);
        let repository_id = configure_profile(&inputs.db_path, "fedora-44");
        insert_operational_package(&inputs.db_path, repository_id, "fedora-44");
        activate_profile_catalog(&inputs, "fedora-44", false);

        let report = evaluate(&inputs);

        assert!(!report.ready);
        assert!(matches!(
            report.source_profiles,
            ProbeOutcome::NotReady { .. }
        ));
    }

    #[test]
    fn missing_active_catalog_is_unavailable_without_operational_fallback() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut inputs = inputs_for(dir.path());
        inputs.required_source_profiles = vec!["fedora-44".to_string()];
        initialize_database(&inputs.db_path);
        let repository_id = configure_profile(&inputs.db_path, "fedora-44");
        insert_operational_package(&inputs.db_path, repository_id, "fedora-44");

        let report = evaluate(&inputs);

        assert!(!report.ready);
        match report.source_profiles {
            ProbeOutcome::Unavailable { reason } => assert!(
                reason.contains("has no active immutable catalog revision"),
                "unexpected reason: {reason}"
            ),
            other => panic!("expected unavailable catalog authority, got {other:?}"),
        }
    }

    #[test]
    fn not_ready_when_only_some_required_profiles_are_populated() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut inputs = inputs_for(dir.path());
        inputs.required_source_profiles = vec!["fedora-44".to_string(), "ubuntu-26.04".to_string()];
        initialize_database(&inputs.db_path);
        configure_profile(&inputs.db_path, "fedora-44");
        activate_profile_catalog(&inputs, "fedora-44", true);
        configure_profile(&inputs.db_path, "ubuntu-26.04");
        activate_profile_catalog(&inputs, "ubuntu-26.04", false);

        let report = evaluate(&inputs);

        assert!(!report.ready);
        match report.source_profiles {
            ProbeOutcome::NotReady { reason } => {
                assert!(
                    reason.contains("ubuntu-26.04"),
                    "unexpected reason: {reason}"
                );
                assert!(!reason.contains("fedora-44"), "unexpected reason: {reason}");
            }
            other => panic!("expected NotReady, got {other:?}"),
        }
    }

    #[test]
    fn ready_when_every_required_profile_is_populated() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut inputs = inputs_for(dir.path());
        inputs.required_source_profiles = vec!["fedora-44".to_string(), "ubuntu-26.04".to_string()];
        initialize_database(&inputs.db_path);
        configure_profile(&inputs.db_path, "fedora-44");
        activate_profile_catalog(&inputs, "fedora-44", true);
        configure_profile(&inputs.db_path, "ubuntu-26.04");
        activate_profile_catalog(&inputs, "ubuntu-26.04", true);

        let report = evaluate(&inputs);

        assert!(report.ready, "expected ready, got {report:?}");
        assert_eq!(report.source_profiles, ProbeOutcome::Ready);
    }

    /// The exact defect this module replaces: the previous check accepted a
    /// missing database whenever its parent directory existed, which on any
    /// normal deployment is always true.
    #[test]
    fn not_ready_when_database_is_absent_but_its_parent_directory_exists() {
        let dir = tempfile::tempdir().expect("tempdir");
        let inputs = inputs_for(dir.path());

        assert!(
            inputs.db_path.parent().expect("db parent").is_dir(),
            "the parent directory must exist for this regression to be meaningful"
        );
        assert!(!inputs.db_path.exists(), "the database must be absent");

        let report = evaluate(&inputs);

        assert!(!report.ready, "absent database must not report ready");
        assert!(
            matches!(report.database, ProbeOutcome::NotReady { .. }),
            "expected NotReady, got {:?}",
            report.database
        );
    }

    #[test]
    fn not_ready_when_database_carries_a_retired_schema_revision() {
        let dir = tempfile::tempdir().expect("tempdir");
        let inputs = inputs_for(dir.path());

        let conn = rusqlite::Connection::open(&inputs.db_path).expect("open database");
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER NOT NULL);
             INSERT INTO schema_version (version) VALUES (3);
             CREATE TABLE converted_packages (id INTEGER PRIMARY KEY);",
        )
        .expect("write retired schema");
        drop(conn);

        let report = evaluate(&inputs);

        assert!(!report.ready, "retired schema must not report ready");
        match report.database {
            ProbeOutcome::NotReady { ref reason } => {
                assert!(
                    reason.contains("rebuild"),
                    "reason should name the rebuild requirement, got {reason}"
                );
            }
            ref other => panic!("expected NotReady, got {other:?}"),
        }
    }

    #[test]
    fn database_probe_is_unavailable_when_the_file_cannot_be_opened() {
        let dir = tempfile::tempdir().expect("tempdir");
        let inputs = inputs_for(dir.path());
        std::fs::write(&inputs.db_path, b"this is not a sqlite database")
            .expect("write junk database");

        let report = evaluate(&inputs);

        assert!(!report.ready, "unreadable database must not report ready");
        assert!(
            matches!(
                report.database,
                ProbeOutcome::NotReady { .. } | ProbeOutcome::Unavailable { .. }
            ),
            "expected a failing outcome, got {:?}",
            report.database
        );
    }

    #[test]
    fn not_ready_when_the_chunk_directory_is_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let inputs = inputs_for(dir.path());
        initialize_database(&inputs.db_path);
        std::fs::remove_dir(&inputs.chunk_dir).expect("remove chunk dir");

        let report = evaluate(&inputs);

        assert!(!report.ready);
        assert!(matches!(report.chunk_dir, ProbeOutcome::NotReady { .. }));
    }

    #[test]
    fn not_ready_when_the_cache_path_is_a_file_rather_than_a_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let inputs = inputs_for(dir.path());
        initialize_database(&inputs.db_path);
        std::fs::remove_dir(&inputs.cache_dir).expect("remove cache dir");
        std::fs::write(&inputs.cache_dir, b"not a directory").expect("write file at cache path");

        let report = evaluate(&inputs);

        assert!(!report.ready);
        assert!(matches!(report.cache_dir, ProbeOutcome::NotReady { .. }));
    }

    /// Insufficient space is a genuine NotReady, distinct from a probe that
    /// could not run at all.
    #[test]
    fn not_ready_when_free_space_is_below_the_configured_threshold() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut inputs = inputs_for(dir.path());
        initialize_database(&inputs.db_path);
        inputs.min_free_bytes = u64::MAX;

        let report = evaluate(&inputs);

        assert!(!report.ready, "insufficient space must not report ready");
        match report.free_space {
            ProbeOutcome::NotReady { ref reason } => {
                assert!(
                    reason.contains("below the required"),
                    "reason should state the shortfall, got {reason}"
                );
            }
            ref other => panic!("expected NotReady, got {other:?}"),
        }
    }

    /// A probe that cannot execute must not read as success. The previous
    /// implementation returned `true` on statvfs failure.
    #[test]
    fn free_space_probe_is_unavailable_when_the_path_cannot_be_measured() {
        let missing = Path::new("/nonexistent-remi-readiness-probe-target");

        let outcome = probe_free_space(missing, 0);

        assert!(
            matches!(outcome, ProbeOutcome::Unavailable { .. }),
            "a failed probe must be Unavailable, got {outcome:?}"
        );
        assert!(!outcome.is_ready(), "a failed probe must never be ready");
    }

    #[test]
    fn report_serializes_each_probe_state_distinctly() {
        let dir = tempfile::tempdir().expect("tempdir");
        let inputs = inputs_for(dir.path());

        let json = serde_json::to_value(evaluate(&inputs)).expect("serialize report");

        assert_eq!(json["ready"], serde_json::json!(false));
        assert_eq!(json["database"]["state"], serde_json::json!("not_ready"));
        assert_eq!(json["chunk_dir"]["state"], serde_json::json!("ready"));
        assert!(
            json["database"]["reason"].is_string(),
            "a failing probe must carry a reason"
        );
    }
}
