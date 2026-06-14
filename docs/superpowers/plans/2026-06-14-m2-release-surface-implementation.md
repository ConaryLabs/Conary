# M2 Release Surface Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finish the remaining M2 release surface by adding signed build attestations, static artifact-form publish gates, foreign package ingestion, and Remi release push with server-side gate parity.

**Architecture:** Keep build attestations as CCS/package evidence, keep static repository mutation as the commit layer, and put release eligibility in focused core gate modules consumed by the CLI, static publish, and Remi. Project-form publish rebuilds hermetically, signs an attestation, verifies its own artifact, and publishes; artifact-form publish verifies an existing artifact without rebuilding. Remi rechecks the same artifact gates server-side after transport authentication and before any public package/index/TUF visibility.

**Tech Stack:** Rust workspace, `serde`, `serde_json`, existing CCS tarball/CBOR/TOML package writer, existing Ed25519 `SigningKeyPair`, existing TUF/static repo metadata, Axum Remi handlers, SQLite Remi metadata tests, `cargo test`, `cargo fmt`, and `cargo clippy`.

---

## Source Design

Implement against:

- `docs/superpowers/specs/2026-06-13-m2-publish-hardening-remi-design.md`
- `docs/superpowers/specs/2026-06-10-packaging-toolchain-design.md`
- `AGENTS.md`

Use the conservative M2b attestation projection from the design:

- Embed `BuildAttestationEnvelope` in `ManifestProvenance.build_attestation`.
- Require artifact-form publish to verify the CBOR package signature, the CBOR `toml_integrity_hash`, the TOML manifest bytes, and the embedded attestation envelope before signer authority or lint decisions.
- Do not add a binary-manifest v2 projection in this slice.

## File Map

Create these files:

- `crates/conary-core/src/ccs/attestation.rs`: attestation DTOs, canonical serialization, signing, verification, output identity, foreign conversion boundary DTO, and report hashing helpers.
- `crates/conary-core/src/repository/static_repo/publish_gate.rs`: static artifact eligibility, `AcceptedStaticSignerSet`, publish lint reason codes, and static publish gate result types.
- `crates/conary-core/src/repository/static_repo/publish_context.rs`: prepared static publish context, active publish key resolution, active-key policy, brand-new repo explicit `--key-dir` behavior, and project-form attestation signing entrypoints.
- `crates/conary-core/src/security/mod.rs`: shared security/risk namespace.
- `crates/conary-core/src/security/command_risk.rs`: shared command-risk taxonomy, reason codes, report DTO, script risk mapping, and adapters for recipe/PKGBUILD/foreign inputs.
- `apps/conary/src/commands/remi_publish.rs`: Remi publish client transport, bearer-token extraction, target routing, and client preflight plumbing.
- `apps/remi/src/server/release_publish.rs`: server-side release upload staging, gate invocation, object promotion ordering, DB/index/TUF commit, and rollback cleanup.

Modify these files:

- `crates/conary-core/src/ccs/mod.rs`: export `attestation`.
- `crates/conary-core/src/ccs/manifest_provenance.rs`: add `build_attestation`.
- `crates/conary-core/src/ccs/builder/package_writer.rs`: preserve TOML integrity over attestation-bearing `MANIFEST.toml`.
- `crates/conary-core/src/ccs/verify.rs`: expose a helper or result field proving TOML integrity is valid.
- `crates/conary-core/src/recipe/hermetic/evidence.rs`: use shared command-risk DTO or preserve compatibility through `From` conversions.
- `crates/conary-core/src/recipe/hermetic/command_risk.rs`: delegate classification to `security::command_risk`.
- `crates/conary-core/src/container/analysis.rs`: map shared command-risk reason codes into `ScriptRisk`.
- `crates/conary-core/src/recipe/pkgbuild.rs`: produce PKGBUILD body risk reports.
- `crates/conary-core/src/ccs/convert/converter.rs`: attach foreign conversion boundary and scriptlet risk evidence to converted artifacts.
- `crates/conary-core/src/ccs/convert/scriptlet_bundle/digest.rs`: reuse scriptlet bundle digest evidence in the boundary.
- `crates/conary-core/src/repository/static_repo/mod.rs`: export new gate/context modules.
- `crates/conary-core/src/repository/static_repo/publish.rs`: use prepared context and gate modules; keep this file as layout/metadata commit owner.
- `apps/conary/src/cli/mod.rs`: unhide artifact-form publish help and parse Remi/static target forms.
- `apps/conary/src/commands/publish.rs`: split project-form and artifact-form flows, force release dirty-tree refusal, route Remi targets.
- `apps/conary/src/commands/cook.rs`: route `.rpm`, `.deb`, and `.pkg.tar.zst` inputs through foreign conversion file-output.
- `apps/remi/src/server/config.rs`: add trusted release signer configuration.
- `apps/remi/src/server/routes/admin.rs`: add distinct release upload route.
- `apps/remi/src/server/handlers/admin/packages.rs`: keep legacy/admin review upload behavior separate from release publication.
- `apps/remi/src/server/handlers/tuf.rs`: implement timestamp refresh used by release push.
- `apps/remi/src/server/handlers/openapi.rs`: document the release upload route and trusted-signer failure response.
- `docs/llms/subsystem-map.md`: update the "look here first" path for M2 release publish ownership.
- `docs/modules/ccs.md`: document attestation ownership when public CCS docs mention provenance.
- `docs/modules/remi.md`: document Remi release-push ownership when public Remi docs mention package upload.

Maintainability budget:

- `crates/conary-core/src/repository/static_repo/publish.rs` starts at 2659 lines. Net line growth after the M2 implementation must be `<= 40` lines; if it grows more, move orchestration into `publish_gate.rs` or `publish_context.rs` before review.
- `crates/conary-core/src/ccs/manifest.rs` starts at 1500 lines. Do not add attestation logic there beyond field-compatible serialization use through existing manifest/provenance paths.
- `crates/conary-core/src/recipe/kitchen/cook.rs` starts at 1686 lines. Keep Kitchen as orchestration; attestation creation belongs in `ccs::attestation` and publish command/context helpers.
- `apps/remi/src/server/handlers/admin/packages.rs` starts at 1198 lines. Release push logic belongs in `apps/remi/src/server/release_publish.rs`; route handlers should call into it.

## Checkpoint Order

- M2b checkpoint: signed build attestations, static prepared context, project-form attestation, artifact-form static publish gates.
- M2c checkpoint: foreign package file-output ingestion and conversion boundary attestations.
- M2d checkpoint: Remi release push, trusted signer config, private staging, TUF timestamp refresh.
- Final checkpoint: due-diligence review loop, focused regression runs, workspace gates, and cleanup.

## Task 1: Attestation Core Types And Canonical Serialization

**Files:**
- Create: `crates/conary-core/src/ccs/attestation.rs`
- Modify: `crates/conary-core/src/ccs/mod.rs`
- Test: `crates/conary-core/src/ccs/attestation.rs`

- [ ] **Step 1: Write failing attestation canonicalization tests**

Add this test module to the new file:

```rust
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    pub(crate) fn sample_output_identity_for_tests() -> BuildOutputIdentity {
        BuildOutputIdentity {
            file_merkle_root: "sha256:files".to_string(),
            package_name: "hello".to_string(),
            package_version: "1.0".to_string(),
            package_release: "1".to_string(),
            architecture: Some("x86_64".to_string()),
            origin_class: "native-built".to_string(),
            hardening_level: "hermetic".to_string(),
            hermetic_evidence_hash: "sha256:evidence".to_string(),
            canonical_content_identity: "sha256:content".to_string(),
        }
    }

    pub(crate) fn sample_payload_for_tests() -> BuildAttestationPayload {
        BuildAttestationPayload {
            schema_version: BUILD_ATTESTATION_SCHEMA_V1,
            origin_class: "native-built".to_string(),
            hardening_level: "hermetic".to_string(),
            build_input: sample_build_input_for_tests(),
            dependency_lock: crate::recipe::hermetic::DependencyLock::default(),
            hermetic_evidence_hash: "sha256:evidence".to_string(),
            output_identity: sample_output_identity_for_tests(),
            build_command_risk_report_hash: "sha256:build-risk".to_string(),
            scriptlet_risk_report_hash: None,
            conversion_boundary_hash: None,
            publish_policy_digest: "m2-policy-v1".to_string(),
            command_risk_classifier_version: "m2-command-risk-v1".to_string(),
            sandbox_profile: "kitchen-pristine-network-none".to_string(),
            seccomp_profile: Some("scriptlet-v1".to_string()),
            builder_identity: "conary-test-builder".to_string(),
            conary_version: "test".to_string(),
            issued_at: "2026-06-14T00:00:00Z".to_string(),
        }
    }

    fn sample_build_input_for_tests() -> crate::recipe::hermetic::BuildInputIdentity {
        crate::recipe::hermetic::BuildInputIdentity {
            recipe: crate::recipe::hermetic::RecipeIdentity::ExplicitRecipe {
                path: "recipe.toml".to_string(),
                hash: "sha256:recipe".to_string(),
            },
            source: crate::recipe::hermetic::SourceIdentity::Archive {
                url: "https://example.invalid/hello.tar.gz".to_string(),
                checksum: "sha256:source".to_string(),
            },
            additional_sources: Vec::new(),
            patches: Vec::new(),
            local_tree: None,
            ecosystem_dependencies: Vec::new(),
            builder_environment: crate::recipe::hermetic::BuilderEnvironmentIdentity {
                kind: crate::recipe::hermetic::BuilderEnvironmentKind::Pristine,
                sysroot_hash: Some("sha256:sysroot".to_string()),
                toolchain_hash: None,
                diagnostics: Vec::new(),
            },
        }
    }

    pub(crate) fn sample_envelope_for_tests(
        key: &crate::ccs::signing::SigningKeyPair,
    ) -> BuildAttestationEnvelope {
        sign_build_attestation(sample_payload_for_tests(), key).unwrap()
    }

    pub(crate) fn sample_hermetic_evidence_for_tests(
    ) -> crate::recipe::hermetic::HermeticBuildEvidence {
        crate::recipe::hermetic::HermeticBuildEvidence {
            schema_version: crate::recipe::hermetic::HERMETIC_EVIDENCE_SCHEMA_V1,
            build_input: sample_build_input_for_tests(),
            dependency_lock: crate::recipe::hermetic::DependencyLock::default(),
            ecosystem_policy: crate::recipe::hermetic::EcosystemPolicyReport::clean("test"),
            command_risk: crate::recipe::hermetic::BuildCommandRiskReport::clean(),
            reproducibility: crate::recipe::hermetic::ReproducibilityRecord {
                source_date_epoch: Some(1),
                path_remap_count: 1,
                env_keys: vec!["SOURCE_DATE_EPOCH".to_string()],
            },
            divergence: crate::recipe::hermetic::DivergenceReport::default(),
            diagnostics: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::test_support::*;

    #[test]
    fn canonical_payload_serialization_is_stable() {
        let payload = sample_payload_for_tests();
        let first = canonical_payload_bytes(&payload).unwrap();
        let second = canonical_payload_bytes(&payload).unwrap();

        assert_eq!(first, second);
        assert!(String::from_utf8(first).unwrap().contains("\"schema_version\":1"));
    }

    #[test]
    fn envelope_signature_round_trips() {
        let key = crate::ccs::signing::SigningKeyPair::generate().with_key_id("test-publisher");
        let envelope = sign_build_attestation(sample_payload_for_tests(), &key).unwrap();
        let verified =
            verify_build_attestation_envelope(&envelope, &key.public_key_base64()).unwrap();

        assert_eq!(verified.signer_key_id, envelope.signer_key_id);
        assert_eq!(verified.payload.output_identity.package_name, "hello");
    }

    #[test]
    fn envelope_signature_rejects_payload_mutation() {
        let key = crate::ccs::signing::SigningKeyPair::generate().with_key_id("test-publisher");
        let mut envelope = sign_build_attestation(sample_payload_for_tests(), &key).unwrap();
        envelope.payload.hardening_level = "sandboxed".to_string();

        let err =
            verify_build_attestation_envelope(&envelope, &key.public_key_base64()).unwrap_err();
        assert!(err.to_string().contains("build attestation signature mismatch"));
    }
}
```

- [ ] **Step 2: Run the new tests and confirm missing module failure**

Run: `cargo test -p conary-core ccs::attestation -- --nocapture`

Expected: FAIL because `ccs::attestation` is not exported and the attestation types/functions do not exist.

- [ ] **Step 3: Add the attestation module export**

In `crates/conary-core/src/ccs/mod.rs`, add:

```rust
pub mod attestation;
```

Add exports near the existing CCS exports:

```rust
pub use attestation::{
    BUILD_ATTESTATION_SCHEMA_V1, BuildAttestationEnvelope, BuildAttestationPayload,
    BuildOutputIdentity, ForeignConversionBoundary, VerifiedBuildAttestation,
};
```

- [ ] **Step 4: Implement the attestation DTOs and signing helpers**

Add this structure to `crates/conary-core/src/ccs/attestation.rs`:

```rust
// conary-core/src/ccs/attestation.rs
//! Build attestation and publish-boundary DTOs for CCS artifacts.

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::ccs::signing::SigningKeyPair;
use crate::ccs::verify::PackageSignature;
use crate::hash;

pub const BUILD_ATTESTATION_SCHEMA_V1: u32 = 1;
pub const FOREIGN_CONVERSION_BOUNDARY_SCHEMA_V1: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BuildOutputIdentity {
    pub file_merkle_root: String,
    pub package_name: String,
    pub package_version: String,
    pub package_release: String,
    pub architecture: Option<String>,
    pub origin_class: String,
    pub hardening_level: String,
    pub hermetic_evidence_hash: String,
    pub canonical_content_identity: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BuildAttestationPayload {
    pub schema_version: u32,
    pub origin_class: String,
    pub hardening_level: String,
    pub build_input: crate::recipe::hermetic::BuildInputIdentity,
    pub dependency_lock: crate::recipe::hermetic::DependencyLock,
    pub hermetic_evidence_hash: String,
    pub output_identity: BuildOutputIdentity,
    pub build_command_risk_report_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scriptlet_risk_report_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversion_boundary_hash: Option<String>,
    pub publish_policy_digest: String,
    pub command_risk_classifier_version: String,
    pub sandbox_profile: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seccomp_profile: Option<String>,
    pub builder_identity: String,
    pub conary_version: String,
    pub issued_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildAttestationEnvelope {
    pub schema_version: u32,
    pub payload: BuildAttestationPayload,
    pub signer_key_id: String,
    pub signature: PackageSignature,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerifiedBuildAttestation {
    pub payload: BuildAttestationPayload,
    pub signer_key_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ForeignConversionBoundary {
    pub schema_version: u32,
    pub source_format: String,
    pub distro: Option<String>,
    pub release: Option<String>,
    pub architecture: Option<String>,
    pub source_checksum: String,
    pub source_identity: String,
    pub converter_name: String,
    pub converter_version: String,
    pub conversion_policy_version: String,
    pub legacy_provenance_digest: String,
    pub legacy_scriptlet_bundle_digest: String,
    pub legacy_scriptlet_summary_digest: String,
    pub build_command_risk_report_hash: String,
    pub scriptlet_risk_report_hash: String,
    pub fidelity: String,
    pub publication_status: String,
    pub output_identity: BuildOutputIdentity,
}

pub fn canonical_payload_bytes(payload: &BuildAttestationPayload) -> Result<Vec<u8>> {
    serde_json::to_vec(payload).context("serialize build attestation payload")
}

pub fn canonical_json_hash<T: Serialize>(value: &T) -> Result<String> {
    let bytes = serde_json::to_vec(value).context("serialize canonical JSON value")?;
    Ok(format!("sha256:{}", hash::sha256(&bytes)))
}

pub fn sign_build_attestation(
    payload: BuildAttestationPayload,
    key: &SigningKeyPair,
) -> Result<BuildAttestationEnvelope> {
    if payload.schema_version != BUILD_ATTESTATION_SCHEMA_V1 {
        bail!(
            "unsupported build attestation schema version {}",
            payload.schema_version
        );
    }
    let canonical = canonical_payload_bytes(&payload)?;
    let signature = key.sign(&canonical);
    let signer_key_id = signature
        .key_id
        .clone()
        .or_else(|| key.key_id().map(str::to_string))
        .unwrap_or_else(|| signature.public_key.clone());
    Ok(BuildAttestationEnvelope {
        schema_version: BUILD_ATTESTATION_SCHEMA_V1,
        payload,
        signer_key_id,
        signature,
    })
}

pub fn verify_build_attestation_envelope(
    envelope: &BuildAttestationEnvelope,
    public_key_base64: &str,
) -> Result<VerifiedBuildAttestation> {
    if envelope.schema_version != BUILD_ATTESTATION_SCHEMA_V1 {
        bail!(
            "unsupported build attestation envelope schema version {}",
            envelope.schema_version
        );
    }
    if envelope.payload.schema_version != BUILD_ATTESTATION_SCHEMA_V1 {
        bail!(
            "unsupported build attestation payload schema version {}",
            envelope.payload.schema_version
        );
    }
    let canonical = canonical_payload_bytes(&envelope.payload)?;
    verify_package_signature_bytes(public_key_base64, &canonical, &envelope.signature)
        .context("verify build attestation signature")?;
    Ok(VerifiedBuildAttestation {
        payload: envelope.payload.clone(),
        signer_key_id: envelope.signer_key_id.clone(),
    })
}

fn verify_package_signature_bytes(
    expected_public_key_base64: &str,
    canonical: &[u8],
    signature: &PackageSignature,
) -> Result<()> {
    if signature.algorithm != "ed25519" {
        bail!(
            "unsupported build attestation signature algorithm {}",
            signature.algorithm
        );
    }
    if signature.public_key != expected_public_key_base64 {
        bail!("build attestation signer key does not match accepted authority");
    }
    let signature_bytes = BASE64
        .decode(&signature.signature)
        .context("decode build attestation signature")?;
    let signature = Signature::from_slice(&signature_bytes)
        .context("parse build attestation Ed25519 signature")?;
    let key_bytes = BASE64
        .decode(expected_public_key_base64)
        .context("decode build attestation public key")?;
    let verifying_key = VerifyingKey::from_bytes(
        &key_bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("build attestation public key must be 32 bytes"))?,
    )
    .context("parse build attestation public key")?;
    if verifying_key.verify(canonical, &signature).is_err() {
        bail!("build attestation signature mismatch");
    }
    Ok(())
}
```

If `SigningKeyPair::key_id()` returns `None`, the attestation signer ID falls back to the base64 public key. Keep `PackageSignature.public_key` as the cryptographic key material used by the verifier.

- [ ] **Step 5: Run attestation tests**

Run: `cargo test -p conary-core ccs::attestation -- --nocapture`

Expected: PASS.

- [ ] **Step 6: Commit Task 1**

```bash
git add crates/conary-core/src/ccs/attestation.rs crates/conary-core/src/ccs/mod.rs crates/conary-core/src/ccs/signing.rs
git commit -m "security(ccs): add build attestation envelope"
```

## Task 2: Manifest Projection And TOML Tamper Tests

**Files:**
- Modify: `crates/conary-core/src/ccs/manifest_provenance.rs`
- Modify: `crates/conary-core/src/ccs/builder.rs`
- Modify: `crates/conary-core/src/ccs/verify.rs`
- Test: `crates/conary-core/src/ccs/builder/package_writer.rs`
- Test: `crates/conary-core/src/ccs/verify.rs`

- [ ] **Step 1: Add failing test for attestation-bearing TOML integrity**

In `crates/conary-core/src/ccs/verify.rs`, add a test that creates a signed CCS package with a build attestation, rewrites only `MANIFEST.toml` inside a copied archive, and verifies that `verify_package` fails TOML integrity before publish can consume it.

Use this assertion shape:

```rust
#[test]
fn verify_package_rejects_tampered_attestation_toml() {
    let temp = tempfile::tempdir().unwrap();
    let key = crate::ccs::signing::SigningKeyPair::generate().with_key_id("test-publisher");
    let package_path = temp.path().join("signed.ccs");
    let tampered_path = temp.path().join("tampered.ccs");

    let mut result = crate::ccs::builder::test_support::minimal_build_result("attested", "1.0");
    result.manifest.provenance.build_attestation =
        Some(crate::ccs::attestation::test_support::sample_envelope_for_tests(&key));
    crate::ccs::builder::write_signed_ccs_package(&result, &package_path, &key).unwrap();
    crate::ccs::builder::test_support::rewrite_manifest_toml_for_tests(
        &package_path,
        &tampered_path,
        |toml| toml.replace("m2-policy-v1", "m2-policy-mutated"),
    )
    .unwrap();

    let verification = verify_package(&tampered_path, &TrustPolicy::TrustAll).unwrap();

    assert!(!verification.toml_integrity_valid);
    assert!(
        verification
            .warnings
            .iter()
            .any(|warning| warning.contains("TOML manifest integrity check failed")),
        "{:?}",
        verification.warnings
    );
}
```

- [ ] **Step 2: Run the tamper test and confirm missing field/helper failure**

Run: `cargo test -p conary-core verify_package_rejects_tampered_attestation_toml -- --nocapture`

Expected: FAIL because `ManifestProvenance.build_attestation`, test helpers, or `toml_integrity_valid` are missing.

- [ ] **Step 3: Add the manifest field**

In `crates/conary-core/src/ccs/manifest_provenance.rs`, add after `hermetic_evidence`:

```rust
/// Signed M2 build attestation used by artifact-form publish gates.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub build_attestation: Option<crate::ccs::attestation::BuildAttestationEnvelope>,
```

- [ ] **Step 4: Expose TOML integrity in verification results**

In `crates/conary-core/src/ccs/verify.rs`, extend `VerificationResult` with:

```rust
pub toml_integrity_valid: bool,
```

Set it from the existing `toml_integrity_valid` local and ensure callers that construct `VerificationResult` in tests initialize the field.

- [ ] **Step 5: Add test helpers without widening production API**

In `crates/conary-core/src/ccs/builder.rs`, add a crate-visible test-support module:

```rust
#[cfg(test)]
pub(crate) mod test_support;
```

Create `crates/conary-core/src/ccs/builder/test_support.rs` with crate-private helpers that build a small `BuildResult` and rewrite tar entries.

Use helper names from the failing test:

```rust
pub(crate) fn minimal_build_result(name: &str, version: &str) -> BuildResult {
    use std::collections::HashMap;

    let mut manifest = crate::ccs::manifest::CcsManifest::new_minimal(name, version);
    manifest.provenance.origin_class = Some("native-built".to_string());
    manifest.provenance.hardening_level = Some("hermetic".to_string());
    manifest.provenance.merkle_root = Some("sha256:empty".to_string());
    manifest.provenance.hermetic_evidence =
        Some(crate::ccs::attestation::test_support::sample_hermetic_evidence_for_tests());

    BuildResult {
        manifest,
        components: HashMap::new(),
        files: Vec::new(),
        blobs: HashMap::new(),
        total_size: 0,
        chunked: false,
        chunk_stats: None,
    }
}

pub(crate) fn rewrite_manifest_toml_for_tests<F>(from: &Path, to: &Path, mutate: F) -> anyhow::Result<()>
where
    F: FnOnce(String) -> String,
{
    use flate2::read::GzDecoder;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::fs::File;
    use tar::{Archive, Builder, Header};

    let input = File::open(from)?;
    let decoder = GzDecoder::new(input);
    let mut archive = Archive::new(decoder);
    let output = File::create(to)?;
    let encoder = GzEncoder::new(output, Compression::default());
    let mut builder = Builder::new(encoder);
    let mut mutate = Some(mutate);

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_path_buf();
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut bytes)?;
        if path == Path::new("MANIFEST.toml") {
            let rewrite = mutate
                .take()
                .ok_or_else(|| anyhow::anyhow!("MANIFEST.toml rewrite closure was already used"))?;
            bytes = rewrite(String::from_utf8(bytes)?).into_bytes();
        }
        let mut header = Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, &path, bytes.as_slice())?;
    }
    builder.finish()?;
    anyhow::ensure!(mutate.is_none(), "test package did not contain MANIFEST.toml");
    Ok(())
}
```

The helper implementation must preserve `MANIFEST` and `MANIFEST.sig` exactly and only mutate `MANIFEST.toml`.

- [ ] **Step 6: Run package writer and verify tests**

Run: `cargo test -p conary-core ccs::verify ccs::builder::package_writer -- --nocapture`

Expected: PASS.

- [ ] **Step 7: Commit Task 2**

```bash
git add crates/conary-core/src/ccs/manifest_provenance.rs crates/conary-core/src/ccs/verify.rs crates/conary-core/src/ccs/builder.rs crates/conary-core/src/ccs/builder/test_support.rs
git commit -m "security(ccs): bind attestations to manifest integrity"
```

## Task 3: Build Output Identity Excluding Signatures And Attestation

**Files:**
- Modify: `crates/conary-core/src/ccs/attestation.rs`
- Modify: `crates/conary-core/src/ccs/package.rs`
- Test: `crates/conary-core/src/ccs/attestation.rs`

- [ ] **Step 1: Write failing tests for content-only identity**

Add tests proving:

```rust
#[test]
fn output_identity_does_not_use_dna_hash() {
    let (_temp, package) = package_with_attestation_and_signature_for_tests("sha256:dna-a");
    let identity = compute_build_output_identity(&package).unwrap();

    assert_ne!(identity.canonical_content_identity, "sha256:dna-a");
}

#[test]
fn output_identity_survives_resigning_and_attestation_replacement() {
    let first_key = crate::ccs::signing::SigningKeyPair::generate().with_key_id("first");
    let second_key = crate::ccs::signing::SigningKeyPair::generate().with_key_id("second");
    let (_first_temp, first) = signed_package_for_identity_tests(&first_key, "m2-policy-v1");
    let (_second_temp, second) = signed_package_for_identity_tests(&second_key, "m2-policy-v2");

    let first_identity = compute_build_output_identity(&first).unwrap();
    let second_identity = compute_build_output_identity(&second).unwrap();

    assert_eq!(
        first_identity.canonical_content_identity,
        second_identity.canonical_content_identity
    );
}
```

- [ ] **Step 2: Add identity test fixtures**

Add these helpers in the `#[cfg(test)]` section of `crates/conary-core/src/ccs/attestation.rs`:

```rust
fn package_with_attestation_and_signature_for_tests(
    dna_hash: &str,
) -> (tempfile::TempDir, crate::ccs::package::CcsPackage) {
    let key = crate::ccs::signing::SigningKeyPair::generate().with_key_id("identity-test");
    let (temp, mut package) = signed_package_for_identity_tests(&key, "m2-policy-v1");
    package.manifest_mut_for_tests().provenance.dna_hash = Some(dna_hash.to_string());
    (temp, package)
}

fn signed_package_for_identity_tests(
    key: &crate::ccs::signing::SigningKeyPair,
    policy_digest: &str,
) -> (tempfile::TempDir, crate::ccs::package::CcsPackage) {
    let temp = tempfile::tempdir().unwrap();
    let package_path = temp.path().join("identity.ccs");
    let mut result = crate::ccs::builder::test_support::minimal_build_result("identity", "1.0");
    let mut payload = crate::ccs::attestation::test_support::sample_payload_for_tests();
    payload.publish_policy_digest = policy_digest.to_string();
    result.manifest.provenance.build_attestation =
        Some(crate::ccs::attestation::sign_build_attestation(payload, key).unwrap());
    crate::ccs::builder::write_signed_ccs_package(&result, &package_path, key).unwrap();
    let package = crate::ccs::package::CcsPackage::parse(package_path.to_str().unwrap()).unwrap();
    (temp, package)
}
```

Add this test-only mutator to `CcsPackage`:

```rust
#[cfg(test)]
pub(crate) fn manifest_mut_for_tests(&mut self) -> &mut CcsManifest {
    &mut self.manifest
}
```

- [ ] **Step 3: Run identity tests and confirm missing function failure**

Run: `cargo test -p conary-core output_identity_ -- --nocapture`

Expected: FAIL because `compute_build_output_identity` does not exist.

- [ ] **Step 4: Implement content-only identity**

In `crates/conary-core/src/ccs/attestation.rs`, add:

```rust
pub fn compute_build_output_identity(package: &crate::ccs::package::CcsPackage) -> Result<BuildOutputIdentity> {
    let manifest = package.manifest();
    let provenance = &manifest.provenance;
    let hardening_level = provenance
        .hardening_level
        .clone()
        .context("build output identity requires hardening_level")?;
    let origin_class = provenance
        .origin_class
        .clone()
        .context("build output identity requires origin_class")?;
    let hermetic_evidence_hash = provenance
        .hermetic_evidence
        .as_ref()
        .map(canonical_json_hash)
        .transpose()?
        .context("build output identity requires hermetic evidence")?;
    let file_merkle_root = provenance
        .merkle_root
        .clone()
        .or_else(|| package.binary_manifest().map(|manifest| manifest.content_root.value.clone()))
        .context("build output identity requires file Merkle root")?;
    let canonical_content_identity = compute_content_identity_excluding_signatures(package)?;
    Ok(BuildOutputIdentity {
        file_merkle_root,
        package_name: manifest.package.name.clone(),
        package_version: manifest.package.version.clone(),
        package_release: manifest.package.release.clone().unwrap_or_else(|| "1".to_string()),
        architecture: manifest.package.architecture.clone(),
        origin_class,
        hardening_level,
        hermetic_evidence_hash,
        canonical_content_identity,
    })
}

pub fn compute_content_identity_excluding_signatures(
    package: &crate::ccs::package::CcsPackage,
) -> Result<String> {
    let mut manifest = package.manifest().clone();
    manifest.provenance.build_attestation = None;
    manifest.provenance.signatures.clear();
    manifest.provenance.dna_hash = None;
    let manifest_bytes = serde_json::to_vec(&manifest).context("serialize content identity manifest projection")?;
    let components_bytes = serde_json::to_vec(package.components()).context("serialize content identity components")?;
    let files_bytes = serde_json::to_vec(package.file_entries()).context("serialize content identity files")?;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&manifest_bytes);
    bytes.extend_from_slice(&components_bytes);
    bytes.extend_from_slice(&files_bytes);
    Ok(format!("sha256:{}", hash::sha256(&bytes)))
}
```

In `crates/conary-core/src/ccs/package.rs`, store the parsed binary manifest on `CcsPackage` and expose it:

```rust
use crate::ccs::binary_manifest::BinaryManifest;

pub struct CcsPackage {
    package_path: PathBuf,
    manifest: CcsManifest,
    binary_manifest: Option<BinaryManifest>,
    files: Vec<FileEntry>,
    components: HashMap<String, ComponentData>,
    package_files: Vec<PackageFile>,
    dependencies: Vec<Dependency>,
    config_files_cache: Vec<ConfigFileInfo>,
}

impl CcsPackage {
    pub fn binary_manifest(&self) -> Option<&BinaryManifest> {
        self.binary_manifest.as_ref()
    }
}
```

In both `CcsPackage::parse` and `CcsPackage::parse_metadata`, set:

```rust
binary_manifest: contents.binary_manifest.clone(),
```

- [ ] **Step 5: Run identity tests**

Run: `cargo test -p conary-core output_identity_ -- --nocapture`

Expected: PASS.

- [ ] **Step 6: Commit Task 3**

```bash
git add crates/conary-core/src/ccs/attestation.rs crates/conary-core/src/ccs/package.rs
git commit -m "security(ccs): compute attested output identity"
```

## Task 4: Shared Command-Risk Taxonomy

**Files:**
- Create: `crates/conary-core/src/security/mod.rs`
- Create: `crates/conary-core/src/security/command_risk.rs`
- Modify: `crates/conary-core/src/lib.rs`
- Modify: `crates/conary-core/src/recipe/hermetic/command_risk.rs`
- Modify: `crates/conary-core/src/container/analysis.rs`
- Modify: `crates/conary-core/src/recipe/pkgbuild.rs`
- Test: `crates/conary-core/src/security/command_risk.rs`

- [ ] **Step 1: Write failing parity tests**

Create `crates/conary-core/src/security/command_risk.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aur_style_package_manager_commands_share_reason_codes() {
        let report = classify_shell_text("pkgbuild:prepare", "npm install atomic-lockfile\nbun add js-digest");

        assert_eq!(report.status, CommandRiskStatus::Blocked);
        assert!(report.entries.iter().any(|entry| entry.reason_code == PACKAGE_MANAGER_FETCH));
    }

    #[test]
    fn dynamic_exec_and_bpf_share_reason_codes() {
        let report = classify_shell_text("scriptlet:postinstall", "python -c 'print(1)'\nbpftool prog show");

        assert!(report.entries.iter().any(|entry| entry.reason_code == DYNAMIC_LANGUAGE_EXEC));
        assert!(report.entries.iter().any(|entry| entry.reason_code == BPF_OR_EBPF));
    }

    #[test]
    fn runtime_auto_sandbox_maps_shared_medium_signals() {
        let report = classify_shell_text("scriptlet:install", "node -e 'console.log(1)'");

        assert!(report.requires_runtime_sandbox());
    }
}
```

- [ ] **Step 2: Run parity tests and confirm missing module failure**

Run: `cargo test -p conary-core security::command_risk -- --nocapture`

Expected: FAIL because the security module does not exist.

- [ ] **Step 3: Add shared taxonomy module**

In `crates/conary-core/src/lib.rs`, add:

```rust
pub mod security;
```

Create `crates/conary-core/src/security/mod.rs`:

```rust
// conary-core/src/security/mod.rs

pub mod command_risk;
```

Create `crates/conary-core/src/security/command_risk.rs` with:

```rust
// conary-core/src/security/command_risk.rs
//! Shared command-risk taxonomy for build, conversion, and runtime scriptlet evidence.

use serde::{Deserialize, Serialize};

use crate::ccs::convert::command_evidence::extract_invocations_from_shell_text;

pub const COMMAND_RISK_CLASSIFIER_VERSION: &str = "m2-command-risk-v1";
pub const PACKAGE_MANAGER_FETCH: &str = "package-manager-fetch";
pub const NETWORK_FETCH: &str = "network-fetch";
pub const DYNAMIC_LANGUAGE_EXEC: &str = "dynamic-language-exec";
pub const CREDENTIAL_PATH: &str = "credential-path";
pub const OBFUSCATION: &str = "obfuscation";
pub const PERSISTENCE: &str = "persistence";
pub const BPF_OR_EBPF: &str = "bpf-or-ebpf";
pub const PROC_STEALTH_OR_DEBUG: &str = "proc-stealth-or-debug";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CommandRiskStatus {
    Clean,
    Review,
    Blocked,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandRiskReport {
    pub status: CommandRiskStatus,
    pub classifier_version: String,
    pub entries: Vec<CommandRiskEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandRiskEntry {
    pub source: String,
    pub command: String,
    pub reason_code: String,
    pub severity: CommandRiskStatus,
    pub evidence: String,
}

impl CommandRiskReport {
    pub fn clean() -> Self {
        Self {
            status: CommandRiskStatus::Clean,
            classifier_version: COMMAND_RISK_CLASSIFIER_VERSION.to_string(),
            entries: Vec::new(),
        }
    }

    pub fn requires_runtime_sandbox(&self) -> bool {
        self.entries.iter().any(|entry| {
            matches!(entry.reason_code.as_str(), PACKAGE_MANAGER_FETCH | NETWORK_FETCH | DYNAMIC_LANGUAGE_EXEC)
        })
    }
}

pub fn classify_shell_text(source: &str, content: &str) -> CommandRiskReport {
    let invocations = extract_invocations_from_shell_text(source, content, Some(source));
    let mut entries = Vec::new();
    for invocation in invocations {
        if let Some(reason_code) = reason_for_command(&invocation.command, &invocation.argv) {
            entries.push(CommandRiskEntry {
                source: source.to_string(),
                command: invocation.command,
                reason_code: reason_code.to_string(),
                severity: CommandRiskStatus::Blocked,
                evidence: invocation.raw_line.unwrap_or_else(|| content.trim().to_string()),
            });
        }
    }
    for line in content.lines().map(str::trim).filter(|line| !line.is_empty() && !line.starts_with('#')) {
        for reason_code in raw_line_reasons(line) {
            if !entries.iter().any(|entry| entry.reason_code == reason_code && entry.evidence == line) {
                entries.push(CommandRiskEntry {
                    source: source.to_string(),
                    command: reason_code.to_string(),
                    reason_code: reason_code.to_string(),
                    severity: CommandRiskStatus::Blocked,
                    evidence: line.to_string(),
                });
            }
        }
    }
    if entries.is_empty() {
        CommandRiskReport::clean()
    } else {
        CommandRiskReport {
            status: CommandRiskStatus::Blocked,
            classifier_version: COMMAND_RISK_CLASSIFIER_VERSION.to_string(),
            entries,
        }
    }
}

fn reason_for_command(command: &str, argv: &[String]) -> Option<&'static str> {
    let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
    if matches!(
        command,
        "npm" | "npx" | "pnpm" | "yarn" | "bun" | "pip" | "pip3" | "gem"
    ) || matches!(command, "cargo" | "go") && argv.first().is_some_and(|arg| *arg == "install")
    {
        return Some(PACKAGE_MANAGER_FETCH);
    }
    if matches!(command, "curl" | "wget" | "aria2c" | "fetch")
        || command == "git" && argv.first().is_some_and(|arg| *arg == "clone")
    {
        return Some(NETWORK_FETCH);
    }
    if command == "node" && argv_contains(&argv, "-e")
        || command.starts_with("python") && argv_contains(&argv, "-c")
        || matches!(command, "perl" | "ruby") && argv_contains(&argv, "-e")
    {
        return Some(DYNAMIC_LANGUAGE_EXEC);
    }
    if command == "eval"
        || command == "base64" && (argv_contains(&argv, "-d") || argv_contains(&argv, "--decode"))
    {
        return Some(OBFUSCATION);
    }
    if command == "crontab" || command == "systemctl" && argv_contains(&argv, "enable") {
        return Some(PERSISTENCE);
    }
    if matches!(command, "bpf" | "bpftool") || command.contains("bpf") {
        return Some(BPF_OR_EBPF);
    }
    if matches!(command, "ptrace" | "strace" | "gdb") {
        return Some(PROC_STEALTH_OR_DEBUG);
    }
    None
}

fn raw_line_reasons(line: &str) -> Vec<&'static str> {
    let lower = line.to_ascii_lowercase();
    let mut reasons = Vec::new();
    if ["npm", "npx", "pnpm", "yarn", "bun", "pip", "pip3", "gem"]
        .iter()
        .any(|command| contains_shell_word(&lower, command))
        || contains_shell_words(&lower, "cargo", "install")
        || contains_shell_words(&lower, "go", "install")
    {
        reasons.push(PACKAGE_MANAGER_FETCH);
    }
    if ["curl", "wget", "aria2c", "fetch"]
        .iter()
        .any(|command| contains_shell_word(&lower, command))
        || contains_shell_words(&lower, "git", "clone")
    {
        reasons.push(NETWORK_FETCH);
    }
    if contains_shell_words(&lower, "node", "-e")
        || contains_shell_words(&lower, "python", "-c")
        || contains_shell_words(&lower, "python3", "-c")
        || contains_shell_words(&lower, "perl", "-e")
        || contains_shell_words(&lower, "ruby", "-e")
    {
        reasons.push(DYNAMIC_LANGUAGE_EXEC);
    }
    if lower.contains(".npmrc")
        || lower.contains(".pypirc")
        || lower.contains(".cargo/credentials")
        || lower.contains("ssh/id_")
        || lower.contains("token")
    {
        reasons.push(CREDENTIAL_PATH);
    }
    if contains_shell_word(&lower, "eval") || contains_shell_words(&lower, "base64", "-d") {
        reasons.push(OBFUSCATION);
    }
    if contains_shell_word(&lower, "crontab") || contains_shell_words(&lower, "systemctl", "enable") {
        reasons.push(PERSISTENCE);
    }
    if lower.contains("bpf") || lower.contains("ebpf") || lower.contains("libbpf") {
        reasons.push(BPF_OR_EBPF);
    }
    if lower.contains("ptrace") || lower.contains("strace") || lower.contains("/proc/") {
        reasons.push(PROC_STEALTH_OR_DEBUG);
    }
    reasons
}

fn argv_contains(argv: &[&str], needle: &str) -> bool {
    argv.contains(&needle)
}

fn contains_shell_words(line: &str, first: &str, second: &str) -> bool {
    contains_shell_word(line, first) && contains_shell_word(line, second)
}

fn contains_shell_word(line: &str, word: &str) -> bool {
    line.split(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-' && c != '.')
        .any(|part| part == word)
}
```

After this module compiles, delete the duplicated command/risk predicate functions from `recipe/hermetic/command_risk.rs`; `PACKAGE_MANAGER_FETCH`, `NETWORK_FETCH`, and the other constants in `security::command_risk` become the only reason-code strings.

- [ ] **Step 4: Adapt hermetic build reports**

In `crates/conary-core/src/recipe/hermetic/command_risk.rs`, keep `collect_recipe_command_text` and make `classify_build_commands` delegate to the shared module:

```rust
pub fn classify_build_commands(commands: &[BuildCommandText]) -> BuildCommandRiskReport {
    let mut entries = Vec::new();
    for command_text in commands {
        let report = crate::security::command_risk::classify_shell_text(
            &format!("recipe:{}", command_text.phase),
            &command_text.content,
        );
        entries.extend(report.entries.into_iter().map(|entry| BuildCommandRiskEntry {
            phase: command_text.phase.clone(),
            command: entry.command,
            reason_code: entry.reason_code,
            severity: match entry.severity {
                crate::security::command_risk::CommandRiskStatus::Clean => PolicyStatus::Clean,
                crate::security::command_risk::CommandRiskStatus::Review => PolicyStatus::Review,
                crate::security::command_risk::CommandRiskStatus::Blocked => PolicyStatus::Blocked,
            },
            evidence: entry.evidence,
        }));
    }
    if entries.is_empty() {
        BuildCommandRiskReport::clean()
    } else {
        BuildCommandRiskReport {
            status: PolicyStatus::Blocked,
            classifier_version: crate::security::command_risk::COMMAND_RISK_CLASSIFIER_VERSION.to_string(),
            entries,
        }
    }
}
```

- [ ] **Step 5: Adapt runtime scriptlet analysis**

In `crates/conary-core/src/container/analysis.rs`, keep existing high-risk regexes and add shared classification before recommendations:

```rust
let shared_report = crate::security::command_risk::classify_shell_text("runtime-scriptlet", content);
if shared_report.requires_runtime_sandbox() && max_risk < ScriptRisk::Medium {
    max_risk = ScriptRisk::Medium;
}
for entry in shared_report.entries {
    patterns.push(format!("{} ({})", entry.reason_code, ScriptRisk::Medium.as_str()));
}
```

- [ ] **Step 6: Add PKGBUILD risk report helper**

In `crates/conary-core/src/recipe/pkgbuild.rs`, add:

```rust
pub fn classify_pkgbuild_function_bodies_for_risk(
    content: &str,
) -> crate::security::command_risk::CommandRiskReport {
    let mut entries = Vec::new();
    for (phase, body) in extract_pkgbuild_function_bodies_for_risk(content) {
        let report = crate::security::command_risk::classify_shell_text(
            &format!("pkgbuild:{phase}"),
            &body,
        );
        entries.extend(report.entries);
    }
    if entries.is_empty() {
        crate::security::command_risk::CommandRiskReport::clean()
    } else {
        crate::security::command_risk::CommandRiskReport {
            status: crate::security::command_risk::CommandRiskStatus::Blocked,
            classifier_version: crate::security::command_risk::COMMAND_RISK_CLASSIFIER_VERSION.to_string(),
            entries,
        }
    }
}
```

- [ ] **Step 7: Run shared risk tests**

Run: `cargo test -p conary-core command_risk package_manager_fetches_are_medium_for_auto_sandbox extract_pkgbuild_function_bodies_for_risk_returns_prepare_body -- --nocapture`

Expected: PASS.

- [ ] **Step 8: Commit Task 4**

```bash
git add crates/conary-core/src/lib.rs crates/conary-core/src/security crates/conary-core/src/recipe/hermetic/command_risk.rs crates/conary-core/src/container/analysis.rs crates/conary-core/src/recipe/pkgbuild.rs
git commit -m "security: share command risk taxonomy"
```

## Task 5: Static Publish Gate And Accepted Signers

**Files:**
- Create: `crates/conary-core/src/repository/static_repo/publish_gate.rs`
- Modify: `crates/conary-core/src/repository/static_repo/mod.rs`
- Modify: `crates/conary-core/src/repository/static_repo/format.rs`
- Test: `crates/conary-core/src/repository/static_repo/publish_gate.rs`

- [ ] **Step 1: Write failing accepted signer tests**

Create tests in `publish_gate.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::static_repo::{PackageKeyEntry, PackageKeyStatus, PackageKeysFile};

    fn package_key(id: &str, public_key: &str, status: PackageKeyStatus) -> PackageKeyEntry {
        PackageKeyEntry {
            algorithm: "ed25519".to_string(),
            public_key: public_key.to_string(),
            key_id: Some(id.to_string()),
            status,
            comment: None,
        }
    }

    #[test]
    fn accepted_signers_include_only_active_package_keys() {
        let keys = PackageKeysFile {
            schema: 1,
            keys: vec![
                package_key("active", "pub-active", PackageKeyStatus::Active),
                package_key("retired", "pub-retired", PackageKeyStatus::Retired),
            ],
        };
        let set = AcceptedStaticSignerSet::from_verified_package_keys(&keys).unwrap();

        assert!(set.accepts_key_id("active"));
        assert!(!set.accepts_key_id("retired"));
    }

    #[test]
    fn retired_signer_cannot_authorize_new_publish() {
        let keys = PackageKeysFile {
            schema: 1,
            keys: vec![package_key("retired", "pub-retired", PackageKeyStatus::Retired)],
        };
        let err = AcceptedStaticSignerSet::from_verified_package_keys(&keys).unwrap_err();

        assert!(err.to_string().contains("no active package keys"));
    }
}
```

- [ ] **Step 2: Run accepted signer tests and confirm missing module failure**

Run: `cargo test -p conary-core static_repo::publish_gate -- --nocapture`

Expected: FAIL because `publish_gate` is not exported.

- [ ] **Step 3: Export the gate module**

In `crates/conary-core/src/repository/static_repo/mod.rs`, add:

```rust
pub mod publish_gate;
```

- [ ] **Step 4: Implement gate DTOs and accepted signer set**

Create `crates/conary-core/src/repository/static_repo/publish_gate.rs`:

```rust
// conary-core/src/repository/static_repo/publish_gate.rs
//! Static artifact-form publish eligibility and signer authority checks.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::ccs::attestation::{BuildAttestationEnvelope, compute_build_output_identity};
use crate::ccs::package::CcsPackage;
use crate::repository::static_repo::{PackageKeyStatus, PackageKeysFile};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedStaticSignerSet {
    active_keys: BTreeMap<String, String>,
}

impl AcceptedStaticSignerSet {
    pub fn from_verified_package_keys(keys: &PackageKeysFile) -> Result<Self> {
        let active_keys = keys
            .keys
            .iter()
            .filter(|key| matches!(key.status, PackageKeyStatus::Active))
            .map(|key| {
                (
                    key.key_id.clone().unwrap_or_else(|| key.public_key.clone()),
                    key.public_key.clone(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        if active_keys.is_empty() {
            bail!("no active package keys can authorize new artifact publish");
        }
        Ok(Self { active_keys })
    }

    pub fn from_initial_key(key_id: impl Into<String>, public_key: impl Into<String>) -> Self {
        Self {
            active_keys: BTreeMap::from([(key_id.into(), public_key.into())]),
        }
    }

    pub fn accepts_key_id(&self, key_id: &str) -> bool {
        self.active_keys.contains_key(key_id)
    }

    pub fn public_key_for(&self, key_id: &str) -> Option<&str> {
        self.active_keys.get(key_id).map(String::as_str)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PublishGateStatus {
    Passed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublishLintReport {
    pub status: PublishGateStatus,
    pub failures: Vec<PublishGateFailure>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublishGateFailure {
    pub code: PublishGateFailureCode,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PublishGateFailureCode {
    MissingAttestation,
    BuildAttestationSignatureMismatch,
    PackageSignatureMismatch,
    OutputIdentityMismatch,
    UnacceptedSignerKey,
    RetiredSignerKey,
    AbsentOrUnknownProvenanceClass,
    NonHermeticHardeningLevel,
    StaleOrUnknownPolicy,
    UncleanCommandRiskReport,
    ForeignConversionMissingBoundary,
    ForeignConversionBoundaryHashMismatch,
    RecordedDraftArtifact,
}

impl PublishLintReport {
    pub fn passed() -> Self {
        Self {
            status: PublishGateStatus::Passed,
            failures: Vec::new(),
        }
    }

    pub fn failed(failures: Vec<PublishGateFailure>) -> Self {
        Self {
            status: PublishGateStatus::Failed,
            failures,
        }
    }

    pub fn is_passed(&self) -> bool {
        self.status == PublishGateStatus::Passed
    }
}

pub fn verify_static_artifact_publish_eligibility(
    package: &CcsPackage,
    accepted_signers: &AcceptedStaticSignerSet,
    accepted_policy_digest: &str,
) -> Result<PublishLintReport> {
    let provenance = &package.manifest().provenance;
    let Some(envelope) = provenance.build_attestation.as_ref() else {
        return Ok(PublishLintReport::failed(vec![failure(
            PublishGateFailureCode::MissingAttestation,
            "artifact is missing a build attestation",
        )]));
    };
    verify_static_attestation(package, envelope, accepted_signers, accepted_policy_digest)
}

fn verify_static_attestation(
    package: &CcsPackage,
    envelope: &BuildAttestationEnvelope,
    accepted_signers: &AcceptedStaticSignerSet,
    accepted_policy_digest: &str,
) -> Result<PublishLintReport> {
    let mut failures = Vec::new();
    if envelope.payload.hardening_level != "hermetic" {
        failures.push(failure(
            PublishGateFailureCode::NonHermeticHardeningLevel,
            "artifact is not hermetic",
        ));
    }
    if envelope.payload.origin_class == "recorded-draft" {
        failures.push(failure(
            PublishGateFailureCode::RecordedDraftArtifact,
            "recorded-draft artifacts are not publishable",
        ));
    }
    if envelope.payload.publish_policy_digest != accepted_policy_digest {
        failures.push(failure(
            PublishGateFailureCode::StaleOrUnknownPolicy,
            "build attestation policy digest is not accepted",
        ));
    }
    let public_key = accepted_signers.public_key_for(&envelope.signer_key_id);
    let Some(public_key) = public_key else {
        failures.push(failure(
            PublishGateFailureCode::UnacceptedSignerKey,
            "build attestation signer is not accepted for this static target",
        ));
        return Ok(PublishLintReport::failed(failures));
    };
    if crate::ccs::attestation::verify_build_attestation_envelope(envelope, public_key).is_err() {
        failures.push(failure(
            PublishGateFailureCode::BuildAttestationSignatureMismatch,
            "build attestation signature mismatch",
        ));
    }
    let actual_identity = compute_build_output_identity(package).context("compute artifact output identity")?;
    if actual_identity != envelope.payload.output_identity {
        failures.push(failure(
            PublishGateFailureCode::OutputIdentityMismatch,
            "build attestation output identity does not match artifact",
        ));
    }
    if failures.is_empty() {
        Ok(PublishLintReport::passed())
    } else {
        Ok(PublishLintReport::failed(failures))
    }
}

fn failure(code: PublishGateFailureCode, message: &str) -> PublishGateFailure {
    PublishGateFailure {
        code,
        message: message.to_string(),
    }
}
```

- [ ] **Step 5: Run publish gate tests**

Run: `cargo test -p conary-core static_repo::publish_gate -- --nocapture`

Expected: PASS.

- [ ] **Step 6: Commit Task 5**

```bash
git add crates/conary-core/src/repository/static_repo/mod.rs crates/conary-core/src/repository/static_repo/publish_gate.rs
git commit -m "security(static): add artifact publish gate"
```

## Task 6: Prepared Static Publish Context

**Files:**
- Create: `crates/conary-core/src/repository/static_repo/publish_context.rs`
- Modify: `crates/conary-core/src/repository/static_repo/mod.rs`
- Modify: `crates/conary-core/src/repository/static_repo/publish.rs`
- Test: `crates/conary-core/src/repository/static_repo/publish_context.rs`

- [ ] **Step 1: Write failing context tests**

Tests must prove:

```rust
#[test]
fn new_repo_requires_explicit_key_dir_for_artifact_form() {
    let err = StaticPublishPrepareOptions::artifact_form_new_repo_without_key_dir_for_tests()
        .prepare()
        .unwrap_err();

    assert!(err.to_string().contains("artifact-form publish to a new static repo requires --key-dir"));
}

#[test]
fn existing_repo_uses_verified_active_package_keys() {
    let temp = tempfile::tempdir().unwrap();
    let context = prepare_existing_repo_context_for_tests(temp.path()).unwrap();

    assert!(context.accepted_signers.accepts_key_id(&context.active_publish_key_id));
}
```

- [ ] **Step 2: Add context test fixtures**

Add these helpers to the `#[cfg(test)]` module in `publish_context.rs`:

```rust
impl StaticPublishPrepareOptions {
    fn artifact_form_new_repo_without_key_dir_for_tests() -> Self {
        Self {
            destination: RepoLocation::File {
                root: std::env::temp_dir().join("conary-m2-new-repo-without-key-dir"),
            },
            key_dir: None,
            publish_form: StaticPublishForm::Artifact,
            force_reinit: true,
        }
    }
}

fn prepare_existing_repo_context_for_tests(
    root: &std::path::Path,
) -> anyhow::Result<PreparedStaticPublishContext> {
    let key_dir = root.join("keys-local");
    std::fs::create_dir_all(&key_dir)?;
    let key = crate::ccs::signing::SigningKeyPair::generate().with_key_id("publish");
    key.save_to_files(&key_dir.join("publish.private"), &key_dir.join("publish.public"))?;
    let repo_root = root.join("repo");
    std::fs::create_dir_all(repo_root.join("keys"))?;
    let keys = PackageKeysFile {
        schema: 1,
        keys: vec![crate::repository::static_repo::PackageKeyEntry {
            algorithm: "ed25519".to_string(),
            public_key: key.public_key_base64(),
            key_id: Some("publish".to_string()),
            status: crate::repository::static_repo::PackageKeyStatus::Active,
            comment: Some("test active key".to_string()),
        }],
    };
    std::fs::write(
        repo_root.join("keys/package-keys.json"),
        serde_json::to_string_pretty(&keys)?,
    )?;
    StaticPublishPrepareOptions {
        destination: RepoLocation::File { root: repo_root },
        key_dir: Some(key_dir),
        publish_form: StaticPublishForm::Artifact,
        force_reinit: false,
    }
    .prepare()
}
```

- [ ] **Step 3: Run context tests and confirm missing module failure**

Run: `cargo test -p conary-core static_repo::publish_context -- --nocapture`

Expected: FAIL because `publish_context` does not exist.

- [ ] **Step 4: Export publish context**

In `crates/conary-core/src/repository/static_repo/mod.rs`, add:

```rust
pub mod publish_context;
```

- [ ] **Step 5: Implement prepared context API**

Create `crates/conary-core/src/repository/static_repo/publish_context.rs`:

```rust
// conary-core/src/repository/static_repo/publish_context.rs
//! Prepared static publish context: target authority before package commit.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use crate::ccs::signing::SigningKeyPair;
use crate::repository::static_repo::publish_gate::AcceptedStaticSignerSet;
use crate::repository::static_repo::{PackageKeysFile, RepoLocation};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaticPublishForm {
    Project,
    Artifact,
}

pub struct StaticPublishPrepareOptions {
    pub destination: RepoLocation,
    pub key_dir: Option<PathBuf>,
    pub publish_form: StaticPublishForm,
    pub force_reinit: bool,
}

pub struct PreparedStaticPublishContext {
    pub destination: RepoLocation,
    pub key_dir: PathBuf,
    pub active_publish_key: SigningKeyPair,
    pub active_publish_key_id: String,
    pub accepted_signers: AcceptedStaticSignerSet,
    pub publish_policy_digest: String,
}

impl StaticPublishPrepareOptions {
    pub fn prepare(self) -> Result<PreparedStaticPublishContext> {
        let key_dir = match self.key_dir {
            Some(key_dir) => key_dir,
            None if self.publish_form == StaticPublishForm::Artifact && self.force_reinit => {
                bail!("artifact-form publish to a new static repo requires --key-dir");
            }
            None => bail!("static publish requires a key directory"),
        };
        let active_publish_key = SigningKeyPair::load_from_file(&key_dir.join("publish.private"))
            .with_context(|| {
                format!(
                    "load active static publish key from {}",
                    key_dir.join("publish.private").display()
                )
            })?;
        let public_key = active_publish_key.public_key_base64();
        let active_publish_key_id = active_publish_key
            .key_id()
            .map(str::to_string)
            .unwrap_or_else(|| public_key.clone());
        let accepted_signers = match self.publish_form {
            StaticPublishForm::Project => {
                AcceptedStaticSignerSet::from_initial_key(active_publish_key_id.clone(), public_key)
            }
            StaticPublishForm::Artifact => {
                if self.force_reinit {
                    AcceptedStaticSignerSet::from_initial_key(active_publish_key_id.clone(), public_key)
                } else {
                    let package_keys = load_verified_package_keys_for_destination(&self.destination)?;
                    AcceptedStaticSignerSet::from_verified_package_keys(&package_keys)?
                }
            }
        };
        Ok(PreparedStaticPublishContext {
            destination: self.destination,
            key_dir,
            active_publish_key,
            active_publish_key_id,
            accepted_signers,
            publish_policy_digest: "m2-static-publish-policy-v1".to_string(),
        })
    }
}

fn load_verified_package_keys_for_destination(destination: &RepoLocation) -> Result<PackageKeysFile> {
    let RepoLocation::File { root } = destination else {
        bail!("static publish context can only load package keys from file destinations");
    };
    let bytes = std::fs::read_to_string(root.join("keys/package-keys.json"))
        .with_context(|| format!("read {}", root.join("keys/package-keys.json").display()))?;
    PackageKeysFile::parse(&bytes).context("parse verified package keys")
}
```

Before committing this task, replace `load_verified_package_keys_for_destination` with a call into the same verified static metadata loading path that `publish.rs` uses for `keys/package-keys.json`. The raw file read in the snippet is acceptable only for the first failing unit test; the committed code must verify destination metadata before deriving `AcceptedStaticSignerSet`.

- [ ] **Step 6: Split `publish.rs` into prepare and commit phases**

Keep `publish_static_repo(options)` as the public entrypoint. Internally, add a helper that accepts `PreparedStaticPublishContext`:

```rust
fn commit_static_publish(
    options: StaticPublishOptions,
    context: PreparedStaticPublishContext,
    forced: ForcedRefresh,
) -> Result<StaticPublishOutcome> {
    // Existing publish_static_repo_inner body moves here.
}
```

`publish_static_repo_inner` should prepare context once, then call `commit_static_publish`.

- [ ] **Step 7: Keep `publish.rs` line budget honest**

Run:

```bash
wc -l crates/conary-core/src/repository/static_repo/publish.rs
```

Expected: line count is `<= 2699`.

- [ ] **Step 8: Run static publish tests**

Run: `cargo test -p conary-core static_repo::publish -- --nocapture`

Expected: PASS.

- [ ] **Step 9: Commit Task 6**

```bash
git add crates/conary-core/src/repository/static_repo/mod.rs crates/conary-core/src/repository/static_repo/publish.rs crates/conary-core/src/repository/static_repo/publish_context.rs
git commit -m "security(static): prepare publish authority context"
```

## Task 7: Project-Form Attestation Signing

**Files:**
- Modify: `apps/conary/src/commands/publish.rs`
- Modify: `crates/conary-core/src/repository/static_repo/publish_context.rs`
- Modify: `crates/conary-core/src/ccs/builder/package_writer.rs`
- Test: `apps/conary/tests/packaging_m2a.rs`
- Test: `apps/conary/src/commands/publish.rs`

- [ ] **Step 1: Write failing CLI tests**

Add tests that prove project-form publish refuses dirty local trees regardless of ambient `CI` and emits an attestation on success:

```rust
#[tokio::test]
async fn project_form_publish_uses_release_dirty_tree_refusal() {
    let _ci = temp_env::with_var("CI", Some("false"));
    let error = publish_dirty_git_fixture_for_tests().await.unwrap_err();

    assert!(error.to_string().contains("local tree is dirty"));
}

#[test]
fn publish_kitchen_config_uses_offline_cache_only_for_release() {
    let recipe_path = std::path::Path::new("/work/pkg/recipe.toml");
    let output_dir = std::path::Path::new("/tmp/conary-publish-out");
    let sysroot = std::path::PathBuf::from("/tmp/sysroot");
    let config = publish_kitchen_config(recipe_path, output_dir, sysroot);

    assert!(!config.allow_network);
    assert_eq!(
        config.source_download_policy,
        conary_core::recipe::SourceDownloadPolicy::OfflineCacheOnly
    );
}
```

- [ ] **Step 2: Run publish tests and confirm failures**

Run: `cargo test -p conary publish_kitchen_config_uses_offline_cache_only_for_release project_form_publish_uses_release_dirty_tree_refusal -- --nocapture`

Expected: FAIL because project-form publish still uses `AllowDownloads` and ambient `detect_ci_mode()`.

- [ ] **Step 3: Add release CI mode helper**

In `apps/conary/src/commands/publish.rs`, add:

```rust
fn release_publish_ci_mode() -> conary_core::recipe::hermetic::CiMode {
    conary_core::recipe::hermetic::CiMode::On
}
```

Change project-form publish from:

```rust
.cook_hermetic(&recipe, hermetic_input, output_dir.path(), detect_ci_mode())
```

to:

```rust
.cook_hermetic(&recipe, hermetic_input, output_dir.path(), release_publish_ci_mode())
```

- [ ] **Step 4: Assert offline build policy at publish boundary**

In `publish_kitchen_config`, change:

```rust
source_download_policy: SourceDownloadPolicy::OfflineCacheOnly,
```

Add a local guard before Kitchen construction:

```rust
fn assert_release_offline_build_config(config: &KitchenConfig) -> Result<()> {
    if config.allow_network {
        bail!("release publish requires allow_network=false at the build boundary");
    }
    if config.source_download_policy != SourceDownloadPolicy::OfflineCacheOnly {
        bail!("release publish requires source_download_policy=OfflineCacheOnly at the build boundary");
    }
    Ok(())
}
```

Call it immediately before `Kitchen::new(config)`.

- [ ] **Step 5: Build and embed project-form attestation**

After `cook_hermetic`, use `PreparedStaticPublishContext` to sign a payload before committing static publish:

```rust
let prepared = prepare_project_form_static_context(&destination, &key_dir, options.force_reinit)?;
let attested_package_path = attach_project_form_attestation(
    &result.package_path,
    result.provenance.as_ref(),
    &prepared,
)?;
```

`attach_project_form_attestation` must:

- parse the unsigned/hermetic package
- compute `BuildOutputIdentity`
- build `BuildAttestationPayload`
- sign it with `prepared.active_publish_key`
- set `manifest.provenance.build_attestation`
- rewrite the CCS package with `write_signed_ccs_package`
- re-run `verify_package`
- re-run `verify_static_artifact_publish_eligibility`

Use `attested_package_path` in `StaticPublishOptions.package_paths`.

- [ ] **Step 6: Remove the M2a preview message**

Replace:

```rust
println!(
    "M2a static publish records hermetic build evidence, but release attestation gates arrive in M2b."
);
```

with:

```rust
println!("Cooking and attesting {} {} for static release publish...", recipe.package.name, recipe.package.version);
```

- [ ] **Step 7: Run project-form publish tests**

Run: `cargo test -p conary publish_ -- --nocapture`

Expected: PASS.

- [ ] **Step 8: Commit Task 7**

```bash
git add apps/conary/src/commands/publish.rs apps/conary/tests/packaging_m2a.rs crates/conary-core/src/repository/static_repo/publish_context.rs crates/conary-core/src/ccs/builder/package_writer.rs
git commit -m "security(publish): sign project-form build attestations"
```

## Task 8: Static Artifact-Form Publish

**Files:**
- Modify: `apps/conary/src/cli/mod.rs`
- Modify: `apps/conary/src/commands/publish.rs`
- Modify: `crates/conary-core/src/repository/static_repo/publish.rs`
- Modify: `crates/conary-core/src/repository/static_repo/publish_gate.rs`
- Test: `apps/conary/src/cli/mod.rs`
- Test: `apps/conary/src/commands/publish.rs`
- Test: `apps/conary/tests/packaging_m2a.rs`

- [ ] **Step 1: Rewrite the artifact-form rejection test into gate tests**

Replace `artifact_form_publish_is_rejected_in_m1a` with tests for:

```rust
#[tokio::test]
async fn artifact_form_publish_rejects_missing_attestation() {
    let error = publish_artifact_fixture_without_attestation_for_tests().await.unwrap_err();

    assert!(error.to_string().contains("artifact is missing a build attestation"));
}

#[tokio::test]
async fn artifact_form_publish_rejects_unaccepted_signer() {
    let error = publish_artifact_fixture_signed_by_other_key_for_tests().await.unwrap_err();

    assert!(error.to_string().contains("build attestation signer is not accepted"));
}

#[tokio::test]
async fn artifact_form_publish_rejects_recorded_draft() {
    let error = publish_recorded_draft_artifact_for_tests().await.unwrap_err();

    assert!(error.to_string().contains("recorded-draft artifacts are not publishable"));
}
```

- [ ] **Step 2: Run artifact-form tests and confirm current rejection failure**

Run: `cargo test -p conary artifact_form_publish_ -- --nocapture`

Expected: FAIL because `cmd_publish` still returns the old unsupported-feature message.

- [ ] **Step 3: Unhide artifact-form CLI help**

In `apps/conary/src/cli/mod.rs`, change the publish command docs:

```rust
/// Publish a recipe project or attested CCS artifact
Publish {
    /// Project-form destination, or artifact path when TARGET is present
    what: String,

    /// Artifact-form destination target
    target: Option<String>,
```

Update CLI help tests so `publish` help contains artifact-form syntax and attestation wording.

- [ ] **Step 4: Split publish command flow**

In `apps/conary/src/commands/publish.rs`, replace the early artifact rejection with:

```rust
pub async fn cmd_publish(options: PublishOptions) -> Result<()> {
    match options.target.as_deref() {
        Some(target) => publish_artifact_form(options, target).await,
        None => publish_project_form(options).await,
    }
}
```

Move the existing project-form body into `publish_project_form`.

- [ ] **Step 5: Implement static artifact-form flow**

Add:

```rust
async fn publish_artifact_form(options: PublishOptions, target: &str) -> Result<()> {
    let artifact_path = PathBuf::from(&options.what);
    let destination = RepoLocation::parse(target)
        .with_context(|| format!("parse publish target {target}"))?;
    ensure_static_local_publish_destination(&destination)?;
    let repo_name = derive_repo_name(target)?;
    let key_dir = resolve_key_dir(options.key_dir.as_deref(), &repo_name)?;
    let prepared = prepare_artifact_form_static_context(&destination, &key_dir, options.force_reinit)?;
    let package = conary_core::ccs::CcsPackage::parse(
        artifact_path
            .to_str()
            .with_context(|| format!("package path is not valid UTF-8: {}", artifact_path.display()))?,
    )
    .map_err(anyhow::Error::from)?;
    let report = conary_core::repository::static_repo::publish_gate::verify_static_artifact_publish_eligibility(
        &package,
        &prepared.accepted_signers,
        &prepared.publish_policy_digest,
    )?;
    if !report.is_passed() {
        bail!("{}", format_publish_gate_failures(&report));
    }
    let state_file = options
        .state_file
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| key_dir.join("last-published.toml"));
    let outcome = publish_static_repo(StaticPublishOptions {
        repo_name: repo_name.clone(),
        repo_description: None,
        destination,
        key_dir,
        state_file,
        package_paths: vec![artifact_path],
        refresh: options.refresh,
        force_reinit: options.force_reinit,
        accept_destination_state: options.accept_destination_state,
        rotate_publish_key: options.rotate_publish_key,
        rotate_root_key: options.rotate_root_key,
    })?;
    println!("Published attested artifact to static repo: {repo_name}");
    println!("Publish key ID: {}", outcome.publish_key_id);
    Ok(())
}
```

Rename `ensure_m1a_publish_destination` to:

```rust
fn ensure_static_local_publish_destination(destination: &RepoLocation) -> Result<()> {
    if matches!(destination, RepoLocation::Http { .. }) {
        bail!("static publisher supports local filesystem destinations; Remi HTTP(S) targets use the Remi release path");
    }
    Ok(())
}
```

- [ ] **Step 6: Enforce gate inside static publish too**

Add an internal gate call in `stage_packages` or the new commit layer so a CLI bypass cannot publish an ungated artifact. Use the prepared context from Task 6. The static publisher should reject before `write_pending_package`.

- [ ] **Step 7: Run static artifact-form tests**

Run: `cargo test -p conary artifact_form_publish_ publish_artifact_form -- --nocapture`

Expected: PASS.

- [ ] **Step 8: Run core static publish tests**

Run: `cargo test -p conary-core static_repo::publish static_repo::publish_gate -- --nocapture`

Expected: PASS.

- [ ] **Step 9: Commit Task 8**

```bash
git add apps/conary/src/cli/mod.rs apps/conary/src/commands/publish.rs apps/conary/tests/packaging_m2a.rs crates/conary-core/src/repository/static_repo/publish.rs crates/conary-core/src/repository/static_repo/publish_gate.rs
git commit -m "security(publish): enable attested artifact-form publish"
```

## Task 9: Foreign Conversion Boundary DTO And File-Output Cook

**Files:**
- Modify: `crates/conary-core/src/ccs/attestation.rs`
- Modify: `crates/conary-core/src/ccs/convert/converter.rs`
- Modify: `crates/conary-core/src/ccs/convert/mod.rs`
- Modify: `apps/conary/src/commands/cook.rs`
- Test: `crates/conary-core/src/ccs/convert/converter.rs`
- Test: `apps/conary/src/commands/cook.rs`

- [ ] **Step 1: Write failing foreign boundary tests**

Add tests for:

```rust
#[test]
fn foreign_boundary_hash_changes_when_source_checksum_changes() {
    let mut boundary = ForeignConversionBoundary::for_tests("rpm", "sha256:one");
    let first = canonical_json_hash(&boundary).unwrap();
    boundary.source_checksum = "sha256:two".to_string();
    let second = canonical_json_hash(&boundary).unwrap();

    assert_ne!(first, second);
}

#[test]
fn foreign_converted_publish_requires_boundary_hash() {
    let package = foreign_converted_package_without_boundary_for_tests();
    let report = verify_static_artifact_publish_eligibility(
        &package,
        &accepted_signers_for_tests(),
        "m2-static-publish-policy-v1",
    )
    .unwrap();

    assert!(report.failures.iter().any(|failure| {
        failure.code == PublishGateFailureCode::ForeignConversionMissingBoundary
    }));
}
```

- [ ] **Step 2: Run tests and confirm boundary gate failure**

Run: `cargo test -p conary-core foreign_boundary foreign_converted_publish_requires_boundary_hash -- --nocapture`

Expected: FAIL because the boundary is not attached or checked.

- [ ] **Step 3: Add boundary hash to attestation payload**

Use `ForeignConversionBoundary` from Task 1. Ensure `BuildAttestationPayload.conversion_boundary_hash` is required when `origin_class == "foreign-converted"` in `publish_gate.rs`:

```rust
if envelope.payload.origin_class == "foreign-converted" && envelope.payload.conversion_boundary_hash.is_none() {
    failures.push(failure(
        PublishGateFailureCode::ForeignConversionMissingBoundary,
        "foreign-converted artifact is missing conversion boundary metadata",
    ));
}
```

- [ ] **Step 4: Attach boundary during conversion**

In `crates/conary-core/src/ccs/convert/converter.rs`, populate:

```rust
manifest.provenance.origin_class = Some("foreign-converted".to_string());
manifest.provenance.hardening_level = Some("hermetic".to_string());
manifest.provenance.foreign_conversion_boundary = Some(boundary);
```

Add this field to `ManifestProvenance` next to `build_attestation`:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub foreign_conversion_boundary: Option<crate::ccs::attestation::ForeignConversionBoundary>,
```

- [ ] **Step 5: Route foreign package inputs through `conary cook`**

In `apps/conary/src/commands/cook.rs`, detect file extensions before recipe resolution:

```rust
fn foreign_package_format(path: &Path) -> Option<&'static str> {
    let name = path.file_name()?.to_str()?;
    if name.ends_with(".rpm") {
        Some("rpm")
    } else if name.ends_with(".deb") {
        Some("deb")
    } else if name.ends_with(".pkg.tar.zst") {
        Some("pkg.tar.zst")
    } else {
        None
    }
}
```

Add a `cook_foreign_package` helper that calls the existing conversion stack and writes a `.ccs` file under the requested output directory. It must not create Remi database rows.

- [ ] **Step 6: Add PKGBUILD and scriptlet risk report attachment**

When the foreign input contains PKGBUILD bodies or legacy scriptlets, run the shared command-risk classifier and include the report hashes in `ForeignConversionBoundary`.

Use:

```rust
let build_report_hash = crate::ccs::attestation::canonical_json_hash(&build_report)?;
let scriptlet_report_hash = crate::ccs::attestation::canonical_json_hash(&scriptlet_report)?;
```

- [ ] **Step 7: Run foreign conversion tests**

Run: `cargo test -p conary-core ccs::convert foreign_boundary -- --nocapture`

Expected: PASS.

- [ ] **Step 8: Run cook command tests**

Run: `cargo test -p conary cook_foreign_package foreign_package_format -- --nocapture`

Expected: PASS.

- [ ] **Step 9: Commit Task 9**

```bash
git add crates/conary-core/src/ccs/attestation.rs crates/conary-core/src/ccs/manifest_provenance.rs crates/conary-core/src/ccs/convert apps/conary/src/commands/cook.rs
git commit -m "feat(cook): emit foreign conversion boundaries"
```

## Task 10: Remi Trusted Signer Config

**Files:**
- Modify: `apps/remi/src/server/config.rs`
- Modify: `apps/remi/src/server/mod.rs`
- Test: `apps/remi/src/server/config.rs`

- [ ] **Step 1: Write failing config tests**

Add tests:

```rust
#[test]
fn release_publish_trusted_signers_parse_from_config() {
    let config = RemiConfig::parse_str(
        r#"
        [release_publish]
        trusted_build_attestation_signers = [
          { key_id = "publisher", public_key = "-----BEGIN PUBLIC KEY-----\nabc\n-----END PUBLIC KEY-----" }
        ]
        "#
    )
    .unwrap();

    assert_eq!(config.release_publish.trusted_build_attestation_signers[0].key_id, "publisher");
}

#[test]
fn release_publish_empty_trusted_signers_fail_closed() {
    let config = RemiConfig::default();

    assert!(config.release_publish.trusted_build_attestation_signers.is_empty());
}
```

- [ ] **Step 2: Run config tests and confirm missing field failure**

Run: `cargo test -p remi release_publish_trusted_signers_parse_from_config release_publish_empty_trusted_signers_fail_closed -- --nocapture`

Expected: FAIL because `[release_publish]` is not parsed.

- [ ] **Step 3: Add config section**

In `apps/remi/src/server/config.rs`, add to `RemiConfig`:

```rust
/// Release artifact publication settings
#[serde(default)]
pub release_publish: ReleasePublishSection,
```

Add:

```rust
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ReleasePublishSection {
    #[serde(default)]
    pub trusted_build_attestation_signers: Vec<TrustedBuildAttestationSigner>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TrustedBuildAttestationSigner {
    pub key_id: String,
    pub public_key: String,
}
```

Wire the section into `ServerConfig` or `ServerState` so request handlers can read it.

- [ ] **Step 4: Run Remi config tests**

Run: `cargo test -p remi release_publish_ -- --nocapture`

Expected: PASS.

- [ ] **Step 5: Commit Task 10**

```bash
git add apps/remi/src/server/config.rs apps/remi/src/server/mod.rs
git commit -m "security(remi): configure trusted release signers"
```

## Task 11: Remi Release Upload Staging And Gate Parity

**Files:**
- Create: `apps/remi/src/server/release_publish.rs`
- Modify: `apps/remi/src/server/routes/admin.rs`
- Modify: `apps/remi/src/server/handlers/openapi.rs`
- Modify: `apps/remi/src/server/handlers/admin/mod.rs`
- Test: `apps/remi/src/server/release_publish.rs`
- Test: `apps/remi/src/server/routes/admin.rs`

- [ ] **Step 1: Write failing release upload tests**

Tests must prove negative staging behavior:

```rust
#[tokio::test]
async fn release_upload_with_unaccepted_signer_leaves_no_public_state() {
    let fixture = remi_release_fixture_with_unaccepted_signer().await;
    let response = fixture.upload_release().await;

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(!fixture.public_package_detail_exists("hello").await);
    assert!(!fixture.public_chunk_exists_for("hello").await);
    assert!(!fixture.tuf_target_exists("hello").await);
    assert!(!fixture.converted_package_row_exists("hello").await);
}

#[tokio::test]
async fn release_upload_empty_trusted_signers_fail_closed() {
    let fixture = remi_release_fixture_without_trusted_signers().await;
    let response = fixture.upload_release().await;

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(fixture.response_text(response).await.contains("no trusted release signers configured"));
}
```

- [ ] **Step 2: Run release upload tests and confirm missing route failure**

Run: `cargo test -p remi release_upload_ -- --nocapture`

Expected: FAIL because the release upload route and module do not exist.

- [ ] **Step 3: Add distinct release route**

In both admin routers, add:

```rust
.route(
    "/v1/admin/releases/{distro}",
    post(admin_handlers::upload_release_package),
)
```

Do not reuse `/v1/admin/packages/{distro}` for release publication.

- [ ] **Step 4: Implement release upload handler**

Add a thin handler in `handlers/admin/mod.rs` or a small admin submodule:

```rust
pub async fn upload_release_package(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(distro): Path<String>,
    request: Request,
) -> Response {
    crate::server::release_publish::handle_release_upload(state, distro, request).await
}
```

- [ ] **Step 5: Implement private staging and gate parity**

Create `apps/remi/src/server/release_publish.rs` with this flow:

```rust
pub async fn handle_release_upload(
    state: Arc<RwLock<ServerState>>,
    distro: String,
    request: axum::extract::Request,
) -> axum::response::Response {
    match release_upload_inner(state, distro, request).await {
        Ok(response) => response.into_response(),
        Err(error) => release_upload_error_response(error),
    }
}

async fn release_upload_inner(
    state: Arc<RwLock<ServerState>>,
    distro: String,
    request: axum::extract::Request,
) -> anyhow::Result<ReleaseUploadResponse> {
    let auth = authenticate_release_upload(&state, &request).await?;
    let staged = stage_release_body(&state, request).await?;
    let package = parse_staged_ccs(&staged.path)?;
    let accepted = accepted_release_signers(&state).await?;
    let lint = verify_remi_release_artifact(&package, &accepted)?;
    if !lint.is_passed() {
        staged.cleanup().await;
        anyhow::bail!("{}", format_publish_gate_failures(&lint));
    }
    let mut transaction = begin_release_transaction(&state, &distro, &auth).await?;
    let promoted = promote_staged_release_objects(&state, &staged, &package).await?;
    write_release_metadata(&mut transaction, &distro, &package, &promoted).await?;
    refresh_release_tuf_metadata(&mut transaction, &distro, &package).await?;
    transaction.commit().await?;
    staged.cleanup().await;
    Ok(ReleaseUploadResponse::created(package.manifest().package.name.clone()))
}
```

The helpers named in this skeleton must preserve the order shown here. If authentication fails, return `401` or `403`. If artifact authorization fails before promotion, remove the staging file and return `422`. If promotion fails after the DB transaction starts, roll back and remove staged public objects before returning `500`.

- [ ] **Step 6: Use shared publish gate**

Build accepted signers from Remi config:

```rust
let trusted: Vec<TrustedArtifactSigner> = state
    .read()
    .await
    .config
    .release_publish
    .trusted_build_attestation_signers
    .iter()
    .map(|signer| TrustedArtifactSigner {
        key_id: signer.key_id.clone(),
        public_key: signer.public_key.clone(),
    })
    .collect();
let accepted = AcceptedStaticSignerSet::from_trusted_artifact_signers(&trusted)?;
let report = verify_static_artifact_publish_eligibility(&package, &accepted, "m2-static-publish-policy-v1")?;
```

Add this Remi-neutral DTO to `publish_gate.rs` so `conary-core` does not depend on Remi crates:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedArtifactSigner {
    pub key_id: String,
    pub public_key: String,
}

impl AcceptedStaticSignerSet {
    pub fn from_trusted_artifact_signers(signers: &[TrustedArtifactSigner]) -> Result<Self> {
        if signers.is_empty() {
            bail!("no trusted release signers configured");
        }
        Ok(Self {
            active_keys: signers
                .iter()
                .map(|signer| (signer.key_id.clone(), signer.public_key.clone()))
                .collect(),
        })
    }
}
```

Remi converts `ReleasePublishSection.trusted_build_attestation_signers` into `TrustedArtifactSigner` values before calling this helper.

- [ ] **Step 7: Run release upload tests**

Run: `cargo test -p remi release_upload_ -- --nocapture`

Expected: PASS.

- [ ] **Step 8: Commit Task 11**

```bash
git add apps/remi/src/server/release_publish.rs apps/remi/src/server/routes/admin.rs apps/remi/src/server/handlers/admin apps/remi/src/server/handlers/openapi.rs crates/conary-core/src/repository/static_repo/publish_gate.rs
git commit -m "security(remi): gate release uploads before visibility"
```

## Task 12: Remi Timestamp Refresh And CLI Remi Target Routing

**Files:**
- Create: `apps/conary/src/commands/remi_publish.rs`
- Modify: `apps/conary/src/commands/mod.rs`
- Modify: `apps/conary/src/commands/publish.rs`
- Modify: `apps/remi/src/server/handlers/tuf.rs`
- Test: `apps/conary/src/commands/publish.rs`
- Test: `apps/remi/src/server/handlers/tuf.rs`

- [ ] **Step 1: Write failing Remi routing and timestamp tests**

Add tests:

```rust
#[tokio::test]
async fn http_publish_target_routes_to_remi_release_path() {
    let route = classify_publish_target_for_tests("https://remi.example.invalid/v1/admin/releases/test").unwrap();

    assert_eq!(route, PublishTargetRoute::RemiRelease);
}

#[tokio::test]
async fn static_local_guard_still_rejects_http_static_path() {
    let destination = RepoLocation::parse("https://repo.example.invalid/static").unwrap();
    let error = ensure_static_local_publish_destination(&destination).unwrap_err();

    assert!(error.to_string().contains("Remi HTTP(S) targets use the Remi release path"));
}

#[tokio::test]
async fn remi_tuf_refresh_timestamp_no_longer_returns_501() {
    let response = call_refresh_timestamp_for_tests().await;

    assert_ne!(response.status(), StatusCode::NOT_IMPLEMENTED);
}
```

- [ ] **Step 2: Run tests and confirm current failures**

Run: `cargo test -p conary http_publish_target_routes_to_remi_release_path static_local_guard_still_rejects_http_static_path -- --nocapture`

Expected: FAIL for missing route classifier.

Run: `cargo test -p remi remi_tuf_refresh_timestamp_no_longer_returns_501 -- --nocapture`

Expected: FAIL because timestamp refresh currently returns `501`.

- [ ] **Step 3: Add Remi publish client**

Create `apps/conary/src/commands/remi_publish.rs`:

```rust
// apps/conary/src/commands/remi_publish.rs
//! Client-side Remi release publish transport.

use std::path::Path;

use anyhow::{Context, Result, bail};

pub struct RemiPublishOptions<'a> {
    pub artifact_path: &'a Path,
    pub target_url: &'a str,
    pub bearer_token: &'a str,
}

pub async fn publish_to_remi(options: RemiPublishOptions<'_>) -> Result<()> {
    let bytes = tokio::fs::read(options.artifact_path)
        .await
        .with_context(|| format!("read artifact {}", options.artifact_path.display()))?;
    let client = reqwest::Client::new();
    let response = client
        .post(options.target_url)
        .bearer_auth(options.bearer_token)
        .body(bytes)
        .send()
        .await
        .context("send Remi release upload")?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        bail!("Remi release upload failed with {status}: {body}");
    }
    Ok(())
}
```

Wire it through `apps/conary/src/commands/mod.rs`.

- [ ] **Step 4: Route HTTP(S) publish targets to Remi**

In `apps/conary/src/commands/publish.rs`, add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PublishTargetRoute {
    StaticLocal,
    RemiRelease,
}

fn classify_publish_target(target: &str) -> Result<PublishTargetRoute> {
    if target.starts_with("http://") || target.starts_with("https://") {
        if target.contains("/v1/admin/releases/") {
            return Ok(PublishTargetRoute::RemiRelease);
        }
        bail!("HTTP(S) publish targets must use the Remi release endpoint /v1/admin/releases/{distro}");
    }
    Ok(PublishTargetRoute::StaticLocal)
}
```

Artifact-form publish should call `publish_to_remi` for `RemiRelease` after client-side artifact preflight. The Remi server remains the trust boundary.

- [ ] **Step 5: Implement timestamp refresh**

In `apps/remi/src/server/handlers/tuf.rs`, replace the `501` stub with a call that regenerates timestamp metadata from current snapshot metadata and writes it atomically. Return `200` with timestamp version on success.

Response shape:

```json
{
  "status": "ok",
  "role": "timestamp",
  "version": 1
}
```

- [ ] **Step 6: Run routing and timestamp tests**

Run: `cargo test -p conary remi_release static_local_guard -- --nocapture`

Expected: PASS.

Run: `cargo test -p remi tuf:: -- --nocapture`

Expected: PASS.

- [ ] **Step 7: Commit Task 12**

```bash
git add apps/conary/src/commands/remi_publish.rs apps/conary/src/commands/mod.rs apps/conary/src/commands/publish.rs apps/remi/src/server/handlers/tuf.rs
git commit -m "feat(remi): publish attested artifacts to release endpoint"
```

## Task 13: End-To-End Regression Matrix

**Files:**
- Modify: `apps/conary/tests/packaging_m2a.rs`
- Modify: `crates/conary-core/tests/m2_publish_gates.rs`
- Modify: `apps/remi/src/server/release_publish.rs` tests
- Modify: `docs/llms/subsystem-map.md`

- [ ] **Step 1: Add static publish rejection matrix**

Add table-driven tests covering:

```rust
[
    ("host", None, "artifact is not hermetic"),
    ("sandboxed", None, "artifact is not hermetic"),
    ("hermetic", None, "artifact is missing a build attestation"),
    ("hermetic", Some("bad-signature"), "build attestation signature mismatch"),
    ("hermetic", Some("unaccepted-signer"), "build attestation signer is not accepted"),
    ("hermetic", Some("stale-policy"), "build attestation policy digest is not accepted"),
    ("hermetic", Some("recorded-draft"), "recorded-draft artifacts are not publishable"),
]
```

- [ ] **Step 2: Add accepted static signer rotation tests**

Prove:

- active key authorizes attestation and final package signature
- retired key verifies history but cannot authorize new artifact publish
- local key-dir mismatch with verified destination metadata fails
- tampered `keys/package-keys.json` fails before signer acceptance
- rotated active key works after verified metadata update

- [ ] **Step 3: Add AUR-style synthetic fixtures**

Use inert fixture commands:

```sh
npm install synthetic-atomic-lockfile
bun add synthetic-js-digest
python -c 'print("synthetic")'
bpftool prog show
```

Expected:

- runtime `--sandbox=auto` classifies Medium or higher
- hermetic build command report has shared reason codes
- PKGBUILD body report has shared reason codes
- foreign conversion scriptlet report has shared reason codes
- missing lock/vendor/prefetch evidence blocks publish

- [ ] **Step 4: Add Remi parity negative tests**

Prove release upload failures leave:

- no converted-package row
- no public chunk object
- no package detail response
- no package index entry
- no TUF target

- [ ] **Step 5: Run focused regression suites**

Run:

```bash
cargo test -p conary-core
cargo test -p conary
cargo test -p remi
```

Expected: PASS.

- [ ] **Step 6: Run integration inventory when manifests change**

Run:

```bash
cargo run -p conary-test -- list
```

Expected: exits 0 and lists suites without manifest parse failures.

- [ ] **Step 7: Commit Task 13**

```bash
git add apps/conary/tests crates/conary-core/tests apps/remi/src/server docs/llms docs/modules
git commit -m "test(packaging): cover m2 release gates"
```

## Task 14: Documentation, Review, And Workspace Gates

**Files:**
- Modify: `docs/llms/subsystem-map.md`
- Modify: `docs/modules/ccs.md`
- Modify: `docs/modules/remi.md`
- Create: review artifacts under `docs/superpowers/reviews/`

- [ ] **Step 1: Update assistant routing docs**

Update the routing docs with:

```md
- M2 release publish gates: start in `crates/conary-core/src/repository/static_repo/publish_gate.rs`, then inspect `crates/conary-core/src/ccs/attestation.rs`.
- Static publish commit/order: `crates/conary-core/src/repository/static_repo/publish.rs`.
- Remi release push: `apps/remi/src/server/release_publish.rs`.
```

- [ ] **Step 2: Run documentation gates**

Run:

```bash
scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
scripts/check-doc-truth.sh
```

Expected: PASS.

- [ ] **Step 3: Run formatting and lint gates**

Run:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS.

- [ ] **Step 4: Run local agentic review**

Run a local review of the implementation diff against this plan and the design spec. The review must check:

- attestation gate cannot be bypassed by calling core static publish directly
- output identity excludes signatures and attestation and does not use `dna_hash`
- Remi release failures leave no public state
- runtime/build/PKGBUILD/foreign risk reports share reason codes
- `publish.rs` stayed within the line-count budget

- [ ] **Step 5: Run external plan/implementation review script if CLIs are configured**

Run:

```bash
scripts/agentic-plan-review.sh docs/superpowers/plans/2026-06-14-m2-release-surface-implementation.md
```

Expected: review artifacts are created under `docs/superpowers/reviews/` or the script reports unavailable providers without failing the implementation.

- [ ] **Step 6: Apply accepted review fixes**

Patch only review findings that are grounded in the design invariant:

```text
Artifact-form publish is allowed only for hermetic artifacts with verified, accepted build attestations.
```

For each accepted finding, add or update a focused regression test before the fix.

- [ ] **Step 7: Run final workspace gates**

Run:

```bash
cargo test -p conary-core
cargo test -p conary
cargo test -p remi
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

Expected: PASS.

- [ ] **Step 8: Commit final docs/review cleanup**

```bash
git add docs scripts apps crates
git commit -m "docs(packaging): finalize m2 release surface implementation"
```

## Final Acceptance

M2 is complete when all of these are true:

- `conary publish <target>` rebuilds hermetically, signs a build attestation, verifies it, signs the target-local package, and publishes.
- `conary publish <pkg.ccs> <target>` refuses every artifact without hermetic hardening, verified build attestation, accepted signer authority, matching output identity, clean publish lint, and valid target package signature.
- Static artifact-form publish to a brand-new repo requires explicit `--key-dir`; there is no one-off accepted-signer flag.
- Retired static keys do not authorize new artifact-form publish.
- `conary cook <foreign-pkg>` emits `foreign-converted` CCS artifacts with `ForeignConversionBoundary` evidence.
- Foreign-converted artifacts fail publish when boundary metadata is missing or mismatched.
- Remi release push requires bearer-token transport auth and server-side artifact authorization.
- Remi release upload failures leave no package row, public chunk, package detail/index result, or TUF target.
- Remi timestamp refresh no longer returns `501`.
- Shared command-risk reason codes cover runtime auto sandboxing, hermetic build commands, PKGBUILD bodies, and foreign conversion scriptlets.
- `crates/conary-core/src/repository/static_repo/publish.rs` line count is `<= 2699`.
- Focused tests, docs checks, formatting, clippy, and final review gates pass.
