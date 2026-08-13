// conary/src/commands/install/inner/tests.rs

use super::*;
use crate::commands::PackageFormatType;
use crate::commands::install::{
    ExtractionResult, InstallSemantics, RepositoryInstallProvenance, TransactionContext,
};
use conary_core::db::models::{
    Changeset, ConfigFile, ConfigSource, FileEntry, InstallSource, InstalledFileCapability,
    Repository, Trove, TroveType,
};
use conary_core::packages::config_authority::{ConfigPayloadAssociation, SourceConfigDeclaration};
use conary_core::packages::traits::{ExtractedFile, PackageFile, PackageFormat};
use conary_core::payload::{
    PayloadContentAuthority, PayloadIdentity, PayloadNode, PayloadNodeKind, PayloadTimestamp,
    ResolvedPayloadNode,
};
use conary_core::repository::versioning::VersionScheme;
use conary_core::transaction::{TransactionConfig, TransactionEngine};
use std::collections::{BTreeMap, HashMap};

fn payload_node(kind: PayloadNodeKind, mode: u32) -> PayloadNode {
    PayloadNode {
        kind,
        mode,
        user: PayloadIdentity::Numeric { id: 0 },
        group: PayloadIdentity::Numeric { id: 0 },
        mtime: PayloadTimestamp::UNIX_EPOCH,
        xattrs: BTreeMap::new(),
    }
}

fn symlink_node(target: &str) -> PayloadNode {
    payload_node(
        PayloadNodeKind::Symlink {
            target: target.to_string(),
        },
        libc::S_IFLNK | 0o777,
    )
}

struct FakePackage {
    name: String,
    version: String,
    version_scheme: VersionScheme,
    provides: Vec<conary_core::repository::dependency_model::ProvidedCapability>,
    files: Vec<PackageFile>,
    extracted_files: Vec<ExtractedFile>,
    config_declarations: Vec<SourceConfigDeclaration>,
}

impl FakePackage {
    fn with_file(name: &str, path: &str, content: &[u8]) -> Self {
        let node = PayloadNode::regular(0o644);
        let content_authority = PayloadContentAuthority {
            sha256: conary_core::hash::sha256(content),
            size: content.len() as u64,
        };
        Self {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            version_scheme: VersionScheme::Conary,
            provides: vec![crate::commands::test_helpers::exact_package_self_provider(
                name,
                "1.0.0",
                VersionScheme::Conary,
            )],
            files: vec![PackageFile {
                path: path.to_string(),
                node: node.clone(),
                content: Some(content_authority.clone()),
            }],
            extracted_files: vec![ExtractedFile {
                path: path.to_string(),
                node,
                content: content.to_vec(),
                content_authority: Some(content_authority),
            }],
            config_declarations: Vec::new(),
        }
    }

    fn payload_files(&self) -> Vec<conary_core::packages::payload::PackagePayloadFile> {
        conary_core::packages::payload::PackagePayload::from_extracted_in_memory(
            self.extracted_files.clone(),
        )
        .unwrap()
        .into_files()
    }
}

fn file_capability(path: &str) -> conary_core::ccs::manifest::FileCapability {
    conary_core::ccs::manifest::FileCapability {
        path: path.to_string(),
        capabilities: vec!["cap_net_bind_service".to_string()],
        permitted: true,
        effective: true,
        inheritable: false,
    }
}

impl PackageFormat for FakePackage {
    fn parse(_path: &str) -> conary_core::Result<Self> {
        unimplemented!("test package is constructed directly")
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn version(&self) -> &str {
        &self.version
    }

    fn version_scheme(&self) -> conary_core::repository::versioning::VersionScheme {
        self.version_scheme
    }

    fn architecture(&self) -> Option<&str> {
        Some("x86_64")
    }

    fn description(&self) -> Option<&str> {
        None
    }

    fn files(&self) -> &[PackageFile] {
        &self.files
    }

    fn requirements(
        &self,
    ) -> &[conary_core::repository::dependency_model::RepositoryRequirementGroup] {
        &[]
    }

    fn resolution_capabilities(
        &self,
    ) -> conary_core::Result<Vec<conary_core::repository::dependency_model::ProvidedCapability>>
    {
        Ok(self.provides.clone())
    }

    fn package_payload(&self) -> conary_core::Result<conary_core::packages::PackagePayload> {
        conary_core::packages::PackagePayload::from_extracted_in_memory(
            self.extracted_files.clone(),
        )
    }

    fn config_declarations(&self) -> conary_core::Result<Vec<SourceConfigDeclaration>> {
        Ok(self.config_declarations.clone())
    }

    fn to_trove(&self) -> Trove {
        Trove::new(
            self.name.clone(),
            self.version.clone(),
            TroveType::Package,
            self.version_scheme,
        )
    }
}

#[test]
fn install_inner_replaces_live_root_owned_overlapping_path() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let db_path = temp.path().join("conary.db");
    std::fs::create_dir_all(&root).unwrap();
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();

    let mut live_root = Trove::new_with_source(
        "conary-live-root".to_string(),
        "0.0.0-captured-root".to_string(),
        TroveType::Package,
        InstallSource::CapturedRoot,
        conary_core::repository::versioning::VersionScheme::Conary,
    );
    let live_root_id = live_root.insert(&conn).unwrap();
    let mut live_file = FileEntry::new(
        "/boot/grub2/grub.cfg".to_string(),
        ResolvedPayloadNode::from_numeric_source(PayloadNode::regular(0o644)).unwrap(),
        Some(PayloadContentAuthority {
            sha256: conary_core::hash::sha256(b"old!"),
            size: 4,
        }),
        live_root_id,
    );
    live_file.insert(&conn).unwrap();

    let package = FakePackage::with_file("grub2", "/boot/grub2/grub.cfg", b"new-grub");
    let extraction = ExtractionResult {
        extracted_files: package.payload_files(),
        classified: HashMap::from([(
            conary_core::components::ComponentType::Runtime,
            vec!["/boot/grub2/grub.cfg".to_string()],
        )]),
        component_names_by_path: None,
        installed_component_names: None,
        ccs_remove_hook: None,
        installed_component_types: vec![conary_core::components::ComponentType::Runtime],
        skipped_components: Vec::new(),
        language_provides: Vec::new(),
    };
    let db_path_string = db_path.to_string_lossy().into_owned();
    let root_string = root.to_string_lossy().into_owned();
    let ctx = TransactionContext {
        db_path: &db_path_string,
        root: &root_string,
        semantics: InstallSemantics::ccs(VersionScheme::Conary),
        selection_reason: None,
        old_trove_to_upgrade: None,
        ccs_capabilities: None,
        ccs_file_capabilities: None,
        defer_generation: false,
        repository_provenance: None,
        native_lifecycle_bundle: None,
        repository_enrollments: &[],
        relation_removals: &[],
        relation_deconfigurations: &[],
        retain_replaced_payload_until_lifecycle: false,
    };
    let tx_config = TransactionConfig::from_paths(root.clone(), db_path.clone());
    let mut engine = TransactionEngine::new(tx_config).unwrap();
    let tx = conn.unchecked_transaction().unwrap();
    let changeset_id = Changeset::new("Install grub2-1.0.0".to_string())
        .insert(&tx)
        .unwrap();

    install_inner(
        &tx,
        &mut engine,
        changeset_id,
        &package,
        &extraction,
        &ctx,
        &InstallProgress::single("Installing"),
    )
    .unwrap();
    tx.commit().unwrap();

    let owner = FileEntry::find_by_path(&conn, "/boot/grub2/grub.cfg")
        .unwrap()
        .and_then(|file| Trove::find_by_id(&conn, file.trove_id).unwrap())
        .unwrap();
    assert_eq!(owner.name, "grub2");
}

#[test]
fn store_install_files_in_cas_preserves_symlink_targets() {
    let temp = tempfile::tempdir().unwrap();
    let config = TransactionConfig::new(temp.path());
    let engine = TransactionEngine::new(config).unwrap();
    let package = FakePackage {
        name: "fixture".to_string(),
        version: "1.0.0".to_string(),
        version_scheme: VersionScheme::Conary,
        provides: vec![crate::commands::test_helpers::exact_package_self_provider(
            "fixture",
            "1.0.0",
            VersionScheme::Conary,
        )],
        files: vec![],
        extracted_files: vec![ExtractedFile {
            path: "/usr/bin/fixture-link".to_string(),
            node: symlink_node("fixture"),
            content: Vec::new(),
            content_authority: None,
        }],
        config_declarations: Vec::new(),
    };
    let extraction = ExtractionResult {
        extracted_files: package.payload_files(),
        classified: HashMap::from([(
            conary_core::components::ComponentType::Runtime,
            vec!["/usr/bin/fixture-link".to_string()],
        )]),
        component_names_by_path: None,
        installed_component_names: None,
        ccs_remove_hook: None,
        installed_component_types: vec![conary_core::components::ComponentType::Runtime],
        skipped_components: Vec::new(),
        language_provides: Vec::new(),
    };

    let stored = store_install_files_in_cas(engine.cas(), &extraction).unwrap();

    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].path, "/usr/bin/fixture-link");
    assert_eq!(
        stored[0].node.kind,
        PayloadNodeKind::Symlink {
            target: "fixture".to_string()
        }
    );
    assert!(
        stored[0]
            .cas_hash
            .as_deref()
            .is_some_and(|hash| !hash.is_empty())
    );
    assert!(stored[0].content.is_none());
}

#[test]
fn store_install_files_reuses_typed_verified_cas_authority() {
    let temp = tempfile::tempdir().unwrap();
    let cas = conary_core::filesystem::CasStore::new(temp.path().join("objects")).unwrap();
    let bytes = b"already verified canonical bytes";
    let hash = conary_core::hash::sha256(bytes);
    let mut batch = cas
        .verified_object_batch([(hash.clone(), bytes.len() as u64)])
        .unwrap();
    batch
        .ingest(&hash, &mut std::io::Cursor::new(bytes))
        .unwrap();
    let set = std::sync::Arc::new(batch.commit().unwrap());
    let source = conary_core::packages::payload::ReopenablePayload::from_verified_cas_object(
        set,
        hash.clone(),
        bytes.len() as u64,
    )
    .unwrap();
    // Prove storage does not open or reread the canonical object after it has
    // consumed the exact typed identity. Metadata admission remains allowed.
    let canonical = cas.hash_to_path(&hash).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&canonical, std::fs::Permissions::from_mode(0o000)).unwrap();
    }
    let payload = conary_core::packages::payload::PackagePayloadFile::new(
        "/usr/bin/verified".to_string(),
        PayloadNode::regular(0o755),
        Some(PayloadContentAuthority {
            sha256: hash.clone(),
            size: bytes.len() as u64,
        }),
        Some(source),
    )
    .unwrap();

    let stored = store_extracted_files_in_cas(&cas, &[payload]).unwrap();

    assert_eq!(stored[0].cas_hash.as_deref(), Some(hash.as_str()));
    assert!(canonical.exists());
}

#[test]
fn install_inner_persists_declared_config_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let db_path = temp.path().join("conary.db");
    std::fs::create_dir_all(&root).unwrap();
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();

    let mut package = FakePackage::with_file(
        "phase4-runtime-fixture",
        "/etc/fixture/app.conf",
        b"setting=1\n",
    );
    package.version_scheme = VersionScheme::Rpm;
    package.provides = vec![crate::commands::test_helpers::exact_package_self_provider(
        "phase4-runtime-fixture",
        "1.0.0",
        VersionScheme::Rpm,
    )];
    package.config_declarations = vec![SourceConfigDeclaration::Rpm(
        conary_core::packages::rpm::authority::RpmConfigDeclaration {
            header_index: 0,
            path: "/etc/fixture/app.conf".to_string(),
            noreplace: true,
            ghost: false,
            missing_ok: false,
            payload: ConfigPayloadAssociation::Matched,
        },
    )];
    let extraction = ExtractionResult {
        extracted_files: package.payload_files(),
        classified: HashMap::from([(
            conary_core::components::ComponentType::Config,
            vec!["/etc/fixture/app.conf".to_string()],
        )]),
        component_names_by_path: None,
        installed_component_names: None,
        ccs_remove_hook: None,
        installed_component_types: vec![conary_core::components::ComponentType::Config],
        skipped_components: Vec::new(),
        language_provides: Vec::new(),
    };
    let db_path_string = db_path.to_string_lossy().into_owned();
    let root_string = root.to_string_lossy().into_owned();
    let ctx = TransactionContext {
        db_path: &db_path_string,
        root: &root_string,
        semantics: InstallSemantics::native_package(PackageFormatType::Rpm),
        selection_reason: None,
        old_trove_to_upgrade: None,
        ccs_capabilities: None,
        ccs_file_capabilities: None,
        defer_generation: false,
        repository_provenance: None,
        native_lifecycle_bundle: None,
        repository_enrollments: &[],
        relation_removals: &[],
        relation_deconfigurations: &[],
        retain_replaced_payload_until_lifecycle: false,
    };
    let tx_config = TransactionConfig::from_paths(root.clone(), db_path.clone());
    let mut engine = TransactionEngine::new(tx_config).unwrap();
    let tx = conn.unchecked_transaction().unwrap();
    let changeset_id = Changeset::new("Install phase4-runtime-fixture-1.0.0".to_string())
        .insert(&tx)
        .unwrap();

    install_inner(
        &tx,
        &mut engine,
        changeset_id,
        &package,
        &extraction,
        &ctx,
        &InstallProgress::single("Installing"),
    )
    .unwrap();
    tx.commit().unwrap();

    let config = ConfigFile::find_by_path(&conn, "/etc/fixture/app.conf")
        .unwrap()
        .expect("declared config file should be tracked");
    let file = FileEntry::find_by_path(&conn, "/etc/fixture/app.conf")
        .unwrap()
        .expect("config file entry should be tracked");
    assert_eq!(config.file_id, file.id);
    assert_eq!(
        config.original_hash.as_deref(),
        Some(file.content.as_ref().unwrap().sha256.as_str())
    );
    assert_eq!(
        config.current_hash.as_deref(),
        Some(file.content.as_ref().unwrap().sha256.as_str())
    );
    assert!(config.noreplace);
    assert_eq!(config.source, ConfigSource::Rpm);
}

#[test]
fn install_inner_persists_selected_installed_file_capability_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let db_path = temp.path().join("conary.db");
    std::fs::create_dir_all(&root).unwrap();
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();

    let package = FakePackage::with_file("server", "/usr/bin/server", b"server\n");
    let extraction = ExtractionResult {
        extracted_files: package.payload_files(),
        classified: HashMap::from([(
            conary_core::components::ComponentType::Runtime,
            vec!["/usr/bin/server".to_string()],
        )]),
        component_names_by_path: None,
        installed_component_names: None,
        ccs_remove_hook: None,
        installed_component_types: vec![conary_core::components::ComponentType::Runtime],
        skipped_components: Vec::new(),
        language_provides: Vec::new(),
    };
    let file_capabilities = vec![
        file_capability("/usr/bin/server"),
        file_capability("/usr/bin/not-installed"),
    ];
    let db_path_string = db_path.to_string_lossy().into_owned();
    let root_string = root.to_string_lossy().into_owned();
    let ctx = TransactionContext {
        db_path: &db_path_string,
        root: &root_string,
        semantics: InstallSemantics::ccs(VersionScheme::Conary),
        selection_reason: None,
        old_trove_to_upgrade: None,
        ccs_capabilities: None,
        ccs_file_capabilities: Some(&file_capabilities),
        defer_generation: false,
        repository_provenance: None,
        native_lifecycle_bundle: None,
        repository_enrollments: &[],
        relation_removals: &[],
        relation_deconfigurations: &[],
        retain_replaced_payload_until_lifecycle: false,
    };
    let tx_config = TransactionConfig::from_paths(root.clone(), db_path.clone());
    let mut engine = TransactionEngine::new(tx_config).unwrap();
    let tx = conn.unchecked_transaction().unwrap();
    let changeset_id = Changeset::new("Install server-1.0.0".to_string())
        .insert(&tx)
        .unwrap();

    let result = install_inner(
        &tx,
        &mut engine,
        changeset_id,
        &package,
        &extraction,
        &ctx,
        &InstallProgress::single("Installing"),
    )
    .unwrap();
    tx.commit().unwrap();

    let rows = InstalledFileCapability::find_by_trove(&conn, result.trove_id).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].path, "/usr/bin/server");
    assert_eq!(rows[0].capabilities, vec!["cap_net_bind_service"]);
    assert!(rows[0].permitted);
    assert!(rows[0].effective);
    assert!(!rows[0].inheritable);
}

#[test]
fn install_inner_persists_usrmerge_normalized_file_capability_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let db_path = temp.path().join("conary.db");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(root.join("usr/bin")).unwrap();
    std::fs::create_dir_all(root.join("usr/sbin")).unwrap();
    std::os::unix::fs::symlink("usr/bin", root.join("bin")).unwrap();
    std::os::unix::fs::symlink("usr/sbin", root.join("sbin")).unwrap();
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();

    let package = FakePackage::with_file("demo", "/usr/bin/demo", b"demo\n");
    let extraction = ExtractionResult {
        extracted_files: package.payload_files(),
        classified: HashMap::from([(
            conary_core::components::ComponentType::Runtime,
            vec!["/usr/bin/demo".to_string()],
        )]),
        component_names_by_path: None,
        installed_component_names: None,
        ccs_remove_hook: None,
        installed_component_types: vec![conary_core::components::ComponentType::Runtime],
        skipped_components: Vec::new(),
        language_provides: Vec::new(),
    };
    let manifest_file_capabilities = vec![
        file_capability("/bin/demo"),
        file_capability("/sbin/admin-tool"),
    ];
    let normalized_file_capabilities = crate::commands::ccs::normalize_ccs_file_capabilities(
        root.as_path(),
        &manifest_file_capabilities,
    )
    .unwrap();
    let db_path_string = db_path.to_string_lossy().into_owned();
    let root_string = root.to_string_lossy().into_owned();
    let ctx = TransactionContext {
        db_path: &db_path_string,
        root: &root_string,
        semantics: InstallSemantics::ccs(VersionScheme::Conary),
        selection_reason: None,
        old_trove_to_upgrade: None,
        ccs_capabilities: None,
        ccs_file_capabilities: Some(&normalized_file_capabilities),
        defer_generation: false,
        repository_provenance: None,
        native_lifecycle_bundle: None,
        repository_enrollments: &[],
        relation_removals: &[],
        relation_deconfigurations: &[],
        retain_replaced_payload_until_lifecycle: false,
    };
    let tx_config = TransactionConfig::from_paths(root.clone(), db_path.clone());
    let mut engine = TransactionEngine::new(tx_config).unwrap();
    let tx = conn.unchecked_transaction().unwrap();
    let changeset_id = Changeset::new("Install demo-1.0.0".to_string())
        .insert(&tx)
        .unwrap();

    let result = install_inner(
        &tx,
        &mut engine,
        changeset_id,
        &package,
        &extraction,
        &ctx,
        &InstallProgress::single("Installing"),
    )
    .unwrap();
    tx.commit().unwrap();

    let rows = InstalledFileCapability::find_by_trove(&conn, result.trove_id).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].path, "/usr/bin/demo");
}

#[test]
fn install_inner_replaces_installed_file_capability_metadata_on_upgrade() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let db_path = temp.path().join("conary.db");
    std::fs::create_dir_all(&root).unwrap();
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();

    let mut old_trove = Trove::new(
        "server".to_string(),
        "1.0.0".to_string(),
        TroveType::Package,
        conary_core::repository::versioning::VersionScheme::Conary,
    );
    let old_trove_id = old_trove.insert(&conn).unwrap();
    InstalledFileCapability::replace_for_trove(
        &conn,
        old_trove_id,
        &[file_capability("/usr/bin/old-server")],
    )
    .unwrap();

    let mut package = FakePackage::with_file("server", "/usr/bin/server", b"server-v2\n");
    package.version = "2.0.0".to_string();
    package.provides = vec![crate::commands::test_helpers::exact_package_self_provider(
        "server",
        "2.0.0",
        VersionScheme::Conary,
    )];
    let extraction = ExtractionResult {
        extracted_files: package.payload_files(),
        classified: HashMap::from([(
            conary_core::components::ComponentType::Runtime,
            vec!["/usr/bin/server".to_string()],
        )]),
        component_names_by_path: None,
        installed_component_names: None,
        ccs_remove_hook: None,
        installed_component_types: vec![conary_core::components::ComponentType::Runtime],
        skipped_components: Vec::new(),
        language_provides: Vec::new(),
    };
    let file_capabilities = vec![file_capability("/usr/bin/server")];
    let db_path_string = db_path.to_string_lossy().into_owned();
    let root_string = root.to_string_lossy().into_owned();
    let ctx = TransactionContext {
        db_path: &db_path_string,
        root: &root_string,
        semantics: InstallSemantics::ccs(VersionScheme::Conary),
        selection_reason: None,
        old_trove_to_upgrade: Some(&old_trove),
        ccs_capabilities: None,
        ccs_file_capabilities: Some(&file_capabilities),
        defer_generation: false,
        repository_provenance: None,
        native_lifecycle_bundle: None,
        repository_enrollments: &[],
        relation_removals: &[],
        relation_deconfigurations: &[],
        retain_replaced_payload_until_lifecycle: false,
    };
    let tx_config = TransactionConfig::from_paths(root.clone(), db_path.clone());
    let mut engine = TransactionEngine::new(tx_config).unwrap();
    let tx = conn.unchecked_transaction().unwrap();
    let changeset_id = Changeset::new("Install server-2.0.0".to_string())
        .insert(&tx)
        .unwrap();

    let result = install_inner(
        &tx,
        &mut engine,
        changeset_id,
        &package,
        &extraction,
        &ctx,
        &InstallProgress::single("Installing"),
    )
    .unwrap();
    tx.commit().unwrap();

    assert!(
        InstalledFileCapability::find_by_trove(&conn, old_trove_id)
            .unwrap()
            .is_empty()
    );
    let rows = InstalledFileCapability::find_by_trove(&conn, result.trove_id).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].path, "/usr/bin/server");
    assert_eq!(rows[0].trove_id, result.trove_id);
}

#[test]
fn install_inner_rejects_installed_file_capability_on_symlink_payload() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let db_path = temp.path().join("conary.db");
    std::fs::create_dir_all(&root).unwrap();
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();

    let package = FakePackage {
        name: "server".to_string(),
        version: "1.0.0".to_string(),
        version_scheme: VersionScheme::Conary,
        provides: vec![crate::commands::test_helpers::exact_package_self_provider(
            "server",
            "1.0.0",
            VersionScheme::Conary,
        )],
        files: vec![PackageFile {
            path: "/usr/bin/server-link".to_string(),
            node: symlink_node("server"),
            content: None,
        }],
        extracted_files: vec![ExtractedFile {
            path: "/usr/bin/server-link".to_string(),
            node: symlink_node("server"),
            content: Vec::new(),
            content_authority: None,
        }],
        config_declarations: Vec::new(),
    };
    let extraction = ExtractionResult {
        extracted_files: package.payload_files(),
        classified: HashMap::from([(
            conary_core::components::ComponentType::Runtime,
            vec!["/usr/bin/server-link".to_string()],
        )]),
        component_names_by_path: None,
        installed_component_names: None,
        ccs_remove_hook: None,
        installed_component_types: vec![conary_core::components::ComponentType::Runtime],
        skipped_components: Vec::new(),
        language_provides: Vec::new(),
    };
    let file_capabilities = vec![file_capability("/usr/bin/server-link")];
    let db_path_string = db_path.to_string_lossy().into_owned();
    let root_string = root.to_string_lossy().into_owned();
    let ctx = TransactionContext {
        db_path: &db_path_string,
        root: &root_string,
        semantics: InstallSemantics::ccs(VersionScheme::Conary),
        selection_reason: None,
        old_trove_to_upgrade: None,
        ccs_capabilities: None,
        ccs_file_capabilities: Some(&file_capabilities),
        defer_generation: false,
        repository_provenance: None,
        native_lifecycle_bundle: None,
        repository_enrollments: &[],
        relation_removals: &[],
        relation_deconfigurations: &[],
        retain_replaced_payload_until_lifecycle: false,
    };
    let tx_config = TransactionConfig::from_paths(root.clone(), db_path.clone());
    let mut engine = TransactionEngine::new(tx_config).unwrap();
    let tx = conn.unchecked_transaction().unwrap();
    let changeset_id = Changeset::new("Install server-1.0.0".to_string())
        .insert(&tx)
        .unwrap();

    let err = match install_inner(
        &tx,
        &mut engine,
        changeset_id,
        &package,
        &extraction,
        &ctx,
        &InstallProgress::single("Installing"),
    ) {
        Ok(_) => panic!("selected symlink file capability target must fail closed"),
        Err(error) => error,
    };

    assert!(err.to_string().contains("is not a regular installed file"));
}

#[test]
fn install_inner_applies_repository_provenance_from_resolution() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let db_path = temp.path().join("conary.db");
    std::fs::create_dir_all(&root).unwrap();
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();
    let mut repo = Repository::new(
        "fedora-remi".to_string(),
        "https://example.invalid/fedora".to_string(),
    );
    let repo_id = repo.insert(&conn).unwrap();

    let mut package = FakePackage::with_file("tree", "/usr/bin/tree", b"tree\n");
    package.version_scheme = VersionScheme::Rpm;
    package.provides = vec![crate::commands::test_helpers::exact_package_self_provider(
        "tree",
        "1.0.0",
        VersionScheme::Rpm,
    )];
    let extraction = ExtractionResult {
        extracted_files: package.payload_files(),
        classified: HashMap::from([(
            conary_core::components::ComponentType::Runtime,
            vec!["/usr/bin/tree".to_string()],
        )]),
        component_names_by_path: None,
        installed_component_names: None,
        ccs_remove_hook: None,
        installed_component_types: vec![conary_core::components::ComponentType::Runtime],
        skipped_components: Vec::new(),
        language_provides: Vec::new(),
    };
    let db_path_string = db_path.to_string_lossy().into_owned();
    let root_string = root.to_string_lossy().into_owned();
    let ctx = TransactionContext {
        db_path: &db_path_string,
        root: &root_string,
        semantics: InstallSemantics::native_package(PackageFormatType::Rpm),
        selection_reason: None,
        old_trove_to_upgrade: None,
        ccs_capabilities: None,
        ccs_file_capabilities: None,
        defer_generation: false,
        repository_provenance: Some(RepositoryInstallProvenance {
            repository_id: repo_id,
            source_identity: Some("fedora-44".to_string()),
            source_profile: Some("fedora-44".to_string()),
            version_scheme: conary_core::repository::versioning::VersionScheme::Rpm,
            source_kind: conary_core::repository::RepositorySourceKind::Native,
        }),
        native_lifecycle_bundle: None,
        repository_enrollments: &[],
        relation_removals: &[],
        relation_deconfigurations: &[],
        retain_replaced_payload_until_lifecycle: false,
    };
    let tx_config = TransactionConfig::from_paths(root.clone(), db_path.clone());
    let mut engine = TransactionEngine::new(tx_config).unwrap();
    let tx = conn.unchecked_transaction().unwrap();
    let changeset_id = Changeset::new("Install tree-1.0.0".to_string())
        .insert(&tx)
        .unwrap();

    install_inner(
        &tx,
        &mut engine,
        changeset_id,
        &package,
        &extraction,
        &ctx,
        &InstallProgress::single("Installing"),
    )
    .unwrap();
    tx.commit().unwrap();

    let troves = Trove::find_by_name(&conn, "tree").unwrap();
    let [trove] = troves.as_slice() else {
        panic!("expected exactly one installed tree trove");
    };
    assert_eq!(trove.install_source, InstallSource::Repository);
    assert_eq!(trove.installed_from_repository_id, Some(repo_id));
    assert_eq!(trove.source_profile.as_deref(), Some("fedora-44"));
    assert_eq!(
        trove.version_scheme,
        conary_core::repository::versioning::VersionScheme::Rpm
    );
}
