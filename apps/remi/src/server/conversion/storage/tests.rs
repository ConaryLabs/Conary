// apps/remi/src/server/conversion/storage/tests.rs

use super::*;
use conary_core::ccs::builder::write_v3_ccs_package_from_bounded_memory_for_tests;
use conary_core::ccs::signing::SigningKeyPair;
use conary_core::ccs::v3::schema::{
    AuthorityDocumentV3, ComponentAuthorityV3, FORMAT_VERSION_V3, FileAuthorityV3,
    FileContentLayoutV3, LifecycleAuthorityV3, PackageDataV3, PackageIdentityV3, PackageKindTagV3,
    PackageKindV3, ProvenanceAuthorityV3,
};
use conary_core::packages::source_authority::{CcsPackageAuthority, SourcePackageAuthority};
use conary_core::packages::traits::{ExtractedFile, PackageFile};
use conary_core::payload::{PayloadContentAuthority, PayloadNode};
use conary_core::repository::versioning::VersionScheme;
use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct InFlight<'a>(&'a AtomicUsize);

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct InjectedStore {
    hits: HashSet<String>,
    failed_put: Option<String>,
    current: AtomicUsize,
    maximum: AtomicUsize,
    completed: AtomicUsize,
    uploaded: Mutex<Vec<String>>,
}

impl InjectedStore {
    fn enter(&self) -> InFlight<'_> {
        let current = self.current.fetch_add(1, Ordering::SeqCst) + 1;
        self.maximum.fetch_max(current, Ordering::SeqCst);
        InFlight(&self.current)
    }

    async fn overlap(&self, hash: &str) {
        let delay = 1 + usize::from(hash.as_bytes()[0]) % 4;
        tokio::time::sleep(Duration::from_millis(delay as u64)).await;
    }
}

#[async_trait]
impl ConversionChunkStore for InjectedStore {
    async fn head_chunk(&self, hash: &str) -> Result<bool> {
        let _in_flight = self.enter();
        self.overlap(hash).await;
        let hit = self.hits.contains(hash);
        if hit {
            self.completed.fetch_add(1, Ordering::SeqCst);
        }
        Ok(hit)
    }

    async fn put_chunk(&self, hash: &str, _data: &[u8]) -> Result<()> {
        let _in_flight = self.enter();
        self.overlap(hash).await;
        self.uploaded.lock().unwrap().push(hash.to_string());
        self.completed.fetch_add(1, Ordering::SeqCst);
        ensure!(
            self.failed_put.as_deref() != Some(hash),
            "injected PUT failure for {hash}"
        );
        Ok(())
    }
}

fn signed_package(root: &Path, signer: &SigningKeyPair, payload: &[u8]) -> std::path::PathBuf {
    let path = root.join("direct-cas.ccs");
    let payload_path = "/usr/bin/direct-cas".to_string();
    let authority = AuthorityDocumentV3 {
        format_version: FORMAT_VERSION_V3,
        identity: PackageIdentityV3 {
            name: "direct-cas".to_string(),
            version: "1.0.0".to_string(),
            version_scheme: VersionScheme::Conary,
            release: "1".to_string(),
            architecture: Some("x86_64".to_string()),
            debian_multi_arch: None,
            platform: Some("linux".to_string()),
            kind: PackageKindTagV3::Package,
        },
        kind: PackageKindV3::Package(PackageDataV3 {
            files: vec![FileAuthorityV3 {
                path: payload_path.clone(),
                node: PayloadNode::regular(0o755),
                content: Some(PayloadContentAuthority {
                    sha256: conary_core::hash::sha256(payload),
                    size: payload.len() as u64,
                }),
                content_layout: FileContentLayoutV3::WholeObject,
                component: "main".to_string(),
                config: None,
                conflict: Default::default(),
            }],
            ..Default::default()
        }),
        provided_capabilities: Vec::new(),
        requirements: Vec::new(),
        relations: Vec::new(),
        execution_capabilities: None,
        file_capabilities: Vec::new(),
        components: BTreeMap::from([(
            "main".to_string(),
            ComponentAuthorityV3 {
                name: "main".to_string(),
                default: true,
                file_count: 1,
                total_size: payload.len() as u64,
            },
        )]),
        lifecycle: LifecycleAuthorityV3::default(),
        provenance: ProvenanceAuthorityV3 {
            origin_class: Some("native-built".to_string()),
            hardening_level: Some("hermetic".to_string()),
            build_input_identity: Some("sha256:build-input".to_string()),
            hermetic_evidence_hash: Some("sha256:evidence".to_string()),
            foreign_conversion_boundary_hash: None,
        },
        debug_toml_sha256: None,
    };
    write_v3_ccs_package_from_bounded_memory_for_tests(
        &authority,
        &BTreeMap::from([(payload_path, payload.to_vec())]),
        &path,
        signer,
        None,
        None,
        None,
    )
    .unwrap();
    path
}

fn direct_cas_service(
    root: &Path,
    signer: &SigningKeyPair,
) -> (ConversionService, std::path::PathBuf) {
    let keys_dir = root.join("keys");
    let profile_dir = keys_dir.join("fedora-44");
    fs::create_dir_all(&profile_dir).unwrap();
    fs::set_permissions(&keys_dir, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(&profile_dir, fs::Permissions::from_mode(0o700)).unwrap();
    crate::server::signing_authority::save_fixture_key_pair(
        signer,
        &profile_dir.join("targets.private"),
        &profile_dir.join("targets.public"),
    )
    .unwrap();

    let db_path = root.join("remi.db");
    conary_core::db::init(&db_path).unwrap();
    let chunk_dir = root.join("chunks");
    let service = ConversionService::new(chunk_dir.clone(), root.join("cache"), db_path, None)
        .with_repository_keys_dir(Some(keys_dir));
    (service, chunk_dir)
}

fn pending_conversion(root: &Path, signer: Arc<SigningKeyPair>) -> PendingConversionResult {
    let payload_bytes = b"verified staging payload";
    let payload_path = "/usr/bin/verified-staging".to_string();
    let content_authority = PayloadContentAuthority {
        sha256: conary_core::hash::sha256(payload_bytes),
        size: payload_bytes.len() as u64,
    };
    let mut metadata = conary_core::ccs::convert::ForeignConversionInput::new(
        Path::new("verified-staging-1.0-1.x86_64.rpm").to_path_buf(),
        "verified-staging".to_string(),
        "1.0".to_string(),
        VersionScheme::Rpm,
    );
    metadata.source_authority = SourcePackageAuthority::Ccs(CcsPackageAuthority {
        name: "verified-staging".to_string(),
        version: "1.0".to_string(),
        version_scheme: VersionScheme::Rpm,
        architecture: Some("x86_64".to_string()),
        debian_multi_arch: None,
        capabilities: Vec::new(),
        config: Vec::new(),
    });
    metadata.files = vec![PackageFile {
        path: payload_path.clone(),
        node: PayloadNode::regular(0o755),
        content: Some(content_authority.clone()),
    }];
    let payload =
        conary_core::packages::PackagePayload::from_extracted_in_memory(vec![ExtractedFile {
            path: payload_path,
            node: PayloadNode::regular(0o755),
            content: payload_bytes.to_vec(),
            content_authority: Some(content_authority),
        }])
        .unwrap();
    conary_core::ccs::convert::NativePackageConverter::new(
        conary_core::ccs::convert::ConversionOptions {
            output_dir: root.to_path_buf(),
        },
    )
    .with_source_profile("fedora-44")
    .with_source_release("1")
    .with_conversion_tool("remi-storage-test")
    .with_signing_key(signer)
    .convert_payload(
        &metadata,
        payload.files(),
        "rpm",
        &conary_core::hash::Hash::new(conary_core::hash::HashAlgorithm::Sha256, "d".repeat(64))
            .unwrap(),
    )
    .unwrap()
}

fn stored_objects(objects_dir: &Path, count: usize) -> (Vec<CcsTransportObjectV1>, Vec<Vec<u8>>) {
    let cas = CasStore::new(objects_dir).unwrap();
    let payloads = (0..count)
        .map(|index| format!("parallel publication object {index}").into_bytes())
        .collect::<Vec<_>>();
    let objects = payloads
        .iter()
        .map(|payload| CcsTransportObjectV1 {
            sha256: cas.store(payload).unwrap(),
            size: payload.len() as u64,
        })
        .collect();
    (objects, payloads)
}

#[test]
fn calculated_sha256_is_typed_and_serializes_with_algorithm() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let file_path = temp_dir.path().join("test.pkg");
    std::fs::write(&file_path, b"hello world").unwrap();

    let checksum = ConversionService::calculate_sha256(&file_path).unwrap();
    assert_eq!(
        checksum.to_prefixed_string(),
        "sha256:b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
    );
}

#[test]
fn calculate_sha256_rejects_a_missing_file() {
    assert!(ConversionService::calculate_sha256(Path::new("/nonexistent/file.pkg")).is_err());
}

#[tokio::test]
async fn remi_verification_streams_once_into_cold_and_warm_permanent_cas() {
    let temp = tempfile::tempdir().unwrap();
    let signer = SigningKeyPair::generate().with_key_id("targets");
    let payload = b"direct verified CAS payload";
    let package = signed_package(temp.path(), &signer, payload);
    let (service, chunk_dir) = direct_cas_service(temp.path(), &signer);

    let cold = service
        .store_signed_ccs_path_with_timing(&package, "fedora-44")
        .await
        .unwrap();
    assert_eq!(cold.transport.objects.len(), 1);
    assert_eq!(cold.cas_metrics.misses, 1);
    assert_eq!(cold.cas_metrics.hits, 0);
    assert_eq!(cold.cas_metrics.incoming_bytes_hashed, payload.len() as u64);
    assert_eq!(
        cold.cas_metrics.persistent_bytes_written,
        payload.len() as u64
    );
    assert_eq!(cold.cas_metrics.objects_hashed, 1);
    assert_eq!(cold.cas_metrics.staged_data_barriers, 1);
    assert_eq!(cold.cas_metrics.canonical_name_barriers, 1);
    assert_eq!(cold.cas_metrics.canonical_bytes_reread, 0);
    let object = &cold.transport.objects[0];
    let cas = CasStore::new(chunk_dir.join("objects")).unwrap();
    assert_eq!(
        fs::read(cas.hash_to_path(&object.sha256).unwrap()).unwrap(),
        payload
    );
    let conn = crate::server::open_runtime_db(&service.db_path).unwrap();
    let persisted_size: i64 = conn
        .query_row(
            "SELECT size_bytes FROM chunk_access WHERE hash = ?1",
            [&object.sha256],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(persisted_size, payload.len() as i64);
    drop(conn);

    let warm = service
        .store_signed_ccs_path_with_timing(&package, "fedora-44")
        .await
        .unwrap();
    assert_eq!(warm.transport, cold.transport);
    assert_eq!(warm.cas_metrics.misses, 0);
    assert_eq!(warm.cas_metrics.hits, 1);
    assert_eq!(warm.cas_metrics.incoming_bytes_hashed, payload.len() as u64);
    assert_eq!(warm.cas_metrics.objects_hashed, 1);
    assert_eq!(warm.cas_metrics.persistent_bytes_written, 0);
    assert_eq!(warm.cas_metrics.staged_data_barriers, 0);
    assert_eq!(warm.cas_metrics.canonical_name_barriers, 0);
    assert_eq!(warm.cas_metrics.canonical_bytes_reread, 0);
}

#[tokio::test]
async fn remi_direct_cas_verification_rejects_an_untrusted_archive_before_bookkeeping() {
    let temp = tempfile::tempdir().unwrap();
    let trusted = SigningKeyPair::generate().with_key_id("targets");
    let untrusted = SigningKeyPair::generate().with_key_id("targets");
    let package = signed_package(temp.path(), &untrusted, b"untrusted payload");
    let (service, chunk_dir) = direct_cas_service(temp.path(), &trusted);

    let error = match service
        .store_signed_ccs_path_with_timing(&package, "fedora-44")
        .await
    {
        Ok(_) => panic!("untrusted signed archive was stored"),
        Err(error) => error,
    };
    assert!(format!("{error:#}").contains("signature"));
    assert_eq!(
        fs::read_dir(chunk_dir.join("objects"))
            .map(|entries| entries.count())
            .unwrap_or(0),
        0
    );
    let conn = crate::server::open_runtime_db(&service.db_path).unwrap();
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM chunk_access", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 0);
}

#[tokio::test]
async fn post_finalizer_same_size_mutation_fails_before_publication_bookkeeping() {
    let temp = tempfile::tempdir().unwrap();
    let signer = Arc::new(SigningKeyPair::generate().with_key_id("targets"));
    let authored = temp.path().join("authored");
    fs::create_dir(&authored).unwrap();
    let pending = pending_conversion(&authored, Arc::clone(&signer));
    let expected_bytes = pending.metrics().ccs_write.ccs_output_bytes;
    let expected_digest = pending.metrics().ccs_write.ccs_output_sha256.clone();
    let (service, _chunk_dir) = direct_cas_service(temp.path(), signer.as_ref());

    let error = match service
        .store_transport_with_timing_then(pending, "fedora-44", move |path| {
            let mut bytes = fs::read(path)?;
            ensure!(bytes.len() as u64 == expected_bytes);
            let offset = bytes.len() / 2;
            bytes[offset] ^= 0x01;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
            fs::write(path, &bytes)?;
            fs::set_permissions(path, fs::Permissions::from_mode(0o400))?;
            fs::File::open(path)?.sync_all()?;
            Ok(())
        })
        .await
    {
        Ok(_) => panic!("post-finalizer mutation was published"),
        Err(error) => error,
    };
    assert!(
        error.to_string().contains("changed after finalization"),
        "{error:#}"
    );

    let packages = service.cache_dir.join("packages");
    let canonical = packages.join(format!("{expected_digest}.ccs"));
    assert!(canonical.exists());
    assert_ne!(
        conary_core::hash::sha256(&fs::read(&canonical).unwrap()),
        expected_digest
    );
    assert_eq!(fs::read_dir(&packages).unwrap().count(), 1);
    let conn = crate::server::open_runtime_db(&service.db_path).unwrap();
    let chunk_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM chunk_access", [], |row| row.get(0))
        .unwrap();
    let conversion_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM converted_packages", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(chunk_rows, 0);
    assert_eq!(conversion_rows, 0);
}

#[tokio::test]
async fn r2_publication_is_bounded_and_accounts_for_out_of_order_work() {
    let temp = tempfile::TempDir::new().unwrap();
    let (objects, payloads) = stored_objects(temp.path(), 9);
    let hit_hashes = [objects[1].sha256.clone(), objects[7].sha256.clone()];
    let store = Arc::new(InjectedStore {
        hits: hit_hashes.into_iter().collect(),
        ..Default::default()
    });
    let bookkeeping_ran = Arc::new(AtomicBool::new(false));
    let object_count = objects.len();

    let (duration, work) =
        publish_transport_objects_then(Arc::clone(&store), temp.path(), &objects, 3, {
            let store = Arc::clone(&store);
            let bookkeeping_ran = Arc::clone(&bookkeeping_ran);
            move || async move {
                assert_eq!(store.current.load(Ordering::SeqCst), 0);
                assert_eq!(store.completed.load(Ordering::SeqCst), object_count);
                bookkeeping_ran.store(true, Ordering::SeqCst);
                Ok(())
            }
        })
        .await
        .unwrap();

    assert!(duration > Duration::ZERO);
    assert!(bookkeeping_ran.load(Ordering::SeqCst));
    assert!(store.maximum.load(Ordering::SeqCst) > 1);
    assert!(store.maximum.load(Ordering::SeqCst) <= 3);
    assert_eq!(work.head_requests, objects.len() as u64);
    assert_eq!(work.hits, 2);
    assert_eq!(work.misses, 7);
    assert_eq!(work.put_requests, 7);
    let expected_bytes = objects
        .iter()
        .zip(&payloads)
        .filter(|(object, _)| !store.hits.contains(&object.sha256))
        .map(|(_, payload)| payload.len() as u64)
        .sum::<u64>();
    assert_eq!(work.bytes_written, expected_bytes);
    assert_eq!(store.uploaded.lock().unwrap().len(), 7);
}

#[tokio::test]
async fn r2_publication_failure_prevents_bookkeeping() {
    let temp = tempfile::TempDir::new().unwrap();
    let (objects, _) = stored_objects(temp.path(), 6);
    let failed_hash = objects[3].sha256.clone();
    let store = Arc::new(InjectedStore {
        failed_put: Some(failed_hash.clone()),
        ..Default::default()
    });
    let bookkeeping_ran = Arc::new(AtomicBool::new(false));

    let result = publish_transport_objects_then(Arc::clone(&store), temp.path(), &objects, 2, {
        let bookkeeping_ran = Arc::clone(&bookkeeping_ran);
        move || async move {
            bookkeeping_ran.store(true, Ordering::SeqCst);
            Ok(())
        }
    })
    .await;

    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains(&format!("injected PUT failure for {failed_hash}"))
    );
    assert!(!bookkeeping_ran.load(Ordering::SeqCst));
    assert_eq!(store.current.load(Ordering::SeqCst), 0);
    assert_eq!(store.completed.load(Ordering::SeqCst), objects.len());
    assert!(store.maximum.load(Ordering::SeqCst) <= 2);
}
