// apps/conary/src/commands/install/conversion/tests.rs

use super::*;
use conary_core::capability::{
    CapabilityDeclaration, FilesystemCapabilities, NetworkCapabilities, SyscallCapabilities,
};
use conary_core::ccs::SigningKeyPair;
use conary_core::ccs::builder::{write_ccs_package, write_signed_ccs_package};
use conary_core::ccs::manifest::{DirectoryHook, ScriptHook};
use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
use conary_core::db::models::{
    Repository, RepositoryPackage, RepositoryPackageKey, RepositoryPackageKeyStatus,
    RepositoryProvide, Trove,
};
use conary_core::db::schema;
use conary_core::hash;
use conary_core::packages::traits::{
    Dependency, ExtractedFile, PackageFile, PackageFormat, Scriptlet,
};
use conary_core::repository::RepositorySourceKind;
use conary_core::version::VersionConstraint;
use std::collections::HashMap;

struct FakeLegacyPackage {
    name: String,
    version: String,
    description: Option<String>,
    files: Vec<PackageFile>,
    extracted_files: Vec<ExtractedFile>,
    dependencies: Vec<Dependency>,
    provides: Vec<Dependency>,
    scriptlets: Vec<Scriptlet>,
}

impl FakeLegacyPackage {
    fn nginx() -> Self {
        let content = b"#!/bin/sh\nexec true\n".to_vec();
        let size = content.len() as i64;
        let hash = hash::sha256(&content);
        Self {
            name: "nginx".to_string(),
            version: "1.0.0".to_string(),
            description: Some("fake nginx legacy package".to_string()),
            files: vec![PackageFile {
                path: "/usr/sbin/nginx".to_string(),
                size,
                mode: 0o100755,
                sha256: Some(hash.clone()),
                symlink_target: None,
            }],
            extracted_files: vec![ExtractedFile {
                path: "/usr/sbin/nginx".to_string(),
                content,
                size,
                mode: 0o100755,
                sha256: Some(hash),
                symlink_target: None,
            }],
            dependencies: Vec::new(),
            provides: Vec::new(),
            scriptlets: Vec::new(),
        }
    }
}

impl PackageFormat for FakeLegacyPackage {
    fn parse(_path: &str) -> conary_core::Result<Self> {
        unimplemented!("test fake package is constructed directly")
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn version(&self) -> &str {
        &self.version
    }

    fn architecture(&self) -> Option<&str> {
        Some("x86_64")
    }

    fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    fn files(&self) -> &[PackageFile] {
        &self.files
    }

    fn dependencies(&self) -> &[Dependency] {
        &self.dependencies
    }

    fn provides(&self) -> &[Dependency] {
        &self.provides
    }

    fn extract_file_contents(&self) -> conary_core::Result<Vec<ExtractedFile>> {
        Ok(self.extracted_files.clone())
    }

    fn scriptlets(&self) -> &[Scriptlet] {
        &self.scriptlets
    }

    fn to_trove(&self) -> Trove {
        Trove::new(
            self.name.clone(),
            self.version.clone(),
            conary_core::db::models::TroveType::Package,
        )
    }
}

fn test_db() -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA foreign_keys = ON;",
    )
    .unwrap();
    schema::migrate(&conn).unwrap();
    conn
}

fn stage_test_boot_assets(root: &std::path::Path) {
    let kernel_version = conary_core::generation::builder::detect_kernel_version_from_troves(&[])
        .unwrap_or_else(|| "test-kernel".to_string());
    let boot_root = root.join("boot");
    std::fs::create_dir_all(boot_root.join("EFI/BOOT")).unwrap();
    std::fs::write(
        boot_root.join(format!("vmlinuz-{kernel_version}")),
        b"test-kernel",
    )
    .unwrap();
    std::fs::write(
        boot_root.join(format!("initramfs-{kernel_version}.img")),
        b"test-initramfs",
    )
    .unwrap();
    std::fs::write(boot_root.join("EFI/BOOT/BOOTX64.EFI"), b"test-efi").unwrap();
}

fn write_runtime_ccs_package(
    temp_dir: &std::path::Path,
    name: &str,
    mut manifest: CcsManifest,
) -> std::path::PathBuf {
    let package_path = temp_dir.join(format!("{name}.ccs"));
    let init_content = b"#!/bin/sh\nexec true\n".to_vec();
    let init_hash = hash::sha256(&init_content);
    let files = vec![FileEntry {
        path: "/usr/sbin/init".to_string(),
        hash: init_hash.clone(),
        size: init_content.len() as u64,
        mode: 0o100755,
        component: "runtime".to_string(),
        file_type: FileType::Regular,
        target: None,
        chunks: None,
    }];
    manifest.components.default = vec!["runtime".to_string()];
    let result = BuildResult {
        manifest,
        components: HashMap::from([(
            "runtime".to_string(),
            ComponentData {
                name: "runtime".to_string(),
                files: files.clone(),
                hash: "runtime".to_string(),
                size: init_content.len() as u64,
            },
        )]),
        files,
        blobs: HashMap::from([(init_hash, init_content)]),
        total_size: 0,
        chunked: false,
        chunk_stats: None,
    };
    write_ccs_package(&result, &package_path).unwrap();
    package_path
}

fn write_runtime_signed_ccs_package(
    temp_dir: &std::path::Path,
    name: &str,
    mut manifest: CcsManifest,
    signing_key: Option<&SigningKeyPair>,
) -> std::path::PathBuf {
    let package_path = temp_dir.join(format!("{name}.ccs"));
    let init_content = b"#!/bin/sh\nexec true\n".to_vec();
    let init_hash = hash::sha256(&init_content);
    let files = vec![FileEntry {
        path: "/usr/sbin/init".to_string(),
        hash: init_hash.clone(),
        size: init_content.len() as u64,
        mode: 0o100755,
        component: "runtime".to_string(),
        file_type: FileType::Regular,
        target: None,
        chunks: None,
    }];
    manifest.components.default = vec!["runtime".to_string()];
    let result = BuildResult {
        manifest,
        components: HashMap::from([(
            "runtime".to_string(),
            ComponentData {
                name: "runtime".to_string(),
                files: files.clone(),
                hash: "runtime".to_string(),
                size: init_content.len() as u64,
            },
        )]),
        files,
        blobs: HashMap::from([(init_hash, init_content)]),
        total_size: 0,
        chunked: false,
        chunk_stats: None,
    };
    if let Some(signing_key) = signing_key {
        write_signed_ccs_package(&result, &package_path, signing_key).unwrap();
    } else {
        write_ccs_package(&result, &package_path).unwrap();
    }
    package_path
}

fn package_key(
    repository_id: i64,
    signing_key: &SigningKeyPair,
    status: RepositoryPackageKeyStatus,
) -> RepositoryPackageKey {
    RepositoryPackageKey {
        repository_id,
        public_key: signing_key.public_key_base64(),
        key_id: signing_key.key_id().map(str::to_string),
        status,
        synced_at: None,
    }
}

fn insert_static_repository_with_keys(
    db_path: &str,
    key_pairs: &[(&SigningKeyPair, RepositoryPackageKeyStatus)],
) -> i64 {
    let conn = conary_core::db::open(db_path).unwrap();
    let mut repo = Repository::new(
        "static-install".to_string(),
        "https://static.example.invalid/repo".to_string(),
    );
    repo.default_strategy = Some("static".to_string());
    let repo_id = repo.insert(&conn).unwrap();
    let keys = key_pairs
        .iter()
        .map(|(signing_key, status)| package_key(repo_id, signing_key, status.clone()))
        .collect::<Vec<_>>();
    RepositoryPackageKey::replace_for_repository(&conn, repo_id, &keys).unwrap();
    repo_id
}

fn static_provenance(repository_id: i64) -> RepositoryInstallProvenance {
    RepositoryInstallProvenance {
        repository_id,
        source_distro: Some("fedora".to_string()),
        version_scheme: Some("rpm".to_string()),
        source_kind: RepositorySourceKind::Static,
    }
}

fn converted_install_options<'a>(
    ccs_path: &'a std::path::Path,
    db_path: &'a str,
    install_root: &'a std::path::Path,
    repository_provenance: Option<RepositoryInstallProvenance>,
) -> ConvertedCcsInstallOptions<'a> {
    ConvertedCcsInstallOptions {
        ccs_path: ccs_path.to_str().unwrap(),
        db_path,
        root: install_root.to_str().unwrap(),
        dry_run: false,
        sandbox_mode: SandboxMode::None,
        no_deps: true,
        no_scripts: true,
        allow_downgrade: false,
        allow_capabilities: false,
        dep_mode: None,
        yes: true,
        dependency_passes_remaining: 0,
        repository_provenance,
        legacy_replay: LegacyReplayOptions::default(),
    }
}

#[tokio::test]
async fn try_convert_to_ccs_does_not_guess_capability_policy() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();
    let legacy_path = temp_dir.path().join("nginx.rpm");

    conary_core::db::init(db_path_str).unwrap();
    std::fs::write(&legacy_path, b"fake legacy package bytes").unwrap();

    let package = FakeLegacyPackage::nginx();
    let result = try_convert_to_ccs(
        &package,
        &legacy_path,
        PackageFormatType::Rpm,
        db_path_str,
        false,
    )
    .await
    .expect("conversion without declared capabilities must not require policy approval");
    let ConversionResult::Converted { ccs_path, .. } = result else {
        panic!("conversion unexpectedly skipped");
    };
    let converted = CcsPackage::parse(&ccs_path).unwrap();
    assert!(
        converted.manifest().capabilities.is_none(),
        "conversion must not guess capability policy from package names or paths"
    );

    let conn = conary_core::db::open(db_path_str).unwrap();
    let converted_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM converted_packages", [], |row| {
            row.get(0)
        })
        .unwrap();
    let trove_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM troves", [], |row| row.get(0))
        .unwrap();
    let changeset_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM changesets", [], |row| row.get(0))
        .unwrap();
    assert_eq!(converted_count, 1);
    assert_eq!(trove_count, 0);
    assert_eq!(changeset_count, 0);
    assert!(
        std::fs::read_link(temp_dir.path().join("current")).is_err(),
        "conversion alone must not activate an installed generation"
    );
}

#[tokio::test]
async fn converted_ccs_install_executes_directory_hooks() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    std::fs::create_dir_all(&install_root).unwrap();
    conary_core::db::init(db_path_str).unwrap();
    stage_test_boot_assets(temp_dir.path());

    let mut manifest = CcsManifest::new_minimal("converted-hooked", "1.0.0");
    manifest.hooks.directories.push(DirectoryHook {
        path: "/var/lib/converted-hooked".to_string(),
        mode: "0755".to_string(),
        owner: "root".to_string(),
        group: "root".to_string(),
        cleanup: None,
        reversible: None,
    });
    let package_path = write_runtime_ccs_package(temp_dir.path(), "converted-hooked", manifest);

    install_converted_ccs(ConvertedCcsInstallOptions {
        ccs_path: package_path.to_str().unwrap(),
        db_path: db_path_str,
        root: install_root.to_str().unwrap(),
        dry_run: false,
        sandbox_mode: SandboxMode::None,
        no_deps: true,
        no_scripts: false,
        allow_downgrade: false,
        allow_capabilities: false,
        dep_mode: None,
        yes: true,
        dependency_passes_remaining: 0,
        repository_provenance: None,
        legacy_replay: LegacyReplayOptions::default(),
    })
    .await
    .unwrap();

    assert!(install_root.join("var/lib/converted-hooked").is_dir());
}

#[tokio::test]
async fn static_repo_ccs_install_rejects_unsigned_package() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    std::fs::create_dir_all(&install_root).unwrap();
    conary_core::db::init(db_path_str).unwrap();
    stage_test_boot_assets(temp_dir.path());
    let repo_id = insert_static_repository_with_keys(db_path_str, &[]);
    let package_path = write_runtime_signed_ccs_package(
        temp_dir.path(),
        "static-unsigned",
        CcsManifest::new_minimal("static-unsigned", "1.0.0"),
        None,
    );

    let err = install_converted_ccs(converted_install_options(
        &package_path,
        db_path_str,
        &install_root,
        Some(static_provenance(repo_id)),
    ))
    .await
    .unwrap_err();

    assert!(
        format!("{err:?}").contains("Static repository package signature verification failed"),
        "static unsigned package should fail signature verification: {err:?}"
    );
}

#[tokio::test]
async fn static_repo_ccs_install_rejects_unlisted_signing_key() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    std::fs::create_dir_all(&install_root).unwrap();
    conary_core::db::init(db_path_str).unwrap();
    stage_test_boot_assets(temp_dir.path());
    let listed_key = SigningKeyPair::generate().with_key_id("listed");
    let unlisted_key = SigningKeyPair::generate().with_key_id("unlisted");
    let repo_id = insert_static_repository_with_keys(
        db_path_str,
        &[(&listed_key, RepositoryPackageKeyStatus::Active)],
    );
    let package_path = write_runtime_signed_ccs_package(
        temp_dir.path(),
        "static-unlisted",
        CcsManifest::new_minimal("static-unlisted", "1.0.0"),
        Some(&unlisted_key),
    );

    let err = install_converted_ccs(converted_install_options(
        &package_path,
        db_path_str,
        &install_root,
        Some(static_provenance(repo_id)),
    ))
    .await
    .unwrap_err();

    assert!(
        format!("{err:?}").contains("Static repository package signature verification failed"),
        "static package signed by an unlisted key should fail: {err:?}"
    );
}

#[tokio::test]
async fn static_repo_ccs_install_accepts_active_package_key() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    std::fs::create_dir_all(&install_root).unwrap();
    conary_core::db::init(db_path_str).unwrap();
    stage_test_boot_assets(temp_dir.path());
    let active_key = SigningKeyPair::generate().with_key_id("active");
    let repo_id = insert_static_repository_with_keys(
        db_path_str,
        &[(&active_key, RepositoryPackageKeyStatus::Active)],
    );
    let package_path = write_runtime_signed_ccs_package(
        temp_dir.path(),
        "static-active",
        CcsManifest::new_minimal("static-active", "1.0.0"),
        Some(&active_key),
    );

    install_converted_ccs(converted_install_options(
        &package_path,
        db_path_str,
        &install_root,
        Some(static_provenance(repo_id)),
    ))
    .await
    .unwrap();

    let conn = conary_core::db::open(db_path_str).unwrap();
    let trove_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM troves WHERE name = 'static-active'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(trove_count, 1);
}

#[tokio::test]
async fn static_repo_ccs_install_rejects_retired_only_package_key() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    std::fs::create_dir_all(&install_root).unwrap();
    conary_core::db::init(db_path_str).unwrap();
    stage_test_boot_assets(temp_dir.path());
    let retired_key = SigningKeyPair::generate().with_key_id("retired");
    let repo_id = insert_static_repository_with_keys(
        db_path_str,
        &[(&retired_key, RepositoryPackageKeyStatus::Retired)],
    );
    let package_path = write_runtime_signed_ccs_package(
        temp_dir.path(),
        "static-retired",
        CcsManifest::new_minimal("static-retired", "1.0.0"),
        Some(&retired_key),
    );

    let err = install_converted_ccs(converted_install_options(
        &package_path,
        db_path_str,
        &install_root,
        Some(static_provenance(repo_id)),
    ))
    .await
    .unwrap_err();

    assert!(
        format!("{err:?}").contains("Static repository package signature verification failed"),
        "static package signed only by a retired key should fail: {err:?}"
    );
}

#[tokio::test]
async fn non_static_repo_ccs_install_keeps_unsigned_behavior() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    std::fs::create_dir_all(&install_root).unwrap();
    conary_core::db::init(db_path_str).unwrap();
    stage_test_boot_assets(temp_dir.path());
    let package_path = write_runtime_signed_ccs_package(
        temp_dir.path(),
        "remi-unsigned",
        CcsManifest::new_minimal("remi-unsigned", "1.0.0"),
        None,
    );
    let repo_id = insert_static_repository_with_keys(db_path_str, &[]);
    let provenance = RepositoryInstallProvenance {
        repository_id: repo_id,
        source_distro: Some("fedora".to_string()),
        version_scheme: Some("rpm".to_string()),
        source_kind: RepositorySourceKind::Remi,
    };

    install_converted_ccs(converted_install_options(
        &package_path,
        db_path_str,
        &install_root,
        Some(provenance),
    ))
    .await
    .unwrap();

    let conn = conary_core::db::open(db_path_str).unwrap();
    let trove_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM troves WHERE name = 'remi-unsigned'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(trove_count, 1);
}

#[tokio::test]
async fn converted_ccs_install_marks_post_hook_failure() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    std::fs::create_dir_all(&install_root).unwrap();
    conary_core::db::init(db_path_str).unwrap();
    stage_test_boot_assets(temp_dir.path());

    let mut manifest = CcsManifest::new_minimal("converted-post-hook-fails", "1.0.0");
    manifest.hooks.post_install = Some(ScriptHook {
        script: "exit 31".to_string(),
        reversible: None,
    });
    let package_path =
        write_runtime_ccs_package(temp_dir.path(), "converted-post-hook-fails", manifest);

    install_converted_ccs(ConvertedCcsInstallOptions {
        ccs_path: package_path.to_str().unwrap(),
        db_path: db_path_str,
        root: install_root.to_str().unwrap(),
        dry_run: false,
        sandbox_mode: SandboxMode::None,
        no_deps: true,
        no_scripts: false,
        allow_downgrade: false,
        allow_capabilities: false,
        dep_mode: None,
        yes: true,
        dependency_passes_remaining: 0,
        repository_provenance: None,
        legacy_replay: LegacyReplayOptions::default(),
    })
    .await
    .unwrap();

    let conn = conary_core::db::open(db_path_str).unwrap();
    let status: String = conn
        .query_row(
            "SELECT status FROM changesets ORDER BY id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(status, "post_hooks_failed");
}

#[tokio::test]
async fn converted_ccs_install_rejects_symlink_child_payload() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    std::fs::create_dir_all(&install_root).unwrap();
    conary_core::db::init(db_path_str).unwrap();
    stage_test_boot_assets(temp_dir.path());

    let package_path = temp_dir.path().join("converted-symlink-child.ccs");
    let link_target = "/tmp/converted-escape".to_string();
    let link_hash = conary_core::filesystem::CasStore::compute_symlink_hash(&link_target);
    let child_content = b"should not persist".to_vec();
    let child_hash = hash::sha256(&child_content);
    let init_content = b"#!/bin/sh\nexec true\n".to_vec();
    let init_hash = hash::sha256(&init_content);
    let files = vec![
        FileEntry {
            path: "/usr/lib/link".to_string(),
            hash: link_hash.clone(),
            size: link_target.len() as u64,
            mode: 0o120777,
            component: "runtime".to_string(),
            file_type: FileType::Symlink,
            target: Some(link_target.clone()),
            chunks: None,
        },
        FileEntry {
            path: "/usr/lib/link/child".to_string(),
            hash: child_hash.clone(),
            size: child_content.len() as u64,
            mode: 0o100644,
            component: "runtime".to_string(),
            file_type: FileType::Regular,
            target: None,
            chunks: None,
        },
        FileEntry {
            path: "/usr/sbin/init".to_string(),
            hash: init_hash.clone(),
            size: init_content.len() as u64,
            mode: 0o100755,
            component: "runtime".to_string(),
            file_type: FileType::Regular,
            target: None,
            chunks: None,
        },
    ];
    let manifest = CcsManifest::new_minimal("converted-symlink-child", "1.0.0");
    let result = BuildResult {
        manifest,
        components: HashMap::from([(
            "runtime".to_string(),
            ComponentData {
                name: "runtime".to_string(),
                files: files.clone(),
                hash: "runtime".to_string(),
                size: (link_target.len() + child_content.len() + init_content.len()) as u64,
            },
        )]),
        files,
        blobs: HashMap::from([
            (link_hash, link_target.into_bytes()),
            (child_hash, child_content),
            (init_hash, init_content),
        ]),
        total_size: 0,
        chunked: false,
        chunk_stats: None,
    };
    write_ccs_package(&result, &package_path).unwrap();

    let err = install_converted_ccs(ConvertedCcsInstallOptions {
        ccs_path: package_path.to_str().unwrap(),
        db_path: db_path_str,
        root: install_root.to_str().unwrap(),
        dry_run: false,
        sandbox_mode: SandboxMode::None,
        no_deps: true,
        no_scripts: true,
        allow_downgrade: false,
        allow_capabilities: false,
        dep_mode: None,
        yes: true,
        dependency_passes_remaining: 0,
        repository_provenance: None,
        legacy_replay: LegacyReplayOptions::default(),
    })
    .await
    .unwrap_err();

    assert!(
        err.to_string().contains("symlink"),
        "converted CCS shared install path should reject child payloads beneath package symlinks: {err:?}"
    );
    let conn = conary_core::db::open(db_path_str).unwrap();
    let persisted: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM files WHERE path = '/usr/lib/link/child'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(persisted, 0);
}

#[tokio::test]
async fn converted_ccs_install_rejects_child_before_package_symlink() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    std::fs::create_dir_all(&install_root).unwrap();
    conary_core::db::init(db_path_str).unwrap();
    stage_test_boot_assets(temp_dir.path());

    let package_path = temp_dir.path().join("converted-reversed-symlink-child.ccs");
    let link_target = "/tmp/converted-reversed-escape".to_string();
    let link_hash = conary_core::filesystem::CasStore::compute_symlink_hash(&link_target);
    let child_content = b"should not persist".to_vec();
    let child_hash = hash::sha256(&child_content);
    let init_content = b"#!/bin/sh\nexec true\n".to_vec();
    let init_hash = hash::sha256(&init_content);
    let files = vec![
        FileEntry {
            path: "/usr/lib/link/child".to_string(),
            hash: child_hash.clone(),
            size: child_content.len() as u64,
            mode: 0o100644,
            component: "runtime".to_string(),
            file_type: FileType::Regular,
            target: None,
            chunks: None,
        },
        FileEntry {
            path: "/usr/lib/link".to_string(),
            hash: link_hash.clone(),
            size: link_target.len() as u64,
            mode: 0o120777,
            component: "runtime".to_string(),
            file_type: FileType::Symlink,
            target: Some(link_target.clone()),
            chunks: None,
        },
        FileEntry {
            path: "/usr/sbin/init".to_string(),
            hash: init_hash.clone(),
            size: init_content.len() as u64,
            mode: 0o100755,
            component: "runtime".to_string(),
            file_type: FileType::Regular,
            target: None,
            chunks: None,
        },
    ];
    let manifest = CcsManifest::new_minimal("converted-reversed-symlink-child", "1.0.0");
    let result = BuildResult {
        manifest,
        components: HashMap::from([(
            "runtime".to_string(),
            ComponentData {
                name: "runtime".to_string(),
                files: files.clone(),
                hash: "runtime".to_string(),
                size: (link_target.len() + child_content.len() + init_content.len()) as u64,
            },
        )]),
        files,
        blobs: HashMap::from([
            (child_hash, child_content),
            (link_hash, link_target.into_bytes()),
            (init_hash, init_content),
        ]),
        total_size: 0,
        chunked: false,
        chunk_stats: None,
    };
    write_ccs_package(&result, &package_path).unwrap();

    let err = install_converted_ccs(ConvertedCcsInstallOptions {
        ccs_path: package_path.to_str().unwrap(),
        db_path: db_path_str,
        root: install_root.to_str().unwrap(),
        dry_run: false,
        sandbox_mode: SandboxMode::None,
        no_deps: true,
        no_scripts: true,
        allow_downgrade: false,
        allow_capabilities: false,
        dep_mode: None,
        yes: true,
        dependency_passes_remaining: 0,
        repository_provenance: None,
        legacy_replay: LegacyReplayOptions::default(),
    })
    .await
    .unwrap_err();

    assert!(
        err.to_string().contains("symlink"),
        "converted CCS shared install path should reject child payloads before package symlinks: {err:?}"
    );
    let conn = conary_core::db::open(db_path_str).unwrap();
    let persisted: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM files WHERE path = '/usr/lib/link/child'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(persisted, 0);
}

#[tokio::test]
async fn converted_ccs_install_rejects_prompted_capabilities_before_db_mutation() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    std::fs::create_dir_all(&install_root).unwrap();
    conary_core::db::init(db_path_str).unwrap();
    stage_test_boot_assets(temp_dir.path());

    let mut manifest = CcsManifest::new_minimal("converted-prompted-capability", "1.0.0");
    manifest.capabilities = Some(CapabilityDeclaration {
        version: 1,
        rationale: Some("binds a privileged test port".to_string()),
        network: NetworkCapabilities {
            outbound: Vec::new(),
            listen: vec!["80".to_string()],
            none: false,
        },
        filesystem: FilesystemCapabilities::default(),
        syscalls: SyscallCapabilities::default(),
    });
    let package_path =
        write_runtime_ccs_package(temp_dir.path(), "converted-prompted-capability", manifest);

    let err = install_converted_ccs(ConvertedCcsInstallOptions {
        ccs_path: package_path.to_str().unwrap(),
        db_path: db_path_str,
        root: install_root.to_str().unwrap(),
        dry_run: false,
        sandbox_mode: SandboxMode::None,
        no_deps: true,
        no_scripts: true,
        allow_downgrade: false,
        allow_capabilities: false,
        dep_mode: None,
        yes: true,
        dependency_passes_remaining: 0,
        repository_provenance: None,
        legacy_replay: LegacyReplayOptions::default(),
    })
    .await
    .unwrap_err();

    assert!(
        err.to_string()
            .contains("requires capability cap-net-bind-service"),
        "converted CCS install should fail closed for prompted capabilities: {err:?}"
    );
    let conn = conary_core::db::open(db_path_str).unwrap();
    let trove_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM troves WHERE name = 'converted-prompted-capability'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let changeset_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM changesets", [], |row| row.get(0))
        .unwrap();
    assert_eq!(trove_count, 0);
    assert_eq!(changeset_count, 0);
    assert!(
        std::fs::read_link(temp_dir.path().join("current")).is_err(),
        "capability rejection must happen before generation activation"
    );
}

#[tokio::test]
async fn converted_ccs_install_accepts_prompted_capabilities_when_allowed() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    std::fs::create_dir_all(&install_root).unwrap();
    conary_core::db::init(db_path_str).unwrap();
    stage_test_boot_assets(temp_dir.path());

    let mut manifest = CcsManifest::new_minimal("converted-allowed-capability", "1.0.0");
    manifest.capabilities = Some(CapabilityDeclaration {
        version: 1,
        rationale: Some("binds a privileged test port".to_string()),
        network: NetworkCapabilities {
            outbound: Vec::new(),
            listen: vec!["80".to_string()],
            none: false,
        },
        filesystem: FilesystemCapabilities::default(),
        syscalls: SyscallCapabilities::default(),
    });
    let package_path =
        write_runtime_ccs_package(temp_dir.path(), "converted-allowed-capability", manifest);

    install_converted_ccs(ConvertedCcsInstallOptions {
        ccs_path: package_path.to_str().unwrap(),
        db_path: db_path_str,
        root: install_root.to_str().unwrap(),
        dry_run: false,
        sandbox_mode: SandboxMode::None,
        no_deps: true,
        no_scripts: true,
        allow_downgrade: false,
        allow_capabilities: true,
        dep_mode: None,
        yes: true,
        dependency_passes_remaining: 0,
        repository_provenance: None,
        legacy_replay: LegacyReplayOptions::default(),
    })
    .await
    .unwrap();

    let conn = conary_core::db::open(db_path_str).unwrap();
    let trove_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM troves WHERE name = 'converted-allowed-capability'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(trove_count, 1);
}

mod dependencies;
