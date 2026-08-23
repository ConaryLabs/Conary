// crates/conary-core/src/test_support.rs

#[cfg(unix)]
use std::path::{Path, PathBuf};

#[cfg(unix)]
#[derive(Debug, Clone, Copy)]
pub(crate) enum HostToolFixture {
    Depmod,
    Dracut,
    ExitFailure,
    ExitSuccess,
    Ldconfig,
    OpenRc,
    SshKeygen,
    Sysctl406,
    Sysctl407,
    Systemd,
    Timeout,
}

#[cfg(unix)]
impl HostToolFixture {
    fn filename(self) -> &'static str {
        match self {
            Self::Depmod => "depmod",
            Self::Dracut => "dracut",
            Self::ExitFailure => "exit-failure",
            Self::ExitSuccess => "exit-success",
            Self::Ldconfig => "ldconfig",
            Self::OpenRc => "openrc",
            Self::SshKeygen => "ssh-keygen",
            Self::Sysctl406 => "sysctl-4.0.6",
            Self::Sysctl407 => "sysctl-4.0.7",
            Self::Systemd => "systemd",
            Self::Timeout => "timeout",
        }
    }
}

#[cfg(unix)]
fn host_tool_source(fixture: HostToolFixture) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/host-tools")
        .join(fixture.filename())
}

/// Link a test-owned command path to a stable checked-in executable.
///
/// Tests may create per-case symlinks and data files, but they must not write
/// the executable inode immediately before production code launches it.
#[cfg(unix)]
pub(crate) fn link_host_tool(path: &Path, fixture: HostToolFixture) {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let source = host_tool_source(fixture);
    let metadata = source
        .metadata()
        .unwrap_or_else(|error| panic!("stat host-tool fixture {}: {error}", source.display()));
    assert!(
        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0,
        "host-tool fixture must be a checked-in executable file: {}",
        source.display()
    );
    symlink(&source, path).unwrap_or_else(|error| {
        panic!(
            "link host-tool fixture {} at {}: {error}",
            source.display(),
            path.display()
        )
    });
}

#[cfg(unix)]
pub(crate) const SYSTEMCTL_CAPTURE_PATH: &str = "var/lib/conary-test/systemctl-calls";

#[cfg(unix)]
pub(crate) const TMPFILES_CAPTURE_PATH: &str = "var/lib/conary-test/systemd-tmpfiles-captured.conf";
