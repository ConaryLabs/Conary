// apps/conary/tests/bundle_replay.rs

mod common;

use common::legacy_scriptlet_fixtures::{
    LegacyBundleFixture, build_ccs_package_fixture, synthetic_legacy_bundle,
};
use conary_core::ccs::CcsPackage;
use conary_core::ccs::legacy_scriptlets::{LifecyclePath, ScriptletDecision, ScriptletFidelity};
use conary_core::packages::PackageFormat;

#[test]
fn synthetic_legacy_bundle_fixtures_cover_task5_matrix() {
    let cases = [
        LegacyBundleFixture::NoBundle,
        LegacyBundleFixture::NativeFree,
        LegacyBundleFixture::ReplacedOnly,
        LegacyBundleFixture::ReviewEntry,
        LegacyBundleFixture::BlockedEntry,
        LegacyBundleFixture::SameSourceLegacyPostInstall,
        LegacyBundleFixture::FutureLegacyPostRemove,
        LegacyBundleFixture::RawTriggerLegacy,
        LegacyBundleFixture::UnsupportedNativeInvocation,
    ];

    for case in cases {
        let bundle = synthetic_legacy_bundle(case);
        let (_temp, package_path) =
            build_ccs_package_fixture(case.package_name(), "1.0.0", bundle.clone())
                .expect("build CCS fixture");
        let parsed = CcsPackage::parse(package_path.to_str().expect("utf-8 package path"))
            .expect("parse CCS fixture");

        assert_eq!(
            parsed.manifest().legacy_scriptlets.is_some(),
            bundle.is_some(),
            "{case:?}"
        );

        if let Some(bundle) = bundle {
            bundle.validate().expect("fixture bundle validates");
            let decisions: Vec<_> = bundle
                .entries
                .iter()
                .map(|entry| entry.decision.clone())
                .collect();

            match case {
                LegacyBundleFixture::NativeFree => {
                    assert!(bundle.entries.is_empty());
                    assert_eq!(bundle.scriptlet_fidelity, ScriptletFidelity::NativeFree);
                }
                LegacyBundleFixture::ReplacedOnly => {
                    assert_eq!(decisions, vec![ScriptletDecision::Replaced]);
                }
                LegacyBundleFixture::ReviewEntry => {
                    assert_eq!(decisions, vec![ScriptletDecision::Review]);
                }
                LegacyBundleFixture::BlockedEntry => {
                    assert_eq!(decisions, vec![ScriptletDecision::Blocked]);
                }
                LegacyBundleFixture::SameSourceLegacyPostInstall => {
                    assert_eq!(decisions, vec![ScriptletDecision::Legacy]);
                    assert_eq!(bundle.entries[0].phase, LifecyclePath::PostInstall);
                }
                LegacyBundleFixture::FutureLegacyPostRemove => {
                    assert_eq!(decisions, vec![ScriptletDecision::Legacy]);
                    assert_eq!(bundle.entries[0].phase, LifecyclePath::PostRemove);
                }
                LegacyBundleFixture::RawTriggerLegacy => {
                    assert_eq!(decisions, vec![ScriptletDecision::Legacy]);
                    assert_eq!(bundle.entries[0].phase, LifecyclePath::Trigger);
                    assert!(bundle.entries[0].rpm_trigger.is_some());
                }
                LegacyBundleFixture::UnsupportedNativeInvocation => {
                    assert_eq!(decisions, vec![ScriptletDecision::Legacy]);
                    assert!(bundle.entries[0].native_invocation.stdin.is_some());
                }
                LegacyBundleFixture::NoBundle => unreachable!("handled by None branch"),
            }
        }
    }
}
