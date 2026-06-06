// tests/query.rs

//! Query operation tests: package queries, dependency lookups, provides, changesets.

mod common;

use conary_core::db;
use std::process::{Command, Output};

fn run_conary(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(args)
        .output()
        .expect("failed to run conary")
}

fn output_text(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn test_query_packages() {
    use conary_core::db::models::{Changeset, ChangesetStatus, Trove, TroveType};

    let (_dir, _path, mut conn) = common::create_test_db();

    // Install multiple packages
    for (name, version) in [
        ("nginx", "1.21.0"),
        ("redis", "6.2.0"),
        ("postgres", "14.0"),
    ] {
        db::transaction(&mut conn, |tx| {
            let mut changeset = Changeset::new(format!("Install {}-{}", name, version));
            let changeset_id = changeset.insert(tx)?;

            let mut trove = Trove::new(name.to_string(), version.to_string(), TroveType::Package);
            trove.installed_by_changeset_id = Some(changeset_id);
            trove.insert(tx)?;

            changeset.update_status(tx, ChangesetStatus::Applied)?;
            Ok(())
        })
        .unwrap();
    }

    // Query all packages
    let all_troves = Trove::list_all(&conn).unwrap();
    assert_eq!(all_troves.len(), 3);

    // Query specific package
    let nginx_troves = Trove::find_by_name(&conn, "nginx").unwrap();
    assert_eq!(nginx_troves.len(), 1);
    assert_eq!(nginx_troves[0].version, "1.21.0");
}

#[test]
fn test_history_shows_operations() {
    use conary_core::db::models::{Changeset, ChangesetStatus};

    let (_dir, _path, mut conn) = common::create_test_db();

    // Create some changesets
    for desc in ["Install nginx", "Install redis", "Remove nginx"] {
        db::transaction(&mut conn, |tx| {
            let mut changeset = Changeset::new(desc.to_string());
            changeset.insert(tx)?;
            changeset.update_status(tx, ChangesetStatus::Applied)?;
            Ok(())
        })
        .unwrap();
    }

    // Verify history
    let changesets = Changeset::list_all(&conn).unwrap();
    assert_eq!(changesets.len(), 3);
    assert_eq!(changesets[0].description, "Remove nginx"); // Most recent first
    assert_eq!(changesets[1].description, "Install redis");
    assert_eq!(changesets[2].description, "Install nginx");

    for changeset in &changesets {
        assert_eq!(changeset.status, ChangesetStatus::Applied);
    }
}

/// Test whatprovides query capability
#[test]
fn test_whatprovides_query() {
    use conary_core::db::models::{ProvideEntry, Trove, TroveType};

    let (_dir, _path, mut conn) = common::create_test_db();

    db::transaction(&mut conn, |tx| {
        // Create a package with various provides
        let mut trove = Trove::new(
            "openssl".to_string(),
            "3.0.0".to_string(),
            TroveType::Package,
        );
        let trove_id = trove.insert(tx)?;

        // Add provides
        let mut p1 = ProvideEntry::new(trove_id, "openssl".to_string(), Some("3.0.0".to_string()));
        p1.insert(tx)?;

        let mut p2 = ProvideEntry::new(trove_id, "soname(libssl.so.3)".to_string(), None);
        p2.insert(tx)?;

        let mut p3 = ProvideEntry::new(trove_id, "soname(libcrypto.so.3)".to_string(), None);
        p3.insert(tx)?;

        Ok(())
    })
    .unwrap();

    // Test exact capability lookup
    let providers = ProvideEntry::find_all_by_capability(&conn, "openssl").unwrap();
    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0].version, Some("3.0.0".to_string()));

    // Test soname lookup
    let ssl_providers = ProvideEntry::find_all_by_capability(&conn, "soname(libssl.so.3)").unwrap();
    assert_eq!(ssl_providers.len(), 1);

    // Test pattern search
    let pattern_results = ProvideEntry::search_capability(&conn, "soname%").unwrap();
    assert_eq!(pattern_results.len(), 2);

    // Test satisfying provider lookup
    let (provider_name, _version) = ProvideEntry::find_satisfying_provider(&conn, "openssl")
        .unwrap()
        .expect("Should find provider");
    assert_eq!(provider_name, "openssl");
}

// =============================================================================
// COMMAND-LEVEL QUERY TESTS
// =============================================================================

/// Test package query operations (equivalent to cmd_query)
#[test]
fn test_query_operations() {
    use conary_core::db::models::{FileEntry, Trove};

    let (_temp_dir, db_path) = common::setup_command_test_db();
    let conn = db::open(&db_path).unwrap();

    // Test listing all packages
    let all_troves = Trove::list_all(&conn).unwrap();
    assert_eq!(all_troves.len(), 2, "Should have 2 packages");

    // Test pattern matching
    let nginx_troves = Trove::find_by_name(&conn, "nginx").unwrap();
    assert_eq!(nginx_troves.len(), 1, "Should find nginx");
    assert_eq!(nginx_troves[0].version, "1.24.0");
    assert_eq!(
        nginx_troves[0].description,
        Some("High performance web server".to_string())
    );

    // Test file path query
    let file = FileEntry::find_by_path(&conn, "/usr/sbin/nginx").unwrap();
    assert!(file.is_some(), "Should find file by path");
    let file = file.unwrap();
    assert_eq!(file.size, 1024000);
    assert_eq!(file.permissions, 0o755_i32);

    // Test finding files by package
    let nginx_id = nginx_troves[0].id.unwrap();
    let files = FileEntry::find_by_trove(&conn, nginx_id).unwrap();
    assert_eq!(files.len(), 2, "nginx should have 2 files");

    // Test non-existent package
    let nonexistent = Trove::find_by_name(&conn, "nonexistent").unwrap();
    assert!(
        nonexistent.is_empty(),
        "Should not find nonexistent package"
    );
}

#[test]
fn list_info_refuses_ambiguous_variants_until_selector_is_given() {
    use conary_core::db::models::{Trove, TroveType};

    let (_tmp, db_path, conn) = common::create_test_db();
    for arch in ["x86_64", "aarch64"] {
        let mut trove = Trove::new(
            "variant-demo".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
        );
        trove.architecture = Some(arch.to_string());
        trove.insert(&conn).unwrap();
    }

    let ambiguous = run_conary(&["list", "variant-demo", "--info", "--db-path", &db_path]);
    assert!(!ambiguous.status.success(), "{}", output_text(&ambiguous));
    let text = output_text(&ambiguous);
    assert!(
        text.contains("Multiple installed variants of 'variant-demo' match"),
        "{text}"
    );
    assert!(text.contains("--arch"), "{text}");

    let selected = run_conary(&[
        "list",
        "variant-demo",
        "--info",
        "--version",
        "1.0.0",
        "--arch",
        "aarch64",
        "--db-path",
        &db_path,
    ]);
    assert!(selected.status.success(), "{}", output_text(&selected));
    let stdout = String::from_utf8_lossy(&selected.stdout);
    assert!(stdout.contains("Architecture: aarch64"), "{stdout}");
    assert!(stdout.contains("Authority   : conary-owned"), "{stdout}");
    assert!(stdout.contains("Source      : file"), "{stdout}");

    let filtered = run_conary(&[
        "list",
        "variant-demo",
        "--version",
        "1.0.0",
        "--arch",
        "aarch64",
        "--db-path",
        &db_path,
    ]);
    assert!(filtered.status.success(), "{}", output_text(&filtered));
    let stdout = String::from_utf8_lossy(&filtered.stdout);
    assert!(stdout.contains("variant-demo 1.0.0"), "{stdout}");
    assert!(stdout.contains("[aarch64]"), "{stdout}");
    assert!(!stdout.contains("[x86_64]"), "{stdout}");
}

#[test]
fn pin_and_unpin_use_same_variant_selector() {
    use conary_core::db::models::{Trove, TroveType};

    let (_tmp, db_path, conn) = common::create_test_db();
    for arch in ["x86_64", "aarch64"] {
        let mut trove = Trove::new(
            "pin-demo".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
        );
        trove.architecture = Some(arch.to_string());
        trove.insert(&conn).unwrap();
    }

    let ambiguous = run_conary(&["pin", "pin-demo", "--db-path", &db_path]);
    assert!(!ambiguous.status.success(), "{}", output_text(&ambiguous));

    let pin = run_conary(&[
        "pin",
        "pin-demo",
        "--version",
        "1.0.0",
        "--arch",
        "x86_64",
        "--db-path",
        &db_path,
    ]);
    assert!(pin.status.success(), "{}", output_text(&pin));

    let pinned = run_conary(&["list", "--pinned", "--db-path", &db_path]);
    assert!(pinned.status.success(), "{}", output_text(&pinned));
    let stdout = String::from_utf8_lossy(&pinned.stdout);
    assert!(stdout.contains("pin-demo 1.0.0 [x86_64]"), "{stdout}");

    let unpin = run_conary(&[
        "unpin",
        "pin-demo",
        "--version",
        "1.0.0",
        "--arch",
        "x86_64",
        "--db-path",
        &db_path,
    ]);
    assert!(unpin.status.success(), "{}", output_text(&unpin));
}

#[test]
fn whatprovides_reports_installed_and_repository_providers() {
    use conary_core::db::models::{Repository, RepositoryPackage, RepositoryProvide};

    let (_tmp, db_path) = common::setup_command_test_db();
    let conn = db::open(&db_path).unwrap();

    let mut repo = Repository::new(
        "daily-driver".to_string(),
        "https://example.test/repo".to_string(),
    );
    repo.gpg_check = false;
    repo.gpg_strict = false;
    let repo_id = repo.insert(&conn).unwrap();

    let mut pkg = RepositoryPackage::new(
        repo_id,
        "openssl-libs".to_string(),
        "3.1.0".to_string(),
        "0".repeat(64),
        10,
        "https://example.test/openssl-libs.ccs".to_string(),
    );
    pkg.architecture = Some("x86_64".to_string());
    let repo_pkg_id = pkg.insert(&conn).unwrap();

    RepositoryProvide::new(
        repo_pkg_id,
        "libssl.so.3".to_string(),
        None,
        "soname".to_string(),
        Some("libssl.so.3".to_string()),
    )
    .insert(&conn)
    .unwrap();

    let output = run_conary(&[
        "query",
        "whatprovides",
        "soname(libssl.so.3)",
        "--db-path",
        &db_path,
    ]);
    assert!(output.status.success(), "{}", output_text(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Installed providers:"), "{stdout}");
    assert!(stdout.contains("openssl 3.0.0"), "{stdout}");
    assert!(stdout.contains("Repository providers:"), "{stdout}");
    assert!(
        stdout.contains("openssl-libs 3.1.0 [x86_64] @daily-driver"),
        "{stdout}"
    );
}

#[test]
fn whatprovides_reads_normalized_repository_provider_metadata() {
    use conary_core::db::models::{Repository, RepositoryPackage, RepositoryProvide};

    let (_tmp, db_path, conn) = common::create_test_db();
    let mut repo = Repository::new(
        "daily-driver".to_string(),
        "https://example.test/repo".to_string(),
    );
    repo.gpg_check = false;
    repo.gpg_strict = false;
    let repo_id = repo.insert(&conn).unwrap();

    let mut pkg = RepositoryPackage::new(
        repo_id,
        "openssl-libs".to_string(),
        "3.1.0".to_string(),
        "0".repeat(64),
        10,
        "https://example.test/openssl-libs.ccs".to_string(),
    );
    pkg.architecture = Some("x86_64".to_string());
    let repo_pkg_id = pkg.insert(&conn).unwrap();

    RepositoryProvide::new(
        repo_pkg_id,
        "libssl.so.3".to_string(),
        None,
        "soname".to_string(),
        Some("libssl.so.3()(64bit)".to_string()),
    )
    .insert(&conn)
    .unwrap();
    drop(conn);

    let untyped = run_conary(&[
        "query",
        "whatprovides",
        "libssl.so.3",
        "--db-path",
        &db_path,
    ]);
    assert!(untyped.status.success(), "{}", output_text(&untyped));
    let untyped_stdout = String::from_utf8_lossy(&untyped.stdout);
    assert!(
        untyped_stdout.contains("No package provides 'libssl.so.3'"),
        "{untyped_stdout}"
    );

    let raw = run_conary(&[
        "query",
        "whatprovides",
        "libssl.so.3()(64bit)",
        "--db-path",
        &db_path,
    ]);
    assert!(raw.status.success(), "{}", output_text(&raw));
    let raw_stdout = String::from_utf8_lossy(&raw.stdout);
    assert!(raw_stdout.contains("Repository providers:"), "{raw_stdout}");
    assert!(
        raw_stdout.contains("openssl-libs 3.1.0 [x86_64] @daily-driver"),
        "{raw_stdout}"
    );

    let output = run_conary(&[
        "query",
        "whatprovides",
        "soname(libssl.so.3)",
        "--db-path",
        &db_path,
    ]);
    assert!(output.status.success(), "{}", output_text(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Repository providers:"), "{stdout}");
    assert!(
        stdout.contains("openssl-libs 3.1.0 [x86_64] @daily-driver"),
        "{stdout}"
    );
}

#[test]
fn whatprovides_reads_normalized_installed_provider_metadata_without_guessing_suffixes() {
    use conary_core::db::models::{ProvideEntry, Trove, TroveType};

    let (_tmp, db_path, conn) = common::create_test_db();

    let mut openssl = Trove::new(
        "openssl-libs".to_string(),
        "3.1.0".to_string(),
        TroveType::Package,
    );
    let openssl_id = openssl.insert(&conn).unwrap();
    ProvideEntry::new_typed(openssl_id, "soname", "libssl.so.3".to_string(), None)
        .insert(&conn)
        .unwrap();

    let mut openssl30 = Trove::new(
        "openssl30-libs".to_string(),
        "3.0.0".to_string(),
        TroveType::Package,
    );
    let openssl30_id = openssl30.insert(&conn).unwrap();
    ProvideEntry::new_typed(openssl30_id, "soname", "libssl.so.30".to_string(), None)
        .insert(&conn)
        .unwrap();
    drop(conn);

    let output = run_conary(&[
        "query",
        "whatprovides",
        "soname(libssl.so.3)",
        "--db-path",
        &db_path,
    ]);
    assert!(output.status.success(), "{}", output_text(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Installed providers:"), "{stdout}");
    assert!(stdout.contains("openssl-libs 3.1.0"), "{stdout}");
    assert!(!stdout.contains("openssl30-libs"), "{stdout}");
}

#[test]
fn whatprovides_ignores_repository_prefix_collisions_and_disabled_repos() {
    use conary_core::db::models::{Repository, RepositoryPackage, RepositoryProvide};

    let (_tmp, db_path, conn) = common::create_test_db();

    let mut enabled_repo = Repository::new(
        "daily-driver".to_string(),
        "https://example.test/enabled".to_string(),
    );
    enabled_repo.gpg_check = false;
    enabled_repo.gpg_strict = false;
    let enabled_repo_id = enabled_repo.insert(&conn).unwrap();
    let mut enabled_pkg = RepositoryPackage::new(
        enabled_repo_id,
        "openssl30-libs".to_string(),
        "3.0.0".to_string(),
        "0".repeat(64),
        10,
        "https://example.test/openssl30-libs.ccs".to_string(),
    );
    let enabled_pkg_id = enabled_pkg.insert(&conn).unwrap();
    RepositoryProvide::new(
        enabled_pkg_id,
        "libssl.so.30".to_string(),
        None,
        "soname".to_string(),
        Some("libssl.so.30()(64bit)".to_string()),
    )
    .insert(&conn)
    .unwrap();

    let mut disabled_repo = Repository::new(
        "disabled-driver".to_string(),
        "https://example.test/disabled".to_string(),
    );
    disabled_repo.enabled = false;
    disabled_repo.gpg_check = false;
    disabled_repo.gpg_strict = false;
    let disabled_repo_id = disabled_repo.insert(&conn).unwrap();
    let mut disabled_pkg = RepositoryPackage::new(
        disabled_repo_id,
        "openssl-libs".to_string(),
        "3.1.0".to_string(),
        "0".repeat(64),
        10,
        "https://example.test/openssl-libs.ccs".to_string(),
    );
    let disabled_pkg_id = disabled_pkg.insert(&conn).unwrap();
    RepositoryProvide::new(
        disabled_pkg_id,
        "libssl.so.3".to_string(),
        None,
        "soname".to_string(),
        Some("libssl.so.3()(64bit)".to_string()),
    )
    .insert(&conn)
    .unwrap();
    drop(conn);

    let untyped = run_conary(&[
        "query",
        "whatprovides",
        "libssl.so.3",
        "--db-path",
        &db_path,
    ]);
    assert!(untyped.status.success(), "{}", output_text(&untyped));
    let untyped_stdout = String::from_utf8_lossy(&untyped.stdout);
    assert!(
        untyped_stdout.contains("No package provides 'libssl.so.3'"),
        "{untyped_stdout}"
    );

    let output = run_conary(&[
        "query",
        "whatprovides",
        "soname(libssl.so.3)",
        "--db-path",
        &db_path,
    ]);
    assert!(output.status.success(), "{}", output_text(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("No package provides 'soname(libssl.so.3)'"),
        "{stdout}"
    );
    assert!(!stdout.contains("openssl30-libs"), "{stdout}");
    assert!(!stdout.contains("openssl-libs 3.1.0"), "{stdout}");
}

#[test]
fn whatbreaks_reports_same_dependency_blocker_as_remove() {
    use conary_core::db::models::{DependencyEntry, FileEntry, InstallSource, Trove, TroveType};

    let (tmp, db_path, conn) = common::create_test_db();
    let provider_payload = tmp.path().join("usr/bin/provider-demo");
    std::fs::create_dir_all(provider_payload.parent().unwrap()).unwrap();
    std::fs::write(&provider_payload, "provider").unwrap();

    let mut provider = Trove::new_with_source(
        "provider-demo".to_string(),
        "1.0.0".to_string(),
        TroveType::Package,
        InstallSource::Repository,
    );
    let provider_id = provider.insert(&conn).unwrap();
    FileEntry::new(
        "/usr/bin/provider-demo".to_string(),
        "0".repeat(64),
        8,
        0o100755,
        provider_id,
    )
    .insert(&conn)
    .unwrap();

    let mut consumer = Trove::new_with_source(
        "consumer-demo".to_string(),
        "1.0.0".to_string(),
        TroveType::Package,
        InstallSource::Repository,
    );
    let consumer_id = consumer.insert(&conn).unwrap();
    DependencyEntry::new(
        consumer_id,
        "provider-demo".to_string(),
        None,
        "runtime".to_string(),
        None,
    )
    .insert(&conn)
    .unwrap();
    drop(conn);

    let whatbreaks = run_conary(&[
        "query",
        "whatbreaks",
        "provider-demo",
        "--db-path",
        &db_path,
    ]);
    assert!(whatbreaks.status.success(), "{}", output_text(&whatbreaks));
    let stdout = String::from_utf8_lossy(&whatbreaks.stdout);
    assert!(
        stdout.contains("Removing 'provider-demo' would break"),
        "{stdout}"
    );
    assert!(stdout.contains("consumer-demo"), "{stdout}");

    let remove = run_conary(&[
        "remove",
        "provider-demo",
        "--db-path",
        &db_path,
        "--root",
        tmp.path().to_str().unwrap(),
        "--no-scripts",
        "--sandbox",
        "never",
        "--yes",
    ]);
    assert!(!remove.status.success(), "{}", output_text(&remove));
    assert!(output_text(&remove).contains("consumer-demo"));
    assert!(output_text(&remove).contains("conary query whatbreaks"));
}

#[test]
fn update_package_selector_refuses_ambiguous_variants_at_cli() {
    use conary_core::db::models::{Trove, TroveType};

    let (_tmp, db_path, conn) = common::create_test_db();
    for arch in ["x86_64", "aarch64"] {
        let mut trove = Trove::new(
            "update-demo".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
        );
        trove.architecture = Some(arch.to_string());
        trove.insert(&conn).unwrap();
    }

    let ambiguous = run_conary(&["update", "update-demo", "--dry-run", "--db-path", &db_path]);
    assert!(!ambiguous.status.success(), "{}", output_text(&ambiguous));
    let text = output_text(&ambiguous);
    assert!(
        text.contains("Multiple installed variants of 'update-demo' match"),
        "{text}"
    );
    assert!(text.contains("--arch"), "{text}");

    let selected = run_conary(&[
        "update",
        "update-demo",
        "--dry-run",
        "--version",
        "1.0.0",
        "--arch",
        "aarch64",
        "--db-path",
        &db_path,
    ]);
    assert!(selected.status.success(), "{}", output_text(&selected));
}

#[test]
fn update_collection_refuses_installed_variant_selectors() {
    let (_tmp, db_path, _conn) = common::create_test_db();

    let output = run_conary(&[
        "update",
        "@base",
        "--dry-run",
        "--arch",
        "x86_64",
        "--db-path",
        &db_path,
    ]);

    assert!(!output.status.success(), "{}", output_text(&output));
    let text = output_text(&output);
    assert!(
        text.contains("cannot be used with collection updates"),
        "{text}"
    );
}

#[test]
fn list_modes_refuse_ignored_installed_variant_selectors() {
    let (_tmp, db_path) = common::setup_command_test_db();

    let pinned = run_conary(&[
        "list",
        "--pinned",
        "--arch",
        "x86_64",
        "--db-path",
        &db_path,
    ]);
    assert!(!pinned.status.success(), "{}", output_text(&pinned));
    let text = output_text(&pinned);
    assert!(text.contains("cannot be used with --pinned"), "{text}");

    let path = run_conary(&[
        "list",
        "--path",
        "/usr/sbin/nginx",
        "--arch",
        "x86_64",
        "--db-path",
        &db_path,
    ]);
    assert!(!path.status.success(), "{}", output_text(&path));
    let text = output_text(&path);
    assert!(text.contains("cannot be used with --path"), "{text}");
}

/// Test dependency query operations (equivalent to cmd_depends/cmd_rdepends)
#[test]
fn test_dependency_queries() {
    use conary_core::db::models::{DependencyEntry, ProvideEntry, Trove};

    let (_temp_dir, db_path) = common::setup_command_test_db();
    let conn = db::open(&db_path).unwrap();

    // Get nginx's dependencies
    let nginx = Trove::find_by_name(&conn, "nginx").unwrap();
    let nginx_id = nginx[0].id.unwrap();
    let deps = DependencyEntry::find_by_trove(&conn, nginx_id).unwrap();
    assert_eq!(deps.len(), 1, "nginx should have 1 dependency");
    assert_eq!(deps[0].depends_on_name, "openssl");
    assert_eq!(deps[0].depends_on_version, Some(">= 3.0".to_string()));

    // Test reverse dependency lookup via provides
    let openssl_providers = ProvideEntry::find_all_by_capability(&conn, "openssl").unwrap();
    assert!(
        !openssl_providers.is_empty(),
        "Should find openssl provider"
    );

    // Verify soname provides
    let libssl_providers =
        ProvideEntry::find_all_by_capability(&conn, "soname(libssl.so.3)").unwrap();
    assert_eq!(
        libssl_providers.len(),
        1,
        "Should find libssl.so.3 provider"
    );
}

/// Test changeset history (equivalent to cmd_history)
#[test]
fn test_changeset_history() {
    use conary_core::db::models::{Changeset, ChangesetStatus};

    let (_temp_dir, db_path) = common::setup_command_test_db();
    let conn = db::open(&db_path).unwrap();

    // List all changesets
    let changesets = Changeset::list_all(&conn).unwrap();
    assert_eq!(changesets.len(), 2, "Should have 2 changesets");

    // Verify changeset details
    let nginx_cs = changesets
        .iter()
        .find(|c| c.description.contains("nginx"))
        .unwrap();
    assert_eq!(nginx_cs.status, ChangesetStatus::Applied);

    let openssl_cs = changesets
        .iter()
        .find(|c| c.description.contains("openssl"))
        .unwrap();
    assert_eq!(openssl_cs.status, ChangesetStatus::Applied);

    // Test finding by ID
    let cs_by_id = Changeset::find_by_id(&conn, nginx_cs.id.unwrap()).unwrap();
    assert!(cs_by_id.is_some());
    assert_eq!(cs_by_id.unwrap().description, nginx_cs.description);
}

/// Test whatprovides functionality
#[test]
fn test_whatprovides_operations() {
    use conary_core::db::models::ProvideEntry;

    let (_temp_dir, db_path) = common::setup_command_test_db();
    let conn = db::open(&db_path).unwrap();

    // Test finding provider by capability
    let webserver_providers = ProvideEntry::find_all_by_capability(&conn, "webserver").unwrap();
    assert_eq!(webserver_providers.len(), 1);

    // Test soname lookup
    let ssl_providers = ProvideEntry::find_all_by_capability(&conn, "soname(libssl.so.3)").unwrap();
    assert_eq!(ssl_providers.len(), 1);

    // Test pattern search
    let soname_results = ProvideEntry::search_capability(&conn, "soname%").unwrap();
    assert_eq!(soname_results.len(), 1, "Should find 1 soname provide");

    // Test satisfying provider
    let (name, _version) = ProvideEntry::find_satisfying_provider(&conn, "openssl")
        .unwrap()
        .expect("Should find openssl provider");
    assert_eq!(name, "openssl");

    // Test non-existent capability
    let nonexistent = ProvideEntry::find_all_by_capability(&conn, "nonexistent").unwrap();
    assert!(nonexistent.is_empty());
}

/// Test dependency tree building
#[test]
fn test_dependency_tree() {
    use conary_core::db::models::{DependencyEntry, Trove};

    let (_temp_dir, db_path) = common::setup_command_test_db();
    let conn = db::open(&db_path).unwrap();

    // Build dependency tree for nginx
    let nginx = Trove::find_by_name(&conn, "nginx").unwrap().pop().unwrap();
    let nginx_deps = DependencyEntry::find_by_trove(&conn, nginx.id.unwrap()).unwrap();

    // nginx depends on openssl
    assert_eq!(nginx_deps.len(), 1);
    assert_eq!(nginx_deps[0].depends_on_name, "openssl");

    // openssl has no dependencies in our test setup
    let openssl = Trove::find_by_name(&conn, "openssl")
        .unwrap()
        .pop()
        .unwrap();
    let openssl_deps = DependencyEntry::find_by_trove(&conn, openssl.id.unwrap()).unwrap();
    assert!(
        openssl_deps.is_empty(),
        "openssl should have no deps in test"
    );

    // This verifies the structure needed for deptree command
}

/// Test what-breaks analysis (reverse dependency check)
#[test]
fn test_what_breaks_analysis() {
    use conary_core::db::models::{DependencyEntry, Trove};

    let (_temp_dir, db_path) = common::setup_command_test_db();
    let conn = db::open(&db_path).unwrap();

    // Find what depends on openssl
    // This requires checking all packages' dependencies
    let all_troves = Trove::list_all(&conn).unwrap();
    let mut dependents = Vec::new();

    for trove in &all_troves {
        if let Some(id) = trove.id {
            let deps = DependencyEntry::find_by_trove(&conn, id).unwrap();
            for dep in deps {
                if dep.depends_on_name == "openssl" {
                    dependents.push(trove.name.clone());
                }
            }
        }
    }

    assert_eq!(dependents.len(), 1);
    assert_eq!(dependents[0], "nginx");

    // This verifies: removing openssl would break nginx
}
