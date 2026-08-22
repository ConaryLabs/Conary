// crates/conary-core/src/repository/universe/enrollment.rs

//! Independent metadata-root enrollment for one Remi universe endpoint.

use rusqlite::{Connection, OptionalExtension, params};

use crate::error::{Error, Result};
use crate::trust::verify::{extract_role_keys, verify_not_expired, verify_root};
use crate::trust::{Role, RootMetadata, Signed};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemiUniverseEnrollmentOutcome {
    Enrolled,
    Unchanged,
}

pub fn normalize_remi_endpoint(endpoint: &str) -> Result<String> {
    let parsed = url::Url::parse(endpoint)
        .map_err(|error| Error::ConfigError(format!("invalid Remi endpoint: {error}")))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(Error::ConfigError(
            "Remi endpoint must use HTTP or HTTPS".to_string(),
        ));
    }
    parsed
        .host_str()
        .ok_or_else(|| Error::ConfigError("Remi endpoint has no host".to_string()))?;
    if !parsed.username().is_empty()
        || parsed.password().is_some()
        || !matches!(parsed.path(), "" | "/")
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(Error::ConfigError(
            "Remi endpoint must be an origin URL without credentials, path, query, or fragment"
                .to_string(),
        ));
    }
    Ok(parsed.origin().ascii_serialization())
}

pub fn enroll_remi_universe_root(
    conn: &Connection,
    endpoint: &str,
    root_bytes: &[u8],
) -> Result<RemiUniverseEnrollmentOutcome> {
    let endpoint = normalize_remi_endpoint(endpoint)?;
    let root: Signed<RootMetadata> = serde_json::from_slice(root_bytes)?;
    if root.signed.type_field != "root" {
        return Err(Error::TrustError(format!(
            "universe metadata root has type '{}' instead of 'root'",
            root.signed.type_field
        )));
    }
    let (root_keys, root_threshold) = extract_role_keys(&root.signed, Role::Root)
        .map_err(|error| Error::TrustError(error.to_string()))?;
    verify_root(&root, &root_keys, root_threshold)
        .map_err(|error| Error::TrustError(error.to_string()))?;
    verify_not_expired(Role::Root, &root.signed.expires)
        .map_err(|error| Error::TrustError(error.to_string()))?;
    let canonical = crate::json::canonical_json(&root).map_err(Error::ParseError)?;
    let root_sha256 = crate::hash::sha256(&canonical);
    let root_json = String::from_utf8(canonical)
        .map_err(|error| Error::ParseError(format!("metadata root is not UTF-8: {error}")))?;
    let root_version = i64::try_from(root.signed.version).map_err(|_| {
        Error::ConfigError("universe metadata-root version exceeds SQLite range".to_string())
    })?;

    let existing = conn
        .query_row(
            "SELECT trusted_root_sha256, trusted_root_json, root_version
             FROM remi_client_universe_trust WHERE endpoint = ?1",
            [&endpoint],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()?;
    if let Some((stored_sha256, stored_json, stored_version)) = existing {
        if stored_sha256 == root_sha256
            && stored_json == root_json
            && stored_version == root_version
        {
            return Ok(RemiUniverseEnrollmentOutcome::Unchanged);
        }
        return Err(Error::ConflictError(format!(
            "Remi endpoint {endpoint} already has a different enrolled metadata root; rotate through signed TUF root metadata"
        )));
    }
    conn.execute(
        "INSERT INTO remi_client_universe_trust (
             endpoint, trusted_root_sha256, trusted_root_json, root_version, fencing_epoch
         ) VALUES (?1, ?2, ?3, ?4, 0)",
        params![endpoint, root_sha256, root_json, root_version],
    )?;
    Ok(RemiUniverseEnrollmentOutcome::Enrolled)
}

#[cfg(test)]
mod tests {
    use crate::ccs::signing::SigningKeyPair;
    use crate::trust::ceremony::create_initial_root;

    use super::*;

    fn root() -> Vec<u8> {
        let root = SigningKeyPair::generate();
        let targets = SigningKeyPair::generate();
        let snapshot = SigningKeyPair::generate();
        let timestamp = SigningKeyPair::generate();
        let signed = create_initial_root(&root, &targets, &snapshot, &timestamp, 30).unwrap();
        crate::json::canonical_json(&signed).unwrap()
    }

    #[test]
    fn enrollment_is_exact_idempotent_and_rejects_replacement() {
        let (_file, conn) = crate::db::testing::create_test_db();
        let first = root();
        assert_eq!(
            enroll_remi_universe_root(&conn, "https://EXAMPLE.test/", &first).unwrap(),
            RemiUniverseEnrollmentOutcome::Enrolled
        );
        assert_eq!(
            enroll_remi_universe_root(&conn, "https://example.test", &first).unwrap(),
            RemiUniverseEnrollmentOutcome::Unchanged
        );
        let error = enroll_remi_universe_root(&conn, "https://example.test", &root()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("different enrolled metadata root")
        );
    }

    #[test]
    fn endpoint_identity_rejects_paths_credentials_and_non_http_schemes() {
        assert_eq!(
            normalize_remi_endpoint("https://EXAMPLE.test:443/").unwrap(),
            "https://example.test"
        );
        for endpoint in [
            "file:///tmp/remi",
            "https://user@example.test",
            "https://example.test/v1",
            "https://example.test?root=1",
        ] {
            assert!(normalize_remi_endpoint(endpoint).is_err(), "{endpoint}");
        }
    }
}
