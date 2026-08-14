// conary-core/src/ccs/native_transaction/eopkg.rs

//! eopkg transaction-wide system configuration event.

use super::{
    NativeBundleRole, NativeBundleView, NativeEventPlacement, NativeEventProgram, NativeEventStage,
    NativeTransactionChange, NativeTransactionEvent,
};
use anyhow::Result;

/// eopkg invokes `/usr/sbin/usysconf run` exactly once after every successful
/// install, upgrade, or remove transaction, never once per package.
pub(super) fn plan(
    bundles: &[NativeBundleView<'_>],
    changes: &[NativeTransactionChange],
    events: &mut Vec<NativeTransactionEvent>,
) -> Result<()> {
    let Some(owner) = bundles.iter().find(|view| {
        view.bundle.source_format.as_str() == "eopkg"
            && matches!(
                view.role,
                NativeBundleRole::Installing | NativeBundleRole::Removing
            )
            && changes
                .iter()
                .any(|change| change.package_name == view.package_name)
    }) else {
        return Ok(());
    };
    events.push(NativeTransactionEvent {
        owner_package: owner.package_name.to_string(),
        owner_version: owner.package_version.to_string(),
        owner_arch: owner.bundle.source_arch.clone(),
        source_format: "eopkg".to_string(),
        stage: NativeEventStage::EopkgSystemConfiguration,
        program: NativeEventProgram::Command {
            argv: vec!["/usr/sbin/usysconf".to_string(), "run".to_string()],
        },
        args: Vec::new(),
        stdin: Vec::new(),
        matched_targets: changes
            .iter()
            .map(|change| change.package_name.clone())
            .collect(),
        rpm_trigger_owner: None,
        deb_package_refcount: None,
        order_key: "eopkg:usysconf".to_string(),
        placement: NativeEventPlacement::TransactionAfter,
    });
    Ok(())
}
