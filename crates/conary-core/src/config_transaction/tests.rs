// conary-core/src/config_transaction/tests.rs

use super::*;
use crate::payload::{PayloadIdentity, PayloadNode, PayloadTimestamp};
use std::collections::BTreeMap;

const OLD: &str = "old";
const LOCAL: &str = "local";
const NEW: &str = "new";

fn resolved_node(kind: PayloadNodeKind, mode: u32) -> ResolvedPayloadNode {
    ResolvedPayloadNode::from_numeric_source(PayloadNode {
        kind,
        mode,
        user: PayloadIdentity::Numeric { id: 0 },
        group: PayloadIdentity::Numeric { id: 0 },
        mtime: PayloadTimestamp::UNIX_EPOCH,
        xattrs: BTreeMap::new(),
    })
    .unwrap()
}

fn regular(content: &[u8], mode: u32) -> ConfigArtifact {
    ConfigArtifact::regular(
        PayloadContentAuthority {
            sha256: crate::hash::sha256(content),
            size: content.len() as u64,
        },
        resolved_node(
            PayloadNodeKind::Regular {
                hardlink_identity: None,
            },
            mode,
        ),
    )
    .unwrap()
}

#[test]
fn alpm_three_identity_matrix_matches_documented_actions() {
    assert_eq!(
        decide_config_install(ConfigSource::Arch, true, Some(OLD), Some(OLD), OLD),
        ConfigInstallDecision::Install
    );
    assert_eq!(
        decide_config_install(ConfigSource::Arch, true, Some(OLD), Some(OLD), NEW),
        ConfigInstallDecision::Install
    );
    assert_eq!(
        decide_config_install(ConfigSource::Arch, true, Some(OLD), Some(LOCAL), OLD),
        ConfigInstallDecision::Keep
    );
    assert_eq!(
        decide_config_install(ConfigSource::Arch, true, Some(OLD), Some(NEW), NEW),
        ConfigInstallDecision::Install
    );
    assert_eq!(
        decide_config_install(ConfigSource::Arch, true, Some(OLD), Some(LOCAL), NEW),
        ConfigInstallDecision::InstallAlternative(ConfigSuffix::PacNew)
    );
    assert_eq!(
        decide_config_install(ConfigSource::Arch, true, None, Some(LOCAL), NEW),
        ConfigInstallDecision::InstallAlternative(ConfigSuffix::PacNew)
    );
}

#[test]
fn rpm_distinguishes_noreplace_save_and_first_install_backup() {
    assert_eq!(
        decide_config_install(ConfigSource::Rpm, true, Some(OLD), Some(LOCAL), NEW),
        ConfigInstallDecision::InstallAlternative(ConfigSuffix::RpmNew)
    );
    assert_eq!(
        decide_config_install(ConfigSource::Rpm, false, Some(OLD), Some(LOCAL), NEW),
        ConfigInstallDecision::SaveCurrentAndInstall(ConfigSuffix::RpmSave)
    );
    assert_eq!(
        decide_config_install(ConfigSource::Rpm, false, None, Some(LOCAL), NEW),
        ConfigInstallDecision::SaveCurrentAndInstall(ConfigSuffix::RpmOrig)
    );
}

#[test]
fn dpkg_default_keeps_dual_edits_and_user_deletions() {
    assert_eq!(
        decide_config_install(ConfigSource::Deb, true, Some(OLD), Some(LOCAL), NEW),
        ConfigInstallDecision::InstallAlternative(ConfigSuffix::DpkgDist)
    );
    assert_eq!(
        decide_config_install(ConfigSource::Deb, true, Some(OLD), None, OLD),
        ConfigInstallDecision::Keep
    );
    assert_eq!(
        decide_config_install(ConfigSource::Deb, true, Some(OLD), None, NEW),
        ConfigInstallDecision::InstallAlternative(ConfigSuffix::DpkgDist)
    );
}

#[test]
fn dpkg_remove_on_upgrade_removes_pristine_and_saves_modified() {
    assert_eq!(
        decide_deb_remove_on_upgrade(Some(OLD), Some(OLD)),
        ConfigRemovalDecision::Remove
    );
    assert_eq!(
        decide_deb_remove_on_upgrade(Some(OLD), Some(LOCAL)),
        ConfigRemovalDecision::SaveCurrent(ConfigSuffix::DpkgOld)
    );
    assert_eq!(
        decide_deb_remove_on_upgrade(Some(OLD), None),
        ConfigRemovalDecision::Remove
    );
}

#[test]
fn eopkg_conflicting_updates_use_newconfig_and_removal_retains_edits() {
    assert_eq!(
        decide_config_install(ConfigSource::Eopkg, false, Some(OLD), Some(OLD), NEW),
        ConfigInstallDecision::Install
    );
    assert_eq!(
        decide_config_install(ConfigSource::Eopkg, false, Some(OLD), Some(LOCAL), NEW),
        ConfigInstallDecision::InstallAlternative(ConfigSuffix::EopkgNew)
    );
    assert_eq!(ConfigSuffix::EopkgNew.as_str(), ".newconfig");
    assert_eq!(
        decide_config_removal(ConfigSource::Eopkg, false, false, Some(OLD), Some(LOCAL)),
        ConfigRemovalDecision::KeepResidual
    );
    assert_eq!(
        decide_config_removal(ConfigSource::Eopkg, false, true, Some(OLD), Some(LOCAL)),
        ConfigRemovalDecision::Remove
    );
}

#[test]
fn removal_contracts_cover_residuals_purge_and_native_backups() {
    assert_eq!(
        decide_config_removal(ConfigSource::Deb, false, false, Some(OLD), Some(LOCAL)),
        ConfigRemovalDecision::KeepResidual
    );
    assert_eq!(
        decide_config_removal(ConfigSource::Deb, false, true, Some(OLD), Some(LOCAL)),
        ConfigRemovalDecision::Remove
    );
    assert_eq!(
        decide_config_removal(ConfigSource::Arch, false, false, Some(OLD), Some(LOCAL)),
        ConfigRemovalDecision::RotatePacsaveAndSaveCurrent
    );
}

#[test]
fn durable_artifact_round_trip_validates_content_identity() {
    let artifact = regular(b"local config", 0o100640);
    let encoded = serde_json::to_string(&artifact).unwrap();
    let decoded: ConfigArtifact = serde_json::from_str(&encoded).unwrap();
    assert_eq!(
        decoded.content_authority().unwrap(),
        &PayloadContentAuthority {
            sha256: crate::hash::sha256(b"local config"),
            size: 12,
        }
    );
    assert!(!encoded.contains("content_base64"));
    assert_eq!(decoded.node().source.mode, 0o100640);
}

#[test]
fn etc_payload_contract_accepts_only_regular_files_and_symlinks() {
    assert!(is_etc_config_payload(
        "/etc/implicit.conf",
        &PayloadNodeKind::Regular {
            hardlink_identity: None
        }
    ));
    assert!(is_etc_config_payload(
        "/etc/implicit-link",
        &PayloadNodeKind::Symlink {
            target: "target".to_string()
        }
    ));
    assert!(!is_etc_config_payload(
        "/etc/directory",
        &PayloadNodeKind::Directory
    ));
    assert!(!is_etc_config_payload("/etc/fifo", &PayloadNodeKind::Fifo));
    assert!(!is_etc_config_payload(
        "/usr/lib/implicit.conf",
        &PayloadNodeKind::Regular {
            hardlink_identity: None
        }
    ));
}

#[test]
fn durable_transaction_rejects_unowned_auxiliary_paths() {
    let artifact = regular(b"new", 0o100640);
    let transaction = GenerationConfigTransaction {
        entries: vec![ConfigPathTransaction {
            path: "/etc/demo.conf".to_string(),
            operation: ConfigTransactionOperation::Install,
            before: None,
            current: None,
            after: Some(ConfigPackageState {
                source: ConfigSource::Auto,
                noreplace: false,
                ghost: false,
                materialized: true,
                original_sha256: Some(artifact.sha256().to_string()),
                artifact: Some(artifact.clone()),
            }),
            auxiliaries: vec![("/etc/other.rpmnew".to_string(), artifact)],
        }],
        ..Default::default()
    };

    assert!(transaction.validate().is_err());
}

#[test]
fn restore_snapshot_is_a_v5_exact_prechange_contract() {
    let old = regular(b"local", 0o100600);
    let saved = regular(b"saved", 0o100640);
    let new = regular(b"new", 0o100644);
    let forward = GenerationConfigTransaction {
        entries: vec![ConfigPathTransaction {
            path: "/etc/demo.conf".to_string(),
            operation: ConfigTransactionOperation::Install,
            before: Some(ConfigPackageState {
                source: ConfigSource::Arch,
                noreplace: true,
                ghost: false,
                materialized: true,
                original_sha256: Some(crate::hash::sha256(b"old")),
                artifact: None,
            }),
            current: Some(old.clone()),
            after: Some(ConfigPackageState {
                source: ConfigSource::Arch,
                noreplace: true,
                ghost: false,
                materialized: true,
                original_sha256: Some(new.sha256().to_string()),
                artifact: Some(new),
            }),
            auxiliaries: vec![("/etc/demo.conf.pacsave".to_string(), saved.clone())],
        }],
        ..Default::default()
    };

    let restore = forward.restore_snapshot().unwrap();

    assert_eq!(restore.schema_version, 5);
    restore.validate_restore_snapshot().unwrap();
    assert_eq!(
        restore.entries[0].operation,
        ConfigTransactionOperation::Restore
    );
    assert_eq!(restore.entries[0].current.as_ref(), Some(&old));
    assert_eq!(
        restore.entries[0].auxiliaries,
        vec![("/etc/demo.conf.pacsave".to_string(), saved)]
    );
    assert!(restore.entries[0].after.is_none());
}

#[test]
fn remove_and_purge_require_exact_prior_package_state() {
    for operation in [
        ConfigTransactionOperation::Remove,
        ConfigTransactionOperation::Purge,
    ] {
        let transaction = GenerationConfigTransaction {
            entries: vec![ConfigPathTransaction {
                path: "/etc/demo.conf".to_string(),
                operation,
                before: None,
                current: None,
                after: None,
                auxiliaries: Vec::new(),
            }],
            ..Default::default()
        };

        let error = transaction.validate().unwrap_err().to_string();
        assert!(error.contains("no exact prior package state"), "{error}");
    }
}

#[test]
fn restore_owned_paths_cover_exact_suffix_grammar_and_pacsave_rotation() {
    let entry = ConfigPathTransaction {
        path: "/etc/demo.conf".to_string(),
        operation: ConfigTransactionOperation::Restore,
        before: None,
        current: None,
        after: None,
        auxiliaries: vec![
            (
                "/etc/demo.conf.pacsave".to_string(),
                regular(b"zero", 0o100600),
            ),
            (
                "/etc/demo.conf.pacsave.4".to_string(),
                regular(b"four", 0o100600),
            ),
        ],
    };

    let paths = entry.restore_owned_paths();

    for expected in [
        "/etc/demo.conf",
        "/etc/demo.conf.rpmnew",
        "/etc/demo.conf.dpkg-dist",
        "/etc/demo.conf.pacsave",
        "/etc/demo.conf.pacsave.1",
        "/etc/demo.conf.pacsave.4",
        "/etc/demo.conf.pacsave.5",
        "/etc/demo.conf.conary-save",
    ] {
        assert!(paths.iter().any(|path| path == expected), "{expected}");
    }
}

#[test]
fn schema_v5_rejects_unknown_fields_at_every_contract_layer() {
    let valid = serde_json::json!({
        "schema_version": 5,
        "entries": [{
            "path": "/etc/demo.conf",
            "operation": "install",
            "before": null,
            "current": null,
            "after": {
                "source": "rpm",
                "noreplace": false,
                "ghost": true,
                "materialized": false,
                "original_sha256": null,
                "artifact": null
            },
            "auxiliaries": []
        }]
    });
    serde_json::from_value::<GenerationConfigTransaction>(valid.clone()).unwrap();

    let mut top_level = valid.clone();
    top_level["legacy"] = serde_json::json!(true);
    assert!(
        serde_json::from_value::<GenerationConfigTransaction>(top_level)
            .unwrap_err()
            .to_string()
            .contains("unknown field")
    );

    let mut entry = valid.clone();
    entry["entries"][0]["legacy"] = serde_json::json!(true);
    assert!(
        serde_json::from_value::<GenerationConfigTransaction>(entry)
            .unwrap_err()
            .to_string()
            .contains("unknown field")
    );

    let mut state = valid;
    state["entries"][0]["after"]["legacy"] = serde_json::json!(true);
    assert!(
        serde_json::from_value::<GenerationConfigTransaction>(state)
            .unwrap_err()
            .to_string()
            .contains("unknown field")
    );
}

#[test]
fn remove_on_upgrade_requires_exact_non_ghost_debian_prior_state() {
    let deb_state = ConfigPackageState {
        source: ConfigSource::Deb,
        noreplace: true,
        ghost: false,
        materialized: true,
        original_sha256: Some(crate::hash::sha256(b"old")),
        artifact: None,
    };
    let transaction = |before| GenerationConfigTransaction {
        entries: vec![ConfigPathTransaction {
            path: "/etc/demo.conf".to_string(),
            operation: ConfigTransactionOperation::RemoveOnUpgrade,
            before,
            current: None,
            after: None,
            auxiliaries: Vec::new(),
        }],
        ..Default::default()
    };

    let missing = transaction(None).validate().unwrap_err().to_string();
    assert!(
        missing.contains("no exact prior package state"),
        "{missing}"
    );

    let mut rpm_state = deb_state.clone();
    rpm_state.source = ConfigSource::Rpm;
    let non_debian = transaction(Some(rpm_state))
        .validate()
        .unwrap_err()
        .to_string();
    assert!(
        non_debian.contains("non-Debian prior source"),
        "{non_debian}"
    );

    let mut incoherent_ghost = deb_state;
    incoherent_ghost.ghost = true;
    let ghost = transaction(Some(incoherent_ghost))
        .validate()
        .unwrap_err()
        .to_string();
    assert!(ghost.contains("ghost config transaction before"), "{ghost}");
}

#[test]
fn non_ghost_prior_state_requires_canonical_sha256_after_deserialization() {
    let transaction = |original_sha256: serde_json::Value| {
        serde_json::from_value::<GenerationConfigTransaction>(serde_json::json!({
            "schema_version": 5,
            "entries": [{
                "path": "/etc/demo.conf",
                "operation": "remove",
                "before": {
                    "source": "rpm",
                    "noreplace": false,
                    "ghost": false,
                    "materialized": true,
                    "original_sha256": original_sha256,
                    "artifact": null
                },
                "current": null,
                "after": null,
                "auxiliaries": []
            }]
        }))
        .unwrap()
    };

    let missing = transaction(serde_json::Value::Null)
        .validate()
        .unwrap_err()
        .to_string();
    assert!(missing.contains("no exact package SHA-256"), "{missing}");

    for malformed in ["not-a-hash", &"A".repeat(64), &"g".repeat(64)] {
        let error = transaction(serde_json::json!(malformed))
            .validate()
            .unwrap_err()
            .to_string();
        assert!(error.contains("lowercase raw 64-hex SHA-256"), "{error}");
    }
}

#[test]
fn numbered_pacsave_suffix_requires_canonical_positive_decimal() {
    assert_eq!(parse_canonical_positive_decimal("1"), Some(1));
    assert_eq!(
        parse_canonical_positive_decimal(&u64::MAX.to_string()),
        Some(u64::MAX)
    );
    for invalid in ["", "0", "00", "01", "+1", "-1", " 1"] {
        assert_eq!(parse_canonical_positive_decimal(invalid), None, "{invalid}");
        let transaction = GenerationConfigTransaction {
            entries: vec![ConfigPathTransaction {
                path: "/etc/demo.conf".to_string(),
                operation: ConfigTransactionOperation::Restore,
                before: None,
                current: None,
                after: None,
                auxiliaries: vec![(
                    format!("/etc/demo.conf.pacsave.{invalid}"),
                    regular(b"saved", 0o100600),
                )],
            }],
            ..Default::default()
        };
        assert!(transaction.validate().is_err(), "{invalid}");
    }
}

#[test]
fn retired_schema_is_rejected_unconditionally() {
    let transaction = GenerationConfigTransaction {
        schema_version: 1,
        entries: Vec::new(),
    };
    assert!(transaction.validate().is_err());
}
