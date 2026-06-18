// apps/remi/src/server/native_publish/public_lookup.rs

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::Result;
use conary_core::db::models::{NativePackagePublication, normalize_native_architecture};

pub enum NativeLookup<T> {
    Ready(T),
    Ambiguous(Vec<String>),
    Missing,
}

pub fn active_native_publications(
    db_path: &Path,
    distro: &str,
    name: &str,
    version: Option<&str>,
    release: Option<&str>,
    architecture: Option<&str>,
) -> Result<Vec<NativePackagePublication>> {
    let conn = crate::server::open_runtime_db(db_path)?;
    NativePackagePublication::find_active(
        &conn,
        distro,
        name,
        version,
        release,
        architecture
            .map(|arch| normalize_native_architecture(Some(arch)))
            .as_deref(),
    )
    .map_err(Into::into)
}

pub fn resolve_active_native_publication(
    db_path: &Path,
    distro: &str,
    name: &str,
    version: Option<&str>,
    release: Option<&str>,
    architecture: Option<&str>,
) -> Result<NativeLookup<NativePackagePublication>> {
    let rows = active_native_publications(db_path, distro, name, version, release, architecture)?;
    match rows.len() {
        0 => Ok(NativeLookup::Missing),
        1 => Ok(NativeLookup::Ready(rows.into_iter().next().unwrap())),
        _ => {
            let releases = rows
                .into_iter()
                .map(|row| row.package_release)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            Ok(NativeLookup::Ambiguous(releases))
        }
    }
}
