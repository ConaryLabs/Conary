// apps/conary/src/commands/bootstrap/cleanup.rs

use std::path::PathBuf;

use anyhow::Result;

/// Clean bootstrap work directory
pub async fn cmd_bootstrap_clean(
    work_dir: &str,
    stage: Option<String>,
    sources: bool,
) -> Result<()> {
    println!("Cleaning bootstrap work directory...");
    println!("  Work directory: {}", work_dir);

    let work_path = PathBuf::from(work_dir);

    if !work_path.exists() {
        println!("Work directory does not exist.");
        return Ok(());
    }

    if let Some(ref stage_name) = stage {
        // Validate stage name: only allow known stage directory names to
        // prevent path traversal (absolute paths, ".." segments, etc.).
        const ALLOWED_STAGES: &[&str] =
            &["cross-tools", "temp-tools", "system", "image", "sources"];
        if !ALLOWED_STAGES.contains(&stage_name.as_str()) {
            anyhow::bail!(
                "Invalid stage '{}'. Allowed stages: {}",
                stage_name,
                ALLOWED_STAGES.join(", ")
            );
        }

        // Clean specific stage
        let stage_dir = work_path.join(stage_name);
        if stage_dir.exists() {
            println!("  Removing: {}", stage_dir.display());
            std::fs::remove_dir_all(&stage_dir)?;
        } else {
            println!("  Stage directory not found: {}", stage_dir.display());
        }
    } else {
        // Clean everything except tarballs (unless --sources)
        for entry in std::fs::read_dir(&work_path)? {
            let entry = entry?;
            let path = entry.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

            if name == "tarballs" && !sources {
                println!("  Keeping: {}", path.display());
                continue;
            }

            if path.is_dir() {
                println!("  Removing: {}", path.display());
                std::fs::remove_dir_all(&path)?;
            } else {
                println!("  Removing: {}", path.display());
                std::fs::remove_file(&path)?;
            }
        }
    }

    println!("\n[OK] Clean complete.");

    Ok(())
}
