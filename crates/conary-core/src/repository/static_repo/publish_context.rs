// conary-core/src/repository/static_repo/publish_context.rs

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::ccs::manifest_provenance::ManifestProvenance;
use crate::ccs::signing::SigningKeyPair;
use crate::hash;
use crate::repository::static_repo::publish_gate::AcceptedStaticSignerSet;
use crate::repository::static_repo::{PackageKeysFile, RepoLocation, validate_repo_relative_path};
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
            None => bail!("static publish requires --key-dir"),
        };

        create_private_dir_all(&key_dir)
            .with_context(|| format!("create static repo key directory {}", key_dir.display()))?;
        let verified_package_keys = if self.publish_form == StaticPublishForm::Artifact {
            load_verified_package_keys_for_destination(&self.destination)?
        } else {
            None
        };
        let active_publish_key = match self.publish_form {
            StaticPublishForm::Project => ensure_key_pair(&key_dir, "publish")?,
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
                if let Some(package_keys) = verified_package_keys {
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
) -> Result<PreparedStaticPublishContext> {
    StaticPublishPrepareOptions {
        destination: destination.clone(),
        key_dir: Some(key_dir.to_path_buf()),
        publish_form: StaticPublishForm::Project,
    }
    .prepare()
}

pub fn prepare_artifact_form_static_context(
    destination: &RepoLocation,
    key_dir: &Path,
) -> Result<PreparedStaticPublishContext> {
    StaticPublishPrepareOptions {
        destination: destination.clone(),
        key_dir: Some(key_dir.to_path_buf()),
        publish_form: StaticPublishForm::Artifact,
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
    let destination = read_destination_state(root)?;
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
    let trusted_key = input.context.active_publish_key.public_key_base64();
    let verification = crate::ccs::verify::verify_package(
        input.package_path,
        &crate::ccs::verify::TrustPolicy::strict(vec![trusted_key.clone()]),
    )?;

    let package = crate::ccs::CcsPackage::from_verified_archive(
        input.package_path.to_str().with_context(|| {
            format!(
                "package path is not valid UTF-8: {}",
                input.package_path.display()
            )
        })?,
        &verification,
    )
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
    let authority = package
        .v2_authority()
        .context("verified v2 package missing authority")?;
    let payloads_by_path = v2_payloads_by_path(&package, authority)?;
    let debug_toml = verification
        .archive()
        .toml_raw
        .as_deref()
        .map(std::str::from_utf8)
        .transpose()
        .context("decode v2 MANIFEST.toml as UTF-8")?;
    let signed_temp =
        tempfile::Builder::new()
            .prefix("conary-attested-")
            .suffix(".ccs")
            .tempfile_in(input.package_path.parent().with_context(|| {
                format!("resolve parent for {}", input.package_path.display())
            })?)?;

    crate::ccs::builder::write_v2_ccs_package(
        authority,
        &payloads_by_path,
        signed_temp.path(),
        &input.context.active_publish_key,
        debug_toml,
        Some(&envelope),
        package.v2_foreign_conversion_boundary(),
    )?;

    crate::ccs::verify::verify_package(
        signed_temp.path(),
        &crate::ccs::verify::TrustPolicy::strict(vec![trusted_key]),
    )?;
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

fn v2_payloads_by_path(
    package: &crate::ccs::CcsPackage,
    authority: &crate::ccs::v2::AuthorityDocumentV2,
) -> Result<std::collections::BTreeMap<String, Vec<u8>>> {
    use crate::ccs::v2::schema::PackageKindV2;
    use crate::payload::PayloadNodeKind;

    let PackageKindV2::Package(data) = &authority.kind else {
        bail!("M4a project-form v2 attestation only supports package payloads");
    };
    let blobs = package.extract_all_content().map_err(anyhow::Error::from)?;
    let mut payloads_by_path = std::collections::BTreeMap::new();
    for file in &data.files {
        if matches!(&file.node.kind, PayloadNodeKind::Regular { .. }) {
            let content = file.content.as_ref().with_context(|| {
                format!(
                    "verified v2 regular node {} has no content authority",
                    file.path
                )
            })?;
            let payload = blobs.get(&content.sha256).with_context(|| {
                format!(
                    "verified v2 package is missing payload blob {} for {}",
                    content.sha256, file.path
                )
            })?;
            payloads_by_path.insert(file.path.clone(), payload.clone());
        }
    }
    Ok(payloads_by_path)
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
    Ok(())
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
) -> Result<Option<PackageKeysFile>> {
    let RepoLocation::File { root } = destination else {
        bail!("static publish context can only load package keys from file destinations");
    };
    let destination = read_destination_state(root)?;
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

pub(crate) fn read_destination_state(repo_root: &Path) -> Result<DestinationState> {
    let root_bytes = read_optional(repo_root, "metadata/root.json")?;
    let targets_bytes = read_optional(repo_root, "metadata/targets.json")?;
    let snapshot_bytes = read_optional(repo_root, "metadata/snapshot.json")?;
    let timestamp_bytes = read_optional(repo_root, "metadata/timestamp.json")?;

    let all_absent = root_bytes.is_none()
        && targets_bytes.is_none()
        && snapshot_bytes.is_none()
        && timestamp_bytes.is_none();
    if all_absent {
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
            "static repo destination is damaged or partially initialized; choose a new empty destination or recover the existing signed metadata"
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
            "destination root role does not match local root key; choose a new empty destination for a fresh repo identity"
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
                "destination {role} role does not match local publish key; choose a new empty destination for a fresh repo identity"
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

mod key_management;
#[cfg(test)]
pub(crate) use key_management::save_key_pair;
pub(crate) use key_management::{
    PendingKeyPromotions, PendingKeyRecovery, build_package_keys_file, create_private_dir_all,
    ensure_key_pair, recover_pending_key_promotions,
};

#[cfg(test)]
#[path = "publish_context/tests.rs"]
mod tests;
