// src/commands/query/components.rs

//! Component query commands
//!
//! Functions for querying package components and their files.

use super::super::open_db;
use anyhow::Result;

/// List components of an installed package
pub async fn cmd_list_components(package_name: &str, db_path: &str) -> Result<()> {
    let conn = open_db(db_path)?;

    // Find the package
    let troves = conary_core::db::models::Trove::find_by_name(&conn, package_name)?;
    if troves.is_empty() {
        return Err(anyhow::anyhow!(
            "Package '{}' is not installed",
            package_name
        ));
    }

    for trove in &troves {
        let trove_id = trove.id.ok_or_else(|| anyhow::anyhow!("Trove has no ID"))?;

        println!("Package: {} {}", trove.name, trove.version);
        if let Some(arch) = &trove.architecture {
            println!("  Architecture: {}", arch);
        }

        // Get components
        let components = conary_core::db::models::Component::find_by_trove(&conn, trove_id)?;
        let payload = conary_core::db::models::PackagePayloadOwnership::load(&conn, trove_id)?;

        if components.is_empty() {
            if payload.entries().is_empty() {
                println!("  Components: (none; metadata-only package)");
            } else {
                anyhow::bail!(
                    "Installed package '{}' {} has {} files but no persisted component authority",
                    trove.name,
                    trove.version,
                    payload.entries().len()
                );
            }
        } else {
            println!("  Components:");
            for comp in &components {
                let file_count = payload
                    .entries()
                    .iter()
                    .filter(|file| file.component_id == comp.id)
                    .count();
                let default_marker = if conary_core::components::ComponentType::parse(&comp.name)
                    .map(|ct| ct.is_default())
                    .unwrap_or(false)
                {
                    " (default)"
                } else {
                    ""
                };
                let installed_marker = if comp.is_installed {
                    ""
                } else {
                    " [not installed]"
                };
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
pub async fn cmd_query_component(component_spec: &str, db_path: &str) -> Result<()> {
    let conn = open_db(db_path)?;

    // Parse the component spec (e.g., "nginx:lib")
    let (package_name, component_name) =
        conary_core::components::parse_component_spec(component_spec).ok_or_else(|| {
            anyhow::anyhow!(
                "Invalid component spec '{}'. Expected format: package:component (e.g., nginx:lib)",
                component_spec
            )
        })?;

    // Find the package
    let troves = conary_core::db::models::Trove::find_by_name(&conn, &package_name)?;
    if troves.is_empty() {
        return Err(anyhow::anyhow!(
            "Package '{}' is not installed",
            package_name
        ));
    }

    for trove in &troves {
        let trove_id = trove.id.ok_or_else(|| anyhow::anyhow!("Trove has no ID"))?;

        // Find the component
        let component = conary_core::db::models::Component::find_by_trove_and_name(
            &conn,
            trove_id,
            &component_name,
        )?;

        match component {
            Some(comp) => {
                let comp_id = comp
                    .id
                    .ok_or_else(|| anyhow::anyhow!("Component has no ID"))?;
                let payload =
                    conary_core::db::models::PackagePayloadOwnership::load(&conn, trove_id)?;
                let files = payload
                    .entries()
                    .iter()
                    .filter(|file| file.component_id == Some(comp_id))
                    .collect::<Vec<_>>();

                println!(
                    "{}:{} ({} {})",
                    package_name, component_name, trove.name, trove.version
                );
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
                let components =
                    conary_core::db::models::Component::find_by_trove(&conn, trove_id)?;
                if components.is_empty() {
                    let payload =
                        conary_core::db::models::PackagePayloadOwnership::load(&conn, trove_id)?;
                    if !payload.entries().is_empty() {
                        anyhow::bail!(
                            "Installed package '{}' {} has {} files but no persisted component authority",
                            trove.name,
                            trove.version,
                            payload.entries().len()
                        );
                    }
                    return Err(anyhow::anyhow!(
                        "Component '{}' not found in metadata-only package '{}'; the package declares no components",
                        component_name,
                        package_name
                    ));
                } else {
                    let available: Vec<String> =
                        components.iter().map(|c| format!(":{}", c.name)).collect();
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

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db::models::{FileEntry, Trove, TroveType};
    use conary_core::payload::{
        PayloadIdentity, PayloadNode, PayloadNodeKind, PayloadTimestamp, ResolvedPayloadNode,
    };

    fn test_database() -> (tempfile::NamedTempFile, String) {
        let file = tempfile::NamedTempFile::new().unwrap();
        let path = file.path().to_string_lossy().into_owned();
        conary_core::db::init(&path).unwrap();
        (file, path)
    }

    #[tokio::test]
    async fn payload_without_component_authority_is_an_invariant_error() {
        let (_file, db_path) = test_database();
        let conn = conary_core::db::open(&db_path).unwrap();
        let mut trove = Trove::new(
            "broken-components".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            conary_core::repository::versioning::VersionScheme::Conary,
        );
        let trove_id = trove.insert(&conn).unwrap();
        let directory = PayloadNode {
            kind: PayloadNodeKind::Directory,
            mode: libc::S_IFDIR | 0o755,
            user: PayloadIdentity::Numeric { id: 0 },
            group: PayloadIdentity::Numeric { id: 0 },
            mtime: PayloadTimestamp::UNIX_EPOCH,
            xattrs: Default::default(),
        };
        FileEntry::new(
            "/opt/broken-components".to_string(),
            ResolvedPayloadNode::from_numeric_source(directory).unwrap(),
            None,
            trove_id,
        )
        .insert(&conn)
        .unwrap();

        let error = cmd_list_components("broken-components", &db_path)
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("files but no persisted component authority"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn metadata_only_package_has_no_implicit_runtime_component() {
        let (_file, db_path) = test_database();
        let conn = conary_core::db::open(&db_path).unwrap();
        Trove::new(
            "metadata-only".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            conary_core::repository::versioning::VersionScheme::Conary,
        )
        .insert(&conn)
        .unwrap();

        let error = cmd_query_component("metadata-only:runtime", &db_path)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("package declares no components"),
            "unexpected error: {error}"
        );
    }
}
