// conary-core/src/recipe/hermetic/source_identity.rs

use crate::error::{Error, Result};
use crate::hash::{self, HashAlgorithm, Hasher};
use crate::recipe::hermetic::evidence::{LocalTreeIdentity, LocalTreeMode};
use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use walkdir::WalkDir;

const DEFAULT_IGNORED_DIRS: &[&str] = &[
    ".git",
    ".conary",
    "dist",
    "target",
    "node_modules",
    "__pycache__",
    ".venv",
    "build",
    "out",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CiMode {
    On,
    Off,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalLocalFile {
    pub relative_path: PathBuf,
    pub hash: String,
    pub kind: CanonicalLocalFileKind,
    pub mode: Option<u32>,
    pub symlink_target: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalLocalFileKind {
    Regular,
    Symlink,
}

pub fn detect_ci_mode() -> CiMode {
    if let Some(value) = std::env::var_os("CONARY_HERMETIC_CI") {
        let normalized = value.to_string_lossy().to_ascii_lowercase();
        if matches!(normalized.as_str(), "1" | "true" | "yes") {
            return CiMode::On;
        }
    }

    if std::env::var_os("CI").is_some() {
        return CiMode::On;
    }

    CiMode::Off
}

pub fn canonical_local_file_list(root: &Path, ci_mode: CiMode) -> Result<Vec<CanonicalLocalFile>> {
    let root = canonical_source_root(root)?;
    if is_git_work_tree(&root) {
        let status = git_status(&root)?;
        canonical_git_file_list(&root, ci_mode, &status)
    } else {
        canonical_filesystem_file_list(&root)
    }
}

pub fn validate_canonical_local_file_list(root: &Path, files: &[CanonicalLocalFile]) -> Result<()> {
    let root = canonical_source_root(root)?;
    for file in files {
        if file.relative_path.as_os_str().is_empty() {
            return Err(Error::InvalidPath(
                "Local source file list entry cannot be empty".to_string(),
            ));
        }
        if file.relative_path.is_absolute() {
            return Err(Error::InvalidPath(format!(
                "Local source file list entry must be relative, not absolute: {}",
                file.relative_path.display()
            )));
        }
        for component in file.relative_path.components() {
            match component {
                Component::Normal(_) | Component::CurDir => {}
                Component::ParentDir => {
                    return Err(Error::PathTraversal(format!(
                        "Local source file list entry contains parent traversal: {}",
                        file.relative_path.display()
                    )));
                }
                Component::RootDir | Component::Prefix(_) => {
                    return Err(Error::InvalidPath(format!(
                        "Local source file list entry must be relative, not absolute: {}",
                        file.relative_path.display()
                    )));
                }
            }
        }

        let Some(target) = &file.symlink_target else {
            continue;
        };
        let link_parent = file
            .relative_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let resolved = if target.is_absolute() {
            target.clone()
        } else {
            root.join(link_parent).join(target)
        };
        let normalized = normalize_path_without_require_existing(&resolved);
        if !normalized.starts_with(&root) {
            return Err(Error::ConfigError(format!(
                "Local source symlink must stay within the source directory: {} -> {}",
                file.relative_path.display(),
                target.display()
            )));
        }
        if let Ok(canonical_target) = std::fs::canonicalize(&normalized)
            && !canonical_target.starts_with(&root)
        {
            return Err(Error::ConfigError(format!(
                "Local source symlink must stay within the source directory: {} -> {}",
                file.relative_path.display(),
                target.display()
            )));
        }
    }
    Ok(())
}

pub fn local_tree_identity(root: &Path, ci_mode: CiMode) -> Result<LocalTreeIdentity> {
    let root = canonical_source_root(root)?;
    if is_git_work_tree(&root) {
        let status = git_status(&root)?;
        let files = canonical_git_file_list(&root, ci_mode, &status)?;
        Ok(LocalTreeIdentity {
            tree_hash: tree_hash(&files),
            file_count: files.len(),
            mode: LocalTreeMode::GitTracked,
            dirty: !status.trim().is_empty(),
            warnings: git_status_warnings(&status),
        })
    } else {
        let files = canonical_filesystem_file_list(&root)?;
        Ok(LocalTreeIdentity {
            tree_hash: tree_hash(&files),
            file_count: files.len(),
            mode: LocalTreeMode::FilesystemWalk,
            dirty: false,
            warnings: vec![
                "filesystem-walk identity is weaker than git-tracked identity".to_string(),
            ],
        })
    }
}

pub(crate) fn hash_canonical_local_file_at(
    path: &Path,
    kind: CanonicalLocalFileKind,
) -> Result<(String, Option<PathBuf>, Option<u32>)> {
    let metadata = fs::symlink_metadata(path).map_err(|e| {
        Error::NotFound(format!(
            "Local source file not found: {} ({e})",
            path.display()
        ))
    })?;

    match kind {
        CanonicalLocalFileKind::Regular => {
            if !metadata.file_type().is_file() {
                return Err(Error::ConfigError(format!(
                    "Local source entry is not a regular file: {}",
                    path.display()
                )));
            }
            let mut file = fs::File::open(path)?;
            let hex = hash::sha256_reader_hex(&mut file)?;
            Ok((format!("sha256:{hex}"), None, mode_bits(&metadata)))
        }
        CanonicalLocalFileKind::Symlink => {
            if !metadata.file_type().is_symlink() {
                return Err(Error::ConfigError(format!(
                    "Local source entry is not a symlink: {}",
                    path.display()
                )));
            }
            let target = fs::read_link(path)?;
            let hash = hash::sha256_prefixed(&path_bytes(&target));
            Ok((hash, Some(target), None))
        }
    }
}

fn canonical_source_root(root: &Path) -> Result<PathBuf> {
    let metadata = fs::metadata(root).map_err(|e| {
        Error::NotFound(format!(
            "Local source root not found: {} ({e})",
            root.display()
        ))
    })?;
    if !metadata.is_dir() {
        return Err(Error::ConfigError(format!(
            "Local source root must be a directory: {}",
            root.display()
        )));
    }
    Ok(fs::canonicalize(root)?)
}

fn is_git_work_tree(root: &Path) -> bool {
    let Ok(output) = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
    else {
        return false;
    };

    output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "true"
}

fn git_status(root: &Path) -> Result<String> {
    let output = run_git(
        root,
        &["status", "--porcelain=v1", "--untracked-files=normal"],
    )?;
    Ok(String::from_utf8_lossy(&output).into_owned())
}

fn canonical_git_file_list(
    root: &Path,
    ci_mode: CiMode,
    status: &str,
) -> Result<Vec<CanonicalLocalFile>> {
    if ci_mode == CiMode::On && !status.trim().is_empty() {
        return Err(Error::ConfigError(format!(
            "dirty local tree: {}",
            status.trim()
        )));
    }

    let output = run_git(root, &["ls-files", "-z"])?;
    let mut files = Vec::new();
    for entry in output.split(|byte| *byte == 0) {
        if entry.is_empty() {
            continue;
        }
        let relative_path = path_from_git_bytes(entry);
        files.push(canonical_file(root, relative_path)?);
    }
    files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(files)
}

fn canonical_filesystem_file_list(root: &Path) -> Result<Vec<CanonicalLocalFile>> {
    let mut files = Vec::new();
    let walker = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(should_visit_filesystem_entry);

    for entry in walker {
        let entry =
            entry.map_err(|e| Error::IoError(format!("Failed to walk local source tree: {e}")))?;
        if entry.depth() == 0 {
            continue;
        }
        let file_type = entry.file_type();
        if !file_type.is_file() && !file_type.is_symlink() {
            continue;
        }
        let relative_path = entry
            .path()
            .strip_prefix(root)
            .map_err(|e| Error::ConfigError(format!("Failed to relativize local file: {e}")))?
            .to_path_buf();
        files.push(canonical_file(root, relative_path)?);
    }

    files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(files)
}

fn should_visit_filesystem_entry(entry: &walkdir::DirEntry) -> bool {
    if entry.depth() == 0 || !entry.file_type().is_dir() {
        return true;
    }

    entry
        .file_name()
        .to_str()
        .map(|name| !DEFAULT_IGNORED_DIRS.contains(&name))
        .unwrap_or(true)
}

fn canonical_file(root: &Path, relative_path: PathBuf) -> Result<CanonicalLocalFile> {
    let path = root.join(&relative_path);
    let metadata = fs::symlink_metadata(&path).map_err(|e| {
        Error::NotFound(format!(
            "Local source file not found: {} ({e})",
            path.display()
        ))
    })?;

    let kind = if metadata.file_type().is_file() {
        CanonicalLocalFileKind::Regular
    } else if metadata.file_type().is_symlink() {
        CanonicalLocalFileKind::Symlink
    } else {
        return Err(Error::ConfigError(format!(
            "Unsupported local source file type: {}",
            path.display()
        )));
    };

    let (hash, symlink_target, mode) = hash_canonical_local_file_at(&path, kind)?;
    Ok(CanonicalLocalFile {
        relative_path,
        hash,
        kind,
        mode,
        symlink_target,
    })
}

fn git_status_warnings(status: &str) -> Vec<String> {
    let untracked = status
        .lines()
        .filter_map(|line| line.strip_prefix("?? "))
        .collect::<Vec<_>>();

    if untracked.is_empty() {
        Vec::new()
    } else {
        vec![format!(
            "untracked local source files are not included in hermetic identity: {}",
            untracked.join(", ")
        )]
    }
}

fn tree_hash(files: &[CanonicalLocalFile]) -> String {
    let mut hasher = Hasher::new(HashAlgorithm::Sha256);
    for file in files {
        hasher.update(kind_label(file.kind).as_bytes());
        hasher.update(b"\0");
        hasher.update(&path_bytes(&file.relative_path));
        hasher.update(b"\0");
        hasher.update(file.hash.as_bytes());
        hasher.update(b"\0");
        if let Some(mode) = file.mode {
            hasher.update(format!("{mode:o}").as_bytes());
        }
        hasher.update(b"\0");
        if let Some(target) = &file.symlink_target {
            hasher.update(&path_bytes(target));
        }
        hasher.update(b"\n");
    }
    let hash = hasher.finalize();
    format!("sha256:{}", hash.value)
}

fn normalize_path_without_require_existing(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            component @ Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}

fn kind_label(kind: CanonicalLocalFileKind) -> &'static str {
    match kind {
        CanonicalLocalFileKind::Regular => "regular",
        CanonicalLocalFileKind::Symlink => "symlink",
    }
}

fn run_git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| Error::IoError(format!("Failed to run git {:?}: {e}", args)))?;

    if !output.status.success() {
        return Err(Error::ConfigError(format!(
            "git {:?} failed in {}: {}",
            args,
            root.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    Ok(output.stdout)
}

#[cfg(unix)]
fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(not(unix))]
fn path_bytes(path: &Path) -> Vec<u8> {
    path.to_string_lossy().as_bytes().to_vec()
}

#[cfg(unix)]
fn path_from_git_bytes(bytes: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStringExt;
    PathBuf::from(OsString::from_vec(bytes.to_vec()))
}

#[cfg(not(unix))]
fn path_from_git_bytes(bytes: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
}

#[cfg(unix)]
fn mode_bits(metadata: &fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(metadata.permissions().mode() & 0o7777)
}

#[cfg(not(unix))]
fn mode_bits(_metadata: &fs::Metadata) -> Option<u32> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash;
    use crate::recipe::hermetic::LocalTreeMode;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::{LazyLock, Mutex};
    use tempfile::TempDir;

    static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    struct GitTreeFixture {
        dir: TempDir,
    }

    impl GitTreeFixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let fixture = Self { dir };
            fixture.git(&["init"]);
            fixture.git(&["config", "user.email", "test@example.invalid"]);
            fixture.git(&["config", "user.name", "Conary Test"]);
            fixture.git(&["config", "commit.gpgsign", "false"]);
            fixture
        }

        fn path(&self) -> &Path {
            self.dir.path()
        }

        fn write(&self, relative: &str, contents: &str) {
            let path = self.path().join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, contents).unwrap();
        }

        fn git(&self, args: &[&str]) {
            let output = Command::new("git")
                .arg("-C")
                .arg(self.path())
                .args(args)
                .output()
                .unwrap();

            assert!(
                output.status.success(),
                "git {:?} failed\nstdout:\n{}\nstderr:\n{}",
                args,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }

        fn commit_all(&self) {
            self.git(&["add", "."]);
            self.git(&["commit", "-m", "initial"]);
        }
    }

    fn with_ci_env(conary_hermetic_ci: Option<&str>, ci: Option<&str>) -> CiMode {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_conary = std::env::var_os("CONARY_HERMETIC_CI");
        let old_ci = std::env::var_os("CI");

        unsafe {
            std::env::remove_var("CONARY_HERMETIC_CI");
            std::env::remove_var("CI");
            if let Some(value) = conary_hermetic_ci {
                std::env::set_var("CONARY_HERMETIC_CI", value);
            }
            if let Some(value) = ci {
                std::env::set_var("CI", value);
            }
        }

        let mode = detect_ci_mode();

        unsafe {
            match old_conary {
                Some(value) => std::env::set_var("CONARY_HERMETIC_CI", value),
                None => std::env::remove_var("CONARY_HERMETIC_CI"),
            }
            match old_ci {
                Some(value) => std::env::set_var("CI", value),
                None => std::env::remove_var("CI"),
            }
        }

        mode
    }

    #[test]
    fn detect_ci_mode_honors_conary_override_and_generic_ci_env() {
        assert_eq!(with_ci_env(Some("1"), None), CiMode::On);
        assert_eq!(with_ci_env(Some("true"), None), CiMode::On);
        assert_eq!(with_ci_env(Some("yes"), None), CiMode::On);
        assert_eq!(with_ci_env(None, Some("")), CiMode::On);
        assert_eq!(with_ci_env(Some("0"), None), CiMode::Off);
        assert_eq!(with_ci_env(None, None), CiMode::Off);
    }

    #[test]
    fn git_tracked_identity_excludes_untracked_files() {
        let fixture = GitTreeFixture::new();
        fixture.write("tracked.txt", "tracked contents\n");
        fixture.commit_all();
        fixture.write("untracked.txt", "do not include me\n");

        let files = canonical_local_file_list(fixture.path(), CiMode::Off).unwrap();

        assert_eq!(
            files
                .iter()
                .map(|file| file.relative_path.clone())
                .collect::<Vec<_>>(),
            vec![PathBuf::from("tracked.txt")]
        );
        assert_eq!(files[0].kind, CanonicalLocalFileKind::Regular);
        assert!(files[0].hash.starts_with("sha256:"));

        let identity = local_tree_identity(fixture.path(), CiMode::Off).unwrap();
        assert_eq!(identity.mode, LocalTreeMode::GitTracked);
        assert_eq!(identity.file_count, 1);
        assert!(identity.dirty);
        assert!(
            identity
                .warnings
                .iter()
                .any(|warning| warning.contains("untracked.txt")),
            "expected untracked warning, got {:?}",
            identity.warnings
        );
    }

    #[test]
    fn ci_mode_refuses_dirty_git_tree() {
        let fixture = GitTreeFixture::new();
        fixture.write("tracked.txt", "original\n");
        fixture.commit_all();
        fixture.write("tracked.txt", "modified\n");

        let error = local_tree_identity(fixture.path(), CiMode::On).unwrap_err();

        assert!(
            error.to_string().contains("dirty local tree"),
            "expected dirty local tree error, got: {error}"
        );
    }

    #[test]
    fn git_tracked_identity_hashes_modified_worktree_contents_when_ci_is_off() {
        let fixture = GitTreeFixture::new();
        fixture.write("tracked.txt", "original\n");
        fixture.commit_all();
        fixture.write("tracked.txt", "modified\n");

        let files = canonical_local_file_list(fixture.path(), CiMode::Off).unwrap();

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].relative_path, PathBuf::from("tracked.txt"));
        assert_eq!(files[0].hash, hash::sha256_prefixed(b"modified\n"));
    }

    #[test]
    fn filesystem_walk_skips_default_ignored_entries_and_keeps_vendor() {
        let dir = tempfile::tempdir().unwrap();
        let ignored = [
            ".git",
            ".conary",
            "dist",
            "target",
            "node_modules",
            "__pycache__",
            ".venv",
            "build",
            "out",
        ];

        for entry in ignored {
            let path = dir.path().join(entry);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join("ignored.txt"), entry).unwrap();
        }
        std::fs::create_dir_all(dir.path().join("vendor")).unwrap();
        std::fs::write(dir.path().join("vendor/included.txt"), "vendored\n").unwrap();
        std::fs::write(dir.path().join("kept.txt"), "kept\n").unwrap();

        let files = canonical_local_file_list(dir.path(), CiMode::Off).unwrap();
        let paths = files
            .iter()
            .map(|file| file.relative_path.clone())
            .collect::<Vec<_>>();

        for entry in ignored {
            assert!(
                !paths.iter().any(|path| path.starts_with(entry)),
                "expected {entry} to be ignored, got {paths:?}"
            );
        }
        assert!(paths.contains(&PathBuf::from("vendor/included.txt")));
        assert!(paths.contains(&PathBuf::from("kept.txt")));

        let identity = local_tree_identity(dir.path(), CiMode::Off).unwrap();
        assert_eq!(identity.mode, LocalTreeMode::FilesystemWalk);
        assert_eq!(identity.file_count, 2);
        assert!(
            identity
                .warnings
                .iter()
                .any(|warning| warning.contains("filesystem-walk identity is weaker")),
            "expected filesystem walk warning, got {:?}",
            identity.warnings
        );
    }

    #[cfg(unix)]
    #[test]
    fn validate_canonical_file_list_rejects_symlink_escape() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), "secret\n").unwrap();
        std::os::unix::fs::symlink("../outside/secret.txt", source.join("escape.txt")).unwrap();

        let files = canonical_local_file_list(&source, CiMode::Off).unwrap();
        let err = validate_canonical_local_file_list(&source, &files).unwrap_err();

        assert!(
            err.to_string()
                .contains("Local source symlink must stay within the source directory"),
            "{err}"
        );
    }
}
