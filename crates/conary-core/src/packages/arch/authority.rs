// conary-core/src/packages/arch/authority.rs

//! Exact ALPM identity and declared-provision authority.

use crate::packages::config_authority::ConfigPayloadAssociation;
use crate::repository::dependency_model::{
    ProvideArchitectureQualifier, ProvideVersionRelation, RepositoryCapabilityKind,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlpmConfigDeclaration {
    pub pkginfo_index: u32,
    /// Exact relative spelling from `.PKGINFO`.
    pub source_path: String,
    /// Canonical absolute install path.
    pub path: String,
    /// Optional installed MD5 baseline carried by local ALPM metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed_hash: Option<String>,
    pub payload: ConfigPayloadAssociation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlpmDeclaredCapability {
    pub pkginfo_index: u32,
    pub kind: RepositoryCapabilityKind,
    pub name: String,
    pub version: Option<String>,
    pub version_relation: Option<ProvideVersionRelation>,
    pub architecture_qualifier: ProvideArchitectureQualifier,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlpmPackageAuthority {
    pub name: String,
    pub version: String,
    pub architecture: String,
    pub package_type: Option<String>,
    pub provides: Vec<AlpmDeclaredCapability>,
    pub config: Vec<AlpmConfigDeclaration>,
}

pub(super) fn parse_config_declarations(
    entries: &[String],
    payload_paths: &BTreeSet<&str>,
) -> crate::Result<Vec<AlpmConfigDeclaration>> {
    let mut declarations = Vec::with_capacity(entries.len());
    let mut paths = BTreeSet::new();
    for (pkginfo_index, entry) in entries.iter().enumerate() {
        let (source_path, installed_hash) = entry
            .split_once('\t')
            .map_or((entry.as_str(), None), |(path, hash)| (path, Some(hash)));
        if source_path.starts_with('/') {
            return Err(crate::Error::ParseError(format!(
                "ALPM backup record {pkginfo_index} path must be relative: {source_path}"
            )));
        }
        let path =
            crate::packages::archive_utils::normalize_path(source_path).map_err(|error| {
                crate::Error::ParseError(format!(
                    "ALPM backup record {pkginfo_index} has unsafe path {source_path}: {error}"
                ))
            })?;
        if !paths.insert(path.clone()) {
            return Err(crate::Error::ParseError(format!(
                "ALPM backup path {path} is declared more than once"
            )));
        }
        let installed_hash = installed_hash
            .map(|hash| {
                if hash.len() != 32
                    || !hash
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                {
                    return Err(crate::Error::ParseError(format!(
                        "ALPM backup record {pkginfo_index} has non-canonical MD5 baseline"
                    )));
                }
                Ok(hash.to_string())
            })
            .transpose()?;
        declarations.push(AlpmConfigDeclaration {
            pkginfo_index: u32::try_from(pkginfo_index).map_err(|_| {
                crate::Error::ParseError("ALPM backup record index exceeds u32".to_string())
            })?,
            source_path: source_path.to_string(),
            path: path.clone(),
            installed_hash,
            payload: if payload_paths.contains(path.as_str()) {
                ConfigPayloadAssociation::Matched
            } else {
                ConfigPayloadAssociation::Absent
            },
        });
    }
    Ok(declarations)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unmatched_backup_remains_declaration_only() {
        let payload = BTreeSet::from([
            "/usr/share/bash-completion/startup-core/000_bash_completion_compat.bash",
        ]);
        let declarations = parse_config_declarations(
            &["etc/bash_completion.d/000_bash_completion_compat.bash".to_string()],
            &payload,
        )
        .unwrap();

        assert_eq!(declarations.len(), 1);
        assert_eq!(
            declarations[0].path,
            "/etc/bash_completion.d/000_bash_completion_compat.bash"
        );
        assert_eq!(declarations[0].payload, ConfigPayloadAssociation::Absent);
    }

    #[test]
    fn duplicate_unsafe_and_malformed_hash_records_fail_closed() {
        let payload = BTreeSet::new();
        assert!(parse_config_declarations(&["../escape".to_string()], &payload).is_err());
        assert!(parse_config_declarations(&["etc/a\tnot-a-hash".to_string()], &payload).is_err());
        assert!(
            parse_config_declarations(&["etc/a".to_string(), "etc/a".to_string()], &payload)
                .is_err()
        );
    }
}
