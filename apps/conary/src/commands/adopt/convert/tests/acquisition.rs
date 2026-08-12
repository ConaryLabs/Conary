// apps/conary/src/commands/adopt/convert/tests/acquisition.rs

use super::super::*;
use conary_core::db::models::{
    NativeSourceEcosystem, NativeSourceStream, RepositoryPolicyScope, RepositorySourcePolicy,
    RepositoryUpdateMode,
};
use conary_core::repository::trust::openpgp::PreparedOpenPgpTrust;
use conary_core::repository::versioning::VersionScheme;
use conary_core::repository::{
    ArchKeyringFormat, ArchKeyringTrust, ArchSigLevel, OpenPgpTrustRoot, RepositoryParserConfig,
    RepositoryTrustPolicy, RpmMetadataAuthority,
};
use sequoia_openpgp as openpgp;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct PgpAuthority {
    certificate: openpgp::Cert,
    root: OpenPgpTrustRoot,
}

fn pgp_authority(directory: &Path) -> PgpAuthority {
    use openpgp::cert::prelude::CertBuilder;
    use openpgp::serialize::Serialize;

    let (certificate, _) = CertBuilder::new()
        .add_userid("Exact Artifact Test Signer")
        .add_signing_subkey()
        .generate()
        .unwrap();
    let public = certificate.clone().strip_secret_key_material();
    let mut bytes = Vec::new();
    public.serialize(&mut bytes).unwrap();
    let path = directory.join("repository-signing-key.pgp");
    std::fs::write(&path, bytes).unwrap();
    PgpAuthority {
        root: OpenPgpTrustRoot {
            url: format!("file://{}", path.display()),
            fingerprint: certificate.fingerprint().to_hex(),
        },
        certificate,
    }
}

fn detached_signature(certificate: &openpgp::Cert, data: &[u8]) -> Vec<u8> {
    use openpgp::policy::StandardPolicy;
    use openpgp::serialize::stream::{Message, Signer};

    let policy = StandardPolicy::new();
    let keypair = certificate
        .keys()
        .unencrypted_secret()
        .with_policy(&policy, None)
        .supported()
        .alive()
        .revoked(false)
        .for_signing()
        .next()
        .unwrap()
        .key()
        .clone()
        .into_keypair()
        .unwrap();
    let mut signature = Vec::new();
    let message = Message::new(&mut signature);
    let mut signer = Signer::new(message, keypair)
        .unwrap()
        .detached()
        .build()
        .unwrap();
    signer.write_all(data).unwrap();
    signer.finalize().unwrap();
    signature
}

fn signed_rpm_bytes(authority: &PgpAuthority) -> Vec<u8> {
    let mut builder = rpm::PackageBuilder::new(
        "exact",
        "1.2.3",
        "MIT",
        "x86_64",
        "exact acquisition fixture",
    );
    builder.release("4");
    builder
        .with_file_contents(
            b"exact rpm payload".to_vec(),
            rpm::FileOptions::new("/usr/bin/exact").permissions(0o755),
        )
        .unwrap();
    let mut package = builder.build().unwrap();
    let signature = detached_signature(&authority.certificate, &package.header_bytes().unwrap());
    package.apply_signature(signature).unwrap();
    let mut bytes = Vec::new();
    package.write(&mut bytes).unwrap();
    bytes
}

async fn serve_artifact(
    package: Vec<u8>,
    signature: Option<Vec<u8>>,
) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&requests);
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let package = package.clone();
            let signature = signature.clone();
            let observed = Arc::clone(&observed);
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut buffer = [0_u8; 1024];
                loop {
                    let count = stream.read(&mut buffer).await.unwrap();
                    if count == 0 {
                        return;
                    }
                    request.extend_from_slice(&buffer[..count]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                observed.fetch_add(1, Ordering::SeqCst);
                let request = String::from_utf8_lossy(&request);
                let wants_signature = request
                    .lines()
                    .next()
                    .is_some_and(|line| line.contains("/exact.pkg.sig"));
                let body = if wants_signature {
                    signature.as_deref()
                } else {
                    Some(package.as_slice())
                };
                let (status, body) = body
                    .map(|body| ("200 OK", body))
                    .unwrap_or(("404 Not Found", &[]));
                let headers = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(headers.as_bytes()).await.unwrap();
                stream.write_all(body).await.unwrap();
            });
        }
    });
    (format!("http://{address}/exact.pkg"), requests, task)
}

fn arch_keyring() -> ArchKeyringTrust {
    let keyring = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/conary-core/tests/fixtures/arch/archlinux-keyring-corpus.pkg.tar.zst")
        .canonicalize()
        .unwrap();
    ArchKeyringTrust {
        url: format!("file://{}", keyring.display()),
        format: ArchKeyringFormat::AlpmPackageZstd,
        master_fingerprints: vec![
            "2AC0A42EFB0B5CBC7A0402ED4DC95B6D7BE9892E".to_string(),
            "3572FA2A1B067F22C58AF155F8B821B42A6FDCD7".to_string(),
            "69E6471E3AE065297529832E6BA0F5A2037F4F41".to_string(),
            "99B6618472A3B3B814185BAED7D3D823B88BDB9B".to_string(),
            "D8AFDDA07A5B6EDFA7D8CCDAD6D055F927843F1C".to_string(),
        ],
        packager_key_threshold: 3,
    }
}

fn configured_source(
    scheme: VersionScheme,
    url: String,
    package: &[u8],
    pgp: Option<&PgpAuthority>,
) -> PackageWithRepo {
    let ecosystem = match scheme {
        VersionScheme::Rpm => NativeSourceEcosystem::Rpm,
        VersionScheme::Debian => NativeSourceEcosystem::Deb,
        VersionScheme::Arch => NativeSourceEcosystem::Alpm,
        other => panic!("unsupported native test scheme {other:?}"),
    };
    let mut repository = Repository::new(
        format!("exact-{}", scheme.as_str()),
        "https://metadata.example.test/repository".to_string(),
    );
    match ecosystem {
        NativeSourceEcosystem::Rpm => {
            let root = pgp.unwrap().root.clone();
            repository
                .set_parser_config(RepositoryParserConfig::Rpm {
                    architecture: "x86_64".to_string(),
                })
                .unwrap();
            repository
                .set_trust_policy(RepositoryTrustPolicy::Rpm {
                    metadata: RpmMetadataAuthority::OpenPgp {
                        keys: vec![root.clone()],
                    },
                    package_keys: vec![root],
                })
                .unwrap();
        }
        NativeSourceEcosystem::Deb => {
            repository
                .set_parser_config(RepositoryParserConfig::Deb {
                    distribution: "stable".to_string(),
                    component: "main".to_string(),
                    architecture: "x86_64".to_string(),
                })
                .unwrap();
            repository
                .set_trust_policy(RepositoryTrustPolicy::Debian {
                    release_keys: vec![pgp.unwrap().root.clone()],
                })
                .unwrap();
        }
        NativeSourceEcosystem::Alpm => {
            repository
                .set_parser_config(RepositoryParserConfig::Arch {
                    database: "core".to_string(),
                })
                .unwrap();
            repository
                .set_trust_policy(RepositoryTrustPolicy::Arch {
                    keyring: arch_keyring(),
                    sig_level: ArchSigLevel::distribution_default(),
                })
                .unwrap();
        }
    }
    let identity = format!("test:{}", repository.name);
    let policy = RepositorySourcePolicy::new(
        "test-source",
        RepositoryPolicyScope::repository(identity.clone()).unwrap(),
        ecosystem,
        NativeSourceStream::channel("stable").unwrap(),
        RepositoryUpdateMode::Follow,
    )
    .unwrap();
    repository
        .set_native_source_policy(policy, identity, None)
        .unwrap();
    let mut package_row = RepositoryPackage::new(
        1,
        "exact".to_string(),
        "1.2.3-4".to_string(),
        scheme,
        conary_core::hash::sha256(package),
        i64::try_from(package.len()).unwrap(),
        url,
    );
    package_row.architecture = Some("x86_64".to_string());
    PackageWithRepo {
        repository,
        package: package_row,
    }
}

#[tokio::test]
async fn repository_fetch_then_offline_cache_hit_cover_all_native_formats() {
    for scheme in [
        VersionScheme::Rpm,
        VersionScheme::Debian,
        VersionScheme::Arch,
    ] {
        let temp = tempfile::tempdir().unwrap();
        let pgp = pgp_authority(temp.path());
        let (package, signature) = match scheme {
            VersionScheme::Rpm => (signed_rpm_bytes(&pgp), None),
            VersionScheme::Debian => (b"authenticated Debian package bytes".to_vec(), None),
            VersionScheme::Arch => {
                let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join(
                    "../../crates/conary-core/tests/fixtures/arch/packages/base-3-3-any.pkg.tar.zst",
                );
                (
                    std::fs::read(&fixture).unwrap(),
                    Some(std::fs::read(fixture.with_extension("zst.sig")).unwrap()),
                )
            }
            other => panic!("unsupported native test scheme {other:?}"),
        };
        let (url, requests, server) = serve_artifact(package.clone(), signature).await;
        let source = configured_source(scheme, url, &package, Some(&pgp));
        let db_path = temp.path().join("runtime/conary.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        PreparedOpenPgpTrust::prepare(
            &source.repository.name,
            &conary_core::db::paths::keyring_dir(db_path.to_str().unwrap()),
            source.repository.require_trust_policy().unwrap(),
        )
        .await
        .unwrap();

        let first = acquire_exact_artifact(db_path.to_str().unwrap(), &source)
            .await
            .unwrap();
        assert_eq!(std::fs::read(&first.path).unwrap(), package);
        let requests_after_fetch = requests.load(Ordering::SeqCst);
        assert!(requests_after_fetch >= 1);

        let second = acquire_exact_artifact(db_path.to_str().unwrap(), &source)
            .await
            .unwrap();
        assert_eq!(second.path, first.path);
        assert_eq!(
            requests.load(Ordering::SeqCst),
            requests_after_fetch,
            "{scheme:?} cache hit unexpectedly required repository network authority"
        );
        server.abort();
    }
}
