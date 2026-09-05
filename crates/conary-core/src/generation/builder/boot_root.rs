// crates/conary-core/src/generation/builder/boot_root.rs

use std::path::{Path, PathBuf};

/// Where a generation's boot assets come from, decided by the caller that
/// knows whether the build targets the live system.
///
/// Policy sites match on the variant; only the file readers see the path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootRoot {
    /// The live system's `/boot`: the initramfs is generated from the
    /// generation sysroot, and boot assets may be reused across generations.
    Host,
    /// An explicit, writable boot directory (test fixtures, staged targets).
    /// Its runtime files are read as they are and never reused across
    /// generations. When it lacks the release initramfs, the builder generates
    /// one into this directory with dracut, running depmod first if
    /// `modules.dep` is also missing; an existing initramfs is used unchanged.
    Staged(PathBuf),
}

impl BootRoot {
    /// The directory the variant names on disk.
    pub fn path(&self) -> &Path {
        match self {
            Self::Host => Path::new("/boot"),
            Self::Staged(path) => path,
        }
    }
}
