// conary-core/src/db/models/converted/tests.rs

use super::*;
use crate::ccs::convert::ScriptletBundleSummary;
use crate::db::testing::create_test_db;

#[test]
fn converted_package_defaults_scriptlet_metadata() {
    let converted = ConvertedPackage::new("rpm".to_string(), "sha256:source".to_string());

    assert_eq!(converted.scriptlet_fidelity, "unknown");
    assert_eq!(converted.target_compatibility, "unknown");
    assert_eq!(converted.publication_status, "public");
    assert_eq!(converted.blocked_reason_codes_json, "[]");
    assert_eq!(converted.scriptlet_summary_json, "{}");
    assert_eq!(converted.review_artifact_path, None);
}

#[test]
fn converted_package_round_trips_scriptlet_metadata() {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::schema::migrate(&conn).unwrap();
    let mut converted = ConvertedPackage::new_server(
        "fedora".to_string(),
        "gtk3".to_string(),
        "3.24.0-1.fc44".to_string(),
        "rpm".to_string(),
        "sha256:source".to_string(),
        &["sha256:chunk".to_string()],
        42,
        "sha256:content".to_string(),
        "/tmp/gtk3.ccs".to_string(),
    );
    let summary = ScriptletBundleSummary {
        scriptlet_fidelity: "review-required".to_string(),
        target_compatibility: "review-required".to_string(),
        publication_status: "private-review".to_string(),
        evidence_digest: Some(crate::hash::sha256_prefixed(b"evidence")),
        blocked_reason_codes: vec!["blocked-class-network".to_string()],
        review_reason_codes: vec!["review-class-debconf".to_string()],
        unknown_command_evidence: vec![crate::ccs::legacy_scriptlets::UnknownCommandEvidence {
            command: "custom-helper".to_string(),
            argv: vec!["--do-it".to_string()],
            phase: Some("post-install".to_string()),
            lifecycle_paths: vec!["post-install".to_string()],
            source: crate::ccs::legacy_scriptlets::CommandEvidenceSource::ShellAst,
            environment: Vec::new(),
            ..crate::ccs::legacy_scriptlets::UnknownCommandEvidence::default()
        }],
        blocked_classes: vec!["network".to_string()],
        ..ScriptletBundleSummary::default()
    };
    converted.set_scriptlet_metadata(&summary).unwrap();
    converted.insert(&conn).unwrap();

    let found = ConvertedPackage::find_by_package_identity_with_arch(
        &conn,
        "fedora",
        "gtk3",
        Some("3.24.0-1.fc44"),
        None,
    )
    .unwrap()
    .unwrap();

    assert_eq!(found.scriptlet_fidelity, "review-required");
    assert_eq!(found.target_compatibility, "review-required");
    assert_eq!(found.publication_status, "private-review");
    assert_eq!(
        found.blocked_reason_codes_json,
        "[\"blocked-class-network\"]"
    );
    assert!(found.scriptlet_summary_json.contains("custom-helper"));
}

#[test]
fn scriptlet_summary_recovers_from_malformed_json_with_scalar_fields() {
    let mut converted = ConvertedPackage::new("rpm".to_string(), "sha256:source".to_string());
    converted.scriptlet_fidelity = "blocked".to_string();
    converted.target_compatibility = "blocked".to_string();
    converted.publication_status = "blocked".to_string();
    converted.evidence_digest = Some(crate::hash::sha256_prefixed(b"fallback-evidence"));
    converted.blocked_reason_codes_json = "[\"blocked-class-network\"]".to_string();
    converted.scriptlet_summary_json = "{not valid json".to_string();

    let summary = converted.scriptlet_summary();

    assert_eq!(summary.scriptlet_fidelity, "blocked");
    assert_eq!(summary.target_compatibility, "blocked");
    assert_eq!(summary.publication_status, "blocked");
    assert_eq!(
        summary.evidence_digest,
        Some(crate::hash::sha256_prefixed(b"fallback-evidence"))
    );
    assert_eq!(summary.blocked_reason_codes, vec!["blocked-class-network"]);
    assert!(summary.review_reason_codes.is_empty());
    assert!(summary.unknown_command_evidence.is_empty());
}

#[test]
fn scriptlet_summary_for_publication_accepts_constructor_default_shape() {
    let converted = ConvertedPackage::new_server(
        "fedora".to_string(),
        "plain".to_string(),
        "1.0".to_string(),
        "ccs".to_string(),
        "upload:fedora:abc".to_string(),
        &["abc".to_string()],
        3,
        "abc".to_string(),
        "/tmp/plain.ccs".to_string(),
    );

    let publication = converted.scriptlet_summary_for_publication();

    assert!(publication.valid);
    assert_eq!(publication.summary.publication_status, "public");
    assert!(converted.is_scriptlet_public_ready());
}

#[test]
fn stale_converted_rows_are_not_scriptlet_public_ready() {
    let mut converted = ConvertedPackage::new_server(
        "fedora".to_string(),
        "stale".to_string(),
        "1.0".to_string(),
        "rpm".to_string(),
        "sha256:source".to_string(),
        &["sha256:chunk".to_string()],
        42,
        "sha256:content".to_string(),
        "/tmp/stale.ccs".to_string(),
    );
    let summary = ScriptletBundleSummary {
        scriptlet_fidelity: "fully-replaced".to_string(),
        target_compatibility: "conary-portable".to_string(),
        publication_status: "public".to_string(),
        evidence_digest: Some(crate::hash::sha256_prefixed(b"evidence")),
        decision_counts: crate::ccs::convert::ScriptletDecisionCountsSummary {
            replaced: 1,
            ..Default::default()
        },
        ..ScriptletBundleSummary::default()
    };
    converted.set_scriptlet_metadata(&summary).unwrap();
    converted.conversion_version = CONVERSION_VERSION - 1;

    assert!(converted.needs_reconversion());
    assert!(!converted.is_scriptlet_public_ready());
}

#[test]
fn non_default_publication_summary_requires_security_policy_intents() {
    let mut converted = ConvertedPackage::new("rpm".to_string(), "sha256:source".to_string());
    converted.scriptlet_fidelity = "fully-replaced".to_string();
    converted.target_compatibility = "conary-portable".to_string();
    converted.publication_status = "public".to_string();
    converted.evidence_digest = Some(crate::hash::sha256_prefixed(b"evidence"));
    converted.scriptlet_summary_json = serde_json::json!({
        "scriptlet_fidelity": "fully-replaced",
        "target_compatibility": "conary-portable",
        "publication_status": "public",
        "decision_counts": {
            "replaced": 1,
            "legacy": 0,
            "blocked": 0,
            "review": 0
        },
        "blocked_reason_codes": [],
        "review_reason_codes": [],
        "unknown_command_evidence": [],
        "blocked_classes": [],
        "boot_security_intents": []
    })
    .to_string();

    assert!(!converted.scriptlet_summary_for_publication().valid);
    assert!(!converted.is_scriptlet_public_ready());
}

#[test]
fn older_non_default_summary_without_security_policy_intents_is_stale_and_not_public_ready() {
    let mut converted = ConvertedPackage::new_server(
        "fedora".to_string(),
        "stale-policy".to_string(),
        "1.0".to_string(),
        "rpm".to_string(),
        "sha256:source".to_string(),
        &["sha256:chunk".to_string()],
        42,
        "sha256:content".to_string(),
        "/tmp/stale-policy.ccs".to_string(),
    );
    converted.scriptlet_fidelity = "fully-replaced".to_string();
    converted.target_compatibility = "conary-portable".to_string();
    converted.publication_status = "public".to_string();
    converted.evidence_digest = Some(crate::hash::sha256_prefixed(b"evidence"));
    converted.conversion_version = CONVERSION_VERSION - 1;
    converted.scriptlet_summary_json = serde_json::json!({
        "scriptlet_fidelity": "fully-replaced",
        "target_compatibility": "conary-portable",
        "publication_status": "public",
        "evidence_digest": crate::hash::sha256_prefixed(b"evidence"),
        "decision_counts": {
            "replaced": 1,
            "legacy": 0,
            "blocked": 0,
            "review": 0
        },
        "blocked_reason_codes": [],
        "review_reason_codes": [],
        "unknown_command_evidence": [],
        "blocked_classes": [],
        "boot_security_intents": []
    })
    .to_string();

    let publication = converted.scriptlet_summary_for_publication();

    assert!(!publication.valid);
    assert!(converted.needs_reconversion());
    assert!(!converted.is_scriptlet_public_ready());
}

#[test]
fn non_default_publication_summary_accepts_security_policy_intents() {
    let mut converted = ConvertedPackage::new("rpm".to_string(), "sha256:source".to_string());
    let summary = ScriptletBundleSummary {
        scriptlet_fidelity: "fully-replaced".to_string(),
        target_compatibility: "conary-portable".to_string(),
        publication_status: "public".to_string(),
        evidence_digest: Some(crate::hash::sha256_prefixed(b"evidence")),
        decision_counts: crate::ccs::convert::ScriptletDecisionCountsSummary {
            replaced: 1,
            ..Default::default()
        },
        ..ScriptletBundleSummary::default()
    };
    converted.set_scriptlet_metadata(&summary).unwrap();

    assert!(converted.scriptlet_summary_for_publication().valid);
    assert!(converted.is_scriptlet_public_ready());
}

#[test]
fn scriptlet_summary_for_publication_rejects_default_json_with_scriptlet_evidence() {
    let mut converted = ConvertedPackage::new("rpm".to_string(), "sha256:source".to_string());
    converted.scriptlet_fidelity = "blocked".to_string();
    converted.target_compatibility = "blocked".to_string();
    converted.publication_status = "public".to_string();
    converted.evidence_digest = Some(crate::hash::sha256_prefixed(b"evidence"));
    converted.scriptlet_summary_json = "{}".to_string();

    let publication = converted.scriptlet_summary_for_publication();

    assert!(!publication.valid);
    assert!(!converted.is_scriptlet_public_ready());
}

#[test]
fn scriptlet_summary_for_publication_rejects_partial_and_malformed_json() {
    let mut converted = ConvertedPackage::new("rpm".to_string(), "sha256:source".to_string());
    converted.scriptlet_summary_json = r#"{"publication_status":"public"}"#.to_string();
    assert!(!converted.scriptlet_summary_for_publication().valid);

    converted.scriptlet_summary_json = "{not valid json".to_string();
    assert!(!converted.scriptlet_summary_for_publication().valid);
}

#[test]
fn malformed_typed_summary_json_is_not_valid_for_publication() {
    let mut converted = ConvertedPackage::new("rpm".to_string(), "sha256:source".to_string());
    converted.scriptlet_summary_json = serde_json::json!({
        "scriptlet_fidelity": "fully-replaced",
        "target_compatibility": "conary-portable",
        "publication_status": "public",
        "decision_counts": "bad",
        "blocked_reason_codes": [],
        "review_reason_codes": [],
        "unknown_command_evidence": [],
        "blocked_classes": [],
        "boot_security_intents": [],
        "security_policy_intents": []
    })
    .to_string();

    let publication = converted.scriptlet_summary_for_publication();

    assert!(!publication.valid);
    assert!(!converted.is_scriptlet_public_ready());
}

#[test]
fn scriptlet_public_ready_requires_valid_summary_and_public_status() {
    let mut converted = ConvertedPackage::new("rpm".to_string(), "sha256:source".to_string());
    let summary = ScriptletBundleSummary {
        scriptlet_fidelity: "review-required".to_string(),
        target_compatibility: "review-required".to_string(),
        publication_status: "private-review".to_string(),
        review_reason_codes: vec!["review-class-debconf".to_string()],
        ..ScriptletBundleSummary::default()
    };
    converted.set_scriptlet_metadata(&summary).unwrap();

    assert!(converted.scriptlet_summary_for_publication().valid);
    assert!(!converted.is_scriptlet_public_ready());
}

#[test]
fn chunk_public_ready_lookup_requires_at_least_one_public_row() {
    let (_temp, conn) = create_test_db();
    let shared_hash = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let mut private = ConvertedPackage::new_server(
        "fedora".to_string(),
        "private".to_string(),
        "1.0".to_string(),
        "rpm".to_string(),
        "sha256:private".to_string(),
        &[shared_hash.to_string()],
        10,
        "sha256:private-content".to_string(),
        "/tmp/private.ccs".to_string(),
    );
    private
        .set_scriptlet_metadata(&ScriptletBundleSummary {
            publication_status: "private-review".to_string(),
            scriptlet_fidelity: "review-required".to_string(),
            target_compatibility: "review-required".to_string(),
            review_reason_codes: vec!["review-class-debconf".to_string()],
            ..Default::default()
        })
        .unwrap();
    private.insert(&conn).unwrap();

    assert_eq!(
        ConvertedPackage::chunk_publication_state(&conn, shared_hash).unwrap(),
        ChunkPublicationState::NonPublicOnly,
    );

    let mut public = ConvertedPackage::new_server(
        "fedora".to_string(),
        "public".to_string(),
        "1.0".to_string(),
        "rpm".to_string(),
        "sha256:public".to_string(),
        &[shared_hash.to_string()],
        10,
        "sha256:public-content".to_string(),
        "/tmp/public.ccs".to_string(),
    );
    public
        .set_scriptlet_metadata(&ScriptletBundleSummary::default())
        .unwrap();
    public.insert(&conn).unwrap();

    assert_eq!(
        ConvertedPackage::chunk_publication_state(&conn, shared_hash).unwrap(),
        ChunkPublicationState::PublicReady,
    );
}

#[test]
fn chunk_publication_state_rejects_malformed_typed_summary_json() {
    let (_temp, conn) = create_test_db();
    let shared_hash = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
    let mut malformed = ConvertedPackage::new_server(
        "fedora".to_string(),
        "malformed-public-looking".to_string(),
        "1.0".to_string(),
        "rpm".to_string(),
        "sha256:malformed-public-looking".to_string(),
        &[shared_hash.to_string()],
        10,
        "sha256:malformed-public-looking-content".to_string(),
        "/tmp/malformed-public-looking.ccs".to_string(),
    );
    malformed.scriptlet_summary_json = serde_json::json!({
        "scriptlet_fidelity": "fully-replaced",
        "target_compatibility": "conary-portable",
        "publication_status": "public",
        "decision_counts": {
            "replaced": 1,
            "legacy": 0,
            "blocked": 0,
            "review": 0
        },
        "blocked_reason_codes": [],
        "review_reason_codes": [],
        "unknown_command_evidence": [],
        "blocked_classes": [],
        "boot_security_intents": [],
        "security_policy_intents": "bad"
    })
    .to_string();
    malformed.insert(&conn).unwrap();

    assert_eq!(
        ConvertedPackage::chunk_publication_state(&conn, shared_hash).unwrap(),
        ChunkPublicationState::NonPublicOnly,
    );
}

#[test]
fn chunk_publication_state_treats_stale_only_references_as_non_public() {
    let (_temp, conn) = create_test_db();
    let shared_hash = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    let mut stale = ConvertedPackage::new_server(
        "fedora".to_string(),
        "stale-public-looking".to_string(),
        "1.0".to_string(),
        "rpm".to_string(),
        "sha256:stale-public-looking".to_string(),
        &[format!("sha256:{shared_hash}")],
        10,
        "sha256:stale-public-looking-content".to_string(),
        "/tmp/stale-public-looking.ccs".to_string(),
    );
    stale
        .set_scriptlet_metadata(&ScriptletBundleSummary {
            scriptlet_fidelity: "fully-replaced".to_string(),
            target_compatibility: "conary-portable".to_string(),
            publication_status: "public".to_string(),
            evidence_digest: Some(crate::hash::sha256_prefixed(b"stale-evidence")),
            decision_counts: crate::ccs::convert::ScriptletDecisionCountsSummary {
                replaced: 1,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
    stale.conversion_version = CONVERSION_VERSION - 1;
    stale.insert(&conn).unwrap();

    assert_eq!(
        ConvertedPackage::chunk_publication_state(&conn, shared_hash).unwrap(),
        ChunkPublicationState::NonPublicOnly,
    );
}

#[test]
fn chunk_publication_state_keeps_current_public_when_stale_also_references_chunk() {
    let (_temp, conn) = create_test_db();
    let shared_hash = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
    let mut stale = ConvertedPackage::new_server(
        "fedora".to_string(),
        "stale-public-looking".to_string(),
        "1.0".to_string(),
        "rpm".to_string(),
        "sha256:stale-shared".to_string(),
        &[shared_hash.to_string()],
        10,
        "sha256:stale-shared-content".to_string(),
        "/tmp/stale-shared.ccs".to_string(),
    );
    stale
        .set_scriptlet_metadata(&ScriptletBundleSummary {
            scriptlet_fidelity: "fully-replaced".to_string(),
            target_compatibility: "conary-portable".to_string(),
            publication_status: "public".to_string(),
            evidence_digest: Some(crate::hash::sha256_prefixed(b"stale-shared-evidence")),
            decision_counts: crate::ccs::convert::ScriptletDecisionCountsSummary {
                replaced: 1,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
    stale.conversion_version = CONVERSION_VERSION - 1;
    stale.insert(&conn).unwrap();

    let mut public = ConvertedPackage::new_server(
        "fedora".to_string(),
        "current-public".to_string(),
        "1.0".to_string(),
        "rpm".to_string(),
        "sha256:current-public".to_string(),
        &[shared_hash.to_string()],
        10,
        "sha256:current-public-content".to_string(),
        "/tmp/current-public.ccs".to_string(),
    );
    public
        .set_scriptlet_metadata(&ScriptletBundleSummary::default())
        .unwrap();
    public.insert(&conn).unwrap();

    assert_eq!(
        ConvertedPackage::chunk_publication_state(&conn, shared_hash).unwrap(),
        ChunkPublicationState::PublicReady,
    );
}

#[test]
fn chunk_publication_state_allows_unreferenced_cas_hashes() {
    let (_temp, conn) = create_test_db();

    assert_eq!(
        ConvertedPackage::chunk_publication_state(
            &conn,
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        )
        .unwrap(),
        ChunkPublicationState::NoConvertedReference,
    );
}

#[test]
fn test_converted_package_crud() {
    let (_temp, conn) = create_test_db();

    // Create a converted package
    let mut converted = ConvertedPackage::new("rpm".to_string(), "sha256:abc123def456".to_string());

    let id = converted.insert(&conn).unwrap();
    assert!(id > 0);

    // Find by checksum
    let found = ConvertedPackage::find_by_checksum(&conn, "sha256:abc123def456")
        .unwrap()
        .unwrap();
    assert_eq!(found.original_format, "rpm");
    assert_eq!(found.scriptlet_fidelity, "unknown");

    // List all
    let all = ConvertedPackage::list_all(&conn).unwrap();
    assert_eq!(all.len(), 1);

    // Delete
    ConvertedPackage::delete_by_checksum(&conn, "sha256:abc123def456").unwrap();
    let deleted = ConvertedPackage::find_by_checksum(&conn, "sha256:abc123def456").unwrap();
    assert!(deleted.is_none());
}

#[test]
fn test_needs_reconversion() {
    let mut converted = ConvertedPackage::new("deb".to_string(), "sha256:test".to_string());
    converted.conversion_version = CONVERSION_VERSION;

    assert!(!converted.needs_reconversion());

    converted.conversion_version = CONVERSION_VERSION - 1;
    assert!(converted.needs_reconversion());
}

#[test]
fn test_count_by_format() {
    let (_temp, conn) = create_test_db();

    // Create converted packages with different formats
    let mut rpm1 = ConvertedPackage::new("rpm".to_string(), "sha256:r1".to_string());
    rpm1.insert(&conn).unwrap();

    let mut rpm2 = ConvertedPackage::new("rpm".to_string(), "sha256:r2".to_string());
    rpm2.insert(&conn).unwrap();

    let mut deb1 = ConvertedPackage::new("deb".to_string(), "sha256:d1".to_string());
    deb1.insert(&conn).unwrap();

    // Count by format
    let counts = ConvertedPackage::count_by_format(&conn).unwrap();
    assert_eq!(counts.len(), 2);

    // RPM should be first (most common)
    assert_eq!(counts[0].0, "rpm");
    assert_eq!(counts[0].1, 2);
    assert_eq!(counts[1].0, "deb");
    assert_eq!(counts[1].1, 1);
}

#[test]
fn test_unique_checksum_constraint() {
    let (_temp, conn) = create_test_db();

    let mut converted1 =
        ConvertedPackage::new("rpm".to_string(), "sha256:same_checksum".to_string());
    converted1.insert(&conn).unwrap();

    // Try to insert with same checksum - should fail
    let mut converted2 =
        ConvertedPackage::new("deb".to_string(), "sha256:same_checksum".to_string());
    let result = converted2.insert(&conn);
    assert!(result.is_err());
}

#[test]
fn test_enhancement_methods() {
    let (_temp, conn) = create_test_db();

    // Create and insert a converted package
    let mut converted = ConvertedPackage::new("rpm".to_string(), "sha256:enhance_test".to_string());
    converted.insert(&conn).unwrap();

    // Check initial enhancement state
    assert_eq!(converted.enhancement_status, "pending");
    assert_eq!(converted.enhancement_version, 0);
    assert!(converted.needs_enhancement(1));

    // Mark as complete
    converted
        .set_enhancement_complete(&conn, 1, Some(r#"{"builder":"conary"}"#))
        .unwrap();
    assert_eq!(converted.enhancement_status, "complete");
    assert_eq!(converted.enhancement_version, 1);
    assert!(!converted.needs_enhancement(1));
    assert!(converted.needs_enhancement(2)); // outdated

    // Verify persisted in database
    let found = ConvertedPackage::find_by_checksum(&conn, "sha256:enhance_test")
        .unwrap()
        .unwrap();
    assert_eq!(found.enhancement_status, "complete");
    assert_eq!(found.enhancement_version, 1);
    assert!(found.extracted_provenance_json.is_some());
}

#[test]
fn test_enhancement_failure() {
    let (_temp, conn) = create_test_db();

    let mut converted = ConvertedPackage::new("deb".to_string(), "sha256:fail_test".to_string());
    converted.insert(&conn).unwrap();

    // Mark as failed
    converted
        .set_enhancement_failed(&conn, "Test error message")
        .unwrap();
    assert_eq!(converted.enhancement_status, "failed");
    assert_eq!(
        converted.enhancement_error.as_deref(),
        Some("Test error message")
    );

    // Verify persisted
    let found = ConvertedPackage::find_by_checksum(&conn, "sha256:fail_test")
        .unwrap()
        .unwrap();
    assert_eq!(found.enhancement_status, "failed");
    assert!(found.enhancement_error.is_some());
}
