// src/commands/query/components.rs

//! Component query commands
//!
//! Functions for querying package components and their files.

use anyhow::Result;

/// List components of an installed package
pub fn cmd_list_components(package_name: &str, db_path: &str) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    // Find the package
    let troves = conary::db::models::Trove::find_by_name(&conn, package_name)?;
    if troves.is_empty() {
        return Err(anyhow::anyhow!("Package '{}' is not installed", package_name));
    }

    for trove in &troves {
        let trove_id = trove.id.ok_or_else(|| anyhow::anyhow!("Trove has no ID"))?;

        println!("Package: {} {}", trove.name, trove.version);
        if let Some(arch) = &trove.architecture {
            println!("  Architecture: {}", arch);
        }

        // Get components
        let components = conary::db::models::Component::find_by_trove(&conn, trove_id)?;

        if components.is_empty() {
            println!("  Components: (none - legacy install)");
        } else {
            println!("  Components:");
            for comp in &components {
                let file_count = conary::db::models::FileEntry::find_by_component(&conn, comp.id.unwrap_or(0))?.len();
                let default_marker = if conary::components::ComponentType::parse(&comp.name)
                    .map(|ct| ct.is_default())
                    .unwrap_or(false)
                {
                    " (default)"
                } else {
                    ""
                };
                let installed_marker = if comp.is_installed { "" } else { " [not installed]" };
                println!(
                    "    :{} - {} files{}{}",
                    comp.name, file_count, default_marker, installed_marker
                );
            }
        }
        println!();
    }

    Ok(())
}

/// Query files in a specific component
pub fn cmd_query_component(component_spec: &str, db_path: &str) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    // Parse the component spec (e.g., "nginx:lib")
    let (package_name, component_name) = conary::components::parse_component_spec(component_spec)
        .ok_or_else(|| anyhow::anyhow!(
            "Invalid component spec '{}'. Expected format: package:component (e.g., nginx:lib)",
            component_spec
        ))?;

    // Find the package
    let troves = conary::db::models::Trove::find_by_name(&conn, &package_name)?;
    if troves.is_empty() {
        return Err(anyhow::anyhow!("Package '{}' is not installed", package_name));
    }

    for trove in &troves {
        let trove_id = trove.id.ok_or_else(|| anyhow::anyhow!("Trove has no ID"))?;

        // Find the component
        let component = conary::db::models::Component::find_by_trove_and_name(&conn, trove_id, &component_name)?;

        match component {
            Some(comp) => {
                let comp_id = comp.id.ok_or_else(|| anyhow::anyhow!("Component has no ID"))?;
                let files = conary::db::models::FileEntry::find_by_component(&conn, comp_id)?;

                println!("{}:{} ({} {})", package_name, component_name, trove.name, trove.version);
                if let Some(desc) = &comp.description {
                    println!("  Description: {}", desc);
                }
                println!("  Files: {}", files.len());
                println!();

                for file in &files {
                    println!("  {}", file.path);
                }
            }
            None => {
                // Check if any components exist
                let components = conary::db::models::Component::find_by_trove(&conn, trove_id)?;
                if components.is_empty() {
                    println!(
                        "Package '{}' was installed without component tracking (legacy install)",
                        package_name
                    );
                    println!("All files belong to the implicit :runtime component.");
                } else {
                    let available: Vec<String> = components.iter().map(|c| format!(":{}", c.name)).collect();
                    return Err(anyhow::anyhow!(
                        "Component '{}' not found in package '{}'. Available: {}",
                        component_name,
                        package_name,
                        available.join(", ")
                    ));
                }
            }
        }
    }

    Ok(())
}
