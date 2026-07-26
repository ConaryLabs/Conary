// apps/conary/src/commands/provenance/sbom.rs

//! SPDX and CycloneDX projections of exact installed provenance.

use super::ProvenanceData;
use anyhow::Result;
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension};
use uuid::Uuid;

fn purl(name: &str, version: &str) -> String {
    format!("pkg:conary/{name}@{version}")
}

pub(super) fn generate_spdx(
    name: &str,
    version: &str,
    prov: &ProvenanceData,
    deps: &[(String, String, Option<String>)],
) -> Result<String> {
    let timestamp = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let doc_id = format!("SPDXRef-DOCUMENT-{}-{}", name, version.replace('.', "-"));
    let pkg_id = format!("SPDXRef-Package-{name}");

    let mut packages = vec![serde_json::json!({
        "SPDXID": pkg_id,
        "name": name,
        "versionInfo": version,
        "downloadLocation": prov.upstream_url.as_deref().unwrap_or("NOASSERTION"),
        "filesAnalyzed": false,
        "checksums": prov.upstream_hash.as_ref().map(|h| vec![{
            let parts: Vec<&str> = h.splitn(2, ':').collect();
            serde_json::json!({
                "algorithm": parts.first().unwrap_or(&"SHA256").to_uppercase(),
                "checksumValue": parts.get(1).unwrap_or(&h.as_str())
            })
        }]).unwrap_or_default(),
        "externalRefs": prov.dna_hash.as_ref().map(|dna| vec![serde_json::json!({
            "referenceCategory": "PACKAGE-MANAGER",
            "referenceType": "purl",
            "referenceLocator": format!("{}?dna={}", purl(name, version), dna)
        })]).unwrap_or_default(),
        "supplier": "NOASSERTION",
        "copyrightText": "NOASSERTION"
    })];

    let mut relationships = vec![serde_json::json!({
        "spdxElementId": doc_id,
        "relatedSpdxElement": pkg_id,
        "relationshipType": "DESCRIBES"
    })];

    for (dep_name, dep_version, dep_dna) in deps {
        let dep_id = format!("SPDXRef-Package-{dep_name}");
        packages.push(serde_json::json!({
            "SPDXID": dep_id,
            "name": dep_name,
            "versionInfo": dep_version,
            "downloadLocation": "NOASSERTION",
            "filesAnalyzed": false,
            "externalRefs": dep_dna.as_ref().map(|dna| vec![serde_json::json!({
                "referenceCategory": "PACKAGE-MANAGER",
                "referenceType": "purl",
                "referenceLocator": format!("{}?dna={}", purl(dep_name, dep_version), dna)
            })]).unwrap_or_default(),
            "supplier": "NOASSERTION",
            "copyrightText": "NOASSERTION"
        }));
        relationships.push(serde_json::json!({
            "spdxElementId": pkg_id,
            "relatedSpdxElement": dep_id,
            "relationshipType": "DEPENDS_ON"
        }));
    }

    let sbom = serde_json::json!({
        "spdxVersion": "SPDX-2.3",
        "dataLicense": "CC0-1.0",
        "SPDXID": doc_id,
        "name": format!("{name}-{version}"),
        "documentNamespace": format!("https://conary.dev/spdx/{name}/{version}"),
        "creationInfo": {
            "created": timestamp,
            "creators": ["Tool: conary-provenance"],
            "licenseListVersion": "3.19"
        },
        "packages": packages,
        "relationships": relationships
    });

    Ok(serde_json::to_string_pretty(&sbom)?)
}

pub(super) fn generate_cyclonedx(
    name: &str,
    version: &str,
    prov: &ProvenanceData,
    deps: &[(String, String, Option<String>)],
) -> Result<String> {
    let timestamp = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let serial = Uuid::new_v4().to_string();

    let mut components = vec![serde_json::json!({
        "type": "library",
        "bom-ref": purl(name, version),
        "name": name,
        "version": version,
        "purl": purl(name, version),
        "hashes": prov.upstream_hash.as_ref().map(|h| {
            let parts: Vec<&str> = h.splitn(2, ':').collect();
            vec![serde_json::json!({
                "alg": parts.first().unwrap_or(&"SHA-256").to_uppercase().replace("SHA", "SHA-"),
                "content": parts.get(1).unwrap_or(&h.as_str())
            })]
        }).unwrap_or_default(),
        "externalReferences": prov.upstream_url.as_ref().map(|url| vec![serde_json::json!({
            "type": "distribution",
            "url": url
        })]).unwrap_or_default()
    })];
    let mut dependencies = vec![serde_json::json!({
        "ref": purl(name, version),
        "dependsOn": deps.iter().map(|(n, v, _)| purl(n, v)).collect::<Vec<_>>()
    })];

    for (dep_name, dep_version, _) in deps {
        components.push(serde_json::json!({
            "type": "library",
            "bom-ref": purl(dep_name, dep_version),
            "name": dep_name,
            "version": dep_version,
            "purl": purl(dep_name, dep_version)
        }));
        dependencies.push(serde_json::json!({
            "ref": purl(dep_name, dep_version),
            "dependsOn": []
        }));
    }

    let sbom = serde_json::json!({
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "serialNumber": format!("urn:uuid:{serial}"),
        "version": 1,
        "metadata": {
            "timestamp": timestamp,
            "tools": [{
                "vendor": "Conary",
                "name": "conary-provenance",
                "version": "0.1.0"
            }],
            "component": {
                "type": "application",
                "name": name,
                "version": version,
                "purl": purl(name, version)
            }
        },
        "components": components,
        "dependencies": dependencies
    });

    Ok(serde_json::to_string_pretty(&sbom)?)
}

pub(super) fn collect_dependencies(
    conn: &Connection,
    trove_id: i64,
) -> Result<Vec<(String, String, Option<String>)>> {
    let mut deps = Vec::new();
    for requirement in
        conary_core::db::models::InstalledRequirementAtom::find_by_trove(conn, trove_id)?
    {
        for dependency in
            conary_core::db::models::Trove::find_by_name(conn, &requirement.depends_on_name)?
        {
            let dna_hash = if let Some(dependency_id) = dependency.id {
                conn.query_row(
                    "SELECT dna_hash FROM provenance WHERE trove_id = ?1",
                    [dependency_id],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()?
                .flatten()
            } else {
                None
            };
            deps.push((dependency.name, dependency.version, dna_hash));
        }
    }
    deps.sort();
    deps.dedup();
    Ok(deps)
}
