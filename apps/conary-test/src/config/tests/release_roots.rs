// conary-test/src/config/tests/release_roots.rs

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::config::{GlobalConfig, ReleaseMediaAuthority};

fn integration_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../conary/tests/integration/remi")
}

fn shipped_config() -> GlobalConfig {
    let path = integration_root().join("config.toml");
    let source = std::fs::read_to_string(&path).expect("read integration config");
    toml::from_str(&source).expect("parse integration config")
}

fn assert_checksum_manifest(fixture_root: &Path) {
    let manifest_path = fixture_root.join("apt.sha256");
    let manifest = std::fs::read_to_string(&manifest_path).expect("read APT checksum manifest");
    for line in manifest.lines() {
        let (expected, relative) = line
            .split_once("  ")
            .unwrap_or_else(|| panic!("malformed line in {}: {line}", manifest_path.display()));
        let path = fixture_root.join("apt").join(relative);
        let bytes =
            std::fs::read(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        assert_eq!(
            hex::encode(Sha256::digest(bytes)),
            expected,
            "{} must match its checked-in release-media digest",
            path.display()
        );
    }
}

#[test]
fn derivative_roots_carry_typed_release_and_package_authority() {
    let config = shipped_config();
    for distro in ["linux-mint-22.3", "pop-os-24.04"] {
        let root = config.distros[distro]
            .release_root
            .as_ref()
            .unwrap_or_else(|| panic!("{distro} must declare release-root authority"));
        root.validate()
            .unwrap_or_else(|error| panic!("{distro} release root is invalid: {error}"));
        assert!(root.base_image.contains("ubuntu:24.04@sha256:"));
        assert_eq!(root.fixture, distro);
        assert_eq!(root.keyring_package.sha256.len(), 64);
        assert_eq!(root.identity_package.sha256.len(), 64);
        assert_eq!(
            root.expected_os_release.ubuntu_codename.as_deref(),
            Some("noble")
        );
        assert!(root.signing_roots.iter().all(|item| {
            item.fingerprints
                .iter()
                .all(|fingerprint| fingerprint.len() == 40)
        }));
        assert_checksum_manifest(
            &integration_root()
                .join("../../fixtures/distro-roots")
                .join(distro),
        );
    }

    let mint = config.distros["linux-mint-22.3"]
        .release_root
        .as_ref()
        .unwrap();
    assert_eq!(mint.expected_os_release.id, "linuxmint");
    assert_eq!(mint.expected_os_release.version_id, "22.3");
    assert_eq!(mint.apt_global_trust.len(), 2);
    assert!(matches!(
        &mint.release_media,
        ReleaseMediaAuthority::OpenPgp { signing_fingerprint, .. }
            if signing_fingerprint == "27DEB15644C6B3CF3BD7D291300F846BA25BAE09"
    ));

    let pop = config.distros["pop-os-24.04"]
        .release_root
        .as_ref()
        .unwrap();
    assert_eq!(pop.expected_os_release.id, "pop");
    assert_eq!(pop.expected_os_release.version_id, "24.04");
    assert!(pop.apt_global_trust.is_empty());
    assert!(matches!(
        &pop.release_media,
        ReleaseMediaAuthority::HttpsMetadata { metadata_url, .. }
            if metadata_url == "https://api.pop-os.org/builds/24.04/generic?arch=amd64"
    ));
}

#[test]
fn rolling_roots_carry_authenticated_native_authority() {
    let config = shipped_config();
    for distro in ["cachyos", "opensuse-tumbleweed"] {
        let root = config.distros[distro]
            .target_root
            .as_ref()
            .unwrap_or_else(|| panic!("{distro} must declare target-root authority"));
        root.validate()
            .unwrap_or_else(|error| panic!("{distro} target root is invalid: {error}"));
        assert!(root.base_image.contains("@sha256:"));
        assert!(root.identity_packages.len() >= 4);
        assert!(!root.repository_declarations.is_empty());
        assert!(!root.signing_roots.is_empty());
        assert_eq!(
            root.repository_trust.len(),
            if distro == "cachyos" { 4 } else { 6 }
        );
    }
    assert_eq!(
        config.distros["cachyos"]
            .target_root
            .as_ref()
            .unwrap()
            .expected_os_release
            .id_like
            .as_deref(),
        Some("arch")
    );
    let tumbleweed = config.distros["opensuse-tumbleweed"]
        .target_root
        .as_ref()
        .unwrap();
    assert_eq!(tumbleweed.expected_os_release.id, "opensuse-tumbleweed");
    assert!(tumbleweed.base_image.contains(&format!(
        ":{}@sha256:",
        tumbleweed.expected_os_release.version_id
    )));
}

#[test]
fn rolling_takeover_helper_uses_typed_configuration_not_distro_names() {
    let path =
        integration_root().join("../../fixtures/distro-roots/run-native-repository-takeover.py");
    let source = std::fs::read_to_string(&path).expect("read native takeover helper");
    for forbidden in ["cachyos", "tumbleweed", "opensuse"] {
        assert!(
            !source.to_ascii_lowercase().contains(forbidden),
            "{} must not select product behavior from {forbidden}",
            path.display()
        );
    }
    for required in [
        "preview_sha256",
        "repository_trust",
        "repository-takeover",
        "projection_sha256",
        "packager_key_threshold",
    ] {
        assert!(source.contains(required), "helper must require {required}");
    }
}

#[test]
fn derivative_takeover_helper_is_data_driven_and_typed() {
    let path =
        integration_root().join("../../fixtures/distro-roots/run-apt-repository-takeover.py");
    let source = std::fs::read_to_string(&path).expect("read derivative takeover helper");
    for forbidden in ["linuxmint", "linux-mint", "pop-os", "pop!_os", "zena"] {
        assert!(
            !source.to_ascii_lowercase().contains(forbidden),
            "{} must not select product behavior from {forbidden}",
            path.display()
        );
    }
    for required in [
        "preview_sha256",
        "implicit-global-authority",
        "apt_global_trust",
        "repository-takeover",
        "projection_sha256",
    ] {
        assert!(source.contains(required), "helper must require {required}");
    }
}

#[test]
fn shared_derivative_containerfile_has_no_distro_name_branch() {
    let path = integration_root().join("containers/Containerfile.debian-derivative");
    let source = std::fs::read_to_string(&path).expect("read derivative Containerfile");
    for forbidden in ["linuxmint", "linux-mint", "pop-os", "pop!_os", "zena"] {
        assert!(
            !source.to_ascii_lowercase().contains(forbidden),
            "{} must not branch on {forbidden}",
            path.display()
        );
    }
    for required in [
        "ARG BASE_IMAGE",
        "ARG DERIVATIVE_ROOT",
        "KEYRING_PACKAGE_SHA256",
        "IDENTITY_PACKAGE_SHA256",
        "61159013263411a7c7a4157a6ddcd4eca3329ee698e508850070af11338098db  /etc/dpkg/dpkg.cfg.d/excludes",
        "rm /etc/dpkg/dpkg.cfg.d/excludes",
        "test ! -e /etc/dpkg/dpkg.cfg.d/excludes",
        "apt-get install -y --allow-downgrades --no-install-recommends /tmp/derivative-identity.deb",
        "EXPECTED_OS_ID",
        "REQUIRED_APT_URIS",
        "sha256sum --check --strict /tmp/derivative-root/apt.sha256",
    ] {
        assert!(
            source.contains(required),
            "Containerfile must require {required}"
        );
    }
}

#[test]
fn release_root_rejects_unpinned_or_shell_shaped_authority() {
    let config = shipped_config();
    let mut root = config.distros["linux-mint-22.3"]
        .release_root
        .clone()
        .unwrap();

    root.base_image = "docker.io/library/ubuntu:24.04".into();
    assert!(
        root.validate()
            .unwrap_err()
            .to_string()
            .contains("digest-pinned")
    );

    root = config.distros["linux-mint-22.3"]
        .release_root
        .clone()
        .unwrap();
    root.base_image = "docker.io/library/ubuntu:24.04@sha256:short".into();
    assert!(
        root.validate()
            .unwrap_err()
            .to_string()
            .contains("64-digit")
    );

    root = config.distros["linux-mint-22.3"]
        .release_root
        .clone()
        .unwrap();
    root.identity_package.url = "https://example.test/pkg.deb;touch bad".into();
    assert!(
        root.validate()
            .unwrap_err()
            .to_string()
            .contains("shell-unsafe")
    );
}
