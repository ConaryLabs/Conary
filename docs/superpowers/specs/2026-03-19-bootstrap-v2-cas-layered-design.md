---
last_updated: 2026-03-19
revision: 3
summary: Full design spec for CAS-layered immutable bootstrap with derivation engine, build profiles, community sharing, and supply chain verification
---

# Bootstrap v2: CAS-Layered Immutable Build System

## Overview

A complete system-from-source build pipeline where a declarative TOML manifest produces a bootable, verified Linux system. The user says **what** they want; Conary figures out **how** to build it.

The system builds on five primitives in the codebase: CAS (content-addressable storage), EROFS image building, composefs mounting, namespace-isolated sandboxes, and the Kitchen recipe executor. These primitives provide the storage, imaging, and execution foundations. This design adds a new derivation engine and build orchestrator on top, composing these primitives into a layered EROFS build pipeline with automatic stage detection, community cache sharing, and full supply chain verification. The derivation engine, profile system, substituter protocol, and verification framework are all **new code** — the existing primitives are building blocks, not a complete solution.

**Target users:** Platform engineers who want to build Linux systems from source without learning Nix. Security teams who need auditable supply chains. Corporate teams who need reproducible, air-gapped system builds.

**Design date:** 2026-03-19

## 1. Core Concepts & Data Model

Five foundational types that everything else builds on.

### 1.1 Derivation

A derivation is a pure build specification — the complete description of how to build one package. Its identity is the hash of all its inputs:

```
derivation_id = hash(
    source_hash,          # SHA-256 of source tarball
    build_script_hash,    # SHA-256 of the recipe's build instructions
    dependency_ids[],     # derivation IDs of all build-time deps
    build_env_hash,       # hash of the EROFS image used as build environment
    target_triple,        # e.g. x86_64-conary-linux-gnu
    build_options{},      # compiler flags, features, etc.
)
```

The `build_env_hash` is the SHA-256 of the EROFS image the build runs against. For toolchain-stage packages, this is the seed's EROFS image hash. For foundation-stage packages, it's the toolchain stage's EROFS image hash. This means: if two different seeds produce identical toolchain EROFS images, the foundation-stage derivation IDs will be identical — enabling meaningful cache hits across seed lineages that converge to the same toolchain. The original seed is tracked in provenance metadata (Section 5.1), not in the derivation ID.

If two people compute the same derivation ID, they're asking for the exact same build. This is the cache key AND the reproducibility claim. After building, the actual output is content-addressed — you can verify that the same derivation produces the same CAS objects.

**Precise computation:** The derivation ID is a SHA-256 hash of a canonical byte string. Inputs are serialized in a deterministic order:

```
CONARY-DERIVATION-V1\n
source:<source_sha256>\n
script:<build_script_sha256>\n
dep:<dep_name_1>:<dep_derivation_id_1>\n
dep:<dep_name_2>:<dep_derivation_id_2>\n
...  (deps sorted lexicographically by name)
env:<build_env_erofs_sha256>\n
target:<target_triple>\n
opt:<key_1>:<value_1>\n
opt:<key_2>:<value_2>\n
...  (options sorted lexicographically by key)
```

The version prefix (`CONARY-DERIVATION-V1`) allows future format changes without hash collisions. The `build_script_hash` covers all recipe sections that affect the build: configure, make, install, and (if present) check, environment, workdir, and script_file — concatenated in deterministic order and hashed as a unit. The `[variables]` section is expanded before hashing, so different variable values produce different script hashes.

**Source derivation ID:** For cross-seed verification (Trust Level 4), a second hash is computed that excludes `build_env_hash`:

```
source_derivation_id = hash(
    same inputs as derivation_id, minus the env: line
)
```

The source derivation ID answers: "what would you build, regardless of build environment?" Two builds from different seeds (and therefore different build environments) with the same source derivation ID are comparable — if their output hashes match, the build is environment-independent (Thompson-attack resistant). The derivation ID (with `build_env_hash`) is the primary cache key; the source derivation ID is used only for cross-seed verification.

### 1.2 Package Output

A derivation's result: a set of files in CAS plus a metadata manifest:

```toml
# Stored as a CAS object itself
derivation_id = "a1b2c3..."
output_hash = "..."          # hash(sorted file hashes + symlink targets)
files = [
    { path = "usr/bin/gcc", hash = "d4e5f6...", size = 2841600, mode = 0o755 },
    { path = "usr/lib/libgcc_s.so.1", hash = "f7a8b9...", size = 141312, mode = 0o755 },
]
symlinks = [
    { path = "usr/lib/libgcc_s.so", target = "libgcc_s.so.1" },
]
build_duration_secs = 342
built_at = "2026-03-19T14:30:00Z"
```

The output manifest is itself content-addressed. The `output_hash = hash(sorted file hashes + symlink targets)`. This lets you verify: "this derivation was supposed to produce output X — did it?"

### 1.3 Seed

Layer 0. Architecturally identical to any other package output, but with import provenance:

```toml
seed_id = "abc123..."        # hash of the EROFS image
source = "community"          # or "imported", "self-built"
origin_url = "https://seeds.conary.io/x86_64/2026Q1"
origin_hash = "abc123..."     # matches seed_id
builder = "conary 0.9.0"      # or "manual", "guix", etc.
packages = ["gcc-15.2.0", "glibc-2.43", "binutils-2.46"]
target_triple = "x86_64-conary-linux-gnu"
verified_by = []               # list of independent rebuild verifiers
```

A seed IS a package output (EROFS image + CAS objects). The only difference is provenance metadata — "imported from X" vs. "built by derivation Y." This means:

- Corporate teams bring their own seed (their blessed toolchain)
- Open source users use the community seed
- Conary can eventually build its own seed from minimal binaries
- All without any architectural change

Community members can build, share, and independently verify seeds. Trust comes from verification count, not authority.

### 1.4 Build Profile

The auto-generated (or hand-edited) build plan — the Cargo.lock analog:

```toml
[profile]
manifest = "my-system.toml"
profile_hash = "deadbeef..."
generated_at = "2026-03-19T15:00:00Z"
target = "x86_64-conary-linux-gnu"

[seed]
id = "abc123..."
source = "https://seeds.conary.io/x86_64/2026Q1"

[[stage]]
name = "toolchain"
build_env = "seed"
derivations = ["d1", "d2", "d3", ...]

[[stage]]
name = "foundation"
build_env = "toolchain"
derivations = ["d4", "d5", "d6", ...]

[[stage]]
name = "system"
build_env = "foundation"
derivations = ["d7", "d8", ...]

[[stage]]
name = "customization"
build_env = "system"
derivations = ["d9", "d10", ...]
```

The profile pins exactly what gets built, in what order, against what. Sharing a profile + seed = guaranteed reproducible build. Auto-generated by default from the manifest, but inspectable, editable, and pinnable.

### 1.5 System Manifest

The user-facing declaration — the Cargo.toml analog:

```toml
[system]
name = "my-server"
target = "x86_64-conary-linux-gnu"

[seed]
source = "community"   # or a URL, or "local:/path/to/seed.erofs"

[packages]
include = ["base-system", "openssh", "curl", "nginx"]
exclude = ["nano"]

[kernel]
config = "server"      # preset, or path to a .config

[customization]
layers = ["./my-company-layer"]   # additional recipe directories
```

The manifest is deliberately small. The user declares what they want. Conary resolves the full dependency graph, computes all derivations, and generates the profile.

## 2. Build Pipeline

How `conary bootstrap my-system.toml` turns a manifest into a bootable system.

### 2.1 Pipeline Overview

```
Manifest → Resolve → Stage Assignment → Profile Generation → Execute → Image
```

### 2.2 Resolve

Conary parses the manifest and computes the full picture:

- Parse package list, resolve dependencies using the SAT resolver
- Fetch seed metadata (verify hash, check CAS for cached seed)
- Compute the transitive build-dependency closure — every package needed to build every package
- Compute derivation IDs for every package (inputs are known, so hashes are computable before any building happens)

### 2.3 Stage Assignment

Conary analyzes the dependency graph and assigns packages to stages automatically.

**Toolchain detection:** Identify the "self-hosting set" — the minimal set of packages that can build themselves. This is gcc, glibc, binutils, plus their transitive build-deps (make, coreutils, bash, sed, etc.). Conary knows this from recipe metadata — recipes declare their build dependencies, and the self-hosting set is the strongly-connected component containing the compiler.

**Automatic stage boundaries:**

| Stage | Build Environment | What Gets Built | Why |
|-------|------------------|-----------------|-----|
| **toolchain** | Seed (Layer 0) | Cross-compile the self-hosting set | Need a native compiler that doesn't depend on the seed |
| **foundation** | Toolchain output | Rebuild the self-hosting set natively | The compiler is now built by itself — no seed taint |
| **system** | Foundation output | Everything else in the manifest | Full system, built with a pure toolchain |
| **customization** | System output | User's custom layers, proprietary packages | Corporate additions on top of a standard base |

The **foundation** stage is the "purity step" — after it, every binary in the system was built by a compiler that was itself built from source. The seed's influence is washed out. This is what makes the reproducibility claim credible and what differentiates "built from source" from "cross-compiled from a binary blob."

Recipes can declare `stage = "toolchain"` to override automatic assignment.

**Worked example — resolving the gcc/glibc bootstrap cycle:**

The compiler toolchain has circular build-dependencies: gcc needs glibc to compile, glibc needs gcc to compile. This is the fundamental bootstrap chicken-and-egg problem. The stage assignment algorithm resolves it using multi-pass recipes (which already exist in the recipe set):

```
Input dependency graph (simplified):
  gcc-pass1      makedepends: [binutils-pass1]        (cross-compile, minimal)
  glibc          makedepends: [gcc-pass1, linux-headers]
  libstdc++      makedepends: [gcc-pass1, glibc]
  gcc-pass2      makedepends: [glibc, libstdc++, binutils-pass2, ...]
  binutils-pass1 makedepends: []                       (cross-compile)
  binutils-pass2 makedepends: [glibc, ...]

Stage assignment result:
  TOOLCHAIN (builds against seed):
    1. binutils-pass1    (no deps within stage)
    2. gcc-pass1         (depends on binutils-pass1)
    3. linux-headers     (no deps within stage)
    4. glibc             (depends on gcc-pass1, linux-headers)
    5. libstdc++         (depends on gcc-pass1, glibc)
    [compose toolchain EROFS from all 5 outputs]

  FOUNDATION (builds against toolchain EROFS):
    1. binutils          (full native rebuild)
    2. gcc               (full native rebuild, depends on binutils, glibc, libstdc++)
    3. glibc             (native rebuild with native gcc)
    ... + make, bash, coreutils, sed, etc.
    [compose foundation EROFS]

  SYSTEM (builds against foundation EROFS):
    ... remaining packages in topological order
```

The key: multi-pass recipes (`gcc-pass1` vs `gcc`) break the circular dependency. `gcc-pass1` is a minimal cross-compiler that can compile glibc. Once glibc exists, `gcc` (full) can be built natively. The stage assignment algorithm detects multi-pass recipes by name convention (`*-pass1`, `*-pass2`) and assigns them to the toolchain stage. Single-name recipes (just `gcc`) are assigned to foundation or system stage based on whether they're in the self-hosting set.

### 2.4 Profile Generation

Conary writes out the build profile with every derivation, ordered within each stage by dependency graph (topological sort). The profile is deterministic — same manifest + same recipes + same seed = same profile hash.

```bash
conary profile generate my-system.toml        # generate without building
conary profile show my-system.toml            # display the plan
conary profile diff old.toml new.toml         # what changed between two builds
```

### 2.5 Execute

For each stage, in order:

```
1. Compose build environment
   - Gather package outputs from previous stage(s)
   - Build single EROFS image from their CAS objects
   - Mount via composefs as read-only sysroot

2. For each derivation in the stage:
   a. Check CAS for cached output (derivation_id → output manifest)
      - Cache HIT: skip, reuse existing CAS objects
      - Cache MISS: continue to build
   b. Fetch source tarball (verify hash)
   c. Create mount namespace:
      - Mount sysroot (composefs, read-only)
      - Mount source (read-only)
      - Mount build dir (read-write, tmpfs)
      - Mount output dir (read-write)
      - No network, no host filesystem access
   d. Execute recipe phases: setup → configure → make → install
   e. Walk output dir, ingest every file into CAS
   f. Write package output manifest (file list + hashes)
   g. Record derivation_id → output_hash mapping

3. Compose stage EROFS image from all package outputs
   (This becomes the build environment for the next stage)
```

**Parallelism:** Within a stage, packages with no interdependencies build concurrently. The dependency graph gives maximum parallelism. The EROFS image is only recomposed when a package that other packages depend on completes.

**Failure handling:** A failed build produces nothing. The CAS is unchanged, the EROFS image is unchanged. Fix the recipe, re-run — Conary skips everything that already succeeded (derivation cache hits) and retries only the failure.

### 2.6 Final Image

After all stages complete:

- Compose final EROFS image from all package outputs across all stages
- Write generation metadata (`.conary-gen.json`)
- Optionally build a bootable disk image (raw/qcow2/iso)
- Output: a generation that can be mounted, booted, or distributed

```bash
conary bootstrap my-system.toml                         # build everything
conary bootstrap my-system.toml --profile pinned.toml   # use a pinned profile
conary bootstrap my-system.toml --up-to toolchain       # build through toolchain only
conary bootstrap my-system.toml --only nginx            # rebuild one package + dependents
```

### 2.7 Edge Cases

**Circular dependencies in the self-hosting set:** gcc needs glibc to compile, glibc needs gcc to compile. This is the fundamental bootstrap chicken-and-egg. Resolution: the toolchain stage uses the *seed's* gcc to cross-compile glibc, then uses the seed's gcc + new glibc to cross-compile gcc. The foundation stage then rebuilds both natively — gcc with the new glibc, glibc with the new gcc. The cycle is broken by the seed providing the initial compiler, and the foundation stage washing out the seed's influence. Each stage's EROFS image is composed only from that stage's completed outputs plus the previous stage's image.

**Packages requiring network during build:** Some packages fetch dependencies at build time (Rust crates, Go modules, Python packages). The build sandbox blocks network access by default. Resolution: recipes declare `[build] network = "fetch"` to enable a two-phase build — a fetch phase (network allowed, output is a vendored source tree captured to CAS) followed by a build phase (network blocked, builds from vendored sources). The fetch output is part of the derivation input, so the build is still reproducible.

**Multi-output packages:** Some packages produce multiple logical outputs (e.g., gcc produces `gcc`, `libgcc`, `libstdc++`). Resolution: a derivation can declare multiple named outputs in its package output manifest. Each output has its own file list and output hash. Dependents reference specific outputs: `dep:gcc:lib` vs `dep:gcc:bin`. The derivation ID is still singular — it's one build, but the outputs are separable for composition.

**Build environment vs. existing sandbox:** The existing `ContainerConfig` presets (e.g., `pristine_for_bootstrap`) bind-mount host paths like `/usr/bin` and `/lib64` into the sandbox. The CAS-layered build model does NOT use these presets. Instead, the build environment is exclusively composed from composefs-mounted EROFS images containing only declared dependencies. The existing `Sandbox` execution mechanism (namespace unsharing, bind mounts, process isolation) is reused, but the mount configuration is new — driven by the derivation engine, not by `ContainerConfig` presets. No host filesystem paths leak into the build environment.

## 3. Sharing & Distribution

How seeds, profiles, and build outputs flow between people.

### 3.1 Artifacts

| Artifact | Size | Use Case |
|----------|------|----------|
| **Seed** | ~200MB | Trust anchor — "start here" |
| **Profile** | ~50KB | Build plan — "build it exactly this way" |
| **Package output** | Varies | Individual cached build — "I already built gcc for you" |
| **Stage image** | ~500MB-2GB | Complete stage — "here's a full toolchain" |
| **System image** | ~2-5GB | Bootable result — "here's the finished system" |

### 3.2 Substituters (Cache Sharing)

A substituter is a remote service that serves pre-built derivation outputs. This is a **new protocol** — the existing `SubstituterChain` in `conary-core/src/repository/substituter.rs` resolves content chunks for runtime package operations (install/update), not derivation outputs. The derivation substituter serves the build system and queries by derivation ID: "do you have output for derivation `a1b2c3`?" If yes, download the CAS objects instead of building. The derivation ID is the trust boundary — if the derivation ID matches, the output is interchangeable. The two substituter systems (runtime chunk-based, build derivation-based) coexist, serving different use cases.

```toml
# In system manifest or global config
[substituters]
sources = [
    "https://cache.conary.io",         # community cache
    "https://builds.mycompany.com",    # corporate internal cache
]
trust = "derivation"    # accept if derivation_id matches
# or: trust = "verified"   # only accept if independently rebuild-verified
```

**First build experience:**

- Without substituters: builds everything from source. Hours.
- With substituters: Conary generates the profile, checks the cache for each derivation, downloads what's available, builds only what's missing. Minutes.

The key: you get the auditability of building from source (every derivation ID is computed locally from your inputs) with the speed of binary packages (pre-built outputs are fetched, not compiled). You can verify any substituted package by rebuilding it yourself and comparing hashes.

### 3.3 Seed Registry

Seeds are published to a registry (Remi serves this naturally):

```
https://seeds.conary.io/
  x86_64/
    2026Q1/
      seed.erofs
      seed.toml         # provenance metadata
      objects/           # CAS objects
    2026Q2/
      ...
  aarch64/
    ...
```

**Community seed lifecycle:**

1. Someone builds a seed (from an older seed, or from a trusted external toolchain)
2. Publishes with provenance metadata: source, builder version, package versions
3. Independent verifiers rebuild from the same inputs, confirm hashes match
4. Seed accumulates verification signatures: "3 independent rebuilds confirmed"
5. Becomes a "community verified" seed — the default for new users

Anyone can publish a seed. Trust comes from verification count, not authority.

### 3.4 Corporate Model

A company runs their own Remi instance and operates a closed loop:

```
Corporate seed (blessed by security team)
  → Corporate profile (pinned, version-controlled)
    → Corporate Remi (internal substituter/cache)
      → Every team builds from the same pinned profile
      → Internal cache means builds take minutes
      → No external dependencies at build time
```

This gives them:

- **Supply chain control:** They choose the seed, they audit the profile
- **Reproducibility:** Every developer gets identical builds
- **Air-gapped capability:** Once the cache is populated, no internet needed
- **Audit trail:** Every file traces back to the corporate seed

### 3.5 Profile Sharing

Profiles are small and portable:

```bash
# Publish a build plan
conary profile publish server-minimal.toml --to https://profiles.conary.io

# Build from someone else's plan
conary bootstrap --profile https://profiles.conary.io/server-minimal.toml

# Compare plans
conary profile diff my-system.toml server-minimal.toml
```

## 4. Recipe Integration

How the existing 114 TOML recipes work in this system.

### 4.1 Design Principle

Recipes already describe how to build a package. The derivation engine wraps around them. The Kitchen already knows how to cook a recipe in a sandbox. What changes is orchestration, not recipe format.

### 4.2 Current Recipe Format (unchanged core)

```toml
# recipes/system/zlib.toml
[package]
name = "zlib"
version = "1.3.2"
release = "1"
summary = "Compression and decompression library"
license = "Zlib"

[source]
archive = "https://zlib.net/fossils/zlib-%(version)s.tar.gz"
checksum = "md5:a1e6c958597af3c67d162995a342138a"

[build]
requires = ["glibc"]
makedepends = []

configure = """
./configure --prefix=/usr
"""

make = """
make -j%(jobs)s
"""

install = """
make install
rm -fv /usr/lib/libz.a
"""

[variables]
jobs = "$(nproc)"
```

Build phases (configure, make, install) live directly in the `[build]` section. Sources use `archive`/`checksum` fields. Variables use `%(name)s` substitution syntax.

### 4.3 New Derivation-Relevant Metadata

Three optional additions. All have sensible defaults — most recipes need zero changes.

**Explicit build dependencies (already exists, now enforced):**

```toml
[build]
requires = ["glibc"]
makedepends = ["gcc", "pkg-config"]
```

`requires` declares runtime deps. `makedepends` declares build-time-only deps. In the CAS-layered system, if a dependency is not in `requires` or `makedepends`, it does not exist in the build environment. This forces recipes to be honest about their deps — honest recipes are reproducible recipes.

**Stage hint (optional, new):**

```toml
[build]
stage = "toolchain"   # optional: toolchain, foundation, system, customization
```

Most recipes omit this — Conary assigns stages automatically. The hint is for edge cases.

**Cross-compilation (already exists in cross-tools recipes):**

Cross-tools recipes already declare their cross-compilation setup inline via `configure` flags referencing `$LFS_TGT` and `$LFS`. The derivation engine recognizes recipes in the `cross-tools/` directory as toolchain-stage, and their `[variables]` section carries `lfs_tgt` (target triple) and build environment configuration. No new `[cross]` section is needed — the existing format works.

### 4.4 Kitchen Integration

The Kitchen does the heavy lifting. The execution flow change:

```
DerivationEngine:
  1. Compute derivation_id from recipe + deps
  2. Check CAS cache → hit? done
  3. Compose build environment EROFS from makedepends
  4. Mount build env via composefs
  5. Kitchen.cook(recipe, output_dir) → build in sandbox against the mount
  6. Walk output_dir → ingest files into CAS
  7. Write package output manifest
  8. Record derivation_id → output_hash
```

The Kitchen doesn't know or care that it's building inside a CAS-layered system. It gets a sysroot, a recipe, and an output directory.

### 4.5 Custom / Corporate Recipes

The manifest's `[customization] layers` field points to additional recipe directories:

```toml
[customization]
layers = ["./recipes-internal"]
```

Custom recipes follow the same format. They build in the `customization` stage by default unless they declare otherwise. They produce CAS objects, get derivation IDs, and are cached and verifiable like everything else.

### 4.6 Recipe Discovery

Conary finds recipes in this order:

1. Built-in recipes (`recipes/` — the 114 LFS-aligned recipes)
2. Manifest-declared layers (`[customization] layers`)
3. Remi repositories (future extension: recipes published and fetched remotely)

Conflicts (two recipes for the same package) are resolved by precedence: custom layers override built-in recipes. A company can fork a recipe without forking Conary.

## 5. Verification & Audit Model

The security story — what makes "built from source" provable.

### 5.1 Provenance Records

Every derivation produces a provenance record alongside its package output:

```toml
[provenance]
derivation_id = "a1b2c3..."
output_hash = "d4e5f6..."
package = "gcc-15.2.0"
stage = "foundation"

[provenance.inputs]
source_url = "https://ftp.gnu.org/gnu/gcc/gcc-15.2.0/gcc-15.2.0.tar.xz"
source_hash = "789abc..."
recipe_hash = "def012..."
seed_id = "abc123..."

[provenance.inputs.dependencies]
glibc = "derivation:345678..."
binutils = "derivation:456789..."
make = "derivation:567890..."

[provenance.build]
builder = "conary 0.9.0"
built_at = "2026-03-19T14:30:00Z"
duration_secs = 342
host_arch = "x86_64"
sandbox = { pid = true, mount = true, network = false }

[provenance.verification]
reproducible = true
independent_rebuilds = 3
```

Provenance records are CAS objects. They are immutable — each rebuild produces a new record.

### 5.2 Trust Levels

| Level | Name | Meaning |
|-------|------|---------|
| 0 | **Unverified** | Built locally, no independent confirmation |
| 1 | **Substituted** | Downloaded from cache, derivation ID matches but never rebuilt locally |
| 2 | **Locally built** | Built from source on this machine |
| 3 | **Independently verified** | 2+ independent builders got the same output hash |
| 4 | **Diverse-verified** | Built from 2+ different seeds, outputs match (Thompson-attack resistant) |

Level 4 is the gold standard. If you build gcc from two completely different seeds and get the same binary, neither seed could have trojaned the compiler. This is the "diverse double-compilation" technique. The derivation model supports it via `source_derivation_id` — two builds with the same source derivation ID but different seeds are directly comparable. If their output hashes match, the build is seed-independent.

### 5.3 Integrity at Rest: fs-verity

The EROFS + composefs stack provides kernel-level integrity:

- Every file in CAS is content-addressed (SHA-256)
- EROFS images reference files by their CAS hash
- composefs supports fs-verity: the kernel computes and checks file hashes at read time
- Tampered files produce I/O errors, not corrupted data

Verification is continuous at runtime, transparently, at kernel speed.

```toml
[integrity]
fsverity = true
erofs_digest = true
```

### 5.4 SBOM (Software Bill of Materials)

An SBOM is a projection of the build profile — effectively free:

```bash
conary sbom --image my-system.erofs --format spdx
conary sbom --image my-system.erofs --format cyclonedx
```

The SBOM includes every package name/version, source URL and hash, build dependencies, derivation ID, and seed lineage. Richer than typical SBOMs because it includes the build graph, not just a flat list.

### 5.5 Verification Commands

```bash
# Trace every package back to the seed
conary verify-chain --image my-system.erofs

# Rebuild a derivation and compare to cached output
conary verify-rebuild --derivation a1b2c3

# Cross-seed verification (Thompson attack resistance)
conary verify-diverse --image my-system.erofs --seed alt-seed-def456

# Profile reproducibility score
conary verify-profile --profile pinned.toml
# Output: 114/114 derivations verified by >= 2 independent builders

# Machine-readable audit report for compliance
conary audit --image my-system.erofs --output audit-report.json
```

### 5.6 Chain Verification Output

```
$ conary verify-chain --image my-system.erofs

Seed: abc123... (community 2026Q1, 3 independent verifications)

Stage: toolchain (12 packages)
  binutils-2.46    derivation:a1b2c3 -> output:d4e5f6  [level 2: locally built]
  gcc-15.2.0       derivation:f7a8b9 -> output:c0d1e2  [level 3: 2 independent]
  glibc-2.43       derivation:345678 -> output:f3a4b5  [level 3: 3 independent]

Stage: foundation (12 packages, purity rebuild)
  gcc-15.2.0       derivation:901234 -> output:c0d1e2  [level 2: locally built]
  [OK] foundation gcc output matches toolchain gcc output (self-consistent)

Stage: system (85 packages)
  ...

Stage: customization (5 packages)
  my-agent-1.0     derivation:567890 -> output:abcdef  [level 0: unverified]
  [WARN] my-agent-1.0 has no independent verification

Chain: COMPLETE
  114/114 derivations traced to seed abc123
  109/114 at trust level >= 2
  1 warning: my-agent-1.0 unverified
```

## 6. Error Handling & Recovery

### 6.1 Build Failure

**Impact: zero.** Builds run in mount namespaces against read-only composefs sysroots with tmpfs build directories. Nothing is written to CAS on failure.

**Recovery:** Fix the recipe, re-run. Every completed derivation is a cache hit — Conary retries only the failure.

**Diagnostics:**

```bash
conary bootstrap my-system.toml --keep-logs
# Logs at: .conary/bootstrap/logs/gcc-15.2.0.log

conary bootstrap my-system.toml --shell-on-failure
# Drop into the exact build environment where the failure occurred
```

`--shell-on-failure` drops you into the mount namespace with the exact sysroot, source tree, and environment variables. Debug live, then update the recipe.

### 6.2 Missing Dependencies

Caught early with clear guidance:

```
Error: build of zlib-1.3.1 failed

  Package 'pkg-config' was called during configure but is not in makedepends.
  The build environment only contains declared dependencies.

  Fix: add 'pkg-config' to makedepends in recipes/system/zlib/recipe.toml

  Hint: run 'conary recipe audit zlib' to detect all missing dependencies
```

`conary recipe audit <package>` builds in a minimal sandbox and traces every external call, reporting what's missing from makedepends.

### 6.3 Cache Corruption

CAS objects are content-addressed — corruption is self-detecting:

```
Error: CAS object d4e5f6... failed integrity check
  Expected: d4e5f6...
  Got:      a1b2c3...

  Recovering: re-fetching from substituter cache.conary.io... OK
```

Self-healing from substituters. If unavailable, the derivation is rebuilt. Proactive check: `conary cas verify`.

### 6.4 Partial Builds (Crash Recovery)

- Completed derivations survive (atomic CAS writes)
- Stage EROFS images survive (atomic rename)
- The build profile survives (computed before building)
- Resume: re-run the same command. Cache hits for everything completed, retries the interrupted package.

### 6.5 Seed Revocation

```bash
conary seed check abc123...
# Warning: seed abc123 has been revoked
# Reason: independent rebuild produced different output
# Revoked: 2026-03-19 by seeds.conary.io

conary bootstrap my-system.toml --seed https://seeds.conary.io/x86_64/2026Q1-v2
```

Conary checks the revocation list before using a seed.

### 6.6 Reproducibility Failures

Same derivation, different output — timestamps, randomized ordering, embedded paths:

```bash
conary verify-rebuild --derivation a1b2c3

  Rebuild of gcc-15.2.0:
    Original output: d4e5f6...
    Rebuild output:  d4e5f7...  MISMATCH

    Differing files:
      usr/bin/gcc: 2 bytes differ at offset 0x4a20 (embedded timestamp)

    Verdict: NON-REPRODUCIBLE (timestamp difference only)
```

This is a warning, not an error. Conary tracks which derivations are reproducible and reports the profile's reproducibility score.

### 6.7 Network Failure

```
Substituter cache.conary.io: unreachable
  Falling back to local build for 34 uncached derivations
```

Air-gapped builds work fine with pre-populated CAS:

```bash
conary cache populate --profile pinned.toml --sources-only    # source tarballs
conary cache populate --profile pinned.toml --full             # sources + pre-built
```

## 7. Implementation Phases

Each phase has a clear deliverable and demo moment.

### Phase 1: Derivation Engine Core

Build the inner loop: recipe -> derivation -> CAS.

- Derivation data model: compute `derivation_id` from recipe inputs + dependency hashes
- Package output model: file manifest + CAS object references
- Build executor: mount sandbox -> Kitchen.cook() -> walk output dir -> ingest to CAS -> write output manifest
- **Derivation index:** persistent `derivation_id` -> `output_hash` mapping stored in SQLite (consistent with database-first principle). This is new infrastructure — the existing CAS is a pure content-addressed blob store with no derivation awareness. The existing `BuildCache` in the recipe module maps `cache_key -> CookResult` using a different key scheme; the derivation index replaces it with a derivation-native lookup.
- Build environment sandbox: composefs-only mount configuration (no host filesystem paths). The derivation engine manages EROFS composition and composefs mounting externally; the Kitchen receives a regular filesystem path as its sysroot and is unmodified.

**Demo:** `conary derivation build recipes/system/zlib/recipe.toml` — builds zlib into CAS, prints derivation ID and output hash. Run again — instant cache hit.

### Phase 2: EROFS Composition & Layered Builds

Build the composition loop: package outputs -> EROFS -> composefs mount -> next build.

- Compose N package outputs into a single EROFS image
- Mount EROFS via composefs as read-only sysroot
- Wire executor to use composed sysroot as build environment
- Chain: build A, compose EROFS from A, build B against that mount

**Demo:** Build binutils into CAS. Compose EROFS. Mount it. Build gcc against that mount. Layered build model working end-to-end.

### Phase 3: Stage Pipeline & Profile Generation

Full pipeline: manifest -> profile -> staged execution.

- Stage assignment algorithm: dependency graph analysis, self-hosting set detection
- Profile generator: manifest + recipes -> ordered profile with all derivation IDs
- Pipeline executor: iterate stages, compose build environments, execute derivations
- Parallelism within stages
- Seed loading: import seed EROFS + CAS objects as Layer 0

**Demo:** `conary bootstrap my-system.toml` — builds a full system from seed through all stages to a bootable generation.

### Phase 4: Caching, Resume & Developer Experience

Make it fast and pleasant.

- Resume support: crash-safe, skip completed derivations
- `--shell-on-failure`: interactive debugging in the build environment
- `conary recipe audit`: detect missing makedepends
- `conary profile show / diff`: inspect and compare profiles
- `--up-to`, `--only`: partial builds
- Build log capture

**Demo:** Change one recipe. Rebuild takes minutes, not hours — only changed package and dependents rebuilt.

### Phase 5: Substituters & Community Sharing

Make builds fast for everyone.

- Substituter protocol: query remote CAS by derivation ID
- Remi integration: serve package outputs as substituter
- Seed registry: publish/fetch/verify seeds
- Profile publishing via Remi
- `conary cache populate`: pre-fetch for air-gapped builds

**Demo:** Fresh machine. `conary bootstrap my-system.toml` fetches 110/114 packages from cache, builds 4 locally. Full system in minutes.

### Phase 6: Verification & Audit

The security story, fully operational.

- Provenance record generation (automatic)
- `conary verify-chain`: trace image to seed
- `conary verify-rebuild`: independent reproducibility check
- `conary verify-diverse`: cross-seed Thompson attack resistance
- Trust level tracking
- SBOM generation (SPDX, CycloneDX)
- Audit report generation
- Seed revocation checking

**Demo:** `conary verify-chain --image prod.erofs` — complete chain with trust levels and reproducibility scores.

### Phase 7: Corporate & Advanced Features

Production readiness for enterprise.

- Private seed management
- Private Remi as internal substituter
- Air-gapped operation
- Profile pinning and version control integration
- Multi-architecture (aarch64, riscv64)
- Custom stage definitions
- CI/CD integration

### Phase Dependencies

| Phase | Deliverable | Depends On |
|-------|-------------|-----------|
| 1 | Single package -> CAS loop | Existing CAS, Kitchen, Sandbox |
| 2 | Layered builds (CAS -> EROFS -> composefs -> CAS) | Phase 1 |
| 3 | Full pipeline: manifest -> bootable image | Phases 1-2 |
| 4 | Fast rebuilds, resume, developer tools | Phase 3 |
| 5 | Community cache, seed sharing, substituters | Phase 3 |
| 6 | Verification, audit, SBOM, trust levels | Phase 3 |
| 7 | Corporate features, multi-arch, CI | Phases 4-6 |

Phases 1-3 are the foundation. Phases 4-7 are independent of each other and can be prioritized based on need.

## Appendix: Relationship to Nix/Guix

| Nix Concept | Conary Equivalent | Difference |
|-------------|-------------------|------------|
| Store path | CAS hash | EROFS + composefs instead of directory tree |
| Derivation | Derivation | Same concept, TOML recipes instead of Nix language |
| Profile | Generation (EROFS image) | Kernel-level integrity via fs-verity |
| Binary cache | Substituter | Same concept, Remi as the server |
| Nix expression | System manifest (TOML) | No functional language to learn |
| Sandbox | Sandbox | Same concept, already implemented |
| Flake | Build profile | Pinned, shareable, reproducible build plan |

The fundamental difference: Conary uses EROFS/composefs as the composition layer, which gives kernel-level integrity (fs-verity), atomic generation switching, and composefs-native mounts — all baked into the Linux kernel rather than implemented in userspace.
