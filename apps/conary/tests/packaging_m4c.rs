// apps/conary/tests/packaging_m4c.rs

use axum::body::{Body, to_bytes};
use axum::extract::Request;
use axum::http::{Method, StatusCode};
use axum::response::Response;
use conary_core::ccs::attestation::{
    BUILD_ATTESTATION_SCHEMA_V1, BuildAttestationPayload, BuildOutputIdentity, canonical_json_hash,
    compute_v2_content_identity, compute_v2_file_merkle_root, sign_build_attestation,
};
use conary_core::ccs::builder::write_v2_ccs_package;
use conary_core::ccs::signing::SigningKeyPair;
use conary_core::ccs::v2::schema::{
    AuthorityDocumentV2, ComponentAuthorityV2, ConflictPolicyV2, FORMAT_VERSION_V2,
    FileAuthorityV2, FileTypeV2, LifecycleAuthorityV2, PackageDataV2, PackageIdentityV2,
    PackageKindTagV2, PackageKindV2, PackagePolicyV2, ProvenanceAuthorityV2,
};
use conary_core::recipe::hermetic::{
    BuildInputIdentity, BuilderEnvironmentIdentity, BuilderEnvironmentKind, DependencyLock,
    DivergenceReport, EcosystemPolicyReport, HERMETIC_EVIDENCE_SCHEMA_V1, HermeticBuildEvidence,
    RecipeIdentity, ReproducibilityRecord, SourceIdentity,
};
use remi::server::config::{ReleasePublishSection, TrustedBuildAttestationSigner};
use remi::server::{ServerConfig, ServerState};
use rusqlite::params;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;
use tokio::sync::RwLock;
use tower::ServiceExt;

const TEST_DISTRO: &str = "fedora";
const TEST_PACKAGE: &str = "hello-m4c";
const TEST_VERSION: &str = "1.0.0";
const TEST_RELEASE: &str = "1";
const TEST_ARCH: &str = "x86_64";
const RELEASE_PUBLISH_POLICY_DIGEST: &str = "m2-static-publish-policy-v1";

#[tokio::test]
async fn remi_native_publication_fetches_and_installs_without_conversion_row() {
    let fixture = M4cFixture::new().await;
    let artifact = fixture.release_artifact();

    let publish = fixture.upload_release(artifact.bytes.clone()).await;
    assert_response_status(publish, StatusCode::CREATED).await;

    let metadata = fixture.package_metadata().await;
    assert_eq!(metadata["name"], TEST_PACKAGE);
    assert_eq!(metadata["version"], TEST_VERSION);
    assert_eq!(metadata["release"], TEST_RELEASE);
    assert_eq!(metadata["source_kind"], "native-ccs");
    assert_eq!(metadata["native"], true);
    assert_eq!(metadata["converted"], false);

    let downloaded = fixture.download_package().await;
    assert_eq!(downloaded, artifact.bytes);
    let downloaded_path = fixture.work.path().join("downloaded-native.ccs");
    fs::write(&downloaded_path, downloaded).unwrap();

    fixture.install_downloaded_package_dry_run(&downloaded_path);
    fixture.assert_no_converted_row();
    fixture.assert_repository_package_release_projection();
}

struct M4cFixture {
    work: tempfile::TempDir,
    state: Arc<RwLock<ServerState>>,
    public_app: axum::Router,
    db_path: PathBuf,
    install_db_path: PathBuf,
    install_root: PathBuf,
    signer: SigningKeyPair,
}

impl M4cFixture {
    async fn new() -> Self {
        let work = tempfile::tempdir().unwrap();
        let storage_root = work.path().join("storage");
        let db_path = storage_root.join("metadata/remi.db");
        let chunk_dir = storage_root.join("chunks");
        let cache_dir = storage_root.join("cache");
        let keys_dir = storage_root.join("keys");
        let install_db_path = work.path().join("install/conary.db");
        let install_root = work.path().join("install/root");
        fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        fs::create_dir_all(&chunk_dir).unwrap();
        fs::create_dir_all(&cache_dir).unwrap();
        fs::create_dir_all(install_db_path.parent().unwrap()).unwrap();
        fs::create_dir_all(&install_root).unwrap();
        conary_core::db::init(&db_path).unwrap();
        conary_core::db::init(&install_db_path).unwrap();
        write_tuf_role_keys(&keys_dir, TEST_DISTRO);

        let signer = SigningKeyPair::generate().with_key_id("publisher");
        let release_publish = ReleasePublishSection {
            repository_keys_dir: Some(keys_dir),
            trusted_build_attestation_signers: vec![TrustedBuildAttestationSigner {
                key_id: signer.key_id().unwrap().to_string(),
                public_key: signer.public_key_base64(),
            }],
        };
        let state = Arc::new(RwLock::new(
            ServerState::new(ServerConfig {
                db_path: db_path.clone(),
                chunk_dir,
                cache_dir,
                release_publish,
                enable_audit_log: false,
                enable_bloom_filter: false,
                enable_rate_limit: false,
                ..ServerConfig::default()
            })
            .expect("test server state"),
        ));
        let public_app = remi::server::create_router(state.clone()).await;

        Self {
            work,
            state,
            public_app,
            db_path,
            install_db_path,
            install_root,
            signer,
        }
    }

    fn release_artifact(&self) -> TestArtifact {
        release_artifact_with_attestation(
            &self.signer,
            TEST_PACKAGE,
            TEST_VERSION,
            TEST_RELEASE,
            b"hello from m4c\n",
        )
    }

    async fn upload_release(&self, bytes: Vec<u8>) -> Response {
        let request = Request::builder()
            .method(Method::POST)
            .uri(format!("/v1/admin/releases/{TEST_DISTRO}"))
            .body(Body::from(bytes))
            .unwrap();
        remi::server::release_publish::handle_release_upload(
            self.state.clone(),
            TEST_DISTRO.to_string(),
            request,
        )
        .await
    }

    async fn package_metadata(&self) -> Value {
        let response = self
            .public_app
            .clone()
            .oneshot(public_request(&format!(
                "/v1/{TEST_DISTRO}/packages/{TEST_PACKAGE}?version={TEST_VERSION}&release={TEST_RELEASE}&arch={TEST_ARCH}"
            )))
            .await
            .unwrap();
        let status = response.status();
        let body = response_bytes(response).await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        serde_json::from_slice(&body).unwrap()
    }

    async fn download_package(&self) -> Vec<u8> {
        let response = self
            .public_app
            .clone()
            .oneshot(public_request(&format!(
                "/v1/{TEST_DISTRO}/packages/{TEST_PACKAGE}/download?version={TEST_VERSION}&release={TEST_RELEASE}&arch={TEST_ARCH}"
            )))
            .await
            .unwrap();
        let status = response.status();
        let body = response_bytes(response).await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        body
    }

    fn install_downloaded_package_dry_run(&self, package_path: &Path) {
        let policy_path = self.work.path().join("release-policy.toml");
        fs::write(
            &policy_path,
            format!(
                "trusted_keys = [\"{}\"]\nallow_unsigned = false\n",
                self.signer.public_key_base64()
            ),
        )
        .unwrap();

        let output = Command::new(env!("CARGO_BIN_EXE_conary"))
            .arg("ccs")
            .arg("install")
            .arg(package_path)
            .arg("--db-path")
            .arg(&self.install_db_path)
            .arg("--root")
            .arg(&self.install_root)
            .arg("--sandbox")
            .arg("never")
            .arg("--dry-run")
            .arg("--no-deps")
            .arg("--policy")
            .arg(&policy_path)
            .output()
            .expect("run conary ccs install");
        assert_success(&output);
    }

    fn assert_no_converted_row(&self) {
        let conn = rusqlite::Connection::open(&self.db_path).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM converted_packages
                 WHERE distro = ?1 AND package_name = ?2",
                params![TEST_DISTRO, TEST_PACKAGE],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    fn assert_repository_package_release_projection(&self) {
        let conn = rusqlite::Connection::open(&self.db_path).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM repository_packages rp
                 JOIN repositories r ON rp.repository_id = r.id
                 WHERE r.name = ?1
                   AND rp.name = ?2
                   AND rp.version = ?3
                   AND rp.package_release = ?4
                   AND rp.architecture = ?5",
                params![
                    TEST_DISTRO,
                    TEST_PACKAGE,
                    TEST_VERSION,
                    TEST_RELEASE,
                    TEST_ARCH
                ],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }
}

fn public_request(uri: &str) -> Request {
    let mut request = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(SocketAddr::from((
            [127, 0, 0, 1],
            49152,
        ))));
    request
}

async fn assert_response_status(response: Response, expected: StatusCode) {
    let status = response.status();
    let body = response_text(response).await;
    assert_eq!(status, expected, "{body}");
}

async fn response_text(response: Response) -> String {
    String::from_utf8_lossy(&response_bytes(response).await).into_owned()
}

async fn response_bytes(response: Response) -> Vec<u8> {
    to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "expected command to succeed\n{}",
        output_text(output)
    );
}

fn output_text(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

struct TestArtifact {
    bytes: Vec<u8>,
}

fn release_artifact_with_attestation(
    signer: &SigningKeyPair,
    name: &str,
    version: &str,
    release: &str,
    payload: &[u8],
) -> TestArtifact {
    let temp = tempfile::tempdir().unwrap();
    let evidence = sample_hermetic_evidence(name, version);
    let hermetic_evidence_hash = canonical_json_hash(&evidence).unwrap();
    let payload_path = "/usr/share/m4c-payload".to_string();
    let payload_hash = conary_core::hash::sha256(payload);
    let authority = AuthorityDocumentV2 {
        format_version: FORMAT_VERSION_V2,
        identity: PackageIdentityV2 {
            name: name.to_string(),
            version: version.to_string(),
            release: release.to_string(),
            architecture: Some(TEST_ARCH.to_string()),
            platform: Some("linux".to_string()),
            kind: PackageKindTagV2::Package,
        },
        kind: PackageKindV2::Package(PackageDataV2 {
            files: vec![FileAuthorityV2 {
                path: payload_path.clone(),
                sha256: payload_hash,
                size: payload.len() as u64,
                file_type: FileTypeV2::Regular,
                mode: 0o644,
                owner: "root".to_string(),
                group: "root".to_string(),
                component: "main".to_string(),
                symlink_target: None,
                config: None,
                conflict: ConflictPolicyV2::Error,
            }],
            config: Vec::new(),
            policy: PackagePolicyV2::default(),
        }),
        provides: Vec::new(),
        requires: Vec::new(),
        components: BTreeMap::from([(
            "main".to_string(),
            ComponentAuthorityV2 {
                name: "main".to_string(),
                default: true,
                file_count: 1,
                total_size: payload.len() as u64,
            },
        )]),
        lifecycle: LifecycleAuthorityV2::default(),
        provenance: ProvenanceAuthorityV2 {
            origin_class: Some("native-built".to_string()),
            hardening_level: Some("hermetic".to_string()),
            build_input_identity: Some("sha256:build-input".to_string()),
            hermetic_evidence_hash: Some(hermetic_evidence_hash.clone()),
            foreign_conversion_boundary_hash: None,
        },
        debug_toml_sha256: None,
    };
    let output_identity = BuildOutputIdentity {
        file_merkle_root: compute_v2_file_merkle_root(&authority).unwrap(),
        package_name: name.to_string(),
        package_version: version.to_string(),
        package_release: release.to_string(),
        architecture: Some(TEST_ARCH.to_string()),
        origin_class: "native-built".to_string(),
        hardening_level: "hermetic".to_string(),
        hermetic_evidence_hash: hermetic_evidence_hash.clone(),
        canonical_content_identity: compute_v2_content_identity(&authority).unwrap(),
    };
    let payloads = BTreeMap::from([(payload_path, payload.to_vec())]);
    let attestation = BuildAttestationPayload {
        schema_version: BUILD_ATTESTATION_SCHEMA_V1,
        origin_class: output_identity.origin_class.clone(),
        hardening_level: output_identity.hardening_level.clone(),
        build_input: evidence.build_input.clone(),
        dependency_lock: evidence.dependency_lock.clone(),
        hermetic_evidence_hash,
        output_identity,
        build_command_risk_report_hash: canonical_json_hash(&evidence.command_risk).unwrap(),
        scriptlet_risk_report_hash: None,
        conversion_boundary_hash: None,
        publish_policy_digest: RELEASE_PUBLISH_POLICY_DIGEST.to_string(),
        command_risk_classifier_version: evidence.command_risk.classifier_version.clone(),
        sandbox_profile: "kitchen-pristine-network-none".to_string(),
        seccomp_profile: Some("scriptlet-v1".to_string()),
        builder_identity: "m4c-integration-test-builder".to_string(),
        conary_version: "test".to_string(),
        issued_at: "2026-06-18T00:00:00Z".to_string(),
    };
    let envelope = sign_build_attestation(attestation, signer).unwrap();
    let package_path = temp.path().join("release.ccs");
    write_v2_ccs_package(
        &authority,
        &payloads,
        &package_path,
        signer,
        None,
        Some(&envelope),
        None,
    )
    .unwrap();

    TestArtifact {
        bytes: fs::read(package_path).unwrap(),
    }
}

fn sample_hermetic_evidence(name: &str, version: &str) -> HermeticBuildEvidence {
    HermeticBuildEvidence {
        schema_version: HERMETIC_EVIDENCE_SCHEMA_V1,
        build_input: BuildInputIdentity {
            recipe: RecipeIdentity::GeneratedRecipe {
                generator: "m4c-integration-test".to_string(),
                canonical_hash: conary_core::hash::sha256_prefixed(
                    format!("{name}:{version}").as_bytes(),
                ),
                inference_trace_hash: conary_core::hash::sha256_prefixed(b"test"),
            },
            source: SourceIdentity::Archive {
                url: "https://example.invalid/source.tar.gz".to_string(),
                checksum: "sha256:source".to_string(),
            },
            additional_sources: Vec::new(),
            patches: Vec::new(),
            local_tree: None,
            ecosystem_dependencies: Vec::new(),
            builder_environment: BuilderEnvironmentIdentity {
                kind: BuilderEnvironmentKind::Pristine,
                sysroot_hash: Some("sha256:sysroot".to_string()),
                toolchain_hash: None,
                diagnostics: Vec::new(),
            },
        },
        dependency_lock: DependencyLock::default(),
        ecosystem_policy: EcosystemPolicyReport::clean("test"),
        command_risk: conary_core::recipe::hermetic::BuildCommandRiskReport::clean(),
        reproducibility: ReproducibilityRecord {
            source_date_epoch: Some(1),
            path_remap_count: 1,
            env_keys: vec!["SOURCE_DATE_EPOCH".to_string()],
        },
        divergence: DivergenceReport::default(),
        diagnostics: Vec::new(),
    }
}

fn write_tuf_role_keys(keys_dir: &Path, distro: &str) {
    let distro_dir = keys_dir.join(distro);
    fs::create_dir_all(&distro_dir).unwrap();
    for role in ["targets", "snapshot", "timestamp"] {
        SigningKeyPair::generate()
            .with_key_id(role)
            .save_to_files(
                &distro_dir.join(format!("{role}.private")),
                &distro_dir.join(format!("{role}.public")),
            )
            .unwrap();
    }
}
