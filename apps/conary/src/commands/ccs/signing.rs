// src/commands/ccs/signing.rs

//! CCS package signing
//!
//! Commands for generating signing keys and signing packages.

use anyhow::{Context, Result};
use std::path::Path;

fn record_authority_manifest_entry(
    entry_path: &str,
    contents: Vec<u8>,
    manifest_bytes: &mut Option<Vec<u8>>,
) -> Result<()> {
    if entry_path != "MANIFEST" && entry_path != "./MANIFEST" {
        return Ok(());
    }
    if manifest_bytes.is_some() {
        anyhow::bail!("Package contains duplicate MANIFEST entries");
    }
    *manifest_bytes = Some(contents);
    Ok(())
}

fn require_current_authority_manifest(manifest_bytes: Option<Vec<u8>>) -> Result<Vec<u8>> {
    let bytes =
        manifest_bytes.ok_or_else(|| anyhow::anyhow!("Package missing required v3 MANIFEST"))?;
    let authority = conary_core::ccs::v3::AuthorityDocumentV3::from_cbor(&bytes)
        .context("Package MANIFEST is not current CCS v3 authority")?;
    conary_core::ccs::v3::validation::validate_authority(&authority).map_err(|error| {
        anyhow::anyhow!("Package MANIFEST is not valid CCS v3 authority: {error}")
    })?;
    Ok(bytes)
}

/// Generate an Ed25519 signing key pair
pub async fn cmd_ccs_keygen(output: &str, key_id: Option<String>, force: bool) -> Result<()> {
    use conary_core::ccs::SigningKeyPair;

    let private_path = Path::new(output).with_extension("private");
    let public_path = Path::new(output).with_extension("public");

    // Check if files already exist
    if !force && (private_path.exists() || public_path.exists()) {
        anyhow::bail!(
            "Key files already exist. Use --force to overwrite.\n  Private: {}\n  Public: {}",
            private_path.display(),
            public_path.display()
        );
    }

    println!("Generating Ed25519 signing key pair...");

    // Generate key pair
    let mut keypair = SigningKeyPair::generate();
    if let Some(id) = key_id {
        keypair = keypair.with_key_id(&id);
    }

    // Save to files
    keypair
        .save_to_files(&private_path, &public_path)
        .context("Failed to save key files")?;

    println!();
    println!("Key pair generated successfully!");
    println!();
    println!("Files created:");
    println!(
        "  Private key: {} (keep this secret!)",
        private_path.display()
    );
    println!(
        "  Public key:  {} (share for verification)",
        public_path.display()
    );
    println!();
    println!("Public key (base64):");
    println!("  {}", keypair.public_key_base64());
    println!();
    println!("To sign a package:");
    println!(
        "  conary ccs-sign package.ccs --key {}",
        private_path.display()
    );
    println!();
    println!("Add the public key to trust policies for verification.");

    Ok(())
}

/// Sign a CCS package with an Ed25519 key
pub async fn cmd_ccs_sign(package: &str, key_path: &str, output: Option<String>) -> Result<()> {
    use conary_core::ccs::signing::SigningKeyPair;
    use flate2::Compression;
    use flate2::read::GzDecoder;
    use flate2::write::GzEncoder;
    use std::fs::File;
    use tar::{Archive, Builder as TarBuilder};

    let package_path = Path::new(package);
    let key_path = Path::new(key_path);

    if !package_path.exists() {
        anyhow::bail!("Package not found: {}", package);
    }

    if !key_path.exists() {
        anyhow::bail!("Key file not found: {}", key_path.display());
    }

    // Load signing key
    println!("Loading signing key from {}...", key_path.display());
    let signing_key =
        SigningKeyPair::load_from_file(key_path).context("Failed to load signing key")?;

    println!("Signing package: {}", package_path.display());

    // Refuse to bless a retired or structurally invalid archive.
    conary_core::ccs::archive_reader::inspect_untrusted_ccs_archive(File::open(package_path)?)
        .context("Package is not a structurally valid current CCS v3 archive")?;

    // Open and read the package.
    let file = File::open(package_path)?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);

    // Create temp directory for extraction
    let temp_dir = tempfile::tempdir()?;

    // Extract all files and capture the sole v3 MANIFEST authority.
    let mut manifest_bytes: Option<Vec<u8>> = None;

    for entry in archive.entries()? {
        let mut entry = entry?;
        let entry_path = entry.path()?.to_path_buf();

        // Extract to temp dir
        entry.unpack(temp_dir.path().join(&entry_path))?;

        // MANIFEST is the only signature authority. MANIFEST.toml is a
        // human-readable projection and must never become a fallback.
        let entry_path_str = entry_path.to_string_lossy();
        let is_manifest = entry_path_str == "MANIFEST" || entry_path_str == "./MANIFEST";
        if is_manifest {
            record_authority_manifest_entry(
                &entry_path_str,
                std::fs::read(temp_dir.path().join(&entry_path))?,
                &mut manifest_bytes,
            )?;
        }
    }

    let manifest_bytes = require_current_authority_manifest(manifest_bytes)?;

    // Sign the exact current authority bytes.
    println!("Creating signature...");
    let signature = signing_key.sign(&manifest_bytes);
    let sig_json = serde_json::to_string_pretty(&signature)?;

    // Write signature file
    std::fs::write(temp_dir.path().join("MANIFEST.sig"), &sig_json)?;

    // Determine output path
    let output_path = output
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| package_path.to_path_buf());

    // Rebuild beside the destination, verify the complete signed package, and
    // only then atomically replace the requested output.
    println!("Writing signed package...");
    let output_parent = output_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let staged = tempfile::NamedTempFile::new_in(output_parent)?;
    let output_file = staged.reopen()?;
    let encoder = GzEncoder::new(output_file, Compression::default());
    let mut archive = TarBuilder::new(encoder);

    // Add all files from temp directory
    archive.append_dir_all(".", temp_dir.path())?;

    let encoder = archive.into_inner()?;
    encoder.finish()?;
    conary_core::ccs::verify::verify_package(
        staged.path(),
        &conary_core::ccs::TrustPolicy::strict(vec![signing_key.public_key_base64()]),
    )
    .context("Signed CCS v3 package failed complete authority and payload verification")?;
    staged
        .persist(&output_path)
        .map_err(|error| error.error)
        .with_context(|| format!("replace signed package {}", output_path.display()))?;

    println!();
    println!("Package signed successfully!");
    println!("  Output: {}", output_path.display());
    if let Some(key_id) = signing_key.key_id() {
        println!("  Key ID: {}", key_id);
    }
    println!(
        "  Timestamp: {}",
        signature.timestamp.as_deref().unwrap_or("none")
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{record_authority_manifest_entry, require_current_authority_manifest};

    #[test]
    fn signing_requires_current_authority_and_never_falls_back_to_toml() {
        let mut captured = None;
        record_authority_manifest_entry(
            "MANIFEST.toml",
            b"debug projection".to_vec(),
            &mut captured,
        )
        .unwrap();

        assert!(captured.is_none());
        assert!(
            require_current_authority_manifest(captured)
                .unwrap_err()
                .to_string()
                .contains("required v3 MANIFEST")
        );
    }

    #[test]
    fn signing_records_one_exact_authority_manifest() {
        let mut captured = None;
        record_authority_manifest_entry("MANIFEST", vec![1, 2, 3], &mut captured).unwrap();
        assert!(
            require_current_authority_manifest(captured)
                .unwrap_err()
                .to_string()
                .contains("current CCS v3 authority")
        );

        let mut duplicate = Some(vec![1]);
        assert!(
            record_authority_manifest_entry("./MANIFEST", vec![2], &mut duplicate)
                .unwrap_err()
                .to_string()
                .contains("duplicate MANIFEST")
        );
    }
}
