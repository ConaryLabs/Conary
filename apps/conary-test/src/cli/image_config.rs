// conary-test/src/cli/image_config.rs

//! Typed distro image and Containerfile resolution for CLI image operations.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use conary_test::config::distro::GlobalConfig;
use conary_test::paths;

/// Resolve the containerfile path for a configured distro.
pub(super) fn containerfile_path(config: &GlobalConfig, distro: &str) -> Result<PathBuf> {
    let distro_config = config
        .distros
        .get(distro)
        .with_context(|| format!("unknown distro: {distro}"))?;
    let default_name = format!("Containerfile.{distro}");
    let filename = distro_config
        .containerfile
        .as_deref()
        .unwrap_or(&default_name);
    let path = paths::default_container_dir()?.join(filename);
    if !path.exists() {
        bail!("containerfile not found: {}", path.display());
    }
    Ok(path)
}

/// Resolve the exact digest-pinned transport image for one distro lane.
pub(super) fn base_image_reference(config: &GlobalConfig, distro: &str) -> Result<String> {
    let distro_config = config
        .distros
        .get(distro)
        .with_context(|| format!("unknown distro: {distro}"))?;
    if let Some(release_root) = &distro_config.release_root {
        release_root.validate()?;
        return Ok(release_root.base_image.clone());
    }

    let containerfile = containerfile_path(config, distro)?;
    let source = std::fs::read_to_string(&containerfile)
        .with_context(|| format!("failed to read {}", containerfile.display()))?;
    let reference = source
        .lines()
        .find_map(|line| line.strip_prefix("FROM "))
        .and_then(|line| line.split_ascii_whitespace().next())
        .with_context(|| format!("{} has no FROM instruction", containerfile.display()))?;
    if !reference.contains("@sha256:") {
        bail!(
            "base image in {} is not digest-pinned: {reference}",
            containerfile.display()
        );
    }
    Ok(reference.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_reference_uses_typed_release_root_for_derivatives() {
        let config = super::super::load_config().unwrap();
        let reference = base_image_reference(&config, "linux-mint-22.3").unwrap();

        assert_eq!(
            reference,
            config.distros["linux-mint-22.3"]
                .release_root
                .as_ref()
                .unwrap()
                .base_image
        );
        assert!(reference.contains("@sha256:"));
    }

    #[test]
    fn base_reference_reads_the_pinned_from_for_standard_lanes() {
        let config = super::super::load_config().unwrap();
        let reference = base_image_reference(&config, "fedora44").unwrap();

        assert!(reference.starts_with("registry.fedoraproject.org/fedora:"));
        assert!(reference.contains("@sha256:"));
    }
}
