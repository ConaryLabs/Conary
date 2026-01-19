// src/commands/derived.rs

//! Derived package management CLI commands
//!
//! Commands for creating and managing derived packages - custom versions
//! of existing packages with patches and file overrides.

use anyhow::Result;
use conary::db::paths::objects_dir;
use std::path::Path;
use tracing::info;

use conary::db::models::{
    DerivedOverride, DerivedPackage, DerivedPatch, DerivedStatus, VersionPolicy,
};

/// List all derived packages
pub fn cmd_derive_list(db_path: &str, verbose: bool) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    let derived = DerivedPackage::list_all(&conn)?;

    if derived.is_empty() {
        println!("No derived packages defined.");
        println!("\nUse 'conary derive <name> --from <parent>' to create a derived package.");
        return Ok(());
    }

    println!("Derived packages ({}):", derived.len());
    for pkg in &derived {
        let status_str = match pkg.status {
            DerivedStatus::Pending => "[PENDING]",
            DerivedStatus::Built => "[BUILT]",
            DerivedStatus::Stale => "[STALE]",
            DerivedStatus::Error => "[ERROR]",
        };

        if verbose {
            println!("  {} {} <- {}", pkg.name, status_str, pkg.parent_name);
            if let Some(desc) = &pkg.description {
                println!("    Description: {}", desc);
            }
            match &pkg.version_policy {
                VersionPolicy::Inherit => println!("    Version: inherit from parent"),
                VersionPolicy::Suffix(s) => println!("    Version: parent{}", s),
                VersionPolicy::Specific(v) => println!("    Version: {}", v),
            }
            if let Ok(patches) = pkg.patches(&conn)
                && !patches.is_empty()
            {
                println!("    Patches: {}", patches.len());
            }
            if let Ok(overrides) = pkg.overrides(&conn)
                && !overrides.is_empty()
            {
                println!("    Overrides: {}", overrides.len());
            }
            if let Some(msg) = &pkg.error_message {
                println!("    Error: {}", msg);
            }
        } else {
            println!("  {} {} <- {}", pkg.name, status_str, pkg.parent_name);
        }
    }

    Ok(())
}

/// Show details of a derived package
pub fn cmd_derive_show(name: &str, db_path: &str) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    let derived = DerivedPackage::find_by_name(&conn, name)?
        .ok_or_else(|| anyhow::anyhow!("Derived package '{}' not found", name))?;

    println!("Derived Package: {}", derived.name);
    println!("Parent: {}", derived.parent_name);
    if let Some(v) = &derived.parent_version {
        println!("Parent Version Constraint: {}", v);
    }

    println!("Version Policy: {}", match &derived.version_policy {
        VersionPolicy::Inherit => "inherit".to_string(),
        VersionPolicy::Suffix(s) => format!("suffix({})", s),
        VersionPolicy::Specific(v) => format!("specific({})", v),
    });

    println!("Status: {}", derived.status.as_str());
    if let Some(msg) = &derived.error_message {
        println!("Error Message: {}", msg);
    }

    if let Some(desc) = &derived.description {
        println!("Description: {}", desc);
    }

    // Show patches
    let patches = derived.patches(&conn)?;
    if !patches.is_empty() {
        println!("\nPatches ({}):", patches.len());
        for patch in &patches {
            println!("  {}. {} (strip -p{})", patch.patch_order, patch.patch_name, patch.strip_level);
        }
    }

    // Show overrides
    let overrides = derived.overrides(&conn)?;
    if !overrides.is_empty() {
        println!("\nFile Overrides ({}):", overrides.len());
        for ov in &overrides {
            if ov.is_removal() {
                println!("  [REMOVE] {}", ov.target_path);
            } else {
                println!("  [REPLACE] {}", ov.target_path);
                if let Some(perms) = ov.permissions {
                    println!("    Permissions: {:o}", perms);
                }
            }
        }
    }

    // Show timestamps
    if let Some(created) = &derived.created_at {
        println!("\nCreated: {}", created);
    }
    if let Some(updated) = &derived.updated_at {
        println!("Updated: {}", updated);
    }

    Ok(())
}

/// Create a new derived package
pub fn cmd_derive_create(
    name: &str,
    parent: &str,
    version_suffix: Option<&str>,
    description: Option<&str>,
    db_path: &str,
) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    // Check if already exists
    if DerivedPackage::find_by_name(&conn, name)?.is_some() {
        return Err(anyhow::anyhow!("Derived package '{}' already exists", name));
    }

    // Create the derived package
    let mut derived = DerivedPackage::new(name.to_string(), parent.to_string());
    derived.description = description.map(String::from);

    if let Some(suffix) = version_suffix {
        derived.version_policy = VersionPolicy::Suffix(suffix.to_string());
    }

    let id = derived.insert(&conn)?;
    info!("Created derived package '{}' with id={}", name, id);

    println!("Created derived package: {}", name);
    println!("  Parent: {}", parent);
    if let Some(suffix) = version_suffix {
        println!("  Version suffix: {}", suffix);
    }
    if let Some(desc) = description {
        println!("  Description: {}", desc);
    }
    println!("\nUse 'conary derive-patch {} <patch-file>' to add patches.", name);
    println!("Use 'conary derive-override {} <target> <source>' to override files.", name);
    println!("Use 'conary derive-build {}' to build the derived package.", name);

    Ok(())
}

/// Add a patch to a derived package
pub fn cmd_derive_patch(
    name: &str,
    patch_file: &str,
    strip_level: Option<i32>,
    db_path: &str,
) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    let derived = DerivedPackage::find_by_name(&conn, name)?
        .ok_or_else(|| anyhow::anyhow!("Derived package '{}' not found", name))?;

    let derived_id = derived.id.unwrap();

    // Read patch content
    let patch_content = std::fs::read(patch_file)?;
    let patch_hash = conary::hash::sha256(&patch_content);
    let patch_name = Path::new(patch_file)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("patch")
        .to_string();

    // Get existing patches to determine order
    let existing = DerivedPatch::find_by_derived(&conn, derived_id)?;
    let patch_order = existing.len() as i32 + 1;

    // Create patch entry
    let mut patch = DerivedPatch::new(derived_id, patch_order, patch_name.clone(), patch_hash);
    patch.strip_level = strip_level.unwrap_or(1);
    patch.insert(&conn)?;

    // Store patch content in CAS
    let objects_dir = objects_dir(db_path);
    let cas = conary::filesystem::CasStore::new(&objects_dir)?;
    cas.store(&patch_content)?;

    info!("Added patch '{}' to derived package '{}'", patch_name, name);
    println!("Added patch to {}: {} (order {}, strip -p{})",
        name, patch_name, patch_order, patch.strip_level);

    Ok(())
}

/// Add a file override to a derived package
pub fn cmd_derive_override(
    name: &str,
    target_path: &str,
    source_file: Option<&str>,
    permissions: Option<u32>,
    db_path: &str,
) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    let derived = DerivedPackage::find_by_name(&conn, name)?
        .ok_or_else(|| anyhow::anyhow!("Derived package '{}' not found", name))?;

    let derived_id = derived.id.unwrap();

    // Check if override already exists
    if DerivedOverride::find_by_path(&conn, derived_id, target_path)?.is_some() {
        return Err(anyhow::anyhow!(
            "Override for '{}' already exists. Use 'conary derive-remove-override' first.",
            target_path
        ));
    }

    if let Some(source) = source_file {
        // Replace file
        let content = std::fs::read(source)?;
        let source_hash = conary::hash::sha256(&content);

        // Store content in CAS
        let objects_dir = objects_dir(db_path);
        let cas = conary::filesystem::CasStore::new(&objects_dir)?;
        cas.store(&content)?;

        let mut ov = DerivedOverride::new_replace(derived_id, target_path.to_string(), source_hash);
        ov.source_path = Some(source.to_string());
        ov.permissions = permissions.map(|p| p as i32);
        ov.insert(&conn)?;

        println!("Added file override to {}: {} <- {}", name, target_path, source);
    } else {
        // Remove file
        let mut ov = DerivedOverride::new_remove(derived_id, target_path.to_string());
        ov.insert(&conn)?;

        println!("Added file removal to {}: {}", name, target_path);
    }

    Ok(())
}

/// Build a derived package
pub fn cmd_derive_build(name: &str, db_path: &str) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    let mut derived = DerivedPackage::find_by_name(&conn, name)?
        .ok_or_else(|| anyhow::anyhow!("Derived package '{}' not found", name))?;

    // Check parent is installed
    let parent_installed = conary::db::models::Trove::find_by_name(&conn, &derived.parent_name)?;
    if parent_installed.is_empty() {
        return Err(anyhow::anyhow!(
            "Parent package '{}' is not installed. Install it first.",
            derived.parent_name
        ));
    }

    // Get CAS
    let objects_dir = objects_dir(db_path);
    let cas = conary::filesystem::CasStore::new(&objects_dir)?;

    println!("Building derived package '{}' from '{}'...", name, derived.parent_name);

    // Build the derived package
    let result = conary::derived::build_from_definition(&conn, &derived, &cas);

    match result {
        Ok(build_result) => {
            println!("Build successful:");
            println!("  Version: {}", build_result.version);
            println!("  Files: {}", build_result.files.len());
            println!("  Patches applied: {}", build_result.patches_applied.len());
            println!("  Files overridden: {}", build_result.files_overridden.len());
            println!("  Files removed: {}", build_result.files_removed.len());

            // Mark as built (for now, without actually creating the trove)
            // In a full implementation, we would create the trove and install the files
            derived.set_status(&conn, DerivedStatus::Built)?;
            println!("\nDerived package '{}' is ready.", name);
        }
        Err(e) => {
            let error_msg: String = e.to_string();
            derived.mark_error(&conn, &error_msg)?;
            return Err(anyhow::anyhow!("Build failed: {}", error_msg));
        }
    }

    Ok(())
}

/// Delete a derived package
pub fn cmd_derive_delete(name: &str, db_path: &str) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    let derived = DerivedPackage::find_by_name(&conn, name)?
        .ok_or_else(|| anyhow::anyhow!("Derived package '{}' not found", name))?;

    let derived_id = derived.id.unwrap();
    DerivedPackage::delete(&conn, derived_id)?;

    println!("Deleted derived package: {}", name);
    Ok(())
}

/// List stale derived packages (parent was updated)
pub fn cmd_derive_stale(db_path: &str) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    let stale = DerivedPackage::find_by_status(&conn, DerivedStatus::Stale)?;

    if stale.is_empty() {
        println!("No stale derived packages.");
        return Ok(());
    }

    println!("Stale derived packages ({}):", stale.len());
    println!("(These need to be rebuilt because their parent was updated)");
    println!();
    for pkg in &stale {
        println!("  {} <- {}", pkg.name, pkg.parent_name);
    }

    println!();
    println!("Use 'conary derive-build <name>' to rebuild.");

    Ok(())
}

/// Mark all derived packages from a parent as stale
/// (Called internally when parent packages are updated)
#[allow(dead_code)]
pub fn cmd_derive_mark_stale(parent_name: &str, db_path: &str) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    let count = DerivedPackage::mark_stale(&conn, parent_name)?;

    if count > 0 {
        println!("Marked {} derived packages as stale (parent '{}' was updated).", count, parent_name);
    } else {
        println!("No derived packages found for parent '{}'.", parent_name);
    }

    Ok(())
}
