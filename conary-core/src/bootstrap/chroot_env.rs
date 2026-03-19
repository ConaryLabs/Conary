// conary-core/src/bootstrap/chroot_env.rs

//! Chroot environment setup and teardown for LFS bootstrap builds.
//!
//! Manages the virtual kernel filesystem mounts required by LFS Chapters 7-8.
//! Uses a mount tracking vector for safe partial teardown on error or panic.

use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{info, warn};

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
            "dev", "proc", "sys", "run",
            "etc", "home", "mnt", "opt", "srv",
            "usr/bin", "usr/lib", "usr/sbin",
            "var/log", "var/mail", "var/spool",
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
        self.mount_bind("/dev", &lfs.join("dev"))?;
        self.mount_fs("devpts", &lfs.join("dev/pts"), "devpts", "gid=5,mode=0620")?;
        self.mount_fs("proc", &lfs.join("proc"), "proc", "")?;
        self.mount_fs("sysfs", &lfs.join("sys"), "sysfs", "")?;
        self.mount_fs("tmpfs", &lfs.join("run"), "tmpfs", "")?;

        info!("Chroot environment ready at {}", lfs.display());
        Ok(())
    }

    fn mount_bind(&mut self, src: &str, dest: &Path) -> anyhow::Result<()> {
        std::fs::create_dir_all(dest)?;
        let status = Command::new("mount")
            .args(["--bind", src, &dest.to_string_lossy()])
            .status()?;
        if status.success() {
            self.mounted.push(dest.to_path_buf());
            Ok(())
        } else {
            anyhow::bail!("mount --bind {} {} failed", src, dest.display());
        }
    }

    fn mount_fs(
        &mut self,
        dev: &str,
        dest: &Path,
        fstype: &str,
        opts: &str,
    ) -> anyhow::Result<()> {
        std::fs::create_dir_all(dest)?;
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

    /// Unmount all tracked mounts in reverse order. Best-effort: errors are logged.
    pub fn teardown(&mut self) {
        while let Some(mount_point) = self.mounted.pop() {
            let result = Command::new("umount")
                .args(["--lazy", &mount_point.to_string_lossy()])
                .status();
            match result {
                Ok(status) if status.success() => {
                    info!("Unmounted {}", mount_point.display());
                }
                Ok(status) => {
                    warn!(
                        "umount {} exited with {}",
                        mount_point.display(),
                        status
                    );
                }
                Err(e) => {
                    warn!(
                        "Failed to run umount for {}: {}",
                        mount_point.display(),
                        e
                    );
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
}
