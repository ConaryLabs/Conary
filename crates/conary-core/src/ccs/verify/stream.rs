// conary-core/src/ccs/verify/stream.rs

//! Signature-first streaming verification of trusted CCS archives.

use super::{PackageSignature, TrustPolicy, VerifyError};
use crate::ccs::archive_layout::is_lower_hex;
use crate::ccs::budget::{AuthorityCensus, BudgetDimension, CCS_BUDGET};
use crate::ccs::builder::ComponentData;
use crate::ccs::v2::schema::{AuthorityDocumentV2, PackageKindV2};
use crate::filesystem::CasStore;
use crate::packages::payload::{
    PackagePayload, PackagePayloadFile, PayloadSpool, ReopenablePayload,
};
use anyhow::{Context, Result};
use flate2::bufread::GzDecoder;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Component, Path};
use tar::Archive;

const EXPECTED_TAR_TRAILING_ZERO_BYTES: u64 = 512;

type ArchiveDecoder = GzDecoder<BufReader<File>>;

pub(super) struct StreamVerifiedArchive {
    pub authority: AuthorityDocumentV2,
    pub signature: PackageSignature,
    pub build_attestation: Option<crate::ccs::attestation::BuildAttestationEnvelope>,
    pub foreign_conversion_boundary: Option<crate::ccs::attestation::ForeignConversionBoundary>,
    pub debug_toml: Option<Vec<u8>>,
    pub components: HashMap<String, ComponentData>,
    pub payload: PackagePayload,
    pub files_checked: usize,
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
    authority: AuthorityDocumentV2,
    signature: PackageSignature,
    build_attestation: Option<crate::ccs::attestation::BuildAttestationEnvelope>,
    foreign_conversion_boundary: Option<crate::ccs::attestation::ForeignConversionBoundary>,
    expected_objects: BTreeMap<String, u64>,
    files_checked: usize,
}

pub(super) fn verify_archive(path: &Path, policy: &TrustPolicy) -> Result<StreamVerifiedArchive> {
    let file = File::open(path).with_context(|| format!("open CCS package {}", path.display()))?;
    let decoder = GzDecoder::new(BufReader::new(file));
    let mut archive = Archive::new(decoder);
    let mut metadata = MetadataState::default();
    let mut authenticated = None;
    let mut spool = None;
    let mut object_store = None;
    let mut object_sources = HashMap::new();
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
                let required = authenticated
                    .as_ref()
                    .expect("just authenticated")
                    .expected_objects
                    .values()
                    .try_fold(0_u64, |total, size| total.checked_add(*size))
                    .context("CCS signed object-size arithmetic overflow")?;
                let payload_spool = PayloadSpool::new(required)?;
                object_store = Some(CasStore::new(payload_spool.root().join("objects"))?);
                spool = Some(payload_spool);
            }
            objects_started = true;
            read_object(
                &mut entry,
                &path,
                authenticated.as_ref().expect("metadata authenticated"),
                object_store.as_ref().expect("object store initialized"),
                spool.as_ref().expect("payload spool initialized"),
                &mut object_sources,
            )?;
            continue;
        }
        read_metadata_entry(&mut entry, &path, &mut metadata)?;
    }
    finish_archive(archive)?;

    let authenticated = match authenticated {
        Some(value) => value,
        None => authenticate_metadata(&metadata, policy)?,
    };
    let archived_objects = object_sources.keys().cloned().collect::<BTreeSet<_>>();
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
    let payload = payload_from_authority(&authenticated.authority, &object_sources)?;
    // Components are derived from the signed authority, which is the source of
    // truth. Archive component entries are still parsed and validated on the
    // way past — name agreement, duplicates, and size budget — so a malformed
    // one fails closed rather than being skipped. Their payload is not the
    // component source: a package may legitimately carry no component entries
    // while its authority declares components.
    let components = crate::ccs::v2::component_view::components(&authenticated.authority);

    Ok(StreamVerifiedArchive {
        authority: authenticated.authority,
        signature: authenticated.signature,
        build_attestation: authenticated.build_attestation,
        foreign_conversion_boundary: authenticated.foreign_conversion_boundary,
        debug_toml: metadata.debug_toml,
        components,
        payload,
        files_checked: authenticated.files_checked,
    })
}

fn authenticate_metadata(
    metadata: &MetadataState,
    policy: &TrustPolicy,
) -> Result<AuthenticatedMetadata> {
    let manifest = metadata
        .manifest
        .as_deref()
        .context("CCS package is missing required current v2 MANIFEST authority")?;
    let verified = crate::ccs::v2::read_authority_document(
        manifest,
        metadata.signature.as_deref(),
        metadata.debug_toml.as_deref(),
        metadata.attestation.as_deref(),
        metadata.conversion_boundary.as_deref(),
        policy,
    )?;
    let (expected_objects, files_checked) = expected_objects(&verified.authority)?;
    Ok(AuthenticatedMetadata {
        authority: verified.authority,
        signature: verified.signature,
        build_attestation: verified.build_attestation,
        foreign_conversion_boundary: verified.foreign_conversion_boundary,
        expected_objects,
        files_checked,
    })
}

fn expected_objects(authority: &AuthorityDocumentV2) -> Result<(BTreeMap<String, u64>, usize)> {
    let PackageKindV2::Package(package) = &authority.kind else {
        return Ok((BTreeMap::new(), 0));
    };
    let mut objects = BTreeMap::new();
    for file in &package.files {
        if let Some(content) = &file.content
            && (content.sha256.len() != 64 || !is_lower_hex(&content.sha256))
        {
            return Err(VerifyError::PayloadInvalid(format!(
                "signed object digest {:?} is not canonical lowercase SHA-256",
                content.sha256
            ))
            .into());
        }
        file.node
            .validate_content(file.content.as_ref())
            .map_err(|error| {
                VerifyError::PayloadInvalid(format!(
                    "v2 payload authority for {} is invalid: {error}",
                    file.path
                ))
            })?;
        if let Some(content) = &file.content
            && let Some(previous) = objects.insert(content.sha256.clone(), content.size)
            && previous != content.size
        {
            return Err(VerifyError::PayloadInvalid(format!(
                "signed object {} carries conflicting sizes {previous} and {}",
                content.sha256, content.size
            ))
            .into());
        }
    }
    Ok((objects, package.files.len()))
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
    let census = crate::ccs::v2::authority_census(&authority)
        .map_err(|error| VerifyError::PackageError(format!("invalid CCS v2 MANIFEST: {error}")))?;
    CCS_BUDGET.admit_encoded_authority(&census, raw.len() as u64)?;
    state.census = Some(census);
    state.manifest = Some(raw);
    Ok(())
}

fn read_object(
    entry: &mut tar::Entry<'_, ArchiveDecoder>,
    path: &str,
    authenticated: &AuthenticatedMetadata,
    store: &CasStore,
    spool: &PayloadSpool,
    sources: &mut HashMap<String, ReopenablePayload>,
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
    if sources.contains_key(&hash) {
        return Err(VerifyError::PackageError(format!(
            "CCS archive contains duplicate object {hash}"
        ))
        .into());
    }
    match store.store_reader_expected(entry, *size, &hash) {
        Ok(_) => {}
        Err(crate::Error::ChecksumMismatch { expected, actual }) => {
            return Err(VerifyError::PayloadInvalid(format!(
                "CCS object path hash mismatch: expected {expected}, got {actual}"
            ))
            .into());
        }
        Err(error) => return Err(error.into()),
    }
    let path = store.hash_to_path(&hash)?;
    sources.insert(hash, spool.source(path));
    Ok(())
}

fn payload_from_authority(
    authority: &AuthorityDocumentV2,
    objects: &HashMap<String, ReopenablePayload>,
) -> Result<PackagePayload> {
    let PackageKindV2::Package(package) = &authority.kind else {
        return Ok(PackagePayload::default());
    };
    package
        .files
        .iter()
        .map(|file| {
            let source = file
                .content
                .as_ref()
                .map(|content| {
                    objects.get(&content.sha256).cloned().ok_or_else(|| {
                        VerifyError::PayloadInvalid(format!(
                            "missing authenticated object {} for {}",
                            content.sha256, file.path
                        ))
                    })
                })
                .transpose()?;
            PackagePayloadFile::new(
                file.path.clone(),
                file.node.clone(),
                file.content.clone(),
                source,
            )
            .map_err(anyhow::Error::from)
        })
        .collect::<Result<Vec<_>>>()
        .map(PackagePayload::new)
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
    if path.is_empty() || path.starts_with('/') || path.ends_with('/') {
        return Err(
            VerifyError::PackageError(format!("noncanonical CCS archive path {path:?}")).into(),
        );
    }
    let parsed = Path::new(path);
    if parsed
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
        || path.split('/').any(str::is_empty)
    {
        return Err(
            VerifyError::PackageError(format!("noncanonical CCS archive path {path:?}")).into(),
        );
    }
    let normalized = parsed
        .to_str()
        .context("CCS archive entry path is not valid UTF-8")?;
    Ok(normalized.to_string())
}

fn finish_archive(archive: Archive<ArchiveDecoder>) -> Result<()> {
    let mut decoder = archive.into_inner();
    let mut trailing = 0_u64;
    let mut buffer = [0_u8; 512];
    loop {
        let read = decoder
            .read(&mut buffer)
            .context("finish CCS gzip member and verify its CRC/footer")?;
        if read == 0 {
            break;
        }
        if buffer[..read].iter().any(|byte| *byte != 0) {
            return Err(VerifyError::PackageError(
                "CCS archive carries non-zero data after the canonical tar terminator".to_string(),
            )
            .into());
        }
        trailing = trailing
            .checked_add(read as u64)
            .context("CCS tar-padding arithmetic overflow")?;
        if trailing > EXPECTED_TAR_TRAILING_ZERO_BYTES {
            return Err(VerifyError::PackageError(format!(
                "CCS tar terminator/padding exceeds {EXPECTED_TAR_TRAILING_ZERO_BYTES} bytes"
            ))
            .into());
        }
    }
    if trailing != EXPECTED_TAR_TRAILING_ZERO_BYTES {
        return Err(VerifyError::PackageError(format!(
            "CCS tar terminator/padding has noncanonical length {trailing}; expected {EXPECTED_TAR_TRAILING_ZERO_BYTES}"
        ))
        .into());
    }
    let mut compressed = decoder.into_inner();
    let mut extra = [0_u8; 1];
    if compressed.read(&mut extra)? != 0 {
        return Err(VerifyError::PackageError(
            "CCS package carries trailing compressed bytes or a second gzip member".to_string(),
        )
        .into());
    }
    Ok(())
}

fn require_known_directory(path: &str) -> Result<()> {
    if crate::ccs::archive_layout::is_known_directory(path) {
        return Ok(());
    }
    Err(VerifyError::PackageError(format!("unknown CCS archive directory {path:?}")).into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::builder::write_v2_ccs_package_from_bounded_memory_for_tests;
    use crate::ccs::signing::SigningKeyPair;
    use flate2::Compression;
    use flate2::read::GzDecoder as ReadGzDecoder;
    use flate2::write::GzEncoder;
    use std::io::Write;
    use tar::{Builder, EntryType, Header};

    #[derive(Clone)]
    struct TestEntry {
        path: String,
        entry_type: EntryType,
        content: Vec<u8>,
    }

    fn fixture() -> (
        tempfile::TempDir,
        Vec<TestEntry>,
        TrustPolicy,
        std::path::PathBuf,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("base.ccs");
        let authority = crate::ccs::v2::test_support::package_authority_with_one_file("stream");
        let payloads = crate::ccs::v2::test_support::one_file_payloads_for_tests();
        let signer = SigningKeyPair::generate();
        write_v2_ccs_package_from_bounded_memory_for_tests(
            &authority, &payloads, &path, &signer, None, None, None,
        )
        .unwrap();
        let policy = TrustPolicy::strict(vec![signer.public_key_base64()]);

        let mut archive = Archive::new(ReadGzDecoder::new(File::open(&path).unwrap()));
        let entries = archive
            .entries()
            .unwrap()
            .map(|entry| {
                let mut entry = entry.unwrap();
                let path = std::str::from_utf8(entry.path_bytes().as_ref())
                    .unwrap()
                    .to_string();
                let entry_type = entry.header().entry_type();
                let mut content = Vec::new();
                entry.read_to_end(&mut content).unwrap();
                TestEntry {
                    path,
                    entry_type,
                    content,
                }
            })
            .collect();
        (temp, entries, policy, path)
    }

    fn write_fixture(path: &Path, entries: &[TestEntry]) {
        let encoder = GzEncoder::new(File::create(path).unwrap(), Compression::default());
        let mut archive = Builder::new(encoder);
        for entry in entries {
            let mut header = Header::new_gnu();
            header.set_path(&entry.path).unwrap();
            header.set_entry_type(entry.entry_type);
            header.set_mode(if entry.entry_type.is_dir() {
                0o755
            } else {
                0o644
            });
            header.set_size(entry.content.len() as u64);
            header.set_cksum();
            archive
                .append_data(&mut header, &entry.path, entry.content.as_slice())
                .unwrap();
        }
        archive.into_inner().unwrap().finish().unwrap();
    }

    fn error_text(path: &Path, policy: &TrustPolicy) -> String {
        match verify_archive(path, policy) {
            Ok(_) => panic!("mutated archive unexpectedly verified"),
            Err(error) => format!("{error:#}"),
        }
    }

    fn decoded_tar(path: &Path) -> Vec<u8> {
        let mut decoded = Vec::new();
        ReadGzDecoder::new(File::open(path).unwrap())
            .read_to_end(&mut decoded)
            .unwrap();
        decoded
    }

    fn write_gzip_payload(path: &Path, payload: &[u8]) {
        let mut encoder = GzEncoder::new(File::create(path).unwrap(), Compression::default());
        encoder.write_all(payload).unwrap();
        encoder.finish().unwrap();
    }

    #[test]
    fn rejects_noncanonical_paths_and_nested_object_directories() {
        let (temp, entries, policy, _) = fixture();
        let error = canonical_path("./MANIFEST").unwrap_err();
        assert!(format!("{error:#}").contains("noncanonical CCS archive path"));

        let object_index = entries
            .iter()
            .position(|entry| entry.path.starts_with("objects/") && !entry.entry_type.is_dir())
            .unwrap();
        let mut nested = entries;
        nested.insert(
            object_index,
            TestEntry {
                path: "objects/ab/cd".to_string(),
                entry_type: EntryType::Directory,
                content: Vec::new(),
            },
        );
        let path = temp.path().join("nested-dir.ccs");
        write_fixture(&path, &nested);
        assert!(error_text(&path, &policy).contains("unknown CCS archive directory"));
    }

    #[test]
    fn rejects_metadata_after_objects_and_duplicate_metadata() {
        let (temp, entries, policy, _) = fixture();
        let signature = entries
            .iter()
            .find(|entry| entry.path == "MANIFEST.sig")
            .unwrap()
            .clone();
        let object_index = entries
            .iter()
            .position(|entry| entry.path.starts_with("objects/") && !entry.entry_type.is_dir())
            .unwrap();
        let mut reordered = entries.clone();
        reordered.insert(object_index + 1, signature);
        let path = temp.path().join("metadata-after.ccs");
        write_fixture(&path, &reordered);
        assert!(error_text(&path, &policy).contains("appears after payload objects"));

        let directory = entries
            .iter()
            .find(|entry| entry.path.starts_with("objects/") && entry.entry_type.is_dir())
            .unwrap()
            .clone();
        let mut directory_after = entries.clone();
        directory_after.insert(object_index + 1, directory);
        let path = temp.path().join("directory-after.ccs");
        write_fixture(&path, &directory_after);
        assert!(error_text(&path, &policy).contains("appears after payload objects"));

        let manifest = entries
            .iter()
            .find(|entry| entry.path == "MANIFEST")
            .unwrap()
            .clone();
        let mut duplicate = entries;
        duplicate.insert(1, manifest);
        let path = temp.path().join("duplicate-metadata.ccs");
        write_fixture(&path, &duplicate);
        assert!(error_text(&path, &policy).contains("duplicate MANIFEST entries"));
    }

    #[test]
    fn rejects_unsigned_missing_duplicate_and_wrong_size_objects() {
        let (temp, entries, policy, _) = fixture();
        let object = entries
            .iter()
            .find(|entry| entry.path.starts_with("objects/") && !entry.entry_type.is_dir())
            .unwrap()
            .clone();

        let unsigned_bytes = b"unsigned".to_vec();
        let unsigned_hash = crate::hash::sha256(&unsigned_bytes);
        let mut unsigned = entries.clone();
        unsigned.push(TestEntry {
            path: format!("objects/{}/{}", &unsigned_hash[..2], &unsigned_hash[2..]),
            entry_type: EntryType::Regular,
            content: unsigned_bytes,
        });
        let path = temp.path().join("unsigned.ccs");
        write_fixture(&path, &unsigned);
        assert!(error_text(&path, &policy).contains("carries unsigned object"));

        let missing = entries
            .iter()
            .filter(|entry| entry.path != object.path)
            .cloned()
            .collect::<Vec<_>>();
        let path = temp.path().join("missing.ccs");
        write_fixture(&path, &missing);
        assert!(error_text(&path, &policy).contains("disagrees with archived set"));

        let mut duplicate = entries.clone();
        duplicate.push(object.clone());
        let path = temp.path().join("duplicate.ccs");
        write_fixture(&path, &duplicate);
        assert!(error_text(&path, &policy).contains("duplicate object"));

        let mut wrong_size = entries;
        wrong_size
            .iter_mut()
            .find(|entry| entry.path == object.path)
            .unwrap()
            .content
            .pop();
        let path = temp.path().join("wrong-size.ccs");
        write_fixture(&path, &wrong_size);
        assert!(error_text(&path, &policy).contains("signed authority requires"));
    }

    #[test]
    fn rejects_truncated_archive_and_aggregate_metadata_without_allocation() {
        let (temp, _, policy, base) = fixture();
        let mut bytes = Vec::new();
        File::open(base).unwrap().read_to_end(&mut bytes).unwrap();
        let truncated = temp.path().join("truncated.ccs");
        let mut output = File::create(&truncated).unwrap();
        output.write_all(&bytes[..bytes.len() / 2]).unwrap();
        assert!(verify_archive(&truncated, &policy).is_err());

        // The aggregate control-document ceiling is derived from this
        // package's own census, so padding is refused without allocation.
        let authority = crate::ccs::v2::test_support::package_authority_with_one_file("stream");
        let census = crate::ccs::v2::authority_census(&authority).unwrap();
        let ceiling = CCS_BUDGET.metadata_bytes_ceiling(&census).unwrap();
        let mut state = MetadataState {
            census: Some(census),
            bytes_read: ceiling,
            ..MetadataState::default()
        };
        let error = reserve_metadata_budget(&mut state, 1).unwrap_err();
        assert!(format!("{error:#}").contains("metadata-bytes"), "{error:#}");
        assert_eq!(state.bytes_read, ceiling);
    }

    #[test]
    fn rejects_missing_extra_and_nonzero_tar_terminator_bytes() {
        let (temp, _, policy, base) = fixture();
        let raw = decoded_tar(&base);
        assert!(raw.len() >= 1024);
        assert!(raw[raw.len() - 1024..].iter().all(|byte| *byte == 0));

        let missing = temp.path().join("missing-terminator.ccs");
        write_gzip_payload(&missing, &raw[..raw.len() - 512]);
        assert!(error_text(&missing, &policy).contains("noncanonical length"));

        let extra = temp.path().join("extra-terminator.ccs");
        let mut extra_raw = raw.clone();
        extra_raw.extend_from_slice(&[0_u8; 512]);
        write_gzip_payload(&extra, &extra_raw);
        assert!(error_text(&extra, &policy).contains("terminator/padding exceeds"));

        let nonzero = temp.path().join("nonzero-terminator.ccs");
        let mut nonzero_raw = raw;
        *nonzero_raw.last_mut().unwrap() = 1;
        write_gzip_payload(&nonzero, &nonzero_raw);
        assert!(error_text(&nonzero, &policy).contains("non-zero data"));
    }

    #[test]
    fn rejects_appended_tar_data_and_second_gzip_member() {
        let (temp, _, policy, base) = fixture();

        let appended_tar = temp.path().join("appended-tar.ccs");
        let mut raw = decoded_tar(&base);
        raw.extend_from_slice(b"unsigned appended tar bytes");
        write_gzip_payload(&appended_tar, &raw);
        assert!(error_text(&appended_tar, &policy).contains("non-zero data"));

        let appended_gzip = temp.path().join("appended-gzip.ccs");
        let mut bytes = Vec::new();
        File::open(&base).unwrap().read_to_end(&mut bytes).unwrap();
        let mut member = GzEncoder::new(Vec::new(), Compression::default());
        member.write_all(b"second member").unwrap();
        bytes.extend(member.finish().unwrap());
        std::fs::write(&appended_gzip, bytes).unwrap();
        assert!(error_text(&appended_gzip, &policy).contains("second gzip member"));
    }

    #[test]
    fn signed_object_authority_uses_lowercase_sha256_and_u64_sizes() {
        let mut authority = crate::ccs::v2::test_support::package_authority_with_one_file("stream");
        {
            let PackageKindV2::Package(package) = &mut authority.kind else {
                unreachable!()
            };
            package.files[0].content.as_mut().unwrap().size = u64::from(u32::MAX) + 1;
        }
        let (objects, _) = expected_objects(&authority).unwrap();
        assert_eq!(
            objects.values().copied().collect::<Vec<_>>(),
            vec![u64::from(u32::MAX) + 1]
        );

        let PackageKindV2::Package(package) = &mut authority.kind else {
            unreachable!()
        };
        package.files[0].content.as_mut().unwrap().sha256 = "A".repeat(64);
        let error = expected_objects(&authority).unwrap_err();
        assert!(format!("{error:#}").contains("canonical lowercase SHA-256"));
    }
}
