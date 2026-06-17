// apps/conary/src/commands/try_session/mod.rs
//! Try-session policy helpers.

use anyhow::Result;
use conary_core::db::models::TrySession;
use std::path::{Path, PathBuf};

mod executor;
mod install;
mod namespace;
mod session;
mod util;
mod validation;
mod watch;
mod watch_source;

pub(crate) use session::{
    activated_try_session_is_live, begin_try_session, current_boot_id,
    namespace_try_session_is_decision_pending, rollback_active_try_session,
};
#[derive(Debug, Clone, Copy)]
pub(crate) struct TryStartRequest<'a> {
    pub db_path: &'a str,
    pub package_path: &'a Path,
    pub activate: bool,
    pub allow_irreversible: bool,
    pub command: Option<&'a [&'a str]>,
    pub watch_marker: Option<TryWatchMarkerRequest<'a>>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct TryWatchMarkerRequest<'a> {
    pub(crate) operation_id: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TryStartOutcome {
    pub session_id: String,
    pub work_dir: PathBuf,
    pub install_root: PathBuf,
    pub copied_package_path: PathBuf,
    pub copied_db_path: PathBuf,
    pub namespace_root: PathBuf,
    pub try_generation_id: i64,
}

pub(crate) async fn cmd_try_package(
    db_path: &str,
    package_path: &Path,
    activate: bool,
    allow_irreversible: bool,
    run: &[String],
) -> Result<()> {
    let command = run.iter().map(String::as_str).collect::<Vec<_>>();
    let outcome = begin_try_session(TryStartRequest {
        db_path,
        package_path,
        activate,
        allow_irreversible,
        command: if command.is_empty() {
            None
        } else {
            Some(command.as_slice())
        },
        watch_marker: None,
    })?;

    println!("Try session {} is active", outcome.session_id);
    println!("Package copy: {}", outcome.copied_package_path.display());
    println!("Namespace root: {}", outcome.namespace_root.display());
    println!("Generation: {}", outcome.try_generation_id);
    if activate {
        println!(
            "Run `conary try keep` to keep it or `conary try rollback` to restore the previous generation."
        );
    } else {
        println!("Run `conary try keep` to promote it or `conary try rollback` to discard it.");
    }
    Ok(())
}

pub(crate) async fn cmd_try_status(db_path: &str) -> Result<()> {
    let live_conn = conary_core::db::open(db_path)?;
    match TrySession::find_active_or_orphaned(&live_conn)? {
        Some(session) => {
            println!("Try session: {}", session.id);
            println!("Status: {}", session.status.as_str());
            println!("Mode: {}", session.mode.as_str());
            if let Some(name) = &session.package_name {
                println!("Package: {name}");
            }
            if let Some(version) = &session.package_version {
                println!("Version: {version}");
            }
            if let Some(generation) = session.try_generation_id {
                println!("Generation: {generation}");
            }
            if let Some(pid) = session.launcher_pid {
                println!("Launcher PID: {pid}");
            }
        }
        None => {
            println!("No active try session");
        }
    }
    Ok(())
}

pub(crate) async fn cmd_try_rollback(db_path: &str) -> Result<()> {
    rollback_active_try_session(db_path)?;
    println!("Try session rolled back");
    Ok(())
}

pub(crate) async fn cmd_try_keep(db_path: &str) -> Result<()> {
    session::keep_active_try_session(db_path)?;
    println!("Try session kept");
    Ok(())
}

pub(crate) async fn cmd_try_watch(
    db_path: &str,
    target: &str,
    recipe: Option<&str>,
    json: bool,
) -> Result<()> {
    watch::cmd_try_watch(watch::TryWatchOptions {
        db_path,
        target,
        recipe,
        json,
    })
    .await
}

#[cfg(test)]
pub(super) mod test_support {
    use std::collections::HashMap;
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    use conary_core::ccs::builder::write_ccs_package;
    use conary_core::ccs::manifest::{
        AlternativeHook, CcsManifest, DirectoryHook, GroupHook, SysctlHook, SystemdHook,
        TmpfilesHook, UserHook,
    };
    use conary_core::ccs::{BuildResult, ComponentData, FileEntry, FileType};
    use conary_core::db::models::TrySession;
    use conary_core::runtime_root::ConaryRuntimeRoot;

    use super::{TryStartOutcome, TryStartRequest, begin_try_session};

    pub(super) struct TryRuntimeFixture {
        pub(super) _temp: tempfile::TempDir,
        pub(super) root: PathBuf,
        pub(super) db_path: PathBuf,
        pub(super) db_path_string: String,
    }

    impl TryRuntimeFixture {
        pub(super) fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().to_path_buf();
            let db_path = root.join("conary.db");
            let db_path_string = db_path.to_string_lossy().into_owned();
            conary_core::db::init(&db_path).unwrap();
            stage_test_boot_assets(&root);
            Self {
                _temp: temp,
                root,
                db_path,
                db_path_string,
            }
        }

        pub(super) fn runtime_root(&self) -> ConaryRuntimeRoot {
            ConaryRuntimeRoot::from_db_path(self.db_path.clone())
        }

        pub(super) fn write_package(&self, name: &str, manifest: CcsManifest) -> PathBuf {
            write_try_package(self.root.join(format!("{name}.ccs")), manifest)
        }

        pub(super) fn open(&self) -> rusqlite::Connection {
            conary_core::db::open(&self.db_path).unwrap()
        }
    }

    fn stage_test_boot_assets(root: &Path) {
        let kernel_version =
            conary_core::generation::builder::detect_kernel_version_from_troves(&[])
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

    fn write_try_package(package_path: PathBuf, manifest: CcsManifest) -> PathBuf {
        let tool_content = format!("#!/bin/sh\necho {}\n", manifest.package.name).into_bytes();
        let tool_hash = conary_core::hash::sha256(&tool_content);
        let init_content = b"#!/bin/sh\nexec true\n".to_vec();
        let init_hash = conary_core::hash::sha256(&init_content);
        let files = vec![
            FileEntry {
                path: format!("/usr/bin/{}", manifest.package.name),
                hash: tool_hash.clone(),
                size: tool_content.len() as u64,
                mode: 0o100755,
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
            files,
            blobs: HashMap::from([(tool_hash, tool_content), (init_hash, init_content)]),
            total_size,
            chunked: false,
            chunk_stats: None,
        };
        write_ccs_package(&result, &package_path).unwrap();
        package_path
    }

    pub(super) fn begin_namespace_try(
        fixture: &TryRuntimeFixture,
        package_path: &Path,
    ) -> anyhow::Result<TryStartOutcome> {
        begin_try_session(TryStartRequest {
            db_path: &fixture.db_path_string,
            package_path,
            activate: false,
            allow_irreversible: false,
            command: None,
            watch_marker: None,
        })
    }

    pub(super) fn begin_activated_try(
        fixture: &TryRuntimeFixture,
        package_path: &Path,
    ) -> anyhow::Result<TryStartOutcome> {
        begin_try_session(TryStartRequest {
            db_path: &fixture.db_path_string,
            package_path,
            activate: true,
            allow_irreversible: false,
            command: None,
            watch_marker: None,
        })
    }

    pub(super) fn stored_session(fixture: &TryRuntimeFixture, id: &str) -> TrySession {
        TrySession::find_by_id(&fixture.open(), id)
            .unwrap()
            .expect("stored try session")
    }

    pub(super) fn create_current_generation_link(root: &Path, generation: i64) {
        std::fs::create_dir_all(root.join(format!("generations/{generation}"))).unwrap();
        conary_core::generation::mount::update_current_symlink(root, generation).unwrap();
    }

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
            owner: "root".to_string(),
            group: "root".to_string(),
            reversible: Some(true),
        });
        manifest
    }

    pub(super) fn manifest_with_sysctl_hook() -> CcsManifest {
        let mut manifest = CcsManifest::new_minimal("sysctl-hook", "1.0.0");
        manifest.hooks.sysctl.push(SysctlHook {
            key: "net.ipv4.ip_forward".to_string(),
            value: "0".to_string(),
            only_if_lower: false,
            reversible: Some(true),
        });
        manifest
    }

    pub(super) fn manifest_with_alternative_hook() -> CcsManifest {
        let mut manifest = CcsManifest::new_minimal("alternative-hook", "1.0.0");
        manifest.hooks.alternatives.push(AlternativeHook {
            name: "try-editor".to_string(),
            path: "/usr/bin/try-editor".to_string(),
            priority: 50,
            reversible: Some(true),
        });
        manifest
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use conary_core::db::models::TrySessionMode;

    use super::install::{build_try_install_plan, build_try_transaction_config};
    use super::test_support::*;

    #[test]
    fn try_transaction_config_override_keeps_live_runtime_paths_for_copied_db() {
        let fixture = TryRuntimeFixture::new();
        let work_dir = fixture.root.join("try/session-a");
        let copied_db = work_dir.join("conary.db");

        let config = build_try_transaction_config(&fixture.runtime_root(), copied_db.clone());

        assert_eq!(config.db_path, copied_db);
        assert_eq!(config.root, fixture.root);
        assert_eq!(config.objects_dir, fixture.root.join("objects"));
        assert_eq!(config.generations_dir, fixture.root.join("generations"));
        assert_eq!(config.etc_state_dir, fixture.root.join("etc-state"));
        assert_eq!(config.mount_point, fixture.root.join("mnt"));
    }

    #[test]
    fn namespace_try_install_plan_uses_scratch_root_no_scripts_and_config_override() {
        let fixture = TryRuntimeFixture::new();
        let work_dir = fixture.root.join("try/session-a");
        let copied_db = work_dir.join("conary.db");

        let plan = build_try_install_plan(
            &fixture.runtime_root(),
            &work_dir,
            copied_db.clone(),
            TrySessionMode::Namespace,
        );

        assert_eq!(plan.install_root, work_dir.join("root"));
        assert_ne!(plan.install_root, PathBuf::from("/"));
        assert!(
            plan.no_scripts,
            "namespace try installs must suppress install-time hooks"
        );
        assert_eq!(plan.transaction_config.db_path, copied_db);
        assert_eq!(
            plan.transaction_config.objects_dir,
            fixture.root.join("objects")
        );
        assert_eq!(
            plan.transaction_config.generations_dir,
            fixture.root.join("generations")
        );
    }
}
