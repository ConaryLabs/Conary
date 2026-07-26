// conary-core/src/scriptlet/test_support.rs

//! Test-only materialized selected roots and namespace re-execution.

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt as _;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

pub(crate) struct MaterializedTestRoot {
    directory: tempfile::TempDir,
}

impl MaterializedTestRoot {
    pub(crate) fn path(&self) -> &Path {
        self.directory.path()
    }

    pub(crate) fn host_path(&self, guest: &str) -> PathBuf {
        let guest = Path::new(guest);
        assert!(guest.is_absolute(), "test guest path must be absolute");
        assert!(
            guest
                .components()
                .all(|component| !matches!(component, Component::ParentDir)),
            "test guest path must not contain parent traversal"
        );
        self.path()
            .join(guest.strip_prefix("/").expect("absolute guest path"))
    }
}

pub(crate) fn materialized_root(
    test_name: &str,
    executables: &[&str],
) -> Option<MaterializedTestRoot> {
    if !run_selected_root_namespace_test(test_name) {
        return None;
    }

    let directory = tempfile::tempdir().expect("materialized selected root");
    let fixture = MaterializedTestRoot { directory };
    let tmp = fixture.host_path("/tmp");
    fs::create_dir_all(&tmp).expect("selected-root tmp directory");
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o1777))
        .expect("selected-root tmp permissions");
    for executable in executables {
        install_host_executable(fixture.path(), executable);
    }
    Some(fixture)
}

pub(crate) fn run_selected_root_namespace_test(test_name: &str) -> bool {
    const CHILD_MARKER: &str = "CONARY_SELECTED_ROOT_NAMESPACE_TEST_MARKER";

    if nix::unistd::geteuid().is_root() && mount_namespace_available() {
        if let Some(marker) = std::env::var_os(CHILD_MARKER) {
            fs::write(marker, test_name).expect("record selected-root namespace test execution");
        }
        return true;
    }
    assert!(
        std::env::var_os(CHILD_MARKER).is_none(),
        "selected-root namespace test child lacks a usable mount namespace"
    );

    let namespace_args = [
        "--user",
        "--map-root-user",
        "--mount",
        "--propagation",
        "private",
    ];
    let probe = Command::new("unshare")
        .args(namespace_args)
        .arg("/bin/true")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if !probe.is_ok_and(|status| status.success()) {
        return false;
    }

    let marker =
        tempfile::NamedTempFile::new().expect("selected-root namespace test execution marker");
    let status = Command::new("unshare")
        .args(namespace_args)
        .arg(std::env::current_exe().expect("current test executable"))
        .arg(test_name)
        .args(["--exact", "--nocapture"])
        .env(CHILD_MARKER, marker.path())
        .status()
        .expect("run selected-root proof in a user and mount namespace");
    assert!(
        status.success(),
        "selected-root namespace child failed for {test_name}"
    );
    assert_eq!(
        fs::read_to_string(marker.path()).expect("read namespace test execution marker"),
        test_name,
        "selected-root namespace child matched no exact test"
    );
    false
}

fn install_host_executable(root: &Path, executable: &str) {
    let guest = Path::new(executable);
    assert!(guest.is_absolute(), "test executable path must be absolute");
    copy_into_root(guest, root, guest);

    let output = Command::new("ldd")
        .arg(guest)
        .output()
        .unwrap_or_else(|error| panic!("run ldd for {executable}: {error}"));
    assert!(
        output.status.success(),
        "ldd failed for {executable}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let dependencies = String::from_utf8(output.stdout)
        .expect("ldd output is UTF-8")
        .split_whitespace()
        .filter(|field| field.starts_with('/'))
        .map(|field| field.trim_end_matches(':').to_string())
        .collect::<BTreeSet<_>>();
    for dependency in dependencies {
        let path = Path::new(&dependency);
        copy_into_root(path, root, path);
    }
}

fn copy_into_root(source: &Path, root: &Path, destination: &Path) {
    let source = fs::canonicalize(source)
        .unwrap_or_else(|error| panic!("resolve host test runtime {}: {error}", source.display()));
    let destination = root.join(
        destination
            .strip_prefix("/")
            .expect("test runtime destination is absolute"),
    );
    fs::create_dir_all(destination.parent().expect("runtime destination parent"))
        .expect("create selected-root runtime directory");
    fs::copy(&source, &destination).unwrap_or_else(|error| {
        panic!(
            "copy host test runtime {} to {}: {error}",
            source.display(),
            destination.display()
        )
    });
}

fn mount_namespace_available() -> bool {
    let mut command = Command::new("/bin/true");
    // Safety: this child-only probe performs no work after unsharing.
    unsafe {
        command.pre_exec(|| {
            nix::sched::unshare(nix::sched::CloneFlags::CLONE_NEWNS).map_err(std::io::Error::from)
        });
    }
    command.status().is_ok()
}
