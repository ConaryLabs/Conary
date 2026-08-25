// conary-core/src/repository/parsers/sink.rs

//! Versioned streaming output contract for authenticated repository parsers.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

use crate::error::{Error, Result};

use super::{
    AuthenticatedMetadataObject, AuthenticatedSnapshotIdentity, ChecksumType, PackageMetadata,
};
use crate::repository::catalog::CatalogMetadataScratchV1;
use crate::repository::dependency_model::RepositoryProvide;
use crate::repository::dependency_model::{RepositoryCapabilityKind, RepositoryRequirementKind};

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

/// One source-native fragment in an Arch repository database.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchPackageFragmentKind {
    Desc,
    Depends,
}

/// One exact paired Arch package record, replayed in source-directory order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchPackageRecord {
    pub directory: String,
    pub desc: String,
    pub depends: Option<String>,
}

#[derive(Debug, Default)]
struct ArchPackageFragments {
    desc: Option<String>,
    depends: Option<String>,
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

    /// Retain exact authenticated-metadata capacity until the sink removes its
    /// run-local work files. Compatibility sinks validate but do not admit
    /// host capacity; Remi's immutable sink owns that policy.
    fn reserve_authenticated_metadata(
        &mut self,
        requirement: CatalogMetadataScratchV1,
    ) -> Result<()>;

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

    /// Retain one exact out-of-order ALPM database fragment without creating
    /// a second parser database.
    fn stage_arch_package_fragment(
        &mut self,
        directory: String,
        kind: ArchPackageFragmentKind,
        content: String,
    ) -> Result<()>;

    /// Take the next complete ALPM record in exact directory order.
    fn take_arch_package_record(&mut self) -> Result<Option<ArchPackageRecord>>;

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

    /// Prove that every positive RPM path requirement has a provider when the
    /// signed repository publishes no complete filelists object.
    fn validate_rpm_primary_file_requirements(&mut self, repo_url: &str) -> Result<()>;
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
    arch_package_fragments: BTreeMap<String, ArchPackageFragments>,
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
            arch_package_fragments: BTreeMap::new(),
        })
    }
}

impl RepositorySnapshotSink for CollectingRepositorySnapshotSink {
    fn work_directory(&self) -> &Path {
        self.work_directory.path()
    }

    fn reserve_authenticated_metadata(
        &mut self,
        requirement: CatalogMetadataScratchV1,
    ) -> Result<()> {
        requirement.validate()
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

    fn stage_arch_package_fragment(
        &mut self,
        directory: String,
        kind: ArchPackageFragmentKind,
        content: String,
    ) -> Result<()> {
        if directory.is_empty() {
            return Err(Error::ParseError(
                "Arch repository package directory is empty".to_string(),
            ));
        }
        let fragments = self
            .arch_package_fragments
            .entry(directory.clone())
            .or_default();
        let (slot, label) = match kind {
            ArchPackageFragmentKind::Desc => (&mut fragments.desc, "desc"),
            ArchPackageFragmentKind::Depends => (&mut fragments.depends, "depends"),
        };
        if slot.is_some() {
            return Err(Error::ParseError(format!(
                "Arch repository repeats {label} metadata for {directory}"
            )));
        }
        *slot = Some(content);
        Ok(())
    }

    fn take_arch_package_record(&mut self) -> Result<Option<ArchPackageRecord>> {
        let Some((directory, fragments)) = self.arch_package_fragments.pop_first() else {
            return Ok(None);
        };
        let desc = fragments.desc.ok_or_else(|| {
            Error::ParseError(format!(
                "Arch repository has depends metadata without desc metadata for {directory}"
            ))
        })?;
        Ok(Some(ArchPackageRecord {
            directory,
            desc,
            depends: fragments.depends,
        }))
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

    fn validate_rpm_primary_file_requirements(&mut self, repo_url: &str) -> Result<()> {
        let provided_paths = self
            .packages
            .iter()
            .flat_map(|package| &package.provides)
            .filter(|provide| provide.kind == RepositoryCapabilityKind::File)
            .map(|provide| provide.name.as_str())
            .collect::<HashSet<_>>();
        for package in &self.packages {
            for group in &package.requirements {
                if !matches!(
                    group.kind,
                    RepositoryRequirementKind::Depends | RepositoryRequirementKind::PreDepends
                ) {
                    continue;
                }
                let mut provided = |path: &str| Ok(provided_paths.contains(path));
                super::fedora::audit::require_primary_file_providers(
                    repo_url,
                    &package.name,
                    &package.version,
                    &group.expression,
                    &mut provided,
                )?;
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compatibility_arch_pairing_is_canonical_and_creates_no_work_file() {
        let mut sink = CollectingRepositorySnapshotSink::create().unwrap();
        let work_directory = sink.work_directory().to_path_buf();
        sink.stage_arch_package_fragment(
            "zeta".to_string(),
            ArchPackageFragmentKind::Depends,
            "zeta depends".to_string(),
        )
        .unwrap();
        sink.stage_arch_package_fragment(
            "alpha".to_string(),
            ArchPackageFragmentKind::Desc,
            "alpha desc".to_string(),
        )
        .unwrap();
        sink.stage_arch_package_fragment(
            "zeta".to_string(),
            ArchPackageFragmentKind::Desc,
            "zeta desc".to_string(),
        )
        .unwrap();

        assert_eq!(
            sink.take_arch_package_record().unwrap().unwrap().directory,
            "alpha"
        );
        assert_eq!(
            sink.take_arch_package_record().unwrap().unwrap().directory,
            "zeta"
        );
        assert_eq!(sink.take_arch_package_record().unwrap(), None);
        assert_eq!(std::fs::read_dir(work_directory).unwrap().count(), 0);
    }
}
