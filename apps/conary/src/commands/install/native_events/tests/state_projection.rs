// apps/conary/src/commands/install/native_events/tests/state_projection.rs

//! Typed selection and scaling proof for native transaction state projection.

use super::*;
use conary_core::ccs::native_transaction::NativeTransactionStep;
use conary_core::db::models::NativeLifecycleResidualState;
use std::cell::RefCell;

fn empty_native_bundle(
    package_name: &str,
    package_version: &str,
    source_format: SourceFormat,
    version_scheme: LifecycleVersionScheme,
) -> NativeLifecycleBundle {
    let mut bundle = pre_remove_bundle(package_name, package_version);
    bundle.source_format = source_format;
    bundle.source_family = "fixture".to_string();
    bundle.source_profile = None;
    bundle.source_release = None;
    bundle.version_scheme = version_scheme;
    bundle.entries.clear();
    bundle
}

fn format_cases() -> [(
    &'static str,
    &'static str,
    SourceFormat,
    LifecycleVersionScheme,
    VersionScheme,
); 4] {
    [
        (
            "rpm-owner",
            "1.0-1.fc44",
            SourceFormat::Rpm,
            LifecycleVersionScheme::Rpm,
            VersionScheme::Rpm,
        ),
        (
            "deb-owner",
            "1.0-1",
            SourceFormat::Deb,
            LifecycleVersionScheme::Deb,
            VersionScheme::Debian,
        ),
        (
            "arch-owner",
            "1.0-1",
            SourceFormat::Arch,
            LifecycleVersionScheme::Arch,
            VersionScheme::Arch,
        ),
        (
            "eopkg-owner",
            "1.0-1",
            SourceFormat::Eopkg,
            LifecycleVersionScheme::Eopkg,
            VersionScheme::Eopkg,
        ),
    ]
}

thread_local! {
    static PREPARATION_SQL: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

fn record_preparation_sql(event: rusqlite::trace::TraceEvent<'_>) {
    let rusqlite::trace::TraceEvent::Stmt(statement, _) = event else {
        return;
    };
    let sql = statement
        .expanded_sql()
        .unwrap_or_else(|| statement.sql().into_owned());
    PREPARATION_SQL.with(|statements| statements.borrow_mut().push(sql));
}

fn lifecycle_free_preparation_with_unrelated_claims(
    claim_count: usize,
) -> (PreparedNativeTransaction, Vec<String>) {
    let (_temp, db_path) = crate::commands::test_helpers::create_test_db();
    let conn = conary_core::db::open(&db_path).unwrap();
    let mut unrelated = Trove::new(
        "unrelated".to_string(),
        "1.0.0".to_string(),
        TroveType::Package,
        VersionScheme::Conary,
    );
    let unrelated_id = unrelated.insert(&conn).unwrap();
    for index in 0..claim_count {
        let content = format!("payload-{index}");
        let mut entry = FileEntry::new(
            format!("/usr/share/unrelated/{index}"),
            conary_core::payload::ResolvedPayloadNode::from_numeric_source(
                conary_core::payload::PayloadNode::regular(0o644),
            )
            .unwrap(),
            Some(conary_core::payload::PayloadContentAuthority {
                sha256: conary_core::hash::sha256(content.as_bytes()),
                size: content.len() as u64,
            }),
            unrelated_id,
        );
        entry.insert(&conn).unwrap();
    }

    PREPARATION_SQL.with(|statements| statements.borrow_mut().clear());
    conn.trace_v2(
        rusqlite::trace::TraceEventCodes::SQLITE_TRACE_STMT,
        Some(record_preparation_sql),
    );
    let prepared = PreparedNativeTransaction::prepare_install(
        &conn,
        NativeInstallInput {
            package_name: "bridge-tool",
            package_version: "1.0.0",
            package_arch: Some("x86_64"),
            version_scheme: VersionScheme::Conary,
            provides: &[],
            new_bundle: None,
            old_trove: None,
            relation_removals: &[],
            relation_deconfigurations: &[],
            paths: vec!["usr/bin/bridge-tool".to_string()],
        },
    )
    .unwrap();
    conn.trace_v2(rusqlite::trace::TraceEventCodes::empty(), None);
    let statements =
        PREPARATION_SQL.with(|statements| std::mem::take(&mut *statements.borrow_mut()));
    (prepared, statements)
}

fn payload_steps(prepared: &PreparedNativeTransaction) -> Vec<NativeTransactionStep> {
    prepared
        .plan
        .graph
        .steps
        .iter()
        .filter(|step| {
            matches!(
                step,
                NativeTransactionStep::ApplyPayload { .. }
                    | NativeTransactionStep::FinalizeOldPayload { .. }
                    | NativeTransactionStep::PurgeConfigFiles { .. }
            )
        })
        .copied()
        .collect()
}

#[test]
fn lifecycle_free_preparation_is_independent_of_unrelated_payload_claims() {
    let (without_claims, without_claim_sql) = lifecycle_free_preparation_with_unrelated_claims(0);
    let (with_claims, with_claim_sql) = lifecycle_free_preparation_with_unrelated_claims(128);

    for prepared in [&without_claims, &with_claims] {
        assert!(prepared.owners.is_empty());
        assert!(prepared.plan.events.is_empty());
        assert_eq!(
            prepared.transaction_state,
            NativeTransactionState::default()
        );
        assert_eq!(
            payload_steps(prepared),
            vec![
                NativeTransactionStep::ApplyPayload { change_index: 0 },
                NativeTransactionStep::FinalizeOldPayload { change_index: 0 },
            ]
        );
        assert_eq!(prepared.changes[0].package_name, "bridge-tool");
        assert_eq!(
            prepared.changes[0].new_paths,
            BTreeSet::from(["usr/bin/bridge-tool".to_string()])
        );
    }

    let global_payload_queries = |statements: &[String]| {
        statements
            .iter()
            .filter(|sql| {
                sql.contains("FROM payload_claims WHERE trove_id")
                    || sql.contains("WHERE path =") && sql.contains("materialization_target_path")
            })
            .count()
    };
    assert_eq!(global_payload_queries(&without_claim_sql), 0);
    assert_eq!(global_payload_queries(&with_claim_sql), 0);
    assert_eq!(
        without_claim_sql.len(),
        with_claim_sql.len(),
        "unrelated installed payload-claim count must not change preparation query work"
    );
}

#[test]
fn incoming_and_installed_native_owners_keep_the_complete_state_path() {
    for (package_name, package_version, source_format, lifecycle_scheme, version_scheme) in
        format_cases()
    {
        let (_temp, db_path) = crate::commands::test_helpers::create_test_db();
        let conn = conary_core::db::open(&db_path).unwrap();
        let bundle = empty_native_bundle(
            package_name,
            package_version,
            source_format,
            lifecycle_scheme,
        );
        let path = format!("usr/lib/{package_name}");
        let prepared = PreparedNativeTransaction::prepare_install(
            &conn,
            NativeInstallInput {
                package_name,
                package_version,
                package_arch: Some("x86_64"),
                version_scheme,
                provides: &[],
                new_bundle: Some(&bundle),
                old_trove: None,
                relation_removals: &[],
                relation_deconfigurations: &[],
                paths: vec![path.clone()],
            },
        )
        .unwrap();
        assert_eq!(prepared.owners.len(), 1, "{package_name}");
        assert!(
            prepared
                .transaction_state
                .installed_paths_after
                .contains(&path),
            "incoming {package_name} must retain the complete native-state projection"
        );
    }

    for (package_name, package_version, source_format, lifecycle_scheme, version_scheme) in
        format_cases()
    {
        let (_temp, db_path) = crate::commands::test_helpers::create_test_db();
        let conn = conary_core::db::open(&db_path).unwrap();
        let mut trove = Trove::new(
            package_name.to_string(),
            package_version.to_string(),
            TroveType::Package,
            version_scheme,
        );
        trove.architecture = Some("x86_64".to_string());
        if version_scheme == VersionScheme::Debian {
            trove.debian_multi_arch = Some(DebianMultiArch::No);
        }
        let trove_id = trove.insert(&conn).unwrap();
        let bundle = empty_native_bundle(
            package_name,
            package_version,
            source_format,
            lifecycle_scheme,
        );
        InstalledNativeLifecycleBundle::new(trove_id, None, &bundle)
            .unwrap()
            .insert_or_replace(&conn)
            .unwrap();

        let prepared = PreparedNativeTransaction::prepare_install(
            &conn,
            NativeInstallInput {
                package_name: "bridge-tool",
                package_version: "1.0.0",
                package_arch: Some("x86_64"),
                version_scheme: VersionScheme::Conary,
                provides: &[],
                new_bundle: None,
                old_trove: None,
                relation_removals: &[],
                relation_deconfigurations: &[],
                paths: vec!["usr/bin/bridge-tool".to_string()],
            },
        )
        .unwrap();
        assert_eq!(prepared.owners.len(), 1, "{package_name}");
        assert!(
            prepared
                .transaction_state
                .installed_packages_before
                .iter()
                .any(|package| package.package_name == package_name),
            "installed {package_name} must retain the complete native-state projection"
        );
    }
}

#[test]
fn lifecycle_free_removal_keeps_the_exact_payload_graph() {
    let (_temp, db_path) = crate::commands::test_helpers::create_test_db();
    let conn = conary_core::db::open(&db_path).unwrap();
    let mut trove = Trove::new(
        "bridge-tool".to_string(),
        "1.0.0".to_string(),
        TroveType::Package,
        VersionScheme::Conary,
    );
    let trove_id = trove.insert(&conn).unwrap();
    let prepared = PreparedNativeTransaction::prepare_remove(
        &conn,
        trove_id,
        "bridge-tool",
        "1.0.0",
        vec!["usr/bin/bridge-tool".to_string()],
        false,
    )
    .unwrap();

    assert_eq!(
        prepared.transaction_state,
        NativeTransactionState::default()
    );
    assert_eq!(
        payload_steps(&prepared),
        vec![
            NativeTransactionStep::ApplyPayload { change_index: 0 },
            NativeTransactionStep::FinalizeOldPayload { change_index: 0 },
        ]
    );
    assert_eq!(
        prepared.changes[0].operation,
        NativeTransactionOperation::Remove
    );
    assert_eq!(
        prepared.changes[0].old_paths,
        BTreeSet::from(["usr/bin/bridge-tool".to_string()])
    );
}

#[test]
fn residual_debian_state_and_arch_implicit_behavior_reject_the_compact_path() {
    let (_temp, db_path) = crate::commands::test_helpers::create_test_db();
    let conn = conary_core::db::open(&db_path).unwrap();
    let residual_bundle = empty_native_bundle(
        "residual-deb",
        "1.0-1",
        SourceFormat::Deb,
        LifecycleVersionScheme::Deb,
    );
    let mut residual = NativeLifecycleResidualState::new(
        &residual_bundle,
        DebPackageState::ConfigFiles,
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    residual.upsert(&conn).unwrap();

    let residual_prepared = PreparedNativeTransaction::prepare_install(
        &conn,
        NativeInstallInput {
            package_name: "bridge-tool",
            package_version: "1.0.0",
            package_arch: Some("x86_64"),
            version_scheme: VersionScheme::Conary,
            provides: &[],
            new_bundle: None,
            old_trove: None,
            relation_removals: &[],
            relation_deconfigurations: &[],
            paths: vec!["usr/bin/bridge-tool".to_string()],
        },
    )
    .unwrap();
    assert_eq!(
        residual_prepared.transaction_state.deb_package_states.len(),
        1
    );
    assert!(
        residual_prepared
            .transaction_state
            .installed_paths_after
            .contains("usr/bin/bridge-tool")
    );

    let (_temp, arch_db_path) = crate::commands::test_helpers::create_test_db();
    let arch_conn = conary_core::db::open(&arch_db_path).unwrap();
    let arch_prepared = PreparedNativeTransaction::prepare_install(
        &arch_conn,
        NativeInstallInput {
            package_name: "arch-payload",
            package_version: "1.0-1",
            package_arch: Some("x86_64"),
            version_scheme: VersionScheme::Arch,
            provides: &[],
            new_bundle: None,
            old_trove: None,
            relation_removals: &[],
            relation_deconfigurations: &[],
            paths: vec!["etc/ld.so.conf".to_string()],
        },
    )
    .unwrap();
    assert!(arch_prepared.arch_transaction);
    assert!(arch_prepared.arch_ldconfig_required_after);
    assert!(
        arch_prepared
            .transaction_state
            .installed_paths_after
            .contains("etc/ld.so.conf")
    );
}
