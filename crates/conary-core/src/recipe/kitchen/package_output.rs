// conary-core/src/recipe/kitchen/package_output.rs

//! CCS package finalization for a completed recipe build.

use crate::ccs::builder::{CcsBuilder, write_signed_current_ccs_package};
use crate::ccs::manifest::{CcsManifest, ManifestProvenance, Platform};
use crate::ccs::verify::{TrustPolicy, verify_package};
use crate::error::{Error, Result};
use crate::recipe::hermetic::compare_host_record;
use crate::repository::dependency_model::RepositoryRequirementKind;
use crate::repository::requirement::parse_native_requirement;
use crate::repository::versioning::VersionScheme;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use target_lexicon::{Architecture, OperatingSystem, Triple};
use tracing::info;

use super::Cook;

impl Cook<'_> {
    /// Phase 4: Plate - package the result as CCS.
    pub(super) fn plate(&mut self, output_dir: &Path) -> Result<(PathBuf, ManifestProvenance)> {
        if fs::read_dir(&self.dest_dir)?.count() == 0 {
            return Err(Error::IoError(
                "No files installed to destdir - install phase may have failed".to_string(),
            ));
        }

        let mut manifest =
            CcsManifest::new_minimal(&self.recipe.package.name, &self.recipe.package.version);
        if let Some(description) = &self.recipe.package.description {
            manifest.package.description = description.clone();
        } else if let Some(summary) = &self.recipe.package.summary {
            manifest.package.description = summary.clone();
        }
        manifest.package.license = self.recipe.package.license.clone();
        manifest.package.homepage = self.recipe.package.homepage.clone();
        manifest.package.release = self.recipe.package.release.clone();
        manifest.package.platform = Some(recipe_target_platform(self.recipe)?);
        for dependency in &self.recipe.build.requires {
            manifest.requirements.push(
                parse_native_requirement(
                    RepositoryRequirementKind::Depends,
                    VersionScheme::Conary,
                    dependency,
                )
                .map_err(|error| {
                    Error::ParseError(format!(
                        "invalid runtime requirement '{dependency}' in recipe: {error}"
                    ))
                })?,
            );
        }

        let mut build_result = CcsBuilder::new(manifest, &self.dest_dir)
            .map_err(|error| {
                Error::ConfigError(format!("invalid CCS build policy configuration: {error}"))
            })?
            .build()
            .map_err(|error| Error::IoError(format!("CCS build failed: {error}")))?;

        // Provenance covers the complete canonical payload authority: node
        // kind, ownership, mode, timestamp, xattrs, and regular-file content.
        for file in &build_result.files {
            let authority_hash =
                crate::ccs::attestation::canonical_json_hash(file).map_err(|error| {
                    Error::IoError(format!(
                        "failed to hash CCS payload authority for {}: {error}",
                        file.path
                    ))
                })?;
            self.provenance
                .record_file_hash(&file.path, &authority_hash);
        }
        self.provenance.compute_merkle_root();
        self.record_hermetic_divergence();

        let provenance = self.provenance.to_manifest_provenance();
        build_result.manifest.provenance = Some(provenance.clone());
        let package_path = output_dir.join(format!(
            "{}-{}-{}.ccs",
            self.recipe.package.name, self.recipe.package.version, self.recipe.package.release
        ));
        let signing_key = self
            .kitchen
            .config
            .ccs_signing_authority
            .as_ref()
            .ok_or_else(|| {
                Error::ConfigError(
                    "Kitchen CCS output requires an explicit CcsPackageSigningAuthority"
                        .to_string(),
                )
            })?
            .key_pair();
        write_signed_current_ccs_package(&build_result, &package_path, signing_key, false)
            .map_err(|error| {
                Error::IoError(format!("Failed to write signed CCS v2 package: {error}"))
            })?;
        verify_package(
            &package_path,
            &TrustPolicy::strict(vec![signing_key.public_key_base64()]),
        )
        .map_err(|error| {
            Error::IoError(format!(
                "Failed to verify newly cooked CCS v2 package {}: {error}",
                package_path.display()
            ))
        })?;

        self.log_line(&format!(
            "Created CCS package: {} ({} files, {} payload sources)",
            package_path.display(),
            build_result.files.len(),
            build_result
                .payloads
                .iter()
                .filter(|payload| payload.node.kind.is_regular())
                .count()
        ));
        info!(
            "Cooked: {} ({} files, DNA: {})",
            package_path.display(),
            build_result.files.len(),
            provenance.dna_hash.as_deref().unwrap_or("unknown")
        );
        Ok((package_path, provenance))
    }

    fn record_hermetic_divergence(&mut self) {
        let Some(evidence) = self.provenance.hermetic_evidence.as_mut() else {
            return;
        };
        let mut report = compare_host_record(
            self.kitchen.config.expected_host_build_record.as_ref(),
            self.provenance.merkle_root.as_deref(),
        );
        report.diagnostics.extend(
            self.kitchen
                .config
                .host_build_record_diagnostics
                .iter()
                .cloned(),
        );
        evidence.divergence = report;
    }
}

fn recipe_target_platform(recipe: &crate::recipe::Recipe) -> Result<Platform> {
    let triple = match recipe
        .cross
        .as_ref()
        .and_then(|cross| cross.target.as_deref())
    {
        Some(target) => Triple::from_str(target).map_err(|error| {
            Error::ConfigError(format!("invalid recipe cross target '{target}': {error}"))
        })?,
        None => Triple::host(),
    };
    if triple.architecture == Architecture::Unknown {
        return Err(Error::ConfigError(
            "recipe target architecture must be known".to_string(),
        ));
    }
    if triple.operating_system == OperatingSystem::Unknown {
        return Err(Error::ConfigError(
            "recipe target operating system must be known".to_string(),
        ));
    }

    Ok(Platform {
        os: triple.operating_system.to_string(),
        arch: Some(triple.architecture.to_string()),
        libc: triple.environment.to_string(),
        abi: None,
    })
}

#[cfg(test)]
mod tests {
    use super::recipe_target_platform;
    use crate::recipe::parse_recipe;

    #[test]
    fn recipe_target_platform_uses_structured_cross_target() {
        let recipe = parse_recipe(
            r#"
[package]
name = "cross-demo"
version = "1.0"

[source]
path = "."

[build]
install = "true"

[cross]
target = "aarch64-unknown-linux-gnu"
"#,
        )
        .unwrap();

        let platform = recipe_target_platform(&recipe).unwrap();
        assert_eq!(platform.os, "linux");
        assert_eq!(platform.arch.as_deref(), Some("aarch64"));
        assert_eq!(platform.libc, "gnu");
    }

    #[test]
    fn recipe_target_platform_rejects_unknown_target() {
        let recipe = parse_recipe(
            r#"
[package]
name = "cross-demo"
version = "1.0"

[source]
path = "."

[build]
install = "true"

[cross]
target = "not-a-real-target"
"#,
        )
        .unwrap();

        let error = recipe_target_platform(&recipe).unwrap_err();
        assert!(error.to_string().contains("invalid recipe cross target"));
    }
}
