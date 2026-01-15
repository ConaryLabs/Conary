// src/commands/ccs.rs
//! CCS package format commands
//!
//! Commands for creating, building, and inspecting CCS packages.

use anyhow::{Context, Result};
use conary::ccs::{builder, inspector, legacy, verify, CcsBuilder, CcsManifest, InspectedPackage, TrustPolicy};
use std::path::Path;

/// Initialize a new CCS manifest in the given directory
pub fn cmd_ccs_init(
    path: &str,
    name: Option<String>,
    version: &str,
    force: bool,
) -> Result<()> {
    let dir = Path::new(path);
    let manifest_path = dir.join("ccs.toml");

    // Check if manifest already exists
    if manifest_path.exists() && !force {
        anyhow::bail!(
            "ccs.toml already exists at {}. Use --force to overwrite.",
            manifest_path.display()
        );
    }

    // Determine package name
    let pkg_name = name.unwrap_or_else(|| {
        dir.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("my-package")
            .to_string()
    });

    // Try to detect existing project metadata
    let manifest = detect_project_and_create_manifest(dir, &pkg_name, version)?;

    // Write the manifest
    let toml = manifest.to_toml().context("Failed to serialize manifest")?;
    std::fs::write(&manifest_path, toml).context("Failed to write ccs.toml")?;

    println!("Created {}", manifest_path.display());
    println!();
    println!("Package: {} v{}", manifest.package.name, manifest.package.version);
    println!();
    println!("Next steps:");
    println!("  1. Edit ccs.toml to add dependencies and hooks");
    println!("  2. Run 'conary ccs-build' to create the package");

    Ok(())
}

/// Detect existing project files and create an appropriate manifest
fn detect_project_and_create_manifest(
    dir: &Path,
    name: &str,
    version: &str,
) -> Result<CcsManifest> {
    let mut manifest = CcsManifest::new_minimal(name, version);

    // Check for Cargo.toml (Rust project)
    let cargo_toml = dir.join("Cargo.toml");
    if cargo_toml.exists() {
        if let Ok(content) = std::fs::read_to_string(&cargo_toml)
            && let Ok(cargo) = content.parse::<toml::Table>()
            && let Some(package) = cargo.get("package").and_then(|p| p.as_table())
        {
            if let Some(n) = package.get("name").and_then(|v| v.as_str()) {
                manifest.package.name = n.to_string();
            }
            if let Some(v) = package.get("version").and_then(|v| v.as_str()) {
                manifest.package.version = v.to_string();
            }
            if let Some(d) = package.get("description").and_then(|v| v.as_str()) {
                manifest.package.description = d.to_string();
            }
            if let Some(l) = package.get("license").and_then(|v| v.as_str()) {
                manifest.package.license = Some(l.to_string());
            }
            if let Some(h) = package.get("homepage").and_then(|v| v.as_str()) {
                manifest.package.homepage = Some(h.to_string());
            }
            if let Some(r) = package.get("repository").and_then(|v| v.as_str()) {
                manifest.package.repository = Some(r.to_string());
            }
        }
        println!("Detected Rust project (Cargo.toml)");
    }

    // Check for package.json (Node.js project)
    let package_json = dir.join("package.json");
    if package_json.exists() {
        if let Ok(content) = std::fs::read_to_string(&package_json)
            && let Ok(pkg) = serde_json::from_str::<serde_json::Value>(&content)
        {
            if let Some(n) = pkg.get("name").and_then(|v| v.as_str()) {
                manifest.package.name = n.to_string();
            }
            if let Some(v) = pkg.get("version").and_then(|v| v.as_str()) {
                manifest.package.version = v.to_string();
            }
            if let Some(d) = pkg.get("description").and_then(|v| v.as_str()) {
                manifest.package.description = d.to_string();
            }
            if let Some(l) = pkg.get("license").and_then(|v| v.as_str()) {
                manifest.package.license = Some(l.to_string());
            }
        }
        println!("Detected Node.js project (package.json)");
    }

    // Check for pyproject.toml (Python project)
    let pyproject = dir.join("pyproject.toml");
    if pyproject.exists() {
        if let Ok(content) = std::fs::read_to_string(&pyproject)
            && let Ok(py) = content.parse::<toml::Table>()
            && let Some(project) = py.get("project").and_then(|p| p.as_table())
        {
            if let Some(n) = project.get("name").and_then(|v| v.as_str()) {
                manifest.package.name = n.to_string();
            }
            if let Some(v) = project.get("version").and_then(|v| v.as_str()) {
                manifest.package.version = v.to_string();
            }
            if let Some(d) = project.get("description").and_then(|v| v.as_str()) {
                manifest.package.description = d.to_string();
            }
        }
        println!("Detected Python project (pyproject.toml)");
    }

    Ok(manifest)
}

/// Build a CCS package from a manifest
pub fn cmd_ccs_build(
    path: &str,
    output: &str,
    target: &str,
    source: Option<String>,
    no_classify: bool,
    dry_run: bool,
) -> Result<()> {
    let path = Path::new(path);

    // Find the manifest
    let manifest_path = if path.is_file() && path.file_name().map(|n| n == "ccs.toml").unwrap_or(false) {
        path.to_path_buf()
    } else if path.is_dir() {
        path.join("ccs.toml")
    } else {
        anyhow::bail!("Cannot find ccs.toml at {}", path.display());
    };

    if !manifest_path.exists() {
        anyhow::bail!(
            "No ccs.toml found at {}. Run 'conary ccs-init' first.",
            manifest_path.display()
        );
    }

    // Parse the manifest
    let manifest = CcsManifest::from_file(&manifest_path)
        .context("Failed to parse ccs.toml")?;

    println!("Building {} v{}", manifest.package.name, manifest.package.version);

    // Determine source directory
    let source_dir = source
        .as_ref()
        .map(|s| Path::new(s).to_path_buf())
        .unwrap_or_else(|| manifest_path.parent().unwrap().to_path_buf());

    // Parse targets
    let targets: Vec<&str> = if target == "all" {
        vec!["ccs", "deb", "rpm", "arch"]
    } else {
        target.split(',').collect()
    };

    // Create output directory
    let output_dir = Path::new(output);
    if !dry_run {
        std::fs::create_dir_all(output_dir)
            .context("Failed to create output directory")?;
    }

    // Build the package data (needed for all targets)
    let build_result = if !dry_run {
        println!("Scanning source directory: {}", source_dir.display());

        let mut builder_instance = CcsBuilder::new(manifest.clone(), &source_dir);
        if no_classify {
            builder_instance = builder_instance.no_classify();
        }

        let result = builder_instance.build()
            .context("Failed to build package")?;

        builder::print_build_summary(&result);
        Some(result)
    } else {
        None
    };

    if dry_run {
        println!();
        println!("[DRY RUN] Would build:");
    }

    for t in &targets {
        let filename = match *t {
            "ccs" => format!("{}-{}.ccs", manifest.package.name, manifest.package.version),
            "deb" => format!("{}_{}_amd64.deb", manifest.package.name, manifest.package.version),
            "rpm" => format!("{}-{}.x86_64.rpm", manifest.package.name, manifest.package.version),
            "arch" => format!("{}-{}-x86_64.pkg.tar.zst", manifest.package.name, manifest.package.version),
            _ => {
                println!("Unknown target format: {}", t);
                continue;
            }
        };

        let output_path = output_dir.join(&filename);

        if dry_run {
            println!("  {} -> {}", t, output_path.display());
        } else {
            let result = build_result.as_ref().unwrap();

            match *t {
                "ccs" => {
                    println!();
                    println!("Writing CCS package...");
                    builder::write_ccs_package(result, &output_path)
                        .context("Failed to write CCS package")?;
                    println!("  Created: {}", output_path.display());
                }
                "deb" => {
                    println!();
                    println!("Generating DEB package...");
                    let gen_result = legacy::deb::generate(result, &output_path)
                        .context("Failed to generate DEB package")?;
                    println!("  Created: {} ({} bytes)", output_path.display(), gen_result.size);
                    gen_result.loss_report.print_summary("DEB");
                }
                "rpm" => {
                    println!();
                    println!("Generating RPM package...");
                    let gen_result = legacy::rpm::generate(result, &output_path)
                        .context("Failed to generate RPM package")?;
                    println!("  Created: {} ({} bytes)", output_path.display(), gen_result.size);
                    gen_result.loss_report.print_summary("RPM");
                }
                "arch" => {
                    println!();
                    println!("Generating Arch package...");
                    let gen_result = legacy::arch::generate(result, &output_path)
                        .context("Failed to generate Arch package")?;
                    println!("  Created: {} ({} bytes)", output_path.display(), gen_result.size);
                    gen_result.loss_report.print_summary("Arch");
                }
                _ => {}
            }
        }
    }

    if !dry_run {
        println!();
        println!("Build complete!");
    }

    Ok(())
}

/// Inspect a CCS package
pub fn cmd_ccs_inspect(
    package: &str,
    show_files: bool,
    show_hooks: bool,
    show_deps: bool,
    format: &str,
) -> Result<()> {
    let path = Path::new(package);

    if !path.exists() {
        anyhow::bail!("Package not found: {}", package);
    }

    // Load and parse the package
    let pkg = InspectedPackage::from_file(path)
        .context("Failed to read CCS package")?;

    // Output in requested format
    if format == "json" {
        inspector::print_json(&pkg, show_files, show_hooks, show_deps)?;
    } else {
        // Human-readable output
        inspector::print_summary(&pkg);

        if show_files {
            println!();
            inspector::print_files(&pkg);
        }

        if show_hooks {
            println!();
            inspector::print_hooks(&pkg);
        }

        if show_deps {
            println!();
            inspector::print_dependencies(&pkg);
        }
    }

    Ok(())
}

/// Verify a CCS package signature and contents
pub fn cmd_ccs_verify(
    package: &str,
    policy_path: Option<String>,
    allow_unsigned: bool,
) -> Result<()> {
    let path = Path::new(package);

    if !path.exists() {
        anyhow::bail!("Package not found: {}", package);
    }

    println!("Verifying: {}", path.display());
    println!();

    // Load or create trust policy
    let policy = if let Some(policy_file) = policy_path {
        TrustPolicy::from_file(Path::new(&policy_file))
            .context("Failed to load trust policy")?
    } else if allow_unsigned {
        TrustPolicy::permissive()
    } else {
        // Default policy: allow unsigned but warn
        TrustPolicy {
            allow_unsigned: true,
            ..Default::default()
        }
    };

    // Run verification
    let result = verify::verify_package(path, &policy)
        .context("Verification failed")?;

    // Print results
    verify::print_result(&result);

    // Return error if verification failed
    if !result.valid {
        anyhow::bail!("Package verification failed");
    }

    Ok(())
}

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

    // Extract all files and find MANIFEST.toml
    let mut manifest_content: Option<String> = None;

    for entry in archive.entries()? {
        let mut entry = entry?;
        let entry_path = entry.path()?.to_path_buf();

        // Extract to temp dir
        entry.unpack(temp_dir.path().join(&entry_path))?;

        // Capture manifest content
        let entry_path_str = entry_path.to_string_lossy();
        if entry_path_str == "MANIFEST.toml" || entry_path_str == "./MANIFEST.toml" {
            manifest_content = Some(std::fs::read_to_string(temp_dir.path().join(&entry_path))?);
        }
    }

    let manifest_content = manifest_content
        .ok_or_else(|| anyhow::anyhow!("Package missing MANIFEST.toml"))?;

    // Sign the manifest
    println!("Creating signature...");
    let signature = signing_key.sign(manifest_content.as_bytes());
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

/// Install a CCS package
///
/// This is a minimal implementation that validates and extracts the package.
/// Full transaction support will be added in a future iteration.
#[allow(clippy::too_many_arguments)]
pub fn cmd_ccs_install(
    package: &str,
    db_path: &str,
    root: &str,
    dry_run: bool,
    allow_unsigned: bool,
    policy: Option<String>,
    _components: Option<Vec<String>>,
    _sandbox: super::SandboxMode,
) -> Result<()> {
    use conary::ccs::{CcsPackage, HookExecutor};
    use conary::packages::traits::PackageFormat;

    let package_path = Path::new(package);

    if !package_path.exists() {
        anyhow::bail!("Package not found: {}", package);
    }

    println!("Installing CCS package: {}", package_path.display());

    // Step 1: Verify signature (unless --allow-unsigned)
    if !allow_unsigned {
        let trust_policy = if let Some(policy_path) = &policy {
            TrustPolicy::from_file(Path::new(policy_path))
                .context("Failed to load trust policy")?
        } else {
            TrustPolicy::default()
        };

        let result = verify::verify_package(package_path, &trust_policy)?;
        if !result.valid {
            if trust_policy.allow_unsigned {
                println!("Warning: Package signature verification failed, but continuing (allow_unsigned policy)");
                for warning in &result.warnings {
                    println!("  - {}", warning);
                }
            } else {
                anyhow::bail!(
                    "Package signature verification failed. Use --allow-unsigned to install anyway.\n  Signature: {:?}\n  Content: {:?}",
                    result.signature_status,
                    result.content_status
                );
            }
        } else {
            println!("Signature verified: {:?}", result.signature_status);
        }
    } else {
        println!("Warning: Skipping signature verification (--allow-unsigned)");
    }

    // Step 2: Parse the package
    println!("Parsing package...");
    let ccs_pkg = CcsPackage::parse(package)?;

    println!(
        "Package: {} v{} ({} files)",
        ccs_pkg.name(),
        ccs_pkg.version(),
        ccs_pkg.files().len()
    );

    // Step 3: Check for existing installation
    let conn = conary::db::open(db_path).context("Failed to open package database")?;

    let existing = conary::db::models::Trove::find_by_name(&conn, ccs_pkg.name())?;
    if !existing.is_empty() {
        let old = &existing[0];
        if old.version == ccs_pkg.version() {
            anyhow::bail!(
                "Package {} version {} is already installed",
                ccs_pkg.name(),
                ccs_pkg.version()
            );
        }
        println!("Upgrading {} from {} to {}", ccs_pkg.name(), old.version, ccs_pkg.version());
    }

    // Step 4: Check dependencies
    println!("Checking dependencies...");
    for dep in ccs_pkg.dependencies() {
        let satisfied = conary::db::models::ProvideEntry::is_capability_satisfied(&conn, &dep.name)?;
        if !satisfied {
            let pkg_exists = conary::db::models::Trove::find_by_name(&conn, &dep.name)?;
            if pkg_exists.is_empty() {
                if dry_run {
                    println!("  Missing dependency: {} (would fail)", dep.name);
                } else {
                    anyhow::bail!(
                        "Missing dependency: {}{}",
                        dep.name,
                        dep.version
                            .as_ref()
                            .map(|v| format!(" {}", v))
                            .unwrap_or_default()
                    );
                }
            }
        }
    }
    println!("Dependencies satisfied.");

    if dry_run {
        println!();
        println!("[DRY RUN] Would install {} files:", ccs_pkg.files().len());
        for file in ccs_pkg.files().iter().take(10) {
            println!("  {}", file.path);
        }
        if ccs_pkg.files().len() > 10 {
            println!("  ... and {} more", ccs_pkg.files().len() - 10);
        }
        return Ok(());
    }

    // Step 5: Extract file contents
    println!("Extracting files...");
    let extracted_files = ccs_pkg.extract_file_contents()?;
    println!("Extracted {} files", extracted_files.len());

    // Step 6: Execute pre-hooks
    let mut hook_executor = HookExecutor::new(Path::new(root));
    let hooks = &ccs_pkg.manifest().hooks;

    if !hooks.users.is_empty() || !hooks.groups.is_empty() || !hooks.directories.is_empty() {
        println!("Executing pre-install hooks...");
        if let Err(e) = hook_executor.execute_pre_hooks(hooks) {
            anyhow::bail!("Pre-install hook failed: {}", e);
        }
    }

    // Step 7: Deploy files to filesystem
    println!("Deploying files to filesystem...");
    let root_path = std::path::Path::new(root);
    let mut files_deployed = 0;

    for file in &extracted_files {
        let dest_path = if file.path.starts_with('/') {
            root_path.join(file.path.trim_start_matches('/'))
        } else {
            root_path.join(&file.path)
        };

        // Create parent directories
        if let Some(parent) = dest_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Write file
        std::fs::write(&dest_path, &file.content)?;

        // Set permissions
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dest_path, std::fs::Permissions::from_mode(file.mode as u32))?;
        }

        files_deployed += 1;
    }

    println!("Deployed {} files to {}", files_deployed, root);

    // Step 8: Register in database
    println!("Updating database...");
    {
        let tx = conn.unchecked_transaction()?;

        // Create trove
        let mut trove = ccs_pkg.to_trove();
        let trove_id = trove.insert(&tx)?;

        // Create provides entry for the package itself
        let mut provide = conary::db::models::ProvideEntry::new(
            trove_id,
            ccs_pkg.name().to_string(),
            Some(ccs_pkg.version().to_string()),
        );
        provide.insert(&tx)?;

        tx.commit()?;
    }

    // Step 9: Execute post-hooks
    if !hooks.systemd.is_empty()
        || !hooks.tmpfiles.is_empty()
        || !hooks.sysctl.is_empty()
        || !hooks.alternatives.is_empty()
    {
        println!("Executing post-install hooks...");
        if let Err(e) = hook_executor.execute_post_hooks(hooks) {
            println!("Warning: Post-install hook failed: {}", e);
        }
    }

    println!();
    println!("Successfully installed {} v{}", ccs_pkg.name(), ccs_pkg.version());

    Ok(())
}
