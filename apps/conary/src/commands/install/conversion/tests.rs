// apps/conary/src/commands/install/conversion/tests.rs

use super::*;
use conary_core::capability::{
    CapabilityDeclaration, FilesystemCapabilities, NetworkCapabilities, SyscallCapabilities,
};
use conary_core::ccs::SigningKeyPair;
use conary_core::ccs::builder::write_signed_current_ccs_package;
use conary_core::ccs::manifest::{DirectoryHook, ScriptHook};
use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry};
use conary_core::db::models::{
    Repository, RepositoryPackageKey, RepositoryPackageKeyStatus, Trove,
};
use conary_core::hash;
use conary_core::packages::traits::{ExtractedFile, PackageFile, PackageFormat};
use conary_core::payload::{
    PayloadContentAuthority, PayloadIdentity, PayloadNode, PayloadNodeKind, PayloadTimestamp,
};
use conary_core::repository::RepositorySourceKind;
use conary_core::repository::dependency_model::ProvidedCapability;
use std::collections::HashMap;

struct FakeNativePackage {
    name: String,
    version: String,
    description: Option<String>,
    files: Vec<PackageFile>,
    extracted_files: Vec<ExtractedFile>,
    provides: Vec<ProvidedCapability>,
}

fn test_regular_node(permissions: u32) -> PayloadNode {
    let mut node = PayloadNode::regular(permissions);
    node.user = conary_core::payload::PayloadIdentity::Numeric {
        id: u64::from(unsafe { libc::geteuid() }),
    };
    node.group = conary_core::payload::PayloadIdentity::Numeric {
        id: u64::from(unsafe { libc::getegid() }),
    };
    node
}

impl FakeNativePackage {
    fn nginx() -> Self {
        let content = b"#!/bin/sh\nexec true\n".to_vec();
        let content_authority = PayloadContentAuthority {
            sha256: hash::sha256(&content),
            size: content.len() as u64,
        };
        Self {
            name: "nginx".to_string(),
            version: "1.0.0".to_string(),
            description: Some("fake nginx native package".to_string()),
            files: vec![PackageFile {
                path: "/usr/sbin/nginx".to_string(),
                node: test_regular_node(0o755),
                content: Some(content_authority.clone()),
            }],
            extracted_files: vec![ExtractedFile {
                path: "/usr/sbin/nginx".to_string(),
                node: test_regular_node(0o755),
                content,
                content_authority: Some(content_authority),
            }],
            provides: Vec::new(),
        }
    }
}

impl PackageFormat for FakeNativePackage {
    fn parse(_path: &str) -> conary_core::Result<Self> {
        unimplemented!("test fake package is constructed directly")
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn version(&self) -> &str {
        &self.version
    }

    fn version_scheme(&self) -> conary_core::repository::versioning::VersionScheme {
        conary_core::repository::versioning::VersionScheme::Rpm
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

    fn requirements(
        &self,
    ) -> &[conary_core::repository::dependency_model::RepositoryRequirementGroup] {
        &[]
    }

    fn resolution_capabilities(&self) -> conary_core::Result<Vec<ProvidedCapability>> {
        Ok(self.provides.clone())
    }

    fn package_payload(&self) -> conary_core::Result<conary_core::packages::PackagePayload> {
        conary_core::packages::PackagePayload::from_extracted_in_memory(
            self.extracted_files.clone(),
        )
    }

    fn to_trove(&self) -> Trove {
        Trove::new(
            self.name.clone(),
            self.version.clone(),
            conary_core::db::models::TroveType::Package,
            conary_core::repository::versioning::VersionScheme::Rpm,
        )
    }
}

fn regular_ccs_file(path: &str, sha256: String, size: u64, mode: u32) -> FileEntry {
    let mut node = PayloadNode::regular(mode);
    node.user = PayloadIdentity::Numeric {
        id: u64::from(unsafe { libc::geteuid() }),
    };
    node.group = PayloadIdentity::Numeric {
        id: u64::from(unsafe { libc::getegid() }),
    };
    FileEntry {
        path: path.to_string(),
        node,
        content: Some(PayloadContentAuthority { sha256, size }),
        component: "runtime".to_string(),
        chunks: None,
    }
}

fn symlink_ccs_file(path: &str, target: String, mode: u32) -> FileEntry {
    FileEntry {
        path: path.to_string(),
        node: PayloadNode {
            kind: PayloadNodeKind::Symlink { target },
            mode: libc::S_IFLNK | (mode & 0o7777),
            user: PayloadIdentity::Numeric {
                id: u64::from(unsafe { libc::geteuid() }),
            },
            group: PayloadIdentity::Numeric {
                id: u64::from(unsafe { libc::getegid() }),
            },
            mtime: PayloadTimestamp::UNIX_EPOCH,
            xattrs: Default::default(),
        },
        content: None,
        component: "runtime".to_string(),
        chunks: None,
    }
}

fn stage_test_boot_assets(root: &std::path::Path) {
    let kernel_version = "test-kernel";
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

fn published_or_pending_generation_has_path(db_path: &std::path::Path, path: &str) -> bool {
    let runtime_root =
        conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(db_path.to_path_buf());
    if let Some(generation) =
        conary_core::generation::mount::current_generation(runtime_root.root()).unwrap()
    {
        let artifact = conary_core::generation::artifact::load_generation_artifact(
            &runtime_root.generation_path(generation),
        )
        .unwrap();
        return artifact
            .generation_root
            .entries
            .iter()
            .chain(&artifact.mutable_state.entries)
            .any(|entry| entry.path == path);
    }

    let conn = conary_core::db::open(db_path).unwrap();
    let debt = conary_core::db::models::GenerationPublication::pending_recoverable(&conn)
        .unwrap()
        .pop()
        .expect("converted install should retain exact publication debt");
    let captured = crate::commands::generation::selected_root::load_publication_candidate(
        &runtime_root,
        &debt,
    )
    .unwrap();
    captured
        .generation
        .entries
        .iter()
        .chain(&captured.state.entries)
        .any(|entry| entry.path == path)
}

fn write_runtime_ccs_package(
    temp_dir: &std::path::Path,
    name: &str,
    mut manifest: CcsManifest,
) -> std::path::PathBuf {
    let package_path = temp_dir.join(format!("{name}.ccs"));
    let init_content = b"#!/bin/sh\nexec true\n".to_vec();
    let init_hash = hash::sha256(&init_content);
    let files = vec![regular_ccs_file(
        "/usr/sbin/init",
        init_hash.clone(),
        init_content.len() as u64,
        0o755,
    )];
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
        files: files.clone(),
        payloads: conary_core::ccs::builder::payloads_from_bounded_memory_for_tests(
            &files,
            HashMap::from([(init_hash, init_content)]),
        )
        .unwrap(),
        total_size: 0,
        chunked: false,
        chunk_stats: None,
    };
    let signing_key = crate::commands::ccs::load_or_create_local_dev_key().unwrap();
    write_signed_current_ccs_package(&result, &package_path, &signing_key, true).unwrap();
    package_path
}

fn write_runtime_signed_ccs_package(
    temp_dir: &std::path::Path,
    name: &str,
    mut manifest: CcsManifest,
    signing_key: &SigningKeyPair,
) -> std::path::PathBuf {
    let package_path = temp_dir.join(format!("{name}.ccs"));
    let init_content = b"#!/bin/sh\nexec true\n".to_vec();
    let init_hash = hash::sha256(&init_content);
    let files = vec![regular_ccs_file(
        "/usr/sbin/init",
        init_hash.clone(),
        init_content.len() as u64,
        0o755,
    )];
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
        files: files.clone(),
        payloads: conary_core::ccs::builder::payloads_from_bounded_memory_for_tests(
            &files,
            HashMap::from([(init_hash, init_content)]),
        )
        .unwrap(),
        total_size: 0,
        chunked: false,
        chunk_stats: None,
    };
    write_signed_current_ccs_package(&result, &package_path, signing_key, false).unwrap();
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
        source_profile: None,
        version_scheme: conary_core::repository::versioning::VersionScheme::Conary,
        source_kind: RepositorySourceKind::Static,
    }
}

fn test_resolution_policy() -> conary_core::repository::resolution_policy::ResolutionPolicy {
    conary_core::repository::resolution_policy::ResolutionPolicy::new()
        .with_mixing(conary_core::repository::resolution_policy::DependencyMixingPolicy::Permissive)
}

fn converted_install_options<'a>(
    ccs_path: &'a std::path::Path,
    db_path: &'a str,
    install_root: &'a std::path::Path,
    repository_provenance: Option<RepositoryInstallProvenance>,
) -> CcsArtifactInstallOptions<'a> {
    let envelope_authority = if repository_provenance.is_some() {
        CcsEnvelopeAuthority::Repository
    } else {
        CcsEnvelopeAuthority::LocalDev
    };
    CcsArtifactInstallOptions {
        ccs_path: ccs_path.to_str().unwrap(),
        db_path,
        root: install_root.to_str().unwrap(),
        dry_run: false,
        sandbox_mode: SandboxMode::Always,
        no_deps: true,
        allow_downgrade: false,
        intent: InstallIntent::PackageChange,
        yes: true,
        envelope_authority,
        repository_provenance,
        resolution_policy: test_resolution_policy(),
    }
}

#[tokio::test]
async fn try_convert_to_ccs_does_not_guess_capability_policy() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();
    let native_path = temp_dir.path().join("nginx.rpm");

    conary_core::db::init(db_path_str).unwrap();
    let native_fixture = rpm::PackageBuilder::new("nginx", "1.0.0", "MIT", "x86_64", "RPM fixture")
        .build()
        .unwrap();
    native_fixture.write_file(&native_path).unwrap();

    let package = FakeNativePackage::nginx();
    let result = try_convert_to_ccs(
        &package,
        &native_path,
        PackageFormatType::Rpm,
        db_path_str,
        &test_resolution_policy(),
    )
    .await
    .expect("conversion without declared capabilities must not require policy approval");
    let ConversionResult::Converted { ccs_path, .. } = result else {
        panic!("conversion unexpectedly skipped");
    };
    let verified = verify_ccs_package_authority(
        db_path_str,
        Path::new(&ccs_path),
        &CcsEnvelopeAuthority::LocalDev,
        None,
    )
    .unwrap();
    let converted = CcsPackage::from_verified_archive(&ccs_path, &verified).unwrap();
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
    assert_eq!(
        converted_count, 0,
        "conversion evidence must remain pending until installation commits"
    );
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

    install_ccs_artifact(CcsArtifactInstallOptions {
        ccs_path: package_path.to_str().unwrap(),
        db_path: db_path_str,
        root: install_root.to_str().unwrap(),
        dry_run: false,
        sandbox_mode: SandboxMode::Always,
        no_deps: true,
        allow_downgrade: false,
        intent: InstallIntent::PackageChange,
        yes: true,
        envelope_authority: CcsEnvelopeAuthority::LocalDev,
        repository_provenance: None,
        resolution_policy: test_resolution_policy(),
    })
    .await
    .unwrap();

    assert!(!install_root.join("var/lib/converted-hooked").exists());
    assert!(published_or_pending_generation_has_path(
        &db_path,
        "/var/lib/converted-hooked"
    ));
}

#[tokio::test]
async fn static_repo_ccs_install_rejects_repository_without_package_authority() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    std::fs::create_dir_all(&install_root).unwrap();
    conary_core::db::init(db_path_str).unwrap();
    stage_test_boot_assets(temp_dir.path());
    let repo_id = insert_static_repository_with_keys(db_path_str, &[]);
    let unlisted_key = SigningKeyPair::generate().with_key_id("unlisted");
    let package_path = write_runtime_signed_ccs_package(
        temp_dir.path(),
        "static-no-authority",
        CcsManifest::new_minimal("static-no-authority", "1.0.0"),
        &unlisted_key,
    );

    let err = install_ccs_artifact(converted_install_options(
        &package_path,
        db_path_str,
        &install_root,
        Some(static_provenance(repo_id)),
    ))
    .await
    .unwrap_err();

    assert!(
        format!("{err:?}").contains("no verified package-authority keys"),
        "repository without package authority must fail before CCS install: {err:?}"
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
        &unlisted_key,
    );

    let err = install_ccs_artifact(converted_install_options(
        &package_path,
        db_path_str,
        &install_root,
        Some(static_provenance(repo_id)),
    ))
    .await
    .unwrap_err();

    assert!(
        format!("{err:?}").contains("package signer is not trusted"),
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
        &active_key,
    );

    install_ccs_artifact(converted_install_options(
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
        &retired_key,
    );

    let err = install_ccs_artifact(converted_install_options(
        &package_path,
        db_path_str,
        &install_root,
        Some(static_provenance(repo_id)),
    ))
    .await
    .unwrap_err();

    assert!(
        format!("{err:?}").contains("no verified package-authority keys"),
        "static package signed only by a retired key should fail: {err:?}"
    );
}

#[tokio::test]
async fn non_static_repo_ccs_install_requires_repository_package_authority() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    std::fs::create_dir_all(&install_root).unwrap();
    conary_core::db::init(db_path_str).unwrap();
    stage_test_boot_assets(temp_dir.path());
    let unlisted_key = SigningKeyPair::generate().with_key_id("remi-unlisted");
    let package_path = write_runtime_signed_ccs_package(
        temp_dir.path(),
        "remi-no-authority",
        CcsManifest::new_minimal("remi-no-authority", "1.0.0"),
        &unlisted_key,
    );
    let repo_id = insert_static_repository_with_keys(db_path_str, &[]);
    let provenance = RepositoryInstallProvenance {
        repository_id: repo_id,
        source_profile: Some("fedora-44".to_string()),
        version_scheme: conary_core::repository::versioning::VersionScheme::Rpm,
        source_kind: RepositorySourceKind::Remi,
    };

    let error = install_ccs_artifact(converted_install_options(
        &package_path,
        db_path_str,
        &install_root,
        Some(provenance),
    ))
    .await
    .unwrap_err();
    assert!(
        format!("{error:?}").contains("no verified package-authority keys"),
        "all repository CCS installs require exact package authority: {error:?}"
    );
}

#[tokio::test]
async fn converted_ccs_install_rolls_back_post_hook_failure() {
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

    let error = install_ccs_artifact(CcsArtifactInstallOptions {
        ccs_path: package_path.to_str().unwrap(),
        db_path: db_path_str,
        root: install_root.to_str().unwrap(),
        dry_run: false,
        sandbox_mode: SandboxMode::Always,
        no_deps: true,
        allow_downgrade: false,
        intent: InstallIntent::PackageChange,
        yes: true,
        envelope_authority: CcsEnvelopeAuthority::LocalDev,
        repository_provenance: None,
        resolution_policy: test_resolution_policy(),
    })
    .await
    .unwrap_err();
    assert!(
        format!("{error:#}").contains("post-install hooks failed"),
        "unexpected post-install error: {error:#}"
    );

    let conn = conary_core::db::open(db_path_str).unwrap();
    let changesets: i64 = conn
        .query_row("SELECT COUNT(*) FROM changesets", [], |row| row.get(0))
        .unwrap();
    let troves: i64 = conn
        .query_row("SELECT COUNT(*) FROM troves", [], |row| row.get(0))
        .unwrap();
    assert_eq!(changesets, 0);
    assert_eq!(troves, 0);
    assert!(
        !install_root
            .join("usr/bin/converted-post-hook-fails")
            .exists()
    );
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
    let child_content = b"should not persist".to_vec();
    let child_hash = hash::sha256(&child_content);
    let init_content = b"#!/bin/sh\nexec true\n".to_vec();
    let init_hash = hash::sha256(&init_content);
    let files = vec![
        symlink_ccs_file("/usr/lib/link", link_target.clone(), 0o777),
        regular_ccs_file(
            "/usr/lib/link/child",
            child_hash.clone(),
            child_content.len() as u64,
            0o644,
        ),
        regular_ccs_file(
            "/usr/sbin/init",
            init_hash.clone(),
            init_content.len() as u64,
            0o755,
        ),
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
                size: (child_content.len() + init_content.len()) as u64,
            },
        )]),
        files: files.clone(),
        payloads: conary_core::ccs::builder::payloads_from_bounded_memory_for_tests(
            &files,
            HashMap::from([(child_hash, child_content), (init_hash, init_content)]),
        )
        .unwrap(),
        total_size: 0,
        chunked: false,
        chunk_stats: None,
    };
    let signing_key = crate::commands::ccs::load_or_create_local_dev_key().unwrap();
    write_signed_current_ccs_package(&result, &package_path, &signing_key, true).unwrap();

    let err = install_ccs_artifact(CcsArtifactInstallOptions {
        ccs_path: package_path.to_str().unwrap(),
        db_path: db_path_str,
        root: install_root.to_str().unwrap(),
        dry_run: false,
        sandbox_mode: SandboxMode::Always,
        no_deps: true,
        allow_downgrade: false,
        intent: InstallIntent::PackageChange,
        yes: true,
        envelope_authority: CcsEnvelopeAuthority::LocalDev,
        repository_provenance: None,
        resolution_policy: test_resolution_policy(),
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
    let child_content = b"should not persist".to_vec();
    let child_hash = hash::sha256(&child_content);
    let init_content = b"#!/bin/sh\nexec true\n".to_vec();
    let init_hash = hash::sha256(&init_content);
    let files = vec![
        regular_ccs_file(
            "/usr/lib/link/child",
            child_hash.clone(),
            child_content.len() as u64,
            0o644,
        ),
        symlink_ccs_file("/usr/lib/link", link_target.clone(), 0o777),
        regular_ccs_file(
            "/usr/sbin/init",
            init_hash.clone(),
            init_content.len() as u64,
            0o755,
        ),
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
                size: (child_content.len() + init_content.len()) as u64,
            },
        )]),
        files: files.clone(),
        payloads: conary_core::ccs::builder::payloads_from_bounded_memory_for_tests(
            &files,
            HashMap::from([(child_hash, child_content), (init_hash, init_content)]),
        )
        .unwrap(),
        total_size: 0,
        chunked: false,
        chunk_stats: None,
    };
    let signing_key = crate::commands::ccs::load_or_create_local_dev_key().unwrap();
    write_signed_current_ccs_package(&result, &package_path, &signing_key, true).unwrap();

    let err = install_ccs_artifact(CcsArtifactInstallOptions {
        ccs_path: package_path.to_str().unwrap(),
        db_path: db_path_str,
        root: install_root.to_str().unwrap(),
        dry_run: false,
        sandbox_mode: SandboxMode::Always,
        no_deps: true,
        allow_downgrade: false,
        intent: InstallIntent::PackageChange,
        yes: true,
        envelope_authority: CcsEnvelopeAuthority::LocalDev,
        repository_provenance: None,
        resolution_policy: test_resolution_policy(),
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

mod authority;
mod capabilities;
mod dependencies;
