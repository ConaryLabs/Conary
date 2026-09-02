// crates/conary-core/src/repository/architecture/tests.rs

use std::collections::{BTreeMap, BTreeSet};

use super::*;

fn machine(scheme: VersionScheme, token: &str) -> NativeMachineIdentityV1 {
    match known_package_architecture(scheme, token) {
        Some(KnownPackageArchitecture::Machine(identity)) => identity,
        value => panic!("expected machine identity for {scheme:?} token '{token}', got {value:?}"),
    }
}

#[test]
fn every_pinned_rpm_architecture_token_has_an_exact_identity() {
    let mut tokens = BTreeSet::new();
    let mut canonical_identities = BTreeMap::new();
    for line in data_lines(RPM_TABLES) {
        let fields = line.split(':').map(str::trim).collect::<Vec<_>>();
        match fields.first().copied() {
            Some("arch_canon") => {
                let input = fields[1];
                let canonical = fields[2].split_whitespace().next().unwrap();
                tokens.insert(input);
                tokens.insert(canonical);
                assert_eq!(
                    machine(VersionScheme::Rpm, input),
                    machine(VersionScheme::Rpm, canonical),
                    "arch_canon must own canonicalization for '{input}'"
                );
                let identity = machine(VersionScheme::Rpm, canonical);
                if let Some(previous) = canonical_identities.insert(identity, canonical) {
                    assert_eq!(
                        previous, canonical,
                        "distinct RPM arch_canon results must not collide"
                    );
                }
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
            "RPM token '{token}' lacks a typed identity"
        );
    }
}

#[test]
fn every_pinned_dpkg_tuple_output_has_a_full_identity() {
    let cpus = data_lines(DPKG_CPUTABLE)
        .map(|line| line.split_whitespace().next().unwrap())
        .collect::<Vec<_>>();
    for cpu in &cpus {
        assert!(
            known_package_architecture(VersionScheme::Debian, cpu).is_some(),
            "dpkg CPU token '{cpu}' lacks a typed identity"
        );
    }
    for line in data_lines(DPKG_TUPLETABLE) {
        let output = line.split_whitespace().nth(1).unwrap();
        if output.contains("<cpu>") {
            for cpu in &cpus {
                let token = output.replace("<cpu>", cpu);
                assert!(
                    known_package_architecture(VersionScheme::Debian, &token).is_some(),
                    "dpkg tuple output '{token}' lacks a typed identity"
                );
            }
        } else {
            assert!(
                known_package_architecture(VersionScheme::Debian, output).is_some(),
                "dpkg tuple output '{output}' lacks a typed identity"
            );
        }
    }
}

#[test]
fn every_pinned_arch_package_architecture_is_known() {
    let carch = data_lines(ARCH_TABLE)
        .find_map(|line| line.strip_prefix("CARCH="))
        .unwrap();
    assert_eq!(carch, "x86_64");
    for token in data_lines(ARCH_TABLE).filter_map(|line| line.strip_prefix("PACKAGE_ARCH=")) {
        assert!(known_package_architecture(VersionScheme::Arch, token).is_some());
    }
}

#[test]
fn dpkg_abi_traps_have_distinct_full_identities() {
    for tokens in [
        &["armel", "armhf"][..],
        &["mips", "mipsel", "mips64", "mips64el"][..],
        &["ppc64", "ppc64el"][..],
        &["i386", "x32"][..],
        &["sparc", "sparc64"][..],
    ] {
        let identities = tokens
            .iter()
            .map(|token| machine(VersionScheme::Debian, token))
            .collect::<BTreeSet<_>>();
        assert_eq!(identities.len(), tokens.len(), "collision in {tokens:?}");
    }
}

#[test]
fn rpm_arch_canon_not_arch_compat_owns_native_identity() {
    for (left, right) in [
        ("x86_64", "amd64"),
        ("i686", "i386"),
        ("armv7hl", "armv7l"),
        ("armv7l", "armv5tel"),
        ("mips", "mipsel"),
        ("mips64", "mips64el"),
        ("ppc64", "ppc64le"),
        ("sparc", "sparc64"),
    ] {
        assert_ne!(
            machine(VersionScheme::Rpm, left),
            machine(VersionScheme::Rpm, right),
            "RPM native-only identity must not apply arch_compat for {left}/{right}"
        );
    }
}

#[test]
fn table_justified_cross_scheme_identities_are_equal() {
    for (debian, rpm) in [
        ("amd64", "x86_64"),
        ("arm64", "aarch64"),
        ("ppc64el", "ppc64le"),
        ("s390x", "s390x"),
        ("riscv64", "riscv64"),
        ("i386", "i686"),
    ] {
        assert_eq!(
            machine(VersionScheme::Debian, debian),
            machine(VersionScheme::Rpm, rpm),
            "cross-scheme identity mismatch for Debian {debian} and RPM {rpm}"
        );
    }
    assert_eq!(
        machine(VersionScheme::Debian, "amd64"),
        machine(VersionScheme::Arch, "x86_64")
    );
}

#[test]
fn tables_do_not_justify_arm_float_abi_cross_scheme_aliases() {
    assert_ne!(
        machine(VersionScheme::Debian, "armhf"),
        machine(VersionScheme::Rpm, "armv7hl")
    );
    for rpm in ["armv5tel", "armv7l"] {
        assert_ne!(
            machine(VersionScheme::Debian, "armel"),
            machine(VersionScheme::Rpm, rpm)
        );
    }
}

#[test]
fn architecture_independent_tokens_resolve_for_supported_profiles() {
    for (scheme, independent, native) in [
        (VersionScheme::Rpm, "noarch", "x86_64"),
        (VersionScheme::Debian, "all", "amd64"),
        (VersionScheme::Arch, "any", "x86_64"),
    ] {
        assert_eq!(
            native_resolution_architecture_decision(scheme, independent, native),
            NativeResolutionArchitectureDecisionV1::Admitted
        );
    }
}

#[test]
fn rpm_native_only_admits_exact_canonical_architecture_and_noarch() {
    for token in ["x86_64", "noarch"] {
        assert_eq!(
            native_resolution_architecture_decision(VersionScheme::Rpm, token, "x86_64"),
            NativeResolutionArchitectureDecisionV1::Admitted,
            "{token}"
        );
    }
    for token in ["amd64", "athlon", "x86_64_v3"] {
        assert_eq!(
            native_resolution_architecture_decision(VersionScheme::Rpm, token, "x86_64"),
            NativeResolutionArchitectureDecisionV1::Excluded {
                identity: machine(VersionScheme::Rpm, token),
            },
            "{token}"
        );
    }
}

#[test]
fn debian_x32_is_known_and_non_native_for_amd64() {
    assert_eq!(
        native_resolution_architecture_decision(VersionScheme::Debian, "x32", "amd64"),
        NativeResolutionArchitectureDecisionV1::Excluded {
            identity: machine(VersionScheme::Debian, "x32"),
        }
    );
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

#[test]
fn independent_architecture_does_not_hide_an_unknown_target() {
    assert_eq!(
        native_resolution_architecture_decision(VersionScheme::Rpm, "noarch", "future-host"),
        NativeResolutionArchitectureDecisionV1::UnknownArchitectureToken {
            scheme: VersionScheme::Rpm,
            token: "future-host".to_string(),
        }
    );
}
