// conary-core/src/canonical/exchange.rs

//! Exact canonical-map wire contract and Remi snapshot persistence.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use rusqlite::Connection;
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer};

use crate::db::models::{CanonicalMappingAuthority, CanonicalPackage, PackageImplementation};
use crate::repository::supported_profiles::profile_by_public_id;
use crate::{Error, Result};

pub(crate) const CANONICAL_MAP_SHA256_HEADER: &str = "x-conary-canonical-sha256";

/// Owned canonical map response ready for validation and persistence.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CanonicalMapSnapshot {
    pub(crate) schema_version: u32,
    pub(crate) revision: u64,
    pub(crate) generated_at: Option<String>,
    pub(crate) entries: Vec<CanonicalMapEntry>,
}

/// A single canonical identity and its exact per-profile implementations.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CanonicalMapEntry {
    pub(crate) canonical: String,
    pub(crate) kind: String,
    #[serde(default)]
    pub(crate) category: Option<String>,
    #[serde(deserialize_with = "deserialize_implementations")]
    pub(crate) implementations: BTreeMap<String, String>,
}

/// Parse and validate the complete canonical-map wire document.
pub(crate) fn parse_snapshot(bytes: &[u8]) -> Result<CanonicalMapSnapshot> {
    let snapshot = serde_json::from_slice::<CanonicalMapSnapshot>(bytes)
        .map_err(|error| Error::ParseError(format!("invalid canonical map JSON: {error}")))?;
    validate_snapshot(&snapshot)?;
    Ok(snapshot)
}

/// Read the exact body checksum declared by a canonical-map response.
pub(crate) fn expected_checksum(headers: &reqwest::header::HeaderMap) -> Result<String> {
    headers
        .get(CANONICAL_MAP_SHA256_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| value.len() == 64 && value.chars().all(|ch| ch.is_ascii_hexdigit()))
        .map(str::to_string)
        .ok_or_else(|| {
            Error::TrustError(
                "canonical map response missing valid X-Conary-Canonical-Sha256 checksum"
                    .to_string(),
            )
        })
}

/// Validate a deserialized snapshot before it reaches persistence.
pub(crate) fn validate_snapshot(snapshot: &CanonicalMapSnapshot) -> Result<()> {
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

    let mut canonical_names = BTreeSet::new();
    for entry in &snapshot.entries {
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
        if !canonical_names.insert(entry.canonical.as_str()) {
            return Err(Error::ParseError(format!(
                "canonical map contains duplicate entry '{}'",
                entry.canonical
            )));
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
    }
    Ok(())
}

/// Atomically replace Remi-owned mappings with one validated snapshot.
///
/// Local Contract rows survive refresh. An identical Remi mapping cannot
/// demote Contract authority, and a conflicting package mapping rolls the
/// entire replacement back.
pub(crate) fn replace_remi_snapshot(
    conn: &Connection,
    snapshot: &CanonicalMapSnapshot,
) -> Result<usize> {
    validate_snapshot(snapshot)?;
    let tx = conn.unchecked_transaction()?;

    tx.execute(
        "DELETE FROM package_implementations WHERE source = 'remi'",
        [],
    )?;

    for entry in &snapshot.entries {
        let mut canonical = CanonicalPackage::new(entry.canonical.clone(), entry.kind.clone());
        canonical.category = entry.category.clone();
        let canonical_id = canonical.insert_or_verify(&tx)?;

        for (profile_id, package_name) in &entry.implementations {
            let mut implementation = PackageImplementation::new(
                canonical_id,
                profile_id.clone(),
                package_name.clone(),
                CanonicalMappingAuthority::Remi,
            );
            implementation.insert_or_verify(&tx)?;
        }
    }

    // Drop stale Remi-only identities only when no installed or repository
    // capability still refers to them.
    tx.execute(
        "DELETE FROM canonical_packages
         WHERE NOT EXISTS (
             SELECT 1 FROM package_implementations
             WHERE package_implementations.canonical_id = canonical_packages.id
         )
         AND NOT EXISTS (
             SELECT 1 FROM repository_packages
             WHERE repository_packages.canonical_id = canonical_packages.id
         )
         AND NOT EXISTS (
             SELECT 1 FROM provides
             WHERE provides.canonical_id = canonical_packages.id
         )",
        [],
    )?;

    tx.commit()?;
    Ok(snapshot.entries.len())
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
    use crate::db::models::CanonicalMappingAuthority;
    use crate::db::schema::ensure_current;

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
                .contains("duplicate entry 'curl'")
        );
    }

    #[test]
    fn exact_contract_survives_identical_remi_snapshot() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_current(&conn).unwrap();
        let mut canonical = CanonicalPackage::new("curl".into(), "package".into());
        let canonical_id = canonical.insert(&conn).unwrap();
        PackageImplementation::new(
            canonical_id,
            "fedora-44".into(),
            "curl".into(),
            CanonicalMappingAuthority::Contract,
        )
        .insert(&conn)
        .unwrap();

        let snapshot = parse_snapshot(
            br#"{
                "schema_version": 1,
                "revision": 1,
                "generated_at": "2026-07-26T00:00:00Z",
                "entries": [{
                    "canonical": "curl",
                    "kind": "package",
                    "implementations": {"fedora-44": "curl"}
                }]
            }"#,
        )
        .unwrap();
        replace_remi_snapshot(&conn, &snapshot).unwrap();

        let stored = PackageImplementation::find_for_distro(&conn, canonical_id, "fedora-44")
            .unwrap()
            .unwrap();
        assert_eq!(stored.source, CanonicalMappingAuthority::Contract);
    }

    #[test]
    fn conflicting_remi_snapshot_rolls_back_previous_remi_rows() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_current(&conn).unwrap();
        let initial = parse_snapshot(
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
                        "canonical": "wget",
                        "kind": "package",
                        "implementations": {"fedora-44": "wget"}
                    }
                ]
            }"#,
        )
        .unwrap();
        replace_remi_snapshot(&conn, &initial).unwrap();

        let canonical = CanonicalPackage::find_by_name(&conn, "curl")
            .unwrap()
            .unwrap();
        let mut local_contract = PackageImplementation::new(
            canonical.id.unwrap(),
            "fedora-44".into(),
            "curl".into(),
            CanonicalMappingAuthority::Contract,
        );
        local_contract.insert_or_verify(&conn).unwrap();

        let conflicting = parse_snapshot(
            br#"{
                "schema_version": 1,
                "revision": 2,
                "generated_at": "2026-07-26T01:00:00Z",
                "entries": [
                    {
                        "canonical": "curl",
                        "kind": "package",
                        "implementations": {"fedora-44": "curl-new"}
                    },
                    {
                        "canonical": "wget",
                        "kind": "package",
                        "implementations": {"fedora-44": "wget-new"}
                    }
                ]
            }"#,
        )
        .unwrap();
        let error = replace_remi_snapshot(&conn, &conflicting).unwrap_err();
        assert!(error.to_string().contains("not incoming 'curl-new'"));

        let fedora =
            PackageImplementation::find_for_distro(&conn, canonical.id.unwrap(), "fedora-44")
                .unwrap()
                .unwrap();
        assert_eq!(fedora.distro_name, "curl");
        assert_eq!(fedora.source, CanonicalMappingAuthority::Contract);

        let wget = CanonicalPackage::find_by_name(&conn, "wget")
            .unwrap()
            .unwrap();
        let wget_mapping =
            PackageImplementation::find_for_distro(&conn, wget.id.unwrap(), "fedora-44")
                .unwrap()
                .unwrap();
        assert_eq!(wget_mapping.distro_name, "wget");
        assert_eq!(wget_mapping.source, CanonicalMappingAuthority::Remi);
    }
}
