// conary-core/src/repository/static_repo/publish.rs

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

pub fn prepare_static_key_dir(base: &Path, repo_name: &str) -> Result<PathBuf> {
    validate_static_repo_name(repo_name)?;

    let key_dir = base.join(repo_name);
    create_private_dir_all(&key_dir)
        .with_context(|| format!("create static repo key directory {}", key_dir.display()))?;

    Ok(key_dir)
}

fn validate_static_repo_name(repo_name: &str) -> Result<()> {
    if repo_name.trim().is_empty() {
        bail!("repo name must not be empty and must be one safe path segment");
    }

    let repo_path = Path::new(repo_name);
    if repo_path.is_absolute()
        || repo_name.contains('/')
        || repo_name.contains('\\')
        || repo_name == "."
        || repo_name == ".."
        || repo_path.components().count() != 1
    {
        bail!("repo name must be one safe path segment");
    }

    Ok(())
}

#[cfg(unix)]
fn create_private_dir_all(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn create_private_dir_all(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)
}

#[cfg(test)]
mod tests {
    use super::prepare_static_key_dir;

    #[test]
    #[cfg(unix)]
    fn prepare_static_key_dir_creates_repo_key_dir_0700() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = tempfile::tempdir().unwrap();
        let key_base = temp_dir.path().join(".config/conary/keys");

        let key_dir = prepare_static_key_dir(&key_base, "test-repo").unwrap();

        assert_eq!(key_dir, key_base.join("test-repo"));
        assert!(key_dir.is_dir());
        let mode = std::fs::metadata(&key_dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn prepare_static_key_dir_rejects_empty_repo_name() {
        let temp_dir = tempfile::tempdir().unwrap();

        let error = prepare_static_key_dir(temp_dir.path(), "").unwrap_err();

        assert!(
            error.to_string().contains("repo name must not be empty"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn prepare_static_key_dir_rejects_unsafe_repo_name_segments() {
        let temp_dir = tempfile::tempdir().unwrap();
        let key_base = temp_dir.path().join(".config/conary/keys");
        let absolute_name = temp_dir.path().join("escape").display().to_string();

        for repo_name in [
            "nested/repo",
            "../escape",
            &absolute_name,
            r"nested\repo",
            "   ",
            ".",
            "..",
        ] {
            let error = prepare_static_key_dir(&key_base, repo_name).unwrap_err();
            assert!(
                error.to_string().contains("safe path segment"),
                "unexpected error for {repo_name:?}: {error}"
            );
        }
    }
}
