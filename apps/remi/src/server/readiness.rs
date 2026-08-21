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
use std::path::{Path, PathBuf};

/// Free space a serving root must have before Remi reports ready.
pub const DEFAULT_MIN_FREE_BYTES: u64 = 10 * 1024 * 1024 * 1024;

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
#[derive(Debug, Clone)]
pub struct ReadinessInputs {
    pub db_path: PathBuf,
    pub chunk_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub min_free_bytes: u64,
}

/// Complete readiness report.
///
/// Field names state exactly what each probe established. `database` is not
/// "the file is present"; it is "the database opened, answered a query, and
/// carries the expected schema revision".
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReadinessReport {
    pub ready: bool,
    pub database: ProbeOutcome,
    pub source_profiles: ProbeOutcome,
    pub chunk_dir: ProbeOutcome,
    pub cache_dir: ProbeOutcome,
    pub free_space: ProbeOutcome,
    pub expected_schema_revision: i32,
}

impl ReadinessReport {
    fn from_probes(
        database: ProbeOutcome,
        source_profiles: ProbeOutcome,
        chunk_dir: ProbeOutcome,
        cache_dir: ProbeOutcome,
        free_space: ProbeOutcome,
    ) -> Self {
        let ready = database.is_ready()
            && source_profiles.is_ready()
            && chunk_dir.is_ready()
            && cache_dir.is_ready()
            && free_space.is_ready();
        Self {
            ready,
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
        probe_database(&inputs.db_path),
        probe_source_profiles(&inputs.db_path),
        probe_directory(&inputs.chunk_dir, "chunk directory"),
        probe_directory(&inputs.cache_dir, "cache directory"),
        probe_free_space(&inputs.chunk_dir, inputs.min_free_bytes),
    )
}

/// Require at least one persisted package for every enabled public profile.
///
/// Both the repository and package row must carry the same exact profile ID.
/// Names, URLs, and distro-family aliases are deliberately irrelevant here.
fn probe_source_profiles(db_path: &Path) -> ProbeOutcome {
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
        "SELECT r.source_profile
         FROM repositories r
         LEFT JOIN repository_packages rp
           ON rp.repository_id = r.id AND rp.source_profile = r.source_profile
         WHERE r.enabled = 1 AND r.source_profile IS NOT NULL
         GROUP BY r.source_profile
         HAVING COUNT(rp.id) = 0
         ORDER BY r.source_profile",
    ) {
        Ok(statement) => statement,
        Err(error) => {
            return ProbeOutcome::unavailable(format!(
                "could not prepare source-profile population query in {}: {error}",
                db_path.display()
            ));
        }
    };
    let missing = match statement.query_map([], |row| row.get::<_, String>(0)) {
        Ok(rows) => match rows.collect::<Result<Vec<_>, _>>() {
            Ok(missing) => missing,
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

    if missing.is_empty() {
        ProbeOutcome::Ready
    } else {
        ProbeOutcome::not_ready(format!(
            "enabled source profiles have no persisted packages: {}",
            missing.join(", ")
        ))
    }
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
        std::fs::create_dir_all(&chunk_dir).expect("create chunk dir");
        std::fs::create_dir_all(&cache_dir).expect("create cache dir");
        ReadinessInputs {
            db_path,
            chunk_dir,
            cache_dir,
            min_free_bytes: 0,
        }
    }

    fn initialize_database(db_path: &Path) {
        let conn = rusqlite::Connection::open(db_path).expect("open database");
        schema::ensure_current(&conn).expect("initialize current schema");
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

    fn mark_profile_populated(db_path: &Path, profile: &str) {
        use conary_core::db::models::RepositoryPackage;
        use conary_core::repository::versioning::VersionScheme;

        let repository_id = configure_profile(db_path, profile);
        let conn = conary_core::db::open_fast(db_path).expect("open database");
        let mut package = RepositoryPackage::new(
            repository_id,
            "readiness-probe".to_string(),
            "1.0.0".to_string(),
            VersionScheme::Conary,
            format!("sha256:{profile}"),
            1,
            format!("https://example.invalid/{profile}/package.ccs"),
        );
        package.source_profile = Some(profile.to_string());
        package.insert(&conn).expect("insert repository package");
    }

    #[test]
    fn ready_when_database_directories_and_space_all_satisfy_their_probes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let inputs = inputs_for(dir.path());
        initialize_database(&inputs.db_path);

        let report = evaluate(&inputs);

        assert!(report.ready, "expected ready, got {report:?}");
        assert_eq!(report.database, ProbeOutcome::Ready);
        assert_eq!(report.expected_schema_revision, SCHEMA_VERSION);
    }

    #[test]
    fn not_ready_when_a_required_profile_has_zero_packages() {
        let dir = tempfile::tempdir().expect("tempdir");
        let inputs = inputs_for(dir.path());
        initialize_database(&inputs.db_path);
        configure_profile(&inputs.db_path, "fedora-44");

        let report = evaluate(&inputs);

        assert!(!report.ready);
        assert!(matches!(
            report.source_profiles,
            ProbeOutcome::NotReady { .. }
        ));
    }

    #[test]
    fn not_ready_when_only_some_required_profiles_are_populated() {
        let dir = tempfile::tempdir().expect("tempdir");
        let inputs = inputs_for(dir.path());
        initialize_database(&inputs.db_path);
        mark_profile_populated(&inputs.db_path, "fedora-44");
        configure_profile(&inputs.db_path, "ubuntu-26.04");

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
        let inputs = inputs_for(dir.path());
        initialize_database(&inputs.db_path);
        mark_profile_populated(&inputs.db_path, "fedora-44");
        mark_profile_populated(&inputs.db_path, "ubuntu-26.04");

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
