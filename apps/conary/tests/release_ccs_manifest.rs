// apps/conary/tests/release_ccs_manifest.rs

use conary_core::ccs::CcsManifest;
use std::path::PathBuf;

#[test]
fn shipping_ccs_manifest_matches_the_stable_release_asset_contract() {
    let manifest_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../packaging/ccs/ccs.toml");

    let manifest = CcsManifest::from_file(&manifest_path).unwrap_or_else(|error| {
        panic!(
            "shipping CCS manifest {} must parse: {error:#}",
            manifest_path.display()
        )
    });
    assert_eq!(manifest.package.name, "conary");
    assert_eq!(manifest.package.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(
        manifest.package.release, "1",
        "the release wrapper normalizes the exact internal -1 package name to the stable version-only self-update asset name"
    );
    let blocking_findings =
        conary_core::ccs::v3::authoring::lint_manifest_for_v3_authoring(&manifest)
            .into_iter()
            .filter(|finding| finding.blocks_build)
            .collect::<Vec<_>>();
    assert!(
        blocking_findings.is_empty(),
        "shipping CCS manifest must satisfy current v3 authoring lint: {blocking_findings:#?}"
    );
}
