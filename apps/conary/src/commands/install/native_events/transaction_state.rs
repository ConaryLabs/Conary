// apps/conary/src/commands/install/native_events/transaction_state.rs

//! Persisted package-state projection for native lifecycle transaction planning.

use anyhow::{Context, Result};
use conary_core::ccs::native_transaction::{NativeInstalledCapability, NativeInstalledPackage};
use conary_core::db::models::{FileEntry, ProvideEntry, Trove};
use rusqlite::Connection;
use std::collections::{BTreeSet, HashSet};

pub(super) fn installed_packages_before(conn: &Connection) -> Result<Vec<NativeInstalledPackage>> {
    Trove::list_packages(conn)?
        .into_iter()
        .map(|trove| {
            let trove_id = trove
                .id
                .with_context(|| format!("installed package '{}' has no trove id", trove.name))?;
            let paths = FileEntry::find_by_trove(conn, trove_id)?
                .into_iter()
                .map(|file| file.path)
                .collect::<BTreeSet<_>>();
            Ok(NativeInstalledPackage {
                package_name: trove.name,
                package_version: trove.version,
                package_arch: trove.architecture,
                paths,
            })
        })
        .collect()
}

pub(super) fn declared_capabilities_excluding(
    conn: &Connection,
    excluded_trove_ids: &HashSet<i64>,
) -> Result<Vec<NativeInstalledCapability>> {
    let mut capabilities = Vec::new();
    for trove in Trove::list_all(conn)? {
        let trove_id = trove
            .id
            .with_context(|| format!("installed package '{}' has no trove id", trove.name))?;
        if excluded_trove_ids.contains(&trove_id) {
            continue;
        }
        let version_scheme = trove.version_scheme;
        capabilities.push(NativeInstalledCapability {
            name: trove.name,
            version: Some(trove.version),
            version_scheme,
        });
        for provide in ProvideEntry::find_by_trove(conn, trove_id)? {
            capabilities.push(NativeInstalledCapability {
                name: provide.capability,
                version: provide.version,
                version_scheme,
            });
        }
    }
    sort_and_deduplicate_capabilities(&mut capabilities);
    Ok(capabilities)
}

pub(super) fn sort_and_deduplicate_capabilities(capabilities: &mut Vec<NativeInstalledCapability>) {
    capabilities.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.version.cmp(&right.version))
            .then_with(|| {
                left.version_scheme
                    .as_str()
                    .cmp(right.version_scheme.as_str())
            })
    });
    capabilities.dedup();
}

pub(super) fn installed_instance_count(conn: &Connection, package_name: &str) -> Result<u32> {
    u32::try_from(Trove::find_by_name(conn, package_name)?.len())
        .context("installed package instance count exceeds u32")
}
