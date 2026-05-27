// conary-core/src/ccs/archive_reader.rs

//! Shared CCS archive reader
//!
//! Extracts metadata, components, signature, and content blobs from a CCS
//! `.tar.gz` archive in a single pass.  Used by `package.rs`, `verify.rs`,
//! and `inspector.rs` to avoid duplicating the tar iteration logic.

use crate::ccs::binary_manifest::{BinaryManifest, Hash};
use crate::ccs::builder::ComponentData;
use crate::ccs::manifest::CcsManifest;
use crate::ccs::package::convert_binary_to_ccs_manifest;
use flate2::read::GzDecoder;
use std::collections::HashMap;
use std::io::Read;
use tar::Archive;
use tracing::warn;

/// Maximum size for a single archive entry (512 MB).
const MAX_ENTRY_SIZE: u64 = 512 * 1024 * 1024;

/// Maximum cumulative extraction size (4 GB).
const MAX_TOTAL_EXTRACTION_SIZE: u64 = 4 * 1024 * 1024 * 1024;

/// Maximum size for manifest entries — MANIFEST or MANIFEST.toml (16 MiB).
const MAX_MANIFEST_SIZE: u64 = 16 * 1024 * 1024;

/// Maximum size for component JSON entries (64 MiB).
const MAX_COMPONENT_SIZE: u64 = 64 * 1024 * 1024;

/// Everything that can be extracted from a CCS archive in a single pass.
#[derive(Debug)]
pub struct CcsArchiveContents {
    /// Parsed `CcsManifest` (converted from CBOR or parsed from TOML).
    pub manifest: CcsManifest,

    /// Raw manifest bytes (CBOR or TOML) — needed for signature verification.
    pub manifest_raw: Vec<u8>,

    /// The CBOR `BinaryManifest`, if the archive contained one.
    pub binary_manifest: Option<BinaryManifest>,

    /// Raw MANIFEST.toml bytes, if the archive contained one.
    /// Used by the verifier to check the TOML integrity hash.
    pub toml_raw: Option<Vec<u8>>,

    /// Parsed `MANIFEST.sig` JSON, if present.
    pub signature_raw: Option<String>,

    /// Component data keyed by component name.
    pub components: HashMap<String, ComponentData>,

    /// Content blobs keyed by their reconstructed SHA-256 hash
    /// (`objects/{prefix}/{suffix}` → `{prefix}{suffix}`).
    pub blobs: HashMap<String, Vec<u8>>,
}

/// Read and parse a CCS archive from any `Read` source.
///
/// The reader should supply the raw `.tar.gz` bytes (the function applies
/// gzip decompression internally).  All recognised entries are extracted in a
/// single pass, with per-entry and cumulative size guards.
///
/// # Errors
///
/// Returns an error if the archive is malformed, exceeds size limits, or
/// contains neither a CBOR `MANIFEST` nor a `MANIFEST.toml`.
pub fn read_ccs_archive<R: Read>(reader: R) -> anyhow::Result<CcsArchiveContents> {
    read_ccs_archive_with_limits(reader, MAX_TOTAL_EXTRACTION_SIZE)
}

fn read_ccs_archive_with_limits<R: Read>(
    reader: R,
    total_extraction_limit: u64,
) -> anyhow::Result<CcsArchiveContents> {
    let decoder = GzDecoder::new(reader).take(total_extraction_limit);
    let mut archive = Archive::new(decoder);

    let mut binary_manifest: Option<BinaryManifest> = None;
    let mut toml_manifest: Option<CcsManifest> = None;
    let mut toml_manifest_raw: Option<Vec<u8>> = None;
    let mut cbor_manifest_raw: Option<Vec<u8>> = None;
    let mut signature_raw: Option<String> = None;
    let mut components: HashMap<String, ComponentData> = HashMap::new();
    // Raw bytes of each component JSON, keyed by component name (for hash verification)
    let mut component_raw: HashMap<String, Vec<u8>> = HashMap::new();
    let mut blobs: HashMap<String, Vec<u8>> = HashMap::new();

    let mut total_bytes: u64 = 0;

    for entry in archive.entries()? {
        let mut entry = entry?;
        let entry_size = entry.header().size()?;

        // Per-entry size guard
        if entry_size > MAX_ENTRY_SIZE {
            anyhow::bail!("CCS archive entry exceeds maximum size limit: {entry_size} bytes");
        }
        total_bytes += entry_size;
        if total_bytes > total_extraction_limit {
            anyhow::bail!("CCS archive total extraction size exceeds limit");
        }

        let entry_path = entry.path()?;
        let entry_path_str = entry_path.to_string_lossy().to_string();

        // ── MANIFEST (CBOR binary manifest) ──────────────────────────
        if entry_path_str == "MANIFEST" || entry_path_str == "./MANIFEST" {
            if entry_size > MAX_MANIFEST_SIZE {
                anyhow::bail!(
                    "MANIFEST entry too large: {entry_size} bytes (limit {MAX_MANIFEST_SIZE})"
                );
            }
            let mut content = Vec::new();
            entry.read_to_end(&mut content)?;
            cbor_manifest_raw = Some(content.clone());
            if let Ok(bin) = BinaryManifest::from_cbor(&content) {
                binary_manifest = Some(bin);
            } else {
                warn!(
                    "Failed to parse CBOR MANIFEST entry; falling back to MANIFEST.toml if present"
                );
            }
        }
        // ── MANIFEST.toml ────────────────────────────────────────────
        else if entry_path_str == "MANIFEST.toml" || entry_path_str == "./MANIFEST.toml" {
            if entry_size > MAX_MANIFEST_SIZE {
                anyhow::bail!(
                    "MANIFEST.toml entry too large: {entry_size} bytes (limit {MAX_MANIFEST_SIZE})"
                );
            }
            let mut content = String::new();
            entry.read_to_string(&mut content)?;
            toml_manifest_raw = Some(content.as_bytes().to_vec());
            toml_manifest = Some(CcsManifest::parse(&content)?);
        }
        // ── MANIFEST.sig — optional signature ────────────────────────
        else if entry_path_str == "MANIFEST.sig" || entry_path_str == "./MANIFEST.sig" {
            let mut content = String::new();
            entry.read_to_string(&mut content)?;
            signature_raw = Some(content);
        }
        // ── components/*.json ────────────────────────────────────────
        else if (entry_path_str.starts_with("components/")
            || entry_path_str.starts_with("./components/"))
            && entry_path_str.ends_with(".json")
        {
            if entry_size > MAX_COMPONENT_SIZE {
                anyhow::bail!(
                    "Component JSON entry too large: {entry_size} bytes (limit {MAX_COMPONENT_SIZE})"
                );
            }
            let mut content_bytes = Vec::new();
            entry.read_to_end(&mut content_bytes)?;
            let content = String::from_utf8(content_bytes.clone())
                .map_err(|e| anyhow::anyhow!("Component JSON is not valid UTF-8: {e}"))?;
            let comp: ComponentData = serde_json::from_str(&content)
                .map_err(|e| anyhow::anyhow!("Invalid component JSON: {e}"))?;
            let comp_name = comp.name.clone();
            components.insert(comp_name.clone(), comp);
            component_raw.insert(comp_name, content_bytes);
        }
        // ── objects/{prefix}/{suffix} — content blobs ────────────────
        else if entry_path_str.starts_with("objects/") || entry_path_str.starts_with("./objects/")
        {
            let stripped = entry_path_str
                .strip_prefix("./")
                .unwrap_or(&entry_path_str)
                .strip_prefix("objects/")
                .unwrap_or("");

            if let Some((prefix, suffix)) = stripped.split_once('/') {
                if !prefix.chars().all(|c| c.is_ascii_hexdigit())
                    || !suffix.chars().all(|c| c.is_ascii_hexdigit())
                {
                    warn!("Skipping non-hex object path: {stripped}");
                    continue;
                }
                let content_hash = format!("{prefix}{suffix}");
                let mut content = Vec::new();
                entry.read_to_end(&mut content)?;
                blobs.insert(content_hash, content);
            }
        }
    }

    // ── Verify component JSON hashes against the signed binary manifest ──
    if let Some(ref bin) = binary_manifest {
        // Reject extra components not listed in the signed manifest
        for name in components.keys() {
            if !bin.components.contains_key(name) {
                anyhow::bail!(
                    "Component '{}' found in archive but not in signed manifest",
                    name
                );
            }
        }

        // Verify each component listed in the signed manifest
        for (name, comp_ref) in &bin.components {
            let raw = component_raw.get(name).ok_or_else(|| {
                anyhow::anyhow!(
                    "Component '{}' listed in signed manifest but missing from archive",
                    name
                )
            })?;

            let actual = Hash::sha256(raw);
            if actual != comp_ref.hash {
                anyhow::bail!(
                    "Component '{}' hash mismatch: expected {}, got {}",
                    name,
                    comp_ref.hash.value,
                    actual.value
                );
            }
        }
    }

    // Save a copy of the raw TOML bytes for integrity verification.
    // The resolution logic below consumes `toml_manifest_raw` in the
    // TOML-only path, so we must clone before that happens.
    let toml_raw_copy = toml_manifest_raw.clone();

    // ── Resolve manifest ─────────────────────────────────────────────
    // When both CBOR and TOML manifests are present, use TOML as primary
    // (it carries fields like config, redirects, policy, provenance that
    // CBOR omits) and verify consistency with the signed CBOR manifest
    // for the fields it does carry.
    let (manifest, manifest_raw) = if let Some(ref bin) = binary_manifest {
        if let Some(toml) = toml_manifest {
            // CBOR is signed and authoritative for all fields it carries.
            // Start from the CBOR-converted manifest, then merge in
            // TOML-only fields that CBOR does not represent.
            let mut merged = convert_binary_to_ccs_manifest(bin);

            // Overlay TOML-only fields (not carried in CBOR)
            merged.package.homepage = toml.package.homepage;
            merged.package.repository = toml.package.repository;
            merged.package.authors = toml.package.authors;
            merged.suggests = toml.suggests;
            merged.components = toml.components;
            merged.scriptlets = toml.scriptlets;
            merged.legacy_scriptlets = toml.legacy_scriptlets;
            merged.config = toml.config;
            merged.policy = toml.policy;
            merged.provenance = toml.provenance;
            merged.redirects = toml.redirects;
            merged.legacy = toml.legacy;
            // Overlay build sub-fields that CBOR doesn't carry
            if let Some(ref toml_build) = toml.build {
                if let Some(ref mut merged_build) = merged.build {
                    merged_build.environment = toml_build.environment.clone();
                    merged_build.commands = toml_build.commands.clone();
                } else {
                    merged.build = Some(toml_build.clone());
                }
            }

            let raw = cbor_manifest_raw.ok_or_else(|| {
                anyhow::anyhow!("CBOR binary manifest present but raw bytes missing")
            })?;
            (merged, raw)
        } else {
            // CBOR only -- convert to CcsManifest (TOML-only fields get defaults)
            let raw = cbor_manifest_raw.ok_or_else(|| {
                anyhow::anyhow!("CBOR binary manifest present but raw bytes missing")
            })?;
            (convert_binary_to_ccs_manifest(bin), raw)
        }
    } else if let Some(toml) = toml_manifest {
        let raw = toml_manifest_raw
            .ok_or_else(|| anyhow::anyhow!("TOML manifest present but raw bytes missing"))?;
        (toml, raw)
    } else {
        anyhow::bail!("CCS package missing both MANIFEST and MANIFEST.toml");
    };

    Ok(CcsArchiveContents {
        manifest,
        manifest_raw,
        binary_manifest,
        toml_raw: toml_raw_copy,
        signature_raw,
        components,
        blobs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::builder::{CcsBuilder, write_ccs_package};
    use crate::ccs::legacy_scriptlets::{
        DecisionCounts, EffectConfidence, EffectReplacement, EffectSource, ForeignReplayPolicy,
        LEGACY_SCRIPTLET_SCHEMA_V1, LegacyScriptletBundle, LegacyScriptletEntry, LifecyclePath,
        NativeInvocation, PublicationPolicy, PublicationStatus, RpmTriggerMetadata,
        RpmTriggerTargetConstraint, ScriptletDecision, ScriptletEffect, ScriptletFidelity,
        SourceFormat, TargetCompatibility, TransactionOrder, VersionScheme,
    };
    use crate::ccs::manifest::CcsManifest;
    use std::collections::BTreeMap;
    use std::fs;
    use tempfile::TempDir;

    fn build_test_package_with_manifest(manifest: CcsManifest) -> (TempDir, std::path::PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let source_dir = temp.path().join("src");
        fs::create_dir_all(source_dir.join("usr/bin")).unwrap();
        fs::write(source_dir.join("usr/bin/hello"), b"hello world\n").unwrap();

        let result = CcsBuilder::new(manifest, &source_dir).build().unwrap();
        let package_path = temp.path().join("test-reader.ccs");
        write_ccs_package(&result, &package_path).unwrap();
        (temp, package_path)
    }

    fn build_test_package() -> (TempDir, std::path::PathBuf) {
        build_test_package_with_manifest(
            CcsManifest::parse(
                r#"
[package]
name = "test-reader"
version = "1.0.0"
description = "archive reader test"
license = "MIT"
"#,
            )
            .unwrap(),
        )
    }

    fn legacy_bundle_fixture() -> LegacyScriptletBundle {
        let body = "ldconfig\n";
        LegacyScriptletBundle {
            schema: LEGACY_SCRIPTLET_SCHEMA_V1.to_string(),
            schema_revision: 1,
            source_format: SourceFormat::Rpm,
            source_family: "fedora-rhel".to_string(),
            source_distro: Some("fedora".to_string()),
            source_release: Some("44".to_string()),
            source_arch: Some("x86_64".to_string()),
            source_package: "nginx".to_string(),
            source_version: "1.28.0-1.fc44".to_string(),
            source_checksum: Some(
                "sha256:3333333333333333333333333333333333333333333333333333333333333333"
                    .to_string(),
            ),
            version_scheme: VersionScheme::Rpm,
            conversion_tool: "remi".to_string(),
            conversion_tool_version: "0.8.0".to_string(),
            conversion_policy: "safe-or-legacy".to_string(),
            adapter_registry_digest: None,
            target_policy_digest: None,
            evidence_digest: None,
            target_compatibility: TargetCompatibility::SourceNative,
            allowed_targets: vec!["rpm/fedora/44/x86_64".to_string()],
            foreign_replay_policy: ForeignReplayPolicy::Deny,
            publication_policy: PublicationPolicy::PublicIfNoBlocked,
            publication_status: PublicationStatus::PrivateReview,
            scriptlet_fidelity: ScriptletFidelity::LegacyReplay,
            decision_counts: DecisionCounts {
                replaced: 0,
                legacy: 1,
                blocked: 0,
                review: 0,
                extra: BTreeMap::new(),
            },
            unsupported_class_counts: BTreeMap::new(),
            entries: vec![LegacyScriptletEntry {
                id: "rpm:%post".to_string(),
                native_slot: "%post".to_string(),
                phase: LifecyclePath::PostInstall,
                lifecycle_paths: vec!["install:first".to_string()],
                interpreter: "/bin/sh".to_string(),
                interpreter_args: vec!["-e".to_string()],
                body_sha256: crate::hash::sha256_prefixed(body.as_bytes()),
                body: body.to_string(),
                body_encoding: None,
                native_invocation: NativeInvocation {
                    args: vec!["1".to_string()],
                    environment: vec!["RPM_INSTALL_PREFIX=/".to_string()],
                    stdin: Some("none".to_string()),
                    chroot: Some("install-root".to_string()),
                    extra: BTreeMap::new(),
                },
                transaction_order: TransactionOrder {
                    position: "after-payload".to_string(),
                    before: vec![],
                    after: vec!["payload".to_string()],
                    extra: BTreeMap::new(),
                },
                timeout_ms: 30_000,
                sandbox: None,
                capabilities: vec!["ldconfig".to_string()],
                decision: ScriptletDecision::Legacy,
                reason_code: "protected-replay-required".to_string(),
                human_reason: None,
                evidence_digest: None,
                source_evidence_refs: vec![],
                effects: vec![ScriptletEffect {
                    kind: "ldconfig".to_string(),
                    source: EffectSource::StaticSignal,
                    confidence: EffectConfidence::Declared,
                    replacement: EffectReplacement::Complete,
                    adapter_id: Some("ldconfig/v1".to_string()),
                    adapter_digest: None,
                    command: Some("ldconfig".to_string()),
                    args: vec![],
                    path: None,
                    reason_code: Some("ldconfig-cache-refresh".to_string()),
                    extra: BTreeMap::new(),
                }],
                unknown_commands: vec![],
                blocked_classes: vec![],
                rpm_trigger: Some(RpmTriggerMetadata {
                    kind: "file-trigger".to_string(),
                    condition: Some("in".to_string()),
                    target_constraints: vec![RpmTriggerTargetConstraint {
                        package: "systemd".to_string(),
                        operator: Some(">=".to_string()),
                        version: Some("255".to_string()),
                        extra: BTreeMap::new(),
                    }],
                    priority: Some(100),
                    file_globs: vec!["/usr/lib/systemd/system/*.service".to_string()],
                    stdin_contract: Some("paths".to_string()),
                    transaction_order: Some("post-transaction".to_string()),
                    extra: BTreeMap::new(),
                }),
                deb_maintainer: None,
                arch_install: None,
                residual_replay: None,
                extra: BTreeMap::new(),
            }],
            extra: BTreeMap::new(),
        }
    }

    fn build_test_package_with_legacy_bundle() -> (TempDir, std::path::PathBuf) {
        let mut manifest = CcsManifest::parse(
            r#"
[package]
name = "test-reader"
version = "1.0.0"
description = "archive reader test"
license = "MIT"
"#,
        )
        .unwrap();
        manifest.legacy_scriptlets = Some(legacy_bundle_fixture());

        build_test_package_with_manifest(manifest)
    }

    #[test]
    fn test_read_ccs_archive_extracts_manifest_and_components() {
        let (_temp, path) = build_test_package();
        let file = std::fs::File::open(&path).unwrap();
        let contents = read_ccs_archive(file).unwrap();

        assert_eq!(contents.manifest.package.name, "test-reader");
        assert_eq!(contents.manifest.package.version, "1.0.0");
        assert!(!contents.components.is_empty());
        assert!(!contents.blobs.is_empty());
        assert!(!contents.manifest_raw.is_empty());
    }

    #[test]
    fn archive_reader_preserves_legacy_scriptlet_bundle_from_toml_overlay() {
        let (_temp, path) = build_test_package_with_legacy_bundle();
        let file = std::fs::File::open(&path).unwrap();

        let contents = read_ccs_archive(file).unwrap();
        let bundle = contents
            .manifest
            .legacy_scriptlets
            .as_ref()
            .expect("legacy scriptlet bundle from TOML overlay");

        assert_eq!(bundle.source_package, "nginx");
        assert_eq!(
            bundle.entries[0]
                .rpm_trigger
                .as_ref()
                .expect("rpm trigger metadata")
                .file_globs,
            vec!["/usr/lib/systemd/system/*.service"]
        );
    }

    #[test]
    fn builder_package_writer_preserves_legacy_scriptlet_bundle() {
        let (_temp, path) = build_test_package_with_legacy_bundle();
        let file = std::fs::File::open(&path).unwrap();

        let contents = read_ccs_archive(file).unwrap();
        let toml_raw = String::from_utf8(contents.toml_raw.expect("MANIFEST.toml raw")).unwrap();

        assert!(toml_raw.contains("[legacy_scriptlets]"));
        assert!(contents.manifest.legacy_scriptlets.is_some());
    }

    #[test]
    fn test_read_ccs_archive_missing_manifest() {
        // Build a tar.gz with no MANIFEST
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use tar::Builder;

        let mut buf = Vec::new();
        {
            let encoder = GzEncoder::new(&mut buf, Compression::default());
            let mut builder = Builder::new(encoder);
            // Add a dummy file so the archive isn't empty
            let data = b"hello";
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, "dummy.txt", &data[..])
                .unwrap();
            let encoder = builder.into_inner().unwrap();
            encoder.finish().unwrap();
        }

        let cursor = std::io::Cursor::new(buf);
        let err = read_ccs_archive(cursor).unwrap_err();
        assert!(
            err.to_string().contains("missing both MANIFEST"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_read_ccs_archive_respects_total_extraction_limit() {
        let (_temp, path) = build_test_package();
        let file = std::fs::File::open(&path).unwrap();
        let err = read_ccs_archive_with_limits(file, 32).unwrap_err();
        assert!(
            err.to_string().contains("missing both MANIFEST")
                || err.to_string().contains("failed to iterate over archive")
                || err.to_string().contains("failed to read entire block")
                || err.to_string().contains("unexpected end of file"),
            "unexpected error: {err}"
        );
    }
}
