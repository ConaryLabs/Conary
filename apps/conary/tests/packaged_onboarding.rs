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

fn assert_generation_activation_is_staged(script: &str) {
    assert!(
        script.contains("packaging/systemd/")
            && script.contains("generation-activation.service")
            && script.contains("multi-user.target.wants"),
        "package must install and statically enable the booted-generation activation service"
    );
}

#[test]
fn native_packages_initialize_the_source_independent_repository_set() {
    assert_exact_root_postinstall(
        &read_packaging_file("packaging/rpm/conary.spec"),
        "system init",
    );
    assert_exact_root_postinstall(
        &read_packaging_file("packaging/deb/debian/postinst"),
        "system init",
    );
    assert_exact_root_postinstall(
        &read_packaging_file("packaging/arch/conary.install"),
        "system init",
    );
}

#[test]
fn every_conary_package_format_activates_exact_booted_generation_work() {
    for path in [
        "packaging/rpm/conary.spec",
        "packaging/deb/debian/rules",
        "packaging/arch/PKGBUILD",
        "packaging/ccs/build.sh",
    ] {
        assert_generation_activation_is_staged(&read_packaging_file(path));
    }

    let unit = read_packaging_file("packaging/systemd/conary-generation-activation.service");
    assert!(unit.contains("Type=oneshot"));
    assert!(unit.contains(
        "ExecStart=/usr/bin/conary system generation activate --db-path /var/lib/conary/conary.db"
    ));
    assert!(unit.contains("Restart=on-failure"));
    assert!(unit.contains("WantedBy=multi-user.target"));
}

#[test]
fn fedora_package_declares_and_preflights_systemd_macro_authority() {
    let spec = read_packaging_file("packaging/rpm/conary.spec");
    assert!(
        spec.lines().any(|line| {
            line.split_once(':').is_some_and(|(field, packages)| {
                field.trim() == "BuildRequires"
                    && packages
                        .split_whitespace()
                        .any(|package| package == "systemd-rpm-macros")
            })
        }),
        "RPM spec must declare the package that provides %{{_unitdir}}"
    );

    let containerfile = read_packaging_file("packaging/rpm/Containerfile.build");
    assert!(containerfile.contains("systemd-rpm-macros"));
    assert!(containerfile.contains("rpm --eval '%{_unitdir}'"));
    assert!(containerfile.contains("/usr/lib/systemd/system"));

    let build_script = read_packaging_file("packaging/rpm/build.sh");
    assert!(build_script.contains("rpm --eval '%{_unitdir}'"));
    assert!(build_script.contains("Install the systemd-rpm-macros build dependency"));

    let release_workflow = read_packaging_file(".github/workflows/release-build.yml");
    assert!(
        release_workflow.lines().any(|line| {
            line.contains("dnf install -y")
                && line
                    .split_whitespace()
                    .any(|package| package == "systemd-rpm-macros")
        }),
        "release RPM build must install systemd-rpm-macros"
    );
}
