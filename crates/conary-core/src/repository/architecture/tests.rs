// crates/conary-core/src/repository/architecture/tests.rs

use super::*;

const RPM_TABLES: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/architecture/rpm-6.0.1-rpmrc-architecture-tables.txt"
));
const DPKG_CPUTABLE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/architecture/dpkg-1.23.7-cputable"
));
const DPKG_TUPLETABLE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/architecture/dpkg-1.23.7-tupletable"
));
const ARCH_TABLE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/architecture/arch-2026-08-02-architecture-authority.txt"
));

#[test]
fn every_pinned_rpm_architecture_token_has_a_typed_class() {
    let mut tokens = std::collections::BTreeSet::new();
    for line in RPM_TABLES
        .lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
    {
        let fields = line.split(':').map(str::trim).collect::<Vec<_>>();
        match fields.first().copied() {
            Some("arch_canon") => {
                tokens.insert(fields[1]);
                tokens.insert(fields[2].split_whitespace().next().unwrap());
            }
            Some("arch_compat" | "buildarch_compat") => {
                tokens.insert(fields[1]);
                tokens.extend(fields[2].split_whitespace());
            }
            _ => panic!("unexpected RPM fixture line: {line}"),
        }
    }
    for token in tokens {
        assert!(
            known_package_architecture(VersionScheme::Rpm, token).is_some(),
            "RPM token '{token}' lacks a typed class"
        );
    }
}

#[test]
fn every_pinned_dpkg_tuple_output_has_a_typed_class() {
    let cpus = DPKG_CPUTABLE
        .lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        .map(|line| line.split_whitespace().next().unwrap())
        .collect::<Vec<_>>();
    for cpu in &cpus {
        assert!(
            known_package_architecture(VersionScheme::Debian, cpu).is_some(),
            "dpkg CPU token '{cpu}' lacks a typed class"
        );
    }
    for line in DPKG_TUPLETABLE
        .lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
    {
        let output = line.split_whitespace().nth(1).unwrap();
        if output.contains("<cpu>") {
            for cpu in &cpus {
                let token = output.replace("<cpu>", cpu);
                assert!(
                    known_package_architecture(VersionScheme::Debian, &token).is_some(),
                    "dpkg tuple output '{token}' lacks a typed class"
                );
            }
        } else {
            assert!(
                known_package_architecture(VersionScheme::Debian, output).is_some(),
                "dpkg tuple output '{output}' lacks a typed class"
            );
        }
    }
}

#[test]
fn every_pinned_arch_package_architecture_is_known() {
    let carch = ARCH_TABLE
        .lines()
        .find_map(|line| line.strip_prefix("CARCH="))
        .unwrap();
    assert_eq!(carch, "x86_64");
    for token in ARCH_TABLE
        .lines()
        .filter_map(|line| line.strip_prefix("PACKAGE_ARCH="))
    {
        assert!(known_package_architecture(VersionScheme::Arch, token).is_some());
    }
}

#[test]
fn rpm_native_only_admits_native_aliases_and_noarch() {
    for token in ["x86_64", "amd64", "ia32e", "em64t", "noarch"] {
        assert_eq!(
            native_resolution_architecture_decision(VersionScheme::Rpm, token, "x86_64"),
            NativeResolutionArchitectureDecisionV1::Admitted,
            "{token}"
        );
    }
    assert_eq!(
        native_resolution_architecture_decision(VersionScheme::Rpm, "athlon", "x86_64"),
        NativeResolutionArchitectureDecisionV1::Excluded {
            class: NativeMachineArchitectureClassV1::X86_32,
        }
    );
    assert_eq!(
        native_resolution_architecture_decision(VersionScheme::Rpm, "x86_64_v3", "x86_64"),
        NativeResolutionArchitectureDecisionV1::Excluded {
            class: NativeMachineArchitectureClassV1::X86_64V3,
        }
    );
}

#[test]
fn debian_x32_is_known_and_non_native_for_amd64() {
    assert_eq!(
        native_resolution_architecture_decision(VersionScheme::Debian, "x32", "amd64"),
        NativeResolutionArchitectureDecisionV1::Excluded {
            class: NativeMachineArchitectureClassV1::X86_64X32,
        }
    );
}

#[test]
fn known_tokens_outside_supported_machine_family_are_typed_exclusions() {
    for (scheme, token, target) in [
        (VersionScheme::Rpm, "aarch64", "x86_64"),
        (VersionScheme::Debian, "arm64", "amd64"),
    ] {
        assert_eq!(
            native_resolution_architecture_decision(scheme, token, target),
            NativeResolutionArchitectureDecisionV1::Excluded {
                class: NativeMachineArchitectureClassV1::Aarch64,
            }
        );
    }
}

#[test]
fn unknown_architecture_is_neither_admitted_nor_excluded() {
    assert_eq!(
        native_resolution_architecture_decision(VersionScheme::Arch, "future-carch", "x86_64"),
        NativeResolutionArchitectureDecisionV1::UnknownArchitectureToken {
            scheme: VersionScheme::Arch,
            token: "future-carch".to_string(),
        }
    );
}
