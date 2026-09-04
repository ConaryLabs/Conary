// apps/conary/src/commands/install/native_events/identity.rs

use anyhow::Result;
use conary_core::ccs::native_lifecycle::SourceFormat;
use conary_core::ccs::native_transaction::NativePackageIdentity;
use conary_core::db::models::InstalledNativeLifecycleBundle;

use super::NativeBundleOwner;

pub(super) fn owner_identity(owner: &NativeBundleOwner) -> NativePackageIdentity {
    NativePackageIdentity::new(
        &owner.package_name,
        &owner.package_version,
        owner.bundle.source_arch.as_deref(),
    )
}

pub(in crate::commands::install) fn deb_identity_for_trove(
    conn: &rusqlite::Connection,
    trove_id: i64,
) -> Result<Option<NativePackageIdentity>> {
    let Some(installed) = InstalledNativeLifecycleBundle::find_by_trove(conn, trove_id)? else {
        return Ok(None);
    };
    let bundle = installed.bundle()?;
    Ok((bundle.source_format == SourceFormat::Deb).then(|| {
        NativePackageIdentity::new(
            &installed.source_package,
            &installed.source_version,
            installed.source_arch.as_deref(),
        )
    }))
}
