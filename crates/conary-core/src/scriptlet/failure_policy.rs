// conary-core/src/scriptlet/failure_policy.rs

//! Typed per-scriptlet-class lifecycle failure posture.
//!
//! The source package format owns its lifecycle ABI, so a lifecycle failure is
//! exactly as fatal as the source format's own contract says it is -- in both
//! directions. This module is the single owner of that table. Conversion
//! consults it to stamp an entry's effective criticality; the install runtime
//! consults it to decide whether a failed entry aborts its transaction.
//!
//! The declared authority is
//! `docs/specs/foreign-package-lifecycle-contracts.md`, "RPM Scriptlet Failure
//! Posture". It is derived from pinned RPM
//! [`lib/rpmscript.cc` `scriptInfo[]`](https://github.com/rpm-software-management/rpm/blob/a8f0192aee1c08bd1454ed2ac6ebaf506004b55c/lib/rpmscript.cc#L54-L99)
//! `deflags`, the file-trigger clear at
//! [`rpmScriptFromTriggerTag()`](https://github.com/rpm-software-management/rpm/blob/a8f0192aee1c08bd1454ed2ac6ebaf506004b55c/lib/rpmscript.cc#L689-L692),
//! and the dispatch in
//! [`runScript()`](https://github.com/rpm-software-management/rpm/blob/a8f0192aee1c08bd1454ed2ac6ebaf506004b55c/lib/transaction.cc#L1709-L1762).
//!
//! Nothing here matches a scriptlet name. The class is a typed value on both
//! sides of the persisted contract.

use super::ScriptletFailureKind;
use crate::ccs::native_lifecycle::{
    RpmCriticality, RpmLifecycleClass, RpmTriggerAction as PersistedRpmTriggerAction,
    RpmTriggerKind, SourceFormat,
};
use crate::packages::native_abi::{
    RpmScriptletCriticality, RpmScriptletSlot, RpmTriggerAction, RpmTriggerFamily,
};

/// What a lifecycle entry's failure does to the transaction that ran it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailurePosture {
    /// The transaction fails. The source format itself refuses to proceed past
    /// this class of failure, so Conary must not be more permissive.
    AbortsTransaction,
    /// The failure is reported as typed evidence and the transaction proceeds,
    /// because the source format proceeds. Conary's promised-path
    /// post-condition remains the backstop for a dependency witness that a
    /// continued failure would otherwise leave unsatisfied.
    WarnAndContinue,
}

/// The per-class cell of a source format's failure table, before the package
/// header is consulted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpmClassAuthority {
    /// The class default is warn-and-continue; a header `CRITICAL` flag on this
    /// entry promotes it to a transaction failure.
    DefaultWarnAndContinue,
    /// The class default is a transaction failure.
    DefaultAbortsTransaction,
    /// RPM clears `CRITICAL` for this class, so no header flag can promote it.
    ForcedWarnAndContinue,
}

impl RpmClassAuthority {
    /// The effective criticality RPM would stamp on an entry of this class
    /// whose header carried (or did not carry) the `CRITICAL` flag.
    pub const fn effective_criticality(self, header_critical: bool) -> RpmScriptletCriticality {
        match self {
            Self::ForcedWarnAndContinue => RpmScriptletCriticality::ForcedWarningOnly,
            Self::DefaultWarnAndContinue | Self::DefaultAbortsTransaction if header_critical => {
                RpmScriptletCriticality::Header
            }
            Self::DefaultWarnAndContinue => RpmScriptletCriticality::WarningOnly,
            Self::DefaultAbortsTransaction => RpmScriptletCriticality::SlotDefault,
        }
    }
}

impl RpmScriptletCriticality {
    /// Project the parsed criticality onto the persisted CCS contract value.
    ///
    /// The two enums are the same fact on either side of the conversion
    /// boundary; this is the only place that crosses it.
    pub const fn persisted(self) -> RpmCriticality {
        match self {
            Self::Header => RpmCriticality::Header,
            Self::SlotDefault => RpmCriticality::SlotDefault,
            Self::WarningOnly => RpmCriticality::WarningOnly,
            Self::ForcedWarningOnly => RpmCriticality::ForcedWarningOnly,
        }
    }

    /// Whether RPM treats a failure of an entry stamped with this criticality
    /// as fatal to its transaction element.
    pub const fn is_critical(self) -> bool {
        FailurePosture::for_rpm_criticality(self.persisted()).aborts_transaction()
    }
}

/// RPM's per-scriptlet-class default, from the pinned `scriptInfo[]` table.
pub const fn rpm_package_slot_authority(slot: RpmScriptletSlot) -> RpmClassAuthority {
    match slot {
        RpmScriptletSlot::Pre
        | RpmScriptletSlot::PreUn
        | RpmScriptletSlot::PreTrans
        | RpmScriptletSlot::PreUnTrans
        | RpmScriptletSlot::Verify
        | RpmScriptletSlot::Sysusers => RpmClassAuthority::DefaultAbortsTransaction,
        RpmScriptletSlot::Post
        | RpmScriptletSlot::PostUn
        | RpmScriptletSlot::PostTrans
        | RpmScriptletSlot::PostUnTrans
        | RpmScriptletSlot::Trigger => RpmClassAuthority::DefaultWarnAndContinue,
    }
}

/// RPM's per-trigger-class default. Package triggers keep the `scriptInfo[]`
/// default for their action; file and transaction-file triggers have `CRITICAL`
/// cleared unconditionally and never fail their transaction element.
pub const fn rpm_trigger_authority(
    family: RpmTriggerFamily,
    action: RpmTriggerAction,
) -> RpmClassAuthority {
    match family {
        RpmTriggerFamily::File | RpmTriggerFamily::TransactionFile => {
            RpmClassAuthority::ForcedWarnAndContinue
        }
        RpmTriggerFamily::Package => match action {
            RpmTriggerAction::PreInstall | RpmTriggerAction::Uninstall => {
                RpmClassAuthority::DefaultAbortsTransaction
            }
            RpmTriggerAction::Install
            | RpmTriggerAction::PostUninstall
            | RpmTriggerAction::Unknown { .. } => RpmClassAuthority::DefaultWarnAndContinue,
        },
    }
}

impl From<RpmTriggerKind> for RpmTriggerFamily {
    fn from(kind: RpmTriggerKind) -> Self {
        match kind {
            RpmTriggerKind::Package => Self::Package,
            RpmTriggerKind::File => Self::File,
            RpmTriggerKind::TransactionFile => Self::TransactionFile,
        }
    }
}

impl From<PersistedRpmTriggerAction> for RpmTriggerAction {
    fn from(action: PersistedRpmTriggerAction) -> Self {
        match action {
            PersistedRpmTriggerAction::PreInstall => Self::PreInstall,
            PersistedRpmTriggerAction::Install => Self::Install,
            PersistedRpmTriggerAction::Uninstall => Self::Uninstall,
            PersistedRpmTriggerAction::PostUninstall => Self::PostUninstall,
        }
    }
}

impl RpmLifecycleClass {
    /// The effective criticality this persisted class authority stamps for a
    /// persisted header flag word carrying (or not carrying) the CRITICAL bit.
    ///
    /// This is the single derivation shared by contract validation and the
    /// install runtime: both consult the current table through the typed
    /// class, so a posture correction applies to already-converted artifacts
    /// without reconversion.
    pub fn effective_criticality(self, header_critical: bool) -> RpmCriticality {
        let authority = match self {
            Self::PackageSlot(slot) => rpm_package_slot_authority(slot),
            Self::Trigger { kind, action } => rpm_trigger_authority(kind.into(), action.into()),
        };
        authority.effective_criticality(header_critical).persisted()
    }
}

/// The typed inputs the runtime has when a native lifecycle entry fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LifecycleFailureClass {
    /// The source format whose lifecycle contract owns this entry.
    pub source_format: SourceFormat,
    /// The persisted typed RPM class of the entry, or `None` for an entry
    /// that carries no declared RPM class.
    pub rpm_class: Option<RpmLifecycleClass>,
    /// Whether the persisted raw scriptlet flag word carries RPM's CRITICAL
    /// bit, re-derived from `RpmRuntimeMetadata::raw_flags`.
    pub header_critical: bool,
    /// How the entry failed.
    pub failure_kind: ScriptletFailureKind,
}

impl FailurePosture {
    /// The posture the install runtime applies to a failed lifecycle entry.
    ///
    /// A source format's warn-and-continue class covers only a program exit or
    /// timeout, and only after exact preflight and the selected-root
    /// enforcement boundary have already succeeded. Missing interpreters,
    /// malformed contracts, and process or sandbox setup failures are Conary
    /// contract failures rather than source-program results, so they abort
    /// whatever the class says: criticality is not a security-boundary bypass.
    pub fn for_lifecycle_failure(class: LifecycleFailureClass) -> Self {
        if !class.failure_kind.is_source_program_result() {
            return Self::AbortsTransaction;
        }
        match class.source_format {
            SourceFormat::Rpm => match class.rpm_class {
                Some(rpm_class) => Self::for_rpm_criticality(
                    rpm_class.effective_criticality(class.header_critical),
                ),
                // An RPM entry with no persisted typed class has no declared
                // class, so it gets the strict posture.
                None => Self::AbortsTransaction,
            },
            // dpkg leaves a package whose maintainer script failed in an
            // errored, unconfigured state rather than proceeding, and libalpm
            // has no warning-only class. Both are transaction failures until
            // their own slice declares a class table.
            SourceFormat::Deb | SourceFormat::Arch | SourceFormat::Eopkg => Self::AbortsTransaction,
        }
    }

    /// The posture carried by an RPM entry's persisted effective criticality.
    pub const fn for_rpm_criticality(criticality: RpmCriticality) -> Self {
        match criticality {
            RpmCriticality::Header | RpmCriticality::SlotDefault => Self::AbortsTransaction,
            RpmCriticality::WarningOnly | RpmCriticality::ForcedWarningOnly => {
                Self::WarnAndContinue
            }
        }
    }

    pub const fn aborts_transaction(self) -> bool {
        matches!(self, Self::AbortsTransaction)
    }
}

#[cfg(test)]
mod tests;
