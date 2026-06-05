// conary-core/src/ccs/convert/golden_fixtures.rs

use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::ccs::convert) enum GoldenFixtureOutcome {
    NativeFree,
    FullyReplaced,
    LegacyReplay,
    ReviewRequired,
    Blocked,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::ccs::convert) struct GoldenFixtureCase {
    pub id: &'static str,
    pub expected_outcome: GoldenFixtureOutcome,
}

pub(in crate::ccs::convert) fn declared_fixture_ids() -> BTreeSet<&'static str> {
    ALL_GOLDEN_FIXTURE_CASES
        .iter()
        .map(|case| case.id)
        .collect()
}

pub(in crate::ccs::convert) fn required_goal8_cases() -> &'static [GoldenFixtureCase] {
    REQUIRED_GOAL8_CASES
}

const REQUIRED_GOAL8_CASES: &[GoldenFixtureCase] = &[
    fixture(
        "adapter-registry-native-free",
        GoldenFixtureOutcome::NativeFree,
    ),
    fixture("adapter-sysusers", GoldenFixtureOutcome::FullyReplaced),
    fixture(
        "adapter-registry-systemd-daemon-reload",
        GoldenFixtureOutcome::FullyReplaced,
    ),
    fixture(
        "adapter-registry-systemd-unit-state",
        GoldenFixtureOutcome::FullyReplaced,
    ),
    fixture(
        "adapter-tmpfiles-create",
        GoldenFixtureOutcome::FullyReplaced,
    ),
    fixture(
        "adapter-registry-ldconfig",
        GoldenFixtureOutcome::FullyReplaced,
    ),
    fixture("adapter-cache-refresh", GoldenFixtureOutcome::FullyReplaced),
    fixture(
        "adapter-alternatives-registration",
        GoldenFixtureOutcome::FullyReplaced,
    ),
    fixture(
        "legacy-replay-unknown-shell",
        GoldenFixtureOutcome::LegacyReplay,
    ),
    fixture(
        "blocked-class-package-manager-recursion",
        GoldenFixtureOutcome::Blocked,
    ),
    fixture(
        "legacy-replay-foreign-replay-rejected",
        GoldenFixtureOutcome::Rejected,
    ),
    fixture(
        "review-class-rpm-trigger",
        GoldenFixtureOutcome::ReviewRequired,
    ),
    fixture(
        "review-class-deb-trigger",
        GoldenFixtureOutcome::ReviewRequired,
    ),
    fixture(
        "review-class-arch-install-function",
        GoldenFixtureOutcome::ReviewRequired,
    ),
];

const ALL_GOLDEN_FIXTURE_CASES: &[GoldenFixtureCase] = &[
    fixture(
        "adapter-registry-native-free",
        GoldenFixtureOutcome::NativeFree,
    ),
    fixture(
        "adapter-registry-ldconfig",
        GoldenFixtureOutcome::FullyReplaced,
    ),
    fixture(
        "adapter-registry-systemd-daemon-reload",
        GoldenFixtureOutcome::FullyReplaced,
    ),
    fixture(
        "adapter-registry-systemd-unit-state",
        GoldenFixtureOutcome::FullyReplaced,
    ),
    fixture(
        "adapter-tmpfiles-create",
        GoldenFixtureOutcome::FullyReplaced,
    ),
    fixture("adapter-sysusers", GoldenFixtureOutcome::FullyReplaced),
    fixture(
        "adapter-alternatives-registration",
        GoldenFixtureOutcome::FullyReplaced,
    ),
    fixture("adapter-cache-refresh", GoldenFixtureOutcome::FullyReplaced),
    fixture("blocked-class-network", GoldenFixtureOutcome::Blocked),
    fixture(
        "blocked-class-package-manager-recursion",
        GoldenFixtureOutcome::Blocked,
    ),
    fixture("blocked-class-pam", GoldenFixtureOutcome::Blocked),
    fixture("blocked-class-selinux", GoldenFixtureOutcome::Blocked),
    fixture("blocked-class-apparmor", GoldenFixtureOutcome::Blocked),
    fixture("blocked-class-kernel-module", GoldenFixtureOutcome::Blocked),
    fixture("blocked-class-initramfs", GoldenFixtureOutcome::Blocked),
    fixture("blocked-class-bootloader", GoldenFixtureOutcome::Blocked),
    fixture("blocked-class-setuid-setcap", GoldenFixtureOutcome::Blocked),
    fixture("blocked-class-sysctl", GoldenFixtureOutcome::Blocked),
    fixture("blocked-class-legacy-init", GoldenFixtureOutcome::Blocked),
    fixture(
        "blocked-class-native-abi-unpreservable",
        GoldenFixtureOutcome::Blocked,
    ),
    fixture(
        "review-class-dbus-policy",
        GoldenFixtureOutcome::ReviewRequired,
    ),
    fixture(
        "review-class-ldconfig-nonstandard",
        GoldenFixtureOutcome::ReviewRequired,
    ),
    fixture(
        "review-class-systemd-runtime-action",
        GoldenFixtureOutcome::ReviewRequired,
    ),
    fixture(
        "review-class-systemd-user-scope",
        GoldenFixtureOutcome::ReviewRequired,
    ),
    fixture(
        "review-class-deb-systemd-helper",
        GoldenFixtureOutcome::ReviewRequired,
    ),
    fixture(
        "review-class-tmpfiles-noncreate",
        GoldenFixtureOutcome::ReviewRequired,
    ),
    fixture(
        "review-class-sysusers-nonstandard",
        GoldenFixtureOutcome::ReviewRequired,
    ),
    fixture(
        "review-class-gconf-schema",
        GoldenFixtureOutcome::ReviewRequired,
    ),
    fixture(
        "review-class-install-info",
        GoldenFixtureOutcome::ReviewRequired,
    ),
    fixture(
        "review-class-alternatives-interactive-or-broad",
        GoldenFixtureOutcome::ReviewRequired,
    ),
    fixture(
        "review-class-cache-refresh-nonstandard",
        GoldenFixtureOutcome::ReviewRequired,
    ),
    fixture(
        "review-class-rpm-verify",
        GoldenFixtureOutcome::ReviewRequired,
    ),
    fixture(
        "review-class-rpm-trigger",
        GoldenFixtureOutcome::ReviewRequired,
    ),
    fixture(
        "review-class-deb-trigger",
        GoldenFixtureOutcome::ReviewRequired,
    ),
    fixture("review-class-debconf", GoldenFixtureOutcome::ReviewRequired),
    fixture("review-class-udev", GoldenFixtureOutcome::ReviewRequired),
    fixture(
        "review-class-arch-alpm-hook",
        GoldenFixtureOutcome::ReviewRequired,
    ),
    fixture(
        "review-class-arch-install-function",
        GoldenFixtureOutcome::ReviewRequired,
    ),
    fixture(
        "legacy-replay-unknown-shell",
        GoldenFixtureOutcome::LegacyReplay,
    ),
    fixture(
        "legacy-replay-foreign-replay-rejected",
        GoldenFixtureOutcome::Rejected,
    ),
];

const fn fixture(id: &'static str, expected_outcome: GoldenFixtureOutcome) -> GoldenFixtureCase {
    GoldenFixtureCase {
        id,
        expected_outcome,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn golden_fixtures_have_unique_ids() {
        let declared = declared_fixture_ids();
        assert_eq!(
            declared.len(),
            ALL_GOLDEN_FIXTURE_CASES.len(),
            "golden fixture ids must be unique"
        );
    }

    #[test]
    fn golden_fixtures_required_cases_are_declared_with_matching_outcomes() {
        for required in REQUIRED_GOAL8_CASES {
            let declared = ALL_GOLDEN_FIXTURE_CASES
                .iter()
                .find(|case| case.id == required.id);
            assert_eq!(
                declared.copied(),
                Some(*required),
                "required Goal 8 fixture {} is not declared with the same outcome",
                required.id
            );
        }
    }
}
