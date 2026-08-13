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
mod tests {
    use super::*;
    use crate::ccs::native_lifecycle::{
        RpmTriggerAction as PersistedRpmTriggerAction, RpmTriggerKind,
    };

    fn rpm_class(
        rpm_class: RpmLifecycleClass,
        failure_kind: ScriptletFailureKind,
    ) -> LifecycleFailureClass {
        LifecycleFailureClass {
            source_format: SourceFormat::Rpm,
            rpm_class: Some(rpm_class),
            header_critical: false,
            failure_kind,
        }
    }

    #[test]
    fn every_rpm_package_slot_matches_the_pinned_scriptinfo_defaults() {
        // Pinned rpm lib/rpmscript.cc scriptInfo[] deflags at
        // a8f0192aee1c08bd1454ed2ac6ebaf506004b55c: prein, preun, pretrans,
        // preuntrans, verify, and sysusers carry RPMSCRIPT_FLAG_CRITICAL; post,
        // postun, posttrans, and postuntrans carry none.
        for (slot, expected) in [
            (
                RpmScriptletSlot::Pre,
                RpmClassAuthority::DefaultAbortsTransaction,
            ),
            (
                RpmScriptletSlot::PreUn,
                RpmClassAuthority::DefaultAbortsTransaction,
            ),
            (
                RpmScriptletSlot::PreTrans,
                RpmClassAuthority::DefaultAbortsTransaction,
            ),
            (
                RpmScriptletSlot::PreUnTrans,
                RpmClassAuthority::DefaultAbortsTransaction,
            ),
            (
                RpmScriptletSlot::Verify,
                RpmClassAuthority::DefaultAbortsTransaction,
            ),
            (
                RpmScriptletSlot::Sysusers,
                RpmClassAuthority::DefaultAbortsTransaction,
            ),
            (
                RpmScriptletSlot::Post,
                RpmClassAuthority::DefaultWarnAndContinue,
            ),
            (
                RpmScriptletSlot::PostUn,
                RpmClassAuthority::DefaultWarnAndContinue,
            ),
            (
                RpmScriptletSlot::PostTrans,
                RpmClassAuthority::DefaultWarnAndContinue,
            ),
            (
                RpmScriptletSlot::PostUnTrans,
                RpmClassAuthority::DefaultWarnAndContinue,
            ),
        ] {
            assert_eq!(
                rpm_package_slot_authority(slot),
                expected,
                "slot {slot:?} diverges from the pinned RPM scriptInfo default"
            );
        }
    }

    #[test]
    fn file_triggers_can_never_be_promoted_by_a_header_flag() {
        for family in [RpmTriggerFamily::File, RpmTriggerFamily::TransactionFile] {
            for action in [
                RpmTriggerAction::PreInstall,
                RpmTriggerAction::Install,
                RpmTriggerAction::Uninstall,
                RpmTriggerAction::PostUninstall,
            ] {
                let authority = rpm_trigger_authority(family, action);
                assert_eq!(authority, RpmClassAuthority::ForcedWarnAndContinue);
                for header_critical in [false, true] {
                    assert_eq!(
                        FailurePosture::for_rpm_criticality(
                            authority.effective_criticality(header_critical).persisted()
                        ),
                        FailurePosture::WarnAndContinue,
                        "a {family:?} {action:?} trigger must never fail its transaction element"
                    );
                }
            }
        }
    }

    #[test]
    fn package_triggers_follow_their_action_class() {
        for (action, expected) in [
            (
                RpmTriggerAction::PreInstall,
                RpmClassAuthority::DefaultAbortsTransaction,
            ),
            (
                RpmTriggerAction::Uninstall,
                RpmClassAuthority::DefaultAbortsTransaction,
            ),
            (
                RpmTriggerAction::Install,
                RpmClassAuthority::DefaultWarnAndContinue,
            ),
            (
                RpmTriggerAction::PostUninstall,
                RpmClassAuthority::DefaultWarnAndContinue,
            ),
        ] {
            assert_eq!(
                rpm_trigger_authority(RpmTriggerFamily::Package, action),
                expected
            );
        }
    }

    #[test]
    fn a_header_critical_flag_promotes_a_promotable_class_and_nothing_else() {
        assert_eq!(
            RpmClassAuthority::DefaultWarnAndContinue.effective_criticality(false),
            RpmScriptletCriticality::WarningOnly
        );
        assert_eq!(
            RpmClassAuthority::DefaultWarnAndContinue.effective_criticality(true),
            RpmScriptletCriticality::Header
        );
        assert_eq!(
            RpmClassAuthority::DefaultAbortsTransaction.effective_criticality(false),
            RpmScriptletCriticality::SlotDefault
        );
        assert_eq!(
            RpmClassAuthority::DefaultAbortsTransaction.effective_criticality(true),
            RpmScriptletCriticality::Header
        );
        assert_eq!(
            RpmClassAuthority::ForcedWarnAndContinue.effective_criticality(true),
            RpmScriptletCriticality::ForcedWarningOnly
        );
    }

    /// The parse side and the persisted side are the same fact. Everything the
    /// parser can stamp must cross the conversion boundary into a value the
    /// runtime admits with the identical posture.
    #[test]
    fn every_parsed_criticality_crosses_into_the_same_runtime_posture() {
        for (parsed, persisted) in [
            (RpmScriptletCriticality::Header, RpmCriticality::Header),
            (
                RpmScriptletCriticality::SlotDefault,
                RpmCriticality::SlotDefault,
            ),
            (
                RpmScriptletCriticality::WarningOnly,
                RpmCriticality::WarningOnly,
            ),
            (
                RpmScriptletCriticality::ForcedWarningOnly,
                RpmCriticality::ForcedWarningOnly,
            ),
        ] {
            assert_eq!(parsed.persisted(), persisted);
            assert_eq!(
                parsed.is_critical(),
                FailurePosture::for_rpm_criticality(persisted).aborts_transaction()
            );
        }
    }

    /// Every class the parser can produce, for every header flag value, must
    /// land on the posture the pinned table declares -- through the persisted
    /// typed class, exactly as the install runtime consults it.
    #[test]
    fn every_rpm_class_reaches_the_runtime_with_its_declared_posture() {
        let classes = [
            (RpmScriptletSlot::Pre, FailurePosture::AbortsTransaction),
            (RpmScriptletSlot::PreUn, FailurePosture::AbortsTransaction),
            (
                RpmScriptletSlot::PreTrans,
                FailurePosture::AbortsTransaction,
            ),
            (
                RpmScriptletSlot::PreUnTrans,
                FailurePosture::AbortsTransaction,
            ),
            (RpmScriptletSlot::Verify, FailurePosture::AbortsTransaction),
            (
                RpmScriptletSlot::Sysusers,
                FailurePosture::AbortsTransaction,
            ),
            (RpmScriptletSlot::Post, FailurePosture::WarnAndContinue),
            (RpmScriptletSlot::PostUn, FailurePosture::WarnAndContinue),
            (RpmScriptletSlot::PostTrans, FailurePosture::WarnAndContinue),
            (
                RpmScriptletSlot::PostUnTrans,
                FailurePosture::WarnAndContinue,
            ),
        ];
        for (slot, without_header_flag) in classes {
            let posture = |header_critical| {
                FailurePosture::for_lifecycle_failure(LifecycleFailureClass {
                    source_format: SourceFormat::Rpm,
                    rpm_class: Some(RpmLifecycleClass::PackageSlot(slot)),
                    header_critical,
                    failure_kind: ScriptletFailureKind::ScriptExited,
                })
            };
            assert_eq!(
                posture(false),
                without_header_flag,
                "slot {slot:?} reaches the runtime with the wrong posture"
            );
            assert_eq!(
                posture(true),
                FailurePosture::AbortsTransaction,
                "a header CRITICAL flag on {slot:?} must reach the runtime as fatal"
            );
        }
    }

    /// The #295 decision table, re-derived through the persisted typed class:
    /// every pinned row of the RPM Scriptlet Failure Posture spec table lands
    /// on its declared posture, with header promotion only where the table
    /// allows it.
    #[test]
    fn persisted_typed_class_matches_the_pinned_decision_table() {
        // Pinned rpm lib/rpmscript.cc scriptInfo[] deflags at
        // a8f0192aee1c08bd1454ed2ac6ebaf506004b55c.
        let package_slots = [
            (
                RpmScriptletSlot::PreTrans,
                FailurePosture::AbortsTransaction,
            ),
            (RpmScriptletSlot::Pre, FailurePosture::AbortsTransaction),
            (RpmScriptletSlot::Post, FailurePosture::WarnAndContinue),
            (RpmScriptletSlot::PostTrans, FailurePosture::WarnAndContinue),
            (
                RpmScriptletSlot::PreUnTrans,
                FailurePosture::AbortsTransaction,
            ),
            (RpmScriptletSlot::PreUn, FailurePosture::AbortsTransaction),
            (RpmScriptletSlot::PostUn, FailurePosture::WarnAndContinue),
            (
                RpmScriptletSlot::PostUnTrans,
                FailurePosture::WarnAndContinue,
            ),
            (RpmScriptletSlot::Verify, FailurePosture::AbortsTransaction),
            (
                RpmScriptletSlot::Sysusers,
                FailurePosture::AbortsTransaction,
            ),
        ];
        for (slot, expected) in package_slots {
            let class = RpmLifecycleClass::PackageSlot(slot);
            assert_eq!(
                FailurePosture::for_lifecycle_failure(LifecycleFailureClass {
                    source_format: SourceFormat::Rpm,
                    rpm_class: Some(class),
                    header_critical: false,
                    failure_kind: ScriptletFailureKind::ScriptExited,
                }),
                expected,
                "class {slot:?} without a header flag diverges from the pinned table"
            );
            assert_eq!(
                FailurePosture::for_lifecycle_failure(LifecycleFailureClass {
                    source_format: SourceFormat::Rpm,
                    rpm_class: Some(class),
                    header_critical: true,
                    failure_kind: ScriptletFailureKind::ScriptExited,
                }),
                FailurePosture::AbortsTransaction,
                "a header CRITICAL flag on class {slot:?} must promote to fatal"
            );
        }
        // Package triggers: %triggerprein and %triggerun abort; %triggerin and
        // %triggerpostun warn and continue.
        for (action, expected) in [
            (
                PersistedRpmTriggerAction::PreInstall,
                FailurePosture::AbortsTransaction,
            ),
            (
                PersistedRpmTriggerAction::Uninstall,
                FailurePosture::AbortsTransaction,
            ),
            (
                PersistedRpmTriggerAction::Install,
                FailurePosture::WarnAndContinue,
            ),
            (
                PersistedRpmTriggerAction::PostUninstall,
                FailurePosture::WarnAndContinue,
            ),
        ] {
            let class = RpmLifecycleClass::Trigger {
                kind: RpmTriggerKind::Package,
                action,
            };
            assert_eq!(
                FailurePosture::for_lifecycle_failure(LifecycleFailureClass {
                    source_format: SourceFormat::Rpm,
                    rpm_class: Some(class),
                    header_critical: false,
                    failure_kind: ScriptletFailureKind::ScriptExited,
                }),
                expected,
                "package {action:?} trigger diverges from the pinned table"
            );
        }
        // File and transaction-file triggers: CRITICAL is cleared, so no header
        // flag can promote them; they never fail their transaction element.
        for kind in [RpmTriggerKind::File, RpmTriggerKind::TransactionFile] {
            for action in [
                PersistedRpmTriggerAction::PreInstall,
                PersistedRpmTriggerAction::Install,
                PersistedRpmTriggerAction::Uninstall,
                PersistedRpmTriggerAction::PostUninstall,
            ] {
                let class = RpmLifecycleClass::Trigger { kind, action };
                for header_critical in [false, true] {
                    assert_eq!(
                        FailurePosture::for_lifecycle_failure(LifecycleFailureClass {
                            source_format: SourceFormat::Rpm,
                            rpm_class: Some(class),
                            header_critical,
                            failure_kind: ScriptletFailureKind::ScriptExited,
                        }),
                        FailurePosture::WarnAndContinue,
                        "a {kind:?} {action:?} trigger must never fail its transaction element"
                    );
                }
            }
        }
    }

    #[test]
    fn warn_and_continue_covers_only_source_program_results() {
        for failure_kind in [
            ScriptletFailureKind::ScriptExited,
            ScriptletFailureKind::ScriptTimedOut,
        ] {
            assert_eq!(
                FailurePosture::for_lifecycle_failure(rpm_class(
                    RpmLifecycleClass::PackageSlot(RpmScriptletSlot::Post),
                    failure_kind
                )),
                FailurePosture::WarnAndContinue
            );
        }
        for failure_kind in [
            ScriptletFailureKind::ContractViolation,
            ScriptletFailureKind::ProgramUnavailable,
            ScriptletFailureKind::ProcessSetupFailed,
            ScriptletFailureKind::SandboxSetupUnavailable,
        ] {
            assert_eq!(
                FailurePosture::for_lifecycle_failure(rpm_class(
                    RpmLifecycleClass::PackageSlot(RpmScriptletSlot::Post),
                    failure_kind
                )),
                FailurePosture::AbortsTransaction,
                "{failure_kind:?} is a Conary contract failure, not an RPM script result"
            );
        }
    }

    #[test]
    fn a_format_without_a_declared_class_table_aborts() {
        for source_format in [SourceFormat::Deb, SourceFormat::Arch] {
            assert_eq!(
                FailurePosture::for_lifecycle_failure(LifecycleFailureClass {
                    source_format,
                    rpm_class: None,
                    header_critical: false,
                    failure_kind: ScriptletFailureKind::ScriptExited,
                }),
                FailurePosture::AbortsTransaction
            );
        }
        assert_eq!(
            FailurePosture::for_lifecycle_failure(LifecycleFailureClass {
                source_format: SourceFormat::Rpm,
                rpm_class: None,
                header_critical: false,
                failure_kind: ScriptletFailureKind::ScriptExited,
            }),
            FailurePosture::AbortsTransaction
        );
    }
}
