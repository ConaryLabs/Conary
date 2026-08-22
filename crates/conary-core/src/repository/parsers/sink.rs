// conary-core/src/repository/parsers/sink.rs

//! Versioned streaming output contract for authenticated repository parsers.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::error::{Error, Result};

use super::{
    AuthenticatedMetadataObject, AuthenticatedSnapshotIdentity, ChecksumType, PackageMetadata,
};
use crate::repository::dependency_model::RepositoryProvide;

/// Normalized parser projection and sink schema version used in cache keys.
pub const REPOSITORY_SNAPSHOT_PROJECTION_VERSION: u32 = 1;

/// Exact parser-level identity used to join authenticated child projections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotPackageIdentity {
    pub name: String,
    pub version: String,
    pub architecture: Option<String>,
    pub checksum: String,
    pub checksum_type: ChecksumType,
}

/// Authenticated child relation that must cover the complete package corpus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SnapshotPackageJoin {
    RpmFilelists,
}

impl SnapshotPackageJoin {
    pub(in crate::repository) const fn as_str(self) -> &'static str {
        match self {
            Self::RpmFilelists => "rpm_filelists",
        }
    }
}

/// Result of merging one child projection into exact package state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SnapshotProvideUpdate {
    pub matched_packages: usize,
    pub added: usize,
    pub already_known: usize,
}

/// Parser-to-storage contract for one authenticated native repository snapshot.
///
/// Implementations may persist each package immediately. Parsers must not infer
/// success from a partially populated sink: only a successful parser return and
/// `finish` make the collected snapshot usable.
pub trait RepositorySnapshotSink {
    /// Private run-local directory for authenticated child downloads and
    /// parser spools. Its contents never become immutable publication.
    fn work_directory(&self) -> &Path;

    /// Record one authenticated child object consumed by the projection.
    fn authenticated_object(&mut self, object: AuthenticatedMetadataObject) -> Result<()>;

    /// Replay a strict normalized projection when every authenticated input
    /// and the exact source binding match. The default sink has no cache.
    fn reuse_cached_projection(
        &mut self,
        _snapshot: &AuthenticatedSnapshotIdentity,
        _objects: &[AuthenticatedMetadataObject],
    ) -> Result<bool> {
        Ok(false)
    }

    /// Admit one complete native package projection.
    fn package(&mut self, package: PackageMetadata) -> Result<()>;

    /// Merge child-projected provides into one exact previously admitted
    /// package identity and mark that package as covered by `join`.
    fn extend_package_provides(
        &mut self,
        join: SnapshotPackageJoin,
        identity: &SnapshotPackageIdentity,
        provides: Vec<RepositoryProvide>,
    ) -> Result<SnapshotProvideUpdate>;

    /// Prove that a child join covered every admitted package exactly once.
    fn finish_package_join(&mut self, join: SnapshotPackageJoin) -> Result<()>;
}

/// Compatibility sink for callers that still replace mutable local repository
/// rows in one transaction. The parser API remains streaming; immutable Remi
/// publication supplies a disk-backed sink instead.
pub(crate) struct CollectingRepositorySnapshotSink {
    packages: Vec<PackageMetadata>,
    authenticated_objects: Vec<AuthenticatedMetadataObject>,
    object_roles: BTreeSet<String>,
    work_directory: tempfile::TempDir,
    join_marks: BTreeMap<SnapshotPackageJoin, BTreeSet<usize>>,
}

impl CollectingRepositorySnapshotSink {
    pub(crate) fn create() -> Result<Self> {
        Ok(Self {
            packages: Vec::new(),
            authenticated_objects: Vec::new(),
            object_roles: BTreeSet::new(),
            work_directory: tempfile::Builder::new()
                .prefix("conary-native-ingest-")
                .tempdir()?,
            join_marks: BTreeMap::new(),
        })
    }
}

impl RepositorySnapshotSink for CollectingRepositorySnapshotSink {
    fn work_directory(&self) -> &Path {
        self.work_directory.path()
    }

    fn authenticated_object(&mut self, object: AuthenticatedMetadataObject) -> Result<()> {
        let role = format!("{:?}", object.role);
        if !self.object_roles.insert(role.clone()) {
            return Err(Error::ConflictError(format!(
                "repository snapshot repeats authenticated metadata role {role}"
            )));
        }
        self.authenticated_objects.push(object);
        Ok(())
    }

    fn package(&mut self, package: PackageMetadata) -> Result<()> {
        self.packages.push(package);
        Ok(())
    }

    fn extend_package_provides(
        &mut self,
        join: SnapshotPackageJoin,
        identity: &SnapshotPackageIdentity,
        provides: Vec<RepositoryProvide>,
    ) -> Result<SnapshotProvideUpdate> {
        let targets = self
            .packages
            .iter()
            .enumerate()
            .filter_map(|(index, package)| {
                (package.checksum == identity.checksum
                    && package.checksum_type == identity.checksum_type)
                    .then_some(index)
            })
            .collect::<Vec<_>>();
        if targets.is_empty() {
            return Err(Error::ParseError(format!(
                "signed filelists.xml publishes file records for pkgid {}, which the signed primary.xml does not publish",
                identity.checksum
            )));
        }
        let marks = self.join_marks.entry(join).or_default();
        let mut result = SnapshotProvideUpdate::default();
        for index in targets {
            let package = &mut self.packages[index];
            let disagreement = if package.name != identity.name {
                Some(("name", identity.name.as_str(), package.name.as_str()))
            } else if package.architecture != identity.architecture {
                Some((
                    "architecture",
                    identity.architecture.as_deref().unwrap_or_default(),
                    package.architecture.as_deref().unwrap_or_default(),
                ))
            } else if package.version != identity.version {
                Some((
                    "version",
                    identity.version.as_str(),
                    package.version.as_str(),
                ))
            } else {
                None
            };
            if let Some((field, child, primary)) = disagreement {
                return Err(Error::ParseError(format!(
                    "signed filelists.xml and primary.xml disagree on pkgid {}: filelists {field} is '{child}' but primary {field} is '{primary}'",
                    identity.checksum,
                )));
            }
            if !marks.insert(index) {
                return Err(Error::ConflictError(format!(
                    "authenticated child metadata repeats package checksum {}",
                    identity.checksum
                )));
            }
            result.matched_packages += 1;
            for provide in &provides {
                if package.provides.contains(provide) {
                    result.already_known += 1;
                } else {
                    package.provides.push(provide.clone());
                    result.added += 1;
                }
            }
        }
        Ok(result)
    }

    fn finish_package_join(&mut self, join: SnapshotPackageJoin) -> Result<()> {
        let marks = self.join_marks.remove(&join).unwrap_or_default();
        if marks.len() != self.packages.len() {
            let index = (0..self.packages.len())
                .find(|index| !marks.contains(index))
                .expect("join cardinality mismatch has an unmatched package");
            let package = &self.packages[index];
            return Err(Error::ParseError(format!(
                "signed filelists.xml publishes no file record for package {} {} (pkgid {})",
                package.name, package.version, package.checksum
            )));
        }
        Ok(())
    }
}

impl CollectingRepositorySnapshotSink {
    pub(crate) fn finish(mut self) -> (Vec<PackageMetadata>, Vec<AuthenticatedMetadataObject>) {
        self.authenticated_objects.sort_by(|left, right| {
            format!("{:?}", left.role)
                .cmp(&format!("{:?}", right.role))
                .then_with(|| left.source_path.cmp(&right.source_path))
        });
        (self.packages, self.authenticated_objects)
    }
}
