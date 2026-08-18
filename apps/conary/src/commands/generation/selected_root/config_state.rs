// apps/conary/src/commands/generation/selected_root/config_state.rs

//! Active generation config-state capture for selected-root transactions.

use anyhow::{Context, Result, bail};
use conary_core::filesystem::CasStore;
use conary_core::generation::root_manifest::{
    CapturedSelectedRoot, SelectedRootOverlayProfile, SelectedRootSnapshot,
    decode_config_state_upper_indexed,
};
use conary_core::runtime_root::ConaryRuntimeRoot;
use std::fs;

pub(super) fn capture_active_upper(
    conn: &rusqlite::Connection,
    runtime_root: &ConaryRuntimeRoot,
    generation: i64,
    prior: SelectedRootSnapshot,
    cas: &CasStore,
) -> Result<Option<(SelectedRootSnapshot, CapturedSelectedRoot)>> {
    let upper = runtime_root.etc_state_dir().join(generation.to_string());
    let metadata = match fs::symlink_metadata(&upper) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_dir() {
        bail!(
            "active generation config-state upper is not a directory: {}",
            upper.display()
        );
    }

    let namespace = format!("generation-{generation}-config-state-v1");
    let delta = decode_config_state_upper_indexed(
        &upper,
        &namespace,
        conn,
        prior,
        cas,
        &SelectedRootOverlayProfile::trusted(),
    )
    .with_context(|| {
        format!("failed to capture active generation {generation} config-state authority")
    })?;
    let snapshot = prior.apply_delta(conn, &delta)?;
    let captured = snapshot.materialize(conn)?;
    Ok(Some((snapshot, captured)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::generation::root_manifest::{
        materialize_captured_selected_root, scan_selected_root,
    };

    #[test]
    fn complete_active_upper_carries_edits_deletions_and_new_files_forward() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("runtime/conary.db");
        fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        conary_core::db::init(&db_path).unwrap();
        let conn = conary_core::db::open(&db_path).unwrap();
        let runtime_root = ConaryRuntimeRoot::from_db_path(&db_path);
        let cas = CasStore::new(runtime_root.objects_dir()).unwrap();

        let prior_root = temp.path().join("prior");
        fs::create_dir_all(prior_root.join("etc/demo")).unwrap();
        fs::create_dir_all(prior_root.join("var/lib/demo")).unwrap();
        fs::write(prior_root.join("etc/demo/local.conf"), b"vendor\n").unwrap();
        fs::write(prior_root.join("etc/demo/deleted.conf"), b"remove me\n").unwrap();
        fs::write(prior_root.join("var/lib/demo/state"), b"unchanged\n").unwrap();
        let prior = scan_selected_root(&prior_root, &cas).unwrap();
        let snapshot = SelectedRootSnapshot::capture(&conn, &prior).unwrap();

        let upper = runtime_root.etc_state_dir().join("7");
        fs::create_dir_all(upper.join("demo")).unwrap();
        fs::write(upper.join("demo/local.conf"), b"locally edited\n").unwrap();
        fs::write(upper.join("demo/unmatched.conf"), b"user created\n").unwrap();
        crate::commands::generation::config_transaction::create_whiteout(
            &upper.join("demo/deleted.conf"),
        )
        .unwrap();

        let (_, captured) = capture_active_upper(&conn, &runtime_root, 7, snapshot, &cas)
            .unwrap()
            .unwrap();
        let materialized = temp.path().join("materialized");
        materialize_captured_selected_root(&captured, &cas, &materialized).unwrap();

        assert_eq!(
            fs::read(materialized.join("etc/demo/local.conf")).unwrap(),
            b"locally edited\n"
        );
        assert!(!materialized.join("etc/demo/deleted.conf").exists());
        assert_eq!(
            fs::read(materialized.join("etc/demo/unmatched.conf")).unwrap(),
            b"user created\n"
        );
        assert_eq!(
            fs::read(materialized.join("var/lib/demo/state")).unwrap(),
            b"unchanged\n"
        );
    }
}
