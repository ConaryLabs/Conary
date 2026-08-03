// apps/conary/src/commands/test_helpers.rs

//! Shared test fixtures for command handler tests.
//!
//! Command tests share exact database, repository-authority, selected-root,
//! and generation-artifact fixtures here instead of reconstructing partial
//! package state independently.

use conary_core::db::models::{
    Changeset, ChangesetStatus, Component, FileEntry, InstallSource, InstalledRequirementGroup,
    LabelEntry, PackageResolution, ProvideEntry, Repository, RepositoryPackage,
    RepositoryPackageKey, RepositoryPackageKeyStatus, Trove, TroveType,
};
use conary_core::db::schema;
use conary_core::generation::artifact::{
    ArtifactWriteInputs, BootAssetSources, CasObjectVerification, stage_boot_assets,
    write_generation_artifact,
};
use conary_core::generation::metadata::{GENERATION_FORMAT, GenerationMetadata};
use conary_core::generation::root_manifest::{
    GENERATION_ROOT_MANIFEST_VERSION, GenerationRootEntry, GenerationRootManifest,
    MutableStateManifest,
};
use conary_core::payload::{
    PayloadContentAuthority, PayloadIdentity, PayloadNode, PayloadNodeKind, ResolvedPayloadNode,
};
use conary_core::repository::dependency_model::ProvidedCapability;
use conary_core::repository::dependency_model::{
    ProvideArchitectureQualifier, ProvideVersionRelation, RepositoryCapabilityKind,
    RepositoryRequirementKind,
};
use conary_core::repository::requirement::parse_native_requirement;
use conary_core::repository::versioning::VersionScheme;
use std::path::Path;
use tempfile::TempDir;

pub(crate) fn persist_test_host_capabilities(conn: &rusqlite::Connection) {
    conary_core::ccs::HostCapabilityInventory::default()
        .persist(conn)
        .unwrap();
}

pub(crate) fn exact_package_self_provider(
    name: &str,
    version: &str,
    version_scheme: VersionScheme,
) -> ProvidedCapability {
    ProvidedCapability {
        kind: RepositoryCapabilityKind::PackageName,
        name: name.to_string(),
        version: Some(version.to_string()),
        version_relation: Some(ProvideVersionRelation::Equal),
        version_scheme,
        architecture_qualifier: ProvideArchitectureQualifier::Implicit,
        provenance: conary_core::repository::dependency_model::CapabilityProvenance::ExactIdentity,
    }
}

fn stored_regular_file(
    path: &str,
    hash: String,
    size: i64,
    permissions: u32,
    trove_id: i64,
) -> FileEntry {
    FileEntry::new(
        path.to_string(),
        test_regular_node(permissions),
        Some(PayloadContentAuthority {
            sha256: hash,
            size: u64::try_from(size).unwrap(),
        }),
        trove_id,
    )
}

pub(crate) fn insert_test_parent_directories(
    conn: &rusqlite::Connection,
    path: &str,
    trove_id: i64,
    component_id: Option<i64>,
) {
    let path = Path::new(path);
    assert!(path.is_absolute(), "fixture path must be absolute");
    let mut parents = path
        .ancestors()
        .skip(1)
        .filter(|parent| *parent != Path::new("/"))
        .collect::<Vec<_>>();
    parents.reverse();

    for parent in parents {
        let parent = parent.to_str().unwrap();
        if let Some(existing) = FileEntry::find_by_path(conn, parent).unwrap() {
            assert!(
                matches!(existing.node.source.kind, PayloadNodeKind::Directory),
                "fixture parent {parent} must be a directory"
            );
            continue;
        }

        let mut entry = FileEntry::new(parent.to_string(), test_directory_node(), None, trove_id);
        entry.component_id = component_id;
        entry.insert(conn).unwrap();
    }
}

pub(crate) fn insert_test_regular_file_with_parents(
    conn: &rusqlite::Connection,
    db_path: &Path,
    path: &str,
    content: &[u8],
    permissions: u32,
    trove_id: i64,
    component_id: Option<i64>,
) -> FileEntry {
    insert_test_parent_directories(conn, path, trove_id, component_id);
    let runtime_root =
        conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(db_path.to_path_buf());
    let sha256 = conary_core::filesystem::CasStore::new(runtime_root.objects_dir())
        .unwrap()
        .store(content)
        .unwrap();
    let mut entry = stored_regular_file(
        path,
        sha256,
        i64::try_from(content.len()).unwrap(),
        permissions,
        trove_id,
    );
    entry.component_id = component_id;
    entry.insert(conn).unwrap();
    entry
}

pub(crate) fn create_test_db() -> (TempDir, String) {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    let db_path_string = db_path.display().to_string();
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
    schema::ensure_current(&conn).unwrap();
    persist_test_host_capabilities(&conn);
    drop(conn);
    stage_test_boot_assets(temp_dir.path());
    (temp_dir, db_path_string)
}

pub(crate) fn setup_command_test_db() -> (TempDir, String) {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir
        .path()
        .join("test.db")
        .to_str()
        .unwrap()
        .to_string();

    conary_core::db::init(&db_path).unwrap();
    stage_test_boot_assets(temp_dir.path());
    let cas = conary_core::filesystem::CasStore::new(temp_dir.path().join("objects")).unwrap();
    let nginx_binary = b"test nginx binary";
    let nginx_binary_hash = cas.store(nginx_binary).unwrap();
    let nginx_binary_size = i64::try_from(nginx_binary.len()).unwrap();
    let init_binary = b"test init binary";
    let init_binary_hash = cas.store(init_binary).unwrap();
    let init_binary_size = i64::try_from(init_binary.len()).unwrap();
    let nginx_config_contents = b"worker_processes 1;\n";
    let nginx_config_hash = cas.store(nginx_config_contents).unwrap();
    let nginx_config_size = i64::try_from(nginx_config_contents.len()).unwrap();
    let mut conn = conary_core::db::open(&db_path).unwrap();
    persist_test_host_capabilities(&conn);

    conary_core::db::transaction(&mut conn, |tx| {
        let mut changeset1 = Changeset::new("Install nginx-1.24.0".to_string());
        let changeset1_id = changeset1.insert(tx)?;

        let mut nginx = Trove::new(
            "nginx".to_string(),
            "1.24.0".to_string(),
            TroveType::Package,
        conary_core::repository::versioning::VersionScheme::Conary);
        nginx.architecture = Some("x86_64".to_string());
        nginx.description = Some("High performance web server".to_string());
        nginx.installed_by_changeset_id = Some(changeset1_id);
        let nginx_id = nginx.insert(tx)?;

        let mut nginx_runtime = Component::new(nginx_id, "runtime".to_string());
        let runtime_id = nginx_runtime.insert(tx)?;

        let mut nginx_config = Component::new(nginx_id, "config".to_string());
        let config_id = nginx_config.insert(tx)?;

        tx.execute(
            "INSERT OR IGNORE INTO file_contents (sha256_hash, content_path, size) VALUES (?1, ?2, ?3)",
            rusqlite::params![
                &nginx_binary_hash,
                format!("objects/{}/{}", &nginx_binary_hash[0..2], &nginx_binary_hash[2..]),
                nginx_binary_size
            ],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO file_contents (sha256_hash, content_path, size) VALUES (?1, ?2, ?3)",
            rusqlite::params![
                &nginx_config_hash,
                format!("objects/{}/{}", &nginx_config_hash[0..2], &nginx_config_hash[2..]),
                nginx_config_size
            ],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO file_contents (sha256_hash, content_path, size) VALUES (?1, ?2, ?3)",
            rusqlite::params![
                &init_binary_hash,
                format!("objects/{}/{}", &init_binary_hash[0..2], &init_binary_hash[2..]),
                init_binary_size
            ],
        )?;

        insert_test_parent_directories(tx, "/usr/sbin/nginx", nginx_id, Some(runtime_id));
        let mut f1 = stored_regular_file(
            "/usr/sbin/nginx",
            nginx_binary_hash.clone(),
            nginx_binary_size,
            0o755,
            nginx_id,
        );
        f1.component_id = Some(runtime_id);
        f1.insert(tx)?;

        insert_test_parent_directories(tx, "/sbin/init", nginx_id, Some(runtime_id));
        let mut init = stored_regular_file(
            "/sbin/init",
            init_binary_hash.clone(),
            init_binary_size,
            0o755,
            nginx_id,
        );
        init.component_id = Some(runtime_id);
        init.insert(tx)?;

        insert_test_parent_directories(tx, "/etc/nginx/nginx.conf", nginx_id, Some(config_id));
        let mut f2 = stored_regular_file(
            "/etc/nginx/nginx.conf",
            nginx_config_hash.clone(),
            nginx_config_size,
            0o644,
            nginx_id,
        );
        f2.component_id = Some(config_id);
        f2.insert(tx)?;

        let mut p1 = ProvideEntry::new(
            nginx_id,
            "nginx".to_string(),
            Some("1.24.0".to_string()),
            VersionScheme::Conary,
        );
        p1.insert(tx)?;
        let mut p2 = ProvideEntry::new(
            nginx_id,
            "webserver".to_string(),
            None,
            VersionScheme::Conary,
        );
        p2.insert(tx)?;

        let dependency = parse_native_requirement(
            RepositoryRequirementKind::Depends,
            VersionScheme::Conary,
            "openssl >= 3.0.0",
        )
        .map_err(conary_core::Error::ParseError)?;
        InstalledRequirementGroup::insert_groups(
            tx,
            nginx_id,
            VersionScheme::Conary,
            &[dependency],
        )?;

        changeset1.update_status(tx, ChangesetStatus::Applied)?;

        let mut changeset2 = Changeset::new("Install openssl-3.0.0".to_string());
        let changeset2_id = changeset2.insert(tx)?;

        let mut openssl = Trove::new(
            "openssl".to_string(),
            "3.0.0".to_string(),
            TroveType::Package,
        conary_core::repository::versioning::VersionScheme::Conary);
        openssl.architecture = Some("x86_64".to_string());
        openssl.description = Some("Cryptography and SSL/TLS toolkit".to_string());
        openssl.installed_by_changeset_id = Some(changeset2_id);
        let openssl_id = openssl.insert(tx)?;

        let mut openssl_runtime = Component::new(openssl_id, "runtime".to_string());
        openssl_runtime.insert(tx)?;

        let mut p3 = ProvideEntry::new(
            openssl_id,
            "openssl".to_string(),
            Some("3.0.0".to_string()),
            VersionScheme::Conary,
        );
        p3.insert(tx)?;
        let mut p4 = ProvideEntry::new(
            openssl_id,
            "soname(libssl.so.3)".to_string(),
            None,
            VersionScheme::Conary,
        );
        p4.insert(tx)?;

        changeset2.update_status(tx, ChangesetStatus::Applied)?;

        Ok(())
    })
    .unwrap();

    (temp_dir, db_path)
}

pub(crate) fn seed_test_bootable_runtime(db_path: &Path) -> i64 {
    let runtime_root =
        conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(db_path.to_path_buf());
    stage_test_boot_assets(runtime_root.root());
    let conn = conary_core::db::open(db_path).unwrap();
    persist_test_host_capabilities(&conn);
    if let Some(init) = FileEntry::find_by_path(&conn, "/sbin/init").unwrap() {
        return init.trove_id;
    }

    let mut trove = Trove::new(
        "test-runtime-base".to_string(),
        "1.0.0".to_string(),
        TroveType::Package,
        VersionScheme::Conary,
    );
    trove.architecture = Some("x86_64".to_string());
    trove.install_reason = conary_core::db::models::InstallReason::Explicit;
    trove.selection_reason = Some("Test runtime root authority".to_string());
    let trove_id = trove.insert(&conn).unwrap();
    insert_test_regular_file_with_parents(
        &conn,
        db_path,
        "/sbin/init",
        b"test init binary",
        0o755,
        trove_id,
        None,
    );
    trove_id
}

pub(crate) fn insert_test_static_ccs_repository(
    conn: &rusqlite::Connection,
    name: &str,
    url: &str,
) -> i64 {
    let mut repository = Repository::new(name.to_string(), url.to_string());
    repository.default_strategy = Some("static".to_string());
    let repository_id = repository.insert(conn).unwrap();
    let signing_key = crate::commands::ccs::load_or_create_local_dev_key().unwrap();
    RepositoryPackageKey::replace_for_repository(
        conn,
        repository_id,
        &[RepositoryPackageKey {
            repository_id,
            public_key: signing_key.public_key_base64(),
            key_id: signing_key.key_id().map(str::to_string),
            status: RepositoryPackageKeyStatus::Active,
            synced_at: None,
        }],
    )
    .unwrap();
    repository_id
}

pub(crate) fn create_active_test_generation(db_path: &Path, generation: i64) {
    let runtime = conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(db_path.to_path_buf());
    let runtime_root = runtime.root();
    stage_test_boot_assets(runtime_root);
    let generation_dir = runtime_root
        .join("generations")
        .join(generation.to_string());
    let objects_dir = runtime_root.join("objects");
    std::fs::create_dir_all(&generation_dir).unwrap();
    let init_bytes = b"test init binary";
    let init_hash = conary_core::filesystem::CasStore::new(&objects_dir)
        .unwrap()
        .store(init_bytes)
        .unwrap();
    write_test_root_manifests(&generation_dir, &init_hash, init_bytes.len());

    let erofs_path = generation_dir.join("root.erofs");
    std::fs::write(&erofs_path, b"test-root-erofs").unwrap();
    let boot_root = runtime_root.join("boot");
    let boot_assets = stage_boot_assets(BootAssetSources {
        generation_dir: &generation_dir,
        generation,
        architecture: "x86_64",
        kernel_version: "test-kernel",
        kernel: &boot_root.join("vmlinuz-test-kernel"),
        initramfs: &boot_root.join("initramfs-test-kernel.img"),
        efi_bootloader: &boot_root.join("EFI/BOOT/BOOTX64.EFI"),
    })
    .unwrap();
    let artifact_manifest_sha256 = write_generation_artifact(ArtifactWriteInputs {
        generation_dir: &generation_dir,
        generation,
        architecture: "x86_64",
        erofs_path: &erofs_path,
        cas_base_rel: "../../objects",
        cas_verification: CasObjectVerification::AlreadyVerified,
        boot_assets,
        carrier_capabilities: Default::default(),
    })
    .unwrap();
    GenerationMetadata {
        generation,
        format: GENERATION_FORMAT.to_string(),
        erofs_size: Some(i64::try_from(std::fs::metadata(&erofs_path).unwrap().len()).unwrap()),
        cas_objects_referenced: Some(1),
        fsverity_enabled: false,
        erofs_verity_digest: None,
        artifact_manifest_sha256: Some(artifact_manifest_sha256),
        security_capability_xattr_count: None,
        created_at: "2026-07-25T00:00:00Z".to_string(),
        package_count: 1,
        kernel_version: Some("test-kernel".to_string()),
        summary: "active test generation".to_string(),
    }
    .write_to(&generation_dir)
    .unwrap();

    let conn = conary_core::db::open(db_path).unwrap();
    let state = conary_core::db::models::StateEngine::new(&conn)
        .create_snapshot_at(
            generation,
            "active test generation",
            Some("Paired test generation artifact and database state authority"),
            None,
        )
        .unwrap();
    assert_eq!(state.state_number, generation);

    let current = runtime_root.join("current");
    if std::fs::symlink_metadata(&current).is_ok() {
        std::fs::remove_file(&current).unwrap();
    }
    std::os::unix::fs::symlink(format!("generations/{generation}"), current).unwrap();
}

fn write_test_root_manifests(generation_dir: &Path, init_hash: &str, init_size: usize) {
    let root = test_directory_node();
    GenerationRootManifest {
        version: GENERATION_ROOT_MANIFEST_VERSION,
        root: root.clone(),
        entries: vec![
            GenerationRootEntry {
                path: "/sbin".to_string(),
                node: root,
                content: None,
            },
            GenerationRootEntry {
                path: "/sbin/init".to_string(),
                node: test_regular_node(0o755),
                content: Some(PayloadContentAuthority {
                    sha256: init_hash.to_string(),
                    size: init_size as u64,
                }),
            },
        ],
    }
    .write_to(generation_dir)
    .unwrap();
    MutableStateManifest::empty()
        .write_to(generation_dir)
        .unwrap();
}

fn test_directory_node() -> ResolvedPayloadNode {
    let mut node = test_regular_payload_node(0o755);
    node.kind = PayloadNodeKind::Directory;
    node.mode = libc::S_IFDIR | 0o755;
    ResolvedPayloadNode::from_numeric_source(node).unwrap()
}

fn test_regular_node(permissions: u32) -> ResolvedPayloadNode {
    ResolvedPayloadNode::from_numeric_source(test_regular_payload_node(permissions)).unwrap()
}

pub(crate) fn test_regular_payload_node(permissions: u32) -> PayloadNode {
    let mut node = PayloadNode::regular(permissions);
    node.user = PayloadIdentity::Numeric {
        id: u64::from(unsafe { libc::geteuid() }),
    };
    node.group = PayloadIdentity::Numeric {
        id: u64::from(unsafe { libc::getegid() }),
    };
    node
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

pub(crate) fn seed_mixed_replatform_fixture(conn: &rusqlite::Connection) {
    let mut fedora_repo = Repository::new(
        "fedora".to_string(),
        "https://example.test/fedora".to_string(),
    );
    fedora_repo.source_profile = Some("fedora-44".to_string());
    let fedora_repo_id = fedora_repo.insert(conn).unwrap();

    let mut arch_repo = Repository::new(
        "arch-core".to_string(),
        "https://example.test/arch".to_string(),
    );
    arch_repo.default_strategy = Some("binary".to_string());
    arch_repo.source_profile = Some("arch".to_string());
    let arch_repo_id = arch_repo.insert(conn).unwrap();

    let mut fedora_label = LabelEntry::new(
        "fedora".to_string(),
        "f43".to_string(),
        "stable".to_string(),
    );
    fedora_label.insert(conn).unwrap();
    fedora_label
        .set_repository(conn, Some(fedora_repo_id))
        .unwrap();

    for (name, version) in [("vim", "9.0.1"), ("bash", "5.1.0"), ("zsh", "5.8.0")] {
        let mut trove = Trove::new_with_source(
            name.to_string(),
            version.to_string(),
            TroveType::Package,
            InstallSource::Repository,
            conary_core::repository::versioning::VersionScheme::Conary,
        );
        trove.architecture = Some("x86_64".to_string());
        trove.label_id = fedora_label.id;
        trove.insert(conn).unwrap();
    }

    for (name, version) in [("vim", "9.1.0"), ("bash", "5.2.0"), ("zsh", "5.9.1")] {
        let mut pkg = RepositoryPackage::new(
            arch_repo_id,
            name.to_string(),
            version.to_string(),
            conary_core::repository::versioning::VersionScheme::Arch,
            format!("sha256:{name}"),
            123,
            format!("https://example.test/arch/{name}.pkg.tar.zst"),
        );
        pkg.architecture = Some("x86_64".to_string());
        pkg.insert(conn).unwrap();
    }

    let vim_package_id = RepositoryPackage::find_by_name_version(conn, "vim", "9.1.0")
        .unwrap()
        .unwrap()
        .id
        .unwrap();
    let bash_package_id = RepositoryPackage::find_by_name_version(conn, "bash", "5.2.0")
        .unwrap()
        .unwrap()
        .id
        .unwrap();
    let mut exact_resolution =
        PackageResolution::repository_package(arch_repo_id, "vim".to_string(), vim_package_id);
    exact_resolution.version = Some("9.1.0".to_string());
    exact_resolution.insert(conn).unwrap();

    let mut any_version_resolution =
        PackageResolution::repository_package(arch_repo_id, "bash".to_string(), bash_package_id);
    any_version_resolution.insert(conn).unwrap();
}

/// Run an exact test body with root authority inside an isolated user and
/// mount namespace.
///
/// The outer invocation re-executes the current test binary through
/// `unshare(1)`. The marker proves that the exact named test ran in the child,
/// so a stale test name cannot produce a false green result.
pub(crate) fn run_exact_test_in_user_mount_namespace(test_name: &str) -> bool {
    use std::fs;
    use std::os::unix::process::CommandExt as _;
    use std::process::{Command, Stdio};

    const CHILD_MARKER: &str = "CONARY_USER_MOUNT_NAMESPACE_TEST_MARKER";

    let mount_namespace_available = || {
        let mut command = Command::new("/bin/true");
        // Safety: this child-only probe performs no work after unsharing.
        unsafe {
            command.pre_exec(|| {
                nix::sched::unshare(nix::sched::CloneFlags::CLONE_NEWNS)
                    .map_err(std::io::Error::from)
            });
        }
        command.status().is_ok()
    };

    if nix::unistd::geteuid().is_root() && mount_namespace_available() {
        if let Some(marker) = std::env::var_os(CHILD_MARKER) {
            fs::write(marker, test_name).expect("record user-mount namespace test execution");
        }
        return true;
    }
    assert!(
        std::env::var_os(CHILD_MARKER).is_none(),
        "user-mount namespace test child lacks a usable mount namespace"
    );

    let namespace_args = [
        "--user",
        "--map-root-user",
        "--mount",
        "--propagation",
        "private",
    ];
    let probe = Command::new("unshare")
        .args(namespace_args)
        .arg("/bin/true")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("probe user and mount namespace support with unshare");
    assert!(
        probe.success(),
        "this exact ownership test requires usable user and mount namespaces"
    );

    let marker = tempfile::NamedTempFile::new().expect("namespace test execution marker");
    let status = Command::new("unshare")
        .args(namespace_args)
        .arg(std::env::current_exe().expect("current test executable"))
        .arg(test_name)
        .args(["--exact", "--nocapture"])
        .env(CHILD_MARKER, marker.path())
        .status()
        .expect("run exact test in a user and mount namespace");
    assert!(
        status.success(),
        "user-mount namespace child failed for {test_name}"
    );
    assert_eq!(
        fs::read_to_string(marker.path()).expect("read namespace test execution marker"),
        test_name,
        "user-mount namespace child matched no exact test"
    );
    false
}
