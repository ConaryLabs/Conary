# Bootstrap v2 Phase 6: Verification & Audit Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add provenance record generation, trust level tracking, three verification commands (chain, rebuild, diverse), and derivation-aware SBOM generation to make "built from source" provable.

**Architecture:** The executor generates a `Provenance` record (using the existing 4-layer provenance module) after each successful build, storing it as a CAS object. Trust levels (0-4) track verification status on `DerivationRecord`. Three CLI commands (`verify chain/rebuild/diverse`) query and upgrade trust levels. A new `sbom` command generates CycloneDX from derivation profiles.

**Tech Stack:** Rust 1.94, existing provenance module (4 layers), DerivationIndex (rusqlite), CycloneDX SBOM types, clap CLI

**Spec:** `docs/superpowers/specs/2026-03-20-bootstrap-v2-phase6-verification-audit.md` (revision 3)

---

## File Structure

### New files

| File | Purpose |
|------|---------|
| `src/cli/verify.rs` | `VerifyCommands` CLI definitions (chain, rebuild, diverse) |
| `src/commands/verify.rs` | Verification command handlers |
| `src/commands/derivation_sbom.rs` | Derivation-aware SBOM generation |

### Modified files

| File | Change |
|------|--------|
| `conary-core/src/db/migrations.rs` | Add `migrate_v56` (ALTER TABLE columns) |
| `conary-core/src/db/schema.rs` | Bump SCHEMA_VERSION to 56 |
| `conary-core/src/derivation/index.rs` | Add fields to `DerivationRecord`, add `set_trust_level`/`set_reproducible` methods |
| `conary-core/src/derivation/executor.rs` | Build `Provenance` after successful build |
| `conary-core/src/derivation/pipeline.rs` | Set trust level 1/2 after substituter hit/local build |
| `src/cli/mod.rs` | Add `Verify(VerifyCommands)` + `Sbom` to Commands enum |
| `src/commands/mod.rs` | Add verify + derivation_sbom modules |
| `src/main.rs` | Wire Verify + Sbom dispatch |

---

## Task 1: Database Migration v56

**Files:**
- Modify: `conary-core/src/db/migrations.rs`
- Modify: `conary-core/src/db/schema.rs`

- [ ] **Step 1: Add migrate_v56**

In `migrations.rs`, after `migrate_v55`:

```rust
pub fn migrate_v56(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        ALTER TABLE derivation_index ADD COLUMN trust_level INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE derivation_index ADD COLUMN provenance_cas_hash TEXT;
        ALTER TABLE derivation_index ADD COLUMN reproducible INTEGER;
        ALTER TABLE derivation_cache ADD COLUMN provenance_cas_hash TEXT;
        ",
    )?;
    Ok(())
}
```

- [ ] **Step 2: Update schema.rs**

Change `SCHEMA_VERSION` from 55 to 56. Add `56 => migrations::migrate_v56(conn),` dispatch case.

- [ ] **Step 3: Verify**

Run: `cargo test -p conary-core db -- --nocapture`

- [ ] **Step 4: Commit**

```
feat(db): add v56 migration for trust levels and provenance

Add trust_level (0-4), provenance_cas_hash, and reproducible columns
to derivation_index. Add provenance_cas_hash to derivation_cache.
```

---

## Task 2: DerivationRecord + DerivationIndex Updates

**Files:**
- Modify: `conary-core/src/derivation/index.rs`

- [ ] **Step 1: Add fields to DerivationRecord**

Add after `build_duration_secs`:

```rust
    /// Trust level (0=unverified, 1=substituted, 2=locally built,
    /// 3=independently verified, 4=diverse-verified).
    pub trust_level: u8,
    /// CAS hash of the JSON provenance record.
    pub provenance_cas_hash: Option<String>,
    /// Reproducibility status: None=unknown, Some(true)=reproducible, Some(false)=not.
    pub reproducible: Option<bool>,
```

- [ ] **Step 2: Update SQL queries**

Update the `INSERT`, `SELECT` queries in `lookup()`, `insert()`, `by_package()` to include the three new columns. For `insert()`, default `trust_level` to 0 if not set. For `lookup()`/`by_package()`, read the new columns from the row.

- [ ] **Step 3: Add set_trust_level and set_reproducible**

```rust
/// Upgrade trust level (monotonic via SQL MAX).
pub fn set_trust_level(&self, derivation_id: &str, level: u8) -> Result<()> {
    self.conn.execute(
        "UPDATE derivation_index SET trust_level = MAX(trust_level, ?2) WHERE derivation_id = ?1",
        rusqlite::params![derivation_id, level],
    )?;
    Ok(())
}

/// Set reproducibility flag.
pub fn set_reproducible(&self, derivation_id: &str, reproducible: bool) -> Result<()> {
    self.conn.execute(
        "UPDATE derivation_index SET reproducible = ?2 WHERE derivation_id = ?1",
        rusqlite::params![derivation_id, reproducible],
    )?;
    Ok(())
}
```

- [ ] **Step 4: Fix all DerivationRecord construction sites**

Search the codebase for `DerivationRecord {` and add the new fields with defaults:

```rust
trust_level: 0,
provenance_cas_hash: None,
reproducible: None,
```

Key locations: `executor.rs`, `pipeline.rs`, `pipeline.rs` tests.

- [ ] **Step 5: Add tests**

```rust
#[test]
fn set_trust_level_is_monotonic() {
    let conn = Connection::open_in_memory().unwrap();
    migrate(&conn).unwrap();
    let index = DerivationIndex::new(&conn);

    let record = DerivationRecord {
        derivation_id: "test_id".to_owned(),
        // ... other fields ...
        trust_level: 2,
        provenance_cas_hash: None,
        reproducible: None,
    };
    index.insert(&record).unwrap();

    // Upgrade from 2 to 3
    index.set_trust_level("test_id", 3).unwrap();
    let r = index.lookup("test_id").unwrap().unwrap();
    assert_eq!(r.trust_level, 3);

    // Attempt downgrade from 3 to 1 -- should stay at 3
    index.set_trust_level("test_id", 1).unwrap();
    let r = index.lookup("test_id").unwrap().unwrap();
    assert_eq!(r.trust_level, 3, "trust level should not decrease");
}
```

- [ ] **Step 6: Verify**

Run: `cargo test -p conary-core derivation::index -- --nocapture`
Run: `cargo test -p conary-core derivation -- --nocapture`
Run: `cargo clippy -p conary-core -- -D warnings`

- [ ] **Step 7: Commit**

```
feat(derivation): add trust level, provenance, reproducible to DerivationRecord

DerivationRecord gains trust_level (u8, 0-4), provenance_cas_hash
(Option<String>), and reproducible (Option<bool>). DerivationIndex
gains set_trust_level (monotonic via SQL MAX) and set_reproducible.
```

---

## Task 3: Provenance Generation in Executor

**Files:**
- Modify: `conary-core/src/derivation/executor.rs`

- [ ] **Step 1: Read the provenance module**

Read `conary-core/src/provenance/mod.rs`, `source.rs`, `build.rs`, `signature.rs`, `content.rs` to understand the constructors. Key methods:
- `SourceProvenance::from_tarball(url, hash)`
- `BuildProvenance::new(recipe_hash)`
- `HostAttestation::from_current_system()`
- `SignatureProvenance::with_builder(sig)` or `SignatureProvenance::default()`
- `ContentProvenance::new(merkle_root)`
- `Provenance::new(source, build, signatures, content)`
- `provenance.to_json()`

- [ ] **Step 2: Add provenance construction after successful build**

In `execute()`, after the success path captures `pkg_output` and records in the derivation index (around line 430-440), add provenance construction:

```rust
// Build provenance record
let source_prov = crate::provenance::SourceProvenance::from_tarball(
    &recipe.source.archive,
    &recipe.source.checksum,
);

let mut build_prov = crate::provenance::BuildProvenance::new(
    &recipe_hash::build_script_hash(recipe),
);
build_prov.build_env.push(("build_env_hash".to_owned(), build_env_hash.to_owned()));
build_prov.build_env.push(("target_triple".to_owned(), target_triple.to_owned()));
build_prov.build_env.push(("derivation_id".to_owned(), derivation_id.as_str().to_owned()));
if let Ok(host) = std::panic::catch_unwind(crate::provenance::HostAttestation::from_current_system) {
    build_prov.set_host_attestation(host);
}

let sig_prov = crate::provenance::SignatureProvenance::default();

let content_prov = {
    let total_size: u64 = pkg_output.manifest.files.iter().map(|f| f.size).sum();
    let file_count = (pkg_output.manifest.files.len() + pkg_output.manifest.symlinks.len()) as u64;
    let mut cp = crate::provenance::ContentProvenance::new(&pkg_output.manifest.output_hash);
    cp.total_size = total_size;
    cp.file_count = file_count;
    cp
};

let provenance = crate::provenance::Provenance::new(source_prov, build_prov, sig_prov, content_prov);

// Store provenance as CAS object
let provenance_cas_hash = match provenance.to_json() {
    Ok(json) => match self.cas.store(json.as_bytes()) {
        Ok(hash) => Some(hash),
        Err(e) => {
            tracing::warn!("failed to store provenance: {e}");
            None
        }
    },
    Err(e) => {
        tracing::warn!("failed to serialize provenance: {e}");
        None
    }
};

// Update the derivation record with provenance hash and trust level
let index = DerivationIndex::new(conn);
if let Some(ref hash) = provenance_cas_hash {
    let _ = conn.execute(
        "UPDATE derivation_index SET provenance_cas_hash = ?2, trust_level = 2 WHERE derivation_id = ?1",
        rusqlite::params![derivation_id.as_str(), hash],
    );
}
```

Note: read the actual `SourceProvenance`, `BuildProvenance`, etc. constructors before implementing — the code samples above are illustrative. The real API may differ slightly (field names, builder patterns). Read the source files listed in Step 1.

- [ ] **Step 3: Add test**

```rust
#[test]
fn successful_build_generates_provenance() {
    // This test would need a recipe that actually builds successfully,
    // which requires Kitchen infrastructure. For now, verify that the
    // provenance types can be constructed without panicking.
    let source = conary_core::provenance::SourceProvenance::from_tarball(
        "https://example.com/test.tar.gz", "sha256:abc123",
    );
    let build = conary_core::provenance::BuildProvenance::new("script_hash");
    let sig = conary_core::provenance::SignatureProvenance::default();
    let content = conary_core::provenance::ContentProvenance::new("output_hash");
    let prov = conary_core::provenance::Provenance::new(source, build, sig, content);
    let json = prov.to_json().unwrap();
    assert!(json.contains("output_hash"));
}
```

- [ ] **Step 4: Verify**

Run: `cargo test -p conary-core derivation::executor -- --nocapture`
Run: `cargo clippy -p conary-core -- -D warnings`

- [ ] **Step 5: Commit**

```
feat(derivation): generate provenance record on successful build

Executor constructs a 4-layer Provenance record after each build using
SourceProvenance (recipe archive), BuildProvenance (recipe hash, env,
host), SignatureProvenance (empty, populated later by verify-rebuild),
and ContentProvenance (output hash, file count, size). Stored as JSON
in CAS, hash recorded on DerivationRecord.
```

---

## Task 4: Trust Level Assignment in Pipeline

**Files:**
- Modify: `conary-core/src/derivation/pipeline.rs`

- [ ] **Step 1: Set trust level 2 on local builds**

In `Pipeline::execute()`, in the `ExecutionResult::Built` arm, after recording the manifest, add:

```rust
// Set trust level 2 (locally built)
let idx = super::index::DerivationIndex::new(conn);
let _ = idx.set_trust_level(derivation_id.as_str(), 2);
```

- [ ] **Step 2: Set trust level 1 on substituter hits**

In the substituter hit block (where `SubstituterHit` event is emitted), add:

```rust
// Set trust level 1 (substituted from remote cache)
let _ = idx.set_trust_level(drv_id.as_str(), 1);
```

- [ ] **Step 3: Verify**

Run: `cargo test -p conary-core derivation::pipeline -- --nocapture`
Run: `cargo clippy -p conary-core -- -D warnings`

- [ ] **Step 4: Commit**

```
feat(derivation): assign trust levels in pipeline execution

Local builds get trust level 2. Substituter cache hits get trust
level 1. Uses monotonic set_trust_level (SQL MAX).
```

---

## Task 5: Verify Chain Command

**Files:**
- Create: `src/cli/verify.rs`
- Create: `src/commands/verify.rs`
- Modify: `src/cli/mod.rs`
- Modify: `src/commands/mod.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Create CLI definitions**

Create `src/cli/verify.rs`:

```rust
// src/cli/verify.rs
//! CLI definitions for verification commands.

use clap::Subcommand;

/// Verification commands for derivation integrity.
#[derive(Subcommand)]
pub enum VerifyCommands {
    /// Trace all packages in a profile back to the seed
    Chain {
        /// Path to profile TOML
        #[arg(long)]
        profile: String,

        /// Show full provenance details
        #[arg(long)]
        verbose: bool,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Rebuild a derivation and compare output hash
    Rebuild {
        /// Derivation ID or package name
        derivation: String,

        /// Working directory for rebuild
        #[arg(long, default_value = ".conary/verify")]
        work_dir: String,
    },

    /// Compare builds from two different seeds
    Diverse {
        /// Profile from first seed build
        #[arg(long)]
        profile_a: String,

        /// Profile from second seed build
        #[arg(long)]
        profile_b: String,
    },
}
```

- [ ] **Step 2: Create verify chain handler**

Create `src/commands/verify.rs` with `cmd_verify_chain`:

```rust
// src/commands/verify.rs
//! Verification command handlers.

use anyhow::Result;
use conary_core::derivation::index::DerivationIndex;
use conary_core::derivation::profile::BuildProfile;

pub fn cmd_verify_chain(profile_path: &str, verbose: bool, json: bool) -> Result<()> {
    let content = std::fs::read_to_string(profile_path)?;
    let profile: BuildProfile = toml::from_str(&content)?;

    let db_path = "/var/lib/conary/conary.db";
    let conn = super::open_db(db_path)?;
    let index = DerivationIndex::new(&conn);

    let mut total = 0usize;
    let mut found = 0usize;
    let mut trust_counts = [0usize; 5]; // levels 0-4
    let mut warnings = Vec::new();
    let mut chain_broken = false;

    println!("Seed: {} ({})", profile.seed.id, profile.seed.source);
    println!();

    for stage in &profile.stages {
        println!("Stage: {} ({} packages)", stage.name, stage.derivations.len());

        for drv in &stage.derivations {
            total += 1;
            if drv.derivation_id == "pending" {
                println!("  {}-{}    [pending]", drv.package, drv.version);
                continue;
            }

            match index.lookup(&drv.derivation_id) {
                Ok(Some(record)) => {
                    found += 1;
                    let level = record.trust_level.min(4) as usize;
                    trust_counts[level] += 1;

                    let trust_name = match record.trust_level {
                        0 => "unverified",
                        1 => "substituted",
                        2 => "locally built",
                        3 => "independently verified",
                        4 => "diverse-verified",
                        _ => "unknown",
                    };

                    println!("  {}-{}    [level {}: {}]",
                        drv.package, drv.version, record.trust_level, trust_name);

                    if verbose {
                        if let Some(ref prov_hash) = record.provenance_cas_hash {
                            println!("    provenance: {}", prov_hash);
                        }
                        println!("    output: {}", &record.output_hash[..16.min(record.output_hash.len())]);
                    }

                    if record.provenance_cas_hash.is_none() {
                        warnings.push(format!("{}: missing provenance", drv.package));
                    }
                }
                Ok(None) => {
                    chain_broken = true;
                    println!("  {}-{}    [MISSING from local index]", drv.package, drv.version);
                }
                Err(e) => {
                    chain_broken = true;
                    println!("  {}-{}    [ERROR: {}]", drv.package, drv.version, e);
                }
            }
        }
        println!();
    }

    // Summary
    let status = if chain_broken { "BROKEN" } else { "COMPLETE" };
    println!("Chain: {status}");
    println!("  {found}/{total} derivations traced to seed {}", &profile.seed.id[..16.min(profile.seed.id.len())]);

    let above_2: usize = trust_counts[2..].iter().sum();
    println!("  {above_2}/{total} at trust level >= 2");

    for w in &warnings {
        println!("  [WARN] {w}");
    }

    if chain_broken {
        std::process::exit(1);
    }

    Ok(())
}
```

- [ ] **Step 3: Wire into CLI and main.rs**

In `src/cli/mod.rs`: add `mod verify;`, `pub use verify::VerifyCommands;`, and `#[command(subcommand)] Verify(VerifyCommands)` to Commands.

In `src/commands/mod.rs`: add `mod verify;`, `pub use verify::{cmd_verify_chain, cmd_verify_rebuild, cmd_verify_diverse};`

In `src/main.rs`: add dispatch for `Commands::Verify(cmd)`.

- [ ] **Step 4: Verify**

Run: `cargo build`
Run: `cargo run -- verify --help`
Run: `cargo run -- verify chain --help`

- [ ] **Step 5: Commit**

```
feat: add conary verify chain command

Traces all derivations in a profile back to the seed, displaying
trust levels per package. Reports chain status (COMPLETE/BROKEN),
trust level distribution, and provenance warnings. Supports
--verbose and --json output modes.
```

---

## Task 6: Verify Rebuild Command

**Files:**
- Modify: `src/commands/verify.rs`

- [ ] **Step 1: Add cmd_verify_rebuild**

```rust
pub fn cmd_verify_rebuild(derivation: &str, work_dir: &str) -> Result<()> {
    let db_path = "/var/lib/conary/conary.db";
    let conn = super::open_db(db_path)?;
    let index = DerivationIndex::new(&conn);

    // Resolve derivation ID (could be a package name)
    let record = if derivation.len() == 64 && derivation.chars().all(|c| c.is_ascii_hexdigit()) {
        index.lookup(derivation)?
            .ok_or_else(|| anyhow::anyhow!("derivation {derivation} not found"))?
    } else {
        // Treat as package name
        let records = index.by_package(derivation)?;
        records.into_iter().next()
            .ok_or_else(|| anyhow::anyhow!("no derivation found for package '{derivation}'"))?
    };

    println!("Rebuilding {}-{} (derivation {}...)",
        record.package_name, record.package_version,
        &record.derivation_id[..16]);

    // Resolve recipe from recipes/ directory
    let recipe_path = find_recipe(&record.package_name)?;
    let recipe = conary_core::recipe::parse_recipe_file(&recipe_path)?;

    // Create fresh in-memory DB for the rebuild (bypasses cache)
    let rebuild_conn = rusqlite::Connection::open_in_memory()?;
    conary_core::db::schema::migrate(&rebuild_conn)?;

    // Set up executor with fresh DB
    let cas_dir = std::path::PathBuf::from(work_dir).join("cas");
    std::fs::create_dir_all(&cas_dir)?;
    let cas = conary_core::filesystem::CasStore::new(&cas_dir)?;
    let exec_config = conary_core::derivation::executor::ExecutorConfig::default();
    let executor = conary_core::derivation::executor::DerivationExecutor::new(
        cas, cas_dir, exec_config,
    );

    let build_env_hash = record.build_env_hash.as_deref().unwrap_or("unknown");
    let sysroot = std::path::PathBuf::from(work_dir).join("sysroot");
    std::fs::create_dir_all(&sysroot)?;

    let dep_ids = std::collections::BTreeMap::new(); // simplified for now
    let target = "x86_64-unknown-linux-gnu";

    match executor.execute(&recipe, build_env_hash, &dep_ids, target, &sysroot, &rebuild_conn) {
        Ok(conary_core::derivation::executor::ExecutionResult::Built { output, .. }) => {
            let new_hash = &output.manifest.output_hash;
            let original_hash = &record.output_hash;

            if new_hash == original_hash {
                println!("  Original output: {}...", &original_hash[..16]);
                println!("  Rebuild output:  {}...  MATCH", &new_hash[..16]);
                println!();
                index.set_trust_level(&record.derivation_id, 3)?;
                index.set_reproducible(&record.derivation_id, true)?;
                println!("  Trust level: {} -> 3 (independently verified)", record.trust_level);
                println!("  Reproducible: true");
            } else {
                println!("  Original output: {}...", &original_hash[..16]);
                println!("  Rebuild output:  {}...  MISMATCH", &new_hash[..16]);
                println!();
                index.set_reproducible(&record.derivation_id, false)?;
                println!("  Reproducible: false");
                // TODO: diff OutputManifests to show which files differ
            }
        }
        Ok(conary_core::derivation::executor::ExecutionResult::CacheHit { .. }) => {
            anyhow::bail!("unexpected cache hit on fresh DB — this should not happen");
        }
        Err(e) => {
            println!("  Rebuild failed: {e}");
            println!("  Cannot verify reproducibility.");
        }
    }

    Ok(())
}

/// Find a recipe file by package name in the recipes/ directory.
fn find_recipe(package_name: &str) -> Result<std::path::PathBuf> {
    for dir in &["recipes/system", "recipes/cross-tools", "recipes/tier2", "recipes"] {
        let path = std::path::PathBuf::from(dir).join(format!("{package_name}.toml"));
        if path.exists() {
            return Ok(path);
        }
    }
    anyhow::bail!("recipe for '{package_name}' not found in recipes/ directory")
}
```

- [ ] **Step 2: Wire dispatch**

In `src/main.rs`, add the `VerifyCommands::Rebuild` match arm.

- [ ] **Step 3: Verify**

Run: `cargo build`
Run: `cargo run -- verify rebuild --help`

- [ ] **Step 4: Commit**

```
feat: add conary verify rebuild command

Rebuilds a derivation in a fresh environment and compares output hash
against the original. On match, upgrades trust level to 3 and marks
as reproducible. On mismatch, flags as non-reproducible.
```

---

## Task 7: Verify Diverse Command

**Files:**
- Modify: `src/commands/verify.rs`

- [ ] **Step 1: Add cmd_verify_diverse**

```rust
pub fn cmd_verify_diverse(profile_a_path: &str, profile_b_path: &str) -> Result<()> {
    let a_content = std::fs::read_to_string(profile_a_path)?;
    let b_content = std::fs::read_to_string(profile_b_path)?;
    let profile_a: BuildProfile = toml::from_str(&a_content)?;
    let profile_b: BuildProfile = toml::from_str(&b_content)?;

    // Verify different seeds
    if profile_a.seed.id == profile_b.seed.id {
        anyhow::bail!("both profiles use the same seed ({}). Diverse verification requires different seeds.", &profile_a.seed.id[..16]);
    }

    println!("Comparing builds from 2 seeds:");
    println!("  Seed A: {}... ({})", &profile_a.seed.id[..16], profile_a.seed.source);
    println!("  Seed B: {}... ({})", &profile_b.seed.id[..16], profile_b.seed.source);
    println!();

    let db_path = "/var/lib/conary/conary.db";
    let conn = super::open_db(db_path)?;
    let index = DerivationIndex::new(&conn);

    // Build lookup maps: (package_name, version) -> derivation_id
    let a_map: std::collections::HashMap<(String, String), String> = profile_a.stages.iter()
        .flat_map(|s| s.derivations.iter())
        .filter(|d| d.derivation_id != "pending")
        .map(|d| ((d.package.clone(), d.version.clone()), d.derivation_id.clone()))
        .collect();

    let mut matches = 0usize;
    let mut mismatches = 0usize;
    let mut unmatched = 0usize;

    for stage in &profile_b.stages {
        for drv in &stage.derivations {
            if drv.derivation_id == "pending" { continue; }

            let key = (drv.package.clone(), drv.version.clone());
            let Some(a_id) = a_map.get(&key) else {
                unmatched += 1;
                continue;
            };

            // Load both records
            let a_record = index.lookup(a_id)?.ok_or_else(|| anyhow::anyhow!("missing record for {a_id}"))?;
            let b_record = index.lookup(&drv.derivation_id)?.ok_or_else(|| anyhow::anyhow!("missing record for {}", drv.derivation_id))?;

            if a_record.output_hash == b_record.output_hash {
                matches += 1;
                println!("  {}-{}:  MATCH (diverse-verified)", drv.package, drv.version);
                index.set_trust_level(a_id, 4)?;
                index.set_trust_level(&drv.derivation_id, 4)?;
            } else {
                mismatches += 1;
                println!("  {}-{}:  MISMATCH", drv.package, drv.version);
            }
        }
    }

    println!();
    let total = matches + mismatches;
    println!("  {matches}/{total} packages diverse-verified");
    if mismatches > 0 {
        println!("  {mismatches} packages with environment-dependent differences");
    }
    if unmatched > 0 {
        println!("  {unmatched} packages only in one profile (skipped)");
    }

    Ok(())
}
```

- [ ] **Step 2: Wire dispatch**

In `src/main.rs`, add the `VerifyCommands::Diverse` match arm.

- [ ] **Step 3: Verify**

Run: `cargo build`
Run: `cargo run -- verify diverse --help`

- [ ] **Step 4: Commit**

```
feat: add conary verify diverse command

Compares two profile builds from different seeds by matching packages
on name+version. Matching output hashes upgrade trust to level 4
(diverse-verified, Thompson attack resistant).
```

---

## Task 8: SBOM Command

**Files:**
- Create: `src/commands/derivation_sbom.rs`
- Modify: `src/cli/mod.rs`
- Modify: `src/commands/mod.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Add Sbom variant to Commands**

In `src/cli/mod.rs`, add to the `Commands` enum:

```rust
    /// Generate SBOM from derivation data
    #[command(name = "sbom")]
    Sbom {
        /// Generate from a profile
        #[arg(long)]
        profile: Option<String>,

        /// Generate for a single derivation
        #[arg(long)]
        derivation: Option<String>,

        /// Output file (default: stdout)
        #[arg(long, short)]
        output: Option<String>,
    },
```

- [ ] **Step 2: Create derivation_sbom handler**

Create `src/commands/derivation_sbom.rs` that generates CycloneDX JSON from a profile or single derivation. Reuse the `Bom`/`Component`/`Hash` types from `src/commands/query/sbom.rs` (read that file first to understand the types).

The handler loads derivation records from the index, constructs CycloneDX components with derivation-specific metadata (derivation_id, trust_level, source URL from provenance if available), and serializes to JSON.

- [ ] **Step 3: Wire into mod.rs and main.rs**

Add module, export, and dispatch.

- [ ] **Step 4: Verify**

Run: `cargo build`
Run: `cargo run -- sbom --help`

- [ ] **Step 5: Commit**

```
feat: add conary sbom command for derivation profiles

Generates CycloneDX SBOM from derivation profiles or individual
derivations. Includes derivation IDs, trust levels, and source
provenance data. Reuses existing CycloneDX types.
```

---

## Summary

| Task | What | Complexity |
|------|------|-----------|
| 1 | DB migration v56 | Small |
| 2 | DerivationRecord + Index updates | Medium (many construction sites to fix) |
| 3 | Provenance generation in executor | Medium (provenance API wiring) |
| 4 | Trust level in pipeline | Small |
| 5 | verify chain CLI | Medium (profile parsing + index queries) |
| 6 | verify rebuild CLI | Large (executor setup, recipe resolution) |
| 7 | verify diverse CLI | Medium (profile comparison) |
| 8 | SBOM command | Medium (CycloneDX type reuse) |

**Dependencies:** 1 -> 2 -> 3 -> 4 (sequential). 5, 6, 7, 8 depend on 2 but are independent of each other. Task 3 should come before 5-7 so provenance data is available.
