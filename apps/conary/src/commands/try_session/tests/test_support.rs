// apps/conary/src/commands/try_session/tests/test_support.rs

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use conary_core::ccs::builder::write_signed_current_ccs_package;
use conary_core::ccs::manifest::{
    AlternativeHook, CcsManifest, DirectoryHook, GroupHook, SysctlHook, SystemdHook, TmpfilesHook,
    UserHook,
};
use conary_core::ccs::{BuildResult, ComponentData, FileEntry, SigningKeyPair, TrustPolicy};
#[cfg(feature = "test-hooks")]
use conary_core::db::models::TrySession;
#[cfg(feature = "test-hooks")]
use conary_core::generation::artifact::{
    ArtifactWriteInputs, BootAssetSources, CasObjectVerification, stage_boot_assets,
    write_generation_artifact,
};
#[cfg(feature = "test-hooks")]
use conary_core::generation::metadata::{GENERATION_FORMAT, GenerationMetadata};
#[cfg(feature = "test-hooks")]
use conary_core::generation::root_manifest::{
    GENERATION_ROOT_MANIFEST_VERSION, GenerationRootManifest, MutableStateManifest,
};
use conary_core::payload::{PayloadContentAuthority, PayloadIdentity, PayloadNode};
use conary_core::runtime_root::ConaryRuntimeRoot;

use super::{TryStartOutcome, TryStartRequest, begin_try_session};

pub(super) struct TryRuntimeFixture {
    pub(super) _temp: tempfile::TempDir,
    pub(super) root: PathBuf,
    pub(super) db_path: PathBuf,
    pub(super) db_path_string: String,
    pub(super) signing_key: SigningKeyPair,
    pub(super) trust_policy: TrustPolicy,
}

impl TryRuntimeFixture {
    pub(super) fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let db_path = root.join("conary.db");
        let db_path_string = db_path.to_string_lossy().into_owned();
        conary_core::db::init(&db_path).unwrap();
        let conn = conary_core::db::open(&db_path).unwrap();
        crate::commands::test_helpers::persist_test_host_capabilities(&conn);
        drop(conn);
        stage_test_boot_assets(&root);
        let signing_key = SigningKeyPair::generate().with_key_id("try-session-test");
        let trust_policy = TrustPolicy::strict(vec![signing_key.public_key_base64()]);
        Self {
            _temp: temp,
            root,
            db_path,
            db_path_string,
            signing_key,
            trust_policy,
        }
    }

    pub(super) fn runtime_root(&self) -> ConaryRuntimeRoot {
        ConaryRuntimeRoot::from_db_path(self.db_path.clone())
    }

    pub(super) fn write_package(&self, name: &str, manifest: CcsManifest) -> PathBuf {
        write_try_package(
            self.root.join(format!("{name}.ccs")),
            manifest,
            &self.signing_key,
        )
    }

    pub(super) fn open(&self) -> rusqlite::Connection {
        conary_core::db::open(&self.db_path).unwrap()
    }
}

fn stage_test_boot_assets(root: &Path) {
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

fn write_try_package(
    package_path: PathBuf,
    manifest: CcsManifest,
    signing_key: &SigningKeyPair,
) -> PathBuf {
    let tool_content = format!("#!/bin/sh\necho {}\n", manifest.package.name).into_bytes();
    let tool_hash = conary_core::hash::sha256(&tool_content);
    let init_content = b"#!/bin/sh\nexec true\n".to_vec();
    let init_hash = conary_core::hash::sha256(&init_content);
    let files = vec![
        FileEntry {
            path: format!("/usr/bin/{}", manifest.package.name),
            node: test_regular_node(0o755),
            content: Some(PayloadContentAuthority {
                sha256: tool_hash.clone(),
                size: tool_content.len() as u64,
            }),
            component: "runtime".to_string(),
            chunks: None,
        },
        FileEntry {
            path: "/usr/sbin/init".to_string(),
            node: test_regular_node(0o755),
            content: Some(PayloadContentAuthority {
                sha256: init_hash.clone(),
                size: init_content.len() as u64,
            }),
            component: "runtime".to_string(),
            chunks: None,
        },
        FileEntry {
            path: "/sbin".to_string(),
            node: test_symlink_node("usr/sbin"),
            content: None,
            component: "runtime".to_string(),
            chunks: None,
        },
    ];
    let total_size = (tool_content.len() + init_content.len()) as u64;
    let result = BuildResult {
        manifest,
        components: HashMap::from([(
            "runtime".to_string(),
            ComponentData {
                name: "runtime".to_string(),
                files: files.clone(),
                hash: "runtime".to_string(),
                size: total_size,
            },
        )]),
        files: files.clone(),
        payloads: conary_core::ccs::builder::payloads_from_bounded_memory_for_tests(
            &files,
            HashMap::from([(tool_hash, tool_content), (init_hash, init_content)]),
        )
        .unwrap(),
        total_size,
        chunked: false,
        chunk_stats: None,
    };
    write_signed_current_ccs_package(&result, &package_path, signing_key, false).unwrap();
    package_path
}

fn test_regular_node(mode: u32) -> PayloadNode {
    let mut node = PayloadNode::regular(mode);
    node.user = PayloadIdentity::Numeric {
        id: u64::from(unsafe { libc::geteuid() }),
    };
    node.group = PayloadIdentity::Numeric {
        id: u64::from(unsafe { libc::getegid() }),
    };
    node
}

fn test_symlink_node(target: &str) -> PayloadNode {
    let mut node = test_regular_node(0o777);
    node.kind = conary_core::payload::PayloadNodeKind::Symlink {
        target: target.to_string(),
    };
    node.mode = libc::S_IFLNK | 0o777;
    node
}

pub(super) fn begin_namespace_try(
    fixture: &TryRuntimeFixture,
    package_path: &Path,
) -> anyhow::Result<TryStartOutcome> {
    begin_try_session(TryStartRequest {
        db_path: &fixture.db_path_string,
        package_path,
        trust_policy: &fixture.trust_policy,
        activate: false,
        command: None,
        watch_marker: None,
    })
}

#[cfg(feature = "test-hooks")]
pub(super) fn begin_activated_try(
    fixture: &TryRuntimeFixture,
    package_path: &Path,
) -> anyhow::Result<TryStartOutcome> {
    begin_try_session(TryStartRequest {
        db_path: &fixture.db_path_string,
        package_path,
        trust_policy: &fixture.trust_policy,
        activate: true,
        command: None,
        watch_marker: None,
    })
}

#[cfg(feature = "test-hooks")]
pub(super) fn stored_session(fixture: &TryRuntimeFixture, id: &str) -> TrySession {
    TrySession::find_by_id(&fixture.open(), id)
        .unwrap()
        .expect("stored try session")
}

#[cfg(feature = "test-hooks")]
pub(super) fn create_current_generation_link(root: &Path, generation: i64) {
    let generation_dir = root.join(format!("generations/{generation}"));
    let objects_dir = root.join("objects");
    std::fs::create_dir_all(&generation_dir).unwrap();
    std::fs::create_dir_all(&objects_dir).unwrap();

    let erofs_path = generation_dir.join("root.erofs");
    std::fs::write(&erofs_path, b"try-test-root-erofs").unwrap();
    let mut root_node = test_regular_node(0o755);
    root_node.kind = conary_core::payload::PayloadNodeKind::Directory;
    root_node.mode = libc::S_IFDIR | 0o755;
    GenerationRootManifest {
        version: GENERATION_ROOT_MANIFEST_VERSION,
        root: conary_core::payload::ResolvedPayloadNode::from_numeric_source(root_node).unwrap(),
        entries: Vec::new(),
    }
    .write_to(&generation_dir)
    .unwrap();
    MutableStateManifest::empty()
        .write_to(&generation_dir)
        .unwrap();

    let kernel_version = "test-kernel".to_string();
    let boot_root = root.join("boot");
    let boot_assets = stage_boot_assets(BootAssetSources {
        generation_dir: &generation_dir,
        generation,
        architecture: "x86_64",
        kernel_version: &kernel_version,
        kernel: &boot_root.join(format!("vmlinuz-{kernel_version}")),
        initramfs: &boot_root.join(format!("initramfs-{kernel_version}.img")),
        efi_bootloader: &boot_root.join("EFI/BOOT/BOOTX64.EFI"),
    })
    .unwrap();
    let artifact_manifest_sha256 = write_generation_artifact(ArtifactWriteInputs {
        generation_dir: &generation_dir,
        generation,
        architecture: "x86_64",
        erofs_path: &erofs_path,
        cas_base_rel: "../../objects",
        cas_verification: CasObjectVerification::Deep,
        boot_assets,
        carrier_capabilities: Default::default(),
    })
    .unwrap();
    GenerationMetadata {
        generation,
        format: GENERATION_FORMAT.to_string(),
        erofs_size: Some(std::fs::metadata(&erofs_path).unwrap().len() as i64),
        cas_objects_referenced: Some(0),
        fsverity_enabled: false,
        erofs_verity_digest: None,
        artifact_manifest_sha256: Some(artifact_manifest_sha256),
        security_capability_xattr_count: None,
        created_at: "2026-07-25T00:00:00Z".to_string(),
        package_count: 0,
        kernel_version: Some(kernel_version),
        summary: "try test generation".to_string(),
    }
    .write_to(&generation_dir)
    .unwrap();
    conary_core::generation::mount::update_current_symlink(root, generation).unwrap();
}

#[cfg(feature = "test-hooks")]
pub(super) fn has_cas_object(root: &Path) -> bool {
    let objects_dir = root.join("objects");
    if !objects_dir.exists() {
        return false;
    }
    walkdir::WalkDir::new(objects_dir)
        .into_iter()
        .filter_map(Result::ok)
        .any(|entry| {
            entry.file_type().is_file()
                && entry.file_name() != "conary.lock"
                && entry.metadata().map(|m| m.len() > 0).unwrap_or(false)
        })
}

#[cfg(feature = "test-hooks")]
pub(super) fn write_try_mountinfo(path: &Path, mounted_paths: &[&Path]) -> anyhow::Result<()> {
    let mut contents = String::new();
    for (index, mounted_path) in mounted_paths.iter().enumerate() {
        contents.push_str(&format!(
            "{} 1 0:{} / {} rw,relatime - overlay overlay rw\n",
            100 + index,
            100 + index,
            escape_mountinfo_path(mounted_path)
        ));
    }
    std::fs::write(path, contents)?;
    Ok(())
}

#[cfg(feature = "test-hooks")]
fn escape_mountinfo_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "\\134")
        .replace(' ', "\\040")
        .replace('\t', "\\011")
        .replace('\n', "\\012")
}

pub(super) struct EnvVarGuard {
    key: &'static str,
    previous: Option<OsString>,
}

impl EnvVarGuard {
    pub(super) fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        assert_ne!(
            key,
            crate::test_hooks::names::SKIP_GENERATION_MOUNT,
            "use composefs_ops::test_mount_skip_guard for the shared mount-skip environment"
        );
        let previous = std::env::var_os(key);
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        unsafe {
            if let Some(previous) = &self.previous {
                std::env::set_var(self.key, previous);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }
}

pub(super) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(super) fn manifest_with_declarative_hook() -> CcsManifest {
    let mut manifest = CcsManifest::new_minimal("declarative", "1.0.0");
    manifest.hooks.directories.push(DirectoryHook {
        path: "/var/lib/declarative".to_string(),
        mode: "0755".to_string(),
        owner: "root".to_string(),
        group: "root".to_string(),
        cleanup: None,
        reversible: None,
    });
    manifest
}

pub(super) fn manifest_with_user_group_hooks() -> CcsManifest {
    let mut manifest = CcsManifest::new_minimal("user-group-hooks", "1.0.0");
    manifest.hooks.groups.push(GroupHook {
        name: "trygroup".to_string(),
        system: true,
        reversible: None,
    });
    manifest.hooks.users.push(UserHook {
        name: "tryuser".to_string(),
        system: true,
        home: Some("/nonexistent".to_string()),
        shell: Some("/usr/sbin/nologin".to_string()),
        group: Some("trygroup".to_string()),
        reversible: None,
    });
    manifest
}

pub(super) fn manifest_with_systemd_hook() -> CcsManifest {
    let mut manifest = CcsManifest::new_minimal("systemd-hook", "1.0.0");
    manifest.hooks.systemd.push(SystemdHook {
        unit: "try-systemd.service".to_string(),
        enable: true,
        reversible: Some(true),
    });
    manifest
}

pub(super) fn manifest_with_tmpfiles_hook() -> CcsManifest {
    let mut manifest = CcsManifest::new_minimal("tmpfiles-hook", "1.0.0");
    manifest.hooks.tmpfiles.push(TmpfilesHook {
        entry_type: "d".to_string(),
        path: "/var/lib/try-tmpfiles".to_string(),
        mode: "0755".to_string(),
        user: "root".to_string(),
        group: "root".to_string(),
        age: "-".to_string(),
        argument: "-".to_string(),
        reversible: Some(true),
    });
    manifest
}

pub(super) fn manifest_with_sysctl_hook() -> CcsManifest {
    let mut manifest = CcsManifest::new_minimal("sysctl-hook", "1.0.0");
    manifest.hooks.sysctl.push(SysctlHook {
        key: "net.ipv4.ip_forward".to_string(),
        value: "0".to_string(),
        reversible: Some(true),
    });
    manifest
}

pub(super) fn manifest_with_alternative_hook() -> CcsManifest {
    let mut manifest = CcsManifest::new_minimal("alternative-hook", "1.0.0");
    manifest.hooks.alternatives.push(AlternativeHook {
        link: "/usr/bin/try-editor".to_string(),
        name: "try-editor".to_string(),
        path: "/usr/bin/try-editor".to_string(),
        priority: 50,
        reversible: Some(true),
    });
    manifest
}
