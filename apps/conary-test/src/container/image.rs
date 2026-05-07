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
    let mut first_manifest_root = None;

    loop {
        let manifest = candidate.join("Cargo.toml");
        if manifest.is_file() {
            if fs::read_to_string(&manifest)
                .with_context(|| format!("failed to read {}", manifest.display()))?
                .contains("[workspace]")
            {
                return Ok(candidate);
            }

            if first_manifest_root.is_none() {
                first_manifest_root = Some(candidate.clone());
            }
        }

        if !candidate.pop() {
            break;
        }
    }

    if let Some(root) = first_manifest_root {
        return Ok(root);
    }

    bail!("failed to locate project root from {}", start.display());
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

fn ensure_phase2_fixture_outputs(fixtures_root: &Path, conary_bin: &Path) -> Result<()> {
    let fixture_root = fixtures_root.join("conary-test-fixture");
    if !fixture_root.is_dir() {
        return Ok(());
    }

    for version in ["v1", "v2"] {
        let version_root = fixture_root.join(version);
        let manifest = version_root.join("ccs.toml");
        let source = version_root.join("stage");
        if !manifest.is_file() || !source.is_dir() {
            continue;
        }

        let output_dir = version_root.join("output");
        fs::create_dir_all(&output_dir)
            .with_context(|| format!("failed to create {}", output_dir.display()))?;

        let has_fixture = fs::read_dir(&output_dir)
            .with_context(|| format!("failed to read {}", output_dir.display()))?
            .any(|entry| {
                entry
                    .map(|entry| entry.path().extension().is_some_and(|ext| ext == "ccs"))
                    .unwrap_or(false)
            });
        if has_fixture {
            continue;
        }

        let output = std::process::Command::new(conary_bin)
            .args(["ccs", "build"])
            .arg(&manifest)
            .arg("--source")
            .arg(&source)
            .arg("--output")
            .arg(&output_dir)
            .output()
            .with_context(|| {
                format!(
                    "failed to build Phase 2 fixture {} with {}",
                    manifest.display(),
                    conary_bin.display()
                )
            })?;

        if !output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "failed to build Phase 2 fixture {}\nstdout:\n{}\nstderr:\n{}",
                manifest.display(),
                stdout.trim_end(),
                stderr.trim_end()
            );
        }
    }

    Ok(())
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

    let fixtures_src = crate::paths::resolve_fixtures_root_for(&project_root);
    if fixtures_src.is_dir() {
        copy_dir_filtered(&fixtures_src, &root.join("fixtures"), &[])?;
    } else {
        fs::create_dir_all(root.join("fixtures"))?;
    }

    let arch_pkgbuild = project_root.join("packaging/arch/PKGBUILD");
    if arch_pkgbuild.is_file() {
        let pkgbuild_dir = root.join("fixtures/pkgbuild");
        fs::create_dir_all(&pkgbuild_dir)?;
        fs::copy(&arch_pkgbuild, pkgbuild_dir.join("PKGBUILD"))
            .with_context(|| format!("failed to copy {}", arch_pkgbuild.display()))?;
    }

    let binary = crate::paths::find_host_conary_binary(&project_root)?;
    let staged_binary = root.join("conary");
    fs::copy(&binary, &staged_binary)
        .with_context(|| format!("failed to stage host conary binary {}", binary.display()))?;

    // Strip debug symbols to shrink the tar context sent over the container
    // socket. A debug build can be 300MB+; stripped it drops to ~70MB, which
    // avoids Podman compat-API stream errors on large payloads.
    let _ = std::process::Command::new("strip")
        .arg(&staged_binary)
        .status();

    ensure_phase2_fixture_outputs(&root.join("fixtures"), &binary)?;

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
    let tag = format!("conary-test-{distro}:latest");
    let force_rebuild = std::env::var("CONARY_TEST_REBUILD_IMAGE")
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false);

    if !force_rebuild {
        match tokio::process::Command::new("podman")
            .args(["image", "exists", &tag])
            .output()
            .await
        {
            Ok(output) if output.status.success() => {
                tracing::info!(image = %tag, "reusing existing distro test image");
                return Ok(tag);
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).context("failed to check existing distro test image"),
        }
    }

    let staged = stage_build_context(containerfile, distro)?;
    backend
        .build_image(&staged.dockerfile, &tag, HashMap::new())
        .await
}

#[cfg(test)]
mod tests {
    use super::{find_project_root, stage_build_context};
    use std::fs;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn stage_build_context_creates_small_remi_context() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let project_root = std::env::temp_dir().join(format!("conary-test-stage-context-{unique}"));
        let remi_root = project_root.join("tests/integration/remi");
        let containerfile = remi_root.join("containers/Containerfile.fedora44");

        fs::create_dir_all(remi_root.join("containers")).expect("create containers");
        fs::create_dir_all(project_root.join("tests/fixtures/recipes/simple-hello"))
            .expect("create fixtures");
        fs::create_dir_all(project_root.join("tests/fixtures/conary-test-fixture/v1/output"))
            .expect("create fixture output");
        fs::create_dir_all(project_root.join("packaging/arch")).expect("create packaging");

        fs::write(
            project_root.join("Cargo.toml"),
            "[workspace]\nmembers = []\n",
        )
        .expect("write cargo");
        fs::write(project_root.join("conary"), "binary").expect("write binary");
        fs::write(&containerfile, "FROM scratch\n").expect("write containerfile");
        fs::write(remi_root.join("config.toml"), "[paths]\n").expect("write config");
        fs::write(
            project_root.join("tests/fixtures/recipes/simple-hello/recipe.toml"),
            "name = 'simple-hello'\n",
        )
        .expect("write fixture");
        fs::write(
            project_root.join("tests/fixtures/conary-test-fixture/v1/output/test.ccs"),
            "fixture-bytes",
        )
        .expect("write fixture output");
        fs::write(
            project_root.join("packaging/arch/PKGBUILD"),
            "pkgname=conary\n",
        )
        .expect("write pkgbuild");

        let staged = stage_build_context(&containerfile, "fedora44").expect("stage build context");

        assert!(
            staged
                .root
                .join("containers/Containerfile.fedora44")
                .is_file()
        );
        assert!(staged.root.join("config.toml").is_file());
        assert!(
            staged
                .root
                .join("fixtures/recipes/simple-hello/recipe.toml")
                .is_file()
        );
        assert!(
            staged
                .root
                .join("fixtures/conary-test-fixture/v1/output/test.ccs")
                .is_file()
        );
        assert!(staged.root.join("fixtures/pkgbuild/PKGBUILD").is_file());
        assert!(staged.root.join("conary").is_file());
        assert!(!staged.root.join("target").exists());
        assert!(!staged.root.join("source").exists());

        drop(staged);
        fs::remove_dir_all(project_root).expect("cleanup project root");
    }

    #[test]
    #[cfg(unix)]
    fn stage_build_context_generates_missing_phase2_fixture_outputs() {
        use std::os::unix::fs::PermissionsExt;

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let project_root =
            std::env::temp_dir().join(format!("conary-test-phase2-fixtures-{unique}"));
        let remi_root = project_root.join("tests/integration/remi");
        let fixture_root = project_root.join("tests/fixtures/conary-test-fixture");
        let containerfile = remi_root.join("containers/Containerfile.arch");
        let conary = project_root.join("conary");

        fs::create_dir_all(remi_root.join("containers")).expect("create containers");
        fs::create_dir_all(fixture_root.join("v1/stage/usr/share/conary-test"))
            .expect("create v1 fixture source");
        fs::create_dir_all(fixture_root.join("v2/stage/usr/share/conary-test"))
            .expect("create v2 fixture source");
        fs::write(
            project_root.join("Cargo.toml"),
            "[workspace]\nmembers = []\n",
        )
        .expect("write cargo");
        fs::write(&containerfile, "FROM scratch\n").expect("write containerfile");
        fs::write(remi_root.join("config.toml"), "[paths]\n").expect("write config");
        fs::write(fixture_root.join("v1/ccs.toml"), "[package]\n").expect("write v1 ccs");
        fs::write(fixture_root.join("v2/ccs.toml"), "[package]\n").expect("write v2 ccs");
        fs::write(
            fixture_root.join("v1/stage/usr/share/conary-test/hello.txt"),
            "hello v1\n",
        )
        .expect("write v1 source");
        fs::write(
            fixture_root.join("v2/stage/usr/share/conary-test/hello.txt"),
            "hello v2\n",
        )
        .expect("write v2 source");
        fs::write(
            &conary,
            r#"#!/usr/bin/env bash
set -euo pipefail
manifest="$3"
output="${!#}"
case "$manifest" in
  */v1/ccs.toml) file="conary-test-fixture-1.0.0.ccs" ;;
  */v2/ccs.toml) file="conary-test-fixture-2.0.0.ccs" ;;
  *) echo "unexpected manifest: $manifest" >&2; exit 2 ;;
esac
mkdir -p "$output"
printf 'fixture\n' > "$output/$file"
"#,
        )
        .expect("write fake conary");
        let mut permissions = fs::metadata(&conary)
            .expect("fake conary metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&conary, permissions).expect("make fake conary executable");

        let staged = stage_build_context(&containerfile, "arch").expect("stage build context");

        assert!(
            staged
                .root
                .join("fixtures/conary-test-fixture/v1/output/conary-test-fixture-1.0.0.ccs")
                .is_file()
        );
        assert!(
            staged
                .root
                .join("fixtures/conary-test-fixture/v2/output/conary-test-fixture-2.0.0.ccs")
                .is_file()
        );

        drop(staged);
        fs::remove_dir_all(project_root).expect("cleanup project root");
    }

    #[test]
    fn find_project_root_prefers_workspace_root_over_nested_package() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let workspace_root =
            std::env::temp_dir().join(format!("conary-test-workspace-root-{unique}"));
        let integration_root = workspace_root.join("apps/conary/tests/integration/remi");

        fs::create_dir_all(integration_root.join("containers")).expect("create integration tree");
        fs::write(
            workspace_root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"apps/conary\"]\n",
        )
        .expect("write workspace cargo");
        fs::create_dir_all(workspace_root.join("apps/conary")).expect("create nested app");
        fs::write(
            workspace_root.join("apps/conary/Cargo.toml"),
            "[package]\nname = \"conary\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .expect("write nested package cargo");

        let found = find_project_root(Path::new(&integration_root)).expect("find project root");
        assert_eq!(found, workspace_root);

        fs::remove_dir_all(workspace_root).expect("cleanup workspace root");
    }

    #[test]
    fn ubuntu_source_builder_installs_native_crypto_build_tools() {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let containerfile = manifest_dir
            .join("../conary/tests/integration/remi/containers/Containerfile.ubuntu-26.04");
        let contents = fs::read_to_string(&containerfile).expect("read ubuntu containerfile");

        assert!(
            contents.contains("cmake"),
            "Ubuntu source-build image must install cmake for aws-lc-sys"
        );
    }
}
