// conary-core/src/repository/declarations/takeover/conformance_tests.rs

use super::*;
use crate::repository::{
    ArchKeyringFormat, ArchKeyringTrust, ArchSigLevel, ArchSignatureRequirement, ArchTrustLevel,
};

fn alpm_fixture(
    sig_level: &str,
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
            "[options]\nArchitecture = auto\nSigLevel = {sig_level}\n\n[cachyos]\nServer = https://mirror.cachyos.example/repo/$arch/$repo\nUsage = Sync Search Install Upgrade\n"
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
            name: "cachyos".to_string(),
            source_identity: "cachyos:rolling:x86_64".to_string(),
            repository_identity: "cachyos:x86_64".to_string(),
            scope: TakeoverPolicyScope::Repository {
                identity: "cachyos:x86_64".to_string(),
            },
            stream: TakeoverSourceStream::Rolling {
                identity: "cachyos".to_string(),
            },
            update: TakeoverUpdatePolicy::Follow,
            metadata_url: "https://mirror.cachyos.example/repo/x86_64/cachyos".to_string(),
            content_url: None,
            parser: RepositoryParserConfig::Arch {
                database: "cachyos".to_string(),
            },
            trust: RepositoryTrustPolicy::Arch {
                keyring: ArchKeyringTrust {
                    url: "https://keys.cachyos.example/cachyos-keyring.pkg.tar.zst".to_string(),
                    format: ArchKeyringFormat::AlpmPackageZstd,
                    master_fingerprints: vec!["A".repeat(40)],
                    packager_key_threshold: 1,
                },
                sig_level: ArchSigLevel::distribution_default(),
            },
            enabled: true,
            priority: 30,
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
