// apps/conary/tests/release_ccs_manifest.rs

use conary_core::ccs::CcsManifest;
use std::path::PathBuf;

#[test]
fn shipping_ccs_manifest_parses_with_current_canonical_contract() {
    let manifest_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../packaging/ccs/ccs.toml");

    CcsManifest::from_file(&manifest_path).unwrap_or_else(|error| {
        panic!(
            "shipping CCS manifest {} must parse: {error:#}",
            manifest_path.display()
        )
    });
}
