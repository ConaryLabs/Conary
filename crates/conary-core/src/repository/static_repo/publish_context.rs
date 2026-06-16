// conary-core/src/repository/static_repo/publish_context.rs

use std::collections::BTreeSet;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::ccs::manifest_provenance::ManifestProvenance;
use crate::ccs::signing::SigningKeyPair;
use crate::hash;
use crate::packages::traits::PackageFormat;
use crate::recipe::hermetic::PolicyStatus;
use crate::repository::static_repo::publish_gate::AcceptedStaticSignerSet;
use crate::repository::static_repo::{
    PackageKeyEntry, PackageKeyStatus, PackageKeysFile, RepoLocation, validate_repo_relative_path,
};
use crate::trust::keys::signing_keypair_to_tuf_key;
use crate::trust::metadata::{
    Role, RootMetadata, Signed, SnapshotMetadata, TargetDescription, TargetsMetadata,
    TimestampMetadata,
};
use crate::trust::verify::{
    extract_role_keys, verify_metadata_hash, verify_signatures, verify_static_snapshot_consistency,
};

pub const STATIC_PUBLISH_POLICY_DIGEST_V1: &str = "m2-static-publish-policy-v1";

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactGateContext {
    pub accepted_signers: AcceptedStaticSignerSet,
    pub publish_policy_digest: String,
}

impl PreparedStaticPublishContext {
    pub fn artifact_gate_context(&self) -> ArtifactGateContext {
        ArtifactGateContext {
            accepted_signers: self.accepted_signers.clone(),
            publish_policy_digest: self.publish_policy_digest.clone(),
        }
    }
}

impl StaticPublishPrepareOptions {
    pub fn prepare(self) -> Result<PreparedStaticPublishContext> {
        ensure_static_local_publish_destination(&self.destination)?;
        let key_dir = match self.key_dir {
            Some(key_dir) => key_dir,
            None if self.publish_form == StaticPublishForm::Artifact && self.force_reinit => {
                bail!("artifact-form publish to a new static repo requires --key-dir");
            }
            None => bail!("static publish requires --key-dir"),
        };

        create_private_dir_all(&key_dir)
            .with_context(|| format!("create static repo key directory {}", key_dir.display()))?;
        let verified_package_keys =
            if self.publish_form == StaticPublishForm::Artifact && !self.force_reinit {
                load_verified_package_keys_for_destination(&self.destination, self.force_reinit)?
            } else {
                None
            };
        let active_publish_key = match self.publish_form {
            StaticPublishForm::Project => ensure_key_pair(&key_dir, "publish")?,
            StaticPublishForm::Artifact if self.force_reinit => {
                ensure_key_pair(&key_dir, "publish")?
            }
            StaticPublishForm::Artifact if verified_package_keys.is_none() => {
                ensure_key_pair(&key_dir, "publish")?
            }
            StaticPublishForm::Artifact => load_key_pair(&key_dir, "publish")?,
        };
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
                    AcceptedStaticSignerSet::from_initial_key(
                        active_publish_key_id.clone(),
                        public_key,
                    )
                } else if let Some(package_keys) = verified_package_keys {
                    AcceptedStaticSignerSet::from_verified_package_keys(&package_keys)?
                } else {
                    AcceptedStaticSignerSet::from_initial_key(
                        active_publish_key_id.clone(),
                        public_key,
                    )
                }
            }
        };

        Ok(PreparedStaticPublishContext {
            destination: self.destination,
            key_dir,
            active_publish_key,
            active_publish_key_id,
            accepted_signers,
            publish_policy_digest: STATIC_PUBLISH_POLICY_DIGEST_V1.to_string(),
        })
    }
}

pub fn prepare_project_form_static_context(
    destination: &RepoLocation,
    key_dir: &Path,
    force_reinit: bool,
) -> Result<PreparedStaticPublishContext> {
    StaticPublishPrepareOptions {
        destination: destination.clone(),
        key_dir: Some(key_dir.to_path_buf()),
        publish_form: StaticPublishForm::Project,
        force_reinit,
    }
    .prepare()
}

pub fn prepare_artifact_form_static_context(
    destination: &RepoLocation,
    key_dir: &Path,
    force_reinit: bool,
) -> Result<PreparedStaticPublishContext> {
    StaticPublishPrepareOptions {
        destination: destination.clone(),
        key_dir: Some(key_dir.to_path_buf()),
        publish_form: StaticPublishForm::Artifact,
        force_reinit,
    }
    .prepare()
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StaticDestinationMetadataVersions {
    pub root_version: u64,
    pub targets_version: u64,
    pub snapshot_version: u64,
    pub timestamp_version: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StaticArtifactDestinationSnapshot {
    pub initial: bool,
    pub root_key_fingerprint: Option<String>,
    pub package_keys_sha256: Option<String>,
    pub accepted_signer_set_hash: Option<String>,
    pub publish_policy_digest: String,
    pub metadata_versions: Option<StaticDestinationMetadataVersions>,
}

pub fn inspect_artifact_form_static_destination(
    destination: &RepoLocation,
) -> Result<StaticArtifactDestinationSnapshot> {
    ensure_static_local_publish_destination(destination)?;
    let RepoLocation::File { root } = destination else {
        bail!("static publish destination inspection only supports file repositories");
    };
    let destination = read_destination_state(root, false)?;
    if destination.initial {
        return Ok(StaticArtifactDestinationSnapshot {
            initial: true,
            root_key_fingerprint: None,
            package_keys_sha256: destination
                .package_keys_bytes
                .as_deref()
                .map(crate::hash::sha256_prefixed),
            accepted_signer_set_hash: None,
            publish_policy_digest: STATIC_PUBLISH_POLICY_DIGEST_V1.to_string(),
            metadata_versions: None,
        });
    }

    let root = destination
        .root
        .as_ref()
        .context("verified destination snapshot missing root metadata")?;
    let root_key_fingerprint = root_role_keyids_fingerprint(root)?;
    let package_keys_sha256 = destination
        .package_keys_bytes
        .as_deref()
        .map(crate::hash::sha256_prefixed);
    let accepted_signer_set_hash = match destination.package_keys_bytes.as_deref() {
        Some(bytes) => {
            let text = std::str::from_utf8(bytes)?;
            let keys = PackageKeysFile::parse(text)?;
            Some(AcceptedStaticSignerSet::from_verified_package_keys(&keys)?.canonical_hash()?)
        }
        None => None,
    };

    Ok(StaticArtifactDestinationSnapshot {
        initial: false,
        root_key_fingerprint: Some(root_key_fingerprint),
        package_keys_sha256,
        accepted_signer_set_hash,
        publish_policy_digest: STATIC_PUBLISH_POLICY_DIGEST_V1.to_string(),
        metadata_versions: Some(StaticDestinationMetadataVersions {
            root_version: root.signed.version,
            targets_version: destination
                .targets
                .as_ref()
                .context("verified destination snapshot missing targets metadata")?
                .signed
                .version,
            snapshot_version: destination
                .snapshot
                .as_ref()
                .context("verified destination snapshot missing snapshot metadata")?
                .signed
                .version,
            timestamp_version: destination
                .timestamp
                .as_ref()
                .context("verified destination snapshot missing timestamp metadata")?
                .signed
                .version,
        }),
    })
}

pub struct ProjectFormAttestationInput<'a> {
    pub package_path: &'a Path,
    pub provenance: &'a ManifestProvenance,
    pub context: &'a PreparedStaticPublishContext,
    pub conary_version: &'a str,
}

pub fn attach_project_form_attestation(input: ProjectFormAttestationInput<'_>) -> Result<PathBuf> {
    let archive = crate::ccs::archive_reader::read_ccs_archive(
        fs::File::open(input.package_path)
            .with_context(|| format!("open {}", input.package_path.display()))?,
    )?;
    if archive.signature_raw.is_some() {
        bail!("project-form publish expected an unsigned cook output before attestation signing");
    }
    let package =
        crate::ccs::CcsPackage::parse(input.package_path.to_str().with_context(|| {
            format!(
                "package path is not valid UTF-8: {}",
                input.package_path.display()
            )
        })?)
        .map_err(anyhow::Error::from)?;
    let output_identity = crate::ccs::attestation::compute_build_output_identity(&package)?;
    let payload = build_project_form_attestation_payload(
        input.provenance,
        output_identity,
        input.context.publish_policy_digest.as_str(),
        input.conary_version,
    )?;
    preflight_project_form_attestation_payload(&payload, input.provenance)?;
    let envelope = crate::ccs::attestation::sign_build_attestation(
        payload,
        &input.context.active_publish_key,
    )?;
    let signed_temp =
        tempfile::Builder::new()
            .prefix("conary-attested-")
            .suffix(".ccs")
            .tempfile_in(input.package_path.parent().with_context(|| {
                format!("resolve parent for {}", input.package_path.display())
            })?)?;
    let build_result = build_result_from_package_with_attestation(&package, envelope)?;
    crate::ccs::builder::write_signed_ccs_package(
        &build_result,
        signed_temp.path(),
        &input.context.active_publish_key,
    )?;
    let trusted_key = input.context.active_publish_key.public_key_base64();
    let verification = crate::ccs::verify::verify_package(
        signed_temp.path(),
        &crate::ccs::verify::TrustPolicy::strict(vec![trusted_key]),
    )?;
    if !verification.valid || !verification.toml_integrity_valid {
        bail!("attested project-form package failed final CCS verification");
    }
    let report =
        crate::repository::static_repo::publish_gate::verify_static_artifact_publish_eligibility(
            signed_temp.path(),
            &input.context.accepted_signers,
            &input.context.publish_policy_digest,
        )?;
    if !report.is_passed() {
        bail!(
            "{}",
            crate::repository::static_repo::publish_gate::format_publish_gate_failures(&report)
        );
    }
    let persisted = signed_temp
        .keep()
        .map_err(|error| anyhow::anyhow!("persist attested package: {}", error.error))?
        .1;
    Ok(persisted)
}

fn build_project_form_attestation_payload(
    provenance: &ManifestProvenance,
    output_identity: crate::ccs::attestation::BuildOutputIdentity,
    publish_policy_digest: &str,
    conary_version: &str,
) -> Result<crate::ccs::attestation::BuildAttestationPayload> {
    let evidence = provenance
        .hermetic_evidence
        .as_ref()
        .context("project-form publish requires hermetic evidence")?;
    Ok(crate::ccs::attestation::BuildAttestationPayload {
        schema_version: crate::ccs::attestation::BUILD_ATTESTATION_SCHEMA_V1,
        origin_class: output_identity.origin_class.clone(),
        hardening_level: output_identity.hardening_level.clone(),
        build_input: evidence.build_input.clone(),
        dependency_lock: evidence.dependency_lock.clone(),
        hermetic_evidence_hash: crate::ccs::attestation::canonical_json_hash(evidence)?,
        output_identity,
        build_command_risk_report_hash: crate::ccs::attestation::canonical_json_hash(
            &evidence.command_risk,
        )?,
        scriptlet_risk_report_hash: None,
        conversion_boundary_hash: None,
        publish_policy_digest: publish_policy_digest.to_string(),
        command_risk_classifier_version: evidence.command_risk.classifier_version.clone(),
        sandbox_profile: "kitchen-pristine-network-none".to_string(),
        seccomp_profile: Some("scriptlet-v1".to_string()),
        builder_identity: "conary-hermetic-kitchen".to_string(),
        conary_version: conary_version.to_string(),
        issued_at: chrono::Utc::now().to_rfc3339(),
    })
}

fn preflight_project_form_attestation_payload(
    payload: &crate::ccs::attestation::BuildAttestationPayload,
    provenance: &ManifestProvenance,
) -> Result<()> {
    if payload.hardening_level != "hermetic" {
        bail!("project-form publish can only sign hermetic build attestations");
    }
    if payload.origin_class == "recorded-draft" {
        bail!("project-form publish cannot sign recorded-draft artifacts");
    }
    if payload.publish_policy_digest != STATIC_PUBLISH_POLICY_DIGEST_V1 {
        bail!("project-form publish attestation uses an unknown policy digest");
    }
    if payload
        .output_identity
        .canonical_content_identity
        .trim()
        .is_empty()
    {
        bail!("project-form publish attestation is missing output content identity");
    }
    let evidence = provenance
        .hermetic_evidence
        .as_ref()
        .context("project-form publish requires hermetic evidence before attestation signing")?;
    let command_risk_hash = crate::ccs::attestation::canonical_json_hash(&evidence.command_risk)?;
    if payload.build_command_risk_report_hash != command_risk_hash {
        bail!("project-form publish command-risk report hash does not match hermetic evidence");
    }
    if payload.command_risk_classifier_version != evidence.command_risk.classifier_version {
        bail!("project-form publish command-risk classifier version mismatch");
    }
    if evidence.command_risk.status != PolicyStatus::Clean {
        bail!("project-form publish refuses unclean hermetic command-risk reports");
    }
    if evidence.ecosystem_policy.status != PolicyStatus::Clean {
        bail!("project-form publish refuses unclean ecosystem offline policy reports");
    }
    Ok(())
}

fn build_result_from_package_with_attestation(
    package: &crate::ccs::CcsPackage,
    envelope: crate::ccs::attestation::BuildAttestationEnvelope,
) -> Result<crate::ccs::BuildResult> {
    let mut manifest = package.manifest().clone();
    manifest
        .provenance
        .get_or_insert_with(Default::default)
        .build_attestation = Some(envelope);
    Ok(crate::ccs::BuildResult {
        manifest,
        components: package.components().clone(),
        files: package.file_entries().to_vec(),
        blobs: package.extract_all_content().map_err(anyhow::Error::from)?,
        total_size: package.file_entries().iter().map(|entry| entry.size).sum(),
        chunked: package
            .file_entries()
            .iter()
            .any(|entry| entry.chunks.is_some()),
        chunk_stats: None,
    })
}

pub(crate) fn ensure_static_local_publish_destination(destination: &RepoLocation) -> Result<()> {
    if matches!(destination, RepoLocation::Http { .. }) {
        bail!(
            "static publisher supports local filesystem destinations; Remi HTTP(S) targets use the Remi release path"
        );
    }
    Ok(())
}

fn load_key_pair(key_dir: &Path, role: &str) -> Result<SigningKeyPair> {
    let private_path = key_dir.join(format!("{role}.private"));
    SigningKeyPair::load_from_file(&private_path)
        .map_err(anyhow::Error::from)
        .with_context(|| format!("load {role} key {}", private_path.display()))
}

fn load_verified_package_keys_for_destination(
    destination: &RepoLocation,
    force_reinit: bool,
) -> Result<Option<PackageKeysFile>> {
    let RepoLocation::File { root } = destination else {
        bail!("static publish context can only load package keys from file destinations");
    };
    let destination = read_destination_state(root, force_reinit)?;
    if destination.initial {
        return Ok(None);
    }
    let Some(package_keys_bytes) = destination.package_keys_bytes else {
        return Ok(None);
    };
    let text = std::str::from_utf8(&package_keys_bytes)?;
    PackageKeysFile::parse(text)
        .map(Some)
        .context("parse verified package keys for static publish")
}

#[derive(Default)]
pub(crate) struct DestinationState {
    pub(crate) initial: bool,
    pub(crate) root: Option<Signed<RootMetadata>>,
    pub(crate) targets: Option<Signed<TargetsMetadata>>,
    pub(crate) snapshot: Option<Signed<SnapshotMetadata>>,
    pub(crate) timestamp: Option<Signed<TimestampMetadata>>,
    pub(crate) root_bytes: Option<Vec<u8>>,
    pub(crate) targets_bytes: Option<Vec<u8>>,
    pub(crate) snapshot_bytes: Option<Vec<u8>>,
    pub(crate) timestamp_bytes: Option<Vec<u8>>,
    pub(crate) identity_bytes: Option<Vec<u8>>,
    pub(crate) index_bytes: Option<Vec<u8>>,
    pub(crate) package_keys_bytes: Option<Vec<u8>>,
}

pub(crate) fn read_destination_state(
    repo_root: &Path,
    force_reinit: bool,
) -> Result<DestinationState> {
    let root_bytes = read_optional(repo_root, "metadata/root.json")?;
    let targets_bytes = read_optional(repo_root, "metadata/targets.json")?;
    let snapshot_bytes = read_optional(repo_root, "metadata/snapshot.json")?;
    let timestamp_bytes = read_optional(repo_root, "metadata/timestamp.json")?;

    let all_absent = root_bytes.is_none()
        && targets_bytes.is_none()
        && snapshot_bytes.is_none()
        && timestamp_bytes.is_none();
    if all_absent || force_reinit {
        return Ok(DestinationState {
            initial: true,
            root_bytes,
            targets_bytes,
            snapshot_bytes,
            timestamp_bytes,
            identity_bytes: read_optional(repo_root, "conary-repo.toml")?,
            index_bytes: read_optional(repo_root, "index.json")?,
            package_keys_bytes: read_optional(repo_root, "keys/package-keys.json")?,
            ..DestinationState::default()
        });
    }

    if root_bytes.is_none()
        || targets_bytes.is_none()
        || snapshot_bytes.is_none()
        || timestamp_bytes.is_none()
    {
        bail!(
            "static repo destination is damaged or partially initialized; rerun with force_reinit to start a fresh identity"
        );
    }

    let root: Signed<RootMetadata> = serde_json::from_slice(root_bytes.as_ref().expect("checked"))
        .context("parse destination metadata/root.json")?;
    let targets: Signed<TargetsMetadata> =
        serde_json::from_slice(targets_bytes.as_ref().expect("checked"))
            .context("parse destination metadata/targets.json")?;
    let snapshot: Signed<SnapshotMetadata> =
        serde_json::from_slice(snapshot_bytes.as_ref().expect("checked"))
            .context("parse destination metadata/snapshot.json")?;
    let timestamp: Signed<TimestampMetadata> =
        serde_json::from_slice(timestamp_bytes.as_ref().expect("checked"))
            .context("parse destination metadata/timestamp.json")?;
    verify_destination_metadata(
        &root,
        &targets,
        &snapshot,
        &timestamp,
        targets_bytes.as_ref().expect("checked"),
        snapshot_bytes.as_ref().expect("checked"),
    )?;
    let identity_bytes = read_optional(repo_root, "conary-repo.toml")?;
    let index_bytes = read_optional(repo_root, "index.json")?;
    let package_keys_bytes = read_optional(repo_root, "keys/package-keys.json")?;
    verify_destination_target_payloads(
        repo_root,
        &targets.signed,
        index_bytes.as_deref(),
        package_keys_bytes.as_deref(),
    )?;

    Ok(DestinationState {
        initial: false,
        root: Some(root),
        targets: Some(targets),
        snapshot: Some(snapshot),
        timestamp: Some(timestamp),
        root_bytes,
        targets_bytes,
        snapshot_bytes,
        timestamp_bytes,
        identity_bytes,
        index_bytes,
        package_keys_bytes,
    })
}

fn verify_destination_metadata(
    root: &Signed<RootMetadata>,
    targets: &Signed<TargetsMetadata>,
    snapshot: &Signed<SnapshotMetadata>,
    timestamp: &Signed<TimestampMetadata>,
    targets_bytes: &[u8],
    snapshot_bytes: &[u8],
) -> Result<()> {
    let (root_keys, root_threshold) =
        extract_role_keys(&root.signed, Role::Root).map_err(anyhow::Error::from)?;
    verify_signatures(root, Role::Root, &root_keys, root_threshold).map_err(anyhow::Error::from)?;

    let (targets_keys, targets_threshold) =
        extract_role_keys(&root.signed, Role::Targets).map_err(anyhow::Error::from)?;
    verify_signatures(targets, Role::Targets, &targets_keys, targets_threshold)
        .map_err(anyhow::Error::from)?;

    let (snapshot_keys, snapshot_threshold) =
        extract_role_keys(&root.signed, Role::Snapshot).map_err(anyhow::Error::from)?;
    verify_signatures(snapshot, Role::Snapshot, &snapshot_keys, snapshot_threshold)
        .map_err(anyhow::Error::from)?;

    let (timestamp_keys, timestamp_threshold) =
        extract_role_keys(&root.signed, Role::Timestamp).map_err(anyhow::Error::from)?;
    verify_signatures(
        timestamp,
        Role::Timestamp,
        &timestamp_keys,
        timestamp_threshold,
    )
    .map_err(anyhow::Error::from)?;

    verify_static_snapshot_consistency(
        &snapshot.signed,
        root.signed.version,
        targets.signed.version,
    )
    .map_err(anyhow::Error::from)?;
    let targets_ref = snapshot
        .signed
        .meta
        .get("targets.json")
        .context("snapshot metadata missing targets.json")?;
    verify_metadata_hash(targets_ref, targets_bytes, true).map_err(anyhow::Error::from)?;
    verify_timestamp_pins_current_snapshot(timestamp, snapshot, snapshot_bytes)?;

    Ok(())
}

fn verify_timestamp_pins_current_snapshot(
    timestamp: &Signed<TimestampMetadata>,
    snapshot: &Signed<SnapshotMetadata>,
    snapshot_bytes: &[u8],
) -> Result<()> {
    let snapshot_ref = timestamp
        .signed
        .meta
        .get("snapshot.json")
        .context("timestamp metadata missing snapshot.json")?;
    if snapshot_ref.version != snapshot.signed.version {
        bail!(
            "timestamp pins snapshot.json v{} but current snapshot is v{}",
            snapshot_ref.version,
            snapshot.signed.version
        );
    }
    let length = snapshot_ref
        .length
        .context("timestamp snapshot.json reference missing length")?;
    if length != snapshot_bytes.len() as u64 {
        bail!(
            "timestamp pins snapshot.json length {} but current snapshot length is {}",
            length,
            snapshot_bytes.len()
        );
    }
    verify_metadata_hash(snapshot_ref, snapshot_bytes, true).map_err(anyhow::Error::from)?;

    Ok(())
}

fn verify_destination_target_payloads(
    repo_root: &Path,
    targets: &TargetsMetadata,
    index_bytes: Option<&[u8]>,
    package_keys_bytes: Option<&[u8]>,
) -> Result<()> {
    for (relative, target) in &targets.targets {
        validate_repo_relative_path(relative)
            .with_context(|| format!("destination target path {relative} is invalid"))?;
        match relative.as_str() {
            "index.json" => {
                let bytes = index_bytes.context("destination target index.json is missing")?;
                verify_target_payload(relative, target, bytes)?;
            }
            "keys/package-keys.json" => {
                let bytes = package_keys_bytes
                    .context("destination target keys/package-keys.json is missing")?;
                verify_target_payload(relative, target, bytes)?;
            }
            _ => {
                let bytes = fs::read(repo_root.join(relative))
                    .with_context(|| format!("read destination target {relative}"))?;
                verify_target_payload(relative, target, &bytes)?;
            }
        }
    }

    Ok(())
}

fn verify_target_payload(
    relative: &str,
    target: &TargetDescription,
    actual_bytes: &[u8],
) -> Result<()> {
    if target.length != actual_bytes.len() as u64 {
        bail!(
            "destination target {relative} length mismatch: targets pins {}, actual {}",
            target.length,
            actual_bytes.len()
        );
    }
    let expected_sha256 = target
        .hashes
        .get("sha256")
        .with_context(|| format!("destination target {relative} missing sha256 hash"))?;
    let actual_sha256 = hash::sha256(actual_bytes);
    if expected_sha256 != &actual_sha256 {
        bail!(
            "destination target {relative} sha256 mismatch: expected {expected_sha256}, got {actual_sha256}"
        );
    }

    Ok(())
}

fn root_role_keyids_fingerprint(root: &Signed<RootMetadata>) -> Result<String> {
    let role = root
        .signed
        .roles
        .get("root")
        .context("destination root metadata missing root role")?;
    let mut keyids = role.keyids.clone();
    keyids.sort();
    crate::ccs::attestation::canonical_json_hash(&keyids)
}

pub(crate) fn verify_destination_matches_operator_keys(
    root: &Signed<RootMetadata>,
    root_key: &SigningKeyPair,
    publish_key: &SigningKeyPair,
) -> Result<()> {
    let (root_key_id, _) = signing_keypair_to_tuf_key(root_key).map_err(anyhow::Error::from)?;
    let root_role = root
        .signed
        .roles
        .get("root")
        .context("destination root metadata missing root role")?;
    if !root_role.keyids.contains(&root_key_id) {
        bail!(
            "destination root role does not match local root key; use force_reinit only for a fresh repo identity"
        );
    }

    let (publish_key_id, _) =
        signing_keypair_to_tuf_key(publish_key).map_err(anyhow::Error::from)?;
    for role in ["targets", "snapshot", "timestamp"] {
        let role_def = root
            .signed
            .roles
            .get(role)
            .with_context(|| format!("destination root metadata missing {role} role"))?;
        if !role_def.keyids.contains(&publish_key_id) {
            bail!(
                "destination {role} role does not match local publish key; use force_reinit only for a fresh repo identity"
            );
        }
    }

    Ok(())
}

pub(crate) fn read_optional(root: &Path, relative: &str) -> Result<Option<Vec<u8>>> {
    validate_repo_relative_path(relative)?;
    let path = root.join(relative);
    match fs::read(&path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
}

#[derive(Default)]
pub(crate) struct PendingKeyRecovery {
    pub(crate) root: bool,
    pub(crate) publish: bool,
}

#[derive(Default)]
pub(crate) struct PendingKeyPromotions {
    entries: Vec<PendingKeyPromotion>,
}

struct PendingKeyPromotion {
    role: String,
    pending_role: String,
}

impl PendingKeyPromotions {
    pub(crate) fn stage_or_load(&mut self, key_dir: &Path, role: &str) -> Result<SigningKeyPair> {
        let pending_role = format!("{role}.pending");
        let key = ensure_pending_key_pair(key_dir, role, &pending_role)?;
        self.track(role);
        Ok(key)
    }

    fn track(&mut self, role: &str) {
        if !self.entries.iter().any(|entry| entry.role == role) {
            self.entries.push(PendingKeyPromotion {
                role: role.to_string(),
                pending_role: format!("{role}.pending"),
            });
        }
    }

    pub(crate) fn promote(&self, key_dir: &Path) -> Result<()> {
        for entry in &self.entries {
            promote_pending_key(key_dir, entry)
                .with_context(|| format!("promote pending {} key", entry.role))?;
        }
        Ok(())
    }
}

pub(crate) fn recover_pending_key_promotions(
    root: &Signed<RootMetadata>,
    key_dir: &Path,
    root_key: &mut SigningKeyPair,
    publish_key: &mut SigningKeyPair,
    pending_key_promotions: &mut PendingKeyPromotions,
) -> Result<PendingKeyRecovery> {
    let mut recovered = PendingKeyRecovery::default();

    if !role_contains_key(root, "root", root_key)?
        && let Some(pending_root_key) = load_pending_key_pair(key_dir, "root")?
        && role_contains_key(root, "root", &pending_root_key)?
    {
        *root_key = pending_root_key;
        pending_key_promotions.track("root");
        recovered.root = true;
    }

    if !publish_roles_contain_key(root, publish_key)?
        && let Some(pending_publish_key) = load_pending_key_pair(key_dir, "publish")?
        && publish_roles_contain_key(root, &pending_publish_key)?
    {
        *publish_key = pending_publish_key;
        pending_key_promotions.track("publish");
        recovered.publish = true;
    }

    Ok(recovered)
}

fn role_contains_key(
    root: &Signed<RootMetadata>,
    role_name: &str,
    key: &SigningKeyPair,
) -> Result<bool> {
    let (key_id, _) = signing_keypair_to_tuf_key(key).map_err(anyhow::Error::from)?;
    let role = root
        .signed
        .roles
        .get(role_name)
        .with_context(|| format!("destination root metadata missing {role_name} role"))?;
    Ok(role.keyids.contains(&key_id))
}

fn publish_roles_contain_key(root: &Signed<RootMetadata>, key: &SigningKeyPair) -> Result<bool> {
    for role_name in ["targets", "snapshot", "timestamp"] {
        if !role_contains_key(root, role_name, key)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn load_pending_key_pair(key_dir: &Path, role: &str) -> Result<Option<SigningKeyPair>> {
    let pending_role = format!("{role}.pending");
    let pending_private = key_dir.join(format!("{pending_role}.private"));
    if !pending_private.exists() {
        return Ok(None);
    }

    let key = SigningKeyPair::load_from_file(&pending_private)
        .map_err(anyhow::Error::from)
        .with_context(|| format!("load pending {role} key {}", pending_private.display()))?;
    save_key_pair(&key, key_dir, &pending_role)
        .with_context(|| format!("refresh pending {role} key files"))?;
    Ok(Some(key))
}

fn ensure_pending_key_pair(
    key_dir: &Path,
    role: &str,
    pending_role: &str,
) -> Result<SigningKeyPair> {
    let pending_private = key_dir.join(format!("{pending_role}.private"));
    if pending_private.exists() {
        let key = SigningKeyPair::load_from_file(&pending_private)
            .map_err(anyhow::Error::from)
            .with_context(|| format!("load pending {role} key {}", pending_private.display()))?;
        save_key_pair(&key, key_dir, pending_role)
            .with_context(|| format!("refresh pending {role} key files"))?;
        return Ok(key);
    }

    let key = SigningKeyPair::generate().with_key_id(role);
    save_key_pair(&key, key_dir, pending_role)
        .with_context(|| format!("stage pending {role} key promotion"))?;
    Ok(key)
}

fn promote_pending_key(key_dir: &Path, entry: &PendingKeyPromotion) -> Result<()> {
    let pending_private = key_dir.join(format!("{}.private", entry.pending_role));
    let pending_public = key_dir.join(format!("{}.public", entry.pending_role));
    let active_private = key_dir.join(format!("{}.private", entry.role));
    let active_public = key_dir.join(format!("{}.public", entry.role));

    fs::rename(&pending_private, &active_private).with_context(|| {
        format!(
            "replace active {} private key {} with {}",
            entry.role,
            active_private.display(),
            pending_private.display()
        )
    })?;
    fs::rename(&pending_public, &active_public).with_context(|| {
        format!(
            "replace active {} public key {} with {}",
            entry.role,
            active_public.display(),
            pending_public.display()
        )
    })
}

pub(crate) fn ensure_key_pair(key_dir: &Path, role: &str) -> Result<SigningKeyPair> {
    let private_path = key_dir.join(format!("{role}.private"));
    if private_path.exists() {
        return SigningKeyPair::load_from_file(&private_path)
            .map_err(anyhow::Error::from)
            .with_context(|| format!("load {role} key {}", private_path.display()));
    }

    let key = SigningKeyPair::generate().with_key_id(role);
    save_key_pair(&key, key_dir, role)?;
    Ok(key)
}

pub(crate) fn save_key_pair(key: &SigningKeyPair, key_dir: &Path, role: &str) -> Result<()> {
    key.save_to_files(
        &key_dir.join(format!("{role}.private")),
        &key_dir.join(format!("{role}.public")),
    )
    .map_err(anyhow::Error::from)
    .with_context(|| format!("save {role} key in {}", key_dir.display()))
}

pub(crate) fn build_package_keys_file(
    old_keys: Option<&PackageKeysFile>,
    publish_key: &SigningKeyPair,
    retired_public_key: Option<String>,
) -> Result<PackageKeysFile> {
    let active_public_key = publish_key.public_key_base64();
    let mut entries = Vec::new();
    let mut seen = BTreeSet::new();

    if let Some(old_keys) = old_keys {
        for key in &old_keys.keys {
            let mut key = key.clone();
            if Some(key.public_key.as_str()) == retired_public_key.as_deref() {
                key.status = PackageKeyStatus::Retired;
            }
            if key.public_key == active_public_key {
                continue;
            }
            if seen.insert(key.public_key.clone()) {
                entries.push(key);
            }
        }
    }

    if let Some(public_key) = retired_public_key
        && public_key != active_public_key
        && seen.insert(public_key.clone())
    {
        entries.push(PackageKeyEntry {
            algorithm: "ed25519".to_string(),
            public_key,
            key_id: Some("publish".to_string()),
            status: PackageKeyStatus::Retired,
            comment: Some("retired publishing key".to_string()),
        });
    }

    entries.push(PackageKeyEntry {
        algorithm: "ed25519".to_string(),
        public_key: active_public_key,
        key_id: Some("publish".to_string()),
        status: PackageKeyStatus::Active,
        comment: Some("primary publishing key".to_string()),
    });

    let keys = PackageKeysFile {
        schema: 1,
        keys: entries,
    };
    keys.validate()?;
    Ok(keys)
}

#[cfg(unix)]
pub(crate) fn create_private_dir_all(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
pub(crate) fn create_private_dir_all(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::static_repo::publish::{StaticPublishOptions, publish_static_repo};
    use crate::repository::static_repo::{PackageKeyEntry, PackageKeyStatus, PackageKeysFile};

    #[test]
    fn new_repo_requires_explicit_key_dir_for_artifact_form() {
        let err =
            match StaticPublishPrepareOptions::artifact_form_new_repo_without_key_dir_for_tests()
                .prepare()
            {
                Ok(_) => panic!("expected missing artifact-form key dir to fail"),
                Err(error) => error,
            };

        assert!(
            err.to_string()
                .contains("artifact-form publish to a new static repo requires --key-dir")
        );
    }

    #[test]
    fn new_artifact_repo_with_key_dir_prepares_initial_publish_key() {
        let temp = tempfile::tempdir().unwrap();
        let key_dir = temp.path().join("keys-local");
        let context = StaticPublishPrepareOptions {
            destination: RepoLocation::File {
                root: temp.path().join("repo"),
            },
            key_dir: Some(key_dir.clone()),
            publish_form: StaticPublishForm::Artifact,
            force_reinit: false,
        }
        .prepare()
        .unwrap();

        assert!(key_dir.join("publish.private").exists());
        assert!(
            context
                .accepted_signers
                .accepts_key_id(&context.active_publish_key_id)
        );
    }

    #[test]
    fn existing_repo_uses_verified_active_package_keys() {
        let temp = tempfile::tempdir().unwrap();
        let context = prepare_existing_repo_context_for_tests(temp.path()).unwrap();

        assert!(
            context
                .accepted_signers
                .accepts_key_id(&context.active_publish_key_id)
        );
    }

    #[test]
    fn new_repo_does_not_trust_stray_package_keys_without_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let key_dir = temp.path().join("keys-local");
        std::fs::create_dir_all(&key_dir).unwrap();
        let key = crate::ccs::signing::SigningKeyPair::generate().with_key_id("publish");
        key.save_to_files(
            &key_dir.join("publish.private"),
            &key_dir.join("publish.public"),
        )
        .unwrap();
        let stray_key = crate::ccs::signing::SigningKeyPair::generate().with_key_id("stray");
        let repo_root = temp.path().join("repo");
        std::fs::create_dir_all(repo_root.join("keys")).unwrap();
        let keys = PackageKeysFile {
            schema: 1,
            keys: vec![PackageKeyEntry {
                algorithm: "ed25519".to_string(),
                public_key: stray_key.public_key_base64(),
                key_id: Some("stray".to_string()),
                status: PackageKeyStatus::Active,
                comment: Some("unverified stray key".to_string()),
            }],
        };
        std::fs::write(
            repo_root.join("keys/package-keys.json"),
            serde_json::to_string_pretty(&keys).unwrap(),
        )
        .unwrap();

        let context = StaticPublishPrepareOptions {
            destination: RepoLocation::File { root: repo_root },
            key_dir: Some(key_dir),
            publish_form: StaticPublishForm::Artifact,
            force_reinit: false,
        }
        .prepare()
        .unwrap();

        assert!(
            context
                .accepted_signers
                .accepts_key_id(&context.active_publish_key_id)
        );
        assert!(!context.accepted_signers.accepts_key_id("stray"));
    }

    #[test]
    fn artifact_destination_snapshot_is_read_only_for_missing_repo() {
        let temp = tempfile::TempDir::new().unwrap();
        let repo = temp.path().join("repo");
        let destination = RepoLocation::File { root: repo.clone() };

        let snapshot = inspect_artifact_form_static_destination(&destination).unwrap();

        assert!(snapshot.initial);
        assert!(snapshot.root_key_fingerprint.is_none());
        assert!(
            !repo.exists(),
            "read-only snapshot must not create repository directories"
        );
    }

    #[test]
    fn artifact_destination_snapshot_reports_existing_trust_state() {
        let temp = tempfile::TempDir::new().unwrap();
        let context = prepare_existing_repo_context_for_tests(temp.path()).unwrap();
        let destination = context.destination.clone();

        let snapshot = inspect_artifact_form_static_destination(&destination).unwrap();

        assert!(!snapshot.initial);
        assert!(
            snapshot
                .root_key_fingerprint
                .as_deref()
                .unwrap()
                .starts_with("sha256:")
        );
        assert!(
            snapshot
                .package_keys_sha256
                .as_deref()
                .unwrap()
                .starts_with("sha256:")
        );
        assert!(
            snapshot
                .accepted_signer_set_hash
                .as_deref()
                .unwrap()
                .starts_with("sha256:")
        );
        assert_eq!(
            snapshot.publish_policy_digest,
            STATIC_PUBLISH_POLICY_DIGEST_V1
        );
        let versions = snapshot.metadata_versions.expect("metadata versions");
        assert!(versions.root_version >= 1);
        assert!(versions.targets_version >= 1);
        assert!(versions.snapshot_version >= 1);
        assert!(versions.timestamp_version >= 1);
    }

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
        key.save_to_files(
            &key_dir.join("publish.private"),
            &key_dir.join("publish.public"),
        )?;
        let repo_root = root.join("repo");
        publish_static_repo(StaticPublishOptions {
            repo_name: "test-repo".to_string(),
            repo_description: None,
            destination: RepoLocation::File {
                root: repo_root.clone(),
            },
            key_dir: key_dir.clone(),
            state_file: root.join("last-published.toml"),
            package_paths: Vec::new(),
            refresh: false,
            force_reinit: false,
            accept_destination_state: false,
            rotate_publish_key: false,
            rotate_root_key: false,
            artifact_gate_context: None,
        })?;
        StaticPublishPrepareOptions {
            destination: RepoLocation::File { root: repo_root },
            key_dir: Some(key_dir),
            publish_form: StaticPublishForm::Artifact,
            force_reinit: false,
        }
        .prepare()
    }
}
