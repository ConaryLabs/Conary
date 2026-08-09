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
use crate::ccs::native_lifecycle::{RpmCriticality, SourceFormat};
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

/// The typed inputs the runtime has when a native lifecycle entry fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LifecycleFailureClass {
    /// The source format whose lifecycle contract owns this entry.
    pub source_format: SourceFormat,
    /// The persisted effective criticality of an RPM entry, or `None` for an
    /// entry that declares no RPM runtime contract.
    pub rpm_criticality: Option<RpmCriticality>,
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
    pub const fn for_lifecycle_failure(class: LifecycleFailureClass) -> Self {
        if !class.failure_kind.is_source_program_result() {
            return Self::AbortsTransaction;
        }
        match class.source_format {
            SourceFormat::Rpm => match class.rpm_criticality {
                Some(criticality) => Self::for_rpm_criticality(criticality),
                // An RPM entry with no persisted RPM runtime contract has no
                // declared class, so it gets the strict posture.
                None => Self::AbortsTransaction,
            },
            // dpkg leaves a package whose maintainer script failed in an
            // errored, unconfigured state rather than proceeding, and libalpm
            // has no warning-only class. Both are transaction failures until
            // their own slice declares a class table.
            SourceFormat::Deb | SourceFormat::Arch => Self::AbortsTransaction,
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

    fn rpm_class(
        criticality: RpmCriticality,
        failure_kind: ScriptletFailureKind,
    ) -> LifecycleFailureClass {
        LifecycleFailureClass {
            source_format: SourceFormat::Rpm,
            rpm_criticality: Some(criticality),
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
    /// land on the posture the pinned table declares.
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
            let authority = rpm_package_slot_authority(slot);
            let posture = |header_critical| {
                FailurePosture::for_lifecycle_failure(LifecycleFailureClass {
                    source_format: SourceFormat::Rpm,
                    rpm_criticality: Some(
                        authority.effective_criticality(header_critical).persisted(),
                    ),
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

    #[test]
    fn warn_and_continue_covers_only_source_program_results() {
        for failure_kind in [
            ScriptletFailureKind::ScriptExited,
            ScriptletFailureKind::ScriptTimedOut,
        ] {
            assert_eq!(
                FailurePosture::for_lifecycle_failure(rpm_class(
                    RpmCriticality::WarningOnly,
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
                    RpmCriticality::WarningOnly,
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
                    rpm_criticality: None,
                    failure_kind: ScriptletFailureKind::ScriptExited,
                }),
                FailurePosture::AbortsTransaction
            );
        }
        assert_eq!(
            FailurePosture::for_lifecycle_failure(LifecycleFailureClass {
                source_format: SourceFormat::Rpm,
                rpm_criticality: None,
                failure_kind: ScriptletFailureKind::ScriptExited,
            }),
            FailurePosture::AbortsTransaction
        );
    }
}
