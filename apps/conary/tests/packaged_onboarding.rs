// apps/conary/tests/packaged_onboarding.rs

use std::fs;
use std::path::{Path, PathBuf};

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

fn read_packaging_file(path: &str) -> String {
    fs::read_to_string(repository_root().join(path))
        .unwrap_or_else(|error| panic!("read {path}: {error}"))
}

fn assert_exact_root_postinstall(script: &str, expected_command: &str) {
    let init_lines = script
        .lines()
        .filter(|line| line.contains("system init"))
        .collect::<Vec<_>>();
    assert!(
        !init_lines.is_empty(),
        "packaging script must initialize Conary"
    );
    for line in init_lines {
        assert!(
            line.contains(expected_command),
            "unexpected init command: {line}"
        );
        assert!(
            !line.contains("sudo"),
            "package script already runs as root"
        );
        assert!(
            !line.contains("2>/dev/null") && !line.contains("|| true") && !line.contains("|| :"),
            "postinstall must not hide initialization failure: {line}"
        );
    }
}

#[test]
fn native_packages_initialize_with_their_exact_public_profile() {
    assert_exact_root_postinstall(
        &read_packaging_file("packaging/rpm/conary.spec"),
        "system init --profile fedora-44",
    );
    assert_exact_root_postinstall(
        &read_packaging_file("packaging/deb/debian/postinst"),
        "system init --profile ubuntu-26.04",
    );
    assert_exact_root_postinstall(
        &read_packaging_file("packaging/arch/conary.install"),
        "system init --profile arch",
    );
}
