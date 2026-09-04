// conary-core/src/ccs/verify/stream.rs

//! Signature-first streaming verification of trusted CCS archives.

use super::archive_identity::ArchiveDecoder;
use super::{PackageSignature, TrustPolicy, VerifyError};
use crate::ccs::archive_layout::is_lower_hex;
use crate::ccs::budget::{AuthorityCensus, BudgetDimension, CCS_BUDGET};
use crate::ccs::builder::ComponentData;
use crate::ccs::v3::schema::AuthorityDocumentV3;
use crate::packages::payload::{PackagePayload, ReopenablePayload};
use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Read;
use std::path::Path;

#[cfg(test)]
use super::content::expected_objects;
#[cfg(test)]
use crate::ccs::v3::schema::{FileContentLayoutV3, PackageKindV3};

pub(super) struct StreamVerifiedArchive {
    pub archive_identity: super::VerifiedArchiveIdentity,
    pub archive_decode_metrics: crate::ccs::archive_framing::ArchiveDecodeMetrics,
    pub authority: AuthorityDocumentV3,
    pub signature: PackageSignature,
    pub build_attestation: Option<crate::ccs::attestation::BuildAttestationEnvelope>,
    pub foreign_conversion_boundary: Option<crate::ccs::attestation::ForeignConversionBoundary>,
    pub debug_toml: Option<Vec<u8>>,
    pub components: HashMap<String, ComponentData>,
    pub payload: PackagePayload,
    pub object_sources: HashMap<String, ReopenablePayload>,
    pub verified_object_metrics: Option<crate::filesystem::VerifiedObjectBatchMetrics>,
    pub files_checked: usize,
    pub raw: super::RawControlDocuments,
}

#[derive(Default)]
struct MetadataState {
    bytes_read: u64,
    census: Option<AuthorityCensus>,
    manifest: Option<Vec<u8>>,
    signature: Option<String>,
    debug_toml: Option<Vec<u8>>,
    attestation: Option<String>,
    conversion_boundary: Option<String>,
    /// Components the archive carried, keyed by name.
    ///
    /// Collected while streaming and reconciled against the signed authority
    /// once it is authenticated, the same way the object set is.
    components: HashMap<String, ComponentData>,
}

struct AuthenticatedMetadata {
    authority: AuthorityDocumentV3,
    signature: PackageSignature,
    build_attestation: Option<crate::ccs::attestation::BuildAttestationEnvelope>,
    foreign_conversion_boundary: Option<crate::ccs::attestation::ForeignConversionBoundary>,
    expected_objects: BTreeMap<String, u64>,
    files_checked: usize,
}

pub(super) fn verify_archive<'a>(
    path: &Path,
    policy: &TrustPolicy,
    destination: super::object_sink::ObjectDestination<'a>,
    workers: usize,
) -> Result<StreamVerifiedArchive> {
    let mut archive = super::archive_identity::open(path, workers)?;
    let mut metadata = MetadataState::default();
    let mut authenticated = None;
    let mut object_sink = None;
    let mut objects_started = false;
    let mut entries_seen = 0_u64;

    for entry in archive.entries()? {
        entries_seen += 1;
        CCS_BUDGET.admit_archive_entry(entries_seen)?;
        let mut entry = entry?;
        let path = canonical_entry_path(&entry)?;
        let is_object = path.starts_with("objects/");
        if objects_started && (entry.header().entry_type().is_dir() || !is_object) {
            return Err(VerifyError::PackageError(format!(
                "CCS metadata or directory entry {path:?} appears after payload objects"
            ))
            .into());
        }
        if entry.header().entry_type().is_dir() {
            require_known_directory(&path)?;
            continue;
        }
        if is_object {
            if authenticated.is_none() {
                authenticated = Some(authenticate_metadata(&metadata, policy)?);
                object_sink = Some(super::object_sink::VerifiedObjectSink::new(
                    destination,
                    &authenticated
                        .as_ref()
                        .expect("just authenticated")
                        .expected_objects,
                )?);
            }
            objects_started = true;
            read_object(
                &mut entry,
                &path,
                authenticated.as_ref().expect("metadata authenticated"),
                object_sink.as_mut().expect("object sink initialized"),
            )?;
            continue;
        }
        read_metadata_entry(&mut entry, &path, &mut metadata)?;
    }
    let (archive_identity, archive_decode_metrics) = super::archive_identity::finish(archive)?;

    let authenticated = match authenticated {
        Some(value) => value,
        None => authenticate_metadata(&metadata, policy)?,
    };
    let object_sink = match object_sink {
        Some(value) => value,
        None => super::object_sink::VerifiedObjectSink::new(
            destination,
            &authenticated.expected_objects,
        )?,
    };
    let archived_objects = object_sink.seen().clone();
    let expected_objects = authenticated
        .expected_objects
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    if archived_objects != expected_objects {
        return Err(VerifyError::PayloadInvalid(format!(
            "signed object set {expected_objects:?} disagrees with archived set {archived_objects:?}"
        ))
        .into());
    }
    let object_output = object_sink.finish()?;
    let payload =
        super::content::payload_from_authority(&authenticated.authority, &object_output.sources)?;
    // Components are derived from the signed authority, which is the source of
    // truth. Archive component entries are still parsed and validated on the
    // way past — name agreement, duplicates, and size budget — so a malformed
    // one fails closed rather than being skipped. Their payload is not the
    // component source: a package may legitimately carry no component entries
    // while its authority declares components.
    let components = crate::ccs::v3::component_view::components(&authenticated.authority);

    let raw = super::RawControlDocuments {
        manifest: metadata
            .manifest
            .context("CCS package is missing required current v3 MANIFEST authority")?,
        signature: metadata.signature.context("CCS v3 package is not signed")?,
        debug_toml: metadata.debug_toml.clone(),
        build_attestation: metadata.attestation,
        foreign_conversion_boundary: metadata.conversion_boundary,
    };
    Ok(StreamVerifiedArchive {
        archive_identity,
        archive_decode_metrics,
        authority: authenticated.authority,
        signature: authenticated.signature,
        build_attestation: authenticated.build_attestation,
        foreign_conversion_boundary: authenticated.foreign_conversion_boundary,
        debug_toml: metadata.debug_toml,
        components,
        payload,
        object_sources: object_output.sources,
        verified_object_metrics: object_output.metrics,
        files_checked: authenticated.files_checked,
        raw,
    })
}

fn authenticate_metadata(
    metadata: &MetadataState,
    policy: &TrustPolicy,
) -> Result<AuthenticatedMetadata> {
    let manifest = metadata
        .manifest
        .as_deref()
        .context("CCS package is missing required current v3 MANIFEST authority")?;
    let verified = crate::ccs::v3::read_authority_document(
        manifest,
        metadata.signature.as_deref(),
        metadata.debug_toml.as_deref(),
        metadata.attestation.as_deref(),
        metadata.conversion_boundary.as_deref(),
        policy,
    )?;
    let (expected_objects, files_checked) = super::content::expected_objects(&verified.authority)?;
    Ok(AuthenticatedMetadata {
        authority: verified.authority,
        signature: verified.signature,
        build_attestation: verified.build_attestation,
        foreign_conversion_boundary: verified.foreign_conversion_boundary,
        expected_objects,
        files_checked,
    })
}

/// Read one control document.
///
/// `MANIFEST` must be the first archived file. Reading it first gives every
/// later ceiling a census derived from this package's own signed structure
/// instead of a global byte guess.
fn read_metadata_entry(
    entry: &mut tar::Entry<'_, ArchiveDecoder>,
    path: &str,
    state: &mut MetadataState,
) -> Result<()> {
    require_regular(entry, path)?;
    if path == "MANIFEST" {
        return read_authority_entry(entry, state);
    }
    let census = state.census.clone().ok_or_else(|| {
        VerifyError::PackageError(
            "CCS archive must carry its MANIFEST authority before every other archived file"
                .to_string(),
        )
    })?;

    match path {
        "MANIFEST.sig" => {
            let value = read_metadata_utf8(
                entry,
                state,
                CCS_BUDGET.signature_bytes_ceiling(),
                BudgetDimension::SignatureBytes,
                path,
            )?;
            set_once(&mut state.signature, value, path)
        }
        "MANIFEST.toml" => {
            let ceiling = CCS_BUDGET.debug_projection_bytes_ceiling(&census)?;
            let value = read_metadata_control(
                entry,
                state,
                ceiling,
                BudgetDimension::DebugProjectionBytes,
                path,
            )?;
            set_once(&mut state.debug_toml, value, path)
        }
        "MANIFEST.attestation.json" => {
            let value = read_metadata_utf8(
                entry,
                state,
                CCS_BUDGET.attestation_bytes_ceiling(),
                BudgetDimension::AttestationBytes,
                path,
            )?;
            set_once(&mut state.attestation, value, path)
        }
        "MANIFEST.conversion-boundary.json" => {
            let value = read_metadata_utf8(
                entry,
                state,
                CCS_BUDGET.attestation_bytes_ceiling(),
                BudgetDimension::AttestationBytes,
                path,
            )?;
            set_once(&mut state.conversion_boundary, value, path)
        }
        _ if crate::ccs::archive_layout::component_entry_name(path).is_some() => {
            // The signed authority owns the component set; this arm proves the
            // archive agrees with it. The structural-budget rewrite dropped
            // this arm entirely, so the reader rejected component entries its
            // own writer emits.
            let name = crate::ccs::archive_layout::component_entry_name(path)
                .expect("component name checked by the guard")
                .to_string();

            let raw = read_metadata_control(
                entry,
                state,
                CCS_BUDGET.metadata_bytes_ceiling(&census)?,
                BudgetDimension::MetadataBytes,
                path,
            )?;
            let component: ComponentData =
                serde_json::from_slice(&raw).context("invalid CCS component JSON")?;

            if component.name != name.as_str() {
                return Err(VerifyError::PackageError(format!(
                    "CCS component path {path:?} disagrees with component name {:?}",
                    component.name
                ))
                .into());
            }

            if state.components.insert(name.clone(), component).is_some() {
                return Err(VerifyError::PackageError(format!(
                    "CCS archive contains duplicate component {name:?}"
                ))
                .into());
            }
            Ok(())
        }
        _ => Err(VerifyError::PackageError(format!("unknown CCS archive entry {path:?}")).into()),
    }
}

/// Read, decode, and structurally admit the signed authority document.
fn read_authority_entry(
    entry: &mut tar::Entry<'_, ArchiveDecoder>,
    state: &mut MetadataState,
) -> Result<()> {
    if state.manifest.is_some() {
        return Err(VerifyError::PackageError(
            "CCS archive contains duplicate MANIFEST entries".to_string(),
        )
        .into());
    }
    let raw = read_metadata_control(
        entry,
        state,
        CCS_BUDGET.max_authority_bytes(),
        BudgetDimension::AuthorityBytes,
        "MANIFEST",
    )?;
    let authority = CCS_BUDGET.decode_authority(&raw)?;
    let census = crate::ccs::v3::authority_census(&authority)
        .map_err(|error| VerifyError::PackageError(format!("invalid CCS v3 MANIFEST: {error}")))?;
    CCS_BUDGET.admit_encoded_authority(&census, raw.len() as u64)?;
    state.census = Some(census);
    state.manifest = Some(raw);
    Ok(())
}

fn read_object(
    entry: &mut tar::Entry<'_, ArchiveDecoder>,
    path: &str,
    authenticated: &AuthenticatedMetadata,
    sink: &mut super::object_sink::VerifiedObjectSink<'_>,
) -> Result<()> {
    require_regular(entry, path)?;
    let object = path
        .strip_prefix("objects/")
        .context("invalid CCS object path")?;
    let (prefix, suffix) = object.split_once('/').context("invalid CCS object path")?;
    if prefix.len() != 2 || suffix.len() != 62 || !is_lower_hex(prefix) || !is_lower_hex(suffix) {
        return Err(VerifyError::PackageError(format!(
            "invalid canonical CCS SHA-256 object path {object:?}"
        ))
        .into());
    }
    let hash = format!("{prefix}{suffix}");
    let size = authenticated.expected_objects.get(&hash).ok_or_else(|| {
        VerifyError::PayloadInvalid(format!("CCS archive carries unsigned object {hash}"))
    })?;
    if entry.header().size()? != *size {
        return Err(VerifyError::PayloadInvalid(format!(
            "object {hash} declares {} archive bytes but signed authority requires {size}",
            entry.header().size()?
        ))
        .into());
    }
    sink.ingest(&hash, *size, entry)
}

fn read_control(entry: &mut impl Read, limit: u64, label: &str) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    entry
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .with_context(|| format!("read CCS {label}"))?;
    if bytes.len() as u64 > limit {
        return Err(
            VerifyError::PackageError(format!("CCS {label} exceeds maximum size {limit}")).into(),
        );
    }
    Ok(bytes)
}

fn read_metadata_control(
    entry: &mut tar::Entry<'_, ArchiveDecoder>,
    state: &mut MetadataState,
    limit: u64,
    dimension: BudgetDimension,
    label: &str,
) -> Result<Vec<u8>> {
    // Refuse the declared length before reading a byte, so a hostile size
    // declaration costs no allocation.
    CCS_BUDGET.admit_control_bytes(dimension, label, entry.header().size()?, limit)?;
    reserve_metadata_budget(state, entry.header().size()?)?;
    read_control(entry, limit, label)
}

fn read_metadata_utf8(
    entry: &mut tar::Entry<'_, ArchiveDecoder>,
    state: &mut MetadataState,
    limit: u64,
    dimension: BudgetDimension,
    label: &str,
) -> Result<String> {
    String::from_utf8(read_metadata_control(
        entry, state, limit, dimension, label,
    )?)
    .with_context(|| format!("CCS {label} is not valid UTF-8"))
}

/// Bound every control document in one archive together.
///
/// Before the authority is decoded the only admissible control document is
/// `MANIFEST` itself; afterwards the aggregate ceiling is derived from that
/// package's own census.
fn reserve_metadata_budget(state: &mut MetadataState, declared: u64) -> Result<()> {
    let ceiling = match &state.census {
        Some(census) => CCS_BUDGET.metadata_bytes_ceiling(census)?,
        None => CCS_BUDGET.max_authority_bytes(),
    };
    let next = state
        .bytes_read
        .checked_add(declared)
        .context("CCS metadata-size arithmetic overflow")?;
    CCS_BUDGET.admit_control_bytes(
        BudgetDimension::MetadataBytes,
        "control documents",
        next,
        ceiling,
    )?;
    state.bytes_read = next;
    Ok(())
}

fn set_once<T>(slot: &mut Option<T>, value: T, label: &str) -> Result<()> {
    if slot.replace(value).is_some() {
        return Err(VerifyError::PackageError(format!(
            "CCS archive contains duplicate {label} entries"
        ))
        .into());
    }
    Ok(())
}

fn require_regular(entry: &tar::Entry<'_, ArchiveDecoder>, label: &str) -> Result<()> {
    if !entry.header().entry_type().is_file() {
        return Err(
            VerifyError::PackageError(format!("CCS {label} must be a regular file")).into(),
        );
    }
    Ok(())
}

fn canonical_entry_path(entry: &tar::Entry<'_, ArchiveDecoder>) -> Result<String> {
    let path = entry.path_bytes();
    let path =
        std::str::from_utf8(path.as_ref()).context("CCS archive entry path is not valid UTF-8")?;
    canonical_path(path)
}

fn canonical_path(path: &str) -> Result<String> {
    if !crate::ccs::archive_layout::is_canonical_entry_path(path) {
        return Err(
            VerifyError::PackageError(format!("noncanonical CCS archive path {path:?}")).into(),
        );
    }
    Ok(path.to_string())
}

fn require_known_directory(path: &str) -> Result<()> {
    if crate::ccs::archive_layout::is_known_directory(path) {
        return Ok(());
    }
    Err(VerifyError::PackageError(format!("unknown CCS archive directory {path:?}")).into())
}

#[cfg(test)]
mod tests;
