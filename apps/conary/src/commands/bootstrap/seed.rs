// apps/conary/src/commands/bootstrap/seed.rs

use std::path::PathBuf;

use anyhow::{Context, Result};

/// Package cross-tools output as a derivation seed
pub async fn cmd_bootstrap_seed(from: &str, output: &str, target: &str) -> Result<()> {
    use conary_core::derivation::compose::erofs_image_hash;
    use conary_core::derivation::seed::{SeedMetadata, SeedSource};
    use conary_core::filesystem::CasStore;
    use conary_core::generation::builder::{FileEntryRef, SymlinkEntryRef, build_erofs_image};
    use std::os::unix::fs::MetadataExt;
    use walkdir::WalkDir;

    let from_path = PathBuf::from(from);
    let output_path = PathBuf::from(output);

    // Validate input
    if !from_path.exists() {
        return Err(anyhow::anyhow!(
            "Cross-tools directory not found: {}",
            from_path.display()
        ));
    }
    if !from_path.join("bin").exists() && !from_path.join("lib").exists() {
        return Err(anyhow::anyhow!(
            "Directory does not look like a cross-toolchain (no bin/ or lib/): {}",
            from_path.display()
        ));
    }

    println!("Creating seed from cross-tools output...");
    println!("  Source: {}", from_path.display());
    println!("  Output: {}", output_path.display());
    println!("  Target: {target}");

    // Create output structure
    std::fs::create_dir_all(&output_path)?;
    let cas_dir = output_path.join("cas");
    let cas = CasStore::new(&cas_dir).context("Failed to create CAS store")?;

    // Walk source tree, store files in CAS, collect entries
    let mut file_entries = Vec::new();
    let mut symlink_entries = Vec::new();
    let mut file_count: u64 = 0;

    for entry in WalkDir::new(&from_path).follow_links(false) {
        let entry = entry.context("Failed to walk directory")?;
        let rel_path = entry
            .path()
            .strip_prefix(&from_path)
            .context("Failed to compute relative path")?;

        // Skip the root directory itself
        if rel_path.as_os_str().is_empty() {
            continue;
        }

        let abs_path = format!("/tools/{}", rel_path.display());
        let metadata = entry.path().symlink_metadata()?;

        if metadata.is_symlink() {
            let link_target = std::fs::read_link(entry.path())?;
            symlink_entries.push(SymlinkEntryRef {
                path: abs_path,
                target: link_target.to_string_lossy().to_string(),
            });
        } else if metadata.is_file() {
            let content = std::fs::read(entry.path())?;
            let hash = cas.store(&content).context("CAS store failed")?;
            file_entries.push(FileEntryRef {
                path: abs_path,
                sha256_hash: hash,
                size: metadata.len(),
                permissions: metadata.mode() & 0o7777,
                owner: None,
                group_name: None,
            });
            file_count += 1;
        }
        // Directories are implicit in EROFS
    }

    println!(
        "  Stored {} files, {} symlinks in CAS",
        file_count,
        symlink_entries.len()
    );

    // Build EROFS image
    let gen_dir = output_path.join("gen");
    std::fs::create_dir_all(&gen_dir)?;
    let build_result = build_erofs_image(&file_entries, &symlink_entries, &gen_dir)
        .context("Failed to build EROFS image")?;

    // Move EROFS image to seed.erofs
    let seed_erofs = output_path.join("seed.erofs");
    std::fs::rename(&build_result.image_path, &seed_erofs)?;
    // Clean up temp gen dir
    let _ = std::fs::remove_dir_all(&gen_dir);

    // Compute image hash
    let seed_id = erofs_image_hash(&seed_erofs).context("Failed to hash seed EROFS image")?;

    // Write seed.toml
    let seed_metadata = SeedMetadata {
        seed_id: seed_id.clone(),
        source: SeedSource::SelfBuilt,
        origin_url: None,
        builder: Some("conary-bootstrap".to_string()),
        packages: vec![
            "binutils-pass1".to_string(),
            "gcc-pass1".to_string(),
            "linux-headers".to_string(),
            "glibc".to_string(),
            "libstdcxx".to_string(),
        ],
        target_triple: target.to_string(),
        verified_by: vec![],
        origin_distro: None,
        origin_version: None,
    };

    let toml_str =
        toml::to_string_pretty(&seed_metadata).context("Failed to serialize seed metadata")?;
    std::fs::write(output_path.join("seed.toml"), &toml_str)?;

    println!("\n[OK] Seed created successfully!");
    println!(
        "  EROFS image: {} ({} bytes)",
        seed_erofs.display(),
        build_result.image_size
    );
    println!("  CAS objects: {file_count}");
    println!("  Seed ID: {}", &seed_id[..16]);

    Ok(())
}

/// Create a seed from the currently adopted system filesystem
pub async fn cmd_bootstrap_seed_adopted(
    output: &str,
    distro: Option<&str>,
    distro_version: Option<&str>,
) -> Result<()> {
    use conary_core::bootstrap::adopt_seed;

    let distro_name = distro.unwrap_or("unknown");
    let version = distro_version.unwrap_or("unknown");

    println!("Building adopted seed from system filesystem...");
    println!("  Distro: {distro_name} {version}");
    println!("  Output: {output}");

    let meta = adopt_seed::build_adopted_seed(std::path::Path::new(output), distro_name, version)?;

    println!("[COMPLETE] Seed built: {}", meta.seed_id);
    Ok(())
}
