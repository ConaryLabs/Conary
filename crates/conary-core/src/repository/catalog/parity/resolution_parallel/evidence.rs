// crates/conary-core/src/repository/catalog/parity/resolution_parallel/evidence.rs

//! Resolution-walk evidence publication and worker explanation allowances.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use super::ResolutionWorkerCount;
use crate::error::{Error, Result};

/// Non-authoritative execution evidence emitted separately from canonical
/// resolution bundles and surveys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolutionWalkImplementationEvidenceV1 {
    pub schema_version: u32,
    pub workers: u64,
    pub worker_load_milliseconds: Vec<u64>,
    pub memory_budget_bytes: u64,
    pub measured_worker_rss_bytes: u64,
}

impl ResolutionWalkImplementationEvidenceV1 {
    pub(crate) fn new(
        workers: ResolutionWorkerCount,
        worker_load_milliseconds: Vec<u64>,
        memory_budget_bytes: u64,
        measured_worker_rss_bytes: u64,
    ) -> Result<Self> {
        if worker_load_milliseconds.len() != workers.get() {
            return Err(Error::InternalError(
                "resolution worker load evidence count drifted".to_string(),
            ));
        }
        Ok(Self {
            schema_version: 1,
            workers: workers.get() as u64,
            worker_load_milliseconds,
            memory_budget_bytes,
            measured_worker_rss_bytes,
        })
    }
}

/// Write non-authoritative worker evidence without altering canonical artifacts.
pub fn write_resolution_walk_implementation_evidence(
    path: &Path,
    evidence: &ResolutionWalkImplementationEvidenceV1,
) -> Result<()> {
    let bytes = crate::json::canonical_json(evidence).map_err(|error| {
        Error::ParseError(format!("serialize resolution worker evidence: {error}"))
    })?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(())
}

/// Reject an implementation-evidence destination inside a strict bundle.
///
/// The comparison resolves existing ancestors before checking containment, so
/// relative paths, parent components, and symlinked parents cannot alias the
/// bundle while evading the exact-layout boundary.
pub fn ensure_resolution_walk_evidence_outside_bundle(
    bundle: &Path,
    evidence: &Path,
) -> Result<()> {
    let bundle = resolved_destination(bundle)?;
    let evidence = resolved_destination(evidence)?;
    if evidence.starts_with(&bundle) {
        return Err(Error::ConfigError(format!(
            "resolution implementation evidence {} must remain outside strict bundle {}",
            evidence.display(),
            bundle.display()
        )));
    }
    Ok(())
}

fn resolved_destination(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut resolved = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            Component::Prefix(_) | Component::RootDir => {
                resolved.push(component.as_os_str());
            }
            Component::Normal(name) => {
                let candidate = resolved.join(name);
                match fs::symlink_metadata(&candidate) {
                    Ok(_) => resolved = fs::canonicalize(&candidate)?,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        resolved.push(name);
                    }
                    Err(error) => return Err(error.into()),
                }
            }
        }
    }
    Ok(resolved)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResolutionExplanationLimits {
    diagnostic_outcome_bytes: u64,
    failure_bytes: u64,
}

impl ResolutionExplanationLimits {
    pub(crate) const fn new(diagnostic_outcome_bytes: u64, failure_bytes: u64) -> Self {
        Self {
            diagnostic_outcome_bytes,
            failure_bytes,
        }
    }

    pub(crate) const fn none() -> Self {
        Self::new(0, 0)
    }

    #[allow(dead_code)] // Consumed by feature-gated native producer binaries.
    pub(crate) const fn diagnostic_outcome_bytes(self) -> u64 {
        self.diagnostic_outcome_bytes
    }

    pub(crate) const fn failure_bytes(self) -> u64 {
        self.failure_bytes
    }
}

pub(super) struct AtomicResolutionExplanationLimits {
    diagnostic_outcome_bytes: AtomicU64,
    failure_bytes: AtomicU64,
}

impl AtomicResolutionExplanationLimits {
    pub(super) fn new(limits: ResolutionExplanationLimits) -> Self {
        Self {
            diagnostic_outcome_bytes: AtomicU64::new(limits.diagnostic_outcome_bytes),
            failure_bytes: AtomicU64::new(limits.failure_bytes),
        }
    }

    pub(super) fn load(&self) -> ResolutionExplanationLimits {
        ResolutionExplanationLimits::new(
            self.diagnostic_outcome_bytes.load(Ordering::Acquire),
            self.failure_bytes.load(Ordering::Acquire),
        )
    }

    pub(super) fn store(&self, limits: ResolutionExplanationLimits) {
        self.diagnostic_outcome_bytes
            .store(limits.diagnostic_outcome_bytes, Ordering::Release);
        self.failure_bytes
            .store(limits.failure_bytes, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn implementation_evidence_is_canonical_and_create_only() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("implementation.json");
        let evidence = ResolutionWalkImplementationEvidenceV1::new(
            ResolutionWorkerCount::new(2).unwrap(),
            vec![11, 13],
            8 * 1024 * 1024 * 1024,
            1536 * 1024 * 1024,
        )
        .unwrap();
        write_resolution_walk_implementation_evidence(&path, &evidence).unwrap();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            crate::json::canonical_json(&evidence).unwrap()
        );
        assert!(write_resolution_walk_implementation_evidence(&path, &evidence).is_err());
    }

    #[test]
    fn implementation_evidence_must_remain_outside_strict_bundle() {
        let directory = tempfile::tempdir().unwrap();
        let bundle = directory.path().join("strict-bundle");
        let sibling = directory.path().join("implementation.json");

        ensure_resolution_walk_evidence_outside_bundle(&bundle, &sibling).unwrap();
        let error = ensure_resolution_walk_evidence_outside_bundle(
            &bundle,
            &bundle.join("nested/../implementation.json"),
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("must remain outside strict bundle")
        );
    }

    #[cfg(unix)]
    #[test]
    fn implementation_evidence_resolves_symlinked_parent_aliases() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let real = directory.path().join("real");
        let alias = directory.path().join("alias");
        std::fs::create_dir(&real).unwrap();
        symlink(&real, &alias).unwrap();

        let bundle = real.join("strict-bundle");
        let evidence = alias.join("strict-bundle/implementation.json");
        assert!(ensure_resolution_walk_evidence_outside_bundle(&bundle, &evidence).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn implementation_evidence_preserves_symlink_parent_component_semantics() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().unwrap();
        let bundle = temporary.path().join("strict-bundle");
        let nested = bundle.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        let alias = temporary.path().join("alias");
        symlink(&nested, &alias).unwrap();

        let evidence = alias.join("../implementation.json");
        assert!(ensure_resolution_walk_evidence_outside_bundle(&bundle, &evidence).is_err());
    }

    #[test]
    fn atomic_limits_publish_independent_allowances() {
        let limits =
            AtomicResolutionExplanationLimits::new(ResolutionExplanationLimits::new(64, 128));
        limits.store(ResolutionExplanationLimits::new(0, 32));
        assert_eq!(limits.load(), ResolutionExplanationLimits::new(0, 32));
    }
}
