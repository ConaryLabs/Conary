// apps/remi/src/server/runtime_session.rs

//! Durable reader ownership installed only after the runtime-root lock.

use std::path::Path;

use anyhow::{Context, Result};
use conary_core::db::models::RemiRuntimeSession;

use super::open_runtime_db;

/// Replace the prior durable runtime session and recover only its reader pins.
/// The caller owns the canonical runtime-root kernel lock before entering this
/// boundary.
pub(super) fn begin(db_path: &Path) -> Result<RemiRuntimeSession> {
    let conn = open_runtime_db(db_path)?;
    RemiRuntimeSession::begin(&conn, unix_seconds()?).map_err(Into::into)
}

fn unix_seconds() -> Result<i64> {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("system time precedes Unix epoch")?
        .as_secs();
    i64::try_from(seconds).context("system time exceeds SQLite integer range")
}

#[cfg(test)]
mod tests {
    use conary_core::db::models::{
        RemiCatalogResource, RemiCatalogResourceKind, RemiProfileRevisionPin, RemiRevisionPinKind,
        RemiRuntimeSession,
    };

    use super::super::{RemiConfig, open_runtime_db, prepare_runtime_storage};

    #[test]
    fn successor_runtime_recovers_only_prior_session_reader_pins() {
        let temp = tempfile::tempdir().expect("create tempdir");
        let mut remi_config = RemiConfig::default();
        remi_config.storage.root = temp.path().join("runtime");
        let server_config = remi_config.to_server_config().expect("server config");
        let first_owner = prepare_runtime_storage(&remi_config, &server_config)
            .expect("prepare first runtime owner");
        let conn = open_runtime_db(&server_config.db_path).expect("open runtime database");
        let first_session = RemiRuntimeSession::current(&conn)
            .expect("read first session")
            .expect("first session exists");
        let manifest_json = r#"{"resource":"runtime-owner-test"}"#;
        let profile_revision_sha256 = conary_core::hash::sha256(manifest_json.as_bytes());
        RemiCatalogResource {
            resource_sha256: profile_revision_sha256.clone(),
            kind: RemiCatalogResourceKind::ProfileRevision,
            source_profile: "test-profile".to_string(),
            artifact_sha256: "a".repeat(64),
            artifact_size: 1,
            logical_digest_sha256: "b".repeat(64),
            manifest_json: manifest_json.to_string(),
            durable: true,
            created_at: 1,
        }
        .insert(&conn)
        .expect("insert durable test profile resource");
        for (pin_id, owner_kind, runtime_session_id) in [
            (
                "reader-pin",
                RemiRevisionPinKind::Reader,
                Some(first_session.session_id.clone()),
            ),
            ("conversion-pin", RemiRevisionPinKind::Conversion, None),
            ("work-pin", RemiRevisionPinKind::Work, None),
        ] {
            RemiProfileRevisionPin {
                pin_id: pin_id.to_string(),
                source_profile: "test-profile".to_string(),
                profile_revision_sha256: profile_revision_sha256.clone(),
                owner_kind,
                owner_identity: format!("{pin_id}-owner"),
                runtime_session_id,
                pinned_at: 1,
            }
            .insert(&conn)
            .expect("insert exact revision pin");
        }
        drop(conn);
        drop(first_owner);

        let _second_owner = prepare_runtime_storage(&remi_config, &server_config)
            .expect("prepare successor runtime owner");
        let conn = open_runtime_db(&server_config.db_path).expect("reopen runtime database");
        let second_session = RemiRuntimeSession::current(&conn)
            .expect("read second session")
            .expect("second session exists");

        assert_ne!(first_session.session_id, second_session.session_id);
        assert!(
            RemiProfileRevisionPin::find(&conn, "reader-pin")
                .unwrap()
                .is_none()
        );
        assert!(
            RemiProfileRevisionPin::find(&conn, "conversion-pin")
                .unwrap()
                .is_some()
        );
        assert!(
            RemiProfileRevisionPin::find(&conn, "work-pin")
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn runtime_session_rejects_a_database_outside_the_locked_root() {
        let temp = tempfile::tempdir().expect("create tempdir");
        let mut remi_config = RemiConfig::default();
        remi_config.storage.root = temp.path().join("runtime");
        let mut server_config = remi_config.to_server_config().expect("server config");
        server_config.db_path = temp.path().join("shared/conary.db");

        let error = prepare_runtime_storage(&remi_config, &server_config)
            .expect_err("database outside locked root must fail closed");

        assert!(
            error
                .to_string()
                .contains("outside the locked storage-root")
        );
        assert!(!remi_config.storage.root.exists());
        assert!(!server_config.db_path.exists());
    }
}
