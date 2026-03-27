// conary-core/src/bootstrap/adopt_seed.rs

//! Create a bootstrap seed from an adopted system's filesystem.

use std::path::Path;
use std::process::Command;

use crate::derivation::compose::erofs_image_hash;
use crate::derivation::seed::{SeedMetadata, SeedSource, SeedValidation};

#[derive(Debug, thiserror::Error)]
pub enum AdoptSeedError {
    #[error("seed validation failed, missing: {0:?}")]
    ValidationFailed(Vec<&'static str>),
    #[error("EROFS build failed: {0}")]
    ErofsBuild(String),
    #[error("I/O error: {0}")]
    Io(String),
}

/// Build a bootstrap seed EROFS image from the currently adopted system's
/// filesystem at `/`.
///
/// # Algorithm
///
/// 1. Probe `/` via [`SeedValidation::probe`] to confirm the system has the
///    required build tools. Returns [`AdoptSeedError::ValidationFailed`] with
///    the list of missing tools if any are absent.
/// 2. Creates `output_dir` on disk.
/// 3. Runs `mkfs.erofs` with `/` as the single source directory so that
///    `/usr`, `/bin`, `/lib`, `/sbin`, and `/etc` are all captured in
///    `output_dir/seed.erofs`.
/// 4. Hashes the resulting image with [`erofs_image_hash`].
/// 5. Writes `output_dir/seed.toml` containing the computed [`SeedMetadata`].
/// 6. Returns the metadata.
///
/// # Errors
///
/// - [`AdoptSeedError::ValidationFailed`] – required build tools are missing.
/// - [`AdoptSeedError::ErofsBuild`] – `mkfs.erofs` exited non-zero or could
///   not be spawned.
/// - [`AdoptSeedError::Io`] – filesystem or serialization failure.
pub fn build_adopted_seed(
    output_dir: &Path,
    distro_name: &str,
    distro_version: &str,
) -> Result<SeedMetadata, AdoptSeedError> {
    let validation = SeedValidation::probe(Path::new("/"));
    if !validation.is_valid() {
        return Err(AdoptSeedError::ValidationFailed(validation.missing_tools()));
    }

    std::fs::create_dir_all(output_dir)
        .map_err(|e| AdoptSeedError::Io(format!("create {}: {e}", output_dir.display())))?;

    let image_path = output_dir.join("seed.erofs");

    // mkfs.erofs takes exactly one source directory; passing multiple
    // directories is not supported and silently uses only the first.
    // Use "/" so that /usr, /bin, /lib, /sbin, and /etc are all captured.
    let status = Command::new("mkfs.erofs")
        .arg(&image_path)
        .arg("/")
        .status()
        .map_err(|e| AdoptSeedError::ErofsBuild(format!("failed to spawn mkfs.erofs: {e}")))?;

    if !status.success() {
        return Err(AdoptSeedError::ErofsBuild(format!(
            "mkfs.erofs exited with {}",
            status
                .code()
                .map_or_else(|| "signal".to_owned(), |c| c.to_string())
        )));
    }

    let seed_id = erofs_image_hash(&image_path)
        .map_err(|e| AdoptSeedError::ErofsBuild(format!("hashing image: {e}")))?;

    let metadata = SeedMetadata {
        seed_id,
        source: SeedSource::Adopted,
        origin_url: None,
        builder: None,
        packages: vec![],
        target_triple: format!("{}-unknown-linux-gnu", std::env::consts::ARCH),
        verified_by: vec![],
        origin_distro: Some(distro_name.to_owned()),
        origin_version: Some(distro_version.to_owned()),
    };

    let toml_content = toml::to_string_pretty(&metadata)
        .map_err(|e| AdoptSeedError::Io(format!("serializing seed.toml: {e}")))?;
    let toml_path = output_dir.join("seed.toml");
    std::fs::write(&toml_path, toml_content)
        .map_err(|e| AdoptSeedError::Io(format!("writing {}: {e}", toml_path.display())))?;

    Ok(metadata)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_failed_error_includes_tools() {
        let err = AdoptSeedError::ValidationFailed(vec!["gcc", "make"]);
        let msg = err.to_string();
        assert!(msg.contains("gcc"));
        assert!(msg.contains("make"));
    }

    #[test]
    fn erofs_build_error_display() {
        let err = AdoptSeedError::ErofsBuild("exit code 1".into());
        assert!(err.to_string().contains("exit code 1"));
    }

    #[test]
    fn io_error_display() {
        let err = AdoptSeedError::Io("permission denied".into());
        assert!(err.to_string().contains("permission denied"));
    }

    #[test]
    fn validation_failed_empty_tools() {
        let err = AdoptSeedError::ValidationFailed(vec![]);
        // Should not panic and should produce a valid display string.
        let msg = err.to_string();
        assert!(msg.contains("missing"));
    }
}
