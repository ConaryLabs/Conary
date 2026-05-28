// conary-core/src/ccs/convert/support_matrix.rs

use crate::ccs::convert::adapters::AdapterRegistry;
use crate::ccs::convert::blocked_classes::{BlockedClassOutcome, BlockedClassRegistry};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportMatrixEntry {
    pub id: &'static str,
    pub command: Option<&'static str>,
    pub class_id: Option<&'static str>,
    pub adapter_id: Option<&'static str>,
    pub outcome: SupportOutcome,
    pub reason_code: &'static str,
    pub source_families: &'static [&'static str],
    pub lifecycle_notes: &'static str,
    pub fixture_names: &'static [&'static str],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupportOutcome {
    Known,
    Review,
    Blocked,
}

#[derive(Debug, Clone)]
pub struct SupportMatrix {
    entries: Vec<SupportMatrixEntry>,
}

impl Default for SupportMatrix {
    fn default() -> Self {
        let mut entries = Vec::new();
        for adapter_id in AdapterRegistry::default().adapter_ids() {
            entries.push(adapter_entry(adapter_id));
        }
        for class in BlockedClassRegistry::default().classes() {
            entries.push(SupportMatrixEntry {
                id: class.id,
                command: class.command_names.first().copied(),
                class_id: Some(class.id),
                adapter_id: None,
                outcome: outcome_for_class(class.default_outcome),
                reason_code: class.reason_code,
                source_families: class.affected_formats,
                lifecycle_notes: class.description,
                fixture_names: fixture_names_for_class(class.id),
            });
        }
        assert_unique_matrix_ids(&entries);
        Self { entries }
    }
}

impl SupportMatrix {
    pub fn entries(&self) -> &[SupportMatrixEntry] {
        &self.entries
    }
}

fn adapter_entry(adapter_id: &'static str) -> SupportMatrixEntry {
    let (command, reason_code, lifecycle_notes, fixture_names): (
        Option<&'static str>,
        &'static str,
        &'static str,
        &'static [&'static str],
    ) = match adapter_id {
        "native-free/v1" => (
            None,
            "native-free-no-scriptlets",
            "Package-level evidence for packages with no scriptlet entries.",
            &["adapter-registry-native-free"] as &'static [&'static str],
        ),
        "ldconfig/v1" => (
            Some("ldconfig"),
            "known-helper-requires-adapter-coverage",
            "Dynamic linker cache helper recognized without claiming replacement.",
            &["adapter-registry-ldconfig"],
        ),
        "systemd-daemon-reload/v1" => (
            Some("systemctl daemon-reload"),
            "known-helper-requires-adapter-coverage",
            "systemd daemon-reload helper recognized without claiming replacement.",
            &["adapter-registry-systemd-daemon-reload"],
        ),
        "systemd-enable-disable/v1" => (
            Some("systemctl enable|disable"),
            "known-helper-requires-adapter-coverage",
            "systemd enable/disable helper recognized without claiming replacement.",
            &["adapter-registry-systemd-enable-disable"],
        ),
        _ => panic!("missing support matrix adapter row definition for {adapter_id}"),
    };

    SupportMatrixEntry {
        id: adapter_id,
        command,
        class_id: None,
        adapter_id: Some(adapter_id),
        outcome: SupportOutcome::Known,
        reason_code,
        source_families: &["rpm", "deb", "arch"],
        lifecycle_notes,
        fixture_names,
    }
}

fn outcome_for_class(outcome: BlockedClassOutcome) -> SupportOutcome {
    match outcome {
        BlockedClassOutcome::Review => SupportOutcome::Review,
        BlockedClassOutcome::Blocked => SupportOutcome::Blocked,
    }
}

fn fixture_names_for_class(class_id: &str) -> &'static [&'static str] {
    match class_id {
        "network" => &["blocked-class-network"],
        "package-manager-recursion" => &["blocked-class-package-manager-recursion"],
        "pam" => &["blocked-class-pam"],
        "selinux" => &["blocked-class-selinux"],
        "apparmor" => &["blocked-class-apparmor"],
        "kernel-module" => &["blocked-class-kernel-module"],
        "initramfs" => &["blocked-class-initramfs"],
        "bootloader" => &["blocked-class-bootloader"],
        "setuid-setcap" => &["blocked-class-setuid-setcap"],
        "sysctl" => &["blocked-class-sysctl"],
        "legacy-init" => &["blocked-class-legacy-init"],
        "native-abi-unpreservable" => &["blocked-class-native-abi-unpreservable"],
        "dbus-policy" => &["review-class-dbus-policy"],
        "rpm-verify" => &["review-class-rpm-verify"],
        "rpm-trigger" => &["review-class-rpm-trigger"],
        "deb-trigger" => &["review-class-deb-trigger"],
        "debconf" => &["review-class-debconf"],
        "udev" => &["review-class-udev"],
        "arch-alpm-hook" => &["review-class-arch-alpm-hook"],
        "arch-install-function" => &["review-class-arch-install-function"],
        _ => panic!("missing support matrix fixture definition for {class_id}"),
    }
}

fn assert_unique_matrix_ids(entries: &[SupportMatrixEntry]) {
    let mut seen = BTreeSet::new();
    for entry in entries {
        assert!(
            seen.insert(entry.id),
            "duplicate support matrix id: {}",
            entry.id
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::convert::adapters::AdapterRegistry;
    use crate::ccs::convert::blocked_classes::BlockedClassRegistry;

    #[test]
    fn support_matrix_covers_every_builtin_adapter() {
        let matrix = SupportMatrix::default();
        let registry = AdapterRegistry::default();

        for adapter_id in registry.adapter_ids() {
            let row = matrix
                .entries()
                .iter()
                .find(|entry| entry.adapter_id == Some(adapter_id));
            assert!(
                row.is_some(),
                "missing support matrix row for adapter {adapter_id}"
            );
            let row = row.unwrap();
            assert_eq!(row.outcome, SupportOutcome::Known);
            assert!(!row.reason_code.is_empty());
            assert!(!row.source_families.is_empty());
            assert!(!row.fixture_names.is_empty());
        }
    }

    #[test]
    fn support_matrix_covers_every_blocked_class_reason() {
        let matrix = SupportMatrix::default();
        let classes = BlockedClassRegistry::default();

        for class in classes.classes() {
            assert!(
                matrix.entries().iter().any(|entry| {
                    entry.class_id == Some(class.id) && entry.reason_code == class.reason_code
                }),
                "missing support matrix row for class {}",
                class.id
            );
        }
    }

    #[test]
    fn support_matrix_has_no_orphan_adapter_or_class_rows() {
        let matrix = SupportMatrix::default();
        let adapter_ids: std::collections::BTreeSet<_> = AdapterRegistry::default()
            .adapter_ids()
            .into_iter()
            .collect();
        let class_ids: std::collections::BTreeSet<_> = BlockedClassRegistry::default()
            .classes()
            .iter()
            .map(|class| class.id)
            .collect();

        for entry in matrix.entries() {
            if let Some(adapter_id) = entry.adapter_id {
                assert!(
                    adapter_ids.contains(adapter_id),
                    "orphan adapter row {adapter_id}"
                );
            }
            if let Some(class_id) = entry.class_id {
                assert!(class_ids.contains(class_id), "orphan class row {class_id}");
            }
        }
    }
}
