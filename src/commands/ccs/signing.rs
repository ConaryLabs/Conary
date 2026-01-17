// src/commands/ccs/signing.rs

//! CCS package signing
//!
//! Commands for generating signing keys and signing packages.

use anyhow::{Context, Result};
use std::path::Path;

/// Generate an Ed25519 signing key pair
pub fn cmd_ccs_keygen(
    output: &str,
    key_id: Option<String>,
    force: bool,
) -> Result<()> {
    use conary::ccs::SigningKeyPair;

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
    keypair.save_to_files(&private_path, &public_path)
        .context("Failed to save key files")?;

    println!();
    println!("Key pair generated successfully!");
    println!();
    println!("Files created:");
    println!("  Private key: {} (keep this secret!)", private_path.display());
    println!("  Public key:  {} (share for verification)", public_path.display());
    println!();
    println!("Public key (base64):");
    println!("  {}", keypair.public_key_base64());
    println!();
    println!("To sign a package:");
    println!("  conary ccs-sign package.ccs --key {}", private_path.display());
    println!();
    println!("Add the public key to trust policies for verification.");

    Ok(())
}

/// Sign a CCS package with an Ed25519 key
pub fn cmd_ccs_sign(
    package: &str,
    key_path: &str,
    output: Option<String>,
) -> Result<()> {
    use conary::ccs::signing::SigningKeyPair;
    use flate2::read::GzDecoder;
    use flate2::write::GzEncoder;
    use flate2::Compression;
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
    let signing_key = SigningKeyPair::load_from_file(key_path)
        .context("Failed to load signing key")?;

    println!("Signing package: {}", package_path.display());

    // Open and read the package
    let file = File::open(package_path)?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);

    // Create temp directory for extraction
    let temp_dir = tempfile::tempdir()?;

    // Extract all files and find MANIFEST (CBOR preferred, TOML fallback)
    let mut manifest_bytes: Option<Vec<u8>> = None;

    for entry in archive.entries()? {
        let mut entry = entry?;
        let entry_path = entry.path()?.to_path_buf();

        // Extract to temp dir
        entry.unpack(temp_dir.path().join(&entry_path))?;

        // Capture manifest content - prefer CBOR MANIFEST over TOML
        let entry_path_str = entry_path.to_string_lossy();
        let is_cbor = entry_path_str == "MANIFEST" || entry_path_str == "./MANIFEST";
        let is_toml = entry_path_str == "MANIFEST.toml" || entry_path_str == "./MANIFEST.toml";
        // CBOR always takes precedence; TOML only if no CBOR found yet
        if is_cbor || (is_toml && manifest_bytes.is_none()) {
            manifest_bytes = Some(std::fs::read(temp_dir.path().join(&entry_path))?);
        }
    }

    let manifest_bytes = manifest_bytes
        .ok_or_else(|| anyhow::anyhow!("Package missing MANIFEST"))?;

    // Sign the manifest (CBOR or TOML bytes)
    println!("Creating signature...");
    let signature = signing_key.sign(&manifest_bytes);
    let sig_json = serde_json::to_string_pretty(&signature)?;

    // Write signature file
    std::fs::write(temp_dir.path().join("MANIFEST.sig"), &sig_json)?;

    // Determine output path
    let output_path = output
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| package_path.to_path_buf());

    // Rebuild the package with signature
    println!("Writing signed package...");
    let output_file = File::create(&output_path)?;
    let encoder = GzEncoder::new(output_file, Compression::default());
    let mut archive = TarBuilder::new(encoder);

    // Add all files from temp directory
    archive.append_dir_all(".", temp_dir.path())?;

    let encoder = archive.into_inner()?;
    encoder.finish()?;

    println!();
    println!("Package signed successfully!");
    println!("  Output: {}", output_path.display());
    if let Some(key_id) = signing_key.key_id() {
        println!("  Key ID: {}", key_id);
    }
    println!("  Timestamp: {}", signature.timestamp.as_deref().unwrap_or("none"));

    Ok(())
}
