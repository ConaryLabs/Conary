// apps/remi/src/server/native_oracle_input.rs

//! Durable exact native-package-manager input materialization for private candidates.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use conary_core::repository::RepositoryClient;
use conary_core::repository::catalog::{ProfileRevisionV2, SourceSnapshotV1};
use serde::{Deserialize, Serialize};
use url::Url;

use super::catalog_authority::{CatalogAuthority, ProfileRevisionSelection};
use super::open_runtime_db;

pub const NATIVE_ORACLE_INPUT_SCHEMA_V1: u32 = 1;
pub const NATIVE_ORACLE_INPUT_MANIFEST_FILE: &str = "manifest.json";
pub const NATIVE_ORACLE_INPUT_OBJECT_DIRECTORY: &str = "objects";
const MAX_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct NativeOracleInputConfig {
    pub db_path: PathBuf,
    pub catalog_dir: PathBuf,
    pub candidates: Vec<ProfileRevisionSelection>,
    pub output_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeOracleInputSetV1 {
    pub schema_version: u32,
    pub profiles: Vec<NativeOracleInputProfileV1>,
    pub objects: Vec<NativeOracleInputObjectV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeOracleInputProfileV1 {
    pub revision: ProfileRevisionV2,
    pub sources: Vec<SourceSnapshotV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeOracleInputObjectV1 {
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NativeOracleInputOutcome {
    pub output_dir: PathBuf,
    pub manifest_sha256: String,
    pub profiles: usize,
    pub sources: usize,
    pub objects: usize,
    pub object_bytes: u64,
}

struct ObjectFetch {
    object: NativeOracleInputObjectV1,
    url: String,
}

/// Materialize exact native metadata for the current ordered private candidates.
///
/// The candidate set is proved before any network or output mutation and again
/// after the durable bundle has been independently reopened. Persisted reader
/// pins keep the selected immutable profile catalogs alive while downloads run.
pub async fn materialize_native_oracle_inputs(
    config: &NativeOracleInputConfig,
) -> Result<NativeOracleInputOutcome> {
    validate_candidate_selections(&config.candidates)?;
    let initial_candidates = capture_current_candidates(&config.db_path, &config.candidates)?;

    let authority = CatalogAuthority::for_inspection(&config.db_path, &config.catalog_dir);
    let mut pins = Vec::with_capacity(config.candidates.len());
    let mut profiles = Vec::with_capacity(config.candidates.len());
    for selection in &config.candidates {
        let pin = authority
            .open_selected_profile(selection)
            .with_context(|| {
                format!(
                    "pin native-oracle input profile '{}' revision {}",
                    selection.source_profile, selection.profile_revision_sha256
                )
            })?;
        ensure!(
            pin.selection() == selection,
            "native-oracle input profile changed during reopen"
        );
        pin.manifest()
            .validate_member_contract()
            .context("validate native-oracle input profile member contract")?;
        let mut sources = Vec::with_capacity(pin.manifest().members.len());
        for member in &pin.manifest().members {
            sources.push(
                authority
                    .source_snapshot_for_member(&pin, member.ordinal)
                    .with_context(|| {
                        format!(
                            "reopen native-oracle source '{}' member {}",
                            selection.source_profile, member.ordinal
                        )
                    })?,
            );
        }
        profiles.push(NativeOracleInputProfileV1 {
            revision: pin.manifest().clone(),
            sources,
        });
        pins.push(pin);
    }

    let (objects, fetches) = collect_objects(&profiles)?;
    let manifest = NativeOracleInputSetV1 {
        schema_version: NATIVE_ORACLE_INPUT_SCHEMA_V1,
        profiles,
        objects,
    };
    validate_manifest(&manifest)?;
    publish_bundle(&config.output_dir, &manifest, fetches).await?;
    let reopened = reopen_native_oracle_input_bundle(&config.output_dir)?;
    ensure!(
        reopened == manifest,
        "published native-oracle input bundle changed during reopen"
    );

    let final_candidates = capture_current_candidates(&config.db_path, &config.candidates)?;
    ensure!(
        final_candidates == initial_candidates,
        "private candidate set changed during native-oracle input materialization"
    );
    drop(pins);

    let manifest_path = config.output_dir.join(NATIVE_ORACLE_INPUT_MANIFEST_FILE);
    let manifest_bytes = fs::read(&manifest_path).context("reopen native-oracle manifest bytes")?;
    let sources = reopened
        .profiles
        .iter()
        .map(|profile| profile.sources.len())
        .sum();
    let object_bytes = reopened.objects.iter().try_fold(0_u64, |total, object| {
        total
            .checked_add(object.size)
            .context("native-oracle input byte count overflow")
    })?;
    Ok(NativeOracleInputOutcome {
        output_dir: config.output_dir.clone(),
        manifest_sha256: conary_core::hash::sha256(&manifest_bytes),
        profiles: reopened.profiles.len(),
        sources,
        objects: reopened.objects.len(),
        object_bytes,
    })
}

fn validate_candidate_selections(candidates: &[ProfileRevisionSelection]) -> Result<()> {
    let public = conary_core::repository::supported_profiles::public_profiles();
    ensure!(
        candidates.len() == public.len(),
        "native-oracle input requires exactly {} ordered public candidates",
        public.len()
    );
    for (candidate, expected) in candidates.iter().zip(public) {
        ensure!(
            candidate.source_profile == expected.id(),
            "native-oracle input profile '{}' must be '{}' at this ordinal",
            candidate.source_profile,
            expected.id()
        );
        validate_sha256(&candidate.profile_revision_sha256)?;
    }
    Ok(())
}

fn capture_current_candidates(
    db_path: &Path,
    selections: &[ProfileRevisionSelection],
) -> Result<Vec<conary_core::repository::ProfileSyncCandidate>> {
    let conn = open_runtime_db(db_path).context("open Remi database for private candidates")?;
    selections
        .iter()
        .map(|selection| {
            let candidate = conary_core::repository::current_profile_sync_candidate(
                &conn,
                &selection.source_profile,
            )?
            .with_context(|| {
                format!(
                    "profile '{}' has no current private candidate",
                    selection.source_profile
                )
            })?;
            ensure!(
                candidate.profile_revision_sha256 == selection.profile_revision_sha256,
                "profile '{}' current private candidate does not match revision {}",
                selection.source_profile,
                selection.profile_revision_sha256
            );
            conary_core::db::models::verify_private_profile_candidate_authority(
                &conn,
                &candidate.source_profile,
                &candidate.profile_revision_sha256,
                &candidate.run_id,
            )
            .with_context(|| {
                format!(
                    "verify native-oracle private candidate '{}' repository authority",
                    candidate.source_profile
                )
            })?;
            Ok(candidate)
        })
        .collect()
}

fn collect_objects(
    profiles: &[NativeOracleInputProfileV1],
) -> Result<(Vec<NativeOracleInputObjectV1>, Vec<ObjectFetch>)> {
    let mut by_digest = BTreeMap::<String, ObjectFetch>::new();
    for profile in profiles {
        for source in &profile.sources {
            require_public_snapshot(source)?;
            for object in &source.authenticated_objects {
                let input = NativeOracleInputObjectV1 {
                    sha256: object.sha256.clone(),
                    size: object.size,
                };
                let url = source_object_url(source, &object.source_path)?;
                match by_digest.get(&input.sha256) {
                    Some(existing) => ensure!(
                        existing.object.size == input.size,
                        "native metadata digest {} has conflicting sizes",
                        input.sha256
                    ),
                    None => {
                        by_digest.insert(input.sha256.clone(), ObjectFetch { object: input, url });
                    }
                }
            }
        }
    }
    ensure!(
        !by_digest.is_empty(),
        "native-oracle input set contains no authenticated metadata objects"
    );
    let objects = by_digest
        .values()
        .map(|fetch| fetch.object.clone())
        .collect();
    let fetches = by_digest.into_values().collect();
    Ok((objects, fetches))
}

fn source_object_url(source: &SourceSnapshotV1, source_path: &str) -> Result<String> {
    let mut base =
        Url::parse(&source.provenance.metadata_url).context("parse native metadata base URL")?;
    ensure!(
        base.scheme() == "https" && base.host_str().is_some(),
        "native metadata base URL must use public HTTPS authority"
    );
    ensure!(
        base.username().is_empty() && base.password().is_none(),
        "native metadata base URL must not contain credentials"
    );
    if !base.path().ends_with('/') {
        let path = format!("{}/", base.path());
        base.set_path(&path);
    }
    let resolved = base
        .join(source_path)
        .context("resolve authenticated native metadata object URL")?;
    ensure!(
        resolved.scheme() == "https"
            && resolved.host_str() == base.host_str()
            && resolved.port_or_known_default() == base.port_or_known_default(),
        "native metadata object escaped its public HTTPS origin"
    );
    Ok(resolved.into())
}

fn require_public_snapshot(source: &SourceSnapshotV1) -> Result<()> {
    let value = serde_json::to_value(source).context("serialize source snapshot for sanitation")?;
    reject_host_local_strings(&value)
}

fn reject_host_local_strings(value: &serde_json::Value) -> Result<()> {
    match value {
        serde_json::Value::String(value) => ensure!(
            !value.starts_with('/') && !value.starts_with("file://"),
            "native-oracle input manifest contains a host-local path"
        ),
        serde_json::Value::Array(values) => {
            for value in values {
                reject_host_local_strings(value)?;
            }
        }
        serde_json::Value::Object(values) => {
            for value in values.values() {
                reject_host_local_strings(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

async fn publish_bundle(
    output_dir: &Path,
    manifest: &NativeOracleInputSetV1,
    fetches: Vec<ObjectFetch>,
) -> Result<()> {
    let parent = output_parent(output_dir)?;
    match fs::symlink_metadata(output_dir) {
        Ok(_) => bail!(
            "native-oracle input output {} already exists",
            output_dir.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("inspect native-oracle input output"),
    }
    let staged = tempfile::Builder::new()
        .prefix("native-oracle-input-")
        .tempdir_in(&parent)
        .context("create staged native-oracle input directory")?;
    let objects_dir = staged.path().join(NATIVE_ORACLE_INPUT_OBJECT_DIRECTORY);
    fs::create_dir(&objects_dir).context("create native-oracle object directory")?;
    fs::set_permissions(&objects_dir, fs::Permissions::from_mode(0o700))?;

    let client = RepositoryClient::new().context("create native metadata client")?;
    for fetch in fetches {
        let path = objects_dir.join(&fetch.object.sha256);
        let identity = client
            .download_file_with_identity_limit(&fetch.url, &path, fetch.object.size)
            .await
            .with_context(|| format!("download native metadata object {}", fetch.object.sha256))?;
        ensure!(
            identity.sha256 == fetch.object.sha256 && identity.size == fetch.object.size,
            "native metadata object {} identity drifted",
            fetch.object.sha256
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        File::open(&path)?.sync_all()?;
    }

    let manifest_bytes = conary_core::json::canonical_json(manifest)
        .map_err(anyhow::Error::msg)
        .context("canonicalize native-oracle input manifest")?;
    let manifest_path = staged.path().join(NATIVE_ORACLE_INPUT_MANIFEST_FILE);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&manifest_path)
        .context("create native-oracle input manifest")?;
    use std::io::Write;
    file.write_all(&manifest_bytes)?;
    file.sync_all()?;
    File::open(&objects_dir)?.sync_all()?;
    File::open(staged.path())?.sync_all()?;

    fs::rename(staged.path(), output_dir).with_context(|| {
        format!(
            "publish native-oracle input directory {}",
            output_dir.display()
        )
    })?;
    File::open(&parent)?.sync_all()?;
    drop(staged);
    Ok(())
}

pub fn reopen_native_oracle_input_bundle(directory: &Path) -> Result<NativeOracleInputSetV1> {
    require_plain_directory(directory, "native-oracle input root")?;
    require_exact_entries(
        directory,
        &[
            NATIVE_ORACLE_INPUT_MANIFEST_FILE,
            NATIVE_ORACLE_INPUT_OBJECT_DIRECTORY,
        ],
    )?;
    let manifest_path = directory.join(NATIVE_ORACLE_INPUT_MANIFEST_FILE);
    let metadata = fs::symlink_metadata(&manifest_path)?;
    ensure!(
        metadata.file_type().is_file()
            && !metadata.file_type().is_symlink()
            && metadata.len() <= MAX_MANIFEST_BYTES,
        "native-oracle input manifest is not a bounded plain file"
    );
    let bytes = fs::read(&manifest_path).context("read native-oracle input manifest")?;
    let manifest: NativeOracleInputSetV1 =
        serde_json::from_slice(&bytes).context("parse native-oracle input manifest")?;
    validate_manifest(&manifest)?;
    let canonical = conary_core::json::canonical_json(&manifest)
        .map_err(anyhow::Error::msg)
        .context("canonicalize reopened native-oracle input manifest")?;
    ensure!(
        canonical == bytes,
        "native-oracle input manifest is not canonical JSON"
    );

    let objects_dir = directory.join(NATIVE_ORACLE_INPUT_OBJECT_DIRECTORY);
    require_plain_directory(&objects_dir, "native-oracle input object root")?;
    let expected = manifest
        .objects
        .iter()
        .map(|object| object.sha256.as_str())
        .collect::<Vec<_>>();
    require_exact_entries(&objects_dir, &expected)?;
    for object in &manifest.objects {
        let path = objects_dir.join(&object.sha256);
        let metadata = fs::symlink_metadata(&path)?;
        ensure!(
            metadata.file_type().is_file()
                && !metadata.file_type().is_symlink()
                && metadata.len() == object.size,
            "native metadata object {} size or file type drifted",
            object.sha256
        );
        conary_core::hash::verify_file_sha256(&path, &object.sha256)
            .map_err(anyhow::Error::msg)
            .with_context(|| format!("reopen native metadata object {}", object.sha256))?;
    }
    Ok(manifest)
}

fn validate_manifest(manifest: &NativeOracleInputSetV1) -> Result<()> {
    ensure!(
        manifest.schema_version == NATIVE_ORACLE_INPUT_SCHEMA_V1,
        "unsupported native-oracle input schema {}",
        manifest.schema_version
    );
    let public = conary_core::repository::supported_profiles::public_profiles();
    ensure!(
        manifest.profiles.len() == public.len(),
        "native-oracle input manifest requires every public profile"
    );
    let mut expected_objects = BTreeMap::<String, u64>::new();
    for (profile, expected) in manifest.profiles.iter().zip(public) {
        ensure!(
            profile.revision.profile == expected.id(),
            "native-oracle input profile '{}' must be '{}' at this ordinal",
            profile.revision.profile,
            expected.id()
        );
        profile
            .revision
            .validate_member_contract()
            .context("validate native-oracle profile revision")?;
        ensure!(
            profile.sources.len() == profile.revision.members.len(),
            "native-oracle profile '{}' source count drifted",
            profile.revision.profile
        );
        for (source, member) in profile.sources.iter().zip(&profile.revision.members) {
            source
                .validate()
                .context("validate native-oracle source snapshot")?;
            require_public_snapshot(source)?;
            ensure!(
                source.manifest_sha256()? == member.source_snapshot_sha256
                    && source.source_profile == profile.revision.profile
                    && source.source_identity == member.source_identity
                    && source.repository_identity == member.repository_identity
                    && source.stream == member.stream,
                "native-oracle source snapshot disagrees with profile member {}",
                member.ordinal
            );
            for object in &source.authenticated_objects {
                match expected_objects.insert(object.sha256.clone(), object.size) {
                    Some(size) => ensure!(
                        size == object.size,
                        "native metadata digest {} has conflicting sizes",
                        object.sha256
                    ),
                    None => {}
                }
            }
        }
    }
    let expected = expected_objects
        .into_iter()
        .map(|(sha256, size)| NativeOracleInputObjectV1 { sha256, size })
        .collect::<Vec<_>>();
    ensure!(
        manifest.objects == expected,
        "native-oracle input object inventory is incomplete or reordered"
    );
    ensure!(
        !manifest.objects.is_empty(),
        "native-oracle input manifest contains no objects"
    );
    for object in &manifest.objects {
        validate_sha256(&object.sha256)?;
    }
    Ok(())
}

fn output_parent(path: &Path) -> Result<PathBuf> {
    let parent = path
        .parent()
        .context("native-oracle output has no parent")?;
    require_plain_directory(parent, "native-oracle output parent")?;
    Ok(parent.to_path_buf())
}

fn require_plain_directory(path: &Path, label: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect {label} {}", path.display()))?;
    ensure!(
        metadata.file_type().is_dir() && !metadata.file_type().is_symlink(),
        "{label} {} must be a plain directory",
        path.display()
    );
    Ok(())
}

fn require_exact_entries(directory: &Path, expected: &[&str]) -> Result<()> {
    let mut actual = fs::read_dir(directory)?
        .map(|entry| {
            entry?
                .file_name()
                .into_string()
                .map_err(|_| anyhow::anyhow!("native-oracle input entry is not UTF-8"))
        })
        .collect::<Result<Vec<_>>>()?;
    actual.sort();
    let mut expected = expected.iter().map(ToString::to_string).collect::<Vec<_>>();
    expected.sort();
    ensure!(
        actual == expected,
        "native-oracle input directory entries are incomplete or unexpected"
    );
    Ok(())
}

fn validate_sha256(value: &str) -> Result<()> {
    ensure!(
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "native-oracle input requires a lowercase SHA-256 digest"
    );
    Ok(())
}

#[cfg(test)]
mod tests;
