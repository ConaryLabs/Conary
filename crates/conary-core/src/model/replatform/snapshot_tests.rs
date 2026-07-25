// conary-core/src/model/replatform/snapshot_tests.rs

use super::*;

#[test]
fn test_visible_realignment_candidates_counts_same_name_target_impls() {
    let (_temp, conn) = create_test_db();

    let mut fedora_repo = Repository::new(
        "fedora".to_string(),
        "https://example.test/fedora".to_string(),
    );
    fedora_repo.default_strategy_distro = Some("fedora-44".to_string());
    let fedora_repo_id = fedora_repo.insert(&conn).unwrap();

    let mut arch_repo =
        Repository::new("arch".to_string(), "https://example.test/arch".to_string());
    arch_repo.default_strategy_distro = Some("arch".to_string());
    let arch_repo_id = arch_repo.insert(&conn).unwrap();

    let mut fedora_label = LabelEntry::new(
        "fedora".to_string(),
        "f43".to_string(),
        "stable".to_string(),
    );
    fedora_label.insert(&conn).unwrap();
    fedora_label
        .set_repository(&conn, Some(fedora_repo_id))
        .unwrap();

    let mut trove = Trove::new_with_source(
        "vim".to_string(),
        "1.0".to_string(),
        TroveType::Package,
        InstallSource::Repository,
        crate::repository::versioning::VersionScheme::Conary,
    );
    trove.architecture = Some("x86_64".to_string());
    trove.label_id = fedora_label.id;
    trove.insert(&conn).unwrap();

    let mut arch_pkg = RepositoryPackage::new(
        arch_repo_id,
        "vim".to_string(),
        "2.0".to_string(),
        crate::repository::versioning::VersionScheme::Arch,
        "sha256:test".to_string(),
        123,
        "https://example.test/arch/vim.pkg.tar.zst".to_string(),
    );
    arch_pkg.architecture = Some("x86_64".to_string());
    arch_pkg.insert(&conn).unwrap();

    let summary = visible_realignment_candidates(&conn, "arch").unwrap();
    assert_eq!(summary.target_distro, "arch");
    assert_eq!(summary.candidate_count, 1);
}

#[test]
fn test_replatform_estimate_from_affinities_uses_target_counts() {
    let affinities = vec![
        SystemAffinity {
            distro: "fedora-44".to_string(),
            package_count: 9,
            percentage: 75.0,
        },
        SystemAffinity {
            distro: "arch".to_string(),
            package_count: 3,
            percentage: 25.0,
        },
    ];

    let estimate = replatform_estimate_from_affinities(&affinities, "arch")
        .expect("expected affinity-based estimate");

    assert_eq!(estimate.target_distro, "arch");
    assert_eq!(estimate.aligned_packages, 3);
    assert_eq!(estimate.packages_to_realign, 9);
    assert_eq!(estimate.total_packages, 12);
}

#[test]
fn test_source_policy_replatform_snapshot_combines_estimate_and_candidates() {
    let (_temp, conn) = create_test_db();

    let mut fedora_repo = Repository::new(
        "fedora".to_string(),
        "https://example.test/fedora".to_string(),
    );
    fedora_repo.default_strategy_distro = Some("fedora-44".to_string());
    let fedora_repo_id = fedora_repo.insert(&conn).unwrap();

    let mut arch_repo =
        Repository::new("arch".to_string(), "https://example.test/arch".to_string());
    arch_repo.default_strategy_distro = Some("arch".to_string());
    let arch_repo_id = arch_repo.insert(&conn).unwrap();

    let mut fedora_label = LabelEntry::new(
        "fedora".to_string(),
        "f43".to_string(),
        "stable".to_string(),
    );
    fedora_label.insert(&conn).unwrap();
    fedora_label
        .set_repository(&conn, Some(fedora_repo_id))
        .unwrap();

    let mut trove = Trove::new_with_source(
        "vim".to_string(),
        "1.0".to_string(),
        TroveType::Package,
        InstallSource::Repository,
        crate::repository::versioning::VersionScheme::Conary,
    );
    trove.architecture = Some("x86_64".to_string());
    trove.label_id = fedora_label.id;
    trove.insert(&conn).unwrap();

    let mut arch_pkg = RepositoryPackage::new(
        arch_repo_id,
        "vim".to_string(),
        "2.0".to_string(),
        crate::repository::versioning::VersionScheme::Arch,
        "sha256:test".to_string(),
        123,
        "https://example.test/arch/vim.pkg.tar.zst".to_string(),
    );
    arch_pkg.architecture = Some("x86_64".to_string());
    arch_pkg.insert(&conn).unwrap();

    conn.execute(
        "INSERT INTO system_affinity (distro, package_count, percentage, updated_at)
             VALUES (?1, ?2, ?3, datetime('now'))",
        ("fedora-44", 1_i64, 100.0_f64),
    )
    .unwrap();

    let snapshot = source_policy_replatform_snapshot(&conn, "arch").unwrap();

    assert_eq!(snapshot.target_distro, "arch");
    assert_eq!(snapshot.visible_realignment_candidates, 1);
    assert_eq!(snapshot.visible_realignment_proposals.len(), 1);
    assert_eq!(snapshot.visible_realignment_proposals[0].package, "vim");
    assert_eq!(
        snapshot.visible_realignment_proposals[0]
            .current_distro
            .as_deref(),
        Some("fedora-44")
    );
    assert_eq!(
        snapshot.visible_realignment_proposals[0].target_distro,
        "arch"
    );
    assert_eq!(
        snapshot.visible_realignment_proposals[0].target_version,
        "2.0"
    );
    assert_eq!(
        snapshot.visible_realignment_proposals[0]
            .architecture
            .as_deref(),
        Some("x86_64")
    );
    let estimate = snapshot.estimate.expect("expected estimate");
    assert_eq!(estimate.aligned_packages, 0);
    assert_eq!(estimate.packages_to_realign, 1);
    assert_eq!(estimate.total_packages, 1);
}

#[test]
fn test_source_policy_replatform_snapshot_uses_native_repo_version_ordering() {
    let (_temp, conn) = create_test_db();

    let mut fedora_repo = Repository::new(
        "fedora".to_string(),
        "https://example.test/fedora".to_string(),
    );
    fedora_repo.default_strategy_distro = Some("fedora-44".to_string());
    let fedora_repo_id = fedora_repo.insert(&conn).unwrap();

    let mut fedora_label = LabelEntry::new(
        "fedora".to_string(),
        "f43".to_string(),
        "stable".to_string(),
    );
    fedora_label.repository_id = Some(fedora_repo_id);
    let fedora_label_id = fedora_label.insert(&conn).unwrap();

    let mut installed = Trove::new_with_source(
        "demo".to_string(),
        "0.9".to_string(),
        TroveType::Package,
        InstallSource::Repository,
        crate::repository::versioning::VersionScheme::Conary,
    );
    installed.label_id = Some(fedora_label_id);
    installed.architecture = Some("amd64".to_string());
    installed.insert(&conn).unwrap();

    let mut ubuntu_repo = Repository::new(
        "ubuntu-noble".to_string(),
        "https://archive.ubuntu.com/ubuntu".to_string(),
    );
    ubuntu_repo.default_strategy_distro = Some("ubuntu-26.04".to_string());
    let ubuntu_repo_id = ubuntu_repo.insert(&conn).unwrap();

    let mut prerelease = RepositoryPackage::new(
        ubuntu_repo_id,
        "demo".to_string(),
        "1.0~beta1".to_string(),
        crate::repository::versioning::VersionScheme::Debian,
        "sha256:beta".to_string(),
        123,
        "https://archive.ubuntu.com/ubuntu/pool/demo_1.0~beta1_amd64.deb".to_string(),
    );
    prerelease.architecture = Some("amd64".to_string());
    prerelease.insert(&conn).unwrap();

    let mut stable = RepositoryPackage::new(
        ubuntu_repo_id,
        "demo".to_string(),
        "1.0".to_string(),
        crate::repository::versioning::VersionScheme::Debian,
        "sha256:stable".to_string(),
        123,
        "https://archive.ubuntu.com/ubuntu/pool/demo_1.0_amd64.deb".to_string(),
    );
    stable.architecture = Some("amd64".to_string());
    stable.insert(&conn).unwrap();

    let snapshot = source_policy_replatform_snapshot(&conn, "ubuntu-26.04").unwrap();

    assert_eq!(snapshot.visible_realignment_candidates, 1);
    assert_eq!(snapshot.visible_realignment_proposals[0].package, "demo");
    assert_eq!(
        snapshot.visible_realignment_proposals[0].target_version,
        "1.0"
    );
}

#[test]
fn test_source_policy_replatform_snapshot_uses_shared_selector_priority_ordering() {
    let (_temp, conn) = create_test_db();

    let mut fedora_repo = Repository::new(
        "fedora".to_string(),
        "https://example.test/fedora".to_string(),
    );
    fedora_repo.default_strategy_distro = Some("fedora-44".to_string());
    let fedora_repo_id = fedora_repo.insert(&conn).unwrap();

    let mut fedora_label = LabelEntry::new(
        "fedora".to_string(),
        "f43".to_string(),
        "stable".to_string(),
    );
    fedora_label.repository_id = Some(fedora_repo_id);
    let fedora_label_id = fedora_label.insert(&conn).unwrap();

    let mut installed = Trove::new_with_source(
        "demo".to_string(),
        "0.9".to_string(),
        TroveType::Package,
        InstallSource::Repository,
        crate::repository::versioning::VersionScheme::Conary,
    );
    installed.label_id = Some(fedora_label_id);
    installed.architecture = Some("x86_64".to_string());
    installed.insert(&conn).unwrap();

    let mut arch_priority_repo = Repository::new(
        "arch-priority".to_string(),
        "https://example.test/arch-priority".to_string(),
    );
    arch_priority_repo.default_strategy_distro = Some("arch".to_string());
    arch_priority_repo.priority = 100;
    let arch_priority_repo_id = arch_priority_repo.insert(&conn).unwrap();

    let mut arch_latest_repo = Repository::new(
        "arch-latest".to_string(),
        "https://example.test/arch-latest".to_string(),
    );
    arch_latest_repo.default_strategy_distro = Some("arch".to_string());
    arch_latest_repo.priority = 10;
    let arch_latest_repo_id = arch_latest_repo.insert(&conn).unwrap();

    let mut priority_pkg = RepositoryPackage::new(
        arch_priority_repo_id,
        "demo".to_string(),
        "1.0".to_string(),
        crate::repository::versioning::VersionScheme::Arch,
        "sha256:priority".to_string(),
        111,
        "https://example.test/arch-priority/demo-1.0.pkg.tar.zst".to_string(),
    );
    priority_pkg.architecture = Some("x86_64".to_string());
    priority_pkg.insert(&conn).unwrap();

    let mut latest_pkg = RepositoryPackage::new(
        arch_latest_repo_id,
        "demo".to_string(),
        "2.0".to_string(),
        crate::repository::versioning::VersionScheme::Arch,
        "sha256:latest".to_string(),
        222,
        "https://example.test/arch-latest/demo-2.0.pkg.tar.zst".to_string(),
    );
    latest_pkg.architecture = Some("x86_64".to_string());
    latest_pkg.insert(&conn).unwrap();

    let snapshot = source_policy_replatform_snapshot(&conn, "arch").unwrap();

    assert_eq!(snapshot.visible_realignment_candidates, 1);
    assert_eq!(
        snapshot.visible_realignment_proposals[0]
            .target_repository
            .as_deref(),
        Some("arch-priority")
    );
    assert_eq!(
        snapshot.visible_realignment_proposals[0].target_version,
        "1.0"
    );
}
