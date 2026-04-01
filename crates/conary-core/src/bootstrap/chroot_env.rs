// conary-core/src/bootstrap/chroot_env.rs

//! Chroot environment setup and teardown for LFS bootstrap builds.
//!
//! Manages the virtual kernel filesystem mounts required by LFS Chapters 7-8.
//! Uses a mount tracking vector for safe partial teardown on error or panic.

use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;
use tracing::{info, warn};

const DEVTMPFS_OPTS: &str = "mode=0755,nosuid";
const DEVPTS_OPTS: &str = "gid=5,mode=0620";
const CHROOT_KILL_GRACE_PERIOD: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy)]
struct DeviceNodeSpec {
    name: &'static str,
    major: u32,
    minor: u32,
    mode: u32,
}

const MINIMAL_DEV_NODES: [DeviceNodeSpec; 6] = [
    DeviceNodeSpec {
        name: "null",
        major: 1,
        minor: 3,
        mode: 0o666,
    },
    DeviceNodeSpec {
        name: "zero",
        major: 1,
        minor: 5,
        mode: 0o666,
    },
    DeviceNodeSpec {
        name: "random",
        major: 1,
        minor: 8,
        mode: 0o666,
    },
    DeviceNodeSpec {
        name: "urandom",
        major: 1,
        minor: 9,
        mode: 0o666,
    },
    DeviceNodeSpec {
        name: "tty",
        major: 5,
        minor: 0,
        mode: 0o666,
    },
    DeviceNodeSpec {
        name: "full",
        major: 1,
        minor: 7,
        mode: 0o666,
    },
];

fn minimal_dev_nodes() -> &'static [DeviceNodeSpec] {
    &MINIMAL_DEV_NODES
}

fn path_is_within_chroot(candidate: &Path, chroot_root: &Path) -> bool {
    candidate == chroot_root || candidate.starts_with(chroot_root)
}

fn umount_attempts(mount_point: &Path) -> Vec<Vec<String>> {
    let mount = mount_point.to_string_lossy().into_owned();
    vec![vec![mount.clone()], vec!["--lazy".to_string(), mount]]
}

/// Manages chroot mount lifecycle for LFS bootstrap builds.
///
/// Tracks which mounts succeeded so teardown only unmounts what was actually
/// mounted. The `Drop` impl ensures cleanup even on panic.
pub struct ChrootEnv {
    lfs_root: PathBuf,
    /// Mounts that succeeded, in order. Teardown reverses this.
    mounted: Vec<PathBuf>,
}

impl ChrootEnv {
    pub fn new(lfs_root: &Path) -> Self {
        Self {
            lfs_root: lfs_root.to_path_buf(),
            mounted: Vec::new(),
        }
    }

    /// Create directory structure and mount virtual filesystems.
    ///
    /// Follows LFS 13 Chapter 7.3-7.4. If a mount fails, previously
    /// mounted filesystems are tracked and will be cleaned up by `Drop`.
    pub fn setup(&mut self) -> anyhow::Result<()> {
        // Clone to avoid holding an immutable borrow while calling &mut self methods.
        let lfs = self.lfs_root.clone();

        // Create directory hierarchy
        for dir in &[
            "dev",
            "proc",
            "sys",
            "run",
            "etc",
            "home",
            "mnt",
            "opt",
            "srv",
            "usr/bin",
            "usr/lib",
            "usr/sbin",
            "var/log",
            "var/mail",
            "var/spool",
        ] {
            std::fs::create_dir_all(lfs.join(dir))?;
        }

        // Create compatibility symlinks (LFS uses merged /usr)
        for (link, target) in &[
            ("bin", "usr/bin"),
            ("lib", "usr/lib"),
            ("sbin", "usr/sbin"),
            ("lib64", "usr/lib"),
        ] {
            let link_path = lfs.join(link);
            if !link_path.exists() {
                std::os::unix::fs::symlink(target, &link_path)?;
            }
        }

        // Mount virtual kernel filesystems
        self.mount_fs("devtmpfs", &lfs.join("dev"), "devtmpfs", DEVTMPFS_OPTS)?;
        self.create_minimal_dev_nodes(&lfs.join("dev"))?;
        self.mount_fs("devpts", &lfs.join("dev/pts"), "devpts", DEVPTS_OPTS)?;
        self.mount_fs("proc", &lfs.join("proc"), "proc", "")?;
        self.mount_fs("sysfs", &lfs.join("sys"), "sysfs", "")?;
        self.mount_fs("tmpfs", &lfs.join("run"), "tmpfs", "")?;

        info!("Chroot environment ready at {}", lfs.display());
        Ok(())
    }

    fn mount_fs(&mut self, dev: &str, dest: &Path, fstype: &str, opts: &str) -> anyhow::Result<()> {
        fs::create_dir_all(dest)?;
        let mut cmd = Command::new("mount");
        cmd.arg("-t").arg(fstype);
        if !opts.is_empty() {
            cmd.arg("-o").arg(opts);
        }
        cmd.arg(dev).arg(dest);

        let status = cmd.status()?;
        if status.success() {
            self.mounted.push(dest.to_path_buf());
            Ok(())
        } else {
            anyhow::bail!("mount -t {} {} {} failed", fstype, dev, dest.display());
        }
    }

    fn create_minimal_dev_nodes(&self, dev_root: &Path) -> anyhow::Result<()> {
        for node in minimal_dev_nodes() {
            let node_path = dev_root.join(node.name);
            if node_path.exists() {
                continue;
            }

            let mode = format!("{:o}", node.mode);
            let major = node.major.to_string();
            let minor = node.minor.to_string();
            let status = Command::new("mknod")
                .args([
                    "-m",
                    &mode,
                    node_path.to_string_lossy().as_ref(),
                    "c",
                    &major,
                    &minor,
                ])
                .status()?;

            if !status.success() {
                anyhow::bail!("mknod {} failed", node_path.display());
            }
        }

        Ok(())
    }

    fn processes_in_chroot(&self, chroot_root: &Path) -> Vec<Pid> {
        let mut pids = Vec::new();
        let Ok(entries) = fs::read_dir("/proc") else {
            return pids;
        };

        for entry in entries.flatten() {
            let pid_text = entry.file_name();
            let Some(pid_text) = pid_text.to_str() else {
                continue;
            };
            let Ok(raw_pid) = pid_text.parse::<i32>() else {
                continue;
            };
            if raw_pid == std::process::id() as i32 {
                continue;
            }

            let Ok(root_link) = fs::read_link(entry.path().join("root")) else {
                continue;
            };
            if path_is_within_chroot(&root_link, chroot_root) {
                pids.push(Pid::from_raw(raw_pid));
            }
        }

        pids
    }

    fn kill_processes_in_chroot(&self) {
        let chroot_root =
            fs::canonicalize(&self.lfs_root).unwrap_or_else(|_| self.lfs_root.clone());
        let initial_pids = self.processes_in_chroot(&chroot_root);
        if initial_pids.is_empty() {
            return;
        }

        for pid in &initial_pids {
            if let Err(error) = kill(*pid, Signal::SIGTERM) {
                warn!("Failed to terminate PID {} in chroot: {}", pid, error);
            }
        }

        thread::sleep(CHROOT_KILL_GRACE_PERIOD);

        for pid in self.processes_in_chroot(&chroot_root) {
            if let Err(error) = kill(pid, Signal::SIGKILL) {
                warn!("Failed to SIGKILL PID {} in chroot: {}", pid, error);
            }
        }
    }

    fn unmount_one(&self, mount_point: &Path) -> anyhow::Result<()> {
        let attempts = umount_attempts(mount_point);
        for (index, args) in attempts.iter().enumerate() {
            let status = Command::new("umount").args(args).status()?;
            if status.success() {
                return Ok(());
            }

            if index == 0 {
                warn!(
                    "umount {} exited with {}; retrying lazy detach",
                    mount_point.display(),
                    status
                );
            } else {
                anyhow::bail!("umount {} exited with {}", mount_point.display(), status);
            }
        }

        Ok(())
    }

    /// Unmount all tracked mounts in reverse order. Best-effort: errors are logged.
    pub fn teardown(&mut self) {
        self.kill_processes_in_chroot();
        while let Some(mount_point) = self.mounted.pop() {
            match self.unmount_one(&mount_point) {
                Ok(()) => {
                    info!("Unmounted {}", mount_point.display());
                }
                Err(e) => {
                    warn!("Failed to unmount {}: {}", mount_point.display(), e);
                }
            }
        }
    }
}

impl Drop for ChrootEnv {
    fn drop(&mut self) {
        if !self.mounted.is_empty() {
            warn!(
                "ChrootEnv dropped with {} active mounts, cleaning up",
                self.mounted.len()
            );
            self.teardown();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn test_chroot_env_creates_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let lfs = tmp.path().join("lfs");
        std::fs::create_dir_all(&lfs).unwrap();

        // setup() will fail on mounts (not root in tests) but dirs should be created
        let mut env = ChrootEnv::new(&lfs);
        let _ = env.setup(); // Ignore mount errors in test

        assert!(lfs.join("dev").exists());
        assert!(lfs.join("proc").exists());
        assert!(lfs.join("sys").exists());
        assert!(lfs.join("run").exists());
        assert!(lfs.join("usr/bin").exists());
        assert!(lfs.join("usr/lib").exists());
        assert!(lfs.join("usr/sbin").exists());
    }

    #[test]
    fn test_chroot_env_teardown_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let lfs = tmp.path().join("lfs");
        std::fs::create_dir_all(&lfs).unwrap();

        let mut env = ChrootEnv::new(&lfs);
        // No mounts succeeded, teardown should not panic
        env.teardown();
        env.teardown(); // Second call should be safe
    }

    #[test]
    fn test_minimal_dev_nodes_match_expected_device_set() {
        let nodes = minimal_dev_nodes();
        let names: BTreeSet<_> = nodes.iter().map(|node| node.name).collect();
        assert_eq!(
            names,
            BTreeSet::from(["full", "null", "random", "tty", "urandom", "zero"])
        );
        assert_eq!(nodes.len(), 6);
    }

    #[test]
    fn test_path_is_within_chroot_respects_component_boundaries() {
        let root = Path::new("/tmp/lfs");
        assert!(path_is_within_chroot(Path::new("/tmp/lfs"), root));
        assert!(path_is_within_chroot(Path::new("/tmp/lfs/usr/bin"), root));
        assert!(!path_is_within_chroot(Path::new("/tmp/lfs-other"), root));
        assert!(!path_is_within_chroot(Path::new("/tmp/other"), root));
    }

    #[test]
    fn test_umount_attempts_try_nonlazy_before_lazy() {
        let attempts = umount_attempts(Path::new("/tmp/lfs/dev"));
        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[0], vec!["/tmp/lfs/dev".to_string()]);
        assert_eq!(
            attempts[1],
            vec!["--lazy".to_string(), "/tmp/lfs/dev".to_string()]
        );
    }
}
