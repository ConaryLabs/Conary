// conary-core/src/ccs/archive_reader.rs

//! Explicitly untrusted CCS v2 archive inspection.
//!
//! This module decodes the current archive grammar and applies structural
//! bounds. It does not establish package trust. Mutation and publication
//! callers must pass the result through `ccs::verify::verify_package`.

use crate::ccs::builder::ComponentData;
use crate::ccs::manifest::CcsManifest;
use crate::ccs::v2::AuthorityDocumentV2;
use anyhow::Context;
use flate2::read::GzDecoder;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use tar::Archive;

const MAX_ENTRY_SIZE: u64 = 512 * 1024 * 1024;
const MAX_TOTAL_EXTRACTION_SIZE: u64 = 4 * 1024 * 1024 * 1024;
const MAX_MANIFEST_SIZE: u64 = 16 * 1024 * 1024;
const MAX_COMPONENT_SIZE: u64 = 64 * 1024 * 1024;

/// Structurally decoded CCS v2 data that has not been authenticated.
///
/// The name is deliberate: possession of this value never authorizes install,
/// update, restore, conversion intake, self-update, or publication.
#[derive(Debug, Clone)]
pub struct UntrustedCcsArchive {
    /// Untrusted compatibility projection for diagnostics and display only.
    pub manifest: CcsManifest,
    /// Parsed current authority. It remains untrusted in this type.
    pub v2_authority: AuthorityDocumentV2,
    /// Exact archived authority bytes used by signature verification.
    pub v2_manifest_raw: Vec<u8>,
    /// Raw build-attestation JSON, when present.
    pub v2_build_attestation_raw: Option<String>,
    /// Raw foreign-conversion-boundary JSON, when present.
    pub v2_foreign_conversion_boundary_raw: Option<String>,
    /// Parsed build attestation, when present.
    pub v2_build_attestation: Option<crate::ccs::attestation::BuildAttestationEnvelope>,
    /// Parsed foreign conversion boundary, when present.
    pub v2_foreign_conversion_boundary: Option<crate::ccs::attestation::ForeignConversionBoundary>,
    /// Raw diagnostic TOML projection, when present.
    pub toml_raw: Option<Vec<u8>>,
    /// Raw package signature JSON. Verification requires this field.
    pub signature_raw: Option<String>,
    /// Untrusted component projections keyed by exact component name.
    pub components: HashMap<String, ComponentData>,
    /// Content objects keyed by their exact lowercase SHA-256 digest.
    pub blobs: HashMap<String, Vec<u8>>,
}

/// Structurally inspect a CCS v2 archive without granting trust.
pub fn inspect_untrusted_ccs_archive<R: Read>(reader: R) -> anyhow::Result<UntrustedCcsArchive> {
    inspect_untrusted_ccs_archive_with_limits(reader, MAX_TOTAL_EXTRACTION_SIZE)
}

/// Identify the exact current CCS v2 archive contract independently of name.
pub fn has_current_ccs_archive_contract(path: impl AsRef<Path>) -> anyhow::Result<bool> {
    let path = path.as_ref();
    let mut file = File::open(path)?;
    let mut magic = [0_u8; 2];
    if file.read(&mut magic)? != magic.len() || magic != [0x1f, 0x8b] {
        return Ok(false);
    }

    let decoder = GzDecoder::new(File::open(path)?).take(MAX_TOTAL_EXTRACTION_SIZE + 1);
    let mut archive = Archive::new(decoder);
    let mut entries_seen = 0_usize;
    for entry in archive.entries()? {
        entries_seen += 1;
        crate::compression::check_archive_entry_limit(entries_seen, "CCS package")?;
        let mut entry = entry?;
        let path = entry.path()?;
        if path != Path::new("MANIFEST") && path != Path::new("./MANIFEST") {
            continue;
        }
        if !entry.header().entry_type().is_file() || entry.size() > MAX_MANIFEST_SIZE {
            return Ok(false);
        }
        let raw = read_entry(&mut entry, MAX_MANIFEST_SIZE, "MANIFEST")?;
        return match cbor_format_version(&raw)? {
            2 => Ok(true),
            1 => anyhow::bail!(
                "CCS v1 archive authority is unsupported; rebuild the package as signed CCS v2"
            ),
            version => {
                anyhow::bail!("unsupported CCS MANIFEST format_version {version}; expected 2")
            }
        };
    }
    Ok(false)
}

fn cbor_format_version(raw: &[u8]) -> anyhow::Result<u64> {
    #[derive(Deserialize)]
    struct Header {
        format_version: u64,
    }

    ciborium::from_reader::<Header, _>(raw)
        .map(|header| header.format_version)
        .map_err(|error| anyhow::anyhow!("invalid CCS CBOR MANIFEST header: {error}"))
}

fn install_manifest_projection(authority: &AuthorityDocumentV2) -> CcsManifest {
    let mut manifest =
        CcsManifest::new_minimal(&authority.identity.name, &authority.identity.version);
    manifest.package.version_scheme = authority.identity.version_scheme;
    manifest.package.release = authority.identity.release.clone();
    manifest.package.kind = authority.identity.kind;
    manifest.requirements = authority.requirements.clone();
    manifest.relations = authority.relations.clone();
    manifest.native_lifecycle = authority.lifecycle.native_lifecycle.clone();
    manifest.package.description = format!(
        "Untrusted inspection projection for CCS v2 {:?}",
        authority.identity.kind
    );
    manifest
}

fn read_entry<R: Read>(entry: &mut R, declared_limit: u64, label: &str) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    entry
        .take(declared_limit + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("read CCS {label}"))?;
    if bytes.len() as u64 > declared_limit {
        anyhow::bail!("CCS {label} exceeds maximum size {declared_limit}");
    }
    Ok(bytes)
}

fn require_regular(entry: &tar::Entry<'_, impl Read>, label: &str) -> anyhow::Result<()> {
    if !entry.header().entry_type().is_file() {
        anyhow::bail!("CCS {label} must be a regular file");
    }
    Ok(())
}

fn canonical_entry_path(entry: &tar::Entry<'_, impl Read>) -> anyhow::Result<String> {
    let path = entry.path()?;
    let path = path
        .to_str()
        .context("CCS archive entry path is not valid UTF-8")?;
    Ok(path.strip_prefix("./").unwrap_or(path).to_string())
}

fn is_lower_hex(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn inspect_untrusted_ccs_archive_with_limits<R: Read>(
    reader: R,
    total_extraction_limit: u64,
) -> anyhow::Result<UntrustedCcsArchive> {
    let decoder = GzDecoder::new(reader).take(total_extraction_limit + 1);
    let mut archive = Archive::new(decoder);
    let mut manifest_raw = None;
    let mut authority = None;
    let mut toml_raw = None;
    let mut signature_raw = None;
    let mut attestation_raw = None;
    let mut boundary_raw = None;
    let mut attestation = None;
    let mut boundary = None;
    let mut components = HashMap::new();
    let mut blobs = HashMap::new();
    let mut total_bytes = 0_u64;
    let mut entries_seen = 0_usize;

    for entry in archive.entries()? {
        entries_seen += 1;
        crate::compression::check_archive_entry_limit(entries_seen, "CCS package")?;
        let mut entry = entry?;
        let entry_size = entry.header().size()?;
        if entry_size > MAX_ENTRY_SIZE {
            anyhow::bail!("CCS archive entry exceeds maximum size: {entry_size} bytes");
        }
        total_bytes = total_bytes
            .checked_add(entry_size)
            .context("CCS archive total extraction size overflow")?;
        if total_bytes > total_extraction_limit {
            anyhow::bail!("CCS archive total extraction size exceeds limit");
        }

        let path = canonical_entry_path(&entry)?;
        if entry.header().entry_type().is_dir() {
            if path.is_empty()
                || path == "."
                || path == "components"
                || path == "objects"
                || path.starts_with("objects/")
            {
                continue;
            }
            anyhow::bail!("unknown CCS archive directory {path:?}");
        }

        match path.as_str() {
            "MANIFEST" => {
                require_regular(&entry, "MANIFEST")?;
                if manifest_raw.is_some() {
                    anyhow::bail!("CCS archive contains duplicate MANIFEST entries");
                }
                let raw = read_entry(&mut entry, MAX_MANIFEST_SIZE, "MANIFEST")?;
                let version = cbor_format_version(&raw)?;
                if version != 2 {
                    if version == 1 {
                        anyhow::bail!(
                            "CCS v1 archive authority is unsupported; rebuild the package as signed CCS v2"
                        );
                    }
                    anyhow::bail!("unsupported CCS MANIFEST format_version {version}; expected 2");
                }
                authority = Some(
                    AuthorityDocumentV2::from_cbor(&raw)
                        .map_err(|error| anyhow::anyhow!("invalid CCS v2 MANIFEST: {error}"))?,
                );
                manifest_raw = Some(raw);
            }
            "MANIFEST.toml" => {
                require_regular(&entry, "MANIFEST.toml")?;
                if toml_raw.is_some() {
                    anyhow::bail!("CCS archive contains duplicate MANIFEST.toml entries");
                }
                toml_raw = Some(read_entry(&mut entry, MAX_MANIFEST_SIZE, "MANIFEST.toml")?);
            }
            "MANIFEST.sig" => {
                require_regular(&entry, "MANIFEST.sig")?;
                if signature_raw.is_some() {
                    anyhow::bail!("CCS archive contains duplicate MANIFEST.sig entries");
                }
                signature_raw = Some(
                    String::from_utf8(read_entry(&mut entry, MAX_MANIFEST_SIZE, "MANIFEST.sig")?)
                        .context("CCS MANIFEST.sig is not valid UTF-8")?,
                );
            }
            "MANIFEST.attestation.json" => {
                require_regular(&entry, "MANIFEST.attestation.json")?;
                if attestation_raw.is_some() {
                    anyhow::bail!(
                        "CCS archive contains duplicate MANIFEST.attestation.json entries"
                    );
                }
                let raw = String::from_utf8(read_entry(
                    &mut entry,
                    MAX_MANIFEST_SIZE,
                    "MANIFEST.attestation.json",
                )?)
                .context("CCS MANIFEST.attestation.json is not valid UTF-8")?;
                attestation = Some(
                    serde_json::from_str(&raw).context("invalid CCS MANIFEST.attestation.json")?,
                );
                attestation_raw = Some(raw);
            }
            "MANIFEST.conversion-boundary.json" => {
                require_regular(&entry, "MANIFEST.conversion-boundary.json")?;
                if boundary_raw.is_some() {
                    anyhow::bail!(
                        "CCS archive contains duplicate MANIFEST.conversion-boundary.json entries"
                    );
                }
                let raw = String::from_utf8(read_entry(
                    &mut entry,
                    MAX_MANIFEST_SIZE,
                    "MANIFEST.conversion-boundary.json",
                )?)
                .context("CCS MANIFEST.conversion-boundary.json is not valid UTF-8")?;
                boundary = Some(
                    serde_json::from_str(&raw)
                        .context("invalid CCS MANIFEST.conversion-boundary.json")?,
                );
                boundary_raw = Some(raw);
            }
            _ if path.starts_with("components/") && path.ends_with(".json") => {
                require_regular(&entry, "component")?;
                let name = path
                    .strip_prefix("components/")
                    .and_then(|path| path.strip_suffix(".json"))
                    .context("invalid CCS component path")?;
                if name.is_empty() || name.contains('/') {
                    anyhow::bail!("invalid CCS component path {path:?}");
                }
                let raw = read_entry(&mut entry, MAX_COMPONENT_SIZE, "component")?;
                let component: ComponentData =
                    serde_json::from_slice(&raw).context("invalid CCS component JSON")?;
                if component.name != name {
                    anyhow::bail!(
                        "CCS component path {path:?} disagrees with component name {:?}",
                        component.name
                    );
                }
                if components
                    .insert(component.name.clone(), component)
                    .is_some()
                {
                    anyhow::bail!("CCS archive contains duplicate component {name:?}");
                }
            }
            _ if path.starts_with("objects/") => {
                require_regular(&entry, "object")?;
                let object = path
                    .strip_prefix("objects/")
                    .context("invalid CCS object path")?;
                let Some((prefix, suffix)) = object.split_once('/') else {
                    anyhow::bail!("invalid CCS object path {object:?}");
                };
                if prefix.len() != 2
                    || suffix.len() != 62
                    || !is_lower_hex(prefix)
                    || !is_lower_hex(suffix)
                {
                    anyhow::bail!("invalid canonical CCS SHA-256 object path {object:?}");
                }
                let hash = format!("{prefix}{suffix}");
                let bytes = read_entry(&mut entry, MAX_ENTRY_SIZE, "object")?;
                let actual = crate::hash::sha256(&bytes);
                if actual != hash {
                    anyhow::bail!("CCS object path hash mismatch: expected {hash}, got {actual}");
                }
                if blobs.insert(hash.clone(), bytes).is_some() {
                    anyhow::bail!("CCS archive contains duplicate object {hash}");
                }
            }
            _ => anyhow::bail!("unknown CCS archive entry {path:?}"),
        }
    }

    let authority =
        authority.context("CCS package is missing required current v2 MANIFEST authority")?;
    let manifest_raw =
        manifest_raw.context("CCS package is missing required current v2 MANIFEST bytes")?;

    if let Some(expected) = &authority.provenance.foreign_conversion_boundary_hash {
        let boundary = boundary.as_ref().context(
            "v2 foreign conversion boundary hash present but MANIFEST.conversion-boundary.json is missing",
        )?;
        let actual = crate::ccs::attestation::canonical_json_hash(boundary)?;
        if &actual != expected {
            anyhow::bail!(
                "v2 foreign conversion boundary hash mismatch: expected {expected}, got {actual}"
            );
        }
    }

    Ok(UntrustedCcsArchive {
        manifest: install_manifest_projection(&authority),
        v2_authority: authority,
        v2_manifest_raw: manifest_raw,
        v2_build_attestation_raw: attestation_raw,
        v2_foreign_conversion_boundary_raw: boundary_raw,
        v2_build_attestation: attestation,
        v2_foreign_conversion_boundary: boundary,
        toml_raw,
        signature_raw,
        components,
        blobs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::builder::write_v2_ccs_package;
    use crate::ccs::signing::SigningKeyPair;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Write;
    use tar::Builder;

    fn current_package() -> (tempfile::TempDir, std::path::PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("current.ccs");
        let authority = crate::ccs::v2::test_support::package_authority_with_one_file("current");
        let payloads = crate::ccs::v2::test_support::one_file_payloads_for_tests();
        write_v2_ccs_package(
            &authority,
            &payloads,
            &path,
            &SigningKeyPair::generate(),
            None,
            None,
            None,
        )
        .unwrap();
        (temp, path)
    }

    fn append<W: Write>(builder: &mut Builder<W>, path: &str, bytes: &[u8]) {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, path, bytes).unwrap();
    }

    #[test]
    fn inspection_is_explicitly_untrusted_and_v2_only() {
        let (_temp, path) = current_package();
        let archive = inspect_untrusted_ccs_archive(File::open(&path).unwrap()).unwrap();

        assert_eq!(archive.v2_authority.identity.name, "current");
        assert_eq!(archive.manifest.package.name, "current");
        assert!(archive.signature_raw.is_some());
        assert!(!archive.blobs.is_empty());
        assert!(has_current_ccs_archive_contract(path).unwrap());
    }

    #[test]
    fn current_contract_detection_rejects_v1() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("v1.ccs");
        let mut manifest = Vec::new();
        ciborium::into_writer(
            &serde_json::json!({
                "format_version": 1,
                "name": "retired-v1-fixture",
            }),
            &mut manifest,
        )
        .unwrap();
        {
            let output = std::fs::File::create(&path).unwrap();
            let encoder = GzEncoder::new(output, Compression::default());
            let mut builder = Builder::new(encoder);
            append(&mut builder, "MANIFEST", &manifest);
            builder.into_inner().unwrap().finish().unwrap();
        }

        let error = inspect_untrusted_ccs_archive(File::open(&path).unwrap()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("CCS v1 archive authority is unsupported")
        );
        let error = has_current_ccs_archive_contract(path).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("CCS v1 archive authority is unsupported")
        );
    }

    #[test]
    fn inspection_rejects_noncanonical_object_paths() {
        let authority = crate::ccs::v2::test_support::package_authority_with_one_file("bad-object");
        let raw = authority.to_cbor().unwrap();
        let mut bytes = Vec::new();
        {
            let encoder = GzEncoder::new(&mut bytes, Compression::default());
            let mut builder = Builder::new(encoder);
            append(&mut builder, "MANIFEST", &raw);
            append(&mut builder, "objects/not-a-digest", b"payload");
            builder.into_inner().unwrap().finish().unwrap();
        }

        let error = inspect_untrusted_ccs_archive(std::io::Cursor::new(bytes)).unwrap_err();
        assert!(error.to_string().contains("invalid CCS object path"));
    }

    #[test]
    fn inspection_rejects_duplicate_manifest_authority() {
        let authority = crate::ccs::v2::test_support::package_authority_with_one_file("duplicate");
        let raw = authority.to_cbor().unwrap();
        let mut bytes = Vec::new();
        {
            let encoder = GzEncoder::new(&mut bytes, Compression::default());
            let mut builder = Builder::new(encoder);
            append(&mut builder, "MANIFEST", &raw);
            append(&mut builder, "./MANIFEST", &raw);
            builder.into_inner().unwrap().finish().unwrap();
        }

        let error = inspect_untrusted_ccs_archive(std::io::Cursor::new(bytes)).unwrap_err();
        assert!(error.to_string().contains("duplicate MANIFEST"));
    }

    #[test]
    fn inspection_enforces_cumulative_extraction_limit() {
        let (_temp, path) = current_package();
        let error =
            inspect_untrusted_ccs_archive_with_limits(File::open(path).unwrap(), 32).unwrap_err();
        assert!(
            error.to_string().contains("extraction size")
                || error.to_string().contains("unexpected end of file")
                || error.to_string().contains("failed to read")
        );
    }
}
