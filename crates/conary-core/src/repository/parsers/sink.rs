// conary-core/src/repository/parsers/sink.rs

//! Versioned streaming output contract for authenticated repository parsers.

use std::collections::BTreeSet;

use crate::error::{Error, Result};

use super::{
    AuthenticatedMetadataObject, AuthenticatedRepositoryMetadata, AuthenticatedSnapshotIdentity,
    PackageMetadata,
};

/// Parser-to-storage contract for one authenticated native repository snapshot.
///
/// Implementations may persist each package immediately. Parsers must not infer
/// success from a partially populated sink: only a successful parser return and
/// `finish` make the collected snapshot usable.
pub trait RepositorySnapshotSink {
    /// Record one authenticated child object consumed by the projection.
    fn authenticated_object(&mut self, object: AuthenticatedMetadataObject) -> Result<()>;

    /// Admit one complete native package projection.
    fn package(&mut self, package: PackageMetadata) -> Result<()>;
}

/// Compatibility sink for callers that still replace mutable local repository
/// rows in one transaction. The parser API remains streaming; immutable Remi
/// publication supplies a disk-backed sink instead.
#[derive(Default)]
pub(crate) struct CollectingRepositorySnapshotSink {
    packages: Vec<PackageMetadata>,
    authenticated_objects: Vec<AuthenticatedMetadataObject>,
    object_roles: BTreeSet<String>,
}

impl RepositorySnapshotSink for CollectingRepositorySnapshotSink {
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
}

impl CollectingRepositorySnapshotSink {
    pub(crate) fn finish(
        mut self,
        snapshot: AuthenticatedSnapshotIdentity,
    ) -> Result<AuthenticatedRepositoryMetadata> {
        self.authenticated_objects.sort_by(|left, right| {
            format!("{:?}", left.role)
                .cmp(&format!("{:?}", right.role))
                .then_with(|| left.source_path.cmp(&right.source_path))
        });
        Ok(AuthenticatedRepositoryMetadata {
            packages: self.packages,
            snapshot,
            authenticated_objects: self.authenticated_objects,
        })
    }
}
