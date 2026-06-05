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
        "ldconfig/v2" => (
            Some("ldconfig"),
            "helper-complete-ldconfig",
            "Simple dynamic linker cache refresh forms are complete passive replacement evidence.",
            &["adapter-registry-ldconfig"],
        ),
        "systemd-daemon-reload/v2" => (
            Some("systemctl daemon-reload"),
            "helper-complete-systemd-daemon-reload",
            "systemd daemon-reload reloads unit definitions without changing service state.",
            &["adapter-registry-systemd-daemon-reload"],
        ),
        "systemd-unit-state/v1" => (
            Some("systemctl enable|disable|preset"),
            "helper-complete-systemd-unit-state",
            "systemd unit enable/disable/preset is complete only when package payload ships every referenced unit.",
            &["adapter-registry-systemd-unit-state"],
        ),
        "systemd-tmpfiles-create/v1" => (
            Some("systemd-tmpfiles --create"),
            "helper-complete-tmpfiles-create",
            "systemd tmpfiles create is complete only when every explicit config path is shipped by the package.",
            &["adapter-tmpfiles-create"],
        ),
        "systemd-sysusers/v1" => (
            Some("systemd-sysusers"),
            "helper-complete-sysusers",
            "systemd sysusers is complete only when every explicit config path is shipped by the package.",
            &["adapter-sysusers"],
        ),
        "alternatives-registration/v1" => (
            Some("update-alternatives|alternatives"),
            "helper-complete-alternatives-registration",
            "Alternatives install/remove registration is complete when the command shape is parseable and non-interactive.",
            &["adapter-alternatives-registration"],
        ),
        "cache-refresh/v1" => (
            Some("cache refresh helpers"),
            "helper-complete-cache-refresh",
            "Known cache refresh helpers are complete only with payload-backed cache input evidence.",
            &["adapter-cache-refresh"],
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
        "ldconfig-nonstandard" => &["review-class-ldconfig-nonstandard"],
        "systemd-runtime-action" => &["review-class-systemd-runtime-action"],
        "systemd-user-scope" => &["review-class-systemd-user-scope"],
        "deb-systemd-helper" => &["review-class-deb-systemd-helper"],
        "tmpfiles-noncreate" => &["review-class-tmpfiles-noncreate"],
        "sysusers-nonstandard" => &["review-class-sysusers-nonstandard"],
        "gconf-schema" => &["review-class-gconf-schema"],
        "install-info" => &["review-class-install-info"],
        "alternatives-interactive-or-broad" => &["review-class-alternatives-interactive-or-broad"],
        "cache-refresh-nonstandard" => &["review-class-cache-refresh-nonstandard"],
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
    use crate::ccs::convert::golden_fixtures;

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

    #[test]
    fn support_matrix_fixture_names_have_declared_golden_cases() {
        let fixtures = golden_fixtures::declared_fixture_ids();

        for entry in SupportMatrix::default().entries() {
            for fixture_name in entry.fixture_names {
                assert!(
                    fixtures.contains(fixture_name),
                    "support-matrix fixture {fixture_name} has no golden case"
                );
            }
        }
    }

    #[test]
    fn public_ready_adapter_rows_have_golden_fixture_evidence() {
        let fixtures: std::collections::BTreeMap<_, _> = golden_fixtures::all_cases()
            .iter()
            .map(|case| (case.id, *case))
            .collect();

        for entry in SupportMatrix::default()
            .entries()
            .iter()
            .filter(|entry| entry.outcome == SupportOutcome::Known)
        {
            let adapter_id = entry
                .adapter_id
                .unwrap_or_else(|| panic!("known support row {} has no adapter id", entry.id));
            assert!(
                !entry.fixture_names.is_empty(),
                "adapter {adapter_id} has no golden fixture evidence"
            );

            for fixture_name in entry.fixture_names {
                let fixture = fixtures.get(fixture_name).unwrap_or_else(|| {
                    panic!("adapter {adapter_id} fixture {fixture_name} is not declared")
                });
                let expected = if adapter_id == "native-free/v1" {
                    golden_fixtures::GoldenFixtureOutcome::NativeFree
                } else {
                    golden_fixtures::GoldenFixtureOutcome::FullyReplaced
                };
                assert_eq!(
                    fixture.expected_outcome, expected,
                    "adapter {adapter_id} fixture {fixture_name} has wrong public-ready outcome"
                );
                assert!(
                    fixture.source_distro_id.is_some() && fixture.target_distro_id.is_some(),
                    "public-ready fixture {fixture_name} must use exact source and target distro ids"
                );
            }
        }
    }

    #[test]
    fn public_ready_golden_fixtures_are_backed_by_adapter_or_native_free_evidence() {
        let matrix = SupportMatrix::default();
        let known_fixture_authority: std::collections::BTreeMap<_, _> = matrix
            .entries()
            .iter()
            .filter(|entry| entry.outcome == SupportOutcome::Known)
            .flat_map(|entry| {
                entry.fixture_names.iter().map(move |fixture| {
                    (
                        *fixture,
                        entry
                            .adapter_id
                            .expect("known support row should carry adapter id"),
                    )
                })
            })
            .collect();

        for fixture in golden_fixtures::all_cases().iter().filter(|case| {
            matches!(
                case.expected_outcome,
                golden_fixtures::GoldenFixtureOutcome::NativeFree
                    | golden_fixtures::GoldenFixtureOutcome::FullyReplaced
            )
        }) {
            let adapter_id = known_fixture_authority.get(fixture.id).unwrap_or_else(|| {
                panic!(
                    "public-ready fixture {} has no adapter or native-free support-matrix evidence",
                    fixture.id
                )
            });
            if fixture.expected_outcome == golden_fixtures::GoldenFixtureOutcome::NativeFree {
                assert_eq!(
                    *adapter_id, "native-free/v1",
                    "native-free fixture {} must be backed by explicit native-free evidence",
                    fixture.id
                );
            } else {
                assert_ne!(
                    *adapter_id, "native-free/v1",
                    "fully-replaced fixture {} must be backed by adapter evidence",
                    fixture.id
                );
            }
        }
    }

    #[test]
    fn review_and_blocked_support_rows_have_stable_reason_fixture_alignment() {
        for entry in SupportMatrix::default()
            .entries()
            .iter()
            .filter(|entry| entry.outcome != SupportOutcome::Known)
        {
            let (reason_prefix, fixture_prefix) = match entry.outcome {
                SupportOutcome::Review => ("review-class-", "review-class-"),
                SupportOutcome::Blocked => ("blocked-class-", "blocked-class-"),
                SupportOutcome::Known => unreachable!("known rows filtered out"),
            };
            assert!(
                entry.reason_code.starts_with(reason_prefix),
                "support row {} has unstable reason id {}",
                entry.id,
                entry.reason_code
            );
            assert!(
                !entry.fixture_names.is_empty(),
                "support row {} has no golden fixture evidence",
                entry.id
            );
            for fixture_name in entry.fixture_names {
                assert!(
                    fixture_name.starts_with(fixture_prefix),
                    "support row {} fixture {} does not match {} outcome",
                    entry.id,
                    fixture_name,
                    fixture_prefix
                );
            }
        }
    }

    #[test]
    fn goal8_required_corpus_rows_are_declared() {
        let fixtures: std::collections::BTreeMap<_, _> = golden_fixtures::required_goal8_cases()
            .iter()
            .map(|case| (case.id, case.expected_outcome))
            .collect();

        for (fixture_id, expected_outcome) in [
            (
                "adapter-registry-native-free",
                golden_fixtures::GoldenFixtureOutcome::NativeFree,
            ),
            (
                "adapter-sysusers",
                golden_fixtures::GoldenFixtureOutcome::FullyReplaced,
            ),
            (
                "adapter-registry-systemd-daemon-reload",
                golden_fixtures::GoldenFixtureOutcome::FullyReplaced,
            ),
            (
                "adapter-registry-systemd-unit-state",
                golden_fixtures::GoldenFixtureOutcome::FullyReplaced,
            ),
            (
                "adapter-tmpfiles-create",
                golden_fixtures::GoldenFixtureOutcome::FullyReplaced,
            ),
            (
                "adapter-registry-ldconfig",
                golden_fixtures::GoldenFixtureOutcome::FullyReplaced,
            ),
            (
                "adapter-cache-refresh",
                golden_fixtures::GoldenFixtureOutcome::FullyReplaced,
            ),
            (
                "adapter-alternatives-registration",
                golden_fixtures::GoldenFixtureOutcome::FullyReplaced,
            ),
            (
                "legacy-replay-unknown-shell",
                golden_fixtures::GoldenFixtureOutcome::LegacyReplay,
            ),
            (
                "blocked-class-package-manager-recursion",
                golden_fixtures::GoldenFixtureOutcome::Blocked,
            ),
            (
                "legacy-replay-foreign-replay-rejected",
                golden_fixtures::GoldenFixtureOutcome::Rejected,
            ),
            (
                "review-class-rpm-trigger",
                golden_fixtures::GoldenFixtureOutcome::ReviewRequired,
            ),
            (
                "review-class-deb-trigger",
                golden_fixtures::GoldenFixtureOutcome::ReviewRequired,
            ),
            (
                "review-class-arch-install-function",
                golden_fixtures::GoldenFixtureOutcome::ReviewRequired,
            ),
        ] {
            assert_eq!(
                fixtures.get(fixture_id).copied(),
                Some(expected_outcome),
                "Goal 8 required corpus fixture {fixture_id} is missing or has the wrong outcome"
            );
        }
    }
}
