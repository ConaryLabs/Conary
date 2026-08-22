// conary-core/src/canonical/exchange.rs

//! Exact canonical-map wire contract for signed Remi universes.

use std::collections::BTreeMap;
use std::fmt;

use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use crate::repository::supported_profiles::profile_by_public_id;
use crate::{Error, Result};

/// Owned canonical map response ready for validation and persistence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalMapSnapshot {
    pub schema_version: u32,
    pub revision: u64,
    pub generated_at: Option<String>,
    pub entries: Vec<CanonicalMapEntry>,
}

/// A single canonical identity and its exact per-profile implementations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalMapEntry {
    pub canonical: String,
    pub kind: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(deserialize_with = "deserialize_implementations")]
    pub implementations: BTreeMap<String, String>,
}

/// Parse and validate the complete canonical-map wire document.
pub fn parse_snapshot(bytes: &[u8]) -> Result<CanonicalMapSnapshot> {
    let snapshot = serde_json::from_slice::<CanonicalMapSnapshot>(bytes)
        .map_err(|error| Error::ParseError(format!("invalid canonical map JSON: {error}")))?;
    validate_snapshot(&snapshot)?;
    Ok(snapshot)
}

/// Validate a deserialized snapshot before it reaches persistence.
pub fn validate_snapshot(snapshot: &CanonicalMapSnapshot) -> Result<()> {
    if snapshot.schema_version != super::CANONICAL_MAP_SCHEMA_VERSION {
        return Err(Error::ParseError(format!(
            "unsupported canonical map schema version {}; expected {}",
            snapshot.schema_version,
            super::CANONICAL_MAP_SCHEMA_VERSION
        )));
    }
    match (snapshot.revision, snapshot.generated_at.as_deref()) {
        (0, None) if snapshot.entries.is_empty() => {}
        (0, _) => {
            return Err(Error::ParseError(
                "canonical map revision zero must be empty and have no generation timestamp"
                    .to_string(),
            ));
        }
        (_, Some(generated_at)) => {
            chrono::DateTime::parse_from_rfc3339(generated_at).map_err(|error| {
                Error::ParseError(format!(
                    "canonical map generated_at is not RFC 3339: {error}"
                ))
            })?;
        }
        (_, None) => {
            return Err(Error::ParseError(
                "positive canonical map revision requires generated_at".to_string(),
            ));
        }
    }

    let mut previous_name = None;
    for entry in &snapshot.entries {
        validate_entry(entry)?;
        if previous_name.is_some_and(|previous| previous >= entry.canonical.as_str()) {
            return Err(Error::ParseError(
                "canonical map entries must be strictly ordered by canonical name".to_string(),
            ));
        }
        previous_name = Some(entry.canonical.as_str());
    }
    Ok(())
}

pub(crate) fn validate_entry(entry: &CanonicalMapEntry) -> Result<()> {
    validate_token("canonical", &entry.canonical)?;
    if !matches!(entry.kind.as_str(), "package" | "group") {
        return Err(Error::ParseError(format!(
            "canonical map entry '{}' has unsupported kind '{}'",
            entry.canonical, entry.kind
        )));
    }
    if let Some(category) = entry.category.as_deref() {
        validate_token("category", category)?;
    }
    if entry.implementations.is_empty() {
        return Err(Error::ParseError(format!(
            "canonical map entry '{}' has no implementations",
            entry.canonical
        )));
    }
    for (profile_id, package_name) in &entry.implementations {
        if profile_by_public_id(profile_id).is_none() {
            return Err(Error::ParseError(format!(
                "canonical map entry '{}' names unsupported public profile '{profile_id}'",
                entry.canonical
            )));
        }
        validate_token("implementations.package", package_name)?;
    }
    Ok(())
}

fn validate_token(field: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || value != value.trim()
        || value.chars().any(char::is_whitespace)
        || value.chars().any(char::is_control)
    {
        return Err(Error::ParseError(format!(
            "canonical map field '{field}' must be a nonempty exact token"
        )));
    }
    Ok(())
}

fn deserialize_implementations<'de, D>(
    deserializer: D,
) -> std::result::Result<BTreeMap<String, String>, D::Error>
where
    D: Deserializer<'de>,
{
    struct ImplementationsVisitor;

    impl<'de> Visitor<'de> for ImplementationsVisitor {
        type Value = BTreeMap<String, String>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("an exact public-profile to package-name map")
        }

        fn visit_map<M>(self, mut access: M) -> std::result::Result<Self::Value, M::Error>
        where
            M: MapAccess<'de>,
        {
            let mut mappings = BTreeMap::new();
            while let Some((profile, package)) = access.next_entry::<String, String>()? {
                if mappings.insert(profile.clone(), package).is_some() {
                    return Err(serde::de::Error::custom(format!(
                        "duplicate canonical implementation profile '{profile}'"
                    )));
                }
            }
            Ok(mappings)
        }
    }

    deserializer.deserialize_map(ImplementationsVisitor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_duplicate_profile_keys_before_map_coalescing() {
        let error = parse_snapshot(
            br#"{
                "schema_version": 1,
                "revision": 1,
                "generated_at": "2026-07-26T00:00:00Z",
                "entries": [{
                    "canonical": "curl",
                    "kind": "package",
                    "implementations": {
                        "fedora-44": "curl",
                        "fedora-44": "curl-minimal"
                    }
                }]
            }"#,
        )
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("duplicate canonical implementation profile 'fedora-44'")
        );
    }

    #[test]
    fn rejects_unknown_fields_profiles_and_duplicate_canonical_entries() {
        let empty_initial = parse_snapshot(
            br#"{
                "schema_version": 1,
                "revision": 0,
                "generated_at": null,
                "entries": []
            }"#,
        )
        .unwrap();
        assert!(empty_initial.entries.is_empty());

        let unsupported_version = parse_snapshot(
            br#"{
                "schema_version": 2,
                "revision": 1,
                "generated_at": "2026-07-26T00:00:00Z",
                "entries": []
            }"#,
        )
        .unwrap_err();
        assert!(
            unsupported_version
                .to_string()
                .contains("unsupported canonical map schema version 2")
        );

        let unknown_field = parse_snapshot(
            br#"{
                "schema_version": 1,
                "revision": 1,
                "generated_at": "2026-07-26T00:00:00Z",
                "entries": [],
                "aliases": {}
            }"#,
        )
        .unwrap_err();
        assert!(
            unknown_field
                .to_string()
                .contains("unknown field `aliases`")
        );

        let unknown_profile = parse_snapshot(
            br#"{
                "schema_version": 1,
                "revision": 1,
                "generated_at": "2026-07-26T00:00:00Z",
                "entries": [{
                    "canonical": "curl",
                    "kind": "package",
                    "implementations": {"fedora": "curl"}
                }]
            }"#,
        )
        .unwrap_err();
        assert!(
            unknown_profile
                .to_string()
                .contains("unsupported public profile 'fedora'")
        );

        let duplicate_canonical = parse_snapshot(
            br#"{
                "schema_version": 1,
                "revision": 1,
                "generated_at": "2026-07-26T00:00:00Z",
                "entries": [
                    {
                        "canonical": "curl",
                        "kind": "package",
                        "implementations": {"fedora-44": "curl"}
                    },
                    {
                        "canonical": "curl",
                        "kind": "package",
                        "implementations": {"arch": "curl"}
                    }
                ]
            }"#,
        )
        .unwrap_err();
        assert!(
            duplicate_canonical
                .to_string()
                .contains("strictly ordered by canonical name")
        );
    }
}
