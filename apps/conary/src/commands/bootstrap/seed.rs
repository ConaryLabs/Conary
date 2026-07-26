// apps/conary/src/commands/bootstrap/seed.rs

use std::path::PathBuf;

use anyhow::{Context, Result};

/// Package cross-tools output as a derivation seed
pub async fn cmd_bootstrap_seed(from: &str, output: &str, target: &str) -> Result<()> {
    use conary_core::derivation::compose::erofs_image_hash;
    use conary_core::derivation::seed::{SeedMetadata, SeedSource};
    use conary_core::filesystem::CasStore;
    use conary_core::generation::root_manifest::{
        GENERATION_ROOT_MANIFEST_VERSION, GenerationRootEntry, GenerationRootManifest,
        build_erofs_image_from_root_manifest, scan_payload_tree,
    };
    use conary_core::payload::PayloadNodeKind;

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

    let (root, source_entries) =
        scan_payload_tree(&from_path, &cas, &format!("bootstrap-seed:{target}"))
            .context("Failed to capture exact cross-tools payload")?;
    let mut entries = Vec::with_capacity(source_entries.len() + 1);
    entries.push(GenerationRootEntry {
        path: "/tools".to_string(),
        node: root.clone(),
        content: None,
    });
    for mut entry in source_entries {
        entry.path = format!("/tools{}", entry.path);
        if let PayloadNodeKind::Hardlink { target, .. } = &mut entry.node.source.kind {
            *target = format!("/tools{target}");
        }
        entries.push(entry);
    }
    let manifest = GenerationRootManifest {
        version: GENERATION_ROOT_MANIFEST_VERSION,
        root,
        entries,
    };
    manifest
        .validate()
        .context("Cross-tools payload is not a valid exact generation root")?;
    let file_count = manifest.regular_contents().count() as u64;
    let symlink_count = manifest
        .entries
        .iter()
        .filter(|entry| matches!(entry.node.source.kind, PayloadNodeKind::Symlink { .. }))
        .count();

    println!(
        "  Stored {} files, {} symlinks in CAS",
        file_count, symlink_count
    );

    // Build EROFS image
    let gen_dir = output_path.join("gen");
    std::fs::create_dir_all(&gen_dir)?;
    let build_result = build_erofs_image_from_root_manifest(&manifest, &gen_dir)
        .context("Failed to build EROFS image")?;
    manifest
        .write_to(&output_path)
        .context("Failed to persist exact seed root manifest")?;

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

    println!();
    crate::ui::status("Created", "seed successfully.");
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

    crate::ui::status("Built", &format!("seed {}", meta.seed_id));
    Ok(())
}
