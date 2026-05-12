// conary-core/src/runtime_root.rs

use std::path::{Path, PathBuf};

const DEFAULT_RUNTIME_ROOT: &str = "/conary";
const DEFAULT_DB_PATH: &str = "/var/lib/conary/conary.db";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConaryRuntimeRoot {
    root: PathBuf,
    db_path: PathBuf,
}

impl Default for ConaryRuntimeRoot {
    fn default() -> Self {
        Self {
            root: PathBuf::from(DEFAULT_RUNTIME_ROOT),
            db_path: PathBuf::from(DEFAULT_DB_PATH),
        }
    }
}

impl ConaryRuntimeRoot {
    pub fn new(root: impl Into<PathBuf>, db_path: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            db_path: db_path.into(),
        }
    }

    pub fn for_test_root(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            db_path: root.join("conary.db"),
            root,
        }
    }

    pub fn from_db_path(db_path: impl Into<PathBuf>) -> Self {
        let db_path = db_path.into();
        if db_path == Path::new(DEFAULT_DB_PATH) {
            return Self::new(DEFAULT_RUNTIME_ROOT, db_path);
        }

        let root = db_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        Self::new(root, db_path)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn objects_dir(&self) -> PathBuf {
        self.root.join("objects")
    }

    pub fn generations_dir(&self) -> PathBuf {
        self.root.join("generations")
    }

    pub fn generation_path(&self, number: i64) -> PathBuf {
        self.generations_dir().join(number.to_string())
    }

    pub fn current_link(&self) -> PathBuf {
        self.root.join("current")
    }

    pub fn mount_dir(&self) -> PathBuf {
        self.root.join("mnt")
    }

    pub fn etc_state_dir(&self) -> PathBuf {
        self.root.join("etc-state")
    }

    pub fn gc_roots_dir(&self) -> PathBuf {
        self.root.join("gc-roots")
    }
}

#[cfg(test)]
mod tests {
    use super::ConaryRuntimeRoot;
    use std::path::{Path, PathBuf};

    #[test]
    fn defaults_keep_boot_visible_generation_state_under_conary() {
        let root = ConaryRuntimeRoot::default();

        assert_eq!(root.root(), Path::new("/conary"));
        assert_eq!(root.db_path(), Path::new("/var/lib/conary/conary.db"));
        assert_eq!(root.objects_dir(), Path::new("/conary/objects"));
        assert_eq!(root.generations_dir(), Path::new("/conary/generations"));
        assert_eq!(root.current_link(), Path::new("/conary/current"));
        assert_eq!(root.mount_dir(), Path::new("/conary/mnt"));
        assert_eq!(root.etc_state_dir(), Path::new("/conary/etc-state"));
        assert_eq!(root.gc_roots_dir(), Path::new("/conary/gc-roots"));
    }

    #[test]
    fn test_roots_can_use_temp_runtime_state_without_changing_db_name() {
        let root = ConaryRuntimeRoot::for_test_root("/tmp/conary-test");

        assert_eq!(root.root(), Path::new("/tmp/conary-test"));
        assert_eq!(root.db_path(), Path::new("/tmp/conary-test/conary.db"));
        assert_eq!(
            root.generation_path(7),
            Path::new("/tmp/conary-test/generations/7")
        );
    }

    #[test]
    fn default_db_path_uses_conary_runtime_root() {
        let root = ConaryRuntimeRoot::from_db_path(PathBuf::from("/var/lib/conary/conary.db"));

        assert_eq!(root.root(), Path::new("/conary"));
        assert_eq!(root.db_path(), Path::new("/var/lib/conary/conary.db"));
        assert_eq!(root.generations_dir(), Path::new("/conary/generations"));
    }

    #[test]
    fn non_default_db_paths_remain_self_contained_for_tests() {
        let root = ConaryRuntimeRoot::from_db_path(PathBuf::from("/tmp/conary-test/conary.db"));

        assert_eq!(root.root(), Path::new("/tmp/conary-test"));
        assert_eq!(root.db_path(), Path::new("/tmp/conary-test/conary.db"));
        assert_eq!(root.objects_dir(), Path::new("/tmp/conary-test/objects"));
    }
}
