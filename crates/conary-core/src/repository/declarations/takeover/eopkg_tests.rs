// conary-core/src/repository/declarations/takeover/eopkg_tests.rs

use super::*;

fn fixture() -> (
    tempfile::TempDir,
    Connection,
    NativeRepositoryTakeoverManifest,
    String,
) {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("var/lib/eopkg/info")).unwrap();
    let declaration = "<REPOS>\n  <Repo>\n    <Name>Solus</Name>\n    <Url>https://cdn.getsol.us/repo/polaris/eopkg-index.xml.xz</Url>\n    <Status>active</Status>\n    <Media>remote</Media>\n  </Repo>\n</REPOS>\n";
    fs::write(root.path().join("var/lib/eopkg/info/repos"), declaration).unwrap();
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
            name: "solus-polaris".to_string(),
            source_identity: "solus-project".to_string(),
            repository_identity: "solus-polaris-x86_64".to_string(),
            scope: TakeoverPolicyScope::Repository {
                identity: "solus-polaris-x86_64".to_string(),
            },
            stream: TakeoverSourceStream::Rolling {
                identity: "polaris".to_string(),
            },
            update: TakeoverUpdatePolicy::Follow,
            metadata_url: "https://cdn.getsol.us/repo/polaris/".to_string(),
            content_url: None,
            parser: RepositoryParserConfig::Eopkg {
                architecture: "x86_64".to_string(),
            },
            trust: RepositoryTrustPolicy::Eopkg {
                origin: "https://cdn.getsol.us/repo/polaris/".to_string(),
            },
            enabled: true,
            priority: 50,
            metadata_expire: 3600,
            source_profile: Some("solus".to_string()),
        }],
    };
    (root, conn, manifest, declaration.to_string())
}

#[test]
fn eopkg_repository_takeover_preserves_declaration_and_follow_policy() {
    let (root, conn, manifest, declaration) = fixture();
    let preview = preview_native_repository_takeover(&conn, root.path(), &manifest).unwrap();
    assert!(preview.blockers.is_empty(), "{:?}", preview.blockers);
    assert_eq!(preview.projections.len(), 1);
    let digest = preview.sha256().unwrap();
    let outcome = apply_native_repository_takeover(&conn, root.path(), &manifest, &digest).unwrap();
    assert_eq!(outcome.status, TakeoverApplyStatus::Applied);

    let repository = Repository::find_by_name(&conn, "solus-polaris")
        .unwrap()
        .unwrap();
    assert_eq!(repository.package_format, RepositoryFormat::Eopkg);
    assert_eq!(repository.source_profile.as_deref(), Some("solus"));
    assert!(matches!(
        repository.source_policy.unwrap().update_mode,
        crate::db::models::RepositoryUpdateMode::Follow
    ));
    assert_eq!(
        fs::read_to_string(root.path().join("var/lib/eopkg/info/repos")).unwrap(),
        declaration
    );

    rollback_native_repository_takeover(&conn, root.path()).unwrap();
    assert!(
        Repository::find_by_name(&conn, "solus-polaris")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        fs::read_to_string(root.path().join("var/lib/eopkg/info/repos")).unwrap(),
        declaration
    );
}
