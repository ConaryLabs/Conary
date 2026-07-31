// conary-core/tests/supported_host_generation_export_fixture_contract.rs

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use conary_core::ccs::CcsManifest;
use conary_core::model::parse_model_file;

const FIXTURE_REL: &str = "apps/conary/tests/fixtures/supported-host-generation-export";

const MANIFEST_RELS: &[&str] = &[
    "apps/conary/tests/integration/remi/manifests/phase3-group-o-generation-export.toml",
    "apps/conary/tests/integration/remi/manifests/phase3-group-p-iso-export.toml",
];

/// Entries the assembled root cannot boot without, each for a reason that is
/// not visible from the package name. Losing one of these turns a several-hour
/// QEMU suite red at the last step, or worse, produces an artifact that only
/// fails at generation mount time.
const LOAD_BEARING_MODEL_ENTRIES: &[&str] = &[
    "filesystem",
    "systemd",
    "systemd-boot-unsigned",
    "kernel-core",
    "kernel-modules-core",
    "composefs",
    "openssh-server",
    "conary-qemu-test-access",
];

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|dir| dir.join(FIXTURE_REL).join("fixture-system.toml").is_file())
        .expect("workspace root not found from crate manifest ancestors")
}

fn fixture_dir() -> PathBuf {
    workspace_root().join(FIXTURE_REL)
}

#[test]
fn fixture_model_parses_and_keeps_the_load_bearing_entries() {
    let model_path = fixture_dir().join("fixture-system.toml");
    let model = parse_model_file(&model_path)
        .unwrap_or_else(|err| panic!("failed to parse {}: {err}", model_path.display()));

    let installed: BTreeSet<&str> = model.config.install.iter().map(String::as_str).collect();
    for entry in LOAD_BEARING_MODEL_ENTRIES {
        assert!(
            installed.contains(entry),
            "supported-host fixture model must install {entry}; got {installed:?}"
        );
    }
}

#[test]
fn access_package_manifest_names_the_package_the_model_expects() {
    let manifest_path = fixture_dir()
        .join("conary-qemu-test-access")
        .join("ccs.toml");
    let manifest = CcsManifest::from_file(&manifest_path)
        .unwrap_or_else(|err| panic!("failed to parse {}: {err}", manifest_path.display()));

    assert_eq!(
        manifest.package.name, "conary-qemu-test-access",
        "the model lists the access package by name, so the CCS manifest name is the contract"
    );
    assert_eq!(
        manifest
            .package
            .platform
            .as_ref()
            .and_then(|platform| platform.arch.as_deref()),
        Some("noarch"),
        "the access-only fixture is architecture-independent and must carry exact noarch authority"
    );

    let model = parse_model_file(&fixture_dir().join("fixture-system.toml")).expect("model parses");
    assert!(
        model
            .config
            .install
            .iter()
            .any(|p| p == "conary-qemu-test-access"),
        "the built access package name must appear in the model"
    );
}

#[test]
fn access_package_stage_carries_the_harness_key_placeholder() {
    let key_file = fixture_dir().join("conary-qemu-test-access/stage/etc/ssh/authorized_keys/root");
    let contents = std::fs::read_to_string(&key_file)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", key_file.display()));
    assert!(
        contents.contains("__CONARY_QEMU_TEST_PUBLIC_KEY__"),
        "the QEMU suites substitute the harness public key into {}; without the placeholder the \
         substitution silently does nothing and the exported image is unreachable over SSH",
        key_file.display()
    );

    let sshd_drop_in = fixture_dir().join(
        "conary-qemu-test-access/stage/etc/ssh/sshd_config.d/00-conary-qemu-test-access.conf",
    );
    let sshd = std::fs::read_to_string(&sshd_drop_in)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", sshd_drop_in.display()));
    assert!(
        sshd.contains("/etc/ssh/authorized_keys/%u"),
        "/root is outside the immutable generation contract, so the authorized key must be read \
         from generation-owned /etc"
    );
}

#[test]
fn stage_has_no_checked_in_symlinks() {
    let stage = fixture_dir().join("conary-qemu-test-access/stage");
    let mut pending = vec![stage.clone()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).expect("stage directory is readable") {
            let entry = entry.expect("stage directory entry is readable");
            let file_type = entry.file_type().expect("stage entry type is readable");
            assert!(
                !file_type.is_symlink(),
                "{} is a checked-in symlink; the QEMU harness copies fixtures with `scp -r`, which \
                 materializes symlinks as copies of their targets. Stage links belong in \
                 stage-links.sh so they arrive in the guest as links.",
                entry.path().display()
            );
            if file_type.is_dir() {
                pending.push(entry.path());
            }
        }
    }
}

#[test]
fn stage_links_script_enables_the_units_the_boot_proof_needs() {
    let script = fixture_dir().join("conary-qemu-test-access/stage-links.sh");
    let staged = tempfile::tempdir().expect("temp stage directory");

    let status = Command::new("sh")
        .arg(&script)
        .arg(staged.path())
        .status()
        .unwrap_or_else(|err| panic!("failed to run {}: {err}", script.display()));
    assert!(status.success(), "{} exited {status}", script.display());

    for (link, target) in [
        (
            "etc/systemd/system/multi-user.target.wants/sshd.service",
            "/usr/lib/systemd/system/sshd.service",
        ),
        (
            "etc/systemd/system/multi-user.target.wants/systemd-networkd.service",
            "/usr/lib/systemd/system/systemd-networkd.service",
        ),
        (
            "etc/systemd/system/multi-user.target.wants/systemd-resolved.service",
            "/usr/lib/systemd/system/systemd-resolved.service",
        ),
    ] {
        let path = staged.path().join(link);
        let resolved = std::fs::read_link(&path)
            .unwrap_or_else(|err| panic!("{} is not a symlink: {err}", path.display()));
        assert_eq!(
            resolved,
            Path::new(target),
            "{link} must be enabled by an explicit package-owned symlink, not by a preset"
        );
    }
}

#[test]
fn publication_convergence_is_idempotent_and_requires_exact_selected_state() {
    let script = fixture_dir().join("publish-selected-generation.sh");
    let contents = std::fs::read_to_string(&script)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", script.display()));

    for required in [
        "conary system generation publish",
        "last_error FROM generation_publications",
        "conary system generation pending",
        "No pending generation publication debt.",
        "readlink -f \"$runtime_root/current\"",
        "\"$runtime_root/generations/$generation\"",
    ] {
        assert!(
            contents.contains(required),
            "{} must prove publication convergence with {required:?}",
            script.display()
        );
    }

    let status = Command::new("sh")
        .arg("-n")
        .arg(&script)
        .status()
        .unwrap_or_else(|err| panic!("failed to parse {}: {err}", script.display()));
    assert!(
        status.success(),
        "{} failed shell parsing",
        script.display()
    );
}

#[cfg(unix)]
#[test]
fn publication_convergence_accepts_current_and_rejects_debt_or_escape() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let temp = tempfile::tempdir().expect("temporary publication fixture");
    let runtime_root = temp.path().join("runtime");
    let generation = runtime_root.join("generations/7");
    std::fs::create_dir_all(generation.join("boot-assets")).expect("generation directories");
    for marker in [
        ".conary-artifact.json",
        "cas-manifest.json",
        "boot-assets/manifest.json",
    ] {
        std::fs::write(generation.join(marker), b"{}\n").expect("generation marker");
    }
    std::fs::write(runtime_root.join("conary.db"), b"").expect("fixture database");
    symlink("generations/7", runtime_root.join("current")).expect("selected generation link");

    let fake_bin = temp.path().join("bin");
    std::fs::create_dir(&fake_bin).expect("fake command directory");
    let fake_conary = fake_bin.join("conary");
    std::fs::write(
        &fake_conary,
        r#"#!/bin/sh
if [ "$1" = "system" ] && [ "$2" = "generation" ]; then
    case "$3" in
        publish)
            echo "Generation publication is already current."
            exit 0
            ;;
        pending)
            printf '%s\n' "${FAKE_PENDING_OUTPUT:-No pending generation publication debt.}"
            exit 0
            ;;
    esac
fi
exit 64
"#,
    )
    .expect("fake conary command");
    let mut permissions = std::fs::metadata(&fake_conary)
        .expect("fake command metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&fake_conary, permissions).expect("fake command permissions");

    let script = fixture_dir().join("publish-selected-generation.sh");
    let db_path = runtime_root.join("conary.db");
    let selected_output = runtime_root.join("selected-generation.txt");
    let path = format!(
        "{}:{}",
        fake_bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let run = |pending_output: &str| {
        Command::new("sh")
            .arg(&script)
            .arg(&db_path)
            .arg(&selected_output)
            .env("PATH", &path)
            .env("FAKE_PENDING_OUTPUT", pending_output)
            .output()
            .unwrap_or_else(|err| panic!("failed to run {}: {err}", script.display()))
    };

    let current = run("No pending generation publication debt.");
    assert!(
        current.status.success(),
        "already-current publication with no debt failed: stdout={} stderr={}",
        String::from_utf8_lossy(&current.stdout),
        String::from_utf8_lossy(&current.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(&selected_output).expect("selected generation output"),
        "7\n"
    );

    let pending = run("Pending generation publication debt:");
    assert!(
        !pending.status.success(),
        "publication convergence accepted outstanding debt"
    );

    std::fs::remove_file(runtime_root.join("current")).expect("remove selected link");
    let outside = temp.path().join("outside/7");
    std::fs::create_dir_all(&outside).expect("outside generation");
    symlink(&outside, runtime_root.join("current")).expect("escaped selected link");
    let escaped = run("No pending generation publication debt.");
    assert!(
        !escaped.status.success(),
        "publication convergence accepted a selected generation outside the runtime root"
    );
}

#[test]
fn export_suites_consume_this_fixture_and_no_bootstrap_fixture() {
    let convergence_script =
        "/opt/remi-tests/fixtures/supported-host-generation-export/publish-selected-generation.sh";
    for rel in MANIFEST_RELS {
        let path = workspace_root().join(rel);
        let contents = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
        assert!(
            contents.contains(FIXTURE_REL),
            "{rel} must stage {FIXTURE_REL} into the guest"
        );
        assert!(
            !contents.contains("bootstrap-generation-export"),
            "{rel} still references the deleted bootstrap generation-export fixture"
        );
        assert!(
            contents.contains("mkfs.ext4 -F -O verity /dev/disk/by-id/virtio-conary-scratch"),
            "{rel} must enable ext4 fs-verity on the selected generation root; otherwise \
             publication correctly refuses to claim truthful composefs protection"
        );
        assert!(
            contents.contains(convergence_script),
            "{rel} must use the shared idempotent publication-convergence contract"
        );
    }
}
