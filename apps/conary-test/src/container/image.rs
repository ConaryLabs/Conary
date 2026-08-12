// conary-test/src/container/image.rs

use anyhow::{Context, Result, bail};
use conary_core::repository::supported_profiles::ProfilePackageFormat;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::backend::ContainerBackend;
use crate::config::DistroBuildContext;

#[derive(Debug)]
struct StagedBuildContext {
    root: PathBuf,
    dockerfile: PathBuf,
}

#[derive(Debug, Clone, Copy)]
struct NativePackageArtifact<'a> {
    path: &'a Path,
    format: ProfilePackageFormat,
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
    let signing_key = crate::paths::fixture_ccs_key_path_for(fixtures_root);
    let trust_policy = crate::paths::fixture_ccs_policy_path_for(fixtures_root);
    for authority_path in [&signing_key, &trust_policy] {
        if !authority_path.is_file() {
            bail!(
                "Phase 2 fixture authority is missing {}; regenerate apps/conary/tests/fixtures/ccs-test-authority",
                authority_path.display()
            );
        }
    }

    for version in ["v1", "v2"] {
        let version_root = fixture_root.join(version);
        let manifest = version_root.join("ccs.toml");
        let source = version_root.join("stage");
        if !manifest.is_file() || !source.is_dir() {
            continue;
        }

        let output_dir = version_root.join("output");
        if output_dir.exists() {
            fs::remove_dir_all(&output_dir)
                .with_context(|| format!("failed to reset {}", output_dir.display()))?;
        }
        fs::create_dir_all(&output_dir)
            .with_context(|| format!("failed to create {}", output_dir.display()))?;

        let output = std::process::Command::new(conary_bin)
            .args(["ccs", "build"])
            .arg(&manifest)
            .arg("--source")
            .arg(&source)
            .arg("--output")
            .arg(&output_dir)
            .arg("--key")
            .arg(&signing_key)
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

        let packages = fs::read_dir(&output_dir)
            .with_context(|| format!("failed to read {}", output_dir.display()))?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.extension().is_some_and(|extension| extension == "ccs"))
            .collect::<Vec<_>>();
        if packages.len() != 1 {
            bail!(
                "expected one signed Phase 2 fixture in {}, found {}",
                output_dir.display(),
                packages.len()
            );
        }
        let verify = std::process::Command::new(conary_bin)
            .args(["ccs", "verify"])
            .arg(&packages[0])
            .arg("--policy")
            .arg(&trust_policy)
            .output()
            .with_context(|| {
                format!(
                    "failed to verify Phase 2 fixture {} with {}",
                    packages[0].display(),
                    conary_bin.display()
                )
            })?;
        if !verify.status.success() {
            let stdout = String::from_utf8_lossy(&verify.stdout);
            let stderr = String::from_utf8_lossy(&verify.stderr);
            bail!(
                "Phase 2 fixture {} did not verify under {}\nstdout:\n{}\nstderr:\n{}",
                packages[0].display(),
                trust_policy.display(),
                stdout.trim_end(),
                stderr.trim_end()
            );
        }
    }

    Ok(())
}

fn canonical_native_package_name(format: ProfilePackageFormat) -> &'static str {
    match format {
        ProfilePackageFormat::Rpm => "conary-release.rpm",
        ProfilePackageFormat::Deb => "conary-release.deb",
        ProfilePackageFormat::Arch => "conary-release.pkg.tar.zst",
    }
}

fn stage_native_package(root: &Path, artifact: NativePackageArtifact<'_>) -> Result<()> {
    let metadata = fs::symlink_metadata(artifact.path).with_context(|| {
        format!(
            "failed to inspect native package artifact {}",
            artifact.path.display()
        )
    })?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        bail!(
            "native package artifact must be a real regular file: {}",
            artifact.path.display()
        );
    }
    if metadata.len() == 0 {
        bail!(
            "native package artifact must not be empty: {}",
            artifact.path.display()
        );
    }

    let destination = root.join(canonical_native_package_name(artifact.format));
    fs::copy(artifact.path, &destination).with_context(|| {
        format!(
            "failed to stage native package artifact {} as {}",
            artifact.path.display(),
            destination.display()
        )
    })?;
    Ok(())
}

/// Pick the Conary binary an image receives.
///
/// The static choice fails closed rather than falling back to the host build:
/// a host binary that happens to run in one image and dies at the dynamic
/// linker in another is the exact failure this capability removes.
fn resolve_stage_source(
    build_context: DistroBuildContext,
    host_binary: &Path,
    target_dir: &Path,
) -> Result<PathBuf> {
    match build_context {
        DistroBuildContext::Binary => Ok(host_binary.to_path_buf()),
        DistroBuildContext::StaticBinary => {
            crate::static_binary::static_conary_binary_in(target_dir)
        }
    }
}

fn stage_build_context(
    containerfile: &Path,
    distro: &str,
    build_context: DistroBuildContext,
    native_package: Option<NativePackageArtifact<'_>>,
) -> Result<StagedBuildContext> {
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

    // Fixture packages are built by running Conary on this host, so that step
    // always uses the host binary. What gets staged into the image is a
    // separate, typed choice: the image's userland is not the host's.
    let host_binary = crate::paths::find_host_conary_binary(&project_root)?;
    let stage_source = resolve_stage_source(
        build_context,
        &host_binary,
        &crate::static_binary::static_target_dir(&project_root),
    )?;
    let staged_binary = root.join("conary");
    fs::copy(&stage_source, &staged_binary)
        .with_context(|| format!("failed to stage conary binary {}", stage_source.display()))?;

    // Strip debug symbols to shrink the tar context sent over the container
    // socket. A debug build can be 300MB+; stripped it drops to ~70MB, which
    // avoids Podman compat-API stream errors on large payloads.
    let _ = std::process::Command::new("strip")
        .arg(&staged_binary)
        .status();

    if let Some(artifact) = native_package {
        stage_native_package(&root, artifact)?;
    }

    ensure_phase2_fixture_outputs(&root.join("fixtures"), &host_binary)?;

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
    build_context: DistroBuildContext,
) -> Result<String> {
    build_distro_image_inner(backend, containerfile, distro, build_context, None).await
}

/// Build a distro-specific test image by installing one exact native package.
///
/// The package format comes from Conary's typed supported-profile catalog.
/// Containerfiles install the staged canonical filename through the distro's
/// native package manager before any lifecycle proof runs.
pub async fn build_distro_image_from_native_package(
    backend: &dyn ContainerBackend,
    containerfile: &Path,
    distro: &str,
    build_context: DistroBuildContext,
    package: &Path,
    package_format: ProfilePackageFormat,
) -> Result<String> {
    build_distro_image_inner(
        backend,
        containerfile,
        distro,
        build_context,
        Some(NativePackageArtifact {
            path: package,
            format: package_format,
        }),
    )
    .await
}

async fn build_distro_image_inner(
    backend: &dyn ContainerBackend,
    containerfile: &Path,
    distro: &str,
    build_context: DistroBuildContext,
    native_package: Option<NativePackageArtifact<'_>>,
) -> Result<String> {
    let tag = format!("conary-test-{distro}:latest");
    let force_rebuild = std::env::var("CONARY_TEST_REBUILD_IMAGE")
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false);
    let reuse_existing = native_package.is_none()
        && std::env::var("CONARY_TEST_REUSE_IMAGE")
            .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
            .unwrap_or(false);

    if reuse_existing && !force_rebuild {
        let images = backend
            .list_images()
            .await
            .context("failed to inspect existing distro test images")?;
        if images
            .iter()
            .any(|image| image.tags.iter().any(|candidate| candidate == &tag))
        {
            tracing::info!(image = %tag, "reusing existing distro test image");
            return Ok(tag);
        }
    }

    let staged = stage_build_context(containerfile, distro, build_context, native_package)?;
    let mut build_args = HashMap::new();
    if native_package.is_some() {
        build_args.insert("INSTALL_MODE".to_string(), "package".to_string());
    }
    backend
        .build_image(&staged.dockerfile, &tag, build_args)
        .await
}

#[cfg(test)]
mod tests {
    use super::{
        NativePackageArtifact, find_project_root, resolve_stage_source, stage_build_context,
        stage_native_package,
    };
    use crate::config::DistroBuildContext;
    use conary_core::repository::supported_profiles::ProfilePackageFormat;
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
        let remi_root = project_root.join("apps/conary/tests/integration/remi");
        let containerfile = remi_root.join("containers/Containerfile.fedora44");

        fs::create_dir_all(remi_root.join("containers")).expect("create containers");
        fs::create_dir_all(project_root.join("apps/conary/tests/fixtures/recipes/simple-hello"))
            .expect("create fixtures");
        fs::create_dir_all(
            project_root.join("apps/conary/tests/fixtures/conary-test-fixture/v1/output"),
        )
        .expect("create fixture output");
        fs::create_dir_all(project_root.join("apps/conary/tests/fixtures/ccs-test-authority"))
            .expect("create fixture authority");
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
            project_root.join("apps/conary/tests/fixtures/recipes/simple-hello/recipe.toml"),
            "name = 'simple-hello'\n",
        )
        .expect("write fixture");
        fs::write(
            project_root.join("apps/conary/tests/fixtures/conary-test-fixture/v1/output/test.ccs"),
            "fixture-bytes",
        )
        .expect("write fixture output");
        fs::write(
            project_root
                .join("apps/conary/tests/fixtures/ccs-test-authority/fixture-signing-key.private"),
            "test private key\n",
        )
        .expect("write fixture private key");
        fs::write(
            project_root.join("apps/conary/tests/fixtures/ccs-test-authority/trust-policy.toml"),
            "trusted_keys = [\"test\"]\n",
        )
        .expect("write fixture trust policy");
        fs::write(
            project_root.join("packaging/arch/PKGBUILD"),
            "pkgname=conary\n",
        )
        .expect("write pkgbuild");

        let staged =
            stage_build_context(&containerfile, "fedora44", DistroBuildContext::Binary, None)
                .expect("stage build context");

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

        let package = project_root.join("release.rpm");
        fs::write(&package, "published package bytes").expect("write release package");
        let staged = stage_build_context(
            &containerfile,
            "fedora44",
            DistroBuildContext::Binary,
            Some(NativePackageArtifact {
                path: &package,
                format: ProfilePackageFormat::Rpm,
            }),
        )
        .expect("stage native package build context");
        assert_eq!(
            fs::read(staged.root.join("conary-release.rpm")).expect("read staged package"),
            b"published package bytes"
        );

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
        let remi_root = project_root.join("apps/conary/tests/integration/remi");
        let fixture_root = project_root.join("apps/conary/tests/fixtures/conary-test-fixture");
        let authority_root = project_root.join("apps/conary/tests/fixtures/ccs-test-authority");
        let containerfile = remi_root.join("containers/Containerfile.arch");
        let conary = project_root.join("conary");

        fs::create_dir_all(remi_root.join("containers")).expect("create containers");
        fs::create_dir_all(fixture_root.join("v1/stage/usr/share/conary-test"))
            .expect("create v1 fixture source");
        fs::create_dir_all(fixture_root.join("v2/stage/usr/share/conary-test"))
            .expect("create v2 fixture source");
        fs::create_dir_all(&authority_root).expect("create fixture authority");
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
            authority_root.join("fixture-signing-key.private"),
            "test private key\n",
        )
        .expect("write fixture private key");
        fs::write(
            authority_root.join("trust-policy.toml"),
            "trusted_keys = [\"test\"]\n",
        )
        .expect("write fixture trust policy");
        fs::write(
            &conary,
            r#"#!/usr/bin/env bash
set -euo pipefail
if [[ "$1 $2" == "ccs verify" ]]; then
  [[ "$4" == "--policy" ]]
  exit 0
fi
manifest="$3"
output=""
for ((i = 1; i <= $#; i++)); do
  if [[ "${!i}" == "--output" ]]; then
    next=$((i + 1))
    output="${!next}"
  fi
done
case "$manifest" in
  */v1/ccs.toml) file="conary-test-fixture-1.0.0-1.ccs" ;;
  */v2/ccs.toml) file="conary-test-fixture-2.0.0-1.ccs" ;;
  *) echo "unexpected manifest: $manifest" >&2; exit 2 ;;
esac
[[ -n "$output" ]]
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

        let staged = stage_build_context(&containerfile, "arch", DistroBuildContext::Binary, None)
            .expect("stage build context");

        assert!(
            staged
                .root
                .join("fixtures/conary-test-fixture/v1/output/conary-test-fixture-1.0.0-1.ccs")
                .is_file()
        );
        assert!(
            staged
                .root
                .join("fixtures/conary-test-fixture/v2/output/conary-test-fixture-2.0.0-1.ccs")
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

    /// Which binary reaches the image is a typed decision, and the static
    /// choice fails closed rather than quietly staging the host build.
    #[test]
    fn staged_binary_is_selected_by_typed_capability() {
        let target_dir = tempfile::tempdir().expect("create temp target directory");
        let host_binary = Path::new("/workspace/target/debug/conary");

        assert_eq!(
            resolve_stage_source(DistroBuildContext::Binary, host_binary, target_dir.path())
                .expect("host staging needs no static artifact"),
            host_binary,
            "the binary capability must stage the host build"
        );

        let error = resolve_stage_source(
            DistroBuildContext::StaticBinary,
            host_binary,
            target_dir.path(),
        )
        .expect_err("a missing static artifact must not fall back to the host binary");
        assert!(
            error
                .to_string()
                .contains("static conary artifact not found"),
            "unexpected message: {error}"
        );

        let artifact = crate::static_binary::static_conary_binary_path_in(target_dir.path());
        fs::create_dir_all(artifact.parent().expect("artifact parent"))
            .expect("create musl profile directory");
        fs::write(
            &artifact,
            crate::static_binary::synthetic_elf(&[crate::static_binary::PT_LOAD]),
        )
        .expect("write static artifact");

        assert_eq!(
            resolve_stage_source(
                DistroBuildContext::StaticBinary,
                host_binary,
                target_dir.path()
            )
            .expect("static staging must accept a static artifact"),
            artifact,
            "the static capability must stage the musl artifact"
        );
    }

    /// Every image now receives an already-built binary. Prove no Containerfile
    /// still expects a staged workspace to compile from.
    #[test]
    fn containerfiles_install_a_staged_binary_without_building_from_source() {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let containers = manifest_dir.join("../conary/tests/integration/remi/containers");
        for file in [
            "Containerfile.fedora44",
            "Containerfile.ubuntu-26.04",
            "Containerfile.arch",
            "Containerfile.artix",
        ] {
            let contents =
                fs::read_to_string(containers.join(file)).expect("read distro containerfile");
            assert!(
                contents.contains("install -m 755 /tmp/install/conary /usr/bin/conary"),
                "{file} must install the staged binary"
            );
            assert!(
                !contents.contains("source/"),
                "{file} must not stage the workspace source tree"
            );
            assert!(
                !contents.contains("cargo build"),
                "{file} must not build Conary from source"
            );
        }
    }

    #[test]
    fn package_mode_containerfiles_install_exact_canonical_artifacts() {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let containers = manifest_dir.join("../conary/tests/integration/remi/containers");
        for (file, package, install) in [
            (
                "Containerfile.fedora44",
                "conary-release.rpm",
                "dnf install -y /tmp/install/conary-release.rpm",
            ),
            (
                "Containerfile.ubuntu-26.04",
                "conary-release.deb",
                "apt-get install -y /tmp/install/conary-release.deb",
            ),
            (
                "Containerfile.arch",
                "conary-release.pkg.tar.zst",
                "pacman -U --noconfirm /tmp/install/conary-release.pkg.tar.zst",
            ),
            (
                "Containerfile.artix",
                "conary-release.pkg.tar.zst",
                "pacman -U --noconfirm /tmp/install/conary-release.pkg.tar.zst",
            ),
        ] {
            let contents =
                fs::read_to_string(containers.join(file)).expect("read package-mode containerfile");
            assert!(contents.contains(package), "{file} must name {package}");
            assert!(contents.contains(install), "{file} must run {install}");
        }
    }

    #[test]
    fn native_package_staging_rejects_empty_files() {
        let directory = tempfile::tempdir().expect("create temp directory");
        let package = directory.path().join("empty.rpm");
        fs::write(&package, []).expect("write empty package");

        let error = stage_native_package(
            directory.path(),
            NativePackageArtifact {
                path: &package,
                format: ProfilePackageFormat::Rpm,
            },
        )
        .expect_err("empty package must fail");

        assert!(error.to_string().contains("must not be empty"));
    }
}
