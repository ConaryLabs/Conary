// conary-core/src/repository/declarations/takeover/conformance_tests.rs

use super::*;
use crate::db::models::AuthenticatedSnapshotIdentity;
use crate::repository::{
    ArchKeyringFormat, ArchKeyringTrust, ArchSigLevel, ArchSignatureRequirement, ArchTrustLevel,
};
use openpgp::cert::prelude::CertBuilder;
use openpgp::serialize::Serialize;
use sequoia_openpgp as openpgp;

fn certificate() -> (Vec<u8>, String) {
    let (certificate, _) = CertBuilder::new()
        .add_userid("Distro takeover conformance fixture")
        .generate()
        .unwrap();
    let fingerprint = certificate.fingerprint().to_hex();
    let mut bytes = Vec::new();
    certificate.serialize(&mut bytes).unwrap();
    (bytes, fingerprint)
}

fn alpm_fixture(
    sig_level: &str,
) -> (
    tempfile::TempDir,
    Connection,
    NativeRepositoryTakeoverManifest,
) {
    alpm_named_fixture("cachyos", "cachyos:rolling:x86_64", sig_level, 30)
}

fn alpm_named_fixture(
    repository_name: &str,
    source_identity: &str,
    sig_level: &str,
    priority: i32,
) -> (
    tempfile::TempDir,
    Connection,
    NativeRepositoryTakeoverManifest,
) {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("etc")).unwrap();
    fs::write(
        root.path().join("etc/pacman.conf"),
        format!(
            "[options]\nArchitecture = auto\nSigLevel = {sig_level}\n\n[{repository_name}]\nServer = https://mirror.example/{repository_name}/$arch/$repo\nUsage = Sync Search Install Upgrade\n"
        ),
    )
    .unwrap();
    let db_path = root.path().join("conary.db");
    crate::db::init(&db_path).unwrap();
    let conn = crate::db::open(&db_path).unwrap();
    let declarations = discover_selected_root(root.path()).unwrap();
    let reference = plan_selected_root_trust(&declarations).repositories[0]
        .repository
        .clone();
    let manifest = NativeRepositoryTakeoverManifest {
        schema: TAKEOVER_MANIFEST_SCHEMA,
        repositories: vec![TakeoverRepositoryInput {
            declaration: reference,
            name: repository_name.to_string(),
            source_identity: source_identity.to_string(),
            repository_identity: format!("{repository_name}:x86_64"),
            scope: TakeoverPolicyScope::Repository {
                identity: format!("{repository_name}:x86_64"),
            },
            stream: TakeoverSourceStream::Rolling {
                identity: repository_name.to_string(),
            },
            update: TakeoverUpdatePolicy::Follow,
            metadata_url: format!(
                "https://mirror.example/{repository_name}/x86_64/{repository_name}"
            ),
            content_url: None,
            parser: RepositoryParserConfig::Arch {
                database: "cachyos".to_string(),
            },
            trust: RepositoryTrustPolicy::Arch {
                keyring: ArchKeyringTrust {
                    url: format!("https://keys.example/{repository_name}-keyring.pkg.tar.zst"),
                    format: ArchKeyringFormat::AlpmPackageZstd,
                    master_fingerprints: vec!["A".repeat(40)],
                    packager_key_threshold: 1,
                },
                sig_level: ArchSigLevel::distribution_default(),
            },
            enabled: true,
            priority,
            metadata_expire: 3600,
            source_profile: None,
        }],
    };
    (root, conn, manifest)
}

fn apt_derivative_fixture(
    source_identity: &str,
    repository_name: &str,
) -> (
    tempfile::TempDir,
    Connection,
    NativeRepositoryTakeoverManifest,
) {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("etc/apt/sources.list.d")).unwrap();
    fs::create_dir_all(root.path().join("etc/apt/keyrings")).unwrap();
    let (certificate, _) = certificate();
    fs::write(
        root.path().join("etc/apt/keyrings/derivative.gpg"),
        certificate,
    )
    .unwrap();
    fs::write(
        root.path()
            .join("etc/apt/sources.list.d/derivative.sources"),
        "Types: deb\nURIs: https://packages.example/debian\nSuites: stable\nComponents: main\nEnabled: yes\nSigned-By: /etc/apt/keyrings/derivative.gpg\n",
    )
    .unwrap();
    let db_path = root.path().join("conary.db");
    crate::db::init(&db_path).unwrap();
    let conn = crate::db::open(&db_path).unwrap();
    let declarations = discover_selected_root(root.path()).unwrap();
    let trust = plan_selected_root_trust(&declarations);
    let reference = trust.repositories[0].repository.clone();
    let policy = expected_trust_policy(
        root.path(),
        &declarations,
        &trust.repositories[0],
        RepositoryFormat::Debian,
    )
    .unwrap()
    .unwrap();
    let repository_identity = format!("{repository_name}:amd64");
    let manifest = NativeRepositoryTakeoverManifest {
        schema: TAKEOVER_MANIFEST_SCHEMA,
        repositories: vec![TakeoverRepositoryInput {
            declaration: reference,
            name: repository_name.to_string(),
            source_identity: source_identity.to_string(),
            repository_identity: repository_identity.clone(),
            scope: TakeoverPolicyScope::Repository {
                identity: repository_identity,
            },
            stream: TakeoverSourceStream::Release {
                identity: "stable".to_string(),
            },
            update: TakeoverUpdatePolicy::Follow,
            metadata_url: "https://packages.example/debian".to_string(),
            content_url: None,
            parser: RepositoryParserConfig::Deb {
                distribution: "stable".to_string(),
                component: "main".to_string(),
                architecture: "amd64".to_string(),
            },
            trust: policy,
            enabled: true,
            priority: 50,
            metadata_expire: 3600,
            source_profile: None,
        }],
    };
    (root, conn, manifest)
}

fn tumbleweed_fixture() -> (
    tempfile::TempDir,
    Connection,
    NativeRepositoryTakeoverManifest,
) {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("etc/zypp/repos.d")).unwrap();
    fs::create_dir_all(root.path().join("etc/pki/rpm-gpg")).unwrap();
    let (certificate, _) = certificate();
    fs::write(root.path().join("etc/pki/rpm-gpg/TUMBLEWEED"), certificate).unwrap();
    let declaration_path = root.path().join("etc/zypp/repos.d/tumbleweed.repo");
    fs::write(
        &declaration_path,
        "[repo-oss]\nname=Main Repository (OSS)\nenabled=1\nautorefresh=1\nbaseurl=https://download.example/tumbleweed/repo/oss/\npriority=47\ngpgcheck=1\nrepo_gpgcheck=1\npkg_gpgcheck=1\ngpgkey=file:///etc/pki/rpm-gpg/TUMBLEWEED\n",
    )
    .unwrap();
    let db_path = root.path().join("conary.db");
    crate::db::init(&db_path).unwrap();
    let conn = crate::db::open(&db_path).unwrap();
    let declarations = discover_selected_root(root.path()).unwrap();
    let trust = plan_selected_root_trust(&declarations);
    let reference = trust.repositories[0].repository.clone();
    let policy = expected_trust_policy(
        root.path(),
        &declarations,
        &trust.repositories[0],
        RepositoryFormat::Fedora,
    )
    .unwrap()
    .unwrap();
    let manifest = NativeRepositoryTakeoverManifest {
        schema: TAKEOVER_MANIFEST_SCHEMA,
        repositories: vec![TakeoverRepositoryInput {
            declaration: reference,
            name: "tumbleweed-oss".to_string(),
            source_identity: "tumbleweed:rolling:x86_64".to_string(),
            repository_identity: "tumbleweed:oss:x86_64".to_string(),
            scope: TakeoverPolicyScope::Repository {
                identity: "tumbleweed:oss:x86_64".to_string(),
            },
            stream: TakeoverSourceStream::Rolling {
                identity: "tumbleweed".to_string(),
            },
            update: TakeoverUpdatePolicy::Follow,
            metadata_url: "https://download.example/tumbleweed/repo/oss/".to_string(),
            content_url: None,
            parser: RepositoryParserConfig::Rpm {
                architecture: "x86_64".to_string(),
            },
            trust: policy,
            enabled: true,
            priority: 47,
            metadata_expire: 3600,
            source_profile: None,
        }],
    };
    (root, conn, manifest)
}

#[test]
fn exact_alpm_keyring_binding_resolves_only_the_native_keyring_ambiguity() {
    let (root, conn, manifest) = alpm_fixture("Required DatabaseOptional");

    let preview = preview_native_repository_takeover(&conn, root.path(), &manifest).unwrap();
    assert!(
        matches!(
            preview.trust.repositories[0].disposition,
            TrustImportDisposition::Ambiguous { ref findings }
                if findings.iter().all(|finding| finding.kind
                    == TrustImportFindingKind::AlpmKeyringBindingMissing)
        ),
        "preview must retain the native ambiguity as evidence"
    );
    assert!(preview.blockers.is_empty(), "{:?}", preview.blockers);

    let applied =
        apply_native_repository_takeover(&conn, root.path(), &manifest, &preview.sha256().unwrap())
            .unwrap();
    assert_eq!(applied.status, TakeoverApplyStatus::Applied);
    let stored = Repository::find_by_name(&conn, "cachyos")
        .unwrap()
        .expect("takeover repository");
    assert_eq!(stored.managed_by, RepositoryOwnership::NativeProjection);
    assert_eq!(
        stored.source_policy.expect("source policy").stream.kind(),
        "rolling"
    );
}

#[test]
fn arch_derivatives_share_one_alpm_takeover_contract() {
    for (repository_name, source_identity, priority) in [
        ("manjaro-core", "manjaro:rolling:x86_64", 10),
        ("cachyos", "cachyos:rolling:x86_64", 20),
        ("artix-system", "artix:rolling:x86_64", 30),
    ] {
        let (root, conn, manifest) = alpm_named_fixture(
            repository_name,
            source_identity,
            "Required DatabaseOptional",
            priority,
        );
        let declaration_path = root.path().join("etc/pacman.conf");
        let declaration_before = fs::read(&declaration_path).unwrap();

        let preview = preview_native_repository_takeover(&conn, root.path(), &manifest).unwrap();
        assert!(preview.blockers.is_empty(), "{:?}", preview.blockers);
        apply_native_repository_takeover(&conn, root.path(), &manifest, &preview.sha256().unwrap())
            .unwrap();

        let stored = Repository::find_by_name(&conn, repository_name)
            .unwrap()
            .expect("derivative repository");
        assert_eq!(stored.priority, priority);
        assert!(stored.enabled);
        assert_eq!(
            stored.source_policy.unwrap().source_identity,
            source_identity
        );
        assert_eq!(fs::read(declaration_path).unwrap(), declaration_before);
    }
}

#[test]
fn debian_derivatives_share_one_apt_takeover_contract() {
    for (source_identity, repository_name) in [
        ("mint:stable:amd64", "mint-main"),
        ("mx:stable:amd64", "mx-main"),
    ] {
        let (root, conn, manifest) = apt_derivative_fixture(source_identity, repository_name);
        let declaration_path = root
            .path()
            .join("etc/apt/sources.list.d/derivative.sources");
        let key_path = root.path().join("etc/apt/keyrings/derivative.gpg");
        let declaration_before = fs::read(&declaration_path).unwrap();
        let key_before = fs::read(&key_path).unwrap();

        let preview = preview_native_repository_takeover(&conn, root.path(), &manifest).unwrap();
        assert!(preview.blockers.is_empty(), "{:?}", preview.blockers);
        apply_native_repository_takeover(&conn, root.path(), &manifest, &preview.sha256().unwrap())
            .unwrap();

        let stored = Repository::find_by_name(&conn, repository_name)
            .unwrap()
            .expect("APT derivative repository");
        assert_eq!(
            stored.source_policy.unwrap().source_identity,
            source_identity
        );
        assert!(stored.enabled);
        assert_eq!(fs::read(declaration_path).unwrap(), declaration_before);
        assert_eq!(fs::read(key_path).unwrap(), key_before);
    }
}

#[test]
fn tumbleweed_preserves_zypper_authority_and_advances_rolling_snapshots() {
    let (root, conn, manifest) = tumbleweed_fixture();
    let declaration_path = root.path().join("etc/zypp/repos.d/tumbleweed.repo");
    let declaration_before = fs::read(&declaration_path).unwrap();
    let preview = preview_native_repository_takeover(&conn, root.path(), &manifest).unwrap();
    assert!(preview.blockers.is_empty(), "{:?}", preview.blockers);

    apply_native_repository_takeover(&conn, root.path(), &manifest, &preview.sha256().unwrap())
        .unwrap();
    let mut stored = Repository::find_by_name(&conn, "tumbleweed-oss")
        .unwrap()
        .expect("Tumbleweed repository");
    assert_eq!(stored.priority, 47);
    assert_eq!(
        stored.source_policy.as_ref().unwrap().stream.kind(),
        "rolling"
    );
    let first = AuthenticatedSnapshotIdentity::for_bytes(b"tumbleweed-snapshot-one");
    let second = AuthenticatedSnapshotIdentity::for_bytes(b"tumbleweed-snapshot-two");
    stored.admit_authenticated_snapshot(first).unwrap();
    stored.admit_authenticated_snapshot(second.clone()).unwrap();
    stored.update(&conn).unwrap();

    assert_eq!(
        Repository::find_by_name(&conn, "tumbleweed-oss")
            .unwrap()
            .unwrap()
            .authenticated_snapshot,
        Some(second)
    );
    assert_eq!(fs::read(declaration_path).unwrap(), declaration_before);
}

#[test]
fn cachyos_follow_advances_and_pin_requires_explicit_reenrollment() {
    let (root, conn, mut manifest) = alpm_fixture("Required DatabaseOptional");
    let preview = preview_native_repository_takeover(&conn, root.path(), &manifest).unwrap();
    apply_native_repository_takeover(&conn, root.path(), &manifest, &preview.sha256().unwrap())
        .unwrap();
    let first = AuthenticatedSnapshotIdentity::for_bytes(b"cachyos-snapshot-one");
    let second = AuthenticatedSnapshotIdentity::for_bytes(b"cachyos-snapshot-two");
    let mut following = Repository::find_by_name(&conn, "cachyos").unwrap().unwrap();
    following
        .admit_authenticated_snapshot(first.clone())
        .unwrap();
    following
        .admit_authenticated_snapshot(second.clone())
        .unwrap();
    following.update(&conn).unwrap();
    assert_eq!(
        Repository::find_by_name(&conn, "cachyos")
            .unwrap()
            .unwrap()
            .authenticated_snapshot,
        Some(second.clone())
    );

    rollback_native_repository_takeover(&conn, root.path()).unwrap();
    manifest.repositories[0].update = TakeoverUpdatePolicy::Pin {
        snapshot_sha256: first.sha256().to_string(),
    };
    let preview = preview_native_repository_takeover(&conn, root.path(), &manifest).unwrap();
    apply_native_repository_takeover(&conn, root.path(), &manifest, &preview.sha256().unwrap())
        .unwrap();
    let mut pinned = Repository::find_by_name(&conn, "cachyos").unwrap().unwrap();
    pinned.admit_authenticated_snapshot(first.clone()).unwrap();
    pinned.update(&conn).unwrap();
    assert!(pinned.admit_authenticated_snapshot(second.clone()).is_err());
    assert_eq!(
        Repository::find_by_name(&conn, "cachyos")
            .unwrap()
            .unwrap()
            .authenticated_snapshot,
        Some(first)
    );

    rollback_native_repository_takeover(&conn, root.path()).unwrap();
    manifest.repositories[0].update = TakeoverUpdatePolicy::Pin {
        snapshot_sha256: second.sha256().to_string(),
    };
    let preview = preview_native_repository_takeover(&conn, root.path(), &manifest).unwrap();
    apply_native_repository_takeover(&conn, root.path(), &manifest, &preview.sha256().unwrap())
        .unwrap();
    let mut advanced = Repository::find_by_name(&conn, "cachyos").unwrap().unwrap();
    advanced
        .admit_authenticated_snapshot(second.clone())
        .unwrap();
    advanced.update(&conn).unwrap();
    assert_eq!(advanced.authenticated_snapshot, Some(second));
}

#[test]
fn alpm_binding_must_match_the_effective_native_siglevel() {
    let (root, conn, mut manifest) = alpm_fixture("Required DatabaseRequired");
    let RepositoryTrustPolicy::Arch { sig_level, .. } = &mut manifest.repositories[0].trust else {
        unreachable!()
    };
    *sig_level = ArchSigLevel::distribution_default();

    let preview = preview_native_repository_takeover(&conn, root.path(), &manifest).unwrap();
    assert!(preview.blockers.iter().any(|blocker| matches!(
        blocker,
        TakeoverBlocker::TrustPolicyMismatch { name, .. } if name == "cachyos"
    )));
    assert!(
        preview
            .blockers
            .iter()
            .any(|blocker| matches!(blocker, TakeoverBlocker::EnabledTrust { .. }))
    );
}

#[test]
fn unsafe_alpm_siglevel_cannot_be_overridden_by_an_exact_keyring() {
    let (root, conn, mut manifest) = alpm_fixture("Optional TrustAll");
    let RepositoryTrustPolicy::Arch { sig_level, .. } = &mut manifest.repositories[0].trust else {
        unreachable!()
    };
    *sig_level = ArchSigLevel {
        database: ArchSignatureRequirement::Optional,
        package: ArchSignatureRequirement::Required,
        trust: ArchTrustLevel::TrustedOnly,
    };

    let preview = preview_native_repository_takeover(&conn, root.path(), &manifest).unwrap();
    assert!(matches!(
        preview.trust.repositories[0].disposition,
        TrustImportDisposition::Unsupported { .. }
    ));
    assert!(
        preview
            .blockers
            .iter()
            .any(|blocker| matches!(blocker, TakeoverBlocker::EnabledTrust { .. }))
    );
}
