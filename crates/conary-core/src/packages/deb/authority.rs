// conary-core/src/packages/deb/authority.rs

//! Exact Debian identity and declared-provision authority.

use crate::packages::config_authority::ConfigPayloadAssociation;
use crate::repository::dependency_model::{
    DebianMultiArch, ProvideArchitectureQualifier, ProvideVersionRelation, RepositoryCapabilityKind,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DebianConfigDeclaration {
    pub control_index: u32,
    pub path: String,
    pub remove_on_upgrade: bool,
    pub payload: ConfigPayloadAssociation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebianDeclaredCapability {
    pub control_index: u32,
    pub kind: RepositoryCapabilityKind,
    pub name: String,
    pub version: Option<String>,
    pub version_relation: Option<ProvideVersionRelation>,
    pub architecture_qualifier: ProvideArchitectureQualifier,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebianPackageAuthority {
    pub name: String,
    pub epoch: Option<u32>,
    pub source_version: String,
    pub version: String,
    pub architecture: String,
    pub multi_arch: DebianMultiArch,
    pub provides: Vec<DebianDeclaredCapability>,
    pub config: Vec<DebianConfigDeclaration>,
}

pub(super) fn parse_config_declarations(
    content: &str,
) -> crate::Result<Vec<DebianConfigDeclaration>> {
    if !content.is_empty() && !content.ends_with('\n') {
        return Err(crate::Error::InitError(
            "Invalid DEBIAN/conffiles: final entry is missing a newline".to_string(),
        ));
    }

    content
        .lines()
        .enumerate()
        .map(|(index, raw)| {
            let line_number = index + 1;
            let line = raw.trim_end();
            if line.is_empty() {
                return Err(crate::Error::InitError(format!(
                    "Invalid DEBIAN/conffiles:{line_number}: empty entries are not allowed"
                )));
            }
            let (path, remove_on_upgrade) = if line.starts_with('/') {
                (line, false)
            } else {
                let split = line.find(char::is_whitespace).ok_or_else(|| {
                    crate::Error::InitError(format!(
                        "Invalid DEBIAN/conffiles:{line_number}: conffile path is not absolute"
                    ))
                })?;
                let flag = &line[..split];
                let path = line[split..].trim_start();
                if flag != "remove-on-upgrade" {
                    return Err(crate::Error::InitError(format!(
                        "Invalid DEBIAN/conffiles:{line_number}: unsupported flag '{flag}'"
                    )));
                }
                (path, true)
            };
            if !path.starts_with('/') {
                return Err(crate::Error::InitError(format!(
                    "Invalid DEBIAN/conffiles:{line_number}: conffile path '{path}' is not absolute"
                )));
            }
            let path = crate::packages::archive_utils::normalize_path(path).map_err(|error| {
                crate::Error::InitError(format!(
                    "Invalid DEBIAN/conffiles:{line_number}: unsafe conffile path '{path}': {error}"
                ))
            })?;
            Ok(DebianConfigDeclaration {
                control_index: u32::try_from(index).map_err(|_| {
                    crate::Error::InitError("DEBIAN/conffiles record index exceeds u32".to_string())
                })?,
                path,
                remove_on_upgrade,
                payload: if remove_on_upgrade {
                    ConfigPayloadAssociation::Absent
                } else {
                    ConfigPayloadAssociation::Matched
                },
            })
        })
        .collect()
}
