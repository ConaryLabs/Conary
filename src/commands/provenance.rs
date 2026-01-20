// src/commands/provenance.rs

//! Command implementations for Package DNA / Provenance queries

use anyhow::Result;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STD_ENGINE};
use chrono::Utc;
use const_oid::db::rfc5280::ID_KP_CODE_SIGNING;
use rusqlite::{Connection, params};
use x509_cert::der::Decode;
use sigstore::crypto::{CosignVerificationKey, Signature, SigningScheme};
use sigstore::crypto::signing_key::SigStoreKeyPair;
use sigstore::fulcio::{FulcioClient, FULCIO_ROOT, TokenProvider};
use sigstore::fulcio::oauth::OauthTokenProvider;
use sigstore::rekor::apis::entries_api;
use sigstore::rekor::models::{
    hashedrekord, log_entry::Body as RekorBody, LogEntry as RekorLogEntry, ProposedEntry,
};
use sigstore::trust::sigstore::SigstoreTrustRoot;
use sigstore::trust::TrustRoot;
use rustls_pki_types::{CertificateDer, TrustAnchor, UnixTime};
use std::str::FromStr;
use std::time::Duration;
use thiserror::Error;
use uuid::Uuid;
use webpki::{EndEntityCert, KeyUsage};

use conary::provenance::{build_slsa_statement, SlsaContext};

#[derive(Debug, Error)]
enum SigstoreCommandError {
    #[error("provenance is missing DNA hash")]
    MissingDnaHash,
    #[error("Rekor entry missing hashedrekord data")]
    MissingHashedRekord,
    #[error("Rekor entry missing signature data")]
    MissingSignature,
    #[error("Rekor entry missing public key data")]
    MissingPublicKey,
    #[error("signing key is required unless --keyless is provided")]
    MissingSigningKey,
    #[error("Rekor entry contains unsupported public key")]
    UnsupportedPublicKey,
    #[error("Fulcio certificate chain verification failed: {0}")]
    FulcioChain(String),
    #[error("Base64 decode failed: {0}")]
    Base64(#[from] base64::DecodeError),
    #[error("Hex decode failed: {0}")]
    Hex(#[from] hex::FromHexError),
    #[error("PEM parse failed: {0}")]
    Pem(#[from] pem::PemError),
    #[error("Sigstore error: {0}")]
    Sigstore(#[from] sigstore::errors::SigstoreError),
    #[error("Rekor API error: {0}")]
    RekorApi(#[from] reqwest::Error),
    #[error("Rekor response parse error: {0}")]
    RekorParse(#[from] serde_json::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("URL parse error: {0}")]
    Url(#[from] url::ParseError),
}

#[derive(Debug)]
struct RekorVerification {
    hash_match: bool,
    signature_valid: bool,
    cert_chain_valid: bool,
    entry_uuid: String,
    entry_index: i64,
    signer_kind: String,
}

/// Show provenance information for a package
pub fn cmd_provenance_show(
    db_path: &str,
    package: &str,
    section: &str,
    recursive: bool,
    format: &str,
) -> Result<()> {
    let conn = Connection::open(db_path)?;

    // Parse package@version format
    let (name, version) = parse_package_spec(package);

    // Look up the package
    let trove_info = find_trove(&conn, &name, version.as_deref())?;

    match trove_info {
        Some((trove_id, trove_name, trove_version)) => {
            // Query provenance from database
            let prov = query_provenance(&conn, trove_id)?;

            match format {
                "json" => print_provenance_json(&prov, section, recursive)?,
                "tree" => print_provenance_tree(&prov, section, recursive)?,
                _ => print_provenance_text(&trove_name, &trove_version, &prov, section, recursive)?,
            }
        }
        None => {
            println!("Package '{}' not found", package);
            std::process::exit(1);
        }
    }

    Ok(())
}

/// Verify provenance against transparency log
pub fn cmd_provenance_verify(
    db_path: &str,
    package: &str,
    all_signatures: bool,
) -> Result<()> {
    let conn = Connection::open(db_path)?;
    let (name, version) = parse_package_spec(package);

    let trove_info = find_trove(&conn, &name, version.as_deref())?;

    match trove_info {
        Some((trove_id, trove_name, trove_version)) => {
            println!("Verifying provenance for {} v{}...", trove_name, trove_version);
            println!();

            let prov = query_provenance(&conn, trove_id)?;

            // Check Rekor log entry
            if let Some(rekor_index) = prov.rekor_log_index {
                println!("[CHECKING] Rekor transparency log entry #{}...", rekor_index);
                let dna_hash = prov
                    .dna_hash
                    .as_deref()
                    .ok_or(SigstoreCommandError::MissingDnaHash)?;
                let entry = rekor_get_entry_by_index(rekor_index)?;
                let report = verify_rekor_entry(&entry, dna_hash)?;

                if report.hash_match {
                    println!("[OK] DNA hash matches Rekor entry");
                } else {
                    println!("[FAIL] DNA hash does not match Rekor entry");
                }

                if report.signature_valid {
                    println!("[OK] Rekor signature verified");
                } else {
                    println!("[FAIL] Rekor signature verification failed");
                }

                if report.cert_chain_valid {
                    println!("[OK] Fulcio certificate chain verified");
                } else if report.signer_kind == "key" {
                    println!("[WARN] Fulcio certificate chain not present (key-based signature)");
                } else {
                    println!("[FAIL] Fulcio certificate chain verification failed");
                }

                println!(
                    "[INFO] Rekor entry: uuid={}, log_index={}",
                    report.entry_uuid, report.entry_index
                );
                println!();
            } else {
                println!("[WARN] No Rekor transparency log entry found");
                println!("       Run 'conary provenance register {}' to register", package);
                println!();
            }

            // Check signatures
            if let Some(ref sigs) = prov.signatures_json {
                println!("[CHECKING] Signatures...");
                let count = serde_json::from_str::<serde_json::Value>(sigs)
                    .ok()
                    .map(|value| {
                        let builder = value
                            .get("builder_sig")
                            .and_then(|v| if v.is_null() { None } else { Some(1) })
                            .unwrap_or(0);
                        let reviewers = value
                            .get("reviewer_sigs")
                            .and_then(|v| v.as_array())
                            .map(|arr| arr.len())
                            .unwrap_or(0);
                        builder + reviewers
                    })
                    .unwrap_or(0);
                if all_signatures {
                    println!("[INFO] Recorded {} signature(s) in provenance", count);
                } else {
                    println!("[INFO] Recorded builder signature in provenance");
                }
            } else {
                println!("[WARN] No signatures found in provenance");
            }
        }
        None => {
            println!("Package '{}' not found", package);
            std::process::exit(1);
        }
    }

    Ok(())
}

/// Compare provenance between two package versions
pub fn cmd_provenance_diff(
    db_path: &str,
    package1: &str,
    package2: &str,
    format: &str,
) -> Result<()> {
    let conn = Connection::open(db_path)?;

    let (name1, version1) = parse_package_spec(package1);
    let (name2, version2) = parse_package_spec(package2);

    let trove1 = find_trove(&conn, &name1, version1.as_deref())?;
    let trove2 = find_trove(&conn, &name2, version2.as_deref())?;

    match (trove1, trove2) {
        (Some((id1, n1, v1)), Some((id2, n2, v2))) => {
            let prov1 = query_provenance(&conn, id1)?;
            let prov2 = query_provenance(&conn, id2)?;

            println!("=== Provenance Diff ===");
            println!("  {} v{}", n1, v1);
            println!("  {} v{}", n2, v2);
            println!();

            match format {
                "json" => {
                    println!("[NOT IMPLEMENTED] JSON diff format");
                }
                _ => {
                    // Compare source
                    if prov1.upstream_hash != prov2.upstream_hash {
                        println!("[SOURCE] Upstream hash changed");
                        println!("  - {}", prov1.upstream_hash.as_deref().unwrap_or("none"));
                        println!("  + {}", prov2.upstream_hash.as_deref().unwrap_or("none"));
                    }

                    // Compare recipe
                    if prov1.recipe_hash != prov2.recipe_hash {
                        println!("[BUILD] Recipe changed");
                        println!("  - {}", prov1.recipe_hash.as_deref().unwrap_or("none"));
                        println!("  + {}", prov2.recipe_hash.as_deref().unwrap_or("none"));
                    }

                    // Compare merkle root
                    if prov1.merkle_root != prov2.merkle_root {
                        println!("[CONTENT] Package content changed");
                        println!("  - {}", prov1.merkle_root.as_deref().unwrap_or("none"));
                        println!("  + {}", prov2.merkle_root.as_deref().unwrap_or("none"));
                    }

                    // Compare DNA
                    if prov1.dna_hash != prov2.dna_hash {
                        println!("[DNA] Provenance chain changed");
                        println!("  - {}", prov1.dna_hash.as_deref().unwrap_or("none"));
                        println!("  + {}", prov2.dna_hash.as_deref().unwrap_or("none"));
                    }
                }
            }
        }
        (None, _) => {
            println!("Package '{}' not found", package1);
            std::process::exit(1);
        }
        (_, None) => {
            println!("Package '{}' not found", package2);
            std::process::exit(1);
        }
    }

    Ok(())
}

/// Find packages built with a specific dependency
pub fn cmd_provenance_find_by_dep(
    db_path: &str,
    dep_name: &str,
    version: Option<&str>,
    dna: Option<&str>,
) -> Result<()> {
    let conn = Connection::open(db_path)?;

    println!("=== Packages Built With {} ===", dep_name);
    if let Some(v) = version {
        println!("Version constraint: {}", v);
    }
    if let Some(d) = dna {
        println!("DNA hash: {}", d);
    }
    println!();

    // Query packages with matching build dependency
    let mut stmt = conn.prepare(
        "SELECT t.name, t.version, p.build_deps_json
         FROM troves t
         JOIN provenance p ON t.id = p.trove_id
         WHERE p.build_deps_json LIKE ?1",
    )?;

    let pattern = format!("%\"name\":\"{}%", dep_name);
    let rows = stmt.query_map([&pattern], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    })?;

    let mut count = 0;
    for row in rows {
        let (name, ver, _deps) = row?;
        println!("  {} v{}", name, ver);
        count += 1;
    }

    if count == 0 {
        println!("  (no packages found)");
    } else {
        println!();
        println!("Found {} package(s)", count);
    }

    Ok(())
}

/// Export provenance as SBOM
pub fn cmd_provenance_export(
    db_path: &str,
    package: &str,
    format: &str,
    output: Option<&str>,
    recursive: bool,
) -> Result<()> {
    let conn = Connection::open(db_path)?;
    let (name, version) = parse_package_spec(package);

    let trove_info = find_trove(&conn, &name, version.as_deref())?;

    match trove_info {
        Some((trove_id, trove_name, trove_version)) => {
            let prov = query_provenance(&conn, trove_id)?;

            // Collect dependencies if recursive
            let deps = if recursive {
                collect_dependencies(&conn, trove_id)?
            } else {
                Vec::new()
            };

            let sbom = match format {
                "slsa" => {
                    let statement = build_slsa_statement(SlsaContext {
                        name: &trove_name,
                        version: &trove_version,
                        dna_hash: prov.dna_hash.as_deref(),
                        upstream_url: prov.upstream_url.as_deref(),
                        upstream_hash: prov.upstream_hash.as_deref(),
                        git_commit: prov.git_commit.as_deref(),
                        recipe_hash: prov.recipe_hash.as_deref(),
                        build_deps_json: prov.build_deps_json.as_deref(),
                        host_arch: prov.host_arch.as_deref(),
                        host_kernel: prov.host_kernel.as_deref(),
                        dependencies: &deps,
                    })?;
                    statement
                }
                "cyclonedx" => generate_cyclonedx_sbom(&trove_name, &trove_version, &prov, &deps)?,
                _ => generate_spdx_sbom(&trove_name, &trove_version, &prov, &deps)?,
            };

            // Output to file or stdout
            match output {
                Some(path) => {
                    std::fs::write(path, &sbom)?;
                    println!("Provenance export written to: {}", path);
                }
                None => {
                    println!("{}", sbom);
                }
            }
        }
        None => {
            println!("Package '{}' not found", package);
            std::process::exit(1);
        }
    }

    Ok(())
}

/// Generate SPDX 2.3 SBOM in JSON format
fn generate_spdx_sbom(
    name: &str,
    version: &str,
    prov: &ProvenanceData,
    deps: &[(String, String, Option<String>)],
) -> Result<String> {
    let timestamp = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let doc_id = format!("SPDXRef-DOCUMENT-{}-{}", name, version.replace('.', "-"));
    let pkg_id = format!("SPDXRef-Package-{}", name);

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
            "referenceLocator": format!("pkg:conary/{}@{}?dna={}", name, version, dna)
        })]).unwrap_or_default(),
        "supplier": "NOASSERTION",
        "copyrightText": "NOASSERTION"
    })];

    let mut relationships = vec![serde_json::json!({
        "spdxElementId": doc_id,
        "relatedSpdxElement": pkg_id,
        "relationshipType": "DESCRIBES"
    })];

    // Add dependencies
    for (dep_name, dep_version, dep_dna) in deps {
        let dep_id = format!("SPDXRef-Package-{}", dep_name);
        packages.push(serde_json::json!({
            "SPDXID": dep_id,
            "name": dep_name,
            "versionInfo": dep_version,
            "downloadLocation": "NOASSERTION",
            "filesAnalyzed": false,
            "externalRefs": dep_dna.as_ref().map(|dna| vec![serde_json::json!({
                "referenceCategory": "PACKAGE-MANAGER",
                "referenceType": "purl",
                "referenceLocator": format!("pkg:conary/{}@{}?dna={}", dep_name, dep_version, dna)
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
        "name": format!("{}-{}", name, version),
        "documentNamespace": format!("https://conary.dev/spdx/{}/{}", name, version),
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

/// Generate CycloneDX 1.5 SBOM in JSON format
fn generate_cyclonedx_sbom(
    name: &str,
    version: &str,
    prov: &ProvenanceData,
    deps: &[(String, String, Option<String>)],
) -> Result<String> {
    let timestamp = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let serial = Uuid::new_v4().to_string();

    let mut components = vec![serde_json::json!({
        "type": "library",
        "bom-ref": format!("pkg:conary/{}@{}", name, version),
        "name": name,
        "version": version,
        "purl": format!("pkg:conary/{}@{}", name, version),
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
        "ref": format!("pkg:conary/{}@{}", name, version),
        "dependsOn": deps.iter().map(|(n, v, _)| format!("pkg:conary/{}@{}", n, v)).collect::<Vec<_>>()
    })];

    // Add dependency components
    for (dep_name, dep_version, _dep_dna) in deps {
        components.push(serde_json::json!({
            "type": "library",
            "bom-ref": format!("pkg:conary/{}@{}", dep_name, dep_version),
            "name": dep_name,
            "version": dep_version,
            "purl": format!("pkg:conary/{}@{}", dep_name, dep_version)
        }));

        dependencies.push(serde_json::json!({
            "ref": format!("pkg:conary/{}@{}", dep_name, dep_version),
            "dependsOn": []
        }));
    }

    let sbom = serde_json::json!({
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "serialNumber": format!("urn:uuid:{}", serial),
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
                "purl": format!("pkg:conary/{}@{}", name, version)
            }
        },
        "components": components,
        "dependencies": dependencies
    });

    Ok(serde_json::to_string_pretty(&sbom)?)
}

/// Collect dependencies for a package
fn collect_dependencies(conn: &Connection, trove_id: i64) -> Result<Vec<(String, String, Option<String>)>> {
    let mut deps = Vec::new();

    let mut stmt = conn.prepare(
        "SELECT t.name, t.version, p.dna_hash
         FROM dependencies d
         JOIN troves t ON d.dependency_name = t.name
         LEFT JOIN provenance p ON t.id = p.trove_id
         WHERE d.trove_id = ?1"
    )?;

    let rows = stmt.query_map([trove_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    })?;

    for row in rows {
        deps.push(row?);
    }

    Ok(deps)
}

/// Register provenance in transparency log
pub fn cmd_provenance_register(
    db_path: &str,
    package: &str,
    key: Option<&str>,
    keyless: bool,
    dry_run: bool,
) -> Result<()> {
    let conn = Connection::open(db_path)?;
    let (name, version) = parse_package_spec(package);

    let trove_info = find_trove(&conn, &name, version.as_deref())?;

    match trove_info {
        Some((trove_id, trove_name, trove_version)) => {
            println!("=== Register Provenance in Transparency Log ===");
            println!();
            println!("Package: {} v{}", trove_name, trove_version);
            if keyless {
                println!("Signing key: keyless (Fulcio)");
            } else if let Some(key) = key {
                println!("Signing key: {}", key);
            }
            println!("Target: rekor.sigstore.dev");
            println!();

            if dry_run {
                println!("[DRY RUN] Would register provenance entry");
                println!();
                println!("Entry would include:");
                println!("  - DNA hash");
                println!("  - Source provenance");
                println!("  - Build provenance");
                println!("  - Signature");
            } else {
                let prov = query_provenance(&conn, trove_id)?;
                let dna_hash = prov
                    .dna_hash
                    .as_deref()
                    .ok_or(SigstoreCommandError::MissingDnaHash)?;

                let signed = if keyless {
                    sign_dna_keyless(dna_hash)?
                } else {
                    let key = key.ok_or(SigstoreCommandError::MissingSigningKey)?;
                    sign_dna_with_key(dna_hash, key)?
                };

                let entry = rekor_create_entry(build_rekor_entry(dna_hash, &signed)?)?;
                println!(
                    "[OK] Rekor entry created: uuid={}, log_index={}",
                    entry.uuid, entry.log_index
                );

                conn.execute(
                    "UPDATE provenance SET rekor_log_index = ?1 WHERE trove_id = ?2",
                    params![entry.log_index, trove_id],
                )?;
            }
        }
        None => {
            println!("Package '{}' not found", package);
            std::process::exit(1);
        }
    }

    Ok(())
}

/// Audit packages for missing provenance
pub fn cmd_provenance_audit(
    db_path: &str,
    missing: Option<&str>,
    include_converted: bool,
) -> Result<()> {
    let conn = Connection::open(db_path)?;

    println!("=== Provenance Audit ===");
    println!();

    if let Some(section) = missing {
        println!("Checking for missing: {}", section);
    }
    if include_converted {
        println!("Including converted packages");
    }
    println!();

    // Query packages with incomplete provenance
    let mut stmt = conn.prepare(
        "SELECT t.name, t.version,
                p.dna_hash IS NOT NULL as has_dna,
                p.upstream_hash IS NOT NULL as has_source,
                p.recipe_hash IS NOT NULL as has_build,
                p.signatures_json IS NOT NULL as has_sigs
         FROM troves t
         LEFT JOIN provenance p ON t.id = p.trove_id
         WHERE t.type = 'package'
         ORDER BY t.name",
    )?;

    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, bool>(2).unwrap_or(false),
            row.get::<_, bool>(3).unwrap_or(false),
            row.get::<_, bool>(4).unwrap_or(false),
            row.get::<_, bool>(5).unwrap_or(false),
        ))
    })?;

    let mut incomplete = 0;
    let mut complete = 0;

    for row in rows {
        let (name, version, has_dna, has_source, has_build, has_sigs) = row?;

        let show = match missing {
            Some("source") => !has_source,
            Some("build") => !has_build,
            Some("signatures") => !has_sigs,
            _ => !has_dna || !has_source || !has_build || !has_sigs,
        };

        if show {
            let mut missing_parts = Vec::new();
            if !has_source { missing_parts.push("source"); }
            if !has_build { missing_parts.push("build"); }
            if !has_sigs { missing_parts.push("signatures"); }
            if !has_dna { missing_parts.push("DNA"); }

            if !missing_parts.is_empty() {
                println!("  {} v{}", name, version);
                println!("    Missing: {}", missing_parts.join(", "));
                incomplete += 1;
            }
        } else {
            complete += 1;
        }
    }

    println!();
    println!("Summary: {} complete, {} incomplete", complete, incomplete);

    Ok(())
}

// === Sigstore helpers ===

#[derive(Debug)]
struct SignedDna {
    signature: Vec<u8>,
    public_key_pem: String,
    signer_kind: String,
}

fn rekor_base_url() -> &'static str {
    "https://rekor.sigstore.dev"
}

fn rekor_get_entry_by_index(index: i64) -> Result<RekorLogEntry, SigstoreCommandError> {
    let client = reqwest::blocking::Client::new();
    let url = format!("{}/api/v1/log/entries?logIndex={}", rekor_base_url(), index);
    let response = client.get(url).send()?.error_for_status()?;
    let body = response.text()?;
    let parsed = entries_api::parse_response(body);
    Ok(RekorLogEntry::from_str(&parsed)?)
}

fn rekor_create_entry(entry: ProposedEntry) -> Result<RekorLogEntry, SigstoreCommandError> {
    let client = reqwest::blocking::Client::new();
    let url = format!("{}/api/v1/log/entries", rekor_base_url());
    let response = client.post(url).json(&entry).send()?.error_for_status()?;
    let body = response.text()?;
    let parsed = entries_api::parse_response(body);
    Ok(RekorLogEntry::from_str(&parsed)?)
}

fn extract_hashedrekord_spec(entry: &RekorLogEntry) -> Result<hashedrekord::Spec, SigstoreCommandError> {
    match &entry.body {
        RekorBody::hashedrekord(payload) => Ok(serde_json::from_value(payload.spec.clone())?),
        _ => Err(SigstoreCommandError::MissingHashedRekord),
    }
}

fn dna_hash_bytes(dna_hash: &str) -> Result<Vec<u8>, SigstoreCommandError> {
    let stripped = dna_hash.strip_prefix("sha256:").unwrap_or(dna_hash);
    Ok(hex::decode(stripped)?)
}

fn sign_dna_with_key(dna_hash: &str, key_path: &str) -> Result<SignedDna, SigstoreCommandError> {
    let key_bytes = std::fs::read(key_path)?;
    let keypair = SigStoreKeyPair::from_pem(&key_bytes).or_else(|_| {
        let password = std::env::var("CONARY_SIGNING_KEY_PASSWORD").unwrap_or_default();
        if password.is_empty() {
            Err(sigstore::errors::SigstoreError::KeyParseError(
                "encrypted key detected but CONARY_SIGNING_KEY_PASSWORD is unset".to_string(),
            ))
        } else {
            SigStoreKeyPair::from_encrypted_pem(&key_bytes, password.as_bytes())
        }
    })?;

    let signing_scheme = match &keypair {
        SigStoreKeyPair::ECDSA(keys) => match keys {
            sigstore::crypto::signing_key::ecdsa::ECDSAKeys::P256(_) => {
                SigningScheme::ECDSA_P256_SHA256_ASN1
            }
            sigstore::crypto::signing_key::ecdsa::ECDSAKeys::P384(_) => {
                SigningScheme::ECDSA_P384_SHA384_ASN1
            }
        },
        SigStoreKeyPair::ED25519(_) => SigningScheme::ED25519,
        SigStoreKeyPair::RSA(_) => SigningScheme::RSA_PSS_SHA256(0),
    };

    let signer = keypair.to_sigstore_signer(&signing_scheme)?;
    let signature = signer.sign(&dna_hash_bytes(dna_hash)?)?;
    let public_key_pem = keypair.public_key_to_pem()?;

    Ok(SignedDna {
        signature,
        public_key_pem,
        signer_kind: "key".to_string(),
    })
}

fn sign_dna_keyless(dna_hash: &str) -> Result<SignedDna, SigstoreCommandError> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let fulcio = FulcioClient::new(
        url::Url::parse(FULCIO_ROOT)?,
        TokenProvider::Oauth(OauthTokenProvider::default()),
    );
    let (signer, cert_chain) = rt.block_on(fulcio.request_cert(SigningScheme::default()))?;
    let signature = signer.sign(&dna_hash_bytes(dna_hash)?)?;
    let public_key_pem = cert_chain.to_string();

    Ok(SignedDna {
        signature,
        public_key_pem,
        signer_kind: "keyless".to_string(),
    })
}

fn build_rekor_entry(dna_hash: &str, signed: &SignedDna) -> Result<ProposedEntry, SigstoreCommandError> {
    let signature_b64 = BASE64_STD_ENGINE.encode(&signed.signature);
    let public_key_b64 = BASE64_STD_ENGINE.encode(signed.public_key_pem.as_bytes());
    let hash_value = dna_hash.strip_prefix("sha256:").unwrap_or(dna_hash).to_string();

    Ok(ProposedEntry::Hashedrekord {
        api_version: "0.0.1".to_string(),
        spec: hashedrekord::Spec::new(
            hashedrekord::Signature::new(signature_b64, hashedrekord::PublicKey::new(public_key_b64)),
            hashedrekord::Data::new(hashedrekord::Hash::new(
                hashedrekord::AlgorithmKind::sha256,
                hash_value,
            )),
        ),
    })
}

fn verify_rekor_entry(
    entry: &RekorLogEntry,
    dna_hash: &str,
) -> Result<RekorVerification, SigstoreCommandError> {
    let spec = extract_hashedrekord_spec(entry)?;
    let expected = dna_hash.strip_prefix("sha256:").unwrap_or(dna_hash);
    let hash_match = expected == spec.data.hash.value;

    let signature_b64 = &spec.signature.content;
    if signature_b64.is_empty() {
        return Err(SigstoreCommandError::MissingSignature);
    }

    // public_key.decode() returns the base64-decoded PEM as a String
    let public_key_pem_str = spec
        .signature
        .public_key
        .decode()
        .map_err(|_| SigstoreCommandError::MissingPublicKey)?;
    if public_key_pem_str.is_empty() {
        return Err(SigstoreCommandError::MissingPublicKey);
    }

    let signature = BASE64_STD_ENGINE.decode(signature_b64)?;
    let public_key_pem = public_key_pem_str.as_bytes().to_vec();
    let parsed_pem = pem::parse(&public_key_pem)?;

    let (verification_key, signer_kind) = if parsed_pem.tag() == "CERTIFICATE" {
        let cert = x509_cert::Certificate::from_der(parsed_pem.contents())
            .map_err(|_| SigstoreCommandError::UnsupportedPublicKey)?;
        let key = CosignVerificationKey::try_from(&cert.tbs_certificate.subject_public_key_info)
            .map_err(|_| SigstoreCommandError::UnsupportedPublicKey)?;
        (key, "keyless".to_string())
    } else {
        let key = CosignVerificationKey::try_from_pem(&public_key_pem)
            .map_err(|_| SigstoreCommandError::UnsupportedPublicKey)?;
        (key, "key".to_string())
    };

    let signature_valid = verification_key
        .verify_signature(Signature::Raw(&signature), &dna_hash_bytes(dna_hash)?)
        .is_ok();

    let cert_chain_valid = if signer_kind == "keyless" {
        verify_fulcio_chain(&public_key_pem, entry.integrated_time).is_ok()
    } else {
        false
    };

    Ok(RekorVerification {
        hash_match,
        signature_valid,
        cert_chain_valid,
        entry_uuid: entry.uuid.clone(),
        entry_index: entry.log_index,
        signer_kind,
    })
}

fn verify_fulcio_chain(cert_pem: &[u8], integrated_time: i64) -> Result<(), SigstoreCommandError> {
    let pem = pem::parse(cert_pem)?;
    if pem.tag() != "CERTIFICATE" {
        return Err(SigstoreCommandError::FulcioChain(
            "public key is not a certificate".to_string(),
        ));
    }

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let trust_root = rt.block_on(SigstoreTrustRoot::new(None))?;
    let fulcio_certs = trust_root.fulcio_certs()?;

    let trust_anchors: Vec<TrustAnchor<'static>> = fulcio_certs
        .into_iter()
        .map(|cert| webpki::anchor_from_trusted_cert(&cert).map(|anchor| anchor.to_owned()))
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|err| SigstoreCommandError::FulcioChain(err.to_string()))?;

    let cert_der = CertificateDer::from(pem.contents().to_vec());
    let end_entity = EndEntityCert::try_from(&cert_der)
        .map_err(|err| SigstoreCommandError::FulcioChain(err.to_string()))?;

    let verification_time = if integrated_time > 0 {
        UnixTime::since_unix_epoch(Duration::from_secs(integrated_time as u64))
    } else {
        UnixTime::since_unix_epoch(Duration::from_secs(Utc::now().timestamp() as u64))
    };

    end_entity
        .verify_for_usage(
            webpki::ALL_VERIFICATION_ALGS,
            &trust_anchors,
            &[],
            verification_time,
            KeyUsage::required(ID_KP_CODE_SIGNING.as_bytes()),
            None,
            None,
        )
        .map_err(|err| SigstoreCommandError::FulcioChain(err.to_string()))?;

    Ok(())
}

// === Helper functions ===

/// Parse package@version format
fn parse_package_spec(spec: &str) -> (String, Option<String>) {
    if let Some(idx) = spec.rfind('@') {
        let name = spec[..idx].to_string();
        let version = spec[idx + 1..].to_string();
        (name, Some(version))
    } else {
        (spec.to_string(), None)
    }
}

/// Find a trove by name and optional version
fn find_trove(
    conn: &Connection,
    name: &str,
    version: Option<&str>,
) -> Result<Option<(i64, String, String)>> {
    let query = if version.is_some() {
        "SELECT id, name, version FROM troves WHERE name = ?1 AND version = ?2 AND type = 'package'"
    } else {
        "SELECT id, name, version FROM troves WHERE name = ?1 AND type = 'package' ORDER BY installed_at DESC LIMIT 1"
    };

    let result = if let Some(v) = version {
        conn.query_row(query, [name, v], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
    } else {
        conn.query_row(query, [name], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
    };

    match result {
        Ok(row) => Ok(Some(row)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Provenance data from database
#[derive(Debug, Default)]
struct ProvenanceData {
    // Source layer
    upstream_url: Option<String>,
    upstream_hash: Option<String>,
    git_commit: Option<String>,
    patches_json: Option<String>,

    // Build layer
    recipe_hash: Option<String>,
    build_deps_json: Option<String>,
    host_arch: Option<String>,
    host_kernel: Option<String>,

    // Signature layer
    signatures_json: Option<String>,
    rekor_log_index: Option<i64>,

    // Content layer
    merkle_root: Option<String>,
    dna_hash: Option<String>,
}

/// Query provenance from database
fn query_provenance(conn: &Connection, trove_id: i64) -> Result<ProvenanceData> {
    let result = conn.query_row(
        "SELECT upstream_url, upstream_hash, source_commit, patches_json,
                recipe_hash, build_deps_json, host_arch, host_kernel,
                signatures_json, rekor_log_index, merkle_root, dna_hash
         FROM provenance WHERE trove_id = ?1",
        [trove_id],
        |row| {
            Ok(ProvenanceData {
                upstream_url: row.get(0)?,
                upstream_hash: row.get(1)?,
                git_commit: row.get(2)?,
                patches_json: row.get(3)?,
                recipe_hash: row.get(4)?,
                build_deps_json: row.get(5)?,
                host_arch: row.get(6)?,
                host_kernel: row.get(7)?,
                signatures_json: row.get(8)?,
                rekor_log_index: row.get(9)?,
                merkle_root: row.get(10)?,
                dna_hash: row.get(11)?,
            })
        },
    );

    match result {
        Ok(prov) => Ok(prov),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(ProvenanceData::default()),
        Err(e) => Err(e.into()),
    }
}

/// Print provenance in text format
fn print_provenance_text(
    name: &str,
    version: &str,
    prov: &ProvenanceData,
    section: &str,
    _recursive: bool,
) -> Result<()> {
    println!("=== Package DNA: {} v{} ===", name, version);
    println!();

    if let Some(ref dna) = prov.dna_hash {
        println!("DNA Hash: {}", dna);
    } else {
        println!("DNA Hash: (not computed)");
    }
    println!();

    let show_source = section == "all" || section == "source";
    let show_build = section == "all" || section == "build";
    let show_signatures = section == "all" || section == "signatures";
    let show_content = section == "all" || section == "content";

    if show_source {
        println!("--- Source Layer ---");
        if let Some(ref url) = prov.upstream_url {
            println!("  Upstream: {}", url);
        }
        if let Some(ref hash) = prov.upstream_hash {
            println!("  Hash: {}", hash);
        }
        if let Some(ref commit) = prov.git_commit {
            println!("  Git commit: {}", commit);
        }
        if prov.patches_json.is_some() {
            println!("  Patches: (see JSON output for details)");
        }
        if prov.upstream_url.is_none() && prov.git_commit.is_none() {
            println!("  (no source provenance recorded)");
        }
        println!();
    }

    if show_build {
        println!("--- Build Layer ---");
        if let Some(ref hash) = prov.recipe_hash {
            println!("  Recipe hash: {}", hash);
        }
        if let Some(ref arch) = prov.host_arch {
            println!("  Build arch: {}", arch);
        }
        if let Some(ref kernel) = prov.host_kernel {
            println!("  Build kernel: {}", kernel);
        }
        if prov.build_deps_json.is_some() {
            println!("  Build deps: (see JSON output for details)");
        }
        if prov.recipe_hash.is_none() {
            println!("  (no build provenance recorded)");
        }
        println!();
    }

    if show_signatures {
        println!("--- Signature Layer ---");
        if prov.signatures_json.is_some() {
            println!("  Signatures: (see JSON output for details)");
        } else {
            println!("  (no signatures recorded)");
        }
        if let Some(idx) = prov.rekor_log_index {
            println!("  Rekor log index: {}", idx);
        }
        println!();
    }

    if show_content {
        println!("--- Content Layer ---");
        if let Some(ref root) = prov.merkle_root {
            println!("  Merkle root: {}", root);
        } else {
            println!("  (no content hash recorded)");
        }
        println!();
    }

    Ok(())
}

/// Print provenance in JSON format
fn print_provenance_json(prov: &ProvenanceData, section: &str, _recursive: bool) -> Result<()> {
    let json = serde_json::json!({
        "dna_hash": prov.dna_hash,
        "source": {
            "upstream_url": prov.upstream_url,
            "upstream_hash": prov.upstream_hash,
            "git_commit": prov.git_commit,
        },
        "build": {
            "recipe_hash": prov.recipe_hash,
            "host_arch": prov.host_arch,
            "host_kernel": prov.host_kernel,
        },
        "signatures": {
            "rekor_log_index": prov.rekor_log_index,
        },
        "content": {
            "merkle_root": prov.merkle_root,
        },
        "section_filter": section,
    });

    println!("{}", serde_json::to_string_pretty(&json)?);
    Ok(())
}

/// Print provenance in tree format
fn print_provenance_tree(prov: &ProvenanceData, _section: &str, _recursive: bool) -> Result<()> {
    println!("DNA: {}", prov.dna_hash.as_deref().unwrap_or("(none)"));
    println!("├── Source");
    println!("│   ├── URL: {}", prov.upstream_url.as_deref().unwrap_or("(none)"));
    println!("│   ├── Hash: {}", prov.upstream_hash.as_deref().unwrap_or("(none)"));
    println!("│   └── Git: {}", prov.git_commit.as_deref().unwrap_or("(none)"));
    println!("├── Build");
    println!("│   ├── Recipe: {}", prov.recipe_hash.as_deref().unwrap_or("(none)"));
    println!("│   ├── Arch: {}", prov.host_arch.as_deref().unwrap_or("(none)"));
    println!("│   └── Kernel: {}", prov.host_kernel.as_deref().unwrap_or("(none)"));
    println!("├── Signatures");
    println!("│   └── Rekor: {}", prov.rekor_log_index.map(|i| i.to_string()).unwrap_or_else(|| "(none)".to_string()));
    println!("└── Content");
    println!("    └── Merkle: {}", prov.merkle_root.as_deref().unwrap_or("(none)"));

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_rekor_entry_uses_dna_hash() {
        let signed = SignedDna {
            signature: vec![1, 2, 3],
            public_key_pem: "-----BEGIN PUBLIC KEY-----\nTEST\n-----END PUBLIC KEY-----".to_string(),
            signer_kind: "key".to_string(),
        };

        let entry = build_rekor_entry("sha256:deadbeef", &signed).unwrap();
        match entry {
            ProposedEntry::Hashedrekord { spec, .. } => {
                assert_eq!(spec.data.hash.value, "deadbeef");
            }
            _ => panic!("expected hashedrekord entry"),
        }
    }

    #[test]
    #[ignore]
    fn rekor_sign_verify_roundtrip() {
        let db_path = std::env::var("CONARY_TEST_DB").expect("CONARY_TEST_DB is required");
        let package = std::env::var("CONARY_TEST_PACKAGE")
            .expect("CONARY_TEST_PACKAGE is required");
        let key = std::env::var("CONARY_TEST_KEY").ok();
        let keyless = std::env::var("CONARY_TEST_KEYLESS").is_ok();

        let _ = cmd_provenance_register(
            &db_path,
            &package,
            key.as_deref(),
            keyless,
            false,
        )
        .unwrap();

        let _ = cmd_provenance_verify(&db_path, &package, true).unwrap();
    }
}
