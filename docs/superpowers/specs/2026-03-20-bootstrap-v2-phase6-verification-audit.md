---
last_updated: 2026-03-20
revision: 3
summary: Phase 6 verification and audit — provenance generation, trust levels, verify-chain/rebuild/diverse, SBOM integration
---

# Bootstrap v2 Phase 6: Verification & Audit

## Overview

Phase 6 makes "built from source" provable. Phases 1-5 built the derivation
engine, developer experience, and sharing infrastructure. Phase 6 adds the
verification layer: every build produces a provenance record, trust levels
track verification status, and CLI commands let users trace, rebuild, and
cross-verify their system.

The design leverages extensive existing infrastructure: a complete 4-layer
provenance module (`conary-core/src/provenance/`), TUF trust metadata
(`conary-core/src/trust/`), CCS package signatures, Sigstore/Rekor
transparency log integration, CycloneDX SBOM generation, and fs-verity
support. Phase 6 wires these into the derivation engine and adds the
verification commands that expose them to users.

**Design date:** 2026-03-20

## Prerequisites

- Derivation engine (Phases 1-3) -- complete
- Developer experience (Phase 4) -- complete
- Substituters & sharing (Phase 5) -- complete
- Existing: provenance module (4 layers), TUF trust, CCS signatures,
  CycloneDX SBOM, fs-verity, `SourceDerivationId`

## Deferred to Phase 7

- SPDX SBOM generation (CycloneDX covers the same ground)
- Seed revocation checking (requires revocation list service on Remi)

## 1. Provenance Record Generation

### Problem

Derivation builds produce outputs (files in CAS + manifest) but no provenance
record. The existing `Provenance` type with its 4 layers (source, build,
signature, content) is fully implemented but never called from the derivation
engine.

### Design

After a successful build in `DerivationExecutor::execute()`, before returning
`ExecutionResult::Built`, the executor constructs a `Provenance` record:

- **Source layer** (`SourceProvenance`): populated from the recipe's `[source]`
  section -- `upstream_url` from archive URL, `upstream_hash` from checksum,
  `patches` from recipe patches if any, `fetch_timestamp` from build time.

- **Build layer** (`BuildProvenance`): `recipe_hash` from
  `build_script_hash`, `build_deps` populated from the dependency map
  (each dep's name + version + `DnaHash` from its provenance, or a
  placeholder if no provenance exists yet). `host_attestation` from
  `std::env::consts::ARCH` and kernel version. `build_start` / `build_end`
  timestamps. The `build_env` key-value map stores derivation-specific
  context: `build_env_hash` (the EROFS image hash), `target_triple`, and
  `derivation_id`. These use the existing `build_env: Vec<(String,
  String)>` field on `BuildProvenance` (pushed as key-value tuples)
  rather than adding new fields.

- **Signature layer** (`SignatureProvenance`): `builder_signature` with
  builder identity (hostname or configured name). No reviewer signatures
  initially (added by `verify-rebuild`). `sbom_ref` set with CycloneDX hash
  of the package component.

- **Content layer** (`ContentProvenance`): `merkle_root` set to
  `output_hash`, `total_size` summed from `OutputManifest.files[].size`,
  `file_count` from `manifest.files.len() + manifest.symlinks.len()`.

### Serialization and Storage

The provenance record is serialized to JSON via the existing `Provenance::
to_json()` method (not `CanonicalBytes`, which produces a binary format for
hashing only). The JSON bytes are stored as a CAS object. The CAS hash of
this JSON blob is recorded as `provenance_cas_hash` on the `DerivationRecord`.

The `CanonicalBytes` trait is used only to compute the `DnaHash` (package
DNA) recorded within the provenance record itself -- this is the existing
pattern in the provenance module.

**Substituter integration**: when a derivation cache hit includes a
provenance hash, it is fetched alongside the manifest and recorded locally.
The Remi `derivation_cache` table gains a `provenance_cas_hash` column.

### Files Modified

- `conary-core/src/derivation/executor.rs` -- provenance construction after
  build
- `conary-core/src/derivation/index.rs` -- add `provenance_cas_hash`,
  `trust_level`, `reproducible` fields to `DerivationRecord`
- `conary-core/src/db/migrations.rs` -- v56 migration
- `conary-core/src/db/schema.rs` -- bump SCHEMA_VERSION, add dispatch case

### Database Migration (v56)

```sql
ALTER TABLE derivation_index
    ADD COLUMN trust_level INTEGER NOT NULL DEFAULT 0;
ALTER TABLE derivation_index
    ADD COLUMN provenance_cas_hash TEXT;
ALTER TABLE derivation_index
    ADD COLUMN reproducible INTEGER;

ALTER TABLE derivation_cache
    ADD COLUMN provenance_cas_hash TEXT;
```

`trust_level`: 0-4 integer (see Section 2).
`provenance_cas_hash`: CAS hash of the JSON-serialized provenance record.
`reproducible`: NULL = unknown, 0 = non-reproducible, 1 = reproducible.

## 2. Trust Level Tracking

### Trust Levels

| Level | Name | When Assigned |
|-------|------|---------------|
| 0 | Unverified | Default for all new records |
| 1 | Substituted | Output fetched from remote cache |
| 2 | Locally built | Built from source on this machine |
| 3 | Independently verified | verify-rebuild confirms matching output |
| 4 | Diverse-verified | verify-diverse confirms match across seeds |

### Assignment Logic

- `Pipeline::execute()` sets level 2 on successful local build (via executor)
- `Pipeline::execute()` sets level 1 on substituter hit
- `conary verify rebuild` upgrades to level 3 if outputs match
- `conary verify diverse` upgrades to level 4 if outputs match across seeds
- Trust levels only increase, never decrease. A level 3 package remains
  level 3 even if a later rebuild produces different output (that is tracked
  by the separate `reproducible` flag).

### API

`DerivationIndex` gains two methods:

```rust
/// Upgrade trust level (monotonic — uses SQL MAX to prevent downgrades).
pub fn set_trust_level(&self, derivation_id: &str, level: u8) -> Result<()>
// SQL: UPDATE derivation_index SET trust_level = MAX(trust_level, ?2)
//      WHERE derivation_id = ?1

pub fn set_reproducible(&self, derivation_id: &str, reproducible: bool) -> Result<()>
```

The `set_trust_level` method enforces monotonicity in SQL via `MAX(trust_level,
?2)`, so callers do not need to read-then-write.

### No Policy Enforcement

Trust levels are informational in Phase 6 -- displayed by `verify chain` and
`cache status`. Policy enforcement ("refuse packages below trust level 2")
is Phase 7 scope.

## 3. Verification Commands

Three new subcommands under a `Verify` subcommand group.

### CLI Structure

```rust
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

### verify chain

**Input**: profile TOML path.

**Logic**:
1. Parse profile to get all derivation IDs per stage.
2. For each derivation, load `DerivationRecord` from the local index.
3. Walk the stage chain: each stage's `build_env` hash traces to the previous
   stage's composed EROFS image, which traces back to the seed ID.
4. Verify chain integrity: every stage's build env must be derivable from
   the previous stage's outputs.

**Chain status definitions**:
- **COMPLETE**: every derivation has a `DerivationRecord` in the local index
  and the stage chain traces back to the seed without gaps.
- **BROKEN**: a derivation is missing from the local index, OR a stage's
  build_env_hash does not match the previous stage's composed image hash.
- **Missing provenance** (`provenance_cas_hash` is NULL): the chain is NOT
  broken -- the derivation is in the index with a valid output hash, it just
  lacks provenance metadata. Reported as a warning in the output.
- **Trust level 0** (unverified): does NOT break the chain. Trust level is
  displayed per-package but is orthogonal to chain integrity.

**Output modes**:
- Default (summary): per-package one-liner with name, trust level, stage.
  Ends with chain status (COMPLETE/BROKEN) and trust level summary.
- `--verbose`: adds source URL, recipe hash, dependency IDs, provenance hash
  (loaded from CAS if available).
- `--json`: machine-readable JSON for CI/compliance pipelines.

**Exit code**: 0 if chain is complete, 1 if broken.

### verify rebuild

**Input**: derivation ID or package name (resolved to latest derivation ID
via `DerivationIndex::by_package()`).

**Recipe resolution**: the `DerivationRecord` stores `package_name` and
`package_version`. The recipe file is resolved by scanning the `recipes/`
directory for a TOML file whose `[package] name` matches. This is the same
convention used by `recipe-audit --all`. If the recipe is not found locally,
the command returns an error: "recipe for '{name}' not found in recipes/
directory."

**Cache bypass**: the rebuild uses a fresh in-memory SQLite database
(`Connection::open_in_memory()`) so the executor's internal cache check
finds no existing record and proceeds to build. The original derivation's
`build_env_hash`, `target_triple`, and dependency IDs are read from the
original `DerivationRecord` and passed to the executor.

**Logic**:
1. Load the original `DerivationRecord` to get expected `output_hash`,
   `package_name`, `build_env_hash`, and `stage`.
2. Resolve the recipe file from `recipes/` by package name.
3. Compute the dependency ID map from the original record's stage context
   (load all records for the same stage and earlier stages from the index).
4. Create a fresh in-memory DB and run the build via
   `DerivationExecutor::execute()` with the same inputs.
5. Compare the new `output_hash` against the original.
6. If match: set trust level to max(current, 3), set `reproducible = true`
   on the ORIGINAL record (not the rebuild's ephemeral record).
7. If mismatch: set `reproducible = false`. Compare the two
   `OutputManifest`s file-by-file to report which files differ.

**Output**: match/mismatch status, trust level change, list of differing
files if applicable.

### verify diverse

**Input**: two profile TOML paths (from builds with different seeds).

**Matching packages across profiles**: both profiles store `package` (name)
and `version` per derivation. The command matches packages by name+version
across the two profiles (not by `SourceDerivationId` -- computing that
would require recipe access and input reconstruction). If the same package
at the same version was built from different seeds, the output hashes are
directly comparable. If the `output_hash` matches despite different
`build_env_hash` chains, the build is seed-independent.

**Logic**:
1. Parse both profiles to get their derivation lists and seed IDs.
2. Verify the two profiles used different seeds (error if same seed_id).
3. Match packages by name+version across both profiles.
4. For each matched pair, load both `DerivationRecord`s from the local
   index.
5. Compare `output_hash` values.
6. If match: upgrade trust level to max(current, 4) on both records.
7. Report matches, mismatches, and unmatched packages.

**Note**: this is a simplified comparison (name+version match) rather
than the full `SourceDerivationId` match described in the v2 spec. The
full `SourceDerivationId` comparison requires reconstructing all
derivation inputs from recipes + dependency graphs, which is deferred
to a future enhancement. The name+version match is correct when both
profiles use the same recipe set (same git revision), which is the
expected use case.

**Output**: per-package match/mismatch, summary counts.

## 4. SBOM Integration

### What Exists

- CycloneDX 1.5 generator in `src/commands/query/sbom.rs` (`cmd_sbom()`)
- `SbomRef` in provenance signature layer with `cyclonedx_hash`
- `provenance export --format cyclonedx` command (exports from installed
  troves, not derivation data)

### What Phase 6 Adds

A new top-level `conary sbom` command that generates SBOMs from derivation
profiles. This is separate from the existing `provenance export` command,
which operates on installed trove provenance data. The two commands have
different data sources:

- `provenance export` -- from installed troves and their provenance records
- `conary sbom` -- from derivation profiles, the derivation index, and
  derivation provenance records

Both commands coexist. `provenance export` is unchanged.

```
conary sbom --profile pinned.toml
conary sbom --derivation a1b2c3
```

The implementation reuses the existing `Bom`/`Component` types from
`query/sbom.rs` but sources data from the derivation index + provenance
records instead of installed troves. For each derivation, the SBOM includes:

- Package name, version
- Source URL and checksum (from provenance source layer, if available)
- Build dependency graph (from derivation record's stage context)
- Derivation ID and output hash
- Trust level
- Seed lineage (from the profile's seed section)

**Automatic SBOM reference**: when the executor generates a provenance record
(Section 1), it computes the CycloneDX hash for that single package's
component and sets `SbomRef.cyclonedx_hash` on the signature layer. Every
built package has an SBOM reference in its provenance without a separate pass.

### CLI

New top-level command (alongside existing `Cook`, `RecipeAudit`, etc.):

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

Format is CycloneDX only (SPDX deferred to Phase 7).

## Summary

| Component | What | Files |
|-----------|------|-------|
| Provenance generation | Build provenance record in executor | `executor.rs`, `index.rs` |
| Trust levels | 0-4 tracking on DerivationRecord | `index.rs`, `pipeline.rs` |
| DB migration v56 | Add columns to derivation_index + derivation_cache | `migrations.rs`, `schema.rs` |
| verify chain | Trace profile to seed | new `src/cli/verify.rs`, `src/commands/verify.rs` |
| verify rebuild | Rebuild + compare hashes | `src/commands/verify.rs` |
| verify diverse | Cross-seed comparison | `src/commands/verify.rs` |
| SBOM | Profile-based CycloneDX generation | new `src/commands/derivation_sbom.rs` |

### Recommended Build Order

1. **DB migration v56** -- add trust_level, provenance_cas_hash, reproducible
2. **DerivationRecord + DerivationIndex updates** -- new fields and methods
3. **Provenance generation in executor** -- build provenance after successful
   build
4. **Trust level assignment in pipeline** -- set level 1/2 during execution
5. **verify chain** -- read-only, queries existing data
6. **verify rebuild** -- requires executor + fresh build + recipe resolution
7. **verify diverse** -- requires profile comparison + trust level updates
8. **SBOM command** -- reuses existing CycloneDX generator
