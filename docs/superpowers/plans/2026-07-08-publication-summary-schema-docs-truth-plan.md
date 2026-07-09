# Publication Summary Schema And Docs Truth Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close Workstream H by making publication/report response metadata sanitize boot and security-policy intents, preserving the `security_policy_intents` publication-summary shape gate, and aligning docs plus audit metadata.

**Architecture:** Reuse Remi's existing scriptlet evidence normalization helpers as the single sanitizer for boot-security and LSM policy intent evidence before publication reports reach public refusal responses or the admin non-public test-serving manifest. Add an explicit raw report path for private operator review artifacts so those artifacts retain diagnostic values while response DTOs stay sanitized. Keep the core database shape gate fail-closed for newly written non-default metadata that omits `security_policy_intents`, while proving older rows are handled by conversion-version staling or invalid summary rejection. Update the active docs and audit ledgers to describe the shape and response-sanitization contract.

**Tech Stack:** Rust unit tests in `remi` and `conary-core`, Remi handler tests, docs-audit checks, coherency ledger checks.

## Global Constraints

- `security_policy_intents` remains required in newly written non-default `scriptlet_summary_json` for publication readiness.
- The empty `{}` scriptlet summary remains a constructor compatibility path only for native/default rows with no scriptlet evidence.
- Stale converted rows with `conversion_version < CONVERSION_VERSION` remain not public-ready, even if their scriptlet summary looks public.
- Public refusal responses and non-public admin test-serving manifests must not expose `review_artifact_path`, private local paths such as `/home/remi/private.pp`, `/tmp/private-review-secret.json`, or embedded secret environment assignments.
- Private review artifact JSON remains operator diagnostic state and may retain raw boot/security intent paths and tokens; it is not serialized through public refusal or non-public test-serving responses.
- Sanitization must preserve public policy semantics: class IDs, reason codes, provider, operation, fallback, reconciliation state, and approved system policy paths such as `/etc/apparmor.d/<profile>`, `/etc/selinux/*`, and `/usr/share/selinux/*`.
- Approved absolute paths must be traversal-free; a lexical allowlist prefix never permits a `..` path component to escape an approved root.
- Non-public test serving remains default-off, admin-scoped, and not publication authority.
- Do not add a new adapter, new publication bypass, new schema migration, public gate exception, or live replay behavior in this slice.
- Documentation changes must keep `docs/SCRIPTLET_SECURITY.md`, `docs/modules/ccs.md`, `docs/modules/remi.md`, `docs/modules/test-fixtures.md`, docs-audit ledger, inventory, and feature coherency ledger aligned.

---

## File Structure

- Modify `apps/remi/src/server/scriptlet_evidence_queue/normalization.rs`
  - Extend the existing sanitizer allowlist to preserve approved AppArmor profile paths while continuing to redact private paths and embedded secrets.
- Modify `apps/remi/src/server/publication.rs`
  - Sanitize boot-security and security-policy intents when building response-oriented `PublicationGateReport` values.
  - Add a raw report helper for private review artifact persistence.
  - Add regression coverage for public refusal/report sanitization.
- Modify `apps/remi/src/server/conversion/persistence.rs`
  - Use the raw report helper when writing private conversion review artifacts.
- Modify `apps/remi/src/server/handlers/admin/packages.rs`
  - Use the raw report helper when writing private admin-upload review artifacts.
- Modify `apps/remi/src/server/handlers/admin/non_public_test_serving.rs`
  - Add HTTP regression coverage proving the non-public test-serving manifest uses the sanitized publication report.
- Modify `crates/conary-core/src/db/models/converted.rs`
  - Add an explicit stale older-summary regression for non-default metadata that lacks `security_policy_intents`.
- Modify `docs/SCRIPTLET_SECURITY.md`
  - Document the public/refusal response sanitization contract.
- Modify `docs/modules/ccs.md`
  - Document the `security_policy_intents` shape requirement and bump front matter when public claims change.
- Modify `docs/modules/remi.md`
  - Clarify that Remi public and admin-test scriptlet metadata exposes sanitized intent evidence only.
- Modify `docs/modules/test-fixtures.md`
  - Register the publication-summary/schema and sanitized-report proof surface.
- Modify `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
  - Register this plan and refreshed docs.
- Modify `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
  - Regenerate after staging this plan and docs.
- Modify `docs/superpowers/feature-coherency-ledger.tsv`
  - Add or refresh a coherency row for scriptlet publication metadata sanitization and schema shape truth.

## Task 1: Sanitize Publication Gate Reports

**Files:**
- Modify: `apps/remi/src/server/scriptlet_evidence_queue/normalization.rs`
- Modify: `apps/remi/src/server/publication.rs`
- Modify: `apps/remi/src/server/conversion/persistence.rs`
- Modify: `apps/remi/src/server/handlers/admin/packages.rs`

**Interfaces:**
- Consumes: `sanitize_boot_security_intents(&[BootSecurityIntentEvidence]) -> Vec<BootSecurityIntentEvidence>`
- Consumes: `sanitize_security_policy_intents(&[SecurityPolicyIntent]) -> Vec<SecurityPolicyIntent>`
- Produces: `PublicationGateReport.boot_security_intents` and `PublicationGateReport.security_policy_intents` with private paths and secret-bearing tokens scrubbed before serialization.
- Produces: `raw_report_from_summary(&ScriptletBundleSummary, bool) -> PublicationGateReport` for private review artifact persistence.
- Preserves: private review artifact JSON may contain raw operator diagnostics, while public/admin response DTOs use sanitized report values.

- [ ] **Step 1: Add sanitizer coverage for AppArmor policy paths and private values**

In `apps/remi/src/server/scriptlet_evidence_queue/normalization.rs`, add this test inside the existing `#[cfg(test)] mod tests` block:

```rust
#[test]
fn sanitizer_preserves_approved_lsm_policy_paths_and_redacts_private_values() {
    assert_eq!(
        normalize_token("/etc/apparmor.d/usr.bin.demo"),
        Some("/etc/apparmor.d/usr.bin.demo".to_string())
    );
    assert_eq!(
        normalize_token("/etc/selinux/targeted/policy/policy.33"),
        Some("/etc/selinux/targeted/policy/policy.33".to_string())
    );
    assert_eq!(
        normalize_token("/usr/share/selinux/packages/demo.pp"),
        Some("/usr/share/selinux/packages/demo.pp".to_string())
    );
    assert_eq!(
        normalize_token("/home/remi/private.pp"),
        Some("<path>".to_string())
    );
    assert_eq!(
        normalize_token("SECRET=/home/remi/token"),
        Some("<env-assignment>".to_string())
    );
}

#[test]
fn sanitizer_rejects_traversal_under_approved_absolute_path_prefixes() {
    for path in [
        "/lib/modules/<kver>/../../../home/remi/private.ko",
        "/usr/lib/modules/<kver>/../../../../home/remi/private.ko",
        "/etc/apparmor.d/../../home/remi/private.pp",
        "/etc/selinux/../home/remi/private.pp",
        "/usr/share/selinux/../../../home/remi/private.pp",
    ] {
        assert_eq!(
            normalize_token(path),
            Some("<path>".to_string()),
            "approved absolute-path prefixes must not admit traversal: {path}"
        );
    }
}

#[test]
fn security_policy_value_sanitizer_redacts_private_object_keys() {
    let value = r#"[{"provider":"selinux","desired_state":{"/home/remi/private.pp":"enabled"},"/home/remi/private-extra":"value"}]"#;

    let sanitized = sanitize_security_policy_intents_value(value);
    let json = serde_json::to_string(&sanitized).unwrap();

    assert!(!json.contains("/home/remi"));
    assert!(json.contains("<path>"));
}
```

- [ ] **Step 2: Run the sanitizer test and verify it fails**

Run:

```bash
cargo test -p remi sanitizer_preserves_approved_lsm_policy_paths_and_redacts_private_values
cargo test -p remi sanitizer_rejects_traversal_under_approved_absolute_path_prefixes
cargo test -p remi security_policy_value_sanitizer_redacts_private_object_keys
cargo test -p remi non_public_test_serving
```

Expected: the first three commands FAIL because `/etc/apparmor.d/usr.bin.demo` is normalized to `<path>`, approved lexical prefixes currently admit `..` traversal components, and object keys are not normalized before this task's implementation. The existing `non_public_test_serving` baseline should PASS.

- [ ] **Step 3: Preserve approved AppArmor policy paths and sanitize dynamic JSON keys**

In `apps/remi/src/server/scriptlet_evidence_queue/normalization.rs`, update `is_approved_absolute_path` to include `/etc/apparmor.d/`:

```rust
fn is_approved_absolute_path(token: &str) -> bool {
    let path = std::path::Path::new(token);
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return false;
    }

    token.starts_with("/lib/modules/<kver>/")
        || token.starts_with("/usr/lib/modules/<kver>/")
        || token.starts_with("/etc/apparmor.d/")
        || token.starts_with("/etc/selinux/")
        || token.starts_with("/usr/share/selinux/")
}
```

This changes the shared normalizer used by scriptlet evidence queue aggregation as well as publication report responses. Existing queue samples that normalized AppArmor profile paths to `<path>` should be re-materialized with `POST /v1/admin/scriptlet-evidence/backfill` after deployment when operators need consistent AppArmor clustering across old and new samples. The preview milestone can tolerate mixed historical samples before that backfill, but the deployment note must be explicit in docs.

Also update both recursive JSON sanitizer object branches so private paths in dynamic map keys cannot leak:

```rust
fn sanitize_boot_security_intent_value(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            let mut sanitized = serde_json::Map::new();
            for (key, mut field) in std::mem::take(fields) {
                sanitize_boot_security_intent_value(&mut field);
                let key = normalize_token(&key).unwrap_or(key);
                sanitized.insert(key, field);
            }
            *fields = sanitized;
        }
        Value::Array(values) => {
            for value in values {
                sanitize_boot_security_intent_value(value);
            }
        }
        Value::String(value) => {
            if let Some(normalized) = normalize_token(value) {
                *value = normalized;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn sanitize_security_policy_intents_value_inner(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            let mut sanitized = serde_json::Map::new();
            for (key, mut field) in std::mem::take(fields) {
                sanitize_security_policy_intents_value_inner(&mut field);
                let key = normalize_token(&key).unwrap_or(key);
                sanitized.insert(key, field);
            }
            *fields = sanitized;
        }
        Value::Array(values) => {
            for value in values {
                sanitize_security_policy_intents_value_inner(value);
            }
        }
        Value::String(value) => {
            if let Some(normalized) = normalize_token(value) {
                *value = normalized;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}
```

- [ ] **Step 4: Add publication-report sanitization regression**

In `apps/remi/src/server/publication.rs`, add this test inside `#[cfg(test)] mod tests` near `publication_report_includes_boot_security_intents`:

```rust
#[test]
fn publication_report_sanitizes_boot_and_security_policy_intents() {
    let mut summary = ScriptletBundleSummary {
        publication_status: "blocked".to_string(),
        blocked_classes: vec!["apparmor".to_string(), "initramfs".to_string()],
        boot_security_intents: vec![
            conary_core::ccs::legacy_scriptlets::BootSecurityIntentEvidence {
                class_id: "initramfs".to_string(),
                reason_code: "blocked-class-initramfs".to_string(),
                command: "dracut".to_string(),
                argv: vec![
                    "--force".to_string(),
                    "/home/remi/private-initramfs.img".to_string(),
                    "SECRET=/home/remi/token".to_string(),
                ],
                phase: Some("post-install".to_string()),
                lifecycle_paths: vec!["/home/remi/private-phase".to_string()],
            },
        ],
        security_policy_intents: vec![apparmor_policy_intent()],
        review_artifact_path: Some("/tmp/private-review-secret.json".to_string()),
        ..ScriptletBundleSummary::default()
    };
    summary.security_policy_intents[0].source.argv.push("SECRET=/home/remi/token".to_string());
    summary.security_policy_intents[0]
        .scope
        .paths
        .push("/home/remi/private.pp".to_string());
    summary.security_policy_intents[0]
        .payload_evidence
        .paths
        .push("/home/remi/private.pp".to_string());

    let report = report_from_summary(&summary, true);
    let json = serde_json::to_string(&report).unwrap();

    assert!(report.review_artifact_available);
    assert_eq!(report.boot_security_intents[0].argv[1], "<path>");
    assert_eq!(report.boot_security_intents[0].argv[2], "<env-assignment>");
    assert_eq!(report.boot_security_intents[0].lifecycle_paths, vec!["<path>"]);
    assert!(
        report.security_policy_intents[0]
            .scope
            .paths
            .contains(&"/etc/apparmor.d/usr.bin.demo".to_string())
    );
    assert!(
        report.security_policy_intents[0]
            .scope
            .paths
            .contains(&"<path>".to_string())
    );
    assert!(!json.contains("/home/remi"));
    assert!(!json.contains("SECRET="));
    assert!(!json.contains("review_artifact_path"));
    assert!(!json.contains("private-review-secret"));
}

#[test]
fn raw_publication_report_retains_private_intents_for_review_artifacts() {
    let mut summary = ScriptletBundleSummary {
        publication_status: "blocked".to_string(),
        blocked_classes: vec!["apparmor".to_string()],
        security_policy_intents: vec![apparmor_policy_intent()],
        ..ScriptletBundleSummary::default()
    };
    summary.security_policy_intents[0]
        .source
        .argv
        .push("SECRET=/home/remi/token".to_string());
    summary.security_policy_intents[0]
        .scope
        .paths
        .push("/home/remi/private.pp".to_string());

    let report = raw_report_from_summary(&summary, true);
    let json = serde_json::to_string(&report).unwrap();

    assert!(json.contains("/home/remi/private.pp"));
    assert!(json.contains("SECRET=/home/remi/token"));
}
```

- [ ] **Step 5: Run the publication-report test and verify it fails**

Run:

```bash
cargo test -p remi publication_report_sanitizes_boot_and_security_policy_intents
cargo test -p remi raw_publication_report_retains_private_intents_for_review_artifacts
cargo test -p remi publication_report_includes_boot_security_intents
cargo test -p remi blocked_apparmor_report_stays_private_and_carries_security_policy_intent
```

Expected: the first command FAILS because `report_from_summary` currently clones the unsanitized summary intents, and the second command FAILS because `raw_report_from_summary` does not exist yet. The existing publication report tests should PASS.

- [ ] **Step 6: Add sanitized and raw report helpers**

In `apps/remi/src/server/publication.rs`, keep `report_from_summary` as the sanitized response path and add `raw_report_from_summary` for private review artifacts:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReportIntentVisibility {
    Sanitized,
    Raw,
}

pub fn report_from_summary(
    summary: &ScriptletBundleSummary,
    summary_valid: bool,
) -> PublicationGateReport {
    report_from_summary_with_intent_visibility(
        summary,
        summary_valid,
        ReportIntentVisibility::Sanitized,
    )
}

pub fn raw_report_from_summary(
    summary: &ScriptletBundleSummary,
    summary_valid: bool,
) -> PublicationGateReport {
    report_from_summary_with_intent_visibility(summary, summary_valid, ReportIntentVisibility::Raw)
}

fn report_from_summary_with_intent_visibility(
    summary: &ScriptletBundleSummary,
    summary_valid: bool,
    intent_visibility: ReportIntentVisibility,
) -> PublicationGateReport {
    let mut reason_codes = Vec::new();
    let mut seen = BTreeSet::new();
    for code in &summary.blocked_reason_codes {
        push_reason(&mut reason_codes, &mut seen, code.clone());
    }
    for code in &summary.review_reason_codes {
        push_reason(&mut reason_codes, &mut seen, code.clone());
    }
    for command in sorted(&summary.unknown_commands) {
        push_reason(
            &mut reason_codes,
            &mut seen,
            format!("unknown-command:{command}"),
        );
    }
    for class_id in sorted(&summary.blocked_classes) {
        push_reason(&mut reason_codes, &mut seen, class_id);
    }
    if !summary_valid {
        push_reason(
            &mut reason_codes,
            &mut seen,
            "publication-gate-malformed-summary".to_string(),
        );
    }

    let (boot_security_intents, security_policy_intents) = match intent_visibility {
        ReportIntentVisibility::Sanitized => (
            crate::server::scriptlet_evidence_queue::normalization::sanitize_boot_security_intents(
                &summary.boot_security_intents,
            ),
            crate::server::scriptlet_evidence_queue::normalization::sanitize_security_policy_intents(
                &summary.security_policy_intents,
            ),
        ),
        ReportIntentVisibility::Raw => (
            summary.boot_security_intents.clone(),
            summary.security_policy_intents.clone(),
        ),
    };

    PublicationGateReport {
        publication_status: summary.publication_status.clone(),
        scriptlet_fidelity: summary.scriptlet_fidelity.clone(),
        target_compatibility: summary.target_compatibility.clone(),
        summary_valid,
        message: message_for_summary(summary, summary_valid),
        reason_codes,
        blocked_reason_codes: summary.blocked_reason_codes.clone(),
        review_reason_codes: summary.review_reason_codes.clone(),
        unknown_commands: sorted(&summary.unknown_commands),
        blocked_classes: sorted(&summary.blocked_classes),
        boot_security_intents,
        security_policy_intents,
        evidence_digest: summary.evidence_digest.clone(),
        curation_evidence_digest: summary.curation_evidence_digest.clone(),
        review_artifact_available: summary.review_artifact_path.is_some(),
    }
}
```

- [ ] **Step 7: Use raw reports for private review artifacts**

In `apps/remi/src/server/conversion/persistence.rs`, import `classify_summary` and `raw_report_from_summary`, then replace the review-artifact decision block with:

```rust
let publication = converted.scriptlet_summary_for_publication();
let decision = classify_summary(publication.clone());
if decision_refusal(decision).is_some() {
    let mut report = raw_report_from_summary(&publication.summary, publication.valid);
    report.review_artifact_available = true;
    let conversion_fidelity = conversion_result.fidelity.level.to_string();
    let artifact_path = write_review_artifact(
        &self.cache_dir,
        ReviewArtifactInput {
            distro: &distro,
            package: &metadata.name,
            version: &metadata.version,
            architecture: converted.package_architecture.as_deref(),
            original_format: &conversion_result.original_format,
            conversion_fidelity: &conversion_fidelity,
            conversion_version: CONVERSION_VERSION,
            ccs_content_hash: &content_hash,
            ccs_total_size: total_size,
            publication: report,
        },
    )?;
    let mut summary = conversion_result.scriptlet_metadata.clone();
    summary.review_artifact_path = Some(artifact_path.to_string_lossy().to_string());
    converted.set_scriptlet_metadata(&summary)?;
}
```

In `apps/remi/src/server/handlers/admin/packages.rs`, import `raw_report_from_summary`, keep `classify_summary` for the decision, and replace the `match refusal` report extraction with:

```rust
let publication = ScriptletSummaryForPublication {
    summary: scriptlet_summary.clone(),
    valid: true,
};
let decision = crate::server::publication::classify_summary(publication.clone());
if decision_refusal(decision).is_some() {
    let mut report = raw_report_from_summary(&publication.summary, publication.valid);
    report.review_artifact_available = true;
    let artifact_path = match write_review_artifact(
        &cache_dir,
        ReviewArtifactInput {
            distro: &distro,
            package: &package_name,
            version: &package_version,
            architecture: package_architecture.as_deref(),
            original_format: "ccs",
            conversion_fidelity: "full",
            conversion_version: CONVERSION_VERSION,
            ccs_content_hash: &content_hash,
            ccs_total_size: size,
            publication: report,
        },
    ) {
        Ok(path) => path,
        Err(err) => {
            tracing::error!("Failed to write scriptlet review artifact: {err}");
            let _ = tokio::fs::remove_file(&staged_path).await;
            return json_error(500, "Failed to publish package", "REVIEW_ARTIFACT_ERROR");
        }
    };
```

- [ ] **Step 8: Run focused Remi report and artifact tests**

Run:

```bash
cargo test -p remi publication_report_sanitizes_boot_and_security_policy_intents
cargo test -p remi raw_publication_report_retains_private_intents_for_review_artifacts
cargo test -p remi blocked_apparmor_report_stays_private_and_carries_security_policy_intent
cargo test -p remi publication_report_includes_boot_security_intents
cargo test -p remi sanitizer_preserves_approved_lsm_policy_paths_and_redacts_private_values
cargo test -p remi sanitizer_rejects_traversal_under_approved_absolute_path_prefixes
cargo test -p remi security_policy_value_sanitizer_redacts_private_object_keys
cargo test -p remi review_artifact
```

Expected: PASS for all selected tests.

- [ ] **Step 9: Commit Task 1**

```bash
git add apps/remi/src/server/scriptlet_evidence_queue/normalization.rs apps/remi/src/server/publication.rs apps/remi/src/server/conversion/persistence.rs apps/remi/src/server/handlers/admin/packages.rs
git commit -m "security(remi): sanitize scriptlet publication reports"
```

## Task 2: Pin Schema-Shape And Admin Manifest Evidence

**Files:**
- Modify: `crates/conary-core/src/db/models/converted.rs`
- Modify: `apps/remi/src/server/handlers/admin/non_public_test_serving.rs`

**Preserved ownership boundary:** `crates/conary-core/src/db/models/converted.rs` is over 1500 lines, and this task preserves its current `db::models` ownership of converted-package persistence plus scriptlet publication summary validation. Do not move behavior or add a new module in this slice; keep the added regression beside the existing summary-shape tests.

**Interfaces:**
- Consumes: `ConvertedPackage::scriptlet_summary_for_publication()`
- Consumes: `ConvertedPackage::needs_reconversion()`
- Consumes: `lookup_non_public_test_package_blocking()`
- Produces: explicit coverage for older non-default summaries without `security_policy_intents` and admin-test manifest sanitization.

- [ ] **Step 1: Add stale older-summary schema regression**

In `crates/conary-core/src/db/models/converted.rs`, add this test after `non_default_publication_summary_requires_security_policy_intents`:

```rust
#[test]
fn older_non_default_summary_without_security_policy_intents_is_stale_and_not_public_ready() {
    let mut converted = ConvertedPackage::new_server(
        "fedora".to_string(),
        "stale-policy".to_string(),
        "1.0".to_string(),
        "rpm".to_string(),
        "sha256:source".to_string(),
        "high".to_string(),
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
        "unknown_commands": [],
        "blocked_classes": [],
        "boot_security_intents": []
    })
    .to_string();

    let publication = converted.scriptlet_summary_for_publication();

    assert!(!publication.valid);
    assert!(converted.needs_reconversion());
    assert!(!converted.is_scriptlet_public_ready());
}
```

- [ ] **Step 2: Run the schema regression**

Run:

```bash
cargo test -p conary-core older_non_default_summary_without_security_policy_intents_is_stale_and_not_public_ready --lib
```

Expected: PASS. If it fails, fix only the shape/staleness behavior needed to satisfy the Workstream H contract.

- [ ] **Step 3: Add non-public test-serving manifest sanitization regression**

In `apps/remi/src/server/handlers/admin/non_public_test_serving.rs`, add this test after `non_public_test_manifest_preserves_apparmor_review_policy_fallback`:

```rust
#[tokio::test]
async fn non_public_test_serving_manifest_sanitizes_private_intent_values() {
    let (app, db_path) = test_app_with_non_public_test_serving(true).await;
    let mut summary = apparmor_review_summary();
    summary.security_policy_intents[0]
        .source
        .argv
        .push("SECRET=/home/remi/token".to_string());
    summary.security_policy_intents[0]
        .scope
        .paths
        .push("/home/remi/private.pp".to_string());
    summary.security_policy_intents[0]
        .payload_evidence
        .paths
        .push("/home/remi/private.pp".to_string());
    seed_non_public_test_row_with_summary(&db_path, "x86_64", "pkg.ccs", summary, true);

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/admin/packages/fedora/pkg/test-manifest?version=1.0&arch=x86_64")
                .header(header::AUTHORIZATION, "Bearer test-admin-token-12345")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = String::from_utf8(body.to_vec()).unwrap();

    assert!(body.contains("\"provider\":\"apparmor\""));
    assert!(body.contains("\"operation\":\"profile-reload\""));
    assert!(body.contains("/etc/apparmor.d/usr.bin.demo"));
    assert!(body.contains("\"<path>\""));
    assert!(!body.contains("/home/remi"));
    assert!(!body.contains("SECRET="));
    assert!(!body.contains("review_artifact_path"));
    assert!(!body.contains("private-review-secret"));
}
```

- [ ] **Step 4: Run the admin manifest regression**

Run:

```bash
cargo test -p remi non_public_test_serving_manifest_sanitizes_private_intent_values
```

Expected: PASS after Task 1's report-level sanitizer is in place.

- [ ] **Step 5: Run the focused schema/report/admin gates**

Run:

```bash
cargo test -p conary-core non_default_publication_summary --lib
cargo test -p conary-core older_non_default_summary_without_security_policy_intents_is_stale_and_not_public_ready --lib
cargo test -p conary-core stale_converted_rows_are_not_scriptlet_public_ready --lib
cargo test -p remi publication_report_sanitizes_boot_and_security_policy_intents
cargo test -p remi raw_publication_report_retains_private_intents_for_review_artifacts
cargo test -p remi non_public_test_serving_manifest_sanitizes_private_intent_values
cargo test -p remi non_public_test_manifest_preserves_apparmor_review_policy_fallback
cargo test -p remi security_policy_value_sanitizer_redacts_private_object_keys
cargo test -p remi review_artifact
```

Expected: all commands PASS.

- [ ] **Step 6: Commit Task 2**

```bash
git add crates/conary-core/src/db/models/converted.rs apps/remi/src/server/handlers/admin/non_public_test_serving.rs
git commit -m "test: pin scriptlet publication schema sanitization"
```

## Task 3: Align Docs, Ledgers, And Verification

**Files:**
- Modify: `docs/SCRIPTLET_SECURITY.md`
- Modify: `docs/modules/ccs.md`
- Modify: `docs/modules/remi.md`
- Modify: `docs/modules/test-fixtures.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/feature-coherency-ledger.tsv`

**Interfaces:**
- Consumes: Workstream H in `docs/superpowers/specs/2026-07-08-scriptlet-public-authority-roadmap-design.md`
- Produces: active docs that say publication summaries require both boot and security-policy intent fields for non-default metadata, and public/admin response surfaces expose sanitized intent evidence only.

- [ ] **Step 1: Update scriptlet security docs**

In `docs/SCRIPTLET_SECURITY.md`, add one paragraph to the public-gate/scriptlet metadata section that states:

```markdown
Publication refusal reports and Remi non-public test-serving manifests expose
boot/security and generic LSM `security_policy_intents` only after Remi's
shared scriptlet-evidence sanitizer runs. Raw review artifact paths, private
local paths, and secret-bearing environment assignments stay private server
state; responses expose only `review_artifact_available` and normalized intent
metadata. Private scriptlet review artifact JSON remains an operator diagnostic
surface and may retain raw path evidence under admin-only artifact access.
```

- [ ] **Step 2: Update CCS module docs and front matter**

In `docs/modules/ccs.md`, bump `revision` by 1 and update `summary` to:

```yaml
summary: Document scriptlet publication summary shape and sanitized intent reports
```

In the `convert/` subsection, add:

```markdown
Non-default scriptlet publication summaries must include both
`boot_security_intents` and `security_policy_intents`; rows that predate the
current conversion version are stale and must be reconverted before they can be
public-ready. The empty `{}` summary shape remains a constructor compatibility
path for native/default rows without scriptlet evidence.
```

- [ ] **Step 3: Update Remi module docs**

In `docs/modules/remi.md`, extend `Passive Scriptlet Metadata` with:

```markdown
Publication refusal responses and the default-off admin non-public test-serving
manifest share the same sanitized `PublicationGateReport`: boot-security and
generic LSM security-policy intent metadata is normalized before serialization,
private paths and secret-bearing tokens are redacted, and local
`review_artifact_path` values remain private. Private review artifact files use
the raw report helper so operators can still inspect exact blocked paths during
triage. Approved system policy paths are preserved only when absolute and free
of `..` traversal components. Preserving `/etc/apparmor.d/<profile>` as an approved policy path also
affects scriptlet evidence queue normalization; operators should run the bounded
admin evidence backfill after deployment when they need historical AppArmor
samples normalized into the same shape as new samples.
```

- [ ] **Step 4: Update fixture docs**

In `docs/modules/test-fixtures.md`, under `remi-scriptlet-publication-gate`, add:

```markdown
- `publication-summary-schema-and-sanitized-intents`: converted-package summary
  shape tests require `security_policy_intents` for non-default metadata and
  Remi publication/admin-test tests prove boot/security intent responses are
  sanitized before serialization.
```

- [ ] **Step 5: Update docs-audit ledger**

Append or refresh a `docs/superpowers/documentation-accuracy-audit-ledger.tsv` row for this plan:

```tsv
docs/superpowers/plans/2026-07-08-publication-summary-schema-docs-truth-plan.md	docs/superpowers/plans/2026-07-08-publication-summary-schema-docs-truth-plan.md	planning	maintainer	scriptlet-security; publication-summary; security-policy-intents; remi-publication-gate; implementation-plan	docs/superpowers/specs/2026-07-08-scriptlet-public-authority-roadmap-design.md; docs/SCRIPTLET_SECURITY.md; docs/modules/ccs.md; docs/modules/remi.md; docs/modules/test-fixtures.md; crates/conary-core/src/db/models/converted.rs; apps/remi/src/server/publication.rs; apps/remi/src/server/handlers/admin/non_public_test_serving.rs	verified	corrected	Implementation plan for Workstream H, tightening scriptlet publication summary schema truth, sanitizing boot/security intent evidence in Remi public refusal and admin non-public test-serving responses, and preserving raw private review artifacts for operator diagnostics.
```

Also refresh the existing ledger rows whose first column is each of the four
canonical docs below. Preserve their existing classification, owner, and prior
evidence, then add these exact values:

| Ledger row | Topics to add | Evidence references to add | Exact audit-note sentence to append |
|---|---|---|---|
| `docs/SCRIPTLET_SECURITY.md` | `publication-summary; sanitized-intent-reports` | `docs/superpowers/plans/2026-07-08-publication-summary-schema-docs-truth-plan.md; crates/conary-core/src/db/models/converted.rs; apps/remi/src/server/scriptlet_evidence_queue/normalization.rs; apps/remi/src/server/publication.rs; apps/remi/src/server/handlers/admin/non_public_test_serving.rs` | `Documented the required non-default publication-summary intent fields, sanitized public/admin report boundary, traversal-free approved policy paths, and raw private review-artifact boundary.` |
| `docs/modules/ccs.md` | `publication-summary; sanitized-intent-reports` | `docs/superpowers/plans/2026-07-08-publication-summary-schema-docs-truth-plan.md; crates/conary-core/src/db/models/converted.rs; apps/remi/src/server/publication.rs` | `Documented that non-default publication summaries require boot and security-policy intent fields and that stale rows must be reconverted before publication.` |
| `docs/modules/remi.md` | `publication-summary; sanitized-intent-reports` | `docs/superpowers/plans/2026-07-08-publication-summary-schema-docs-truth-plan.md; apps/remi/src/server/scriptlet_evidence_queue/normalization.rs; apps/remi/src/server/publication.rs; apps/remi/src/server/handlers/admin/non_public_test_serving.rs` | `Documented sanitized publication and admin-test intent reports, traversal-free approved policy paths, raw private review artifacts, and the AppArmor evidence-backfill note.` |
| `docs/modules/test-fixtures.md` | `publication-summary; sanitized-intent-reports` | `docs/superpowers/plans/2026-07-08-publication-summary-schema-docs-truth-plan.md; crates/conary-core/src/db/models/converted.rs; apps/remi/src/server/publication.rs; apps/remi/src/server/handlers/admin/non_public_test_serving.rs` | `Registered focused publication-summary shape and sanitized-intent response proof.` |

- [ ] **Step 6: Update feature coherency ledger**

Append a row to `docs/superpowers/feature-coherency-ledger.tsv`:

The `ROUTE-REMI-SCRIPTLET-EVIDENCE-001` relationship below is a dependency: this publication-report row reuses the evidence queue normalization helpers. It does not replace or invalidate the evidence-queue workflow claim.

```tsv
DOC-SCRIPTLET-PUBLICATION-SUMMARY-001	scriptlet publication summary and sanitized reports	doc:docs/SCRIPTLET_SECURITY.md;doc:docs/modules/ccs.md;doc:docs/modules/remi.md;doc:docs/modules/test-fixtures.md;path:crates/conary-core/src/db/models/converted.rs;path:apps/remi/src/server/publication.rs;path:apps/remi/src/server/handlers/admin/non_public_test_serving.rs	ROUTE-REMI-SCRIPTLET-EVIDENCE-001	scriptlet-publication-summary-schema	Remi Publication, Serving, Admin, And Fixture Artifacts	Non-default scriptlet publication summaries require boot and security-policy intent fields, stale converted rows stay non-public, and Remi public refusal plus admin non-public test-serving responses serialize sanitized boot/security intent metadata only	Focused conary-core schema tests and Remi publication/admin tests prove missing security_policy_intents is not public-ready, stale summaries require reconversion, review_artifact_path stays private, private paths, traversal-bearing approved-prefix paths, or secret-bearing tokens are normalized before responses, and private review artifacts keep raw operator diagnostics	works	resolved-repaired	2026-07-08	doc:docs/SCRIPTLET_SECURITY.md;doc:docs/modules/ccs.md;doc:docs/modules/remi.md;doc:docs/modules/test-fixtures.md;path:crates/conary-core/src/db/models/converted.rs;path:apps/remi/src/server/publication.rs;path:apps/remi/src/server/handlers/admin/non_public_test_serving.rs	none	test:cargo test -p conary-core non_default_publication_summary_requires_security_policy_intents;test:cargo test -p conary-core older_non_default_summary_without_security_policy_intents_is_stale_and_not_public_ready;test:cargo test -p remi publication_report_sanitizes_boot_and_security_policy_intents;test:cargo test -p remi raw_publication_report_retains_private_intents_for_review_artifacts;test:cargo test -p remi non_public_test_serving_manifest_sanitizes_private_intent_values;test:cargo test -p remi sanitizer_rejects_traversal_under_approved_absolute_path_prefixes;test:cargo test -p remi security_policy_value_sanitizer_redacts_private_object_keys;cmd:bash scripts/check-doc-truth.sh;cmd:bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete;cmd:LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -;cmd:bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv	verify	Re-run schema, publication, admin-test, review-artifact, docs, and coherency proof before changing scriptlet publication summary validation or report serialization	Public/admin report sanitization is a response-layer contract; private review artifacts remain admin-only raw diagnostic state
```

- [ ] **Step 7: Regenerate inventory after staging docs**

Run:

```bash
git add docs/superpowers/plans/2026-07-08-publication-summary-schema-docs-truth-plan.md docs/SCRIPTLET_SECURITY.md docs/modules/ccs.md docs/modules/remi.md docs/modules/test-fixtures.md docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/feature-coherency-ledger.tsv
LC_ALL=C bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
git add docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

- [ ] **Step 8: Run docs and focused code verification**

Run:

```bash
cargo test -p conary-core non_default_publication_summary --lib
cargo test -p conary-core older_non_default_summary_without_security_policy_intents_is_stale_and_not_public_ready --lib
cargo test -p conary-core stale_converted_rows_are_not_scriptlet_public_ready --lib
cargo test -p remi publication_report_sanitizes_boot_and_security_policy_intents
cargo test -p remi raw_publication_report_retains_private_intents_for_review_artifacts
cargo test -p remi non_public_test_serving_manifest_sanitizes_private_intent_values
cargo test -p remi non_public_test_manifest_preserves_apparmor_review_policy_fallback
cargo test -p remi security_policy_value_sanitizer_redacts_private_object_keys
cargo test -p remi review_artifact
bash scripts/check-doc-truth.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
git diff --check
```

Expected: all commands PASS.

- [ ] **Step 9: Commit Task 3**

```bash
git add docs/SCRIPTLET_SECURITY.md docs/modules/ccs.md docs/modules/remi.md docs/modules/test-fixtures.md docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/feature-coherency-ledger.tsv
git commit -m "docs: align scriptlet publication summary truth"
```

## Final Slice Verification

`conary-test` integration coverage for sanitized publication responses is deferred to a future Remi publication hardening slice; this Workstream H gate relies on focused Remi handler/report tests plus full Remi publication and non-public test-serving module coverage.

After all tasks and task reviews pass, run:

```bash
cargo test -p conary-core non_default_publication_summary --lib
cargo test -p conary-core older_non_default_summary_without_security_policy_intents_is_stale_and_not_public_ready --lib
cargo test -p conary-core stale_converted_rows_are_not_scriptlet_public_ready --lib
cargo test -p remi publication
cargo test -p remi non_public_test_serving
cargo test -p remi review_artifact
cargo test -p remi sanitizer_rejects_traversal_under_approved_absolute_path_prefixes
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
bash scripts/check-doc-truth.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
git diff --check
```

Expected: all commands PASS before the Workstream H final review.
