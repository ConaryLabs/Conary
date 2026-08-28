// crates/conary-core/src/repository/catalog/bundle/tests.rs

use super::*;
use crate::repository::catalog::{
    CatalogArtifactV1, CatalogCandidateWriter, CatalogContentV1, CatalogPackageOriginV1,
    CatalogPackageRecordV1, CatalogScopeV1, CatalogSourceEvidenceV1, PROFILE_REVISION_SCHEMA_V2,
    ProfileSourceMemberV2, SOURCE_SNAPSHOT_SCHEMA_V1, SourceEcosystemV1,
    SourceMetadataObjectRoleV1, SourceMetadataObjectV1, SourceProvenanceV1, SourceStreamKindV1,
    SourceStreamV1, logical_verification_passes_for_test, physical_verification_passes_for_test,
    write_catalog_candidate,
};
use crate::repository::supported_profiles::ProfileSourceRole;
use crate::repository::versioning::VersionScheme;
use crate::repository::{
    OpenPgpTrustRoot, RepositoryParserConfig, RepositoryTrustPolicy, RpmMetadataAuthority,
};
use std::io::{Read, Seek, SeekFrom, Write};

fn digest(byte: char) -> String {
    byte.to_string().repeat(64)
}

fn artifact(byte: char, size: u64) -> CatalogArtifactV1 {
    CatalogArtifactV1 {
        sha256: digest(byte),
        size,
    }
}

fn package(origin: CatalogPackageOriginV1) -> CatalogPackageRecordV1 {
    CatalogPackageRecordV1 {
        package_key_sha256: String::new(),
        origin,
        source_profile: "fedora-44".to_string(),
        name: "bash".to_string(),
        version: "5.2.37-1".to_string(),
        package_release: "1".to_string(),
        architecture: Some("x86_64".to_string()),
        debian_multi_arch: None,
        description: Some("shell".to_string()),
        checksum: digest('c'),
        size: 2048,
        download_url: "https://example.test/bash.rpm".to_string(),
        metadata: Some("{}".to_string()),
        is_security_update: false,
        severity: None,
        cve_ids: None,
        advisory_id: None,
        advisory_url: None,
        version_scheme: VersionScheme::Rpm,
        provides: Vec::new(),
        requirement_groups: Vec::new(),
    }
}

fn source_object() -> SourceMetadataObjectV1 {
    let bytes = source_object_bytes();
    SourceMetadataObjectV1 {
        role: SourceMetadataObjectRoleV1::RpmPrimary,
        source_path: "repodata/primary.xml.zst".to_string(),
        sha256: crate::hash::sha256(&bytes),
        size: bytes.len() as u64,
    }
}

fn source_object_bytes() -> Vec<u8> {
    vec![b'x'; 1024]
}

fn retain_source_object(candidate: &Path, object: &SourceMetadataObjectV1) {
    let work = tempfile::Builder::new()
        .prefix("source-object-")
        .tempdir_in(candidate.parent().expect("candidate parent"))
        .expect("create source-object work directory");
    let path = work.path().join("authenticated-object");
    fs::write(&path, source_object_bytes()).expect("write authenticated source object");
    retain_source_metadata_object(candidate, work.path(), &path, object)
        .expect("retain authenticated source object");
}

fn source_content() -> CatalogContentV1 {
    let object = source_object();
    CatalogContentV1::new(
        CatalogScopeV1::Source {
            source_profile: "fedora-44".to_string(),
            source_identity: "fedora-project".to_string(),
            repository_identity: "fedora-everything-x86_64".to_string(),
        },
        vec![CatalogSourceEvidenceV1::AuthenticatedObject {
            role: object.role,
            source_path: object.source_path,
            sha256: object.sha256,
            size: object.size,
        }],
        vec![package(CatalogPackageOriginV1::Source {
            source_identity: "fedora-project".to_string(),
            repository_identity: "fedora-everything-x86_64".to_string(),
        })],
    )
    .unwrap()
}

fn source_manifest(binding: &CatalogBindingV1) -> SourceSnapshotV1 {
    let parser_config = RepositoryParserConfig::Rpm {
        architecture: "x86_64".to_string(),
    };
    let trust_policy = RepositoryTrustPolicy::Rpm {
        metadata: RpmMetadataAuthority::Metalink {
            url: "https://example.test/metalink".to_string(),
        },
        package_keys: vec![
            OpenPgpTrustRoot::new(
                "https://example.test/fedora.gpg".to_string(),
                "A".repeat(40),
            )
            .unwrap(),
        ],
    };
    SourceSnapshotV1 {
        schema_version: SOURCE_SNAPSHOT_SCHEMA_V1,
        source_profile: "fedora-44".to_string(),
        source_identity: "fedora-project".to_string(),
        repository_identity: "fedora-everything-x86_64".to_string(),
        stream: SourceStreamV1 {
            kind: SourceStreamKindV1::Release,
            identity: "44".to_string(),
        },
        stream_binding_sha256: digest('e'),
        parser_projection_version: crate::repository::catalog::SOURCE_CATALOG_PROJECTION_VERSION_V2,
        provenance: SourceProvenanceV1 {
            ecosystem: SourceEcosystemV1::Rpm,
            metadata_url: "https://example.test/repository".to_string(),
            content_url: Some("https://content.example.test/repository".to_string()),
            parser_config_sha256: crate::hash::sha256(
                &crate::json::canonical_json(&parser_config).unwrap(),
            ),
            parser_config,
            trust_policy_sha256: crate::hash::sha256(
                &crate::json::canonical_json(&trust_policy).unwrap(),
            ),
            trust_policy,
        },
        authenticated_root: artifact('2', 512),
        authenticated_objects: vec![source_object()],
        catalog: binding.artifact.clone(),
        logical_digest_sha256: binding.logical_digest_sha256.clone(),
        counts: binding.counts,
    }
}

fn source_candidate(root: &Path, name: &str) -> (PathBuf, SourceSnapshotV1) {
    let candidate = root.join(name);
    fs::create_dir(&candidate).unwrap();
    let binding =
        write_catalog_candidate(candidate.join(CATALOG_FILE_NAME), &source_content()).unwrap();
    let manifest = source_manifest(&binding);
    retain_source_object(&candidate, &manifest.authenticated_objects[0]);
    write_source_catalog_manifest(&candidate, &manifest).unwrap();
    (candidate, manifest)
}

fn profile_content(source_snapshot_sha256: &str) -> CatalogContentV1 {
    CatalogContentV1::new(
        CatalogScopeV1::Profile {
            profile: "fedora-44".to_string(),
        },
        vec![CatalogSourceEvidenceV1::SourceSnapshot {
            member_ordinal: 0,
            source_identity: "fedora-project".to_string(),
            repository_identity: "fedora-everything-x86_64".to_string(),
            source_snapshot_sha256: source_snapshot_sha256.to_string(),
        }],
        vec![package(CatalogPackageOriginV1::Profile {
            member_ordinal: 0,
            source_identity: "fedora-project".to_string(),
            repository_identity: "fedora-everything-x86_64".to_string(),
            source_snapshot_sha256: source_snapshot_sha256.to_string(),
        })],
    )
    .unwrap()
}

fn profile_manifest(binding: &CatalogBindingV1, source_snapshot_sha256: &str) -> ProfileRevisionV2 {
    ProfileRevisionV2 {
        schema_version: PROFILE_REVISION_SCHEMA_V2,
        profile: "fedora-44".to_string(),
        projection_version: 1,
        members: vec![ProfileSourceMemberV2 {
            ordinal: 0,
            role: ProfileSourceRole::Base,
            source_identity: "fedora-project".to_string(),
            repository_identity: "fedora-everything-x86_64".to_string(),
            stream: SourceStreamV1 {
                kind: SourceStreamKindV1::Release,
                identity: "44".to_string(),
            },
            precedence: 10,
            required: true,
            source_snapshot_sha256: source_snapshot_sha256.to_string(),
        }],
        catalog: binding.artifact.clone(),
        logical_digest_sha256: binding.logical_digest_sha256.clone(),
        counts: binding.counts,
    }
}

#[test]
fn source_bundle_is_verified_before_atomic_content_addressed_publication() {
    let directory = tempfile::tempdir().unwrap();
    let candidates = directory.path().join("candidates");
    let candidate = candidates.join("source");
    let catalogs = directory.path().join("catalogs");
    fs::create_dir(&candidates).unwrap();
    fs::create_dir(&candidate).unwrap();
    fs::create_dir(&catalogs).unwrap();
    let binding =
        write_catalog_candidate(candidate.join(CATALOG_FILE_NAME), &source_content()).unwrap();
    let manifest = source_manifest(&binding);
    retain_source_object(&candidate, &manifest.authenticated_objects[0]);
    let staged_reader = write_source_catalog_manifest(&candidate, &manifest).unwrap();
    assert_eq!(
        staged_reader.binding().artifact.sha256,
        manifest.catalog.sha256
    );
    verify_source_catalog_bundle(&candidate, &manifest).unwrap();

    let expected_identity = manifest.manifest_sha256().unwrap();
    let publication =
        publish_source_catalog_bundle_with_provenance(&candidate, &catalogs, &manifest).unwrap();
    assert!(publication.newly_created);
    let published = publication.path;
    assert_eq!(published.file_name().unwrap(), expected_identity.as_str());
    assert!(!candidate.exists());
    verify_source_catalog_bundle(&published, &manifest).unwrap();
}

#[test]
fn registered_profile_bundle_reopen_uses_its_durable_v2_logical_attestation() {
    let directory = tempfile::tempdir().unwrap();
    let candidate = directory.path().join("profile");
    let catalogs = directory.path().join("catalogs");
    fs::create_dir(&candidate).unwrap();
    fs::create_dir(&catalogs).unwrap();
    let source_snapshot_sha256 = digest('8');
    let binding = write_catalog_candidate(
        candidate.join(CATALOG_FILE_NAME),
        &profile_content(&source_snapshot_sha256),
    )
    .unwrap();
    let manifest = profile_manifest(&binding, &source_snapshot_sha256);
    write_profile_catalog_manifest(&candidate, &manifest).unwrap();
    let published = publish_profile_catalog_bundle(&candidate, &catalogs, &manifest).unwrap();
    let logical_passes_after_publication = logical_verification_passes_for_test();

    let reader = verify_registered_profile_catalog_bundle(&published, &manifest).unwrap();
    assert_eq!(reader.binding(), &binding);
    assert!(reader.verification_proof().is_ok());
    assert_eq!(
        logical_verification_passes_for_test(),
        logical_passes_after_publication,
        "registered profile reopen must not replay normalized catalog rows"
    );
}

#[test]
fn registered_source_bundle_reopen_uses_its_durable_logical_attestation() {
    let directory = tempfile::tempdir().unwrap();
    let candidate = directory.path().join("source");
    let catalogs = directory.path().join("catalogs");
    fs::create_dir(&candidate).unwrap();
    fs::create_dir(&catalogs).unwrap();
    let binding =
        write_catalog_candidate(candidate.join(CATALOG_FILE_NAME), &source_content()).unwrap();
    let manifest = source_manifest(&binding);
    retain_source_object(&candidate, &manifest.authenticated_objects[0]);
    write_source_catalog_manifest(&candidate, &manifest).unwrap();
    let published = publish_source_catalog_bundle(&candidate, &catalogs, &manifest).unwrap();
    let logical_passes_after_publication = logical_verification_passes_for_test();

    let reader = verify_registered_source_catalog_bundle(&published, &manifest).unwrap();
    assert_eq!(reader.binding(), &binding);
    assert!(reader.verification_proof().is_ok());
    assert_eq!(
        logical_verification_passes_for_test(),
        logical_passes_after_publication,
        "registered source reopen must not replay normalized catalog rows"
    );
}

#[test]
fn local_logical_proof_survives_manifesting_and_atomic_publication() {
    let directory = tempfile::tempdir().unwrap();
    let candidate = directory.path().join("candidate");
    let catalogs = directory.path().join("catalogs");
    fs::create_dir(&candidate).unwrap();
    fs::create_dir(&catalogs).unwrap();
    let catalog_path = candidate.join(CATALOG_FILE_NAME);
    let binding = write_catalog_candidate(&catalog_path, &source_content()).unwrap();
    let manifest = source_manifest(&binding);
    retain_source_object(&candidate, &manifest.authenticated_objects[0]);
    let verified = CatalogReader::open_verified(&catalog_path, &binding).unwrap();
    let candidate_physical_passes = physical_verification_passes_for_test();
    let manifested =
        write_source_catalog_manifest_verified(&candidate, &manifest, verified).unwrap();
    assert_eq!(
        physical_verification_passes_for_test(),
        candidate_physical_passes,
        "manifesting must consume the exact candidate proof without reopening it"
    );

    let published =
        publish_source_catalog_bundle_verified(&candidate, &catalogs, &manifest, manifested)
            .unwrap();
    assert_eq!(
        physical_verification_passes_for_test(),
        candidate_physical_passes + 1,
        "publication must perform exactly one independent destination reopen"
    );
    assert!(!candidate.exists());
    verify_source_catalog_bundle(&published, &manifest).unwrap();
}

#[test]
fn verified_manifest_handoff_requires_the_exact_candidate_reader() {
    let directory = tempfile::tempdir().unwrap();
    let first = directory.path().join("first");
    let second = directory.path().join("second");
    fs::create_dir(&first).unwrap();
    fs::create_dir(&second).unwrap();
    let first_catalog = first.join(CATALOG_FILE_NAME);
    let binding = write_catalog_candidate(&first_catalog, &source_content()).unwrap();
    let manifest = source_manifest(&binding);
    retain_source_object(&first, &manifest.authenticated_objects[0]);
    fs::copy(&first_catalog, second.join(CATALOG_FILE_NAME)).unwrap();
    retain_source_object(&second, &manifest.authenticated_objects[0]);
    let verified = CatalogReader::open_verified(&first_catalog, &binding).unwrap();

    let error = match write_source_catalog_manifest_verified(&second, &manifest, verified) {
        Err(error) => error,
        Ok(_) => panic!("a reader for another path finalized the candidate"),
    };
    assert!(error.to_string().contains("does not own exact candidate"));
    assert!(!second.join(CATALOG_MANIFEST_FILE_NAME).exists());
}

#[test]
fn profile_manifest_and_publication_reuse_one_candidate_proof() {
    let directory = tempfile::tempdir().unwrap();
    let candidate = directory.path().join("profile");
    let catalogs = directory.path().join("catalogs");
    fs::create_dir(&candidate).unwrap();
    fs::create_dir(&catalogs).unwrap();
    let source_snapshot_sha256 = digest('8');
    let catalog_path = candidate.join(CATALOG_FILE_NAME);
    let binding =
        write_catalog_candidate(&catalog_path, &profile_content(&source_snapshot_sha256)).unwrap();
    let manifest = profile_manifest(&binding, &source_snapshot_sha256);
    let verified = CatalogReader::open_verified(&catalog_path, &binding).unwrap();
    let candidate_physical_passes = physical_verification_passes_for_test();

    let manifested =
        write_profile_catalog_manifest_verified(&candidate, &manifest, verified).unwrap();
    assert_eq!(
        physical_verification_passes_for_test(),
        candidate_physical_passes
    );
    let published =
        publish_profile_catalog_bundle_verified(&candidate, &catalogs, &manifest, manifested)
            .unwrap();
    assert_eq!(
        physical_verification_passes_for_test(),
        candidate_physical_passes + 1
    );
    assert!(!candidate.exists());
    assert!(published.exists());
}

#[test]
fn local_logical_proof_never_bypasses_bundle_byte_binding() {
    let directory = tempfile::tempdir().unwrap();
    let candidate = directory.path().join("candidate");
    let catalogs = directory.path().join("catalogs");
    fs::create_dir(&candidate).unwrap();
    fs::create_dir(&catalogs).unwrap();
    let catalog_path = candidate.join(CATALOG_FILE_NAME);
    let binding = write_catalog_candidate(&catalog_path, &source_content()).unwrap();
    let manifest = source_manifest(&binding);
    retain_source_object(&candidate, &manifest.authenticated_objects[0]);
    let verified = CatalogReader::open_verified(&catalog_path, &binding).unwrap();
    let manifested =
        write_source_catalog_manifest_verified(&candidate, &manifest, verified).unwrap();

    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&catalog_path)
        .unwrap();
    file.seek(SeekFrom::Start(128)).unwrap();
    file.write_all(&[0xff]).unwrap();
    file.sync_all().unwrap();
    drop(file);

    let error =
        publish_source_catalog_bundle_verified(&candidate, &catalogs, &manifest, manifested)
            .unwrap_err();
    assert!(error.to_string().contains("Checksum mismatch"));
    assert!(!candidate.exists());
    assert!(catalogs.join("sources").exists());
    assert_eq!(fs::read_dir(catalogs.join("sources")).unwrap().count(), 0);
}

#[test]
fn source_manifest_finalization_rejects_unowned_candidate_entries() {
    let directory = tempfile::tempdir().unwrap();
    let candidate = directory.path().join("source");
    fs::create_dir(&candidate).unwrap();
    let binding =
        write_catalog_candidate(candidate.join(CATALOG_FILE_NAME), &source_content()).unwrap();
    let manifest = source_manifest(&binding);
    retain_source_object(&candidate, &manifest.authenticated_objects[0]);
    fs::write(candidate.join("unowned.tmp"), b"not part of the bundle").unwrap();

    let error = write_source_catalog_manifest(&candidate, &manifest)
        .err()
        .expect("unowned candidate entry must fail manifest finalization");
    assert!(error.to_string().contains("incomplete or unexpected"));
}

#[test]
fn source_bundle_verification_streams_high_presentation_cardinality() {
    const CHILD_ENV: &str = "CONARY_SOURCE_BUNDLE_RSS_CHILD";
    if std::env::var_os(CHILD_ENV).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "repository::catalog::bundle::tests::source_bundle_verification_streams_high_presentation_cardinality",
                "--nocapture",
            ])
            .env(CHILD_ENV, "1")
            .output()
            .unwrap();
        print!("{}", String::from_utf8_lossy(&output.stdout));
        std::io::stderr().write_all(&output.stderr).unwrap();
        assert!(output.status.success(), "source bundle RSS child failed");
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("SOURCE_BUNDLE_VM_HWM_KIB="),
            "source bundle RSS child did not report VmHWM"
        );
        return;
    }

    const PACKAGES: usize = 4_096;
    const PRESENTATION_BYTES: usize = 64 * 1024;
    const RSS_LIMIT_KIB: u64 = 192 * 1024;
    let directory = tempfile::tempdir().unwrap();
    let candidate = directory.path().join("source");
    fs::create_dir(&candidate).unwrap();
    let scope = CatalogScopeV1::Source {
        source_profile: "fedora-44".to_string(),
        source_identity: "fedora-project".to_string(),
        repository_identity: "fedora-everything-x86_64".to_string(),
    };
    let mut writer =
        CatalogCandidateWriter::create(candidate.join(CATALOG_FILE_NAME), scope).unwrap();
    for index in 0..PACKAGES {
        let name = format!("presentation-{index:04}");
        let mut record = package(CatalogPackageOriginV1::Source {
            source_identity: "fedora-project".to_string(),
            repository_identity: "fedora-everything-x86_64".to_string(),
        });
        record.name = name.clone();
        record.checksum = crate::hash::sha256(name.as_bytes());
        record.download_url = format!("https://example.test/{name}.rpm");
        record.metadata = Some(format!(
            "{{\"presentation\":\"{}\"}}",
            "x".repeat(PRESENTATION_BYTES)
        ));
        writer.package(record).unwrap();
    }
    let object = source_object();
    let binding = writer
        .finish(vec![CatalogSourceEvidenceV1::AuthenticatedObject {
            role: object.role,
            source_path: object.source_path,
            sha256: object.sha256,
            size: object.size,
        }])
        .unwrap();
    let manifest = source_manifest(&binding);
    retain_source_object(&candidate, &manifest.authenticated_objects[0]);
    write_source_catalog_manifest(&candidate, &manifest).unwrap();
    let reader = verify_source_catalog_bundle(&candidate, &manifest).unwrap();
    assert_eq!(reader.binding().counts.packages, PACKAGES as u64);

    let high_water_kib = vm_hwm_kib().unwrap();
    println!("SOURCE_BUNDLE_VM_HWM_KIB={high_water_kib}");
    assert!(
        high_water_kib < RSS_LIMIT_KIB,
        "VmHWM {high_water_kib} KiB exceeded fixed {RSS_LIMIT_KIB} KiB bound"
    );
}

fn vm_hwm_kib() -> Option<u64> {
    let mut status = String::new();
    std::fs::File::open("/proc/self/status")
        .ok()?
        .read_to_string(&mut status)
        .ok()?;
    status.lines().find_map(|line| {
        line.strip_prefix("VmHWM:")
            .and_then(|value| value.split_whitespace().next())
            .and_then(|value| value.parse().ok())
    })
}

#[test]
fn existing_exact_destination_is_reused_and_survives_caller_rollback() {
    let directory = tempfile::tempdir().unwrap();
    let candidates = directory.path().join("candidates");
    let catalogs = directory.path().join("catalogs");
    fs::create_dir(&candidates).unwrap();
    fs::create_dir(&catalogs).unwrap();

    let (first_candidate, manifest) = source_candidate(&candidates, "first");
    let first =
        publish_source_catalog_bundle_with_provenance(&first_candidate, &catalogs, &manifest)
            .unwrap();
    assert!(first.newly_created);

    let (second_candidate, second_manifest) = source_candidate(&candidates, "second");
    assert_eq!(manifest, second_manifest);
    let reused = publish_source_catalog_bundle_with_provenance(
        &second_candidate,
        &catalogs,
        &second_manifest,
    )
    .unwrap();
    assert!(!reused.newly_created);
    assert_eq!(reused.path, first.path);

    if reused.newly_created {
        fs::remove_dir_all(&reused.path).unwrap();
    }
    assert!(reused.path.exists());
    assert!(second_candidate.exists());
}

#[test]
fn malformed_existing_destination_errors_without_removal() {
    let directory = tempfile::tempdir().unwrap();
    let candidates = directory.path().join("candidates");
    let catalogs = directory.path().join("catalogs");
    fs::create_dir(&candidates).unwrap();
    fs::create_dir(&catalogs).unwrap();
    let (candidate, manifest) = source_candidate(&candidates, "source");
    let destination = catalogs
        .join("sources")
        .join(manifest.manifest_sha256().unwrap());
    fs::create_dir_all(&destination).unwrap();
    fs::write(destination.join("unexpected"), b"not a catalog bundle").unwrap();

    let error = publish_source_catalog_bundle_with_provenance(&candidate, &catalogs, &manifest)
        .unwrap_err();
    assert!(error.to_string().contains("incomplete or unexpected"));
    assert!(destination.exists());
    assert!(destination.join("unexpected").exists());
    assert!(candidate.exists());
}

#[test]
fn post_rename_verification_failure_removes_only_new_destination() {
    let directory = tempfile::tempdir().unwrap();
    let candidate = directory.path().join("candidate");
    let parent = directory.path().join("parent");
    fs::create_dir(&candidate).unwrap();
    fs::create_dir(&parent).unwrap();

    let error = publish_verified_directory(&candidate, &parent, "bundle", |_| {
        Err(Error::ConflictError(
            "verification failed after rename".to_string(),
        ))
    })
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("verification failed after rename")
    );
    assert!(!candidate.exists());
    assert!(!parent.join("bundle").exists());
}

#[test]
fn malformed_or_extra_candidate_content_is_never_published() {
    let directory = tempfile::tempdir().unwrap();
    let candidate = directory.path().join("candidate");
    let catalogs = directory.path().join("catalogs");
    fs::create_dir(&candidate).unwrap();
    fs::create_dir(&catalogs).unwrap();
    let binding =
        write_catalog_candidate(candidate.join(CATALOG_FILE_NAME), &source_content()).unwrap();
    let manifest = source_manifest(&binding);
    retain_source_object(&candidate, &manifest.authenticated_objects[0]);
    write_source_catalog_manifest(&candidate, &manifest).unwrap();
    fs::write(candidate.join("unowned.tmp"), b"nope").unwrap();

    assert!(publish_source_catalog_bundle(&candidate, &catalogs, &manifest).is_err());
    assert!(candidate.exists());
    assert!(!catalogs.join("sources").exists());
}

#[test]
fn source_bundle_rejects_missing_truncated_tampered_and_extra_metadata() {
    let directory = tempfile::tempdir().unwrap();
    let candidates = directory.path().join("candidates");
    fs::create_dir(&candidates).unwrap();

    for mutation in ["missing", "truncated", "tampered", "extra"] {
        let (candidate, manifest) = source_candidate(&candidates, mutation);
        let metadata = candidate.join(SOURCE_METADATA_DIRECTORY_NAME);
        let object = metadata.join(&manifest.authenticated_objects[0].sha256);
        match mutation {
            "missing" => fs::remove_file(&object).unwrap(),
            "truncated" => fs::write(&object, b"").unwrap(),
            "tampered" => fs::write(&object, vec![b'y'; 1024]).unwrap(),
            "extra" => fs::write(metadata.join("unexpected"), b"nope").unwrap(),
            _ => unreachable!(),
        }
        assert!(
            verify_source_catalog_bundle(&candidate, &manifest).is_err(),
            "{mutation} metadata must fail closed"
        );
    }
}

#[cfg(unix)]
#[test]
fn source_bundle_rejects_symlinked_metadata() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().unwrap();
    let candidates = directory.path().join("candidates");
    fs::create_dir(&candidates).unwrap();
    let (candidate, manifest) = source_candidate(&candidates, "symlinked");
    let object = candidate
        .join(SOURCE_METADATA_DIRECTORY_NAME)
        .join(&manifest.authenticated_objects[0].sha256);
    fs::remove_file(&object).unwrap();
    symlink(candidate.join(CATALOG_FILE_NAME), &object).unwrap();

    assert!(verify_source_catalog_bundle(&candidate, &manifest).is_err());
}

#[test]
fn legacy_source_projection_cannot_claim_retained_metadata_authority() {
    let directory = tempfile::tempdir().unwrap();
    let candidate = directory.path().join("candidate");
    fs::create_dir(&candidate).unwrap();
    let binding =
        write_catalog_candidate(candidate.join(CATALOG_FILE_NAME), &source_content()).unwrap();
    let mut manifest = source_manifest(&binding);
    manifest.parser_projection_version = 1;
    retain_source_object(&candidate, &manifest.authenticated_objects[0]);

    let error = write_source_catalog_manifest(&candidate, &manifest)
        .err()
        .expect("legacy projection must fail manifest finalization");
    assert!(
        error
            .to_string()
            .contains("projection version 1 is unsupported")
    );
}

#[test]
fn profile_bundle_rejects_mixed_member_evidence() {
    let directory = tempfile::tempdir().unwrap();
    let candidate = directory.path().join("profile");
    fs::create_dir(&candidate).unwrap();
    let source_snapshot_sha256 = digest('3');
    let content = CatalogContentV1::new(
        CatalogScopeV1::Profile {
            profile: "fedora-44".to_string(),
        },
        vec![CatalogSourceEvidenceV1::SourceSnapshot {
            member_ordinal: 0,
            source_identity: "fedora-project".to_string(),
            repository_identity: "fedora-everything-x86_64".to_string(),
            source_snapshot_sha256: source_snapshot_sha256.clone(),
        }],
        vec![package(CatalogPackageOriginV1::Profile {
            member_ordinal: 0,
            source_identity: "fedora-project".to_string(),
            repository_identity: "fedora-everything-x86_64".to_string(),
            source_snapshot_sha256: source_snapshot_sha256.clone(),
        })],
    )
    .unwrap();
    let binding = write_catalog_candidate(candidate.join(CATALOG_FILE_NAME), &content).unwrap();
    let manifest = ProfileRevisionV2 {
        schema_version: PROFILE_REVISION_SCHEMA_V2,
        profile: "fedora-44".to_string(),
        projection_version: 1,
        members: vec![ProfileSourceMemberV2 {
            ordinal: 0,
            source_identity: "fedora-project".to_string(),
            repository_identity: "fedora-everything-x86_64".to_string(),
            stream: SourceStreamV1 {
                kind: SourceStreamKindV1::Release,
                identity: "44".to_string(),
            },
            role: crate::repository::supported_profiles::ProfileSourceRole::Base,
            precedence: 10,
            required: true,
            source_snapshot_sha256: digest('4'),
        }],
        catalog: binding.artifact,
        logical_digest_sha256: binding.logical_digest_sha256,
        counts: binding.counts,
    };
    let error = write_profile_catalog_manifest(&candidate, &manifest)
        .err()
        .expect("mixed profile evidence must fail manifest finalization");
    assert!(error.to_string().contains("members do not match"));
    assert!(!candidate.join(CATALOG_MANIFEST_FILE_NAME).exists());
}
