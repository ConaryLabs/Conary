// apps/conary-test/src/container/image/tests.rs

use super::{
    NativePackageArtifact, find_project_root, resolve_stage_source, stage_build_context,
    stage_native_package,
};
use crate::config::DistroBuildContext;
use conary_core::repository::supported_profiles::ProfilePackageFormat;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[cfg(unix)]
#[test]
fn filtered_copy_preserves_directory_modes() {
    let source = tempfile::tempdir().expect("create source directory");
    let destination = tempfile::tempdir().expect("create destination directory");
    let fixture = source.path().join("fixture");
    let nested = fixture.join("usr/bin");
    fs::create_dir_all(&nested).expect("create fixture tree");
    fs::set_permissions(&fixture, fs::Permissions::from_mode(0o751))
        .expect("set fixture root mode");
    fs::set_permissions(fixture.join("usr"), fs::Permissions::from_mode(0o755))
        .expect("set usr mode");
    fs::set_permissions(&nested, fs::Permissions::from_mode(0o750)).expect("set nested mode");

    let copied = destination.path().join("fixture");
    super::copy_dir_filtered(&fixture, &copied, &[]).expect("copy fixture tree");

    assert_eq!(
        fs::metadata(&copied)
            .expect("copied root metadata")
            .permissions()
            .mode()
            & 0o7777,
        0o751
    );
    assert_eq!(
        fs::metadata(copied.join("usr"))
            .expect("copied usr metadata")
            .permissions()
            .mode()
            & 0o7777,
        0o755
    );
    assert_eq!(
        fs::metadata(copied.join("usr/bin"))
            .expect("copied nested metadata")
            .permissions()
            .mode()
            & 0o7777,
        0o750
    );
}

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

    let staged = stage_build_context(&containerfile, "fedora44", DistroBuildContext::Binary, None)
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
    let project_root = std::env::temp_dir().join(format!("conary-test-phase2-fixtures-{unique}"));
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
    let workspace_root = std::env::temp_dir().join(format!("conary-test-workspace-root-{unique}"));
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
        "Containerfile.debian-derivative",
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
fn release_containerfiles_keep_a_separate_test_hook_binary() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let containers = manifest_dir.join("../conary/tests/integration/remi/containers");
    for file in [
        "Containerfile.fedora44",
        "Containerfile.ubuntu-26.04",
        "Containerfile.arch",
    ] {
        let contents =
            fs::read_to_string(containers.join(file)).expect("read release containerfile");
        assert!(
            contents.contains(
                "install -D -m 755 /tmp/install/conary \
                     /usr/libexec/conary-test/conary-test-hooks",
            ),
            "{file} must preserve the integration binary beside the published binary"
        );
    }
}

#[test]
fn artix_container_uses_core_mirrors_before_package_sync() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let containerfile =
        manifest_dir.join("../conary/tests/integration/remi/containers/Containerfile.artix");
    let contents = fs::read_to_string(containerfile).expect("read Artix containerfile");

    let mirror1 = contents
        .find("Server = https://mirror1.artixlinux.org/repos/$repo/os/$arch")
        .expect("Artix container must select the first official core mirror");
    let cvut = contents
        .find("Server = https://ftp.sh.cvut.cz/artix-linux/$repo/os/$arch")
        .expect("Artix container must select the second official core mirror");
    let sync = contents
        .find("pacman -Syyu --noconfirm")
        .expect("Artix container must force-refresh package databases before upgrading");

    assert!(
        mirror1 < sync && cvut < sync,
        "Artix core mirrors must be configured before package synchronization"
    );
    assert!(
        !contents.contains("--overwrite"),
        "the image must resolve repository coherence instead of masking file conflicts"
    );
}

#[test]
fn arch_container_uses_pinned_image_archive_before_package_sync() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let containerfile =
        manifest_dir.join("../conary/tests/integration/remi/containers/Containerfile.arch");
    let contents = fs::read_to_string(containerfile).expect("read Arch containerfile");

    let archive = contents
        .find("Server = https://archive.archlinux.org/repos/2026/08/02/$repo/os/$arch")
        .expect("Arch container must select the pinned image's official archive snapshot");
    let sync = contents
        .find("pacman -Syyu --noconfirm")
        .expect("Arch container must force-refresh archive package databases");

    assert!(
        archive < sync,
        "Arch archive authority must be configured before package synchronization"
    );
    assert!(
        contents.contains("version 20260802.0.566770"),
        "Arch archive date must remain visibly coupled to pinned image provenance"
    );
    assert!(
        contents.contains("pacman -Syyu --noconfirm --disable-download-timeout"),
        "Arch package synchronization must tolerate slow archive downloads"
    );
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
        (
            "Containerfile.debian-derivative",
            "conary-release.deb",
            "apt-get install -y /tmp/install/conary-release.deb",
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
