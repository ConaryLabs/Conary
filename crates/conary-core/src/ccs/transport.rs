// conary-core/src/ccs/transport.rs

//! Versioned repository transport for signed CCS control documents and objects.
//!
//! A transport envelope never invents payload authority. It carries the exact
//! signed control documents from a verified CCS archive plus the canonical,
//! sorted object set derived from that authority. Repository clients
//! authenticate the controls before fetching any object and bind downloaded or
//! already trusted CAS objects back to the same signed set before exposing a
//! package capability.

use crate::ccs::budget::{BudgetDimension, CCS_BUDGET};
use crate::ccs::verify::{RawControlDocuments, TrustPolicy, VerifiedCcsArchive, VerifyError};
use crate::filesystem::{CasStore, VerifiedObjectSet};
use crate::packages::payload::ReopenablePayload;
use anyhow::{Context, Result};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

pub const CCS_TRANSPORT_SCHEMA_V1: u32 = 1;
const TRANSPORT_ARCHIVE_MTIME: u64 = 1_704_067_200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CcsTransportObjectV1 {
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CcsTransportEnvelopeV1 {
    pub schema_version: u32,
    pub manifest_base64: String,
    pub signature_json: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debug_toml_base64: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_attestation_json: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub foreign_conversion_boundary_json: Option<String>,
    pub objects: Vec<CcsTransportObjectV1>,
}

#[derive(Debug)]
pub struct PreparedCcsTransport {
    authority: crate::ccs::v3::AuthorityDocumentV3,
    signature: crate::ccs::verify::PackageSignature,
    build_attestation: Option<crate::ccs::attestation::BuildAttestationEnvelope>,
    foreign_conversion_boundary: Option<crate::ccs::attestation::ForeignConversionBoundary>,
    debug_toml: Option<Vec<u8>>,
    expected_objects: BTreeMap<String, u64>,
    files_checked: usize,
    raw: RawControlDocuments,
}

impl CcsTransportEnvelopeV1 {
    pub fn from_verified_archive(verified: &VerifiedCcsArchive) -> Result<Self> {
        let (objects, _) = super::verify::content::expected_objects(verified.authority())?;
        let raw = verified.raw_control_documents();
        Ok(Self {
            schema_version: CCS_TRANSPORT_SCHEMA_V1,
            manifest_base64: BASE64.encode(&raw.manifest),
            signature_json: raw.signature.clone(),
            debug_toml_base64: raw.debug_toml.as_ref().map(|value| BASE64.encode(value)),
            build_attestation_json: raw.build_attestation.clone(),
            foreign_conversion_boundary_json: raw.foreign_conversion_boundary.clone(),
            objects: objects
                .into_iter()
                .map(|(sha256, size)| CcsTransportObjectV1 { sha256, size })
                .collect(),
        })
    }

    pub fn authenticate(&self, policy: &TrustPolicy) -> Result<PreparedCcsTransport> {
        if self.schema_version != CCS_TRANSPORT_SCHEMA_V1 {
            return Err(VerifyError::PackageError(format!(
                "unknown CCS repository transport schema {}",
                self.schema_version
            ))
            .into());
        }

        let manifest = decode_bounded_base64(
            &self.manifest_base64,
            CCS_BUDGET.max_authority_bytes(),
            "transport MANIFEST",
        )?;
        let authority = CCS_BUDGET.decode_authority(&manifest)?;
        let census = crate::ccs::v3::authority_census(&authority).map_err(|error| {
            VerifyError::PackageError(format!("invalid CCS v3 MANIFEST: {error}"))
        })?;
        CCS_BUDGET.admit_encoded_authority(&census, manifest.len() as u64)?;

        let debug_toml = self
            .debug_toml_base64
            .as_deref()
            .map(|value| {
                decode_bounded_base64(
                    value,
                    CCS_BUDGET.debug_projection_bytes_ceiling(&census)?,
                    "transport MANIFEST.toml",
                )
            })
            .transpose()?;
        CCS_BUDGET.admit_control_bytes(
            BudgetDimension::SignatureBytes,
            "transport MANIFEST.sig",
            self.signature_json.len() as u64,
            CCS_BUDGET.signature_bytes_ceiling(),
        )?;
        for (label, value) in [
            (
                "transport MANIFEST.attestation.json",
                self.build_attestation_json.as_deref(),
            ),
            (
                "transport MANIFEST.conversion-boundary.json",
                self.foreign_conversion_boundary_json.as_deref(),
            ),
        ] {
            if let Some(value) = value {
                CCS_BUDGET.admit_control_bytes(
                    BudgetDimension::AttestationBytes,
                    label,
                    value.len() as u64,
                    CCS_BUDGET.attestation_bytes_ceiling(),
                )?;
            }
        }
        let metadata_bytes = [
            manifest.len() as u64,
            self.signature_json.len() as u64,
            debug_toml.as_ref().map_or(0, |value| value.len() as u64),
            self.build_attestation_json
                .as_ref()
                .map_or(0, |value| value.len() as u64),
            self.foreign_conversion_boundary_json
                .as_ref()
                .map_or(0, |value| value.len() as u64),
        ]
        .into_iter()
        .try_fold(0_u64, |total, size| total.checked_add(size))
        .context("CCS transport metadata-size arithmetic overflow")?;
        CCS_BUDGET.admit_control_bytes(
            BudgetDimension::MetadataBytes,
            "transport control documents",
            metadata_bytes,
            CCS_BUDGET.metadata_bytes_ceiling(&census)?,
        )?;

        let verified = crate::ccs::v3::read_authority_document(
            &manifest,
            Some(&self.signature_json),
            debug_toml.as_deref(),
            self.build_attestation_json.as_deref(),
            self.foreign_conversion_boundary_json.as_deref(),
            policy,
        )?;
        let (expected_objects, files_checked) =
            super::verify::content::expected_objects(&verified.authority)?;
        let declared = self
            .objects
            .iter()
            .map(|object| (object.sha256.clone(), object.size))
            .collect::<Vec<_>>();
        let expected = expected_objects
            .iter()
            .map(|(sha256, size)| (sha256.clone(), *size))
            .collect::<Vec<_>>();
        if declared != expected {
            return Err(VerifyError::PayloadInvalid(format!(
                "transport object sequence {declared:?} disagrees with signed canonical set {expected:?}"
            ))
            .into());
        }

        let raw = RawControlDocuments {
            manifest,
            signature: self.signature_json.clone(),
            debug_toml: debug_toml.clone(),
            build_attestation: self.build_attestation_json.clone(),
            foreign_conversion_boundary: self.foreign_conversion_boundary_json.clone(),
        };
        Ok(PreparedCcsTransport {
            authority: verified.authority,
            signature: verified.signature,
            build_attestation: verified.build_attestation,
            foreign_conversion_boundary: verified.foreign_conversion_boundary,
            debug_toml,
            expected_objects,
            files_checked,
            raw,
        })
    }
}

impl PreparedCcsTransport {
    pub fn package_name(&self) -> &str {
        &self.authority.identity.name
    }

    pub fn package_version(&self) -> &str {
        &self.authority.identity.version
    }

    pub fn expected_objects(&self) -> impl ExactSizeIterator<Item = (&str, u64)> {
        self.expected_objects
            .iter()
            .map(|(sha256, size)| (sha256.as_str(), *size))
    }

    pub fn verify_object_set(self, set: VerifiedObjectSet) -> Result<VerifiedCcsArchive> {
        let actual = set
            .objects()
            .map(|(sha256, size)| (sha256.to_string(), size))
            .collect::<BTreeMap<_, _>>();
        if actual != self.expected_objects {
            return Err(VerifyError::PayloadInvalid(format!(
                "verified CAS object set {actual:?} disagrees with signed set {:?}",
                self.expected_objects
            ))
            .into());
        }
        let metrics = set.metrics();
        let set = Arc::new(set);
        let object_sources = self
            .expected_objects
            .iter()
            .map(|(sha256, size)| {
                ReopenablePayload::from_verified_cas_object(Arc::clone(&set), sha256.clone(), *size)
                    .map(|source| (sha256.clone(), source))
            })
            .collect::<crate::Result<HashMap<_, _>>>()?;
        super::verify::content::archive_from_parts(super::verify::content::VerifiedArchiveParts {
            authority: self.authority,
            signature: self.signature,
            build_attestation: self.build_attestation,
            foreign_conversion_boundary: self.foreign_conversion_boundary,
            debug_toml: self.debug_toml,
            object_sources,
            verified_object_metrics: Some(metrics),
            files_checked: self.files_checked,
            raw: self.raw,
        })
    }
}

/// Copy one already verified archive object set into a durable CAS batch.
pub fn persist_verified_archive_objects(
    verified: &VerifiedCcsArchive,
    cas: &CasStore,
) -> Result<VerifiedObjectSet> {
    let descriptor = CcsTransportEnvelopeV1::from_verified_archive(verified)?;
    let mut batch = cas.verified_object_batch(
        descriptor
            .objects
            .iter()
            .map(|object| (object.sha256.clone(), object.size)),
    )?;
    for object in &descriptor.objects {
        if batch.reuse_trusted(&object.sha256)? {
            continue;
        }
        let source = verified
            .object_sources()
            .get(&object.sha256)
            .ok_or_else(|| {
                VerifyError::PayloadInvalid(format!(
                    "verified archive lost object source {}",
                    object.sha256
                ))
            })?;
        batch.ingest(&object.sha256, source.open()?.as_mut())?;
    }
    Ok(batch.commit()?)
}

pub fn write_verified_transport_archive(
    verified: &VerifiedCcsArchive,
    output_path: &Path,
) -> Result<()> {
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use tar::{Builder, EntryType, Header};

    let parent = output_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let mut staged = tempfile::Builder::new()
        .prefix(".conary-ccs-transport-")
        .tempfile_in(parent)?;
    {
        let encoder = GzEncoder::new(staged.as_file_mut(), Compression::default());
        let mut archive = Builder::new(encoder);
        let mut directory = Header::new_gnu();
        directory.set_entry_type(EntryType::Directory);
        directory.set_mode(0o755);
        directory.set_size(0);
        directory.set_mtime(TRANSPORT_ARCHIVE_MTIME);
        directory.set_cksum();
        archive.append_data(&mut directory, "objects", std::io::empty())?;

        let raw = verified.raw_control_documents();
        let mut controls: BTreeMap<&str, &[u8]> = BTreeMap::new();
        controls.insert("MANIFEST", &raw.manifest);
        if let Some(value) = raw.build_attestation.as_deref() {
            controls.insert("MANIFEST.attestation.json", value.as_bytes());
        }
        if let Some(value) = raw.foreign_conversion_boundary.as_deref() {
            controls.insert("MANIFEST.conversion-boundary.json", value.as_bytes());
        }
        controls.insert("MANIFEST.sig", raw.signature.as_bytes());
        if let Some(value) = raw.debug_toml.as_deref() {
            controls.insert("MANIFEST.toml", value);
        }
        for (path, bytes) in controls {
            append_bytes(&mut archive, path, bytes)?;
        }

        let (expected_objects, _) = super::verify::content::expected_objects(verified.authority())?;
        let mut objects = verified.object_sources().iter().collect::<Vec<_>>();
        objects.sort_by_key(|(sha256, _)| *sha256);
        for (sha256, source) in objects {
            let path = format!("objects/{}/{}", &sha256[..2], &sha256[2..]);
            let mut reader = source.open()?;
            let size = expected_objects
                .get(sha256)
                .copied()
                .context("verified transport object is absent from signed authority")?;
            let mut header = Header::new_gnu();
            header.set_entry_type(EntryType::Regular);
            header.set_mode(0o644);
            header.set_size(size);
            header.set_mtime(TRANSPORT_ARCHIVE_MTIME);
            header.set_cksum();
            archive.append_data(&mut header, path, &mut reader)?;
        }
        archive.into_inner()?.finish()?;
    }
    staged.as_file_mut().flush()?;
    staged.persist(output_path).map_err(|error| error.error)?;
    Ok(())
}

fn append_bytes<W: Write>(archive: &mut tar::Builder<W>, path: &str, bytes: &[u8]) -> Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Regular);
    header.set_mode(0o644);
    header.set_size(bytes.len() as u64);
    header.set_mtime(TRANSPORT_ARCHIVE_MTIME);
    header.set_cksum();
    archive.append_data(&mut header, path, bytes)?;
    Ok(())
}

fn decode_bounded_base64(encoded: &str, decoded_limit: u64, label: &str) -> Result<Vec<u8>> {
    let encoded_limit = decoded_limit
        .checked_add(2)
        .and_then(|value| value.checked_div(3))
        .and_then(|value| value.checked_mul(4))
        .context("CCS transport base64 ceiling overflow")?;
    if encoded.len() as u64 > encoded_limit {
        return Err(VerifyError::PackageError(format!(
            "CCS {label} exceeds encoded size ceiling {encoded_limit}"
        ))
        .into());
    }
    let decoded = BASE64
        .decode(encoded)
        .with_context(|| format!("decode CCS {label} base64"))?;
    if decoded.len() as u64 > decoded_limit {
        return Err(VerifyError::PackageError(format!(
            "CCS {label} exceeds decoded size ceiling {decoded_limit}"
        ))
        .into());
    }
    Ok(decoded)
}

#[cfg(test)]
pub(crate) fn test_transport(hashes: &[String]) -> CcsTransportEnvelopeV1 {
    CcsTransportEnvelopeV1 {
        schema_version: CCS_TRANSPORT_SCHEMA_V1,
        manifest_base64: String::new(),
        signature_json: "{}".to_string(),
        debug_toml_base64: None,
        build_attestation_json: None,
        foreign_conversion_boundary_json: None,
        objects: hashes
            .iter()
            .map(|sha256| CcsTransportObjectV1 {
                sha256: sha256.clone(),
                size: 1,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::builder::write_v3_ccs_package_from_bounded_memory_for_tests;
    use crate::ccs::signing::SigningKeyPair;
    use crate::ccs::v3::{FileContentLayoutV3, PackageKindV3};
    use crate::ccs::verify::{verify_package, verify_package_into_cas};

    fn fixture() -> (
        tempfile::TempDir,
        std::path::PathBuf,
        SigningKeyPair,
        TrustPolicy,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let package_path = temp.path().join("transport.ccs");
        let authority = crate::ccs::v3::test_support::package_authority_with_one_file("transport");
        let payloads = crate::ccs::v3::test_support::one_file_payloads_for_tests();
        let signer = SigningKeyPair::generate();
        write_v3_ccs_package_from_bounded_memory_for_tests(
            &authority,
            &payloads,
            &package_path,
            &signer,
            None,
            None,
            None,
        )
        .unwrap();
        let policy = TrustPolicy::strict(vec![signer.public_key_base64()]);
        (temp, package_path, signer, policy)
    }

    #[test]
    fn transport_round_trip_reuses_the_signed_object_set() {
        let (temp, package_path, _signer, policy) = fixture();
        let verified = verify_package(&package_path, &policy).unwrap();
        let descriptor = CcsTransportEnvelopeV1::from_verified_archive(&verified).unwrap();
        let cas = CasStore::new(temp.path().join("objects")).unwrap();
        let set = persist_verified_archive_objects(&verified, &cas).unwrap();
        let transported = descriptor
            .authenticate(&policy)
            .unwrap()
            .verify_object_set(set)
            .unwrap();
        let reconstructed = temp.path().join("reconstructed.ccs");
        write_verified_transport_archive(&transported, &reconstructed).unwrap();

        let checked = verify_package_into_cas(&reconstructed, &policy, &cas).unwrap();
        assert_eq!(checked.authority(), verified.authority());
        assert_eq!(checked.verified_object_metrics().unwrap().misses, 0);
    }

    #[test]
    fn transport_rejects_reordered_and_size_inconsistent_object_sequences() {
        let (_temp, package_path, _signer, policy) = fixture();
        let verified = verify_package(&package_path, &policy).unwrap();
        let mut descriptor = CcsTransportEnvelopeV1::from_verified_archive(&verified).unwrap();
        let object = descriptor.objects.first().unwrap().clone();
        descriptor.objects.push(object.clone());
        assert!(
            descriptor
                .authenticate(&policy)
                .unwrap_err()
                .to_string()
                .contains("object sequence")
        );

        descriptor.objects = vec![CcsTransportObjectV1 {
            size: object.size + 1,
            ..object
        }];
        assert!(
            descriptor
                .authenticate(&policy)
                .unwrap_err()
                .to_string()
                .contains("object sequence")
        );
    }

    #[test]
    fn transport_object_batch_refuses_missing_corrupt_and_oversized_objects() {
        let (temp, package_path, _signer, policy) = fixture();
        let verified = verify_package(&package_path, &policy).unwrap();
        let descriptor = CcsTransportEnvelopeV1::from_verified_archive(&verified).unwrap();
        let expected = descriptor
            .objects
            .iter()
            .map(|object| (object.sha256.clone(), object.size))
            .collect::<Vec<_>>();
        let (hash, size) = expected[0].clone();

        let missing_cas = CasStore::new(temp.path().join("missing-objects")).unwrap();
        let missing = missing_cas.verified_object_batch(expected.clone()).unwrap();
        assert!(
            missing
                .commit()
                .unwrap_err()
                .to_string()
                .contains("is ingested")
        );
        assert!(!missing_cas.exists(&hash));

        let corrupt_cas = CasStore::new(temp.path().join("corrupt-objects")).unwrap();
        let mut corrupt = corrupt_cas.verified_object_batch(expected.clone()).unwrap();
        let corrupt_bytes = vec![0_u8; size as usize];
        corrupt
            .ingest(&hash, &mut std::io::Cursor::new(corrupt_bytes))
            .unwrap_err();
        assert!(!corrupt_cas.exists(&hash));

        let oversized_cas = CasStore::new(temp.path().join("oversized-objects")).unwrap();
        let mut oversized = oversized_cas.verified_object_batch(expected).unwrap();
        let oversized_bytes = vec![0_u8; size as usize + 1];
        assert!(
            oversized
                .ingest(&hash, &mut std::io::Cursor::new(oversized_bytes))
                .unwrap_err()
                .to_string()
                .contains("size")
        );
        assert!(!oversized_cas.exists(&hash));
    }

    fn write_origin_fixture(
        path: &Path,
        name: &str,
        version: &str,
        origin: &str,
        bytes: &[u8],
        signer: &SigningKeyPair,
    ) {
        let mut authority = crate::ccs::v3::test_support::package_authority_with_one_file(name);
        authority.identity.version = version.to_string();
        authority.provenance.origin_class = Some(origin.to_string());
        let PackageKindV3::Package(package) = &mut authority.kind else {
            unreachable!()
        };
        package.files[0].content = Some(crate::payload::PayloadContentAuthority {
            sha256: crate::hash::sha256(bytes),
            size: bytes.len() as u64,
        });
        package.files[0].content_layout =
            if bytes.len() < crate::ccs::chunking::MIN_CHUNK_SIZE as usize {
                FileContentLayoutV3::WholeObject
            } else {
                FileContentLayoutV3::FastCdcV2020 {
                    min_size: crate::ccs::chunking::MIN_CHUNK_SIZE,
                    average_size: crate::ccs::chunking::AVG_CHUNK_SIZE,
                    max_size: crate::ccs::chunking::MAX_CHUNK_SIZE,
                    chunks: crate::ccs::chunking::Chunker::new()
                        .chunk_bytes(bytes)
                        .iter()
                        .map(crate::ccs::chunking::Chunk::reference)
                        .collect(),
                }
            };
        authority.components.get_mut("main").unwrap().total_size = bytes.len() as u64;
        let payloads = BTreeMap::from([("/usr/bin/hello".to_string(), bytes.to_vec())]);
        write_v3_ccs_package_from_bounded_memory_for_tests(
            &authority, &payloads, path, signer, None, None, None,
        )
        .unwrap();
    }

    #[test]
    fn source_format_and_version_do_not_change_shared_object_identity() {
        let temp = tempfile::tempdir().unwrap();
        let signer = SigningKeyPair::generate();
        let policy = TrustPolicy::strict(vec![signer.public_key_base64()]);
        let v1 = (0..(crate::ccs::chunking::MAX_CHUNK_SIZE as usize * 8))
            .map(|index| ((index * 31 + index / 251) % 256) as u8)
            .collect::<Vec<_>>();
        let mut v2 = v1.clone();
        let edit = v2.len() / 2;
        for byte in &mut v2[edit..edit + 4096] {
            *byte ^= 0x5a;
        }
        let eopkg = temp.path().join("eopkg-v1.ccs");
        let arch = temp.path().join("arch-v1.1.ccs");
        write_origin_fixture(
            &eopkg,
            "cross-format",
            "1.0.0",
            "foreign-eopkg",
            &v1,
            &signer,
        );
        write_origin_fixture(&arch, "cross-format", "1.1.0", "foreign-arch", &v2, &signer);

        let v1_verified = verify_package(&eopkg, &policy).unwrap();
        let v2_verified = verify_package(&arch, &policy).unwrap();
        let v1_transport = CcsTransportEnvelopeV1::from_verified_archive(&v1_verified).unwrap();
        let v2_transport = CcsTransportEnvelopeV1::from_verified_archive(&v2_verified).unwrap();
        let v1_objects = v1_transport
            .objects
            .iter()
            .map(|object| (&object.sha256, object.size))
            .collect::<BTreeMap<_, _>>();
        let v2_objects = v2_transport
            .objects
            .iter()
            .map(|object| (&object.sha256, object.size))
            .collect::<BTreeMap<_, _>>();
        let shared = v1_objects
            .iter()
            .filter(|(hash, size)| v2_objects.get(*hash) == Some(size))
            .count();
        assert!(
            shared > 0,
            "the local edit must preserve reusable CDC objects"
        );
        assert!(
            shared < v2_objects.len(),
            "the local edit must also introduce at least one new object"
        );
        let cas = CasStore::new(temp.path().join("cross-format-objects")).unwrap();
        let v1_set = persist_verified_archive_objects(&v1_verified, &cas).unwrap();
        assert_eq!(v1_set.metrics().misses, v1_objects.len() as u64);
        let v2_set = persist_verified_archive_objects(&v2_verified, &cas).unwrap();
        let v2_metrics = v2_set.metrics();
        assert_eq!(v2_metrics.hits, shared as u64);
        assert_eq!(v2_metrics.misses, (v2_objects.len() - shared) as u64);
        assert!(v2_metrics.persistent_bytes_written < v2.len() as u64);
        println!(
            "ISSUE400_V2_OBJECTS={} ISSUE400_SHARED_OBJECTS={} ISSUE400_NEW_OBJECTS={} ISSUE400_NEW_BYTES={} ISSUE400_FULL_BYTES={}",
            v2_objects.len(),
            shared,
            v2_metrics.misses,
            v2_metrics.persistent_bytes_written,
            v2.len()
        );

        // Whole-object identity is byte authority too; provenance and package
        // identity cannot fork it.
        let identical = b"identical whole file\n";
        let deb = temp.path().join("deb-whole.ccs");
        let ccs = temp.path().join("ccs-whole.ccs");
        write_origin_fixture(
            &deb,
            "deb-origin",
            "1.0.0",
            "foreign-deb",
            identical,
            &signer,
        );
        write_origin_fixture(
            &ccs,
            "native-origin",
            "9.0.0",
            "native-built",
            identical,
            &signer,
        );
        let deb_transport =
            CcsTransportEnvelopeV1::from_verified_archive(&verify_package(&deb, &policy).unwrap())
                .unwrap();
        let ccs_transport =
            CcsTransportEnvelopeV1::from_verified_archive(&verify_package(&ccs, &policy).unwrap())
                .unwrap();
        assert_eq!(deb_transport.objects, ccs_transport.objects);
    }
}
