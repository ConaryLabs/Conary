// src/commands/test_helpers.rs

//! Shared test fixtures for command handler tests.
//!
//! Both `model.rs` and `update.rs` use identical database setup and
//! replatform seeding logic. This module provides those fixtures once
//! so each test module can import them with `use super::test_helpers::*`.

use conary_core::db::models::{
    InstallSource, LabelEntry, PackageResolution, PrimaryStrategy, Repository, RepositoryPackage,
    ResolutionStrategy, Trove, TroveType,
};
use conary_core::db::schema;
use tempfile::NamedTempFile;

pub(crate) fn create_test_db() -> (NamedTempFile, String) {
    let temp_file = NamedTempFile::new().unwrap();
    let db_path = temp_file.path().display().to_string();
    let conn = rusqlite::Connection::open(temp_file.path()).unwrap();
    conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
    schema::migrate(&conn).unwrap();
    drop(conn);
    (temp_file, db_path)
}

pub(crate) fn seed_mixed_replatform_fixture(conn: &rusqlite::Connection) {
    let mut fedora_repo = Repository::new(
        "fedora".to_string(),
        "https://example.test/fedora".to_string(),
    );
    fedora_repo.default_strategy_distro = Some("fedora-43".to_string());
    let fedora_repo_id = fedora_repo.insert(conn).unwrap();

    let mut arch_repo = Repository::new(
        "arch-core".to_string(),
        "https://example.test/arch".to_string(),
    );
    arch_repo.default_strategy = Some("legacy".to_string());
    arch_repo.default_strategy_distro = Some("arch".to_string());
    let arch_repo_id = arch_repo.insert(conn).unwrap();

    let mut fedora_label = LabelEntry::new(
        "fedora".to_string(),
        "f43".to_string(),
        "stable".to_string(),
    );
    fedora_label.insert(conn).unwrap();
    fedora_label
        .set_repository(conn, Some(fedora_repo_id))
        .unwrap();

    for (name, version) in [("vim", "9.0.1"), ("bash", "5.1.0"), ("zsh", "5.8.0")] {
        let mut trove = Trove::new_with_source(
            name.to_string(),
            version.to_string(),
            TroveType::Package,
            InstallSource::Repository,
        );
        trove.architecture = Some("x86_64".to_string());
        trove.label_id = fedora_label.id;
        trove.insert(conn).unwrap();
    }

    for (name, version) in [("vim", "9.1.0"), ("bash", "5.2.0"), ("zsh", "5.9.1")] {
        let mut pkg = RepositoryPackage::new(
            arch_repo_id,
            name.to_string(),
            version.to_string(),
            format!("sha256:{name}"),
            123,
            format!("https://example.test/arch/{name}.pkg.tar.zst"),
        );
        pkg.architecture = Some("x86_64".to_string());
        pkg.insert(conn).unwrap();
    }

    let mut exact_resolution = PackageResolution::new(
        arch_repo_id,
        "vim".to_string(),
        vec![ResolutionStrategy::Binary {
            url: "https://example.test/arch/vim-9.1.0.ccs".to_string(),
            checksum: "sha256:vim-exact".to_string(),
            delta_base: None,
        }],
    );
    exact_resolution.primary_strategy = PrimaryStrategy::Binary;
    exact_resolution.version = Some("9.1.0".to_string());
    exact_resolution.insert(conn).unwrap();

    let mut any_version_resolution = PackageResolution::new(
        arch_repo_id,
        "bash".to_string(),
        vec![ResolutionStrategy::Binary {
            url: "https://example.test/arch/bash-latest.ccs".to_string(),
            checksum: "sha256:bash-any".to_string(),
            delta_base: None,
        }],
    );
    any_version_resolution.primary_strategy = PrimaryStrategy::Binary;
    any_version_resolution.insert(conn).unwrap();
}
