// conary-core/src/packages/native_scriptlet_support.rs

//! Code-owned native package-manager scriptlet support matrix.
//!
//! This module tracks upstream script/control surfaces that Conary parsers
//! must preserve and hand to CCS conversion instead of dropping silently.

use crate::packages::native_abi::{
    NativeLifecyclePath, NativeScriptletEntry, NativeScriptletFormat, NativeScriptletKind,
    NativeScriptletSupport,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeScriptletSupportRow {
    pub format: NativeScriptletFormat,
    pub upstream_reference: &'static str,
    pub upstream_surface: &'static str,
    pub kind: NativeScriptletKind,
    pub slot: NativeSlotPattern,
    pub primary_lifecycle: NativeLifecyclePath,
    pub support: NativeSupportExpectation,
}

impl NativeScriptletSupportRow {
    pub fn matches_entry(&self, entry: &NativeScriptletEntry) -> bool {
        self.format == entry.format && self.slot.matches(entry.native_slot.as_str())
    }

    pub fn slot_label(&self) -> &'static str {
        self.slot.label()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeSlotPattern {
    Exact(&'static str),
    Prefix(&'static str),
}

impl NativeSlotPattern {
    pub fn matches(&self, slot: &str) -> bool {
        match self {
            Self::Exact(expected) => slot == *expected,
            Self::Prefix(prefix) => slot.starts_with(prefix),
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Exact(slot) | Self::Prefix(slot) => slot,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeSupportExpectation {
    Parsed,
    DeferredReview(&'static str),
    Unpreservable(&'static str),
}

impl NativeSupportExpectation {
    pub fn matches(&self, support: &NativeScriptletSupport) -> bool {
        match (self, support) {
            (Self::Parsed, NativeScriptletSupport::Parsed) => true,
            (
                Self::DeferredReview(expected),
                NativeScriptletSupport::DeferredReview { reason_code },
            ) => reason_code == expected,
            (
                Self::Unpreservable(expected),
                NativeScriptletSupport::Unpreservable { reason_code },
            ) => reason_code == expected,
            _ => false,
        }
    }
}

pub fn upstream_native_scriptlet_support_rows() -> &'static [NativeScriptletSupportRow] {
    UPSTREAM_NATIVE_SCRIPTLET_SUPPORT_ROWS
}

const UPSTREAM_NATIVE_SCRIPTLET_SUPPORT_ROWS: &[NativeScriptletSupportRow] = &[
    rpm_script("%pre", NativeLifecyclePath::PreInstall),
    rpm_script("%post", NativeLifecyclePath::PostInstall),
    rpm_script("%preun", NativeLifecyclePath::PreRemove),
    rpm_script("%postun", NativeLifecyclePath::PostRemove),
    rpm_script("%pretrans", NativeLifecyclePath::PreTransaction),
    rpm_script("%posttrans", NativeLifecyclePath::PostTransaction),
    rpm_script("%preuntrans", NativeLifecyclePath::PreUntransaction),
    rpm_script("%postuntrans", NativeLifecyclePath::PostUntransaction),
    NativeScriptletSupportRow {
        format: NativeScriptletFormat::Rpm,
        upstream_reference: "rpm-scriptlets(7)",
        upstream_surface: "%verify",
        kind: NativeScriptletKind::Executable,
        slot: NativeSlotPattern::Exact("%verify"),
        primary_lifecycle: NativeLifecyclePath::Verify,
        support: NativeSupportExpectation::DeferredReview("rpm-verify-scriptlet-deferred"),
    },
    rpm_trigger(
        "%triggerprein",
        NativeLifecyclePath::Trigger,
        "rpm-trigger-semantics-deferred",
    ),
    rpm_trigger(
        "%triggerin",
        NativeLifecyclePath::Trigger,
        "rpm-trigger-semantics-deferred",
    ),
    rpm_trigger(
        "%triggerun",
        NativeLifecyclePath::Trigger,
        "rpm-trigger-semantics-deferred",
    ),
    rpm_trigger(
        "%triggerpostun",
        NativeLifecyclePath::Trigger,
        "rpm-trigger-semantics-deferred",
    ),
    rpm_trigger(
        "%filetriggerin",
        NativeLifecyclePath::FileTrigger,
        "rpm-file-trigger-semantics-deferred",
    ),
    rpm_trigger(
        "%filetriggerun",
        NativeLifecyclePath::FileTrigger,
        "rpm-file-trigger-semantics-deferred",
    ),
    rpm_trigger(
        "%filetriggerpostun",
        NativeLifecyclePath::FileTrigger,
        "rpm-file-trigger-semantics-deferred",
    ),
    rpm_trigger(
        "%transfiletriggerin",
        NativeLifecyclePath::TransactionFileTrigger,
        "rpm-trans-file-trigger-semantics-deferred",
    ),
    rpm_trigger(
        "%transfiletriggerun",
        NativeLifecyclePath::TransactionFileTrigger,
        "rpm-trans-file-trigger-semantics-deferred",
    ),
    rpm_trigger(
        "%transfiletriggerpostun",
        NativeLifecyclePath::TransactionFileTrigger,
        "rpm-trans-file-trigger-semantics-deferred",
    ),
    deb_script("config", NativeLifecyclePath::Config),
    deb_script("preinst", NativeLifecyclePath::PreInstall),
    deb_script("postinst", NativeLifecyclePath::PostInstall),
    deb_script("prerm", NativeLifecyclePath::PreRemove),
    deb_script("postrm", NativeLifecyclePath::PostRemove),
    NativeScriptletSupportRow {
        format: NativeScriptletFormat::Deb,
        upstream_reference: "Debian Policy maintainer scripts and deb-triggers(5)",
        upstream_surface: "triggers",
        kind: NativeScriptletKind::ControlArtifact,
        slot: NativeSlotPattern::Exact("triggers"),
        primary_lifecycle: NativeLifecyclePath::Trigger,
        support: NativeSupportExpectation::DeferredReview("deb-trigger-semantics-deferred"),
    },
    arch_install("pre_install", NativeLifecyclePath::PreInstall),
    arch_install("post_install", NativeLifecyclePath::PostInstall),
    arch_install("pre_upgrade", NativeLifecyclePath::PreUpgrade),
    arch_install("post_upgrade", NativeLifecyclePath::PostUpgrade),
    arch_install("pre_remove", NativeLifecyclePath::PreRemove),
    arch_install("post_remove", NativeLifecyclePath::PostRemove),
    NativeScriptletSupportRow {
        format: NativeScriptletFormat::Arch,
        upstream_reference: "alpm-hooks(5)",
        upstream_surface: "packaged ALPM hook",
        kind: NativeScriptletKind::ControlArtifact,
        slot: NativeSlotPattern::Prefix("alpm-hook:"),
        primary_lifecycle: NativeLifecyclePath::Trigger,
        support: NativeSupportExpectation::DeferredReview("arch-alpm-hook-semantics-deferred"),
    },
];

const fn rpm_script(
    slot: &'static str,
    primary_lifecycle: NativeLifecyclePath,
) -> NativeScriptletSupportRow {
    NativeScriptletSupportRow {
        format: NativeScriptletFormat::Rpm,
        upstream_reference: "rpm-scriptlets(7)",
        upstream_surface: slot,
        kind: NativeScriptletKind::Executable,
        slot: NativeSlotPattern::Exact(slot),
        primary_lifecycle,
        support: NativeSupportExpectation::Parsed,
    }
}

const fn rpm_trigger(
    slot: &'static str,
    primary_lifecycle: NativeLifecyclePath,
    reason_code: &'static str,
) -> NativeScriptletSupportRow {
    NativeScriptletSupportRow {
        format: NativeScriptletFormat::Rpm,
        upstream_reference: "rpm-scriptlets(7)",
        upstream_surface: slot,
        kind: NativeScriptletKind::Executable,
        slot: NativeSlotPattern::Exact(slot),
        primary_lifecycle,
        support: NativeSupportExpectation::DeferredReview(reason_code),
    }
}

const fn deb_script(
    slot: &'static str,
    primary_lifecycle: NativeLifecyclePath,
) -> NativeScriptletSupportRow {
    NativeScriptletSupportRow {
        format: NativeScriptletFormat::Deb,
        upstream_reference: "Debian Policy maintainer scripts",
        upstream_surface: slot,
        kind: NativeScriptletKind::Executable,
        slot: NativeSlotPattern::Exact(slot),
        primary_lifecycle,
        support: NativeSupportExpectation::Parsed,
    }
}

const fn arch_install(
    slot: &'static str,
    primary_lifecycle: NativeLifecyclePath,
) -> NativeScriptletSupportRow {
    NativeScriptletSupportRow {
        format: NativeScriptletFormat::Arch,
        upstream_reference: "alpm-install-scriptlet(5)",
        upstream_surface: slot,
        kind: NativeScriptletKind::Executable,
        slot: NativeSlotPattern::Exact(slot),
        primary_lifecycle,
        support: NativeSupportExpectation::Parsed,
    }
}
