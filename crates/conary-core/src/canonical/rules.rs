// conary-core/src/canonical/rules.rs

//! Exact, versioned canonical package-map contracts.
//!
//! Canonical equivalence affects package selection and therefore mutation
//! authority. The contract deliberately supports only literal canonical names,
//! literal package names, and exact supported profile IDs. Regexes, aliases,
//! filename precedence, and first-match behavior are not part of the format.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Deserialize;

use crate::repository::supported_profiles::profile_by_public_id;
use crate::{Error, Result};

const CONTRACT_VERSION: u32 = 1;

/// One profile ID or a list of profile IDs.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

impl OneOrMany {
    pub fn iter(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        match self {
            Self::One(value) => Box::new(std::iter::once(value.as_str())),
            Self::Many(values) => Box::new(values.iter().map(String::as_str)),
        }
    }
}

/// One exact mapping from a canonical identity to a package in one or more
/// supported profiles.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExactMapping {
    pub canonical: String,
    pub package: String,
    pub profiles: OneOrMany,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
}

impl ExactMapping {
    #[must_use]
    pub fn kind(&self) -> &str {
        self.kind.as_deref().unwrap_or("package")
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContractDocument {
    version: u32,
    mappings: Vec<ExactMapping>,
}

/// A validated collection of exact canonical mappings.
#[derive(Debug, Clone)]
pub struct CanonicalMapContract {
    mappings: Vec<ExactMapping>,
}

impl CanonicalMapContract {
    /// Validate mappings assembled from one or more contract documents.
    pub fn new(mappings: Vec<ExactMapping>) -> Result<Self> {
        validate_mappings(&mappings)?;
        Ok(Self { mappings })
    }

    /// Load all YAML contract documents in a directory.
    ///
    /// Filenames affect only stable diagnostics. Precedence does not exist:
    /// duplicate or conflicting authority is rejected across the full set.
    pub fn load_from_dir(dir: &Path) -> Result<Self> {
        let mut paths = std::fs::read_dir(dir)
            .map_err(|error| {
                Error::IoError(format!(
                    "cannot read canonical contract directory {}: {error}",
                    dir.display()
                ))
            })?
            .filter_map(|entry| {
                let path = entry.ok()?.path();
                matches!(
                    path.extension().and_then(|extension| extension.to_str()),
                    Some("yaml" | "yml")
                )
                .then_some(path)
            })
            .collect::<Vec<_>>();
        paths.sort();

        let mut mappings = Vec::new();
        for path in paths {
            let text = std::fs::read_to_string(&path).map_err(|error| {
                Error::IoError(format!(
                    "cannot read canonical contract {}: {error}",
                    path.display()
                ))
            })?;
            let contract = parse_contract(&text).map_err(|error| {
                Error::ParseError(format!(
                    "canonical contract {} is invalid: {error}",
                    path.display()
                ))
            })?;
            mappings.extend(contract.mappings);
        }

        Self::new(mappings)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.mappings.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.mappings.is_empty()
    }

    pub fn mappings(&self) -> impl Iterator<Item = &ExactMapping> {
        self.mappings.iter()
    }
}

/// Parse and validate one versioned YAML contract document.
pub fn parse_contract(yaml: &str) -> Result<CanonicalMapContract> {
    let document: ContractDocument = serde_yaml::from_str(yaml)
        .map_err(|error| Error::ParseError(format!("YAML parse error: {error}")))?;
    if document.version != CONTRACT_VERSION {
        return Err(Error::ParseError(format!(
            "unsupported canonical contract version {}; expected {CONTRACT_VERSION}",
            document.version
        )));
    }
    CanonicalMapContract::new(document.mappings)
}

fn validate_mappings(mappings: &[ExactMapping]) -> Result<()> {
    let mut exact_owners = BTreeMap::<(String, String), String>::new();
    let mut canonical_metadata = BTreeMap::<String, (String, Option<String>)>::new();

    for mapping in mappings {
        validate_token("canonical", &mapping.canonical)?;
        validate_token("package", &mapping.package)?;
        let kind = mapping.kind();
        if !matches!(kind, "package" | "group") {
            return Err(Error::ParseError(format!(
                "canonical mapping '{}' has unsupported kind '{kind}'",
                mapping.canonical
            )));
        }
        if let Some(category) = mapping.category.as_deref() {
            validate_token("category", category)?;
        }

        let metadata = (kind.to_string(), mapping.category.clone());
        if let Some(existing) = canonical_metadata.get(&mapping.canonical) {
            if existing != &metadata {
                return Err(Error::ParseError(format!(
                    "canonical mapping '{}' has conflicting kind/category metadata",
                    mapping.canonical
                )));
            }
        } else {
            canonical_metadata.insert(mapping.canonical.clone(), metadata);
        }

        let mut profiles = BTreeSet::new();
        for profile_id in mapping.profiles.iter() {
            validate_token("profiles", profile_id)?;
            if profile_by_public_id(profile_id).is_none() {
                return Err(Error::ParseError(format!(
                    "canonical mapping '{}' names unsupported profile '{profile_id}'",
                    mapping.canonical
                )));
            }
            if !profiles.insert(profile_id) {
                return Err(Error::ParseError(format!(
                    "canonical mapping '{}' repeats profile '{profile_id}'",
                    mapping.canonical
                )));
            }

            let key = (mapping.canonical.clone(), profile_id.to_string());
            if let Some(existing_package) = exact_owners.insert(key, mapping.package.clone()) {
                let detail = if existing_package == mapping.package {
                    "duplicates"
                } else {
                    "conflicts with"
                };
                return Err(Error::ParseError(format!(
                    "canonical mapping '{}' for profile '{profile_id}' {detail} package '{existing_package}' with package '{}'",
                    mapping.canonical, mapping.package
                )));
            }
        }
        if profiles.is_empty() {
            return Err(Error::ParseError(format!(
                "canonical mapping '{}' has no profiles",
                mapping.canonical
            )));
        }
    }

    Ok(())
}

fn validate_token(field: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || value != value.trim()
        || value.chars().any(char::is_control)
        || value.chars().any(char::is_whitespace)
    {
        return Err(Error::ParseError(format!(
            "canonical contract field '{field}' must be a nonempty exact token"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXACT: &str = r#"
version: 1
mappings:
  - canonical: apache-httpd
    package: httpd
    profiles: fedora-44
  - canonical: apache-httpd
    package: apache2
    profiles: ubuntu-26.04
"#;

    #[test]
    fn parses_exact_versioned_contract() {
        let contract = parse_contract(EXACT).unwrap();
        assert_eq!(contract.len(), 2);
        assert_eq!(
            contract
                .mappings()
                .map(|mapping| mapping.package.as_str())
                .collect::<Vec<_>>(),
            ["httpd", "apache2"]
        );
    }

    #[test]
    fn rejects_unknown_fields_instead_of_accepting_regex_authority() {
        let error = parse_contract(
            r#"
version: 1
mappings:
  - canonical: openssl
    package: openssl
    profiles: fedora-44
    namepat: "^openssl.*$"
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("unknown field `namepat`"));
    }

    #[test]
    fn rejects_aliases_and_unknown_profiles() {
        let error = parse_contract(
            r#"
version: 1
mappings:
  - canonical: openssl
    package: openssl
    profiles: fedora
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("unsupported profile 'fedora'"));
    }

    #[test]
    fn rejects_conflicting_implementation_authority() {
        let error = parse_contract(
            r#"
version: 1
mappings:
  - canonical: openssl
    package: openssl
    profiles: fedora-44
  - canonical: openssl
    package: openssl-libs
    profiles: fedora-44
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("conflicts with"));
    }

    #[test]
    fn rejects_duplicate_implementation_authority() {
        let error = parse_contract(
            r#"
version: 1
mappings:
  - canonical: openssl
    package: openssl
    profiles: fedora-44
  - canonical: openssl
    package: openssl
    profiles: fedora-44
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("duplicates"));
    }

    #[test]
    fn load_rejects_conflicts_across_documents() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("one.yaml"), EXACT).unwrap();
        std::fs::write(
            dir.path().join("two.yaml"),
            r#"
version: 1
mappings:
  - canonical: apache-httpd
    package: apache
    profiles: fedora-44
"#,
        )
        .unwrap();

        let error = CanonicalMapContract::load_from_dir(dir.path()).unwrap_err();
        assert!(error.to_string().contains("conflicts with"));
    }

    #[test]
    fn bundled_contract_is_exact_and_current() {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/canonical-rules");
        let contract = CanonicalMapContract::load_from_dir(&directory).unwrap();
        assert!(!contract.is_empty());
    }
}
