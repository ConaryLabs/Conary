// conary-core/src/ccs/archive_reader.rs

//! Explicitly untrusted CCS v3 archive inspection.
//!
//! This module decodes the current archive grammar and applies the canonical
//! structural budget in `crate::ccs::budget`. It owns no limits of its own, so
//! anything the authoring writer emits is admissible here. It does not
//! establish package trust: mutation and publication callers must pass the
//! result through `ccs::verify::verify_package`.

use crate::ccs::archive_layout::is_lower_hex;
use crate::ccs::budget::{AuthorityCensus, BudgetDimension, CCS_BUDGET};
use crate::ccs::builder::ComponentData;
use crate::ccs::manifest::CcsManifest;
use crate::ccs::v3::AuthorityDocumentV3;
use anyhow::Context;
use flate2::read::GzDecoder;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use tar::Archive;

/// Structurally decoded CCS v3 data that has not been authenticated.
///
/// The name is deliberate: possession of this value never authorizes install,
/// update, restore, conversion intake, self-update, or publication.
#[derive(Debug, Clone)]
pub struct UntrustedCcsArchive {
    /// Untrusted compatibility projection for diagnostics and display only.
    pub manifest: CcsManifest,
    /// Parsed current authority. It remains untrusted in this type.
    pub v3_authority: AuthorityDocumentV3,
    /// Exact archived authority bytes used by signature verification.
    pub v3_manifest_raw: Vec<u8>,
    /// Structural census the shared budget measured for this authority.
    pub census: AuthorityCensus,
    /// Raw build-attestation JSON, when present.
    pub v3_build_attestation_raw: Option<String>,
    /// Raw foreign-conversion-boundary JSON, when present.
    pub v3_foreign_conversion_boundary_raw: Option<String>,
    /// Parsed build attestation, when present.
    pub v3_build_attestation: Option<crate::ccs::attestation::BuildAttestationEnvelope>,
    /// Parsed foreign conversion boundary, when present.
    pub v3_foreign_conversion_boundary: Option<crate::ccs::attestation::ForeignConversionBoundary>,
    /// Raw diagnostic TOML projection, when present.
    pub toml_raw: Option<Vec<u8>>,
    /// Raw package signature JSON. Verification requires this field.
    pub signature_raw: Option<String>,
    /// Component view derived from the decoded authority.
    pub components: HashMap<String, ComponentData>,
}

/// Structurally inspect a CCS v3 archive without granting trust.
pub fn inspect_untrusted_ccs_archive<R: Read>(reader: R) -> anyhow::Result<UntrustedCcsArchive> {
    let mut archive = Archive::new(GzDecoder::new(reader));
    let mut state = InspectionState::default();
    let mut entries_seen = 0_u64;

    for entry in archive.entries()? {
        entries_seen += 1;
        CCS_BUDGET.admit_archive_entry(entries_seen)?;
        let mut entry = entry?;
        let path = canonical_entry_path(&entry)?;

        if entry.header().entry_type().is_dir() {
            require_known_directory(&path)?;
            continue;
        }
        state.read_regular_entry(&mut entry, &path)?;
    }

    state.finish()
}

/// Identify the exact current CCS v3 archive contract independently of name.
pub fn has_current_ccs_archive_contract(path: impl AsRef<Path>) -> anyhow::Result<bool> {
    let path = path.as_ref();
    let mut file = File::open(path)?;
    let mut magic = [0_u8; 2];
    if file.read(&mut magic)? != magic.len() || magic != [0x1f, 0x8b] {
        return Ok(false);
    }

    let mut archive = Archive::new(GzDecoder::new(File::open(path)?));
    let mut entries_seen = 0_u64;
    for entry in archive.entries()? {
        entries_seen += 1;
        CCS_BUDGET.admit_archive_entry(entries_seen)?;
        let mut entry = entry?;
        if canonical_entry_path(&entry)? != "MANIFEST" {
            continue;
        }
        if !entry.header().entry_type().is_file() {
            return Ok(false);
        }
        if entry.header().size()? > CCS_BUDGET.max_authority_bytes() {
            return Ok(false);
        }
        let raw = read_bounded(
            &mut entry,
            CCS_BUDGET.max_authority_bytes(),
            BudgetDimension::AuthorityBytes,
            "MANIFEST",
        )?;
        return require_format_version_3(&raw).map(|()| true);
    }
    Ok(false)
}

#[derive(Default)]
struct InspectionState {
    authority: Option<AuthorityDocumentV3>,
    census: AuthorityCensus,
    manifest_raw: Option<Vec<u8>>,
    toml_raw: Option<Vec<u8>>,
    signature_raw: Option<String>,
    attestation_raw: Option<String>,
    boundary_raw: Option<String>,
    attestation: Option<crate::ccs::attestation::BuildAttestationEnvelope>,
    boundary: Option<crate::ccs::attestation::ForeignConversionBoundary>,
    metadata_bytes: u64,
    payload_bytes: u64,
    payload_objects: u64,
    objects_seen: std::collections::BTreeSet<String>,
}

impl InspectionState {
    /// Read one non-directory entry.
    ///
    /// `MANIFEST` must arrive before any other archived file, so every later
    /// ceiling is derived from this package's own signed census rather than
    /// from a global guess.
    fn read_regular_entry<R: Read>(
        &mut self,
        entry: &mut tar::Entry<'_, R>,
        path: &str,
    ) -> anyhow::Result<()> {
        require_regular(entry, path)?;
        if path == "MANIFEST" {
            return self.read_authority(entry);
        }
        let census = self.census_or_bail()?;

        match path {
            "MANIFEST.toml" => {
                let ceiling = CCS_BUDGET.debug_projection_bytes_ceiling(&census)?;
                let raw = self.read_metadata(
                    entry,
                    ceiling,
                    BudgetDimension::DebugProjectionBytes,
                    "MANIFEST.toml",
                )?;
                set_once(&mut self.toml_raw, raw, "MANIFEST.toml")
            }
            "MANIFEST.sig" => {
                let raw = self.read_metadata(
                    entry,
                    CCS_BUDGET.signature_bytes_ceiling(),
                    BudgetDimension::SignatureBytes,
                    "MANIFEST.sig",
                )?;
                let raw = String::from_utf8(raw).context("CCS MANIFEST.sig is not valid UTF-8")?;
                set_once(&mut self.signature_raw, raw, "MANIFEST.sig")
            }
            "MANIFEST.attestation.json" => {
                let raw = self.read_metadata(
                    entry,
                    CCS_BUDGET.attestation_bytes_ceiling(),
                    BudgetDimension::AttestationBytes,
                    "MANIFEST.attestation.json",
                )?;
                let raw = String::from_utf8(raw)
                    .context("CCS MANIFEST.attestation.json is not valid UTF-8")?;
                self.attestation = Some(
                    serde_json::from_str(&raw).context("invalid CCS MANIFEST.attestation.json")?,
                );
                set_once(&mut self.attestation_raw, raw, "MANIFEST.attestation.json")
            }
            "MANIFEST.conversion-boundary.json" => {
                let raw = self.read_metadata(
                    entry,
                    CCS_BUDGET.attestation_bytes_ceiling(),
                    BudgetDimension::AttestationBytes,
                    "MANIFEST.conversion-boundary.json",
                )?;
                let raw = String::from_utf8(raw)
                    .context("CCS MANIFEST.conversion-boundary.json is not valid UTF-8")?;
                self.boundary = Some(
                    serde_json::from_str(&raw)
                        .context("invalid CCS MANIFEST.conversion-boundary.json")?,
                );
                set_once(
                    &mut self.boundary_raw,
                    raw,
                    "MANIFEST.conversion-boundary.json",
                )
            }
            _ if path.starts_with("objects/") => self.read_object(entry, path, &census),
            _ => anyhow::bail!("unknown CCS archive entry {path:?}"),
        }
    }

    fn read_authority<R: Read>(&mut self, entry: &mut tar::Entry<'_, R>) -> anyhow::Result<()> {
        if self.manifest_raw.is_some() {
            anyhow::bail!("CCS archive contains duplicate MANIFEST entries");
        }
        let ceiling = CCS_BUDGET.max_authority_bytes();
        CCS_BUDGET.admit_control_bytes(
            BudgetDimension::AuthorityBytes,
            "MANIFEST",
            entry.header().size()?,
            ceiling,
        )?;
        let raw = read_bounded(entry, ceiling, BudgetDimension::AuthorityBytes, "MANIFEST")?;
        require_format_version_3(&raw)?;
        let authority = CCS_BUDGET.decode_authority(&raw)?;
        let census = crate::ccs::v3::authority_census(&authority)
            .map_err(|error| anyhow::anyhow!("invalid CCS v3 MANIFEST: {error}"))?;
        CCS_BUDGET.admit_encoded_authority(&census, raw.len() as u64)?;
        self.metadata_bytes = raw.len() as u64;
        self.census = census;
        self.authority = Some(authority);
        self.manifest_raw = Some(raw);
        Ok(())
    }

    fn read_object<R: Read>(
        &mut self,
        entry: &mut tar::Entry<'_, R>,
        path: &str,
        census: &AuthorityCensus,
    ) -> anyhow::Result<()> {
        let hash = canonical_object_hash(path)?;
        let declared = entry.header().size()?;
        CCS_BUDGET.admit_control_bytes(
            BudgetDimension::PayloadObjectBytes,
            path,
            declared,
            CCS_BUDGET.max_payload_object_bytes,
        )?;
        self.payload_objects += 1;
        CCS_BUDGET.admit_control_bytes(
            BudgetDimension::PayloadObjectCount,
            "objects",
            self.payload_objects,
            census.payload_objects,
        )?;
        self.payload_bytes = self
            .payload_bytes
            .checked_add(declared)
            .context("CCS payload size arithmetic overflow")?;
        CCS_BUDGET.admit_control_bytes(
            BudgetDimension::TotalPayloadBytes,
            "objects",
            self.payload_bytes,
            census.payload_bytes,
        )?;
        if !self.objects_seen.insert(hash.clone()) {
            anyhow::bail!("CCS archive contains duplicate object {hash}");
        }

        // Hash the object as it streams. Untrusted inspection never retains
        // payload bytes, so it introduces no whole-package buffering.
        let mut hasher = crate::hash::Hasher::new(crate::hash::HashAlgorithm::Sha256);
        let mut buffer = vec![0_u8; 64 * 1024];
        let mut read_bytes = 0_u64;
        loop {
            let read = entry.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            read_bytes += read as u64;
            hasher.update(&buffer[..read]);
        }
        if read_bytes != declared {
            anyhow::bail!("CCS object {hash} declares {declared} bytes but carries {read_bytes}");
        }
        let actual = hasher.finalize().value;
        if actual != hash {
            anyhow::bail!("CCS object path hash mismatch: expected {hash}, got {actual}");
        }
        Ok(())
    }

    fn read_metadata<R: Read>(
        &mut self,
        entry: &mut tar::Entry<'_, R>,
        ceiling: u64,
        dimension: BudgetDimension,
        label: &str,
    ) -> anyhow::Result<Vec<u8>> {
        CCS_BUDGET.admit_control_bytes(dimension, label, entry.header().size()?, ceiling)?;
        let raw = read_bounded(entry, ceiling, dimension, label)?;
        self.metadata_bytes = self
            .metadata_bytes
            .checked_add(raw.len() as u64)
            .context("CCS metadata-size arithmetic overflow")?;
        CCS_BUDGET.admit_control_bytes(
            BudgetDimension::MetadataBytes,
            "control documents",
            self.metadata_bytes,
            CCS_BUDGET.metadata_bytes_ceiling(&self.census)?,
        )?;
        Ok(raw)
    }

    fn census_or_bail(&self) -> anyhow::Result<AuthorityCensus> {
        if self.authority.is_none() {
            anyhow::bail!(
                "CCS archive must carry its MANIFEST authority before every other archived file"
            );
        }
        Ok(self.census.clone())
    }

    fn finish(self) -> anyhow::Result<UntrustedCcsArchive> {
        let authority = self
            .authority
            .context("CCS package is missing required current v3 MANIFEST authority")?;
        let manifest_raw = self
            .manifest_raw
            .context("CCS package is missing required current v3 MANIFEST bytes")?;

        if let Some(expected) = &authority.provenance.foreign_conversion_boundary_hash {
            let boundary = self.boundary.as_ref().context(
                "v3 foreign conversion boundary hash present but MANIFEST.conversion-boundary.json is missing",
            )?;
            let actual = crate::ccs::attestation::canonical_json_hash(boundary)?;
            if &actual != expected {
                anyhow::bail!(
                    "v3 foreign conversion boundary hash mismatch: expected {expected}, got {actual}"
                );
            }
        }

        Ok(UntrustedCcsArchive {
            manifest: untrusted_manifest_projection(&authority),
            components: crate::ccs::v3::component_view::components(&authority),
            census: self.census,
            v3_authority: authority,
            v3_manifest_raw: manifest_raw,
            v3_build_attestation_raw: self.attestation_raw,
            v3_foreign_conversion_boundary_raw: self.boundary_raw,
            v3_build_attestation: self.attestation,
            v3_foreign_conversion_boundary: self.boundary,
            toml_raw: self.toml_raw,
            signature_raw: self.signature_raw,
        })
    }
}

fn require_format_version_3(raw: &[u8]) -> anyhow::Result<()> {
    #[derive(Deserialize)]
    struct Header {
        format_version: u64,
    }

    let header: Header =
        ciborium::de::from_reader_with_recursion_limit(raw, CCS_BUDGET.max_cbor_nesting_depth)
            .map_err(|error| anyhow::anyhow!("invalid CCS CBOR MANIFEST header: {error}"))?;
    match header.format_version {
        3 => Ok(()),
        1 | 2 => anyhow::bail!(
            "CCS v{} archive authority is unsupported; rebuild the package as signed CCS v3",
            header.format_version
        ),
        version => anyhow::bail!("unsupported CCS MANIFEST format_version {version}; expected 3"),
    }
}

fn untrusted_manifest_projection(authority: &AuthorityDocumentV3) -> CcsManifest {
    let mut manifest = crate::ccs::v3::project_manifest_identity(
        authority,
        format!(
            "Untrusted inspection projection for CCS v3 {:?}",
            authority.identity.kind
        ),
    );
    manifest.requirements = authority.requirements.clone();
    manifest.relations = authority.relations.clone();
    manifest.native_lifecycle = authority.lifecycle.native_lifecycle.clone();
    manifest
}

fn read_bounded<R: Read>(
    entry: &mut R,
    ceiling: u64,
    dimension: BudgetDimension,
    label: &str,
) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    entry
        .take(ceiling.saturating_add(1))
        .read_to_end(&mut bytes)
        .with_context(|| format!("read CCS {label}"))?;
    CCS_BUDGET.admit_control_bytes(dimension, label, bytes.len() as u64, ceiling)?;
    Ok(bytes)
}

fn set_once<T>(slot: &mut Option<T>, value: T, label: &str) -> anyhow::Result<()> {
    if slot.replace(value).is_some() {
        anyhow::bail!("CCS archive contains duplicate {label} entries");
    }
    Ok(())
}

fn require_regular<R: Read>(entry: &tar::Entry<'_, R>, label: &str) -> anyhow::Result<()> {
    if !entry.header().entry_type().is_file() {
        anyhow::bail!("CCS {label} must be a regular file");
    }
    Ok(())
}

fn require_known_directory(path: &str) -> anyhow::Result<()> {
    if crate::ccs::archive_layout::is_known_directory(path) {
        return Ok(());
    }
    anyhow::bail!("unknown CCS archive directory {path:?}")
}

fn canonical_object_hash(path: &str) -> anyhow::Result<String> {
    let object = path
        .strip_prefix("objects/")
        .context("invalid CCS object path")?;
    let Some((prefix, suffix)) = object.split_once('/') else {
        anyhow::bail!("invalid CCS object path {object:?}");
    };
    if prefix.len() != 2 || suffix.len() != 62 || !is_lower_hex(prefix) || !is_lower_hex(suffix) {
        anyhow::bail!("invalid canonical CCS SHA-256 object path {object:?}");
    }
    Ok(format!("{prefix}{suffix}"))
}

fn canonical_entry_path<R: Read>(entry: &tar::Entry<'_, R>) -> anyhow::Result<String> {
    let path = entry.path()?;
    let path = path
        .to_str()
        .context("CCS archive entry path is not valid UTF-8")?;
    Ok(path.strip_prefix("./").unwrap_or(path).to_string())
}

#[cfg(test)]
#[path = "archive_reader/tests.rs"]
mod tests;
