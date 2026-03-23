---
last_updated: 2026-03-22
revision: 1
summary: Wire the CAS-layered derivation pipeline to produce a bootable conaryOS image
---

# Bootstrap Image Pipeline Design

## Problem

The CAS-layered derivation pipeline (`conary-core/src/derivation/`) is fully
implemented but not connected to the CLI. The `bootstrap run` command is a stub.
We need to wire the pipeline end-to-end to produce the first conaryOS image --
a bootable qcow2 backed by content-addressed EROFS generations.

## Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Seed strategy | LFS Phase 1 cross-tools packaged as seed EROFS | Clean bootstrap-from-source story; cross-tools code already works |
| Output format | EROFS generation + qcow2 wrapper | Native artifact for architecture, qcow2 for demo/boot |
| Package scope | Full LFS 80 + Tier 2 8 = 88 packages | Complete self-hosting system |
| Build machine | Remi (12 cores, 64GB RAM, 1.7TB disk) | Fastest available hardware |
| Workflow | Four separate commands | Maximum control, each step independently verifiable |

## Workflow

```
Step 1: conary bootstrap cross-tools
        Input:  Host compiler (Remi's gcc)
        Output: /conary/bootstrap/lfs/tools/  (cross binutils, gcc, glibc, libstdc++)

Step 2: conary bootstrap seed
        Input:  Cross-tools directory
        Output: /conary/bootstrap/seed/
                  seed.erofs, seed.toml, cas/

Step 3: conary bootstrap run conaryos.toml
        Input:  System manifest + seed + recipes
        Output: /conary/bootstrap/output/
                  objects/, generations/1/root.erofs, db.sqlite3

Step 4: conary bootstrap image --from-generation
        Input:  Pipeline output (EROFS generation)
        Output: conaryos.qcow2  (bootable VM image)
```

All artifacts live under `/conary/bootstrap/` on Remi. Each step is independently
re-runnable.

## Components

### 1. New Command: `bootstrap seed`

**Files:** `src/cli/bootstrap.rs`, `src/commands/bootstrap/mod.rs`

**CLI:**

```
conary bootstrap seed
    --from <path>       Cross-tools directory to package
    --output <path>     Seed output directory
    --target <triple>   Target triple (default: x86_64-conary-linux-gnu)
```

**Implementation (`cmd_bootstrap_seed`):**

1. Validate `--from` directory exists and contains a toolchain (`bin/`, `lib/`)
2. Create `CasStore` at `<output>/cas/`
3. Walk the cross-tools directory, store every file in CAS (SHA-256 keyed)
4. Collect `FileEntryRef` and `SymlinkEntryRef` entries for all stored files
5. Build EROFS image via `build_erofs_image()` (composefs-rs, userspace)
6. Write EROFS to `<output>/seed.erofs`
7. Compute image hash via `erofs_image_hash()`
8. Write `seed.toml`:
   ```toml
   seed_id = "<sha256 of seed.erofs>"
   source = "selfbuilt"
   builder = "conary-bootstrap"
   packages = ["binutils-pass1", "gcc-pass1", "linux-headers", "glibc", "libstdcxx"]
   target_triple = "x86_64-conary-linux-gnu"
   verified_by = []
   ```
9. Print summary: file count, CAS object count, EROFS image size, seed_id

**Dependencies:** Uses existing `CasStore` (note: `CasStore::new()` returns
`Result` -- propagate errors), `build_erofs_image()`, `erofs_image_hash()`,
`FileEntryRef`, `SymlinkEntryRef`. No new library code.

**Estimated size:** ~100 lines.

### 2. Wire `cmd_bootstrap_run()`

**Files:** `src/commands/bootstrap/mod.rs`, `src/cli/bootstrap.rs`

**New CLI flags on `Run` variant:**

- `--seed <path>` (required) -- path to seed directory
- `--recipe-dir <path>` (default: `recipes/`) -- recipe search path

**Implementation replaces the stub:**

1. `SystemManifest::load(manifest_path)` -- parse the TOML manifest
2. `Seed::load_local(seed_path)` -- load and verify seed (hash check)
3. Walk all subdirectories of `recipe_dir/` (`cross-tools/`, `temp-tools/`,
   `system/`, `tier2/`), parse every `.toml` into `HashMap<String, Recipe>`.
   Filter to packages in `manifest.packages.include` plus their transitive
   `makedepends`/`requires`. Loading all recipes (including cross-tools and
   temp-tools) ensures `assign_stages()` can detect pass suffixes for correct
   Foundation classification, even though only manifest-included packages are built.
4. `assign_stages(&recipes, &custom_packages)` -- auto-classify into
   Toolchain/Foundation/System/Customization with topological sort per stage
5. Open/create SQLite DB at `work_dir/derivations.db`, run schema migrations
6. Create `CasStore` (reuse seed's CAS dir or create at `work_dir/output/objects/`)
7. Create `DerivationExecutor::new(cas, cas_dir, executor_config)`
8. Create `Pipeline::new(pipeline_config, executor)`
9. `pipeline.execute(seed, recipes, assignments, conn, on_event).await`
10. After completion: copy final composed EROFS and CAS to
    `work_dir/output/generations/1/` with metadata JSON. Also serialize the
    final `BuildProfile` and combined `OutputManifest` (merged from all stages)
    to `work_dir/output/generations/1/manifest.toml` -- this is needed by
    `bootstrap image --from-generation` to locate the kernel in CAS.
11. Print `BuildProfile` summary

**Event handler prints progress:**

```
[toolchain] Stage started (5 packages)
[toolchain] Building binutils-pass1... built in 120s
[toolchain] Stage completed
[foundation] Stage started (24 packages)
...
[COMPLETE] 88 packages built, 0 cached
```

**`PipelineConfig` construction from CLI flags:**

```rust
PipelineConfig {
    cas_dir: work_dir.join("output/objects"),
    work_dir: work_dir.join("pipeline"),
    target_triple: manifest.system.target.clone(),
    jobs: num_cpus::get(),
    log_dir: Some(work_dir.join("logs")),
    keep_logs: opts.keep_logs,
    shell_on_failure: opts.shell_on_failure,
    up_to_stage: opts.up_to.map(|s| Stage::from_str_name(&s)).transpose()?,
    only_packages: opts.only.clone(),
    cascade: opts.cascade,
    substituter_sources: if opts.no_substituters { vec![] } else { vec![] },
    publish_endpoint: None,
    publish_token: None,
}
```

**Estimated size:** ~150 lines.

### 3. Extend `bootstrap image` for Generation Input

**Files:** `conary-core/src/bootstrap/image.rs`, `src/commands/bootstrap/mod.rs`,
`src/cli/bootstrap.rs`

**New CLI flag:** `--from-generation <path>` on the `Image` variant, alternative to
the current implicit sysroot detection.

**New method in `ImageBuilder`:** `build_from_generation(generation_dir)`:

1. Read generation metadata from `generation_dir/generations/1/.conary-gen.json`
2. Locate the EROFS image at `generation_dir/generations/1/root.erofs`
3. Create GPT disk image with ESP (512MB FAT32) + root partition
4. Write the EROFS image directly to the root partition (the kernel boots EROFS
   natively)
5. Extract kernel from CAS for the ESP: the `linux` recipe installs `vmlinuz` to
   `/boot/vmlinuz`. Read the pipeline's final `OutputManifest` (serialized in CAS
   alongside the generation metadata), find the file entry for `/boot/vmlinuz`,
   retrieve the file bytes from CAS by its SHA-256 hash, and write to the ESP at
   `/EFI/conaryos/vmlinuz`. Note: `CasStore::retrieve(hash)` returns the raw bytes.
6. Install systemd-boot to ESP, write boot loader entry:
   ```
   title   conaryOS
   linux   /EFI/conaryos/vmlinuz
   options root=LABEL=conaryos rootfstype=erofs ro
   ```
7. Convert raw to qcow2 via `qemu-img` if format is qcow2
8. Return `ImageResult`

**Modification to `cmd_bootstrap_image`:** Check for `--from-generation` flag.
If present, call `build_from_generation()` instead of `build()`.

**Estimated size:** ~80 lines new code in `image.rs`, ~15 lines CLI changes.

### 4. System Manifest: `conaryos.toml`

**File:** `conaryos.toml` (project root)

```toml
[system]
name = "conaryos-base"
target = "x86_64-conary-linux-gnu"

[seed]
source = "local:seed"

[packages]
include = [
    # Core system (LFS Ch8 aligned)
    "man-pages", "iana-etc", "glibc", "zlib", "bzip2", "xz", "zstd",
    "lz4", "file", "readline", "m4", "bc", "flex", "pkgconf", "binutils",
    "gmp", "mpfr", "mpc", "attr", "acl", "libcap", "libxcrypt", "shadow",
    "gcc", "ncurses", "sed", "psmisc", "gettext", "bison", "grep", "bash",
    "libtool", "gdbm", "gperf", "expat", "inetutils", "less", "perl",
    "xml-parser", "intltool", "autoconf", "automake", "openssl", "kmod",
    "elfutils", "libffi", "python", "flit-core", "wheel", "setuptools",
    "ninja", "meson", "coreutils", "diffutils", "gawk", "findutils",
    "groff", "gzip", "iproute2", "kbd", "libpipeline", "make", "patch",
    "tar", "texinfo", "vim", "markupsafe", "jinja2", "systemd", "dbus",
    "man-db", "procps-ng", "util-linux", "e2fsprogs", "pcre2", "sqlite",
    "linux",
    # Tier 2: Self-hosting
    "linux-pam", "openssh", "make-ca", "curl", "sudo", "nano", "rust",
    "conary",
]
exclude = []

[kernel]
config = "defconfig"
```

88 packages total. Uses `linux-pam` (matching the recipe filename), `elfutils`
(provides libelf). Drops test-suite-only packages (tcl, expect, dejagnu, check).

## What Already Exists (No Changes)

- `Pipeline::execute()` -- full staged build loop with caching, substituters,
  EROFS composition between stages
- `DerivationExecutor::execute()` -- Kitchen builds, CAS capture, provenance,
  derivation index
- `assign_stages()` -- auto-classifies packages into 4 stages with topo sort
- `compose_erofs()` -- merges package outputs into composed EROFS images
- `Seed::load_local()` -- seed loading and hash verification
- `SystemManifest::load()` -- manifest parsing
- `CasStore` -- content-addressable storage
- `build_erofs_image()` -- EROFS builder via composefs-rs (userspace)
- `Kitchen` -- recipe execution engine
- `CrossToolsBuilder` -- Phase 1 cross-compilation (5 packages)
- `ImageBuilder` -- disk image creation (raw, qcow2, ISO)
- All 114 recipes across cross-tools/, temp-tools/, system/, tier2/

## Execution Plan (on Remi)

```bash
# Rsync source
rsync -az --delete --exclude target --exclude .git \
    ~/Conary/ root@ssh.conary.io:/root/conary-src/

# Build conary on Remi
ssh root@ssh.conary.io 'cd /root/conary-src && cargo build'

# Step 1: Cross-toolchain (~15-30 min)
./target/debug/conary bootstrap cross-tools \
    -w /conary/bootstrap --lfs-root /conary/bootstrap/lfs

# Step 2: Package seed (~1-2 min)
./target/debug/conary bootstrap seed \
    --from /conary/bootstrap/lfs/tools \
    --output /conary/bootstrap/seed

# Step 3: Derivation pipeline (several hours)
./target/debug/conary bootstrap run conaryos.toml \
    -w /conary/bootstrap \
    --seed /conary/bootstrap/seed \
    --recipe-dir recipes \
    --keep-logs \
    --shell-on-failure

# Step 4: Wrap qcow2 (~5 min)
./target/debug/conary bootstrap image \
    --from-generation /conary/bootstrap/output \
    -o /conary/bootstrap/conaryos.qcow2 \
    -f qcow2

# Verify
qemu-system-x86_64 \
    -drive file=/conary/bootstrap/conaryos.qcow2,format=qcow2 \
    -m 2G -enable-kvm -nographic
```

## Estimates

| Component | New/Modified Lines | Risk |
|-----------|--------------------|------|
| `bootstrap seed` command | ~100 | Low -- orchestration of existing APIs |
| `cmd_bootstrap_run()` wiring | ~150 | Medium -- connecting many pieces |
| `image.rs` generation support | ~80 | Medium -- new EROFS-to-disk path |
| `conaryos.toml` manifest | ~40 | Low -- declarative config |
| CLI definitions | ~30 | Low -- clap boilerplate |
| **Total** | **~400** | |

## Out of Scope

- Binary seed distribution (publishing seeds to packages.conary.io) -- future work
- Substituter integration (remote build caching) -- pipeline supports it, no endpoint configured
- Reproducibility verification (Phase 6 verify-chain/verify-rebuild) -- designed but not wired
- Multi-architecture (aarch64, riscv64) -- recipes and config support it, not tested
- The old LFS-style mutable sysroot pipeline -- untouched, can be deprecated later
