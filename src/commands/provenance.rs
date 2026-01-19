// src/commands/provenance.rs

//! Command implementations for Package DNA / Provenance queries

use anyhow::Result;
use rusqlite::Connection;

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
                println!("[NOT IMPLEMENTED] Would verify against rekor.sigstore.dev");
                println!();
            } else {
                println!("[WARN] No Rekor transparency log entry found");
                println!("       Run 'conary provenance register {}' to register", package);
                println!();
            }

            // Check signatures
            if let Some(ref sigs) = prov.signatures_json {
                println!("[CHECKING] Signatures...");
                let count = sigs.matches("keyid").count();
                if all_signatures {
                    println!("[NOT IMPLEMENTED] Would verify {} signature(s)", count);
                } else {
                    println!("[NOT IMPLEMENTED] Would verify builder signature");
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
            let _prov = query_provenance(&conn, trove_id)?;

            println!("[NOT IMPLEMENTED] SBOM Export");
            println!();
            println!("Package: {} v{}", trove_name, trove_version);
            println!("Format: {}", format);
            println!("Output: {}", output.unwrap_or("stdout"));
            println!("Recursive: {}", recursive);
            println!();
            println!("Would generate {} SBOM with provenance information.", format.to_uppercase());
        }
        None => {
            println!("Package '{}' not found", package);
            std::process::exit(1);
        }
    }

    Ok(())
}

/// Register provenance in transparency log
pub fn cmd_provenance_register(
    db_path: &str,
    package: &str,
    key: &str,
    dry_run: bool,
) -> Result<()> {
    let conn = Connection::open(db_path)?;
    let (name, version) = parse_package_spec(package);

    let trove_info = find_trove(&conn, &name, version.as_deref())?;

    match trove_info {
        Some((_trove_id, trove_name, trove_version)) => {
            println!("=== Register Provenance in Transparency Log ===");
            println!();
            println!("Package: {} v{}", trove_name, trove_version);
            println!("Signing key: {}", key);
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
                println!("[NOT IMPLEMENTED] Would sign and upload to Rekor");
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
