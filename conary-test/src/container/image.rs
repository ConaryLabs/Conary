// conary-test/src/container/image.rs

use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::backend::ContainerBackend;

struct StagedBuildContext {
    root: PathBuf,
    dockerfile: PathBuf,
}

impl Drop for StagedBuildContext {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn find_project_root(start: &Path) -> Result<PathBuf> {
    let mut candidate = start
        .canonicalize()
        .context("failed to canonicalize path when locating project root")?;

    loop {
        if candidate.join("Cargo.toml").is_file() {
            return Ok(candidate);
        }

        if !candidate.pop() {
            bail!("failed to locate project root from {}", start.display());
        }
    }
}

fn copy_dir_filtered(src: &Path, dst: &Path, skip_names: &[&str]) -> Result<()> {
    fs::create_dir_all(dst)
        .with_context(|| format!("failed to create directory {}", dst.display()))?;

    for entry in
        fs::read_dir(src).with_context(|| format!("failed to read directory {}", src.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if skip_names.iter().any(|skip| *skip == name) {
            continue;
        }

        let target = dst.join(name.as_ref());
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            copy_dir_filtered(&path, &target, skip_names)?;
        } else if file_type.is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&path, &target).with_context(|| {
                format!("failed to copy {} to {}", path.display(), target.display())
            })?;
        } else if file_type.is_symlink() {
            let link_target = fs::read_link(&path)
                .with_context(|| format!("failed to read symlink {}", path.display()))?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(&link_target, &target).with_context(|| {
                format!(
                    "failed to recreate symlink {} -> {}",
                    target.display(),
                    link_target.display()
                )
            })?;
        }
    }

    Ok(())
}

fn find_host_conary_binary(project_root: &Path) -> Result<PathBuf> {
    let candidates = [
        std::env::var_os("CONARY_HOST_BIN").map(PathBuf::from),
        std::env::var_os("CONARY_BIN").map(PathBuf::from),
        Some(project_root.join("conary")),
        Some(project_root.join("target/debug/conary")),
        Some(project_root.join("target/release/conary")),
    ];

    for candidate in candidates.into_iter().flatten() {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    bail!(
        "failed to locate host conary binary; tried CONARY_HOST_BIN, CONARY_BIN, ./conary, target/debug/conary, and target/release/conary"
    )
}

fn stage_build_context(containerfile: &Path, distro: &str) -> Result<StagedBuildContext> {
    let integration_root = containerfile
        .parent()
        .and_then(Path::parent)
        .context("containerfile is missing expected remi directory structure")?;
    let project_root = find_project_root(integration_root)?;

    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system time before unix epoch")?
        .as_nanos();
    let root = std::env::temp_dir().join(format!("conary-test-image-{distro}-{unique}"));

    fs::create_dir_all(root.join("containers"))?;
    let dockerfile_name = containerfile
        .file_name()
        .context("containerfile has no filename")?;
    fs::copy(containerfile, root.join("containers").join(dockerfile_name))
        .with_context(|| format!("failed to copy {}", containerfile.display()))?;
    fs::copy(
        integration_root.join("config.toml"),
        root.join("config.toml"),
    )
    .context("failed to copy integration config.toml")?;
    copy_dir_filtered(
        &integration_root.join("runner"),
        &root.join("runner"),
        &["__pycache__"],
    )?;

    let fixtures_src = project_root.join("tests/fixtures");
    if fixtures_src.is_dir() {
        copy_dir_filtered(&fixtures_src, &root.join("fixtures"), &["output"])?;
    } else {
        fs::create_dir_all(root.join("fixtures"))?;
    }

    let arch_pkgbuild = project_root.join("packaging/arch/PKGBUILD");
    if arch_pkgbuild.is_file() {
        let pkgbuild_dir = root.join("fixtures/pkgbuild");
        fs::create_dir_all(&pkgbuild_dir)?;
        fs::copy(&arch_pkgbuild, pkgbuild_dir.join("PKGBUILD")).with_context(|| {
            format!("failed to copy {}", arch_pkgbuild.display())
        })?;
    }

    let binary = find_host_conary_binary(&project_root)?;
    fs::copy(&binary, root.join("conary"))
        .with_context(|| format!("failed to stage host conary binary {}", binary.display()))?;

    if distro.starts_with("ubuntu-") {
        copy_dir_filtered(
            &project_root,
            &root.join("source"),
            &[".git", "target", ".jj", ".direnv"],
        )?;
    }

    Ok(StagedBuildContext {
        dockerfile: root.join("containers").join(dockerfile_name),
        root,
    })
}

/// Build a distro-specific test image from a Containerfile.
///
/// Tags the image as `conary-test-{distro}:latest`.
pub async fn build_distro_image(
    backend: &dyn ContainerBackend,
    containerfile: &Path,
    distro: &str,
) -> Result<String> {
    let staged = stage_build_context(containerfile, distro)?;
    let tag = format!("conary-test-{distro}:latest");
    backend
        .build_image(&staged.dockerfile, &tag, HashMap::new())
        .await
}

#[cfg(test)]
mod tests {
    use super::stage_build_context;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn stage_build_context_creates_small_remi_context() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let project_root = std::env::temp_dir().join(format!("conary-test-stage-context-{unique}"));
        let remi_root = project_root.join("tests/integration/remi");
        let containerfile = remi_root.join("containers/Containerfile.fedora43");

        fs::create_dir_all(remi_root.join("containers")).expect("create containers");
        fs::create_dir_all(remi_root.join("runner")).expect("create runner");
        fs::create_dir_all(project_root.join("tests/fixtures/recipes/simple-hello"))
            .expect("create fixtures");
        fs::create_dir_all(project_root.join("packaging/arch")).expect("create packaging");

        fs::write(project_root.join("Cargo.toml"), "[workspace]\nmembers = []\n")
            .expect("write cargo");
        fs::write(project_root.join("conary"), "binary").expect("write binary");
        fs::write(&containerfile, "FROM scratch\n").expect("write containerfile");
        fs::write(remi_root.join("config.toml"), "[paths]\n").expect("write config");
        fs::write(remi_root.join("runner/test_runner.py"), "print('ok')\n").expect("write runner");
        fs::write(
            project_root.join("tests/fixtures/recipes/simple-hello/recipe.toml"),
            "name = 'simple-hello'\n",
        )
        .expect("write fixture");
        fs::write(project_root.join("packaging/arch/PKGBUILD"), "pkgname=conary\n")
            .expect("write pkgbuild");

        let staged = stage_build_context(&containerfile, "fedora43").expect("stage build context");

        assert!(staged.root.join("containers/Containerfile.fedora43").is_file());
        assert!(staged.root.join("config.toml").is_file());
        assert!(staged.root.join("runner/test_runner.py").is_file());
        assert!(staged
            .root
            .join("fixtures/recipes/simple-hello/recipe.toml")
            .is_file());
        assert!(staged.root.join("fixtures/pkgbuild/PKGBUILD").is_file());
        assert!(staged.root.join("conary").is_file());
        assert!(!staged.root.join("target").exists());
        assert!(!staged.root.join("source").exists());

        drop(staged);
        fs::remove_dir_all(project_root).expect("cleanup project root");
    }
}
