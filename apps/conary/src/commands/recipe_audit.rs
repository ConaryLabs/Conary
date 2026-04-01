// src/commands/recipe_audit.rs

//! Implementation of `conary recipe-audit` command.

use std::path::Path;

use anyhow::Result;
use conary_core::recipe::audit::{FindingKind, static_audit};

/// Run recipe audit on a single recipe or all recipes.
pub async fn cmd_recipe_audit(recipe_path: Option<&str>, all: bool, trace: bool) -> Result<()> {
    if trace {
        println!("--trace mode is not yet implemented. Running static analysis only.");
    }

    if all {
        return audit_all_recipes();
    }

    let path = recipe_path.ok_or_else(|| anyhow::anyhow!("provide a recipe path or use --all"))?;

    let recipe = conary_core::recipe::parse_recipe_file(Path::new(path))
        .map_err(|e| anyhow::anyhow!("failed to parse recipe {path}: {e}"))?;

    match static_audit(&recipe) {
        Ok(report) => {
            println!(
                "\nStatic analysis of {}-{}:",
                report.package_name, report.package_version
            );

            for finding in &report.findings {
                match finding.kind {
                    FindingKind::Missing => {
                        println!(
                            "  [WARN] '{}' used in {} but not in makedepends",
                            finding.tool, finding.context
                        );
                    }
                    FindingKind::Verified => {
                        println!("  [OK]   '{}' declared and used", finding.tool);
                    }
                    FindingKind::Ignored => {}
                }
            }

            let missing = report.count(FindingKind::Missing);
            let verified = report.count(FindingKind::Verified);

            println!("\n  {verified} verified, {missing} potential missing dependencies found.");
            if missing > 0 {
                println!("  Run with --trace for build-time verification.");
            }
        }
        Err(e) => eprintln!("  audit error: {e}"),
    }

    Ok(())
}

fn audit_all_recipes() -> Result<()> {
    let recipes_dir = Path::new("recipes");
    if !recipes_dir.exists() {
        anyhow::bail!("recipes/ directory not found in current directory");
    }

    let mut total = 0;
    let mut total_missing = 0;

    for entry in walkdir::WalkDir::new(recipes_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "toml"))
    {
        let path = entry.path();
        let recipe = match conary_core::recipe::parse_recipe_file(path) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[WARN] skipping {}: {e}", path.display());
                continue;
            }
        };

        match static_audit(&recipe) {
            Ok(report) => {
                let missing = report.count(FindingKind::Missing);
                if missing > 0 {
                    println!("\n{} ({}):", report.package_name, path.display());
                    for f in &report.findings {
                        if f.kind == FindingKind::Missing {
                            println!("  [WARN] '{}' used but not in makedepends", f.tool);
                        }
                    }
                    total_missing += missing;
                }
                total += 1;
            }
            Err(_) => continue,
        }
    }

    println!("\nAudited {total} recipes. {total_missing} potential missing dependencies found.");
    Ok(())
}
