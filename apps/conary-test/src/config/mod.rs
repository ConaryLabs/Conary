// conary-test/src/config/mod.rs

pub mod corpus;
pub mod distro;
pub mod manifest;
pub mod release_root;

pub use distro::{DistroBuildContext, DistroConfig, GlobalConfig, TestPackage};
pub use manifest::{Assertion, StepType, TestDef, TestManifest};
pub use release_root::{
    AptGlobalTrustBinding, AuthenticatedTargetRoot, DistroReleaseRoot, ExpectedNativePackage,
    ExpectedOsRelease, NativePackageQuery, NativeRepositoryTrustBinding, PinnedTargetFile,
    ReleaseMediaAuthority, TargetArtifactIdentity, TargetArtifactRole, TargetPackageIdentity,
    TargetReleaseEvidence, TargetReleaseStage, TargetReleaseStageResult,
};

use anyhow::{Result, bail};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub fn load_manifest(path: &Path) -> Result<TestManifest> {
    let content = std::fs::read_to_string(path)?;
    let manifest: TestManifest = toml::from_str(&content)?;
    manifest.validate()?;
    Ok(manifest)
}

pub fn load_global_config(path: &Path) -> Result<GlobalConfig> {
    let content = std::fs::read_to_string(path)?;
    let config: GlobalConfig = toml::from_str(&content)?;
    config.apply_env_overrides()
}

pub fn validate_unique_test_ids(manifests: &[(PathBuf, TestManifest)]) -> Result<()> {
    let mut locations_by_id: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for (path, manifest) in manifests {
        for test in &manifest.test {
            locations_by_id
                .entry(test.id.clone())
                .or_default()
                .push(path.display().to_string());
        }
    }

    let duplicates: Vec<String> = locations_by_id
        .into_iter()
        .filter_map(|(id, locations)| {
            (locations.len() > 1)
                .then(|| format!("duplicate test id `{id}` in {}", locations.join(", ")))
        })
        .collect();

    if !duplicates.is_empty() {
        bail!("duplicate manifest test IDs:\n{}", duplicates.join("\n"));
    }

    Ok(())
}

#[cfg(test)]
mod tests;
pub use corpus::{
    CorpusCaseDef, CorpusCoverageClaimDef, CorpusSourceFormat, CorpusSuiteDef, CorpusTargetDef,
};
