// conary-core/src/repository/sync/tests.rs

mod tests {
    use super::*;
    use crate::ccs::signing::SigningKeyPair;
    use crate::repository::sync::native::normalized_repository_capabilities;
    use crate::db::models::{
        ConvertedPackage, RepositoryPackage, RepositoryPackageKey, RepositoryProvide,
        RepositoryRequirement, RepositoryRequirementGroup as DbRequirementGroup,
        SecurityAdvisorySupport,
    };
    use crate::db::schema::ensure_current;
    use crate::hash::sha256;
    use crate::repository::dependency_model::{
        self as dep_model, ConditionalRequirementBehavior, RepositoryDependencyFlavor,
        RepositoryRequirementKind,
    };
    use crate::repository::metadata::RepositoryMetadata as JsonRepositoryMetadata;
    use crate::repository::parsers::PackageMetadata;
    use crate::repository::remi_metadata::{
        RemiProvide, RemiRequirement, RemiRequirementGroup, RemiSparseResolutionVersionEntry,
    };
    use crate::repository::versioning::VersionScheme;
    use crate::trust::metadata::{TargetDescription, VerifiedTufState};
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::path::Path;

    const STATIC_PACKAGE_PATH: &str = "packages/acme-widget/acme-widget-1.4.2-1-x86_64.ccs";

    struct StaticSyncFixture {
        _tempdir: tempfile::TempDir,
        repo: Repository,
        package_bytes: Vec<u8>,
        active_key: String,
        retired_key: String,
    }

    impl StaticSyncFixture {
        fn new() -> Self {
            let tempdir = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(tempdir.path().join("packages/acme-widget")).unwrap();
            std::fs::create_dir_all(tempdir.path().join("keys")).unwrap();

            let package_bytes = b"static ccs payload".to_vec();
            std::fs::write(tempdir.path().join(STATIC_PACKAGE_PATH), &package_bytes).unwrap();

            let active_key = SigningKeyPair::generate().public_key_base64();
            let retired_key = SigningKeyPair::generate().public_key_base64();

            let mut repo = Repository::new(
                "static-test".to_string(),
                tempdir.path().display().to_string(),
            );
            repo.default_strategy = Some("static".to_string());
            repo.tuf_enabled = true;

            let mut fixture = Self {
                _tempdir: tempdir,
                repo,
                package_bytes,
                active_key,
                retired_key,
            };
            fixture.write_static_files();
            fixture
        }

        fn root(&self) -> &Path {
            self._tempdir.path()
        }

        fn insert_repo(&mut self, conn: &Connection) -> i64 {
            self.repo.insert(conn).unwrap();
            self.repo.id.unwrap()
        }

        fn write_static_files(&mut self) {
            let package_sha = sha256(&self.package_bytes);
            let mut self_provide = dep_model::RepositoryProvide::package_name(
                "acme-widget".to_string(),
                Some("1.4.2".to_string()),
            );
            self_provide.native_text = Some("acme-widget".to_string());

            let mut libfoo_clause = dep_model::RepositoryRequirementClause::versioned(
                "libfoo".to_string(),
                ">= 2.0.0".to_string(),
            );
            libfoo_clause.capability_kind = Some(dep_model::RepositoryCapabilityKind::Generic);
            libfoo_clause.native_text = Some("libfoo >= 2.0.0".to_string());
            let libfoo = dep_model::RepositoryRequirementGroup::simple(
                RepositoryRequirementKind::Depends,
                libfoo_clause,
            )
            .with_native_text("libfoo >= 2.0.0".to_string());

            let mut libbar_clause =
                dep_model::RepositoryRequirementClause::name_only("libbar".to_string());
            libbar_clause.capability_kind = Some(dep_model::RepositoryCapabilityKind::Generic);
            libbar_clause.native_text = Some("libbar".to_string());
            let libbar = dep_model::RepositoryRequirementGroup::simple(
                RepositoryRequirementKind::Depends,
                libbar_clause,
            )
            .with_native_text("libbar".to_string());

            let index = json!({
                "schema": crate::repository::static_repo::SCHEMA_VERSION,
                "name": "acme-tools",
                "index_version": 7,
                "generated": "2026-06-10T18:00:00Z",
                "packages": [{
                    "name": "acme-widget",
                    "version": "1.4.2",
                    "version_scheme": "conary",
                    "release": "1",
                    "arch": "x86_64",
                    "path": STATIC_PACKAGE_PATH,
                    "sha256": package_sha,
                    "size": self.package_bytes.len() as u64,
                    "description": "Widget frobnicator",
                    "provides": [self_provide],
                    "requirements": [libfoo, libbar],
                    "relations": []
                }]
            });
            std::fs::write(
                self.root().join("index.json"),
                serde_json::to_vec(&index).unwrap(),
            )
            .unwrap();

            let keys = json!({
                "schema": crate::repository::static_repo::SCHEMA_VERSION,
                "keys": [
                    {
                        "algorithm": "ed25519",
                        "public_key": self.active_key,
                        "key_id": "active-key",
                        "status": "active"
                    },
                    {
                        "algorithm": "ed25519",
                        "public_key": self.retired_key,
                        "key_id": "retired-key",
                        "status": "retired"
                    }
                ]
            });
            std::fs::write(
                self.root().join("keys/package-keys.json"),
                serde_json::to_vec(&keys).unwrap(),
            )
            .unwrap();
        }

        fn verified(&self) -> VerifiedTufState {
            let mut targets = BTreeMap::new();
            targets.insert(
                "index.json".to_string(),
                target_for_bytes(&std::fs::read(self.root().join("index.json")).unwrap()),
            );
            targets.insert(
                "keys/package-keys.json".to_string(),
                target_for_bytes(
                    &std::fs::read(self.root().join("keys/package-keys.json")).unwrap(),
                ),
            );
            targets.insert(
                STATIC_PACKAGE_PATH.to_string(),
                target_for_bytes(&self.package_bytes),
            );

            VerifiedTufState {
                root_version: 1,
                targets_version: 7,
                snapshot_version: 7,
                timestamp_version: 7,
                targets,
            }
        }
    }

    fn target_for_bytes(bytes: &[u8]) -> TargetDescription {
        let mut hashes = BTreeMap::new();
        hashes.insert("sha256".to_string(), sha256(bytes));
        TargetDescription {
            length: bytes.len() as u64,
            hashes,
        }
    }

    fn static_repo(name: &str, url: &str) -> Repository {
        let mut repo = Repository::new(name.to_string(), url.to_string());
        repo.default_strategy = Some("static".to_string());
        repo
    }

    fn assert_repin_error(error: impl std::fmt::Display, name: &str, url: &str) {
        let error = error.to_string();
        assert!(error.contains("Static repository trust is not established"));
        assert!(error.contains(&format!(
            "conary repo add {name} {url} --fingerprint <root-key-id> --replace"
        )));
    }

    #[tokio::test]
    async fn static_repo_without_tuf_enabled_hard_fails_before_native_or_json_fetch() {
        let (_temp, conn) = crate::db::testing::create_test_db();
        let mut repo = static_repo("static-test", "file:///definitely/missing/static-repo");
        repo.insert(&conn).unwrap();

        let err = sync_repository(&conn, &mut repo).await.unwrap_err();

        assert_repin_error(err, "static-test", "file:///definitely/missing/static-repo");
    }

    #[tokio::test]
    async fn static_repo_db_path_without_tuf_enabled_hard_fails_before_native_or_json_fetch() {
        let (temp, _conn) = crate::db::testing::create_test_db();
        let repo = static_repo("static-test", "file:///definitely/missing/static-repo");

        let err = sync_repository_from_db_path(temp.path().to_path_buf(), repo)
            .await
            .unwrap_err();

        assert_repin_error(err, "static-test", "file:///definitely/missing/static-repo");
    }

    #[tokio::test]
    async fn static_repo_without_trusted_root_hard_fails_with_repin_command() {
        let (_temp, conn) = crate::db::testing::create_test_db();
        let mut repo = static_repo("static-test", "file:///definitely/missing/static-repo");
        repo.tuf_enabled = true;
        repo.insert(&conn).unwrap();

        let err = sync_repository(&conn, &mut repo).await.unwrap_err();

        assert_repin_error(err, "static-test", "file:///definitely/missing/static-repo");
    }

    #[tokio::test]
    async fn static_repo_db_path_without_trusted_root_hard_fails_with_repin_command() {
        let (temp, conn) = crate::db::testing::create_test_db();
        let mut repo = static_repo("static-test", "file:///definitely/missing/static-repo");
        repo.tuf_enabled = true;
        repo.insert(&conn).unwrap();

        let err = sync_repository_from_db_path(temp.path().to_path_buf(), repo)
            .await
            .unwrap_err();

        assert_repin_error(err, "static-test", "file:///definitely/missing/static-repo");
    }

    #[test]
    fn static_sync_snapshot_persists_packages_keys_and_normalized_rows_atomically() {
        let (_temp, conn) = crate::db::testing::create_test_db();
        let mut fixture = StaticSyncFixture::new();
        let repo_id = fixture.insert_repo(&conn);
        let snapshot = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(
                crate::repository::static_repo::sync::fetch_static_sync_snapshot(
                    &fixture.repo,
                    &fixture.verified(),
                ),
            )
            .unwrap();

        let count = persist_repository_sync_snapshot(&conn, &mut fixture.repo, snapshot).unwrap();

        assert_eq!(count, 1);
        assert!(fixture.repo.last_sync.is_some());

        let packages = RepositoryPackage::find_by_repository(&conn, repo_id).unwrap();
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name, "acme-widget");
        assert_eq!(packages[0].version, "1.4.2");
        assert_eq!(packages[0].package_release, "1");
        assert!(
            packages[0]
                .metadata
                .as_deref()
                .is_some_and(|metadata| metadata.contains(r#""release":"1""#))
        );

        let package_id = packages[0].id.unwrap();
        let provides = RepositoryProvide::find_by_repository_package(&conn, package_id).unwrap();
        assert!(provides.iter().any(|provide| {
            provide.capability == "acme-widget"
                && provide.version.as_deref() == Some("1.4.2")
                && provide.raw.as_deref() == Some("acme-widget")
        }));

        let requirements =
            RepositoryRequirement::find_by_repository_package(&conn, package_id).unwrap();
        assert!(requirements.iter().any(|requirement| {
            requirement.capability == "libfoo"
                && requirement.version_constraint.as_deref() == Some(">= 2.0.0")
                && requirement.raw.as_deref() == Some("libfoo >= 2.0.0")
        }));

        let trusted = RepositoryPackageKey::trusted_keys_for_repository(&conn, repo_id).unwrap();
        assert_eq!(trusted.len(), 1);
        assert!(trusted.contains(&fixture.active_key));

        let stored_keys_and_statuses: Vec<(String, String)> = conn
            .prepare(
                "SELECT public_key, status FROM repository_package_keys
                 WHERE repository_id = ?1 ORDER BY status",
            )
            .unwrap()
            .query_map([repo_id], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            stored_keys_and_statuses,
            vec![
                (fixture.active_key.clone(), "active".to_string()),
                (fixture.retired_key.clone(), "retired".to_string())
            ]
        );
    }

    #[test]
    fn test_rebase_download_url_no_content_url() {
        // When no content_url, the download_url should be unchanged
        let download_url = "https://example.com/fedora/Packages/foo-1.0.rpm";
        let metadata_url = "https://example.com/fedora";

        let result = rebase_download_url(download_url, metadata_url, None);
        assert_eq!(result, download_url);
    }

    #[test]
    fn test_rebase_download_url_with_content_url() {
        // Rebase from metadata URL to content URL
        let download_url = "https://metadata.example.com/fedora/Packages/foo-1.0.rpm";
        let metadata_url = "https://metadata.example.com/fedora";
        let content_url = "https://mirror.local/fedora";

        let result = rebase_download_url(download_url, metadata_url, Some(content_url));
        assert_eq!(result, "https://mirror.local/fedora/Packages/foo-1.0.rpm");
    }

    #[test]
    fn test_rebase_download_url_trailing_slashes() {
        // Handle trailing slashes correctly - no double slashes in output
        let download_url = "https://metadata.example.com/fedora/Packages/foo-1.0.rpm";
        let metadata_url = "https://metadata.example.com/fedora/";
        let content_url = "https://mirror.local/fedora/";

        let result = rebase_download_url(download_url, metadata_url, Some(content_url));
        assert_eq!(result, "https://mirror.local/fedora/Packages/foo-1.0.rpm");
        // Verify no double slashes
        assert!(
            !result.contains("//P"),
            "Should not have double slashes before path"
        );
    }

    #[test]
    fn test_rebase_download_url_no_leading_slash() {
        // Handle case where relative path has no leading slash
        let download_url = "https://metadata.example.com/fedora/Packages/foo-1.0.rpm";
        let metadata_url = "https://metadata.example.com/fedora";
        let content_url = "https://mirror.local/content";

        let result = rebase_download_url(download_url, metadata_url, Some(content_url));
        assert_eq!(result, "https://mirror.local/content/Packages/foo-1.0.rpm");
    }

    #[test]
    fn test_rebase_download_url_ubuntu_example() {
        // Real-world example: Ubuntu with local metadata but archive.ubuntu.com content
        let download_url = "https://your-server.com/ubuntu/pool/main/n/nginx/nginx_1.24.0.deb";
        let metadata_url = "https://your-server.com/ubuntu";
        let content_url = "https://archive.ubuntu.com/ubuntu";

        let result = rebase_download_url(download_url, metadata_url, Some(content_url));
        assert_eq!(
            result,
            "https://archive.ubuntu.com/ubuntu/pool/main/n/nginx/nginx_1.24.0.deb"
        );
    }

    #[test]
    fn test_rebase_download_url_different_base() {
        // When download_url doesn't match metadata_url prefix, return as-is
        let download_url = "https://other-server.com/packages/foo.rpm";
        let metadata_url = "https://metadata.example.com/fedora";
        let content_url = "https://mirror.local/fedora";

        let result = rebase_download_url(download_url, metadata_url, Some(content_url));
        // Can't rebase - different base, return as-is
        assert_eq!(result, "https://other-server.com/packages/foo.rpm");
    }

    include!("tests/native.rs");
}
