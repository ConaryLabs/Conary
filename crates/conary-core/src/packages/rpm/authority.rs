// conary-core/src/packages/rpm/authority.rs

//! Exact RPM identity and declared-provision authority.

use crate::packages::config_authority::ConfigPayloadAssociation;
use crate::repository::dependency_model::{
    ProvideArchitectureQualifier, ProvideVersionRelation, RepositoryCapabilityKind,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RpmConfigDeclaration {
    pub header_index: u32,
    pub path: String,
    pub noreplace: bool,
    pub ghost: bool,
    pub missing_ok: bool,
    pub payload: ConfigPayloadAssociation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpmDeclaredCapability {
    pub header_index: u32,
    pub kind: RepositoryCapabilityKind,
    pub name: String,
    pub version: Option<String>,
    pub version_relation: Option<ProvideVersionRelation>,
    pub architecture_qualifier: ProvideArchitectureQualifier,
}

/// A header path the package owns without shipping content for it (`%ghost`).
///
/// RPM keeps these in the file table and satisfies file dependencies from them,
/// but excludes them from the payload, so they are the package's promise that
/// its own lifecycle will create the path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpmPromisedPath {
    pub header_index: u32,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpmPackageAuthority {
    pub name: String,
    pub epoch: Option<u32>,
    pub version_component: String,
    pub release: String,
    pub evr: String,
    pub architecture: String,
    pub provides: Vec<RpmDeclaredCapability>,
    pub config: Vec<RpmConfigDeclaration>,
    pub promised_paths: Vec<RpmPromisedPath>,
}

pub(super) fn parse_promised_paths(package: &rpm::Package) -> crate::Result<Vec<RpmPromisedPath>> {
    use rpm::FileFlags;

    package
        .metadata
        .get_file_entries()
        .map_err(|error| {
            crate::Error::ParseError(format!("Failed to read RPM file metadata: {error}"))
        })?
        .into_iter()
        .enumerate()
        .filter(|(_, entry)| entry.flags().contains(FileFlags::GHOST))
        .map(|(header_index, entry)| {
            Ok(RpmPromisedPath {
                header_index: u32::try_from(header_index).map_err(|_| {
                    crate::Error::ParseError("RPM file record index exceeds u32".to_string())
                })?,
                path: entry.path().to_string_lossy().to_string(),
            })
        })
        .collect()
}

pub(super) fn parse_config_declarations(
    package: &rpm::Package,
) -> crate::Result<Vec<RpmConfigDeclaration>> {
    use rpm::FileFlags;

    package
        .metadata
        .get_file_entries()
        .map_err(|error| {
            crate::Error::ParseError(format!("Failed to read RPM config-file metadata: {error}"))
        })?
        .into_iter()
        .enumerate()
        .filter(|(_, entry)| entry.flags().contains(FileFlags::CONFIG))
        .map(|(header_index, entry)| {
            let ghost = entry.flags().contains(FileFlags::GHOST);
            Ok(RpmConfigDeclaration {
                header_index: u32::try_from(header_index).map_err(|_| {
                    crate::Error::ParseError("RPM config record index exceeds u32".to_string())
                })?,
                path: entry.path().to_string_lossy().to_string(),
                noreplace: entry.flags().contains(FileFlags::NOREPLACE),
                ghost,
                missing_ok: entry.flags().contains(FileFlags::MISSINGOK),
                payload: if ghost {
                    ConfigPayloadAssociation::Absent
                } else {
                    ConfigPayloadAssociation::Matched
                },
            })
        })
        .collect()
}
