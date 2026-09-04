// apps/remi/src/server/readiness.rs

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

use super::catalog_authority::CatalogAuthority;

mod source_profiles;

#[cfg(test)]
use source_profiles::active_profile_is_populated;

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
        source_profiles::probe(
            &inputs.db_path,
            &inputs.catalog_authority,
            &inputs.required_source_profiles,
        ),
        probe_directory(&inputs.chunk_dir, "chunk directory"),
        probe_directory(&inputs.cache_dir, "cache directory"),
        probe_free_space(&inputs.chunk_dir, inputs.min_free_bytes),
    )
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
mod tests;
