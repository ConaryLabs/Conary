// conary-core/src/scriptlet/failure_policy/tests.rs

use super::*;
use crate::ccs::native_lifecycle::{RpmTriggerAction as PersistedRpmTriggerAction, RpmTriggerKind};

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
