// apps/conary/src/commands/provenance/render.rs

//! Human and machine-readable provenance presentation.

use super::ProvenanceData;
use anyhow::Result;

pub(super) fn print_text(
    name: &str,
    version: &str,
    prov: &ProvenanceData,
    section: &str,
    _recursive: bool,
) -> Result<()> {
    println!("=== Package DNA: {name} v{version} ===");
    println!();

    if let Some(ref dna) = prov.dna_hash {
        println!("DNA Hash: {dna}");
    } else {
        println!("DNA Hash: (not computed)");
    }
    println!();

    let show = |name: &str| section == "all" || section == name;

    if show("source") {
        println!("--- Source Layer ---");
        if let Some(ref url) = prov.upstream_url {
            println!("  Upstream: {url}");
        }
        if let Some(ref hash) = prov.upstream_hash {
            println!("  Hash: {hash}");
        }
        if let Some(ref commit) = prov.git_commit {
            println!("  Git commit: {commit}");
        }
        if prov.patches_json.is_some() {
            println!("  Patches: (see JSON output for details)");
        }
        if prov.upstream_url.is_none() && prov.git_commit.is_none() {
            println!("  (no source provenance recorded)");
        }
        println!();
    }

    if show("build") {
        println!("--- Build Layer ---");
        if let Some(ref hash) = prov.recipe_hash {
            println!("  Recipe hash: {hash}");
        }
        if let Some(ref arch) = prov.host_arch {
            println!("  Build arch: {arch}");
        }
        if let Some(ref kernel) = prov.host_kernel {
            println!("  Build kernel: {kernel}");
        }
        if prov.build_deps_json.is_some() {
            println!("  Build deps: (see JSON output for details)");
        }
        if prov.recipe_hash.is_none() {
            println!("  (no build provenance recorded)");
        }
        println!();
    }

    if show("signatures") {
        println!("--- Signature Layer ---");
        if prov.signatures_json.is_some() {
            println!("  Signatures: (see JSON output for details)");
        } else {
            println!("  (no signatures recorded)");
        }
        if let Some(index) = prov.rekor_log_index {
            println!("  Rekor log index: {index}");
        }
        println!();
    }

    if show("content") {
        println!("--- Content Layer ---");
        if let Some(ref root) = prov.merkle_root {
            println!("  Merkle root: {root}");
        } else {
            println!("  (no content hash recorded)");
        }
        println!();
    }

    Ok(())
}

pub(super) fn print_json(prov: &ProvenanceData, section: &str, _recursive: bool) -> Result<()> {
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

pub(super) fn print_tree(prov: &ProvenanceData, _section: &str, _recursive: bool) -> Result<()> {
    fn field(value: &Option<String>) -> &str {
        value.as_deref().unwrap_or("(none)")
    }

    let rekor = prov
        .rekor_log_index
        .map(|index| index.to_string())
        .unwrap_or_else(|| "(none)".to_string());

    println!("DNA: {}", field(&prov.dna_hash));
    println!("├── Source");
    println!("│   ├── URL: {}", field(&prov.upstream_url));
    println!("│   ├── Hash: {}", field(&prov.upstream_hash));
    println!("│   └── Git: {}", field(&prov.git_commit));
    println!("├── Build");
    println!("│   ├── Recipe: {}", field(&prov.recipe_hash));
    println!("│   ├── Arch: {}", field(&prov.host_arch));
    println!("│   └── Kernel: {}", field(&prov.host_kernel));
    println!("├── Signatures");
    println!("│   └── Rekor: {rekor}");
    println!("└── Content");
    println!("    └── Merkle: {}", field(&prov.merkle_root));

    Ok(())
}
