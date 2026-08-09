// apps/conary/src/commands/install/batch/tests/witness_universe.rs

//! The identities a transaction may reason with are the ones it leaves behind.
//!
//! Each test installs a prior transaction, then asks the ordering validator
//! about a batch that deletes one of the identities that transaction left. The
//! deleted identity's capabilities must not certify a dependency edge, because
//! they are gone the moment this batch commits -- while the same capability
//! carried forward by the incoming version, or held by a package this batch
//! leaves alone, must still certify it.

use super::*;
use conary_core::repository::dependency_model::{
    ProvidedCapability, RepositoryRequirementClause, RepositoryRequirementGroup,
    RepositoryRequirementKind, SourcePackageFormat,
};
use conary_core::repository::versioning::VersionScheme;

const PROMISED_CONFIG: &str = "/etc/crypto-policies/back-ends/krb5.config";
const SHIPPED_LIBRARY: &str = "/usr/lib64/libcrypto.so.3";

/// The exact installed trove a later batch supersedes.
fn installed_trove(conn: &rusqlite::Connection, name: &str) -> Trove {
    Trove::list_all(conn)
        .unwrap()
        .into_iter()
        .find(|trove| trove.name == name)
        .unwrap_or_else(|| panic!("the prior transaction must have installed {name}"))
}

/// `name` prepared as the exact upgrade of the installed trove it replaces.
///
/// Preparation binds an incoming package to the trove it supersedes. That
/// binding, not the shared package name, is what makes the installed identity
/// outgoing: a name can be shared by identities this transaction never touches.
fn upgrade_of(conn: &rusqlite::Connection, name: &str, payload: &str) -> PreparedPackage {
    let mut package = prepared_test_package(name, payload, b"upgraded");
    package.version = "2.0.0".to_string();
    package.provides = vec![crate::commands::test_helpers::exact_package_self_provider(
        name,
        "2.0.0",
        VersionScheme::Rpm,
    )];
    package.install_reason = InstallReason::Dependency;
    package.is_upgrade = true;
    package.old_trove = Some(Box::new(installed_trove(conn, name)));
    package
}

/// An ordinary source-derived provider for a path the package ships.
fn shipped(path: &str) -> ProvidedCapability {
    ProvidedCapability::payload_file(SourcePackageFormat::Rpm, path)
}

fn obsoletes(name: &str) -> RepositoryRequirementGroup {
    RepositoryRequirementGroup::simple(
        RepositoryRequirementKind::Obsolete,
        RepositoryRequirementClause::name_only(name.to_string()),
    )
}

fn database_after(temp: &std::path::Path, prior: Vec<PreparedPackage>) -> rusqlite::Connection {
    let (db_path, _db_path_string) = db_with_prior_transaction(temp, prior);
    conary_core::db::open(&db_path).unwrap()
}

fn ordered_names(packages: &[PreparedPackage]) -> Vec<&str> {
    packages
        .iter()
        .map(|package| package.name.as_str())
        .collect()
}

fn assert_unsatisfied_for(error: &anyhow::Error, dependent: &str) {
    let message = format!("{error:#}");
    assert!(
        message.contains(&format!(
            "selected package transaction does not satisfy exact depends requirement for {dependent}"
        )),
        "{message}"
    );
}

#[test]
fn an_upgrade_that_drops_a_promise_cannot_witness_the_edge_that_leaned_on_it() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp = tempfile::tempdir().unwrap();
    let conn = database_after(
        temp.path(),
        vec![crypto_policies_with_promise(PROMISED_CONFIG)],
    );

    // The installed 1.0.0 promises the path. This transaction replaces it with
    // a version that does not, so reading the promise off the row the
    // transaction is about to delete certifies an edge against a capability
    // that stops existing at commit.
    let mut dropped = vec![
        krb5_depending_on(depends_on(PROMISED_CONFIG)),
        upgrade_of(
            &conn,
            "crypto-policies",
            "/usr/share/crypto-policies/DEFAULT",
        ),
    ];

    let error =
        super::super::ordering::order_packages_for_transaction(&conn, &mut dropped).unwrap_err();

    assert_unsatisfied_for(&error, "krb5-libs");
}

#[test]
fn an_upgrade_that_keeps_the_promise_still_witnesses_the_edge() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp = tempfile::tempdir().unwrap();
    let conn = database_after(
        temp.path(),
        vec![crypto_policies_with_promise(PROMISED_CONFIG)],
    );

    // Same supersession, but the incoming version carries the promise forward.
    // Withholding the outgoing identity must not cost the transaction an edge
    // its own members still provide -- and the promising member now has to be
    // installed before the package that leans on it.
    let mut kept = vec![
        krb5_depending_on(depends_on(PROMISED_CONFIG)),
        upgrade_of(
            &conn,
            "crypto-policies",
            "/usr/share/crypto-policies/DEFAULT",
        ),
    ];
    kept[1].provides.push(promised(PROMISED_CONFIG));

    super::super::ordering::order_packages_for_transaction(&conn, &mut kept).unwrap();

    assert_eq!(ordered_names(&kept), vec!["crypto-policies", "krb5-libs"]);
}

#[test]
fn an_upgrade_that_drops_a_shipped_path_cannot_witness_the_edge_that_leaned_on_it() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp = tempfile::tempdir().unwrap();
    let mut installed = prepared_test_package("openssl-libs", SHIPPED_LIBRARY, b"crypto");
    installed.provides.push(shipped(SHIPPED_LIBRARY));
    let conn = database_after(temp.path(), vec![installed]);

    // Nothing about this is specific to promises: an ordinary provide the
    // incoming version drops is just as absent at commit as a dropped promise.
    let mut dropped = vec![
        krb5_depending_on(depends_on(SHIPPED_LIBRARY)),
        upgrade_of(&conn, "openssl-libs", "/usr/lib64/libcrypto.so.4"),
    ];

    let error =
        super::super::ordering::order_packages_for_transaction(&conn, &mut dropped).unwrap_err();

    assert_unsatisfied_for(&error, "krb5-libs");
}

#[test]
fn an_upgrade_that_keeps_a_shipped_path_still_witnesses_the_edge() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp = tempfile::tempdir().unwrap();
    let mut installed = prepared_test_package("openssl-libs", SHIPPED_LIBRARY, b"crypto");
    installed.provides.push(shipped(SHIPPED_LIBRARY));
    let conn = database_after(temp.path(), vec![installed]);

    let mut kept = vec![
        krb5_depending_on(depends_on(SHIPPED_LIBRARY)),
        upgrade_of(&conn, "openssl-libs", SHIPPED_LIBRARY),
    ];
    kept[1].provides.push(shipped(SHIPPED_LIBRARY));

    super::super::ordering::order_packages_for_transaction(&conn, &mut kept).unwrap();

    assert_eq!(ordered_names(&kept), vec!["openssl-libs", "krb5-libs"]);
}

#[test]
fn a_package_this_transaction_removes_by_relation_cannot_witness_an_edge() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp = tempfile::tempdir().unwrap();
    let mut legacy = prepared_test_package("legacy-tool", "/usr/bin/tool", b"tool");
    legacy.provides.push(shipped("/usr/bin/tool"));
    let conn = database_after(temp.path(), vec![legacy]);
    let dependent = || {
        let mut dependent =
            prepared_test_package("tool-plugin", "/usr/lib/tool/plugin.so", b"plug");
        dependent.requirements = vec![depends_on("/usr/bin/tool")];
        dependent
    };

    // Nothing in the batch is upgrading legacy-tool: the incoming package
    // obsoletes it, which is decided by the same relation authority that plans
    // the removal. The installed provider is outgoing all the same, so the path
    // it owns cannot certify the plugin's requirement.
    let mut obsoleting = prepared_test_package("modern-tool", "/usr/bin/modern-tool", b"modern");
    obsoleting.relations = vec![obsoletes("legacy-tool")];
    let mut removing = vec![dependent(), obsoleting];

    let error =
        super::super::ordering::order_packages_for_transaction(&conn, &mut removing).unwrap_err();

    assert_unsatisfied_for(&error, "tool-plugin");

    // The same batch without the relation leaves legacy-tool installed, and its
    // path witnesses the requirement exactly as before.
    let mut untouched = vec![
        dependent(),
        prepared_test_package("modern-tool", "/usr/bin/modern-tool", b"modern"),
    ];

    super::super::ordering::order_packages_for_transaction(&conn, &mut untouched)
        .expect("an installed provider this batch leaves alone must still witness the edge");
}

#[test]
fn a_lone_package_that_upgrades_away_its_own_promised_provider_fails() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp = tempfile::tempdir().unwrap();
    let conn = database_after(
        temp.path(),
        vec![crypto_policies_with_promise(PROMISED_CONFIG)],
    );

    // One package, nothing to order, and the only holder of the promise it
    // depends on is the version it replaces. Withholding that identity is
    // right, but the batch must then say so: with no holder left there is no
    // promise obligation to record, so silence here would commit an
    // unsatisfiable dependency with nothing downstream left to catch it.
    let mut lone = vec![upgrade_of(
        &conn,
        "crypto-policies",
        "/usr/share/crypto-policies/DEFAULT",
    )];
    lone[0].requirements = vec![depends_on(PROMISED_CONFIG)];

    let error =
        super::super::ordering::order_packages_for_transaction(&conn, &mut lone).unwrap_err();

    assert_unsatisfied_for(&error, "crypto-policies");
}

#[test]
fn a_lone_package_that_upgrades_away_a_shipped_path_it_requires_fails() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp = tempfile::tempdir().unwrap();
    let mut openssl = prepared_test_package("openssl-libs", SHIPPED_LIBRARY, b"crypto");
    openssl.provides.push(shipped(SHIPPED_LIBRARY));
    let conn = database_after(
        temp.path(),
        vec![openssl, crypto_policies_with_promise(PROMISED_CONFIG)],
    );

    // The withheld capability is an ordinary provide this time. Every
    // single-package batch is certified against the end-state universe since
    // #345, promised path or not, and the requirement that fails is the
    // shipped one the superseded version alone provided.
    let mut lone = vec![upgrade_of(
        &conn,
        "openssl-libs",
        "/usr/lib64/libcrypto.so.4",
    )];
    lone[0].requirements = vec![depends_on(PROMISED_CONFIG), depends_on(SHIPPED_LIBRARY)];

    let error =
        super::super::ordering::order_packages_for_transaction(&conn, &mut lone).unwrap_err();

    assert_unsatisfied_for(&error, "openssl-libs");
}

#[test]
fn a_lone_package_whose_provider_survives_the_transaction_orders() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp = tempfile::tempdir().unwrap();
    let conn = database_after(
        temp.path(),
        vec![crypto_policies_with_promise(PROMISED_CONFIG)],
    );

    // Same single-package shape, same promise, but this batch touches nobody:
    // the installed holder is still standing at commit, so the requirement is
    // certified and the transaction proceeds to its promise obligations.
    let mut lone = vec![krb5_depending_on(depends_on(PROMISED_CONFIG))];

    super::super::ordering::order_packages_for_transaction(&conn, &mut lone)
        .expect("a surviving installed holder must still certify a lone package's requirement");
}

#[test]
fn a_lone_package_satisfied_by_surviving_installed_state_passes_validation() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp = tempfile::tempdir().unwrap();
    let mut provider = prepared_test_package("libprovider", "/usr/lib64/libprovider.so.1", b"lib");
    provider
        .provides
        .push(shipped("/usr/lib64/libprovider.so.1"));
    let conn = database_after(temp.path(), vec![provider]);

    // No promised path anywhere: since #345 the end-state universe is computed
    // for every single-package batch, so an installed provider this batch
    // leaves alone must certify the lone package's requirement exactly as it
    // would certify an edge in an ordered batch. Uniform validation must not
    // over-reject a correct lone install.
    let mut lone = prepared_test_package("consumer", "/usr/bin/consumer", b"consumer");
    lone.requirements = vec![depends_on("/usr/lib64/libprovider.so.1")];

    super::super::ordering::order_packages_for_transaction(&conn, &mut vec![lone])
        .expect("a surviving installed provider must certify a lone package's requirement");
}
