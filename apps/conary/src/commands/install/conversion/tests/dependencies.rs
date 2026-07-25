// apps/conary/src/commands/install/conversion/tests/dependencies.rs

use super::*;
use conary_core::repository::versioning::VersionScheme;

#[test]
fn pending_providers_satisfy_exact_versioned_capability() {
    let pending = vec![PendingCcsProvider {
        version_scheme: VersionScheme::Rpm,
        provides: vec![PendingProvide {
            name: "kernel-uname-r".to_string(),
            version: Some("6.17.1-300.fc44.x86_64".to_string()),
        }],
    }];
    let dependency = MissingDependency {
        name: "kernel-uname-r".to_string(),
        constraint: VersionConstraint::parse("= 6.17.1-300.fc44.x86_64").unwrap(),
        required_by: vec!["kernel-modules-core".to_string()],
    };

    assert!(pending_provider_satisfies_dependency(&pending, &dependency).unwrap());
}

#[test]
fn pending_provider_names_require_exact_capability_identity() {
    let provider = PendingCcsProvider {
        version_scheme: VersionScheme::Rpm,
        provides: vec![PendingProvide {
            name: "libexample.so.1".to_string(),
            version: None,
        }],
    };
    let dependency = MissingDependency {
        name: "libexample.so.1()(64bit)".to_string(),
        constraint: VersionConstraint::Any,
        required_by: vec!["consumer".to_string()],
    };

    assert!(!pending_provider_directly_satisfies(&provider, &dependency).unwrap());
}

#[test]
fn unversioned_pending_provide_cannot_satisfy_versioned_requirement() {
    let provider = PendingCcsProvider {
        version_scheme: VersionScheme::Rpm,
        provides: vec![PendingProvide {
            name: "virtual-abi".to_string(),
            version: None,
        }],
    };
    let dependency = MissingDependency {
        name: "virtual-abi".to_string(),
        constraint: VersionConstraint::parse(">= 2").unwrap(),
        required_by: vec!["consumer".to_string()],
    };

    assert!(!pending_provider_directly_satisfies(&provider, &dependency).unwrap());
}

#[test]
fn default_dependency_passes_reach_kernel_initramfs_toolchain() {
    assert_eq!(DEFAULT_CCS_DEPENDENCY_PASSES, 2);
}
