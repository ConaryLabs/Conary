// src/commands/query/sbom.rs

//! Software Bill of Materials (SBOM) export
//!
//! Functions for generating SBOM in CycloneDX format.

use anyhow::Result;
use std::fs::File;
use std::io::Write;
use tracing::info;

/// CycloneDX 1.5 SBOM format structures
mod cyclonedx {
    use serde::Serialize;

    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct Bom {
        pub bom_format: String,
        pub spec_version: String,
        pub serial_number: String,
        pub version: u32,
        pub metadata: Metadata,
        pub components: Vec<Component>,
    }

    #[derive(Serialize)]
    pub struct Metadata {
        pub timestamp: String,
        pub tools: Vec<Tool>,
    }

    #[derive(Serialize)]
    pub struct Tool {
        pub vendor: String,
        pub name: String,
        pub version: String,
    }

    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct Component {
        #[serde(rename = "type")]
        pub component_type: String,
        pub name: String,
        pub version: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub description: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub purl: Option<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        pub hashes: Vec<Hash>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        pub external_references: Vec<ExternalReference>,
    }

    #[derive(Serialize)]
    pub struct Hash {
        pub alg: String,
        pub content: String,
    }

    #[derive(Serialize)]
    pub struct ExternalReference {
        #[serde(rename = "type")]
        pub ref_type: String,
        pub url: String,
    }
}

/// Generate SBOM for a package or all packages
pub fn cmd_sbom(
    package_name: &str,
    db_path: &str,
    format: &str,
    output: Option<&str>,
) -> Result<()> {
    info!("Generating SBOM for: {}", package_name);

    if format != "cyclonedx" {
        return Err(anyhow::anyhow!(
            "Unsupported format '{}'. Currently only 'cyclonedx' is supported.",
            format
        ));
    }

    let conn = conary::db::open(db_path)?;

    // Get packages to include
    let troves = if package_name == "all" {
        conary::db::models::Trove::list_all(&conn)?
    } else {
        let found = conary::db::models::Trove::find_by_name(&conn, package_name)?;
        if found.is_empty() {
            return Err(anyhow::anyhow!("Package '{}' not found", package_name));
        }
        found
    };

    // Build CycloneDX BOM
    let bom = build_cyclonedx_bom(&conn, &troves)?;

    // Serialize to JSON
    let json = serde_json::to_string_pretty(&bom)?;

    // Output
    if let Some(path) = output {
        let mut file = File::create(path)?;
        file.write_all(json.as_bytes())?;
        println!("SBOM written to: {}", path);
    } else {
        println!("{}", json);
    }

    Ok(())
}

/// Build a CycloneDX BOM from troves
fn build_cyclonedx_bom(
    conn: &rusqlite::Connection,
    troves: &[conary::db::models::Trove],
) -> Result<cyclonedx::Bom> {
    use chrono::Utc;
    use uuid::Uuid;

    let mut components = Vec::new();

    for trove in troves {
        let trove_id = match trove.id {
            Some(id) => id,
            None => continue,
        };

        // Build PURL (Package URL) - a standard way to identify packages
        let purl = build_purl(trove);

        // Get file hashes if available
        let hashes = get_package_hashes(conn, trove_id)?;

        // Build external references (e.g., upstream URL)
        let external_refs = Vec::new(); // Could be extended with repository URLs

        components.push(cyclonedx::Component {
            component_type: "library".to_string(),
            name: trove.name.clone(),
            version: trove.version.clone(),
            description: trove.description.clone(),
            purl: Some(purl),
            hashes,
            external_references: external_refs,
        });
    }

    Ok(cyclonedx::Bom {
        bom_format: "CycloneDX".to_string(),
        spec_version: "1.5".to_string(),
        serial_number: format!("urn:uuid:{}", Uuid::new_v4()),
        version: 1,
        metadata: cyclonedx::Metadata {
            timestamp: Utc::now().to_rfc3339(),
            tools: vec![cyclonedx::Tool {
                vendor: "ConaryLabs".to_string(),
                name: "conary".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            }],
        },
        components,
    })
}

/// Build a Package URL (PURL) for a trove
/// Format: pkg:conary/name@version?arch=x86_64
fn build_purl(trove: &conary::db::models::Trove) -> String {
    let mut purl = format!("pkg:conary/{}@{}", trove.name, trove.version);

    if let Some(ref arch) = trove.architecture {
        purl.push_str(&format!("?arch={}", arch));
    }

    purl
}

/// Get SHA256 hashes for package files (aggregate)
fn get_package_hashes(
    conn: &rusqlite::Connection,
    trove_id: i64,
) -> Result<Vec<cyclonedx::Hash>> {
    // Get unique file hashes from the package
    let files = conary::db::models::FileEntry::find_by_trove(conn, trove_id)?;

    // Include first non-empty sha256 as a representative hash
    for file in &files {
        if !file.sha256_hash.is_empty() {
            return Ok(vec![cyclonedx::Hash {
                alg: "SHA-256".to_string(),
                content: file.sha256_hash.clone(),
            }]);
        }
    }

    Ok(Vec::new())
}
