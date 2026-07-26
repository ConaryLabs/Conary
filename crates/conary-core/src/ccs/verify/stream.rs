// conary-core/src/ccs/verify/stream.rs

//! Signature-first streaming verification of trusted CCS archives.

use super::{PackageSignature, TrustPolicy, VerifyError};
use crate::ccs::builder::ComponentData;
use crate::ccs::v2::schema::{AuthorityDocumentV2, PackageKindV2};
use crate::filesystem::CasStore;
use crate::packages::payload::{
    PackagePayload, PackagePayloadFile, PayloadSpool, ReopenablePayload,
};
use anyhow::{Context, Result};
use flate2::bufread::GzDecoder;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Component, Path};
use tar::Archive;

const MAX_CONTROL_SIZE: u64 = 16 * 1024 * 1024;
const MAX_COMPONENT_SIZE: u64 = 64 * 1024 * 1024;
const MAX_METADATA_TOTAL_SIZE: u64 = 128 * 1024 * 1024;
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
    manifest: Option<Vec<u8>>,
    signature: Option<String>,
    debug_toml: Option<Vec<u8>>,
    attestation: Option<String>,
    conversion_boundary: Option<String>,
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
    let mut entries_seen = 0_usize;

    for entry in archive.entries()? {
        entries_seen += 1;
        crate::compression::check_archive_entry_limit(entries_seen, "CCS package")?;
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
    Ok(StreamVerifiedArchive {
        authority: authenticated.authority,
        signature: authenticated.signature,
        build_attestation: authenticated.build_attestation,
        foreign_conversion_boundary: authenticated.foreign_conversion_boundary,
        debug_toml: metadata.debug_toml,
        components: metadata.components,
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
    verify_components(&verified.authority, &metadata.components)?;
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

fn verify_components(
    authority: &AuthorityDocumentV2,
    components: &HashMap<String, ComponentData>,
) -> Result<()> {
    let signed_names = authority.components.keys().collect::<BTreeSet<_>>();
    let archived_names = components.keys().collect::<BTreeSet<_>>();
    if signed_names != archived_names {
        return Err(VerifyError::PayloadInvalid(format!(
            "signed component set {signed_names:?} disagrees with archived set {archived_names:?}"
        ))
        .into());
    }
    let PackageKindV2::Package(package) = &authority.kind else {
        return Ok(());
    };
    let signed_files = package
        .files
        .iter()
        .map(|file| ((file.component.as_str(), file.path.as_str()), file))
        .collect::<HashMap<_, _>>();
    let mut archived_files = HashSet::new();
    for (name, component) in components {
        let signed = authority
            .components
            .get(name)
            .expect("component sets are equal");
        let file_count = u32::try_from(component.files.len()).map_err(|_| {
            VerifyError::PayloadInvalid(format!("component {name:?} file count exceeds u32"))
        })?;
        let total_size = component.files.iter().try_fold(0_u64, |total, file| {
            total.checked_add(file.content.as_ref().map_or(0, |content| content.size))
        });
        let total_size = total_size.ok_or_else(|| {
            VerifyError::PayloadInvalid(format!("component {name:?} size overflows u64"))
        })?;
        if file_count != signed.file_count || total_size != signed.total_size {
            return Err(VerifyError::PayloadInvalid(format!(
                "component {name:?} summary disagrees with signed authority"
            ))
            .into());
        }
        let actual_hash = crate::hash::sha256_prefixed(
            &crate::ccs::attestation::canonical_json_bytes(&component.files)?,
        );
        if component.hash != actual_hash {
            return Err(VerifyError::PayloadInvalid(format!(
                "component {name:?} projection hash mismatch"
            ))
            .into());
        }
        for file in &component.files {
            if file.chunks.is_some() {
                return Err(VerifyError::PayloadInvalid(format!(
                    "component file {} uses retired chunk projection",
                    file.path
                ))
                .into());
            }
            let key = (name.as_str(), file.path.as_str());
            if !archived_files.insert(key) {
                return Err(VerifyError::PayloadInvalid(format!(
                    "component {name:?} repeats file {}",
                    file.path
                ))
                .into());
            }
            let source = signed_files.get(&key).ok_or_else(|| {
                VerifyError::PayloadInvalid(format!(
                    "v2 archive carries unsigned file {} in component {name}",
                    file.path
                ))
            })?;
            if file.node != source.node
                || file.content != source.content
                || file.component != source.component
            {
                return Err(VerifyError::PayloadInvalid(format!(
                    "v2 file authority mismatch for {} in component {name}",
                    file.path
                ))
                .into());
            }
        }
    }
    if archived_files.len() != signed_files.len() {
        return Err(VerifyError::PayloadInvalid(
            "one or more signed files are missing from component projections".to_string(),
        )
        .into());
    }
    Ok(())
}

fn read_metadata_entry(
    entry: &mut tar::Entry<'_, ArchiveDecoder>,
    path: &str,
    state: &mut MetadataState,
) -> Result<()> {
    require_regular(entry, path)?;
    match path {
        "MANIFEST" => {
            let value = read_metadata_control(entry, state, MAX_CONTROL_SIZE, path)?;
            set_once(&mut state.manifest, value, path)
        }
        "MANIFEST.sig" => {
            let value = read_metadata_utf8(entry, state, MAX_CONTROL_SIZE, path)?;
            set_once(&mut state.signature, value, path)
        }
        "MANIFEST.toml" => {
            let value = read_metadata_control(entry, state, MAX_CONTROL_SIZE, path)?;
            set_once(&mut state.debug_toml, value, path)
        }
        "MANIFEST.attestation.json" => {
            let value = read_metadata_utf8(entry, state, MAX_CONTROL_SIZE, path)?;
            set_once(&mut state.attestation, value, path)
        }
        "MANIFEST.conversion-boundary.json" => {
            let value = read_metadata_utf8(entry, state, MAX_CONTROL_SIZE, path)?;
            set_once(&mut state.conversion_boundary, value, path)
        }
        _ if path.starts_with("components/") && path.ends_with(".json") => {
            let name = path
                .strip_prefix("components/")
                .and_then(|value| value.strip_suffix(".json"))
                .context("invalid CCS component path")?;
            if name.is_empty() || name.contains('/') {
                return Err(VerifyError::PackageError(format!(
                    "invalid CCS component path {path:?}"
                ))
                .into());
            }
            let raw = read_metadata_control(entry, state, MAX_COMPONENT_SIZE, path)?;
            let component: ComponentData =
                serde_json::from_slice(&raw).context("invalid CCS component JSON")?;
            if component.name != name {
                return Err(VerifyError::PackageError(format!(
                    "CCS component path {path:?} disagrees with component name {:?}",
                    component.name
                ))
                .into());
            }
            if state
                .components
                .insert(component.name.clone(), component)
                .is_some()
            {
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
        .take(limit + 1)
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
    label: &str,
) -> Result<Vec<u8>> {
    let declared = entry.header().size()?;
    if declared > limit {
        return Err(
            VerifyError::PackageError(format!("CCS {label} exceeds maximum size {limit}")).into(),
        );
    }
    reserve_metadata_budget(&mut state.bytes_read, declared)?;
    read_control(entry, limit, label)
}

fn read_metadata_utf8(
    entry: &mut tar::Entry<'_, ArchiveDecoder>,
    state: &mut MetadataState,
    limit: u64,
    label: &str,
) -> Result<String> {
    String::from_utf8(read_metadata_control(entry, state, limit, label)?)
        .with_context(|| format!("CCS {label} is not valid UTF-8"))
}

fn reserve_metadata_budget(total: &mut u64, declared: u64) -> Result<()> {
    let next = total
        .checked_add(declared)
        .context("CCS metadata-size arithmetic overflow")?;
    if next > MAX_METADATA_TOTAL_SIZE {
        return Err(VerifyError::PackageError(format!(
            "CCS aggregate metadata exceeds maximum size {MAX_METADATA_TOTAL_SIZE}"
        ))
        .into());
    }
    *total = next;
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
    if path == "components" || path == "objects" {
        return Ok(());
    }
    if let Some(prefix) = path.strip_prefix("objects/")
        && prefix.len() == 2
        && is_lower_hex(prefix)
    {
        return Ok(());
    }
    Err(VerifyError::PackageError(format!("unknown CCS archive directory {path:?}")).into())
}

fn is_lower_hex(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::builder::write_v2_ccs_package;
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
        write_v2_ccs_package(&authority, &payloads, &path, &signer, None, None, None).unwrap();
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
        let bytes = std::fs::read(base).unwrap();
        let truncated = temp.path().join("truncated.ccs");
        let mut output = File::create(&truncated).unwrap();
        output.write_all(&bytes[..bytes.len() / 2]).unwrap();
        assert!(verify_archive(&truncated, &policy).is_err());

        let mut total = MAX_METADATA_TOTAL_SIZE;
        let error = reserve_metadata_budget(&mut total, 1).unwrap_err();
        assert!(format!("{error:#}").contains("aggregate metadata"));
        assert_eq!(total, MAX_METADATA_TOTAL_SIZE);
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
        let mut bytes = std::fs::read(&base).unwrap();
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
