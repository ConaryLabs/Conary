# Bootstrap Build Environment Design

## Summary

Replace the multi-stage EROFS derivation pipeline with a mutable chroot pipeline for bootstrap builds. Support multiple seed sources (adopted distro, Phase 1+2 cross-tools, community pre-built). Add content-addressed output hashing to enable seed-independent convergence verification.

## Problem Statement

The derivation pipeline's current stage model (Toolchain/Foundation/System) creates EROFS images between stages. Packages within a stage can only see the previous stage's read-only EROFS, not outputs from sibling packages. This forces workarounds:

- GCC moved out of Foundation because it needs gmp/mpfr/mpc headers from the same stage
- `FOUNDATION_PACKAGES` hardcoded to manually control stage membership
- Python PGO disabled and -Werror stripped due to host/chroot header mismatches

The seed contains only Phase 1 output (5 packages) -- insufficient for chroot builds. Phase 1+2 together would provide a complete build environment, but the cross-compilation pipeline is complex and the intra-stage visibility problem remains.

## Design

### Core Execution Model

Replace multi-stage EROFS pipeline with a mutable chroot pipeline.

**Current model:**
```
Seed EROFS -> [Toolchain] -> compose EROFS1 -> [Foundation] -> compose EROFS2 -> [System] -> compose EROFS3
```

**New model:**
```
Seed EROFS -> mount as mutable overlayfs -> build pkg1 -> install to chroot -> build pkg2 -> install to chroot -> ... -> build pkg88 -> compose final EROFS from CAS
```

The chroot accumulates packages as they build. Each package sees everything built before it, like LFS Chapter 8. CAS capture happens before install, so the pipeline produces both a live chroot (working state) AND content-addressed records (source of truth).

**Key invariant:** The final EROFS generation is composed from CAS manifests, not from the chroot filesystem. The chroot can accumulate build artifacts without polluting the final image.

**Two-step mount for mutable sysroot:**

Seed EROFS images use composefs (metadata-only EROFS + CAS digest xattrs, no inline file data). The mutable chroot requires two mount layers:

1. Mount the seed EROFS via the existing composefs path (`mount_generation` in `generation/mount.rs`), producing a read-only mountpoint.
2. Stack a writable overlayfs on top:

```
mount -t overlay overlay \
  -o lowerdir=<composefs-mount>,upperdir=<work/upper>,workdir=<work/work> \
  <sysroot>
```

The seed stays pristine (read-only composefs lowerdir). All modifications go to upperdir. Multiple builds can share the same seed. Reset to seed state by wiping upperdir.

**Note on adopted-distro seeds:** Seeds created via `--from-adopted` contain inline file data (not composefs metadata-only) because they are built from a real filesystem, not from CAS objects. These can use a single-step overlayfs mount with the EROFS as lowerdir directly. The `BuildEnvironment` mount logic must handle both cases.

**Overlayfs upper directory persistence:** The upper directory is persisted across runs within the same seed. On resume after failure, the chroot state from previous packages is already present -- no need to reinstall from CAS. The upper directory is wiped only when the seed changes (detected by comparing seed_id).

**Per-package flow:**

1. Fetch + verify source archive (existing `PackageBuildRunner`)
2. Build inside chroot via `cook.rs` chroot execution (existing)
3. Capture DESTDIR to CAS with `DerivationId` + `OutputHash` (existing + enhanced)
4. Install DESTDIR files into live chroot sysroot (new -- see install algorithm below)
5. Record in derivation index (existing)

**Step 4: Chroot install algorithm (`derivation/install.rs`):**

Walk the `OutputManifest` from step 3 (already in memory). For each entry:

- **Files**: Copy from CAS object store to the sysroot path. Use hard links where possible (same filesystem), fall back to copy. Preserve permissions from the manifest.
- **Symlinks**: Create in sysroot with the target from the manifest.
- **Directories**: Create with appropriate permissions.
- **Post-install**: Run `ldconfig` in the chroot if any `.so` files were installed (required for the next package's configure step to find shared libraries via `ld.so.cache`).
- **Conflicts**: Last-writer-wins (same as `compose_erofs`). If two packages install the same path, the later one in build order takes precedence. Log a warning.

This reads from CAS (not from DESTDIR), so the install is idempotent -- safe to re-run on resume.

### Seed Abstraction

Any source that provides a working build environment is a valid seed. Three supported sources:

| Source | Command | Mechanism |
|--------|---------|-----------|
| Adopted distro | `conary bootstrap seed --from-adopted` | Packages current adopted system as EROFS |
| Phase 1+2 | `conary bootstrap seed` | Cross-tools + temp-tools output as EROFS |
| Community | `conary bootstrap seed --fetch <url>` | Downloads pre-built seed, verifies hash |

**Seed validation** uses probing, not package-name matching:

```rust
pub struct SeedValidation {
    pub has_c_compiler: bool,     // chroot $SEED gcc --version
    pub has_libc_headers: bool,   // chroot $SEED test -f /usr/include/stdio.h
    pub has_make: bool,           // chroot $SEED make --version
    pub has_shell: bool,          // chroot $SEED /bin/sh -c "echo ok"
    pub has_coreutils: bool,      // chroot $SEED ls --version
    pub has_binutils: bool,       // chroot $SEED ld --version
}
```

Probe-based validation means any distro works without mapping package names.

**The `--from-adopted` path:**

1. Reads the adopted system's filesystem (not the DB -- raw filesystem state)
2. Creates an EROFS image of the rootfs (/usr, /bin, /lib, /sbin, /etc)
3. Writes `seed.toml` with `source = "Adopted"`, distro name, package list
4. Result: a seed EROFS identical in shape to Phase 1+2 seeds

**Seed metadata (`seed.toml`):**

```toml
seed_id = "sha256:..."
source = "adopted"            # serde lowercase: "selfbuilt", "community", "imported", "adopted"
target_triple = "x86_64-unknown-linux-gnu"
origin_distro = "archlinux"   # new field: which distro was adopted (Adopted seeds only)
origin_version = "2026.03.01" # new field: distro release (Adopted seeds only)
packages = ["gcc", "glibc", "make", "bash", "coreutils", "..."]
builder = "conary-ci"
verified_by = ["sha256:...", "sha256:..."]
```

**New fields on `SeedMetadata` struct:**

- `origin_distro: Option<String>` -- populated for Adopted seeds, `None` for others
- `origin_version: Option<String>` -- populated for Adopted seeds, `None` for others

Existing fields (`origin_url`, `builder`, `packages`, `target_triple`, `verified_by`) remain unchanged. The `SeedSource` enum gains a new `Adopted` variant alongside `Community`, `Imported`, `SelfBuilt`. Serde serialization uses `#[serde(rename_all = "lowercase")]` (existing convention).

### Build Order

Replace stage-based grouping with a single topological sort. Stages become informational labels.

```rust
pub enum BuildPhase {
    Toolchain,      // gcc, glibc, binutils, linux-headers, libstdc++
    Foundation,     // make, bash, coreutils, sed, grep, etc.
    System,         // everything else
    Customization,  // user-specified
}

pub struct BuildStep {
    pub package: String,
    pub order: usize,           // Global topological position
    pub phase: BuildPhase,      // Informational label for progress reporting
}

pub fn compute_build_order(
    recipes: &HashMap<String, Recipe>,
    custom_packages: &HashSet<String>,
) -> Result<Vec<BuildStep>, BuildOrderError>
```

Phase assignment is categorization for UI:

- **Toolchain**: compilers/linkers/libc (small hardcoded set: gcc, glibc, binutils, linux-headers, libstdc++)
- **Foundation**: LFS essential tools (coreutils, make, bash, sed, grep, etc.)
- **System**: everything else from conaryos.toml
- **Customization**: user additions

The topological sort uses Kahn's algorithm with `BTreeMap`/`BTreeSet` for determinism (existing implementation). GCC naturally sorts after gmp/mpfr/mpc via `makedepends`.

Progress reporting uses the phase labels:
```
[Toolchain 2/5]   Building glibc...          [OK 4m32s]
[Foundation 3/20]  Building bash...           [OK 52s]
[System 14/63]     Building openssl...        [OK 1m08s]
```

### Content-Addressed Output Hashing

Extend the existing output hash mechanism to enable cross-seed convergence checking.

**Existing infrastructure:** `OutputManifest::compute_output_hash()` in `derivation/output.rs` already computes a deterministic SHA-256 from sorted `file:<path>:<hash>` and `symlink:<path>:<target>` lines. The `derivation_index` table already has an `output_hash TEXT NOT NULL` column (migration v54). The `DerivationRecord` struct already carries `output_hash`.

**What's new -- permissions in the output hash:**

The existing hash format does not include file permissions. Two builds producing the same file content but different permissions (e.g., 0755 vs 0644 on a binary) would get the same output hash. For convergence verification, permissions matter.

Update `compute_output_hash()` to include permissions:

```rust
// Current format:
//   file:<path>:<content_hash>
// New format (v2):
//   file:<path>:<permissions>:<content_hash>
```

This is a breaking change to the hash format. To handle the transition:
- Add a `hash_version: u8` field to `OutputManifest` (1 = current, 2 = with permissions).
- The `output_equivalence` table only compares hashes of the same version.
- Existing derivation index entries remain valid -- they just won't match v2 hashes (no false positives, only missed cutoff opportunities).
- No migration needed for existing records.

**Two-tier cache lookup:**

1. Compute `DerivationId` from inputs
2. Check derivation index for exact `DerivationId` hit (existing fast path)
3. If miss: build the package, compute `OutputHash` (v2 with permissions)
4. Check `output_equivalence` table: does any prior build of this package (from any seed) have the same `OutputHash`? If yes, downstream packages can reuse cached builds even though the `DerivationId` differs.

Step 4 is the early cutoff. Optional -- the pipeline works without it.

**New DB table (migration v57):**

```sql
CREATE TABLE output_equivalence (
    package_name TEXT NOT NULL,
    output_hash TEXT NOT NULL,
    derivation_id TEXT NOT NULL,
    seed_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (package_name, output_hash, seed_id)
);
```

Note: The `derivation_index.output_hash` column already exists (v54). No ALTER TABLE needed.

### Convergence Verification

New subcommand to verify seed independence.

```
conary bootstrap verify-convergence --seed-a ./arch-seed/ --seed-b ./phase12-seed/
```

Compares `OutputHash` for each package across two seeds. Reports:

```
Convergence Report: arch-seed vs phase12-seed

[MATCH]     linux-headers    abc123...
[MATCH]     glibc            def456...
[MISMATCH]  python           a1b2c3... vs d4e5f6...
            Cause: __DATE__ macro in _sysconfigdata.py

Result: 87/88 converged (1 mismatch)
```

**Convergence levels:**

| Level | Threshold | Trust implication |
|-------|-----------|-------------------|
| Full | 100% | Seed is completely irrelevant -- bootstrap proven sound |
| High | >95% | Few packages have embedded timestamps/paths -- cosmetic |
| Partial | <95% | Seed influences outputs -- investigate toolchain leakage |

**`--diff` flag:** Drills into mismatches by comparing OutputManifests file-by-file:

```
[MISMATCH]  python           a1b2c3... vs d4e5f6...
  Files differing: 3
    /usr/lib/python3.14/_sysconfigdata.py   content differs (embedded __DATE__)
    /usr/lib/python3.14/config-3.14/Makefile  content differs (absolute paths)
    /usr/bin/python3.14                       permissions differ (0755 vs 0755, content same)
  Files only in seed-a: 0
  Files only in seed-b: 0
```

**Seed diffing:**

```
conary bootstrap diff-seeds ./arch-seed/ ./phase12-seed/
```

Mounts both seed EROFS images, diffs file trees. Answers "what's different about these build environments?" before running the pipeline. Useful for diagnosing why outputs diverge.

**Convergence patching:** When only a few packages diverge, rebuild just those:

```
conary bootstrap run --seed ./arch-seed/ --only python --cascade
```

Rebuilds Python and its reverse-deps, then re-checks convergence. The `--cascade` flag (existing pipeline feature) rebuilds everything downstream of the named package.

### Bootstrap Flow

**Path A: Distro-bootstrapped (fast, pragmatic)**
```bash
conary system init
conary adopt --system
conary bootstrap seed --from-adopted
conary bootstrap run --seed ./seed/
conary bootstrap image --from-generation
```

**Path B: Self-contained (verification, audit)**
```bash
conary bootstrap cross-tools
conary bootstrap temp-tools
conary bootstrap seed
conary bootstrap run --seed ./seed/
conary bootstrap image --from-generation
```

**Path C: Convergence verification**
```bash
conary bootstrap verify-convergence \
    --seed-a ./arch-seed/ \
    --seed-b ./phase12-seed/
```

All three paths share `bootstrap run`. The pipeline does not know or care where the seed came from.

**Resume on failure:** If package N fails, fix the recipe, re-run. Packages 1..N-1 get `DerivationId` cache hits and skip building. Because the overlayfs upper directory is persisted across runs (see Core Execution Model), the chroot already contains the installed outputs from packages 1..N-1 -- no reinstall step needed. The pipeline detects the existing chroot state via the upper directory and resumes from package N.

If the seed changes (different `seed_id`), the upper directory is wiped and the pipeline starts fresh.

### Python/GCC -Werror Resolution

With a proper seed (adopted distro or Phase 1+2), the chroot has consistent glibc headers matching its GCC. The -Werror issue was caused by Python's configure detecting mismatches between host headers and cross-compiled libraries. In a consistent chroot, this does not occur. The PGO disable and Makefile sed patches can be reverted.

## Codebase Impact

### Modified Files

| File | Change |
|------|--------|
| `derivation/pipeline.rs` | Add `BuildMode::Chroot` with install-between-builds loop |
| `derivation/stages.rs` | Refactor to `derivation/build_order.rs` -- keep `topological_sort()` (Kahn's algorithm), replace `assign_stages()` with `compute_build_order()` returning flat `Vec<BuildStep>`. Rename `Stage` enum to `BuildPhase` (informational only). Existing `StageAssignment` maps directly to `BuildStep` (`stage`->`phase`, `build_order`->`order`). |
| `derivation/output.rs` | Update `compute_output_hash()` to v2 format (add permissions). Add `hash_version` field to `OutputManifest`. |
| `derivation/seed.rs` | Add `SeedSource::Adopted` variant. Add `origin_distro: Option<String>` and `origin_version: Option<String>` to `SeedMetadata`. Add probe-based `SeedValidation`. |
| `derivation/environment.rs` | Add mutable overlayfs mount mode (two-step: composefs then overlay; or single-step for adopted seeds). Handle upper dir persistence. |
| `bootstrap/mod.rs` | Wire `seed --from-adopted` subcommand |
| `db/schema.rs` | Migration v57: `output_equivalence` table only (`output_hash` column already exists in v54) |

### New Files

| File | Purpose |
|------|---------|
| `derivation/convergence.rs` | `verify-convergence` comparison, `diff-seeds`, `--diff` reporting |
| `derivation/install.rs` | Install CAS manifest entries into live chroot sysroot + ldconfig |
| `bootstrap/adopt_seed.rs` | Create seed EROFS from adopted system filesystem |

### Integration Point

The 88-package build set comes from `conaryos.toml` via the existing `SystemManifest` in `derivation/manifest.rs`. `compute_build_order()` receives recipes loaded from this manifest, same as `assign_stages()` does today.

### Removed / Simplified

| What | Why |
|------|-----|
| `FOUNDATION_PACKAGES` constant | Topological sort handles ordering |
| Per-stage EROFS composition loop | Only composed once at the end |
| GCC-out-of-Foundation workaround | makedepends handles ordering |
| Python PGO disable (commit `cab904b`) | Consistent chroot fixes header mismatch |
| Python -Werror strip (commit `7bbec44`) | Same reason |

### Unchanged

| What | Why |
|------|-----|
| `DerivationId` / `DerivationInputs` | Core CAS tracking, primary cache key |
| `DerivationExecutor` | Per-package build + capture |
| `cook.rs` chroot execution | Already handles sysroot builds |
| `compose_erofs()` | Used once at the end |
| `ChrootEnv` | Still needed for Phase 1+2 seed path |
| All bootstrap Phase 1-6 code | Phase 1+2 becomes one of N seed producers |
| `SourceDerivationId` | Cross-seed derivation comparison |
| `PipelineEvent` enum | Progress reporting unchanged |

## Research Context

This design was informed by analysis of:

- **Nix stdenv bootstrap**: 7-stage pipeline, minimum 4 stages needed (seed, rebuild compiler, rebuild libc, rebuild everything). Stage boundaries defined by what changes, not what's built. Within stages, packages see each other freely.
- **Wolfi OS**: Bootstrapped from Alpine in 3 stages, then verified zero Alpine references. Proven model for distro-bootstrapped builds.
- **Chimera Linux**: All-LLVM/musl distribution, bootstraps from an existing system with clang+lld+libc++. Unprivileged namespace sandboxing.
- **Nix CA derivations**: Output-addressed early cutoff -- if output unchanged, skip downstream rebuilds regardless of input changes. Experimental in Nix, not used for bootstrap verification.
- **Zig bootstrap**: WASM as architecture-neutral seed format. Single blob bootstraps on any architecture via minimal wasm2c converter.
- **Attestable Builds (CCS 2025)**: TEE-based verified compilation with 14% overhead, no build system modifications required.

The novel contribution is using output-hash convergence across seeds as the bootstrap correctness mechanism -- proving the seed doesn't matter rather than proving the seed is trustworthy.
