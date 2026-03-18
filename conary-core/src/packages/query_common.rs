// conary-core/src/packages/query_common.rs
//! Shared types and helpers for native package manager queries.

/// Dependency with version constraint
#[derive(Debug, Clone)]
pub struct DependencyInfo {
    pub name: String,
    pub constraint: Option<String>, // e.g., ">= 1.0", "< 2.0"
}

/// Information about a single installed file from a native package manager.
#[derive(Debug, Clone)]
pub struct InstalledFileInfo {
    pub path: String,
    pub size: i64,
    pub mode: i32,
    pub digest: Option<String>,
    pub user: Option<String>,
    pub group: Option<String>,
    pub link_target: Option<String>,
    pub mtime: Option<i64>, // RPM provides this, others don't
}

impl InstalledFileInfo {
    /// Check if this file is a symlink (mode & S_IFMT == S_IFLNK)
    pub fn is_symlink(&self) -> bool {
        // S_IFLNK = 0o120000 = 0xA000
        (self.mode & 0o170000) == 0o120000
    }

    /// Check if this file is a directory (mode & S_IFMT == S_IFDIR)
    pub fn is_directory(&self) -> bool {
        // S_IFDIR = 0o040000
        (self.mode & 0o170000) == 0o040000
    }

    /// Check if this file is a regular file (mode & S_IFMT == S_IFREG)
    pub fn is_regular_file(&self) -> bool {
        // S_IFREG = 0o100000
        (self.mode & 0o170000) == 0o100000
    }
}

/// Run an external command and return stdout as a string.
pub fn run_query_command(cmd: &str, args: &[&str]) -> crate::Result<String> {
    let output = std::process::Command::new(cmd)
        .args(args)
        .output()
        .map_err(|e| crate::error::Error::IoError(format!("Failed to run {cmd}: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(crate::error::Error::IoError(format!("{cmd} failed: {stderr}")));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
