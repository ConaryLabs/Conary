// src/commands/derivation_sbom.rs
//! Derivation-aware SBOM generation (CycloneDX).

use anyhow::Result;
use conary_core::derivation::index::DerivationIndex;
use conary_core::derivation::profile::BuildProfile;

/// CycloneDX 1.5 SBOM types (mirrors query/sbom.rs structure).
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

    impl Bom {
        /// Number of components in this BOM (used for summary output).
        pub fn component_count(&self) -> usize {
            self.components.len()
        }
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
        #[serde(skip_serializing_if = "Vec::is_empty")]
        pub hashes: Vec<Hash>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        pub properties: Vec<Property>,
    }

    #[derive(Serialize)]
    pub struct Hash {
        pub alg: String,
        pub content: String,
    }

    #[derive(Serialize)]
    pub struct Property {
        pub name: String,
        pub value: String,
    }
}

/// Generate SBOM from a derivation profile or single derivation.
pub fn cmd_derivation_sbom(
    profile_path: Option<&str>,
    derivation_id: Option<&str>,
    output_path: Option<&str>,
) -> Result<()> {
    if profile_path.is_none() && derivation_id.is_none() {
        anyhow::bail!("specify --profile or --derivation");
    }

    let db_path = "/var/lib/conary/conary.db";
    let conn = super::open_db(db_path)?;
    let index = DerivationIndex::new(&conn);

    let mut components = Vec::new();

    if let Some(profile_path) = profile_path {
        let content = std::fs::read_to_string(profile_path)?;
        let profile: BuildProfile = toml::from_str(&content)?;

        for stage in &profile.stages {
            for drv in &stage.derivations {
                if drv.derivation_id == "pending" {
                    continue;
                }

                let component = match index.lookup(&drv.derivation_id) {
                    Ok(Some(record)) => build_component(&record),
                    Ok(None) => {
                        // Derivation not in local index -- minimal component
                        let id_prefix_len = 16.min(drv.derivation_id.len());
                        cyclonedx::Component {
                            component_type: "library".to_owned(),
                            name: drv.package.clone(),
                            version: drv.version.clone(),
                            description: Some(format!(
                                "derivation {} (not in local index)",
                                &drv.derivation_id[..id_prefix_len]
                            )),
                            hashes: vec![],
                            properties: vec![],
                        }
                    }
                    Err(_) => continue,
                };
                components.push(component);
            }
        }
    } else if let Some(drv_id) = derivation_id {
        let record = index
            .lookup(drv_id)?
            .ok_or_else(|| anyhow::anyhow!("derivation {drv_id} not found"))?;
        components.push(build_component(&record));
    }

    let bom = cyclonedx::Bom {
        bom_format: "CycloneDX".to_owned(),
        spec_version: "1.5".to_owned(),
        serial_number: format!("urn:uuid:{}", uuid::Uuid::new_v4()),
        version: 1,
        metadata: cyclonedx::Metadata {
            timestamp: chrono::Utc::now().to_rfc3339(),
            tools: vec![cyclonedx::Tool {
                vendor: "Conary Project".to_owned(),
                name: "conary".to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
            }],
        },
        components,
    };

    let json = serde_json::to_string_pretty(&bom)?;

    if let Some(path) = output_path {
        std::fs::write(path, &json)?;
        println!("SBOM written to {path} ({} components)", bom.component_count());
    } else {
        println!("{json}");
    }

    Ok(())
}

/// Build a CycloneDX component from a `DerivationRecord`.
fn build_component(
    record: &conary_core::derivation::index::DerivationRecord,
) -> cyclonedx::Component {
    let trust_name = match record.trust_level {
        0 => "unverified",
        1 => "substituted",
        2 => "locally-built",
        3 => "independently-verified",
        4 => "diverse-verified",
        _ => "unknown",
    };

    let mut properties = vec![
        cyclonedx::Property {
            name: "conary:derivation_id".to_owned(),
            value: record.derivation_id.clone(),
        },
        cyclonedx::Property {
            name: "conary:trust_level".to_owned(),
            value: format!("{} ({})", record.trust_level, trust_name),
        },
    ];

    if let Some(ref stage) = record.stage {
        properties.push(cyclonedx::Property {
            name: "conary:stage".to_owned(),
            value: stage.clone(),
        });
    }

    if let Some(ref prov_hash) = record.provenance_cas_hash {
        properties.push(cyclonedx::Property {
            name: "conary:provenance_hash".to_owned(),
            value: prov_hash.clone(),
        });
    }

    if let Some(reproducible) = record.reproducible {
        properties.push(cyclonedx::Property {
            name: "conary:reproducible".to_owned(),
            value: reproducible.to_string(),
        });
    }

    cyclonedx::Component {
        component_type: "library".to_owned(),
        name: record.package_name.clone(),
        version: record.package_version.clone(),
        description: None,
        hashes: vec![cyclonedx::Hash {
            alg: "SHA-256".to_owned(),
            content: record.output_hash.clone(),
        }],
        properties,
    }
}
