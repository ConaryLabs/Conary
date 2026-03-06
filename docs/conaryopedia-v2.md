---
last_updated: 2026-03-06
revision: 1
summary: Comprehensive technical guide covering all Conary subsystems
---

# Conaryopedia v2

*A comprehensive guide to Conary package management*

This document is the definitive reference for Conary's concepts, commands, and architecture. It is organized to build understanding incrementally -- each chapter builds on the vocabulary established in the previous one.

The [original Conaryopedia](conaryopedia.md) documented the rPath-era Conary (2005-2012). This document covers the modern Conary, rewritten in Rust with the same conceptual DNA but significantly expanded capabilities.

---

## Table of Contents

**1. [Core Concepts](#1-core-concepts)**
- [1.1 Troves](#11-troves) -- [1.2 Changesets](#12-changesets) -- [1.3 Components](#13-components) -- [1.4 Labels](#14-labels) -- [1.5 Flavors](#15-flavors) -- [1.6 Versions](#16-versions) -- [1.7 Content-Addressable Storage](#17-content-addressable-storage) -- [1.8 System State](#18-system-state) -- [1.9 Dependencies](#19-dependencies) -- [1.10 Collections](#110-collections)

**2. [System Management](#2-system-management)**
- [2.1 Initialization](#21-initialization) -- [2.2 Installing Packages](#22-installing-packages) -- [2.3 Removing Packages](#23-removing-packages) -- [2.4 Updating Packages](#24-updating-packages) -- [2.5 Searching and Listing](#25-searching-and-listing) -- [2.6 Dependency Queries](#26-dependency-queries) -- [2.7 Pinning](#27-pinning) -- [2.8 Orphan Cleanup](#28-orphan-cleanup) -- [2.9 System Adoption](#29-system-adoption) -- [2.10 System State and Rollback](#210-system-state-and-rollback) -- [2.11 Changeset History](#211-changeset-history) -- [2.12 Integrity Verification](#212-integrity-verification) -- [2.13 Repository Management](#213-repository-management) -- [2.14 Configuration File Management](#214-configuration-file-management) -- [2.15 Garbage Collection](#215-garbage-collection) -- [2.16 Labels](#216-labels) -- [2.17 Triggers](#217-triggers) -- [2.18 Redirects](#218-redirects) -- [2.19 SBOM Generation](#219-sbom-generation) -- [2.20 Shell Completions](#220-shell-completions)

**3. [Declarative System Model](#3-declarative-system-model)**
- [3.1 The Model File](#31-the-model-file) -- [3.2 Diff](#32-diff-see-what-would-change) -- [3.3 Apply](#33-apply-sync-system-to-model) -- [3.4 Check](#34-check-drift-detection) -- [3.5 Snapshot](#35-snapshot-capture-current-state) -- [3.6 Derived Packages](#36-derived-packages) -- [3.7 Remote Includes](#37-remote-includes) -- [3.8 Lockfiles](#38-lockfiles) -- [3.9 Remote Diff](#39-remote-diff) -- [3.10 Publishing](#310-publishing) -- [3.11 Automation](#311-automation) -- [3.12 Federation in the Model](#312-federation-in-the-model) -- [3.13 Workflow Summary](#313-workflow-summary)

**4. [CCS Package Format](#4-ccs-package-format)**
- [4.1 Package Structure](#41-package-structure) -- [4.2 The Manifest](#42-the-manifest-ccstoml) -- [4.3 Building Packages](#43-building-packages) -- [4.4 Build Policies](#44-build-policies) -- [4.5 Installing CCS Packages](#45-installing-ccs-packages) -- [4.6 Signing and Verification](#46-signing-and-verification) -- [4.7 Inspecting Packages](#47-inspecting-packages) -- [4.8 OCI Export](#48-oci-export) -- [4.9 Ephemeral Environments](#49-ephemeral-environments) -- [4.10 Enhancement](#410-enhancement) -- [4.11 Lockfiles](#411-lockfiles) -- [4.12 Provenance](#412-provenance-package-dna)

**5. [Recipe System](#5-recipe-system)**
- [5.1 Culinary Terminology](#51-culinary-terminology) -- [5.2 Recipe Format](#52-recipe-format) -- [5.3 The Cook Command](#53-the-cook-command) -- [5.4 Hermetic Build Architecture](#54-hermetic-build-architecture) -- [5.5 Kitchen Configuration](#55-kitchen-configuration) -- [5.6 Build Phases](#56-build-phases-in-detail) -- [5.7 Container Isolation](#57-container-isolation) -- [5.8 Cross-Compilation](#58-cross-compilation-support) -- [5.9 Bootstrap Stages](#59-bootstrap-stages) -- [5.10 Build Provenance](#510-build-provenance-capture) -- [5.11 Build Caching](#511-build-caching) -- [5.12 Dependency Graph](#512-dependency-graph) -- [5.13 Bootstrap Plans](#513-bootstrap-plans) -- [5.14 PKGBUILD Conversion](#514-pkgbuild-conversion) -- [5.15 Source Management](#515-source-management)

**6. [Remi Server](#6-remi-server)**
- [6.1 Architecture Overview](#61-architecture-overview) -- [6.2 Configuration](#62-toml-configuration) -- [6.3 Conversion Pipeline](#63-on-demand-conversion-pipeline) -- [6.4 Chunk Storage](#64-content-addressed-chunk-storage) -- [6.5 LRU Eviction](#65-lru-cache-eviction) -- [6.6 Bloom Filter](#66-bloom-filter-dos-protection) -- [6.7 Pull-Through Caching](#67-pull-through-caching-and-request-coalescing) -- [6.8 Chunk Endpoints](#68-chunk-serving-endpoints) -- [6.9 R2/CDN](#69-r2cdn-integration) -- [6.10 Sparse Index](#610-sparse-http-index) -- [6.11 Search](#611-full-text-search) -- [6.12 OCI Distribution](#612-oci-distribution-spec-v2) -- [6.13 Security](#613-security) -- [6.14 Negative Cache](#614-negative-cache) -- [6.15 Job Management](#615-job-management) -- [6.16 Analytics](#616-analytics-and-metrics) -- [6.17 Delta Manifests](#617-delta-manifests) -- [6.18 Pre-Warming](#618-pre-warming-pipeline) -- [6.19 Federated Index](#619-federated-sparse-index) -- [6.20 Remi Lite](#620-remi-lite-zero-config-lan-proxy) -- [6.21 Index Signing](#621-index-generation-and-signing) -- [6.22 Deployment](#622-deployment) -- [6.23 API Reference](#623-complete-api-reference)

**7. [Security and Trust](#7-security-and-trust)**
- [7.1 Capability Declarations](#71-capability-declarations) -- [7.2 Capability Inference](#72-capability-inference) -- [7.3 Capability Enforcement](#73-capability-enforcement) -- [7.4 Capability-Based Resolution](#74-capability-based-dependency-resolution) -- [7.5 Capability Audit](#75-capability-audit) -- [7.6 Container Sandboxing](#76-container-sandboxing) -- [7.7 Package DNA](#77-package-dna-provenance) -- [7.8 TUF Supply Chain Trust](#78-tuf-supply-chain-trust) -- [7.9 Hermetic Build Security](#79-hermetic-build-security) -- [7.10 Security Architecture Summary](#710-security-architecture-summary)

**8. [Advanced Topics](#chapter-8----advanced-topics)**
- [8.1 Bootstrap](#81-bootstrap-building-an-os-from-nothing) -- [8.2 CAS Federation](#82-cas-federation-distributed-content-sharing) -- [8.3 Delta Updates](#83-delta-updates-binary-diffs) -- [8.4 System Model](#84-system-model-declarative-os-state) -- [8.5 Recipe System](#85-recipe-system-building-from-source) -- [8.6 Automated Maintenance](#86-automated-maintenance) -- [8.7 Transaction Engine](#87-transaction-engine-crash-safe-operations) -- [8.8 Putting It All Together](#88-putting-it-all-together)

[Conclusion](#conclusion)

---

## 1. Core Concepts

Conary's design revolves around a small set of concepts that compose together to produce powerful behavior. Understanding these concepts is essential before using any Conary command.

### 1.1 Troves

A **trove** is the fundamental unit in Conary. Every piece of managed software -- whether a full package, a single component, or a curated collection -- is a trove.

A trove has:

- A **name** (e.g., `nginx`, `openssl:devel`, `@web-stack`)
- A **version** (e.g., `1.24.0-2.fc43`)
- A **type** -- one of four kinds:

| Type | Description | Example |
|------|-------------|---------|
| **Package** | A complete software package with files | `nginx` |
| **Component** | A functional subset of a package | `nginx:runtime` |
| **Collection** | A named group of packages | `@web-stack` |
| **Redirect** | A pointer from an old name to a new one | `libfoo` -> `libfoo2` |

Every trove in the database has a unique identity: the combination of its name, version, and architecture.

#### Install Source

Conary tracks *how* each trove arrived on the system:

| Source | Meaning |
|--------|---------|
| `file` | Installed from a local package file (`.rpm`, `.deb`, `.ccs`) |
| `repository` | Downloaded and installed from a remote repository |
| `adopted-track` | Metadata adopted from the system package manager (files not in CAS) |
| `adopted-full` | Fully adopted with file contents stored in CAS |
| `taken` | Taken over from the system package manager; Conary fully owns the files |

The `adopted-track` vs `adopted-full` distinction matters for rollback. Only troves with files in the content-addressable store can be fully restored.

#### Install Reason

Each trove also records *why* it was installed:

| Reason | Meaning |
|--------|---------|
| `explicit` | The user directly requested this package |
| `dependency` | Installed automatically to satisfy another package's requirements |

This distinction powers `conary autoremove` -- when an explicitly installed package is removed, its dependency-only packages become orphans and can be safely cleaned up.

A dependency can be **promoted** to explicit if the user later installs it by name. The `selection_reason` field provides a human-readable explanation (e.g., "Required by nginx", "Installed via @web-stack").

### 1.2 Changesets

A **changeset** is an atomic transaction. Every operation that modifies system state -- install, remove, update, rollback -- creates a changeset. This is the mechanism that prevents half-configured systems.

A changeset records:

- A description of the operation (e.g., "Install nginx 1.24.0")
- A status: `pending`, `applied`, or `rolled_back`
- Timestamps for creation, application, and rollback
- A `tx_uuid` for crash recovery correlation

#### How Changesets Work

```
1. User runs: conary install nginx
2. Conary creates a changeset (status: pending)
3. Files are staged to a temporary directory
4. The database transaction begins
5. Files are deployed to their final locations
6. The changeset is marked "applied"
7. A system state snapshot is created
```

If *anything* fails at steps 3-6, the entire operation is rolled back -- files are restored from CAS, database changes are reverted, and the changeset is marked accordingly. The `tx_uuid` ensures that even if the process crashes mid-transaction, recovery can identify and clean up incomplete work.

#### Rollback

Because every changeset records what it changed, any changeset can be reversed:

```bash
conary system state list         # Show all state snapshots
conary system state rollback 5   # Reverse all changes since state 5
```

Rollback creates a *new* changeset that undoes the effects of previous ones. This means rollbacks themselves can be rolled back -- there is no destructive operation.

### 1.3 Components

A **component** is a functional subdivision of a package. When Conary installs a package, it automatically classifies every file into a component based on its path:

| Component | Contains | Default? |
|-----------|----------|----------|
| `:runtime` | Executables, assets, helpers -- the main package content | Yes |
| `:lib` | Shared libraries (`.so` files in lib directories) | Yes |
| `:config` | Configuration files (`/etc/*`) | Yes |
| `:devel` | Headers, static libraries, pkg-config files | No |
| `:doc` | Man pages, info pages, documentation | No |
| `:debuginfo` | Debug symbols (`.debug` files, build-id indexed) | No |
| `:test` | Test suites and test data | No |

**Default components** (`:runtime`, `:lib`, `:config`) are installed when you run `conary install nginx`. Non-default components must be explicitly requested.

```bash
conary install nginx              # Installs :runtime, :lib, :config
conary install nginx:devel        # Just the development headers
conary install nginx:all          # Everything
```

Components are addressable troves in their own right. They have their own dependency relationships -- for example, `openssl:devel` depends on `openssl:lib` but not on `openssl:runtime`.

#### Classification Rules

Classification is based on file paths, not metadata. Some examples:

| File Path | Component |
|-----------|-----------|
| `/usr/bin/nginx` | `:runtime` |
| `/usr/lib64/libssl.so.3` | `:lib` |
| `/usr/include/openssl/ssl.h` | `:devel` |
| `/usr/lib/pkgconfig/openssl.pc` | `:devel` |
| `/usr/share/man/man1/nginx.1.gz` | `:doc` |
| `/etc/nginx/nginx.conf` | `:config` |
| `/usr/lib/debug/.build-id/ab/cdef.debug` | `:debuginfo` |

The classifier is conservative: `:runtime` is the default bucket. A file is only classified elsewhere when the path unambiguously identifies its role. Multi-arch paths (`/usr/lib/x86_64-linux-gnu/`, `/usr/lib64/`) are handled correctly.

### 1.4 Labels

A **label** tracks where a package came from -- its provenance. Labels use the format inherited from original Conary:

```
repository@namespace:tag
```

The three parts:

| Part | Purpose | Example |
|------|---------|---------|
| **Repository** | The source hostname or identifier | `fedora.conary.io` |
| **Namespace** | A grouping within the repository | `fc` |
| **Tag** | The branch or release identifier | `43` |

Full example: `fedora.conary.io@fc:43` means "Fedora 43 packages from fedora.conary.io."

#### Label Path

The **label path** is an ordered list of labels that defines where Conary searches for packages. When resolving a dependency, Conary walks the label path from highest to lowest priority.

```bash
conary query label path                  # Show current search order
conary query label add local@dev:main    # Add a label
conary query label add fedora@fc:43      # Add another
```

#### Label Delegation

Labels can **delegate** to other labels, creating chains:

```bash
conary query label delegate local@dev:main fedora@fc:43
```

This means: "When resolving packages via `local@dev:main`, if the package isn't found locally, try `fedora@fc:43`." Delegation chains are followed with cycle detection.

Labels can also be **linked** to repositories, allowing a label to serve as a named alias for a repository's package set:

```bash
conary query label link fedora@fc:43 fedora-43
```

### 1.5 Flavors

A **flavor** describes the build-time conditions under which a package was produced. The same package can have multiple flavors -- for example, one built with SSL support and one without, or one for x86_64 and one for aarch64.

Flavor syntax uses square brackets with comma-separated items:

```
[ssl, !debug, ~vmware, is: x86_64]
```

#### Flavor Operators

| Operator | Prefix | Meaning |
|----------|--------|---------|
| Required | *(none)* | Package is built for systems *with* this feature |
| Not | `!` | Package is built for systems *without* this feature |
| Prefers | `~` | Soft preference: use if no `!X` variant exists |
| Prefers Not | `~!` | Soft preference: use if no `X` variant exists |

Architecture flavors are listed at the end after `is:`:

```
[ssl, ~!debug is: x86_64]
```

When Conary resolves which flavor of a package to install, it matches the system's flavor against available flavors, preferring exact matches and falling back through the preference operators.

### 1.6 Versions

Conary handles version strings from multiple packaging ecosystems. The primary format is RPM-style:

```
[epoch:]version[-release]
```

| Component | Description | Example |
|-----------|-------------|---------|
| **Epoch** | Numeric override for ordering (optional, default 0) | `2:` |
| **Version** | The upstream version string | `1.24.0` |
| **Release** | The distribution release (optional) | `2.fc43` |

Examples:

```
1.24.0                   # Simple version
1.24.0-2.fc43            # Version with release
2:1.0.0-1.el9            # Epoch override (2: beats any 1.x)
```

#### Version Constraints

Dependencies can specify version constraints:

| Constraint | Meaning |
|------------|---------|
| `>= 1.0` | Version 1.0 or later |
| `< 2.0` | Any version before 2.0 |
| `= 1.24.0-2.fc43` | Exactly this version |
| `>= 1.0, < 2.0` | Range constraint |
| *(none)* | Any version satisfies |

Version comparison follows RPM algorithm rules: segments are compared numerically when both are numbers, lexicographically otherwise. Tildes (`~`) sort before anything (used for pre-release versions like `1.0~beta1`).

### 1.7 Content-Addressable Storage

The **CAS** (Content-Addressable Storage) is Conary's file storage layer, inspired by git's object model. Files are stored by their content hash, not by name or path.

```
/var/lib/conary/objects/
  ab/
    ab3def4567890123456789...   # SHA-256 of file contents
  cd/
    cd1234567890abcdef0123...
```

This design provides three key properties:

1. **Deduplication**: If two packages contain the same file (even under different names), it is stored once.

2. **Integrity verification**: Any file can be verified by recomputing its hash:
   ```bash
   conary system verify nginx    # Check all nginx files against CAS
   ```

3. **Rollback support**: Old file versions are never deleted (until garbage collected). Rolling back means pointing to the old hash, not restoring from backup.

#### Hash Algorithms

The CAS supports two hash algorithms:

| Algorithm | Use Case |
|-----------|----------|
| **SHA-256** | Default. Cryptographic hash for security-critical verification. |
| **XXH128** | Fast non-cryptographic hash for pure deduplication scenarios. |

For network transfers and package verification, SHA-256 is always used. XXH128 is available as an optimization for local-only CAS operations where speed matters more than cryptographic guarantees.

#### Content-Defined Chunking

For packages built with the CCS format, files are further split into **chunks** using FastCDC (Fast Content-Defined Chunking):

| Parameter | Value |
|-----------|-------|
| Minimum chunk | 16 KB |
| Average chunk | 64 KB |
| Maximum chunk | 256 KB |

Chunking provides implicit delta compression: when a file changes between versions, most chunks remain identical. The client only downloads the new chunks. There is no need to pre-compute version-to-version deltas -- the chunking handles it naturally.

### 1.8 System State

A **system state** is a snapshot of every package installed on the system at a point in time. States are created automatically after each changeset is applied.

```bash
conary system state list          # Show all snapshots
conary system state show 12       # Details of state 12
conary system state diff 5 8      # What changed between states 5 and 8
conary system state rollback 5    # Return to state 5
```

Each state records:

- A sequential **state number**
- A **summary** (e.g., "Install nginx 1.24.0")
- The **changeset** that produced this state
- An **is_active** flag marking the current state
- The **package count**
- A complete member list: every trove name, version, architecture, and install reason

States enable time travel. You can compare any two states to see exactly what changed, or roll back to any previous state. State pruning (`conary system state prune`) removes old snapshots to reclaim space while preserving the ability to roll back to recent states.

### 1.9 Dependencies

A **dependency** declares that one trove requires another to function. Conary supports **typed dependencies** -- each dependency has a *kind* that identifies what type of interface is being required:

| Kind | Format | Example |
|------|--------|---------|
| `package` | Package name | `openssl` |
| `soname` | Shared library name | `soname(libssl.so.3)` |
| `python` | Python module | `python(flask)` |
| `perl` | Perl module | `perl(CGI)` |
| `ruby` | Ruby gem | `ruby(rake)` |
| `java` | Java package | `java(org.apache.commons)` |
| `pkgconfig` | pkg-config name | `pkgconfig(zlib)` |
| `cmake` | CMake package | `cmake(OpenSSL)` |
| `binary` | Executable in PATH | `binary(gcc)` |
| `file` | Specific file path | `file(/usr/bin/python3)` |
| `interpreter` | Script interpreter | `interpreter(/usr/bin/perl)` |

Typed dependencies enable precise resolution. When `nginx` depends on `soname(libssl.so.3)`, Conary finds the trove that *provides* that specific soname, not just any package named "openssl."

#### Dependency Resolution

Resolution uses a SAT solver (via resolvo) with topological sorting:

1. Build a dependency graph from all requested packages and their transitive dependencies
2. Detect conflicts (two packages providing the same file, version incompatibilities)
3. Find a satisfying assignment using the SAT solver
4. Produce an installation order via topological sort (dependencies before dependents)
5. Report any missing dependencies with information about what requires them

```bash
conary query depends nginx        # What does nginx need?
conary query rdepends openssl     # What needs openssl?
conary query deptree nginx        # Full dependency tree
conary query whatprovides libc.so.6  # Who provides this?
```

#### Provides

The counterpart to dependencies is **provides** -- declarations of what capabilities a trove offers. Provides are auto-detected during installation:

- Shared libraries are scanned for sonames
- Python/Perl/Ruby/Java modules are detected from file paths
- Executables in PATH generate `binary()` provides
- pkg-config and CMake files generate typed provides

### 1.10 Collections

A **collection** (called a "group" in original Conary) is a named set of packages. Collections are themselves troves, but they contain no files -- only references to member packages.

```bash
conary collection create web-stack --members nginx,postgresql,redis
conary install @web-stack          # Install all members
conary collection show web-stack   # List members
```

Collections support:

- **Optional members**: Packages that are part of the collection but not installed by default
- **Bulk operations**: Install, update, or remove all members at once
- **Version coherence**: All members are installed from the same label path

Collections are the building block for defining system roles. A server might use `@group-base-server` that includes `@group-networking`, `@group-security-tools`, and specific service packages.

---

## 2. System Management

This chapter covers the day-to-day operations of managing packages on a Conary system: installing, removing, updating, searching, querying, and maintaining system state.

### 2.1 Initialization

Before Conary can manage packages, its database must be initialized:

```bash
conary system init
```

This creates the SQLite database at `/var/lib/conary/conary.db` and sets up all tables (currently schema v36). The database is the single source of truth for all package state -- there are no configuration files for runtime state.

You can specify an alternate database path with `-d`:

```bash
conary system init -d /path/to/custom.db
```

### 2.2 Installing Packages

The `install` command accepts three types of targets:

| Target Type | Example | Description |
|-------------|---------|-------------|
| Package name | `conary install nginx` | Resolved from configured repositories |
| File path | `conary install ./nginx-1.24.0.rpm` | Local RPM, DEB, Arch, or CCS file |
| Collection | `conary install @web-stack` | All members of a collection |

#### Basic Installation

```bash
conary install nginx                     # From repository
conary install nginx --version 1.24.0    # Specific version
conary install nginx --repo fedora-43    # From specific repository
conary install ./package.rpm             # Local file
conary install @web-stack                # Collection
```

#### Component Installation

Components follow the `package:component` syntax from Chapter 1:

```bash
conary install nginx              # Default components (:runtime, :lib, :config)
conary install nginx:devel        # Just development headers
conary install nginx:all          # Everything including docs and debuginfo
```

#### Installation Flags

| Flag | Effect |
|------|--------|
| `--dry-run` | Show what would be installed without making changes |
| `--no-deps` | Skip dependency resolution (dangerous) |
| `--no-scripts` | Don't run package scriptlets |
| `--sandbox auto\|always\|never` | Control scriptlet sandboxing |
| `--allow-downgrade` | Allow installing an older version |
| `--force` | Install even if already adopted from system package manager |
| `--skip-optional` | Skip optional members when installing a collection |

#### CCS Conversion on Install

When installing a legacy package (RPM/DEB/Arch), you can convert it to CCS format in-flight:

```bash
conary install nginx --convert-to-ccs
```

This enables CAS deduplication, component selection, and atomic transactions for the installed package. Scriptlets are automatically captured and converted to declarative hooks. Use `--no-capture` to disable scriptlet capture (the scriptlets will run imperatively at install time instead).

#### What Happens During Install

1. **Resolution**: The package name is resolved through redirects, labels, and repositories
2. **Dependency solving**: The SAT solver builds a complete dependency graph
3. **Download**: Packages are fetched (from repository, CAS federation, or local file)
4. **Staging**: Files are extracted to a temporary directory
5. **Changeset creation**: An atomic changeset is opened (status: `pending`)
6. **Deployment**: Files are moved to their final locations
7. **Scriptlets**: Package install hooks run (optionally sandboxed)
8. **Triggers**: File-pattern triggers fire (e.g., `ldconfig` for new `.so` files)
9. **Commit**: The changeset is marked `applied` and a system state snapshot is created

If any step fails, the entire operation rolls back automatically.

### 2.3 Removing Packages

```bash
conary remove nginx                      # Remove nginx
conary remove nginx --version 1.24.0     # Remove specific version
conary remove nginx --purge-files        # Also delete files for adopted packages
```

By default, removing a package that was adopted from the system package manager (`adopted-track` or `adopted-full`) only removes Conary's tracking metadata -- the files remain on disk because the system package manager still owns them. Use `--purge-files` to also delete the files.

Removal respects dependencies: if other packages depend on the one being removed, Conary will refuse unless `--no-deps` is specified.

| Flag | Effect |
|------|--------|
| `--version` | Remove a specific version (required if multiple versions installed) |
| `--no-scripts` | Skip removal scriptlets |
| `--sandbox` | Control scriptlet sandboxing |
| `--purge-files` | Delete files on disk (for adopted packages) |

### 2.4 Updating Packages

```bash
conary update                    # Update all packages
conary update nginx              # Update just nginx
conary update @web-stack         # Update all members of a collection
conary update --security         # Only security updates (critical/important)
```

The update command checks configured repositories for newer versions of installed packages. When no package is specified, all installed packages are checked.

Security-only updates (`--security`) filter for packages with `critical` or `important` severity advisories, allowing rapid patching without changing other packages.

### 2.5 Searching and Listing

#### Searching Repositories

```bash
conary search nginx              # Search available packages by name/description
```

Searches the synced metadata from all enabled repositories.

#### Listing Installed Packages

```bash
conary list                      # List all installed packages
conary list "lib*"               # Filter by glob pattern
conary list --pinned             # Show only pinned packages
conary list --info nginx         # Detailed info for nginx
conary list --files nginx        # List all files in nginx
conary list --lsl nginx          # Files in ls -l format
conary list --path /usr/bin/vim  # Which package owns this file?
```

#### Repository Queries

```bash
conary query repquery            # List all available packages
conary query repquery "nginx*"   # Filter available packages
conary query repquery --info nginx  # Detailed info from repo metadata
```

### 2.6 Dependency Queries

Conary provides deep dependency introspection:

```bash
conary query depends nginx       # What does nginx need?
conary query rdepends openssl    # What needs openssl?
conary query deptree nginx       # Full dependency tree
conary query deptree nginx --reverse  # Reverse tree (what depends on nginx?)
conary query deptree nginx --depth 3  # Limit tree depth
conary query whatprovides libc.so.6   # Who provides this capability?
conary query whatbreaks nginx    # What would break if nginx is removed?
conary query reason              # Show why each package was installed
conary query reason explicit     # Only explicitly installed packages
conary query reason dependency   # Only auto-installed dependencies
```

#### Component Queries

```bash
conary query components nginx          # List nginx's components
conary query component nginx:lib       # Files in nginx:lib
```

#### Conflict Detection

```bash
conary query conflicts           # Check for file ownership conflicts
conary query conflicts --verbose # Detailed conflict analysis
```

### 2.7 Pinning

Pinning prevents a package from being updated or removed:

```bash
conary pin nginx                 # Pin nginx at current version
conary unpin nginx               # Allow updates again
conary list --pinned             # Show all pinned packages
```

Pinned packages are skipped during `conary update` and protected from `conary remove`. This is useful for holding back packages that might break a production workload.

### 2.8 Orphan Cleanup

When a package is removed, any packages that were only installed as its dependencies become orphans. The `autoremove` command cleans these up:

```bash
conary autoremove                # Remove orphaned packages
conary autoremove --dry-run      # Preview what would be removed
```

This relies on the `install_reason` tracking from Chapter 1 -- only packages with reason `dependency` whose parent is no longer installed are candidates.

### 2.9 System Adoption

Conary can coexist with your system's native package manager (dnf, apt, pacman). **Adoption** imports the system package manager's metadata into Conary's database, giving you unified visibility across both systems.

#### Adopting Individual Packages

```bash
conary system adopt nginx curl vim    # Adopt specific packages
conary system adopt nginx --full      # Adopt with files copied to CAS
```

The `--full` flag copies all files into Conary's content-addressable store. This is slower but enables rollback and integrity verification for those packages.

#### Adopting the Entire System

```bash
conary system adopt --system                      # Adopt all packages
conary system adopt --system --full               # Full adoption (CAS)
conary system adopt --system --pattern "lib*"     # Only matching packages
conary system adopt --system --exclude "kernel*"  # Skip kernel packages
conary system adopt --system --explicit-only      # Skip auto-installed deps
conary system adopt --system --dry-run            # Preview
```

#### Checking Adoption Status

```bash
conary system adopt --status     # Summary of adopted/native packages
```

#### Refreshing Adopted Packages

If packages have been updated via the system package manager, refresh detects the version drift:

```bash
conary system adopt --refresh    # Update adopted packages that changed
```

#### Converting Adopted Packages to CCS

Bulk convert adopted packages to CCS format for deduplication and atomic transactions:

```bash
conary system adopt --convert              # Convert all adopted packages
conary system adopt --convert --jobs 8     # With 8 parallel threads
conary system adopt --convert --no-chunking  # Skip CDC chunking
```

#### Takeover

For full Conary management, you can **take over** packages from the system package manager:

```bash
conary system adopt --takeover   # Conary fully owns the files
conary system adopt --takeover --yes  # Skip confirmation
```

After takeover, the system package manager will no longer track those packages. This is an advanced operation.

#### Sync Hooks

Install hooks that automatically notify Conary when the system package manager installs or removes packages:

```bash
conary system adopt --sync-hook              # Install the hooks
conary system adopt --sync-hook --remove-hook  # Remove them
```

### 2.10 System State and Rollback

Every operation that modifies the system creates a state snapshot (see Chapter 1.8). These snapshots enable time-travel:

#### Listing States

```bash
conary system state list             # All snapshots
conary system state list --limit 10  # Most recent 10
```

Each state shows its number, timestamp, summary (e.g., "Install nginx 1.24.0"), and package count.

#### Inspecting a State

```bash
conary system state show 12          # Full details of state 12
```

Shows all member packages with their names, versions, architectures, and install reasons.

#### Comparing States

```bash
conary system state diff 5 12       # What changed between states 5 and 12
```

Shows packages added, removed, and version-changed between two states.

#### Rolling Back

```bash
conary system state revert 5         # Return to state 5
conary system state revert 5 --dry-run  # Preview the rollback
```

Reverting creates a new changeset that undoes all changes since state 5. This means the revert itself can be reverted -- no operation is destructive.

You can also roll back a specific changeset:

```bash
conary system state rollback 42      # Undo changeset #42
```

#### Creating Manual Snapshots

```bash
conary system state create "Before major upgrade"
conary system state create "Stable baseline" --description "All tests passing"
```

Manual snapshots are useful as bookmarks before risky operations.

#### Pruning Old States

```bash
conary system state prune 50         # Keep only the 50 most recent states
conary system state prune 50 --dry-run  # Preview what would be pruned
```

### 2.11 Changeset History

View the history of all operations:

```bash
conary system history
```

Each changeset shows its ID, operation description, timestamp, and status (`applied`, `rolled_back`).

### 2.12 Integrity Verification

Conary can verify that installed files match what was originally installed:

```bash
conary system verify                 # Verify all packages
conary system verify nginx           # Verify just nginx
conary system verify nginx --rpm     # Verify adopted packages against RPM DB
```

Verification recomputes file hashes and compares them against the CAS. Any modified, missing, or unexpected files are reported. This is essential for detecting unauthorized modifications or filesystem corruption.

#### File Restoration

If verification finds problems, restore files from the CAS:

```bash
conary system restore nginx              # Restore missing/modified files
conary system restore nginx --force      # Overwrite even if files exist
conary system restore nginx --dry-run    # Preview restoration
conary system restore all                # Check and restore all packages
```

### 2.13 Repository Management

Repositories are where Conary finds packages to install and update.

#### Adding Repositories

```bash
conary repo add fedora-43 https://mirrors.fedoraproject.org/metalink
conary repo add fedora-43 https://mirror.example.com/fedora/43 \
    --priority 100
conary repo add custom https://repo.example.com/metadata \
    --content-url https://cdn.example.com/packages \
    --gpg-key https://repo.example.com/keys/signing.pub
```

The `--content-url` flag enables the **reference mirror** pattern: metadata is fetched from the primary URL, but packages are downloaded from the content URL. This allows hosting custom metadata that points to upstream package mirrors.

#### Managing Repositories

```bash
conary repo list                     # List enabled repositories
conary repo list --all               # Include disabled repositories
conary repo enable fedora-43         # Enable a repository
conary repo disable fedora-43        # Disable a repository
conary repo remove fedora-43         # Delete a repository
```

#### Syncing Metadata

```bash
conary repo sync                     # Sync all enabled repositories
conary repo sync fedora-43           # Sync one repository
conary repo sync --force             # Force re-sync even if recent
```

#### GPG Key Management

```bash
conary repo key-import fedora-43 /path/to/RPM-GPG-KEY-fedora
conary repo key-import fedora-43 https://keys.fedoraproject.org/key.pub
conary repo key-list
conary repo key-remove fedora-43
```

When GPG checking is enabled, every package from the repository is verified against the imported key. Use `--gpg-strict` when adding a repo to require valid signatures on all packages.

#### Resolution Strategies

Repositories can specify how packages are resolved:

```bash
conary repo add remi-fed https://packages.conary.io \
    --default-strategy remi \
    --remi-endpoint https://packages.conary.io \
    --remi-distro fedora
```

| Strategy | Behavior |
|----------|----------|
| `binary` | Download pre-built packages directly (default) |
| `remi` | Convert packages on-the-fly via a Remi server |
| `legacy` | Same as binary (uses `repository_packages` table) |

### 2.14 Configuration File Management

Conary tracks configuration files (`/etc/*`) separately from other package files. This enables three-way merge during updates and dedicated backup/restore workflows.

```bash
conary config list                   # Show modified config files
conary config list nginx             # Config files for nginx
conary config list --all             # All config files (including unmodified)
conary config check                  # Check status of all config files
conary config check nginx            # Check config status for nginx
conary config diff /etc/nginx/nginx.conf   # Show local changes
conary config backup /etc/nginx/nginx.conf # Create a backup
conary config backups /etc/nginx/nginx.conf  # List backups
conary config restore /etc/nginx/nginx.conf  # Restore latest backup
conary config restore /etc/nginx/nginx.conf --backup-id 3  # Restore specific backup
```

### 2.15 Garbage Collection

Over time, old file versions accumulate in the CAS. Garbage collection removes unreferenced objects:

```bash
conary system gc                     # GC with 30-day retention
conary system gc --keep-days 7       # More aggressive pruning
conary system gc --dry-run           # Preview what would be removed
```

Files referenced by any installed package or by recent file history (within the retention period) are preserved. Only truly orphaned objects are removed.

### 2.16 Labels

Labels track package provenance (see Chapter 1.4). The query subcommand manages labels:

```bash
conary query label list                  # List all labels
conary query label list --verbose        # With descriptions and counts
conary query label add fedora@fc:43      # Create a label
conary query label add fedora@fc:43 --description "Fedora 43 packages"
conary query label remove fedora@fc:43   # Remove a label
conary query label show nginx            # Show label for a package
conary query label set nginx fedora@fc:43  # Set label for a package
conary query label query fedora@fc:43    # Find packages with this label
```

#### Label Path

```bash
conary query label path                  # Show search order
conary query label path --add local@dev:main --priority 10
conary query label path --remove old@label:name
```

#### Label Federation

```bash
conary query label link fedora@fc:43 fedora-43    # Link label to repository
conary query label link fedora@fc:43 --unlink     # Remove link
conary query label delegate local@dev:main fedora@fc:43  # Set delegation
conary query label delegate local@dev:main --undelegate  # Remove delegation
```

### 2.17 Triggers

Triggers are pattern-based actions that run automatically when matching files are installed or removed. For example, installing a new shared library triggers `ldconfig`, or installing a new `.desktop` file triggers the desktop database update.

```bash
conary system trigger list           # List all triggers
conary system trigger list --builtin # Only built-in triggers
conary system trigger show ldconfig  # Details of a trigger
conary system trigger enable ldconfig
conary system trigger disable ldconfig
conary system trigger run            # Run pending triggers
conary system trigger run 42         # Run triggers for changeset 42
```

#### Custom Triggers

```bash
conary system trigger add my-trigger \
    --pattern "/usr/share/fonts/*" \
    --handler "fc-cache -f" \
    --description "Update font cache" \
    --priority 50
conary system trigger remove my-trigger
```

### 2.18 Redirects

Redirects handle package renames, obsoletions, merges, and splits:

```bash
conary system redirect list                   # List all redirects
conary system redirect list --type obsolete   # Only obsoletions
conary system redirect add libfoo libfoo2 --type rename
conary system redirect add libfoo libfoo2 --type rename \
    --source-version ">= 1.0, < 2.0" \
    --message "libfoo was renamed to libfoo2 in version 2.0"
conary system redirect show libfoo            # Redirect details
conary system redirect resolve libfoo         # Follow the redirect chain
conary system redirect remove libfoo          # Delete a redirect
```

| Redirect Type | Use Case |
|---------------|----------|
| `rename` | Package was renamed (old name becomes alias) |
| `obsolete` | Package was superseded by another |
| `merge` | Multiple packages merged into one |
| `split` | One package split into multiple |

### 2.19 SBOM Generation

Generate a Software Bill of Materials for security auditing and compliance:

```bash
conary system sbom nginx            # SBOM for nginx
conary system sbom all              # SBOM for entire system
conary system sbom nginx --format cyclonedx --output nginx-sbom.json
```

Output is CycloneDX 1.5 JSON format, suitable for consumption by vulnerability scanners and compliance tools.

### 2.20 Shell Completions

```bash
conary system completions bash       # Bash completions
conary system completions zsh        # Zsh completions
conary system completions fish       # Fish completions

# Install for bash:
conary system completions bash > /etc/bash_completion.d/conary
```

---

## 3. Declarative System Model

Chapter 2 covered imperative package management -- running individual `install`, `remove`, and `update` commands. The **system model** provides a declarative alternative: you describe the desired state in a TOML file, and Conary computes and applies the difference.

This is the same paradigm as NixOS's `configuration.nix`, but designed for traditional Linux distributions rather than requiring a complete ecosystem replacement.

### 3.1 The Model File

A system model lives at `/etc/conary/system.toml` by default. Here is a complete example:

```toml
[model]
version = 1

# Package search path (checked in order)
search = [
    "fedora@fc:43",
    "conary@extras:stable",
]

# Packages to install and keep installed
install = [
    "nginx",
    "postgresql",
    "redis",
    "git",
    "tmux",
]

# Packages to never install (even as dependencies)
exclude = [
    "sendmail",
    "postfix",
]

# Version pins (glob patterns)
[pin]
openssl = "3.0.*"
kernel = "6.12.*"

# Optional packages (install if available, no error if missing)
[optional]
packages = ["nginx-module-geoip", "redis-sentinel"]
```

#### Model Sections

| Section | Purpose |
|---------|---------|
| `[model]` | Core config: version, search path, install list, exclude list |
| `[pin]` | Version constraints for specific packages |
| `[optional]` | Packages to install if available (soft requirements) |
| `[[derive]]` | Derived package definitions (customized variants) |
| `[include]` | Remote model includes (composition from upstream) |
| `[automation]` | Automated maintenance policies |
| `[federation]` | CAS federation settings |

### 3.2 Diff: See What Would Change

The `diff` command compares the model against the current system state:

```bash
conary model diff                          # Default: /etc/conary/system.toml
conary model diff --model /path/to/model.toml
conary model diff --offline                # Use cached data only
```

Output shows what needs to change to reach the desired state:

```
Install:
  + nginx (from fedora@fc:43)
  + postgresql (from fedora@fc:43)
  + redis (from fedora@fc:43)

Remove:
  - sendmail 8.17.1 (excluded by model)

Pin:
  ~ openssl -> 3.0.* (currently 3.2.1)

Derived:
  * nginx-custom (build from nginx + patches)

Summary: 3 installs, 1 removal, 1 pin, 1 derived build
```

The diff engine computes these action types:

| Action | Meaning |
|--------|---------|
| `Install` | Package in model but not on system |
| `Remove` | Package on system but excluded by model |
| `Update` | Installed version doesn't match pin constraint |
| `Pin` | Package should be pinned to a version pattern |
| `Unpin` | Package is pinned but shouldn't be |
| `MarkExplicit` | Dependency should be marked as explicitly installed |
| `MarkDependency` | Explicit package should be marked as dependency |
| `BuildDerived` | Derived package needs to be built |
| `RebuildDerived` | Derived package is stale (parent updated) |

### 3.3 Apply: Sync System to Model

```bash
conary model apply                         # Apply the model
conary model apply --dry-run               # Preview without changes
conary model apply --strict                # Remove packages not in model
conary model apply --skip-optional         # Skip optional packages
conary model apply --no-autoremove         # Don't clean up orphans after
conary model apply --offline               # Use cached remote data only
```

Apply performs the diff and then executes all actions as a single atomic changeset. If anything fails, the entire operation rolls back.

The `--strict` flag is important: without it, packages not mentioned in the model are left alone. With `--strict`, any package not in the install list (and not a dependency of something in the list) is removed. This is the "cattle, not pets" mode for managed servers.

### 3.4 Check: Drift Detection

```bash
conary model check                         # Exit 0 if system matches model
conary model check --verbose               # Show details of differences
conary model check --offline               # Use cached data only
```

Returns exit code 0 if the system matches the model, exit code 1 if there are differences. This is designed for CI/CD pipelines and monitoring systems:

```bash
# Cron job: alert on drift
if ! conary model check --offline 2>/dev/null; then
    echo "System has drifted from model" | mail admin@example.com
fi
```

### 3.5 Snapshot: Capture Current State

Create a model file from the current system state:

```bash
conary model snapshot                      # Write to system.toml
conary model snapshot --output baseline.toml
conary model snapshot --description "Production baseline 2026-03"
```

Snapshot captures all explicitly installed packages (not auto-installed dependencies) and any active pins. This is useful for:

- Creating an initial model from an existing system
- Recording a known-good state before changes
- Reproducing a system configuration on another machine

### 3.6 Derived Packages

A **derived package** is a customized variant of a base package. Instead of maintaining a full fork, you declare patches and file overrides:

```toml
[model]
version = 1
install = ["nginx-custom"]

[[derive]]
name = "nginx-custom"
from = "nginx"
version = "inherit"           # Track the parent's version
patches = ["patches/custom-module.patch"]

[derive.override_files]
"/etc/nginx/nginx.conf" = "files/nginx.conf"
"/etc/nginx/conf.d/default.conf" = "files/default.conf"
```

When the model is applied:

1. The parent package (`nginx`) is fetched
2. Patches are applied in order
3. Override files replace existing files
4. The result is installed as `nginx-custom`

When the parent package is updated, Conary detects that the derived package is stale and rebuilds it.

| Field | Purpose |
|-------|---------|
| `name` | Name of the derived package |
| `from` | Parent package to derive from |
| `version` | `"inherit"` (track parent) or a specific version |
| `patches` | List of patch files (paths relative to model file) |
| `override_files` | Map of destination path to source file |

### 3.7 Remote Includes

Models can compose from upstream collections using the `[include]` section. This enables organizational package policies: a central team publishes a "base server" collection, and individual servers extend it with their own packages.

```toml
[model]
version = 1
install = ["custom-app", "monitoring-agent"]

[include]
models = [
    "group-base-server@corp:production",
    "group-security-tools@corp:production",
]
on_conflict = "local"     # Local definitions win on conflict
require_signatures = true
trusted_keys = [
    "a1b2c3d4e5f6...",   # Hex-encoded Ed25519 public key
]
```

#### How Include Resolution Works

1. Each include spec (`group-name@repo:tag`) is parsed into a name and label
2. The label is resolved to a repository URL via the label database
3. The collection is fetched from the Remi server's `/v1/models/:name` endpoint
4. Nested includes are resolved recursively (max depth: 10, with cycle detection)
5. Members are merged according to the conflict strategy

#### Conflict Strategies

| Strategy | Behavior |
|----------|----------|
| `local` | Local model definitions take precedence (default) |
| `remote` | Remote definitions override local ones |
| `error` | Fail immediately on any conflict |

#### Caching

Remote collections are cached in SQLite with a TTL. The `--offline` flag restricts resolution to cached data only, enabling air-gapped operation.

### 3.8 Lockfiles

Remote includes introduce a risk: the upstream collection could change silently. The lockfile pins the exact content hash of each resolved remote:

```bash
conary model lock                          # Create/update model.lock
conary model lock --output custom.lock     # Custom output path
```

This produces a lockfile alongside the model:

```toml
[metadata]
generated_at = "2026-03-03T12:00:00Z"
model_hash = "sha256:abc123..."

[[collection]]
name = "group-base-server"
label = "corp:production"
version = "2.1.0"
content_hash = "sha256:def456..."
locked_at = "2026-03-03T12:00:00Z"
member_count = 47
```

When `conary model apply` runs with a lockfile present, it verifies that remote collections match their locked hashes. If they don't, the operation is aborted with a drift warning.

#### Updating Locks

```bash
conary model update                        # Refresh remotes and update lock
```

This fetches all remote includes, shows what changed, and writes new hashes to the lockfile. Review the changes before applying.

### 3.9 Remote Diff

Compare local state against what remote collections expect:

```bash
conary model remote-diff                   # Compare against remotes
conary model remote-diff --refresh         # Force-refresh remote data
```

This is useful for seeing what the upstream team has changed in their collections without applying anything.

### 3.10 Publishing

A local model can be published as a versioned collection to a repository, enabling other systems to include it:

```bash
conary model publish \
    --name web-stack \
    --version 2.0.0 \
    --repo https://remi.example.com \
    --description "Production web server stack" \
    --sign-key /etc/conary/keys/signing.key
```

Publishing converts the model's install list, pins, and optional packages into a `CollectionData` structure, computes a content hash, optionally signs it with an Ed25519 key, and uploads it to the Remi server's admin API.

| Flag | Purpose |
|------|---------|
| `--name` | Collection name (auto-prefixed with `group-` if missing) |
| `--version` | Semantic version string |
| `--repo` | Repository URL (local `file://` or remote `https://`) |
| `--sign-key` | Path to Ed25519 signing key for integrity |
| `--force` | Overwrite existing collection on remote |
| `--description` | Human-readable description |

### 3.11 Automation

The `[automation]` section configures automated maintenance policies. Each category can have its own mode that overrides the global default:

```toml
[automation]
mode = "suggest"              # Global default: suggest changes, don't apply
check_interval = "6h"
notify = ["admin@example.com"]

[automation.security]
mode = "auto"                 # Auto-apply security updates
within = "24h"                # Must be applied within 24 hours
severities = ["critical", "high"]
reboot = "suggest"            # Suggest reboot if needed

[automation.orphans]
mode = "suggest"
after = "30d"                 # Grace period before suggesting removal
keep = ["libfoo"]             # Never remove these even if orphaned

[automation.updates]
mode = "disabled"             # Don't auto-update regular packages
frequency = "weekly"
window = "02:00-06:00"        # Maintenance window
exclude = ["kernel"]          # Never auto-update kernel

[automation.major_upgrades]
require_approval = true       # Always ask for major version changes
allow_auto = ["nodejs"]       # Exception: auto-upgrade nodejs

[automation.repair]
integrity_check = true        # Periodic file integrity verification
check_interval = "24h"
auto_restore = true           # Auto-fix corrupted files from CAS

[[automation.repair.rollback_triggers]]
name = "nginx-health"
command = "curl -f localhost/health"
timeout = "10s"
failure_window = "5m"         # Monitor for 5 min after changes
auto_rollback = true          # Auto-rollback if health check fails
```

#### Automation Modes

| Mode | Behavior |
|------|----------|
| `suggest` | Detect issues and notify, but wait for confirmation (default) |
| `auto` | Automatically apply changes without confirmation |
| `disabled` | Don't check for this category at all |

Each category inherits the global mode unless overridden.

#### Rollback Triggers

Rollback triggers are health checks that run after changes are applied. If the health check fails within the `failure_window`, the system automatically reverts to the previous state. This provides a safety net for automated updates.

### 3.12 Federation in the Model

The `[federation]` section configures CAS chunk sharing directly in the model file:

```toml
[federation]
enabled = true
tier = "leaf"                 # leaf, cell_hub, or region_hub
cell_hubs = ["http://rack-cache.local:7891"]
region_hubs = ["https://remi.conary.io:7891"]
enable_mdns = true            # Auto-discover LAN peers
prefer_cell = true            # Try LAN before WAN
```

Federation is covered in detail in [Chapter 8 -- Advanced Topics](#8-advanced-topics).

### 3.13 Workflow Summary

The typical model workflow:

```
1. Snapshot current state:    conary model snapshot --output baseline.toml
2. Edit the model:            vim /etc/conary/system.toml
3. Preview changes:           conary model diff
4. Lock remote includes:      conary model lock
5. Apply:                     conary model apply
6. Verify:                    conary model check
7. Monitor drift:             conary model check (via cron/systemd timer)
8. Publish for others:        conary model publish --name my-stack --version 1.0
```

---

## 4. CCS Package Format

CCS (Conary Component Specification) is Conary's native package format. While Conary can install RPM, DEB, and Arch packages directly, CCS provides additional capabilities: content-defined chunking for delta updates, a Merkle tree for content integrity, declarative hooks instead of imperative scriptlets, and a build policy engine for automated quality enforcement.

### 4.1 Package Structure

A `.ccs` file is a gzip-compressed tar archive with this layout:

```
package.ccs
├── MANIFEST.cbor        # Binary manifest (CBOR-encoded, authoritative)
├── MANIFEST.toml        # Human-readable manifest (for debugging)
├── SIGNATURE            # Ed25519 signature (optional)
├── components/
│   ├── runtime.json     # File list for :runtime component
│   ├── lib.json         # File list for :lib component
│   ├── config.json      # File list for :config component
│   ├── devel.json       # File list for :devel component (if present)
│   └── ...
└── objects/
    ├── ab/
    │   └── ab3def...    # Content blob (file or chunk, by SHA-256)
    └── cd/
        └── cd1234...
```

The binary manifest (`MANIFEST.cbor`) is the authoritative record. It contains:
- Package metadata (name, version, description, license)
- Provides and requires declarations
- Component references (name -> hash of component JSON)
- Declarative hook definitions
- Build provenance information
- A **Merkle root** -- the SHA-256 root of a tree built from all content hashes

The Merkle root provides a single hash that covers every file in the package. Changing any file changes the root, making tamper detection efficient.

### 4.2 The Manifest (ccs.toml)

The CCS manifest is the source-of-truth for building a package. It uses TOML:

```toml
[package]
name = "myapp"
version = "1.2.3"
description = "My application"
license = "MIT"
homepage = "https://example.com/myapp"

[package.platform]
os = "linux"
arch = "x86_64"
libc = "gnu"

[package.authors]
maintainers = ["Jane Doe <jane@example.com>"]
upstream = "https://github.com/example/myapp"
```

#### Provides and Requires

```toml
[provides]
capabilities = ["cli-tool", "json-parsing"]
sonames = ["libmyapp.so.1"]
binaries = ["/usr/bin/myapp"]
pkgconfig = ["myapp"]

[requires]
capabilities = [
    "glibc",
    { name = "tls", version = ">=1.2" },
]
packages = [
    { name = "openssl", version = ">=3.0" },
]

[suggests]
capabilities = ["systemd"]
```

Capabilities can be simple strings or versioned objects. The `sonames`, `binaries`, and `pkgconfig` fields in `[provides]` are usually auto-detected during build, but can be declared explicitly.

#### Components

```toml
[components]
default = ["runtime", "lib", "config"]

# Override automatic classification for specific files
[components.files]
"/usr/bin/helper" = "lib"

# Override by glob pattern
[[components.overrides]]
path = "/usr/share/myapp/plugins/*"
component = "runtime"
```

#### Declarative Hooks

CCS replaces imperative scriptlets (bash scripts that run as root) with declarative hooks that describe *what* should happen, not *how*:

```toml
# System users
[[hooks.users]]
name = "myapp"
system = true
home = "/var/lib/myapp"
shell = "/sbin/nologin"
group = "myapp"

# System groups
[[hooks.groups]]
name = "myapp"
system = true

# Directories with specific ownership
[[hooks.directories]]
path = "/var/lib/myapp"
mode = "0750"
owner = "myapp"
group = "myapp"

# Systemd units
[[hooks.systemd]]
unit = "myapp.service"
enable = false

# tmpfiles.d entries
[[hooks.tmpfiles]]
type = "d"
path = "/run/myapp"
mode = "0755"
owner = "myapp"
group = "myapp"

# sysctl settings
[[hooks.sysctl]]
key = "net.core.somaxconn"
value = "4096"
only_if_lower = true

# Alternatives
[[hooks.alternatives]]
name = "editor"
path = "/usr/bin/myapp-edit"
priority = 30

# Service management
[[hooks.services]]
name = "myapp"
action = "enable"
```

| Hook Type | Purpose |
|-----------|---------|
| `users` | Create system users (sysusers-style) |
| `groups` | Create system groups |
| `directories` | Create directories with permissions |
| `systemd` | Enable/disable systemd units |
| `tmpfiles` | tmpfiles.d entries for runtime directories |
| `sysctl` | Kernel parameter tuning |
| `alternatives` | update-alternatives entries |
| `services` | Enable/disable/start/stop services |

Declarative hooks are safer than scriptlets because:
1. They are idempotent -- running them twice produces the same result
2. They can be validated before execution
3. They can be sandboxed with fine-grained control
4. They can be reverted (users deleted, services disabled, etc.)

#### Configuration Files

```toml
[config]
files = ["/etc/myapp/config.toml", "/etc/myapp/logging.yaml"]
noreplace = true    # Don't overwrite user-modified configs during update
```

#### Redirects

Packages can declare how they relate to other packages for clean evolution:

```toml
[[redirects.renames]]
old_name = "myapp-legacy"
message = "Renamed from myapp-legacy in v2.0"

[[redirects.obsoletes]]
package = "old-myapp"
version = "<1.0"
message = "old-myapp is no longer maintained"

[[redirects.merges]]
package = "myapp-extras"
message = "Extras merged into main package"

[[redirects.splits]]
from_package = "monolithic-myapp"
component = "core"
```

#### Build Policies

```toml
[policy]
reject_paths = ["/home/*", "/tmp/*", "/root/*"]
strip_binaries = true
normalize_timestamps = true
compress_manpages = true

[policy.fix_shebangs]
"/usr/bin/env python" = "/usr/bin/python3"
"/usr/bin/env python2" = "/usr/bin/python3"
```

#### Legacy Format Generation

A single CCS manifest can generate packages for multiple formats:

```toml
[legacy.rpm]
group = "Applications/System"
requires = ["systemd"]

[legacy.deb]
section = "utils"
priority = "optional"
depends = ["systemd"]

[legacy.arch]
groups = ["system"]
```

### 4.3 Building Packages

#### Initialize a New Project

```bash
conary ccs init                          # Create ccs.toml in current dir
conary ccs init --name myapp --version 1.0.0
conary ccs init --force                  # Overwrite existing
```

#### Build

```bash
conary ccs build                         # Build from ./ccs.toml
conary ccs build --output ./dist         # Custom output directory
conary ccs build --source /path/to/files # Specify source directory
conary ccs build --target ccs            # CCS format (default)
conary ccs build --target deb            # Generate .deb
conary ccs build --target rpm            # Generate .rpm
conary ccs build --target all            # All formats
conary ccs build --no-classify           # Skip component auto-classification
conary ccs build --no-chunked            # Disable CDC chunking
conary ccs build --dry-run               # Preview without building
```

#### Content-Defined Chunking

By default, CCS packages use FastCDC (Fast Content-Defined Chunking) to split file content into variable-size chunks:

| Parameter | Value |
|-----------|-------|
| Minimum chunk | 16 KB |
| Average chunk | 64 KB |
| Maximum chunk | 256 KB |

Files smaller than the minimum chunk size are stored as whole blobs. Chunking provides two key benefits:

1. **Intra-package deduplication**: If the same content appears in multiple files (common with locale data, icons, etc.), it is stored once
2. **Cross-version delta efficiency**: When a file changes between versions, most chunks remain identical -- the client only downloads new chunks

The `--no-chunked` flag disables chunking, storing files as whole blobs. This produces slightly smaller packages but loses delta efficiency.

### 4.4 Build Policies

The policy engine runs automatically during builds, applying transformations and validations to each file:

| Policy | What It Does |
|--------|--------------|
| `DenyPaths` | Rejects files matching forbidden glob patterns (e.g., `/home/*`) |
| `StripBinaries` | Strips debug symbols from ELF executables and shared libraries |
| `NormalizeTimestamps` | Sets all file timestamps to `SOURCE_DATE_EPOCH` for reproducible builds |
| `FixShebangs` | Rewrites script shebangs (e.g., `/usr/bin/env python` -> `/usr/bin/python3`) |
| `CompressManpages` | Gzip-compresses uncompressed man pages |

Policies are applied in sequence. Each policy can:
- **Keep** the file unchanged
- **Replace** the file content (e.g., strip debug symbols)
- **Skip** the file (remove from package)
- **Reject** the entire build with an error

### 4.5 Installing CCS Packages

```bash
conary ccs install ./myapp-1.2.3.ccs
conary ccs install ./myapp-1.2.3.ccs --components runtime,lib
conary ccs install ./myapp-1.2.3.ccs --allow-unsigned
conary ccs install ./myapp-1.2.3.ccs --policy trust.toml
conary ccs install ./myapp-1.2.3.ccs --dry-run
```

CCS installation verifies the package signature and Merkle root before extracting any files. Component selection lets you install only what you need -- for example, install `:runtime` and `:lib` on a server, and add `:devel` only on build machines.

### 4.6 Signing and Verification

CCS uses Ed25519 signatures for package authentication.

#### Key Generation

```bash
conary ccs keygen --output mykey
conary ccs keygen --output mykey --key-id "release@example.com"
```

This produces two files: `mykey.key` (private, 32 bytes) and `mykey.pub` (public, 32 bytes). Keys can be raw binary or hex-encoded.

#### Signing

```bash
conary ccs sign ./myapp-1.2.3.ccs --key mykey.key
conary ccs sign ./myapp-1.2.3.ccs --key mykey.key --output signed.ccs
```

The signature covers the CBOR-encoded binary manifest, which includes the Merkle root of all content. Signing the manifest transitively authenticates every file in the package.

#### Verification

```bash
conary ccs verify ./myapp-1.2.3.ccs
conary ccs verify ./myapp-1.2.3.ccs --policy trust.toml
conary ccs verify ./myapp-1.2.3.ccs --allow-unsigned
```

Verification checks:
1. The signature is valid for the manifest
2. The signing key is trusted (per trust policy)
3. The Merkle root matches the recomputed tree
4. Every file hash matches its content

### 4.7 Inspecting Packages

```bash
conary ccs inspect ./myapp-1.2.3.ccs           # Basic info
conary ccs inspect ./myapp-1.2.3.ccs --files    # File listing
conary ccs inspect ./myapp-1.2.3.ccs --hooks    # Hook definitions
conary ccs inspect ./myapp-1.2.3.ccs --deps     # Dependencies and provides
conary ccs inspect ./myapp-1.2.3.ccs --format json  # JSON output
```

### 4.8 OCI Export

CCS packages can be exported to OCI container image format for use with container runtimes:

```bash
conary ccs export ./myapp-1.2.3.ccs --output myapp.tar --format oci
conary ccs export ./base.ccs ./myapp.ccs --output image.tar --format oci
```

The mapping is:
- CCS chunks become OCI layers (blobs)
- CCS manifest becomes the OCI image manifest
- Component data becomes OCI annotations
- `artifactType: application/vnd.conary.package.v1`

Multiple CCS packages can be composed into a single OCI image, with each package's files becoming a separate layer.

### 4.9 Ephemeral Environments

CCS supports temporary package availability without permanent installation, similar to `nix-shell` and `nix run`:

#### Shell

```bash
conary ccs shell nginx redis             # Shell with nginx and redis available
conary ccs shell nginx --shell /bin/zsh  # Use specific shell
conary ccs shell nginx --env PORT=8080   # Set environment variables
conary ccs shell nginx --keep            # Keep temp dir after exit (debugging)
```

Creates an ephemeral environment where the specified packages are available. When the shell exits, the temporary environment is cleaned up.

#### Run

```bash
conary ccs run nginx -- nginx -t         # Run nginx config test
conary ccs run python3 -- python3 -c "print('hello')"
```

Executes a single command with the specified package available, then cleans up.

### 4.10 Enhancement

Packages converted from legacy formats (RPM/DEB/Arch) can be retroactively enhanced with CCS features:

```bash
conary ccs enhance --all-pending         # Enhance all pending packages
conary ccs enhance --trove-id 42         # Enhance a specific trove
conary ccs enhance --update-outdated     # Re-enhance with latest version
conary ccs enhance --types capabilities,provenance  # Specific enhancements
conary ccs enhance --force               # Re-enhance even if already done
conary ccs enhance --stats               # Show enhancement statistics
conary ccs enhance --dry-run             # Preview
```

Enhancement types:
- **capabilities**: Infer what system resources the package needs (network, filesystem, syscalls)
- **provenance**: Extract source origin, build environment, and signature information
- **subpackages**: Detect relationships between subpackages (e.g., `nginx`, `nginx-devel`, `nginx-doc`)

### 4.11 Lockfiles

CCS lockfiles (`ccs.lock`) capture the exact resolved dependency state for reproducible builds:

```toml
[metadata]
version = 1
generated = "2026-03-03T12:00:00Z"
generator = "conary 0.2.0"
package = "myapp"
package_version = "1.0.0"

[[dependencies]]
name = "openssl"
version = "3.1.4"
content_hash = "sha256:abc123..."
source = "https://repo.example.com"
kind = "runtime"

[[dependencies]]
name = "gcc"
version = "13.2.0"
content_hash = "sha256:def456..."
kind = "build"
```

Dependencies are classified by kind: `runtime` (needed to run), `build` (needed to compile), or `test` (needed for testing). Platform-specific overrides are supported for cross-compilation scenarios.

### 4.12 Provenance (Package DNA)

CCS manifests can carry full provenance information -- the complete lineage of a package:

```toml
[provenance]
upstream_url = "https://github.com/example/myapp/archive/v1.2.3.tar.gz"
upstream_hash = "sha256:abc123..."
git_commit = "a1b2c3d4e5f6..."
recipe_hash = "sha256:def456..."
build_timestamp = "2026-03-03T12:00:00Z"
host_arch = "x86_64"
merkle_root = "sha256:789abc..."
dna_hash = "sha256:final..."

[[provenance.patches]]
url = "https://example.com/security-fix.patch"
hash = "sha256:patch1..."
author = "security@example.com"
reason = "CVE-2026-1234 fix"

[[provenance.build_deps]]
name = "gcc"
version = "13.2.0"
dna_hash = "sha256:gcc-dna..."

[[provenance.signatures]]
keyid = "release@example.com"
sig = "base64-encoded-signature"
scope = "build"
timestamp = "2026-03-03T12:00:00Z"
```

The **DNA hash** is a unique identifier computed from the entire provenance chain: source hashes, patch hashes, build dependency DNA hashes, and content hashes. Two packages with the same DNA hash are provably identical in their lineage, even if built independently.

---

## 5. Recipe System

The recipe system is Conary's build-from-source infrastructure, following the culinary tradition of the original Conary: packages are built by **cooking** **recipes** in an isolated **kitchen**. This chapter covers the recipe TOML format, the hermetic build architecture, cross-compilation support, dependency graph resolution, build caching, and PKGBUILD conversion.

### 5.1 Culinary Terminology

Every part of the build pipeline uses cooking metaphors, a tradition inherited from the original Conary:

| Term | Meaning |
|------|---------|
| **Recipe** | TOML file describing how to build a package |
| **Kitchen** | The isolated build environment |
| **Cook** | Build a package from a recipe |
| **Ingredients** | Source archives and patches |
| **Prep** | Fetch and cache sources (network allowed) |
| **Simmer** | Run the actual build (network blocked) |
| **Plate** | Package the result as CCS |

### 5.2 Recipe Format

Recipes are TOML files with the following sections:

```toml
[package]
name = "nginx"
version = "1.24.0"
release = "1"                          # Rebuild number (default: "1")
summary = "High-performance HTTP server"
description = "Full description..."
license = "BSD-2-Clause"               # SPDX identifier
homepage = "https://nginx.org"

[source]
archive = "https://nginx.org/download/nginx-%(version)s.tar.gz"
checksum = "sha256:77a2541637b92..."
signature = "https://nginx.org/download/nginx-%(version)s.tar.gz.asc"  # Optional GPG sig
extract_dir = "nginx-1.24.0"          # Override auto-detected source dir

# Additional source archives (multi-source builds)
[[source.additional]]
url = "https://example.com/extra-module.tar.gz"
checksum = "sha256:def456..."
extract_to = "modules/extra"          # Relative to main source

[build]
requires = ["openssl:devel", "pcre:devel", "zlib:devel"]  # Runtime deps
makedepends = ["gcc", "make", "pkgconf"]                   # Build-only deps
setup = "autoreconf -fi"                                   # Pre-configure
configure = "./configure --prefix=/usr --with-http_ssl_module"
make = "make -j%(jobs)s"
check = "make test"                                         # Optional test step
install = "make install DESTDIR=%(destdir)s"
post_install = "strip --strip-unneeded %(destdir)s/usr/bin/nginx"
workdir = "subdir"                                          # Build in a subdirectory
script_file = "build.lua"                                   # Alternative: Lua build script
jobs = 4                                                    # Override auto-detected parallelism
environment = { CFLAGS = "-O2 -fPIC", LDFLAGS = "-Wl,-z,now" }

[patches]
files = [
    { file = "nginx-1.24-fix-headers.patch", strip = 1 },
    { file = "https://example.com/remote.patch", checksum = "sha256:...", strip = 1 },
]

[components]
devel = ["/usr/include/**"]            # Override component classification
doc = ["/usr/share/doc/**"]
lib = ["/usr/lib/*.so*"]
exclude = ["/usr/share/info/dir"]      # Files to omit from packaging

[variables]
jobs = "4"
ssl_version = "3.0"
```

**Variable substitution** uses the `%(name)s` syntax throughout:
- `%(name)s` -- package name
- `%(version)s` -- package version
- `%(destdir)s` -- install destination directory
- Any key from `[variables]` section

**Checksum formats**: `sha256:...`, `sha512:...`, or `blake3:...`. The parser rejects other algorithms.

### 5.3 The Cook Command

```
conary cook <recipe.toml>              # Build with isolation (default)
conary cook recipe.toml --fetch-only   # Pre-fetch sources for offline build
conary cook recipe.toml --validate-only # Check recipe without building
conary cook recipe.toml --hermetic     # Maximum isolation, no host mounts
conary cook recipe.toml --no-isolation # Unsafe: disable container sandbox
conary cook recipe.toml -o ./output    # Specify output directory
conary cook recipe.toml -j 8          # Override parallel job count
conary cook recipe.toml --keep-builddir # Don't clean up (for debugging)
conary cook recipe.toml --source-cache /mnt/sources  # Custom source cache
```

### 5.4 Hermetic Build Architecture

Conary's build pipeline follows a two-phase model inspired by BuildStream, separating network access from build execution:

```
     Phase 1: FETCH (network ALLOWED)         Phase 2: BUILD (network BLOCKED)
    +-------------------------------+        +--------------------------------+
    | Download source archives      |        | Extract cached sources         |
    | Download patches              |   ->   | Apply patches                  |
    | Verify checksums              |        | Run setup/configure/make       |
    | Cache locally                 |        | Run install to destdir         |
    +-------------------------------+        | Package result as CCS          |
                                             +--------------------------------+
```

This separation guarantees reproducibility: once all sources are cached, the build phase has no network access and cannot introduce external dependencies. The `--fetch-only` flag enables pre-fetching for air-gapped builds.

### 5.5 Kitchen Configuration

The Kitchen is configured via `KitchenConfig` with these options:

| Option | Default | Description |
|--------|---------|-------------|
| `source_cache` | `/var/cache/conary/sources` | Directory for cached source archives |
| `timeout` | 3600s (1 hour) | Maximum build time |
| `jobs` | auto (CPU count) | Parallel build jobs |
| `allow_network` | `false` | Allow network during build (defeats hermeticity) |
| `keep_builddir` | `false` | Preserve temp build directory |
| `use_isolation` | `true` | Enable container sandbox |
| `memory_limit` | 4 GB | Memory limit for isolated builds |
| `cpu_time_limit` | 0 (none) | CPU time limit in seconds |
| `pristine_mode` | `false` | No host system mounts at all |
| `sysroot` | none | Sysroot for pristine/bootstrap builds |
| `auto_makedepends` | `false` | Auto-install build dependencies |
| `cleanup_makedepends` | `true` | Remove auto-installed deps after build |

Pre-built configurations:
- `KitchenConfig::default()` -- standard isolated build
- `KitchenConfig::for_bootstrap(sysroot)` -- pristine mode, no host contamination
- `KitchenConfig::with_auto_makedepends(cleanup)` -- auto-resolve build deps

### 5.6 Build Phases in Detail

The full cooking process runs through six phases:

**Phase 0: Makedepends** -- If `auto_makedepends` is enabled, the Kitchen checks for missing build dependencies and installs them via a `MakedependsResolver`. The resolver is a trait, allowing any package installation backend:

```rust
pub trait MakedependsResolver: Send + Sync {
    fn check_missing(&self, deps: &[&str]) -> Result<Vec<String>>;
    fn install(&self, deps: &[String]) -> Result<Vec<String>>;
    fn cleanup(&self, installed: &[String]) -> Result<()>;
}
```

**Phase 1: Prep** -- Fetch all ingredients (source archives, additional sources, remote patches). Each download is checksum-verified and cached by its checksum as the cache key. On subsequent builds, cached sources are verified before reuse; mismatches trigger re-download.

**Phase 2: Unpack and Patch** -- Extract the source archive into a temp directory. If the archive contains a single top-level directory, that becomes the source root. Apply patches in order with the configured strip level.

**Phase 3: Simmer** -- Execute build commands in order: `setup` -> `configure` -> `make` -> `check` -> `install` -> `post_install`. Each command runs through variable substitution (replacing `%(version)s`, `%(destdir)s`, etc.). If `use_isolation` is true, commands run inside a container sandbox with:
- PID, UTS, IPC, mount, and network namespace isolation
- Read-only bind mounts for host system directories (`/usr`, `/lib`, `/bin`, etc.)
- Read-only source directory, writable destdir and build directory
- Network blocked by default (no `/etc/resolv.conf` mounted unless `allow_network` is set)
- Resource limits (memory, CPU time)
- In **pristine mode**, no host mounts at all -- only the specified sysroot

Test failures (`check`) generate warnings but don't fail the build.

**Phase 4: Plate** -- Build a CCS package from the destdir contents. Creates a `CcsManifest` with metadata from the recipe, constructs files with content hashing, and writes the final `.ccs` package. This phase also captures build provenance (see Section 5.10).

**Phase 5: Cleanup** -- If `auto_makedepends` was used, uninstall any packages that were temporarily installed for the build. Cleanup failures produce warnings but don't fail the build.

### 5.7 Container Isolation

By default, builds run inside a Linux namespace container (`Sandbox`):

| Feature | Behavior |
|---------|----------|
| PID namespace | Build sees only its own processes |
| Network namespace | Only loopback, no external access |
| Mount namespace | Private `/tmp`, controlled bind mounts |
| UTS namespace | Hostname set to `conary-build` |
| Source directory | Mounted read-only |
| Destdir | Mounted writable |
| Host system dirs | Read-only (`/usr`, `/lib`, `/bin`, `/sbin`) |
| `/etc/resolv.conf` | Only mounted if `allow_network` is true |

**Pristine mode** goes further: no host directories are mounted at all. Instead, a sysroot directory provides the toolchain. This is critical for bootstrap builds where host contamination must be avoided.

### 5.8 Cross-Compilation Support

The `[cross]` section configures cross-compilation:

```toml
[cross]
target = "x86_64-conary-linux-gnu"     # Target triple
sysroot = "/opt/sysroot/stage0"        # Target libraries/headers
cross_tools = "/opt/cross/bin"         # Cross-compiler directory
stage = "stage1"                       # Bootstrap stage
tool_prefix = "x86_64-conary-linux-gnu"  # Tool name prefix

# Individual tool overrides
cc = "/custom/path/clang"
cxx = "/custom/path/clang++"
# ar, ld, ranlib, nm, strip also overridable
```

When a `[cross]` section is present, the Kitchen sets cross-compilation environment variables:

| Variable | Value |
|----------|-------|
| `CC` | `{cross_tools}/{tool_prefix}-gcc` (or override) |
| `CXX` | `{cross_tools}/{tool_prefix}-g++` (or override) |
| `AR`, `LD`, `RANLIB`, `NM`, `STRIP` | Prefixed equivalents |
| `TARGET` | The target triple |
| `CROSS_COMPILE` | `{tool_prefix}-` |
| `SYSROOT` | The sysroot path |
| `CFLAGS`, `CXXFLAGS`, `LDFLAGS` | `--sysroot={sysroot}` appended |
| `CONARY_STAGE` | `stage0`, `stage1`, `stage2`, or `final` |

### 5.9 Bootstrap Stages

Conary supports multi-stage bootstrap builds following the LFS 12.4 methodology. The pipeline proceeds through a well-defined sequence, with optional stages that can be skipped for faster iteration:

```
Stage 0 --> Stage 1 --> Stage 2 (optional) --> BaseSystem --> Conary (optional) --> Image
```

| Stage | Description | Example |
|-------|-------------|---------|
| `stage0` | Cross-compiled from host toolchain | Minimal binutils 2.45 + GCC 15.2.0 targeting new system |
| `stage1` | Built with stage0 tools, runs on target | Self-hosted compiler, may still link some host libs |
| `stage2` | Pure rebuild with stage1 compiler (optional) | Eliminates all host contamination |
| `base` | Core userspace with per-package checkpointing | coreutils, bash, util-linux, systemd |
| `conary` | Build Conary itself for self-hosting (optional) | Self-managing system |
| `image` | Bootable disk image via systemd-repart | Raw, qcow2, or ISO output |

Stage 2 is optional but recommended for production images -- it guarantees that every binary was compiled by a Conary-native compiler with no host system contamination. The Conary stage builds Conary itself using the Rust toolchain from earlier stages, producing a self-managing system.

All source downloads enforce SHA-256 checksum verification. Placeholder checksums are no longer accepted.

The **`StageRegistry`** holds configurations for all stages, with a convenience constructor for standard bootstrap layouts:

```rust
// Creates stage0/stage1/stage2 under /opt/bootstrap
let registry = StageRegistry::bootstrap_standard(
    Path::new("/opt/bootstrap"),
    "x86_64-conary-linux-gnu"
);
```

Build sandboxing uses `ContainerConfig::pristine_for_bootstrap()` to create a minimal namespace environment with no host filesystem leakage. The `RecipeGraph` determines build order with automatic cycle detection and breaking (e.g., the gcc/glibc circular dependency).

A **dry-run** mode validates the entire pipeline -- checking prerequisites, verifying checksums, and confirming dependency ordering -- without writing any files:

```bash
conary bootstrap dry-run
```

### 5.10 Build Provenance Capture

Every cook operation automatically captures provenance data through the `ProvenanceCapture` system. Data is collected across all build phases:

| Phase | Captured Data |
|-------|--------------|
| Prep | Source URL, source hash, fetch timestamp |
| Patch | Patch source, content hash (SHA-256), strip level, author |
| Simmer | Build timestamp, host arch, kernel version, isolation mode |
| Plate | File hashes (SHA-256 per file), Merkle root |

From this data, a **DNA hash** is computed -- a single SHA-256 digest covering:
1. Source URL and hash
2. Git commit (if from git)
3. All patch content hashes
4. Recipe file hash
5. Build dependency names, versions, and their own DNA hashes
6. Merkle root of output file hashes

Two independently built packages with the same DNA hash are provably identical in their complete lineage. The DNA hash and all provenance data are embedded in the CCS manifest.

### 5.11 Build Caching

The `BuildCache` stores built CCS packages indexed by a composite cache key:

```
cache_key = SHA-256(recipe_hash || toolchain_hash [|| deps_hash])
```

Where:
- **recipe_hash** covers: name, version, release, source URL, checksum, patches, configure/make/install commands, environment variables, and dependencies
- **toolchain_hash** covers: compiler version, linker version, target triple, sysroot path, and bootstrap stage
- **deps_hash** (optional, for BuildStream-grade reproducibility) covers: content hashes of all installed build dependencies via `DependencyHashes`

Cache entries are stored in sharded directories (`{cache_dir}/{key[0:2]}/{key}.ccs`) with metadata files.

**Cache operations:**

```rust
// Simple cache (recipe + toolchain)
let key = cache.cache_key(&recipe, &toolchain);

// BuildStream-grade cache (includes dependency content hashes)
let mut deps = DependencyHashes::new();
deps.add("gcc", "sha256:content-hash-of-gcc...");
deps.add("make", "sha256:content-hash-of-make...");
let key = cache.cache_key_with_deps(&recipe, &toolchain, Some(&deps));
```

**Cache eviction** uses LRU (oldest-first by modification time) when `max_size` is exceeded. Entries also expire after `max_age` (default: 30 days). Integrity verification checks that cached `.ccs` files are non-empty.

**Cached cooking flow:**

```
Kitchen::cook_cached()
  |
  +-- Check cache by key
  |     |
  |     +-- HIT: copy cached .ccs to output dir, return
  |     |
  |     +-- MISS: fall through
  |
  +-- Kitchen::cook() (full build)
  |
  +-- Store result in cache for next time
```

**Batch cooking** (`cook_batch`) processes multiple recipes in order, using the cache to skip unchanged packages.

### 5.12 Dependency Graph

The `RecipeGraph` provides topological sorting for determining build order:

```rust
let mut graph = RecipeGraph::new();
graph.add_recipe("glibc", &["gcc", "linux-headers"]);
graph.add_recipe("gcc", &["glibc", "binutils"]);
graph.add_recipe("binutils", &["glibc"]);
graph.add_recipe("linux-headers", &[]);
```

**Circular dependencies** (common in bootstrap scenarios like gcc <-> glibc) are detected and can be broken:

```rust
// Manual: specify which edge to ignore
graph.mark_bootstrap_edge("glibc", "gcc");

// Automatic: uses heuristics from LFS/Gentoo patterns
let broken_edges = graph.auto_break_cycles();

// Inspect: find all cycles
let cycles = graph.find_cycles();
```

The auto-break heuristic knows common patterns (glibc->gcc, musl->gcc, libstdc++->glibc, perl->glibc, python->glibc). For unknown cycles, it breaks the edge from the node with the fewest dependents.

**Transitive queries:**

```rust
// All packages X transitively depends on
let deps = graph.transitive_dependencies("gcc");

// All packages that transitively depend on X
let rdeps = graph.transitive_dependents("glibc");
```

### 5.13 Bootstrap Plans

A `BootstrapPlan` automates multi-stage bootstrap sequencing from a recipe graph:

```rust
let plan = BootstrapPlan::from_graph(&mut graph)?;

for phase in &plan.phases {
    println!("{}: {} recipes (stage {:?})", phase.name, phase.recipes.len(), phase.stage);
}
// stage0: 2 recipes (Stage0)    -- cross-compiled, no deps
// stage1: 1 recipe  (Stage1)    -- packages from broken cycles
// final:  3 recipes (Final)     -- everything else
```

Phase assignment:
- **stage0**: packages with zero dependencies (or only `linux-headers`)
- **stage1**: packages that were part of dependency cycles
- **final**: all remaining packages

### 5.14 PKGBUILD Conversion

Conary can convert Arch Linux PKGBUILD files to recipe format:

```
conary convert-pkgbuild PKGBUILD       # Convert and print recipe TOML
```

The converter handles:
- Variable extraction (`pkgname`, `pkgver`, `pkgrel`, `pkgdesc`, `url`, `license`)
- Array parsing (`depends`, `makedepends`, `source`, `sha256sums`)
- Function extraction (`build()`, `package()`, `prepare()`, `check()`)
- URL variable substitution (`$pkgver` -> `%(version)s`, `$pkgname` -> `%(name)s`)
- Build command splitting (detects `./configure` vs `cmake` vs `meson` for the configure step, `make` vs `ninja` for the build step)
- Patch detection from source arrays (`.patch` and `.diff` files)
- Install command conversion (`$pkgdir` -> `%(destdir)s`)

**Limitations:**
- Split packages (`pkgname=(...)`) are not supported
- VCS packages (`-git`, `-svn`) need manual URL adjustment
- Dynamic `pkgver()` functions need manual version update
- Complex Bash expressions are simplified

### 5.15 Source Management

Source caching uses the checksum as the cache key:

```
/var/cache/conary/sources/
  sha256_77a2541637b92.../   # Cached archive, keyed by checksum
  sha256_abc123.../          # Another cached source
```

The `Kitchen::sources_cached()` method checks whether all sources (main archive, additional archives, remote patches) are available locally, enabling offline build verification.

**Fetch workflow:**
1. Compute cache key from checksum
2. If cached file exists, verify checksum before using (re-download on mismatch)
3. Download to `.tmp` file, verify checksum, atomically rename to final location
4. Return path to cached file

---

# 6. Remi Server

Remi is Conary's server-side component: an HTTP server that converts legacy Linux packages (RPM, DEB, Arch) into CCS format on demand, stores the results as content-addressed chunks, and serves them to clients through a CDN-friendly API. Where a traditional mirror network replicates entire repositories, Remi converts only what clients actually request, then caches the results indefinitely since chunks are immutable. The name is short for "repository middleware."

Feature-gated behind `--features server`, Remi adds roughly 20 submodules to the binary. A production instance runs at `packages.conary.io` on a Hetzner dedicated server (12 cores, 64 GB RAM, 2x 1 TB NVMe) behind Cloudflare.

```
src/server/
  mod.rs              ServerConfig, ServerState, run_server_from_config()
  config.rs           RemiConfig TOML parser (11 sections)
  routes.rs           Axum router: public + admin, middleware stack
  conversion.rs       ConversionService: download -> parse -> CCS -> store
  cache.rs            ChunkCache: two-level CAS, DB-backed LRU eviction
  bloom.rs            ChunkBloomFilter: fast negative lookups (~1.2 MB)
  handlers/
    chunks.rs         HEAD/GET/batch chunk serving, pull-through caching
    packages.rs       Package metadata + on-demand conversion trigger
    sparse.rs         Sparse HTTP index (crates.io-style)
    oci.rs            OCI Distribution Spec v2 compatibility layer
    ...               federation, search, models, tuf, detail, recipes
  r2.rs               Cloudflare R2 object storage
  search.rs           Tantivy full-text search engine
  analytics.rs        Buffered download analytics recorder
  metrics.rs          Prometheus-format server metrics
  security.rs         Rate limiter (token bucket) + ban list
  negative_cache.rs   TTL cache for 404 responses
  jobs.rs             Conversion job queue with semaphore control
  index_gen.rs        Repository index generation with Ed25519 signing
  lite.rs             Remi Lite zero-config LAN proxy
  delta_manifests.rs  Pre-computed chunk diffs between versions
  federated_index.rs  Multi-instance sparse index federation
  prewarm.rs          Background popularity-driven conversion
  popularity.rs       Upstream popularity data fetching
```

## 6.1 Architecture Overview

Remi runs two Axum HTTP servers concurrently:

- **Public API** (default `0.0.0.0:8080`): Chunk serving, package metadata, sparse index, search, OCI, federation, health checks, Prometheus metrics.
- **Admin API** (default `127.0.0.1:8081`): Conversion triggers, cache management, Bloom filter rebuild, recipe builds (SSRF-sensitive). Always bound to localhost -- access via SSH tunnel only.

Both servers share a single `ServerState` behind `Arc<RwLock<>>`:

```rust
// src/server/mod.rs
pub struct ServerState {
    pub config: ServerConfig,
    pub job_manager: JobManager,
    pub chunk_cache: ChunkCache,
    pub conversion_service: ConversionService,
    pub bloom_filter: Option<Arc<ChunkBloomFilter>>,
    pub http_client: reqwest::Client,
    pub metrics: Arc<ServerMetrics>,
    pub ban_list: Arc<BanList>,
    pub negative_cache: Arc<NegativeCache>,
    pub trusted_proxy_header: Option<String>,
    pub r2_store: Option<Arc<R2Store>>,
    pub r2_redirect: bool,
    pub search_engine: Option<Arc<SearchEngine>>,
    pub analytics: Option<Arc<AnalyticsRecorder>>,
    pub federated_config: Option<FederatedIndexConfig>,
    pub federated_cache: Option<Arc<FederatedIndexCache>>,
    pub inflight_fetches: Arc<DashMap<String, broadcast::Sender<()>>>,
}
```

The `run_server_from_config()` startup sequence:

1. Parse TOML config and validate
2. Create storage directories (`chunks/`, `metadata/`, `cache/`, `keys/`, etc.)
3. Initialize SQLite database if absent
4. Build `ServerState` with all subsystems
5. Initialize R2 storage (if enabled)
6. Initialize Tantivy search engine and rebuild index from DB in background
7. Start analytics recording loop (5-minute flush interval)
8. Scan existing chunks into Bloom filter (async background task)
9. Configure federated index peers (if federation enabled)
10. Create public and admin routers with middleware stack
11. Start background tasks: LRU eviction loop, negative cache cleanup, pre-warming
12. Bind both TCP listeners and `tokio::select!` to serve

## 6.2 TOML Configuration

Remi uses a single TOML file (typically `/etc/conary/remi.toml`) parsed into `RemiConfig`:

```rust
// src/server/config.rs
pub struct RemiConfig {
    pub server: ServerSection,      // bind, admin_bind, workers, metrics, audit_log
    pub storage: StorageSection,    // root, eviction_threshold, max_cache_size
    pub upstream: HashMap<String, UpstreamSection>,  // per-distro sources
    pub conversion: ConversionSection,  // chunking, chunk_min/avg/max, strip_debug
    pub federation: FederationSection,  // tier, mTLS, peers
    pub security: SecuritySection,      // rate_limit, ban, CORS, trusted_proxy_header
    pub builder: BuilderSection,        // isolation, network_blocked
    pub r2: R2Section,                  // Cloudflare R2 bucket, write_through, redirect
    pub search: SearchSection,          // Tantivy index directory
    pub prewarm: PrewarmConfigSection,  // distros, top_n, interval
    pub web: WebSection,               // SvelteKit frontend path
}
```

A production config example (`deploy/remi.toml.example`):

```toml
[server]
bind = "127.0.0.1:8080"        # Behind nginx/Cloudflare
admin_bind = "127.0.0.1:8081"  # SSH tunnel only

[storage]
root = "/conary"
max_cache_size = "700GB"
eviction_threshold = 0.90
negative_cache_ttl = "15m"

[upstream.fedora]
metalink = "https://mirrors.fedoraproject.org/metalink"
releases = ["43"]
arches = ["x86_64"]
metadata_refresh = "6h"

[upstream.arch]
base_url = "https://archive.archlinux.org"
releases = ["latest"]
arches = ["x86_64"]

[upstream.ubuntu]
base_url = "http://archive.ubuntu.com/ubuntu"
releases = ["noble"]
arches = ["amd64"]

[conversion]
chunking = true
chunk_min = 16384     # 16 KB
chunk_avg = 65536     # 64 KB
chunk_max = 262144    # 256 KB
max_concurrent = 4

[r2]
enabled = true
bucket = "conary-chunks"
write_through = true

[security]
rate_limit = true
rate_limit_rps = 200
rate_limit_burst = 400
trusted_proxy_header = "CF-Connecting-IP"
ban_threshold = 20
ban_duration = "10m"
```

The config includes helper parsers for human-readable sizes (`parse_size("700GB")`) and durations (`parse_duration("15m")`). Validation catches invalid bind addresses, out-of-range eviction thresholds, chunk size ordering violations, and invalid federation tiers.

Storage directories are derived from the root path:

```
/conary/
  chunks/          # CAS chunk objects
  converted/       # Converted CCS packages
  built/           # Recipe-built packages
  bootstrap/       # Bootstrap images
  build/           # Recipe build work directory
  metadata/        # SQLite database (conary.db)
  manifests/       # Signed repository manifests
  keys/            # Signing keys, mTLS certificates
  cache/           # Scratch/temp space
  search-index/    # Tantivy search index
```

## 6.3 On-Demand Conversion Pipeline

The core differentiator. When a client requests a package that hasn't been converted yet, Remi converts it in real time:

```
Client: GET /v1/fedora/packages/nginx
  |
  v
[Check DB: converted_packages table]
  |-- Already converted? Return metadata immediately (200 OK)
  |
  |-- Active job? Return job ID for polling (202 Accepted)
  |
  |-- Neither? Create new conversion job
        |
        v
      [JobManager] assigns a JobId, tracks status
        |
        v
      [Background task: run_conversion()]
        |
        v
      [ConversionService.convert_package()]
        |
        1. Query repository_packages for upstream metadata
        2. Download RPM/DEB/Arch package from upstream mirror
        3. Parse via unified PackageMetadata trait
        4. Convert to CCS via LegacyConverter (FastCDC chunking)
        5. Store each chunk in CAS (atomic write: .tmp -> rename)
        6. Write-through to R2 (if enabled)
        7. Record in converted_packages table
        8. Update job status to Ready
```

The `ConversionService` (`src/server/conversion.rs`) orchestrates this pipeline:

```rust
pub struct ConversionService {
    chunk_dir: PathBuf,
    cache_dir: PathBuf,
    db_path: PathBuf,
    r2_store: Option<Arc<R2Store>>,
}
```

Package type detection uses the distro name: `fedora` -> RPM, `ubuntu`/`debian` -> DEB, `arch` -> Arch. Each format's parser (`RpmPackage`, `DebPackage`, `ArchPackage`) implements the `PackageFormat` trait, producing a `PackageMetadata` with files, dependencies, and scripts. The `LegacyConverter` then chunks the content using FastCDC boundaries (16 KB min, 64 KB avg, 256 KB max).

Filename sanitization prevents path traversal -- `safe_ccs_filename()` passes both the package name and version through `sanitize_filename()` before constructing the output path.

The 202 Accepted pattern lets clients poll for completion:

```
POST triggers conversion -> returns { "job_id": "abc-123", "status": "converting" }
GET /v1/jobs/abc-123     -> returns { "status": "ready" } or { "status": "converting" }
```

## 6.4 Content-Addressed Chunk Storage

Every chunk is stored by its SHA-256 hash in a two-level directory structure:

```
/conary/chunks/objects/
  ab/
    cdef0123456789...    # Full hash minus first 2 chars
  01/
    23456789abcdef...
```

The `ChunkCache` (`src/server/cache.rs`) manages this store:

```rust
pub struct ChunkCache {
    chunk_dir: PathBuf,    // Root: /conary/chunks
    max_bytes: u64,        // 700 GB default
    ttl_days: u32,         // 30 days default
    db_path: PathBuf,      // For chunk_access table
}
```

**Chunk path computation** splits the hash: `hash[0:2]` becomes the subdirectory prefix, `hash[2:]` becomes the filename. This prevents any single directory from accumulating millions of entries.

**Atomic writes** prevent partial chunks on crash:

```rust
pub async fn store_chunk(&self, hash: &str, data: &[u8]) -> Result<PathBuf> {
    let path = self.chunk_path(hash);
    let temp_path = path.with_extension("tmp");
    tokio::fs::write(&temp_path, data).await?;
    tokio::fs::rename(&temp_path, &path).await?;
    // Record in DB for LRU tracking
    self.record_access(hash).await?;
    Ok(path)
}
```

**Access tracking** uses the `chunk_access` database table. Every read updates `last_accessed` and increments `access_count`, feeding the LRU eviction algorithm.

## 6.5 LRU Cache Eviction

The `run_eviction_loop()` runs hourly in the background with a two-phase approach:

**Phase 1: TTL eviction** -- Remove chunks not accessed within the TTL period (default 30 days). These are stale regardless of cache pressure.

**Phase 2: LRU size-based eviction** -- If total cache exceeds `max_bytes`, sort remaining chunks by `last_accessed` ascending (least recently used first) and evict until under the threshold.

Both phases skip **protected chunks** -- chunks belonging to actively-referenced packages. The `protect_chunks()` / `unprotect_chunks()` mechanism prevents evicting chunks that a client is currently downloading.

Cache statistics are exposed via the admin API:

```json
{
  "total_bytes": 524288000000,
  "total_size_human": "488.3 GB",
  "max_bytes": 751619276800,
  "chunk_count": 847293,
  "usage_percent": 69.7,
  "stale_chunks": 12043,
  "protected_chunks": 156,
  "ttl_days": 30
}
```

The eviction loop also cleans up completed conversion jobs older than 1 hour from the `JobManager`.

## 6.6 Bloom Filter (DoS Protection)

Chunks are content-addressed, meaning any 64-character hex string is a valid-looking request. An attacker could flood the server with requests for non-existent chunks, forcing disk I/O for each one. The Bloom filter eliminates this:

```rust
// src/server/bloom.rs
pub struct ChunkBloomFilter {
    bits: Vec<AtomicU64>,      // Bit array using atomic u64 words
    num_bits: usize,           // ~9.6 million for 1M chunks at 1% FP
    num_hashes: usize,         // Optimal k = 7 for these parameters
}
```

**Sizing**: For 1,000,000 expected chunks at a 1% false positive rate, the filter uses ~9.6M bits = ~1.2 MB of memory. The optimal number of hash functions is `k = (m/n) * ln(2) = 7`.

**Double hashing**: Uses two independent SipHash computations as base hashes, then derives `k` hash positions via `h1 + i * h2 + i^2` (enhanced double hashing):

```rust
fn hash_positions(&self, key: &str) -> Vec<usize> {
    let h1 = siphash(key, 0, 0);
    let h2 = siphash(key, 1, 1);
    (0..self.num_hashes)
        .map(|i| {
            let i = i as u64;
            ((h1.wrapping_add(i.wrapping_mul(h2))
                .wrapping_add(i.wrapping_mul(i))) % self.num_bits as u64) as usize
        })
        .collect()
}
```

**Usage in chunk handler** (`HEAD /v1/chunks/:hash`):

```rust
// Fast path: if Bloom filter says "definitely not present," return 404 immediately
if let Some(bloom) = &state.bloom_filter {
    if !bloom.might_contain(&hash) {
        state.metrics.record_bloom_reject();
        return StatusCode::NOT_FOUND;  // No disk I/O needed
    }
}
// Bloom says "maybe present" -- check disk
```

The filter is populated at startup by scanning the `objects/` directory tree, and updated whenever a new chunk is stored. The admin API exposes `POST /v1/admin/bloom/rebuild` for manual reconstruction.

**Estimated false positive rate**: `(1 - e^(-kn/m))^k` -- with 500K chunks loaded, the FP rate is approximately 0.1%.

## 6.7 Pull-Through Caching and Request Coalescing

When Remi operates as a caching proxy (with `upstream_url` configured), a chunk miss triggers a fetch from the upstream Remi instance. The critical challenge is the **thundering herd problem**: if 100 clients request the same missing chunk simultaneously, only one upstream fetch should occur.

**Request coalescing** uses a `DashMap` of broadcast channels:

```rust
// src/server/handlers/chunks.rs
pub inflight_fetches: Arc<DashMap<String, broadcast::Sender<()>>>,

async fn pull_through_fetch(state: &ServerState, hash: &str) -> Result<Vec<u8>> {
    // Check if someone is already fetching this chunk
    if let Some(entry) = state.inflight_fetches.get(hash) {
        // Wait for the first fetcher to complete
        let mut rx = entry.subscribe();
        drop(entry);
        let _ = rx.recv().await;
        // The chunk should now be on disk
        return read_from_disk(hash);
    }

    // We're the first -- create a broadcast channel
    let (tx, _) = broadcast::channel(1);
    state.inflight_fetches.insert(hash.to_string(), tx.clone());

    // Fetch from upstream with InflightGuard for cleanup
    let _guard = InflightGuard { hash, map: &state.inflight_fetches, tx: &tx };
    let data = fetch_upstream(state, hash).await?;

    // Verify hash before storing (don't trust upstream blindly)
    verify_hash(hash, &data)?;
    state.chunk_cache.store_chunk(hash, &data).await?;

    Ok(data)
}
```

The `InflightGuard` ensures cleanup on both success and failure (including panics) via its `Drop` implementation. When it drops, it notifies all waiters via the broadcast channel and removes the entry from the `DashMap`.

**Hash verification**: After fetching from upstream, the SHA-256 of the received data is computed and compared against the requested hash. This prevents a compromised upstream from serving tampered data.

## 6.8 Chunk Serving Endpoints

The chunk handler (`src/server/handlers/chunks.rs`) provides five endpoints:

**HEAD /v1/chunks/:hash** -- Existence check. Bloom filter -> disk check. Returns 200 (with `Content-Length`) or 404. Used by clients to determine which chunks they already have.

**GET /v1/chunks/:hash** -- Full chunk retrieval with a cascading lookup:

```
1. Bloom filter: definitely absent? -> 404
2. Negative cache: recently confirmed absent? -> 404
3. Disk: file exists? -> serve (with Range support)
4. R2 redirect enabled? -> 307 to presigned URL
5. Pull-through: fetch from upstream -> store -> serve
6. Not found anywhere -> 404 (add to negative cache)
```

**Range request support** (HTTP 206): Clients can request partial chunks via `Range: bytes=start-end`. The handler parses the range header, seeks to the offset, and streams only the requested bytes. This enables resumable downloads.

**POST /v1/chunks/find-missing** -- Batch existence check. Client sends up to 10,000 chunk hashes; server returns the subset not present locally. Used during `conary install` to minimize round trips:

```json
// Request
{ "hashes": ["abc123...", "def456...", ...] }

// Response
{ "missing": ["def456...", ...], "checked": 10000 }
```

**POST /v1/chunks/batch** -- Batch fetch. Client requests up to 100 chunks; server returns them as either `multipart/mixed` (binary, for CLI clients) or JSON with base64 encoding (for web clients), based on the `Accept` header.

## 6.9 R2/CDN Integration

Cloudflare R2 provides S3-compatible object storage with zero egress fees, making it ideal for chunk distribution.

**Write-through**: When `r2.write_through = true`, every newly-stored chunk is also uploaded to R2 immediately after conversion. The chunk is stored locally first (for immediate serving), then uploaded asynchronously.

**Presigned URL redirect**: When `r2.r2_redirect = true`, chunk GET requests return `307 Temporary Redirect` to a presigned R2 URL instead of streaming data from the origin. This offloads bandwidth entirely to Cloudflare's CDN edge network:

```
Client: GET /v1/chunks/abc123...
Server: 307 Temporary Redirect
        Location: https://account.r2.cloudflarestorage.com/conary-chunks/chunks/abc123...?signature=...
Client: GET (follows redirect, served from R2 CDN edge)
```

The `R2Store` (`src/server/r2.rs`) wraps the AWS S3 SDK, configured with the R2-specific endpoint:

```rust
pub struct R2Config {
    pub endpoint: String,    // https://{account}.r2.cloudflarestorage.com
    pub bucket: String,      // "conary-chunks"
    pub prefix: String,      // "chunks/"
    pub region: String,      // "auto" for R2
}
```

Credentials come from environment variables (`CONARY_R2_ACCESS_KEY`, `CONARY_R2_SECRET_KEY`), never from the config file.

Combined with Cloudflare's cache rules, chunks achieve effectively infinite caching since they're immutable (the URL contains the content hash):

| Path pattern | Edge TTL | Browser TTL |
|---|---|---|
| `/v1/chunks/*` | 365 days | 365 days |
| `/_app/*` | 365 days | 365 days |
| `/v1/packages/*` | 5 minutes | 60 seconds |
| `/v1/index/*` | 60 seconds | 30 seconds |
| `/v1/search/*` | 30 seconds | 15 seconds |
| `/v1/admin/*` | Bypass | Bypass |

## 6.10 Sparse HTTP Index

Inspired by crates.io's sparse index, Remi provides per-package JSON documents that are individually cacheable by CDNs:

```
GET /v1/index/fedora/nginx
```

Returns a `SparseIndexEntry`:

```json
{
  "name": "nginx",
  "distro": "fedora",
  "versions": [
    {
      "version": "1.24.0-3.fc43",
      "deps": ["openssl", "pcre2", "zlib"],
      "provides": ["webserver", "nginx"],
      "arch": "x86_64",
      "size": 892416,
      "converted": true,
      "content_hash": "sha256:abcdef..."
    },
    {
      "version": "1.26.0-1.fc43",
      "deps": ["openssl", "pcre2", "zlib"],
      "provides": ["webserver", "nginx"],
      "arch": "x86_64",
      "size": 924672,
      "converted": false,
      "content_hash": null
    }
  ]
}
```

The `converted` field tells clients whether the package is ready for immediate download or will require an on-demand conversion (triggering the 202 Accepted flow).

Package listing is paginated:

```
GET /v1/index/fedora?page=1&per_page=100
```

Returns distinct package names with pagination metadata. This enables clients to sync the full index incrementally.

When **federated index** is enabled, the sparse entry builder merges local data with entries fetched in parallel from upstream Remi instances, preferring versions where `converted = true`.

## 6.11 Full-Text Search

Remi embeds a Tantivy search engine for package discovery:

```rust
// src/server/search.rs
pub struct SearchEngine {
    index: tantivy::Index,
    reader: IndexReader,
    schema: Schema,
    // Fields
    name: Field,          // Tokenized, 3x boosted
    name_exact: Field,    // STRING (for autocomplete regex)
    version: Field,
    distro: Field,        // Faceted
    description: Field,   // Tokenized
    dependencies: Field,
    size: Field,
    converted: Field,
}
```

**Index building** uses a 50 MB heap for bulk indexing. On startup, `rebuild_from_db()` reads all `repository_packages` and `converted_packages`, creating one document per unique (name, distro) pair. The `name_distro` composite field enables efficient deletion when updating entries.

**Search** (`GET /v1/search?q=web+server&distro=fedora`):

```rust
pub fn search(&self, query: &str, distro: Option<&str>, limit: usize)
    -> Result<Vec<SearchResult>>
```

Constructs a `BooleanQuery` with the user's terms (matched against name and description) and an optional distro facet filter. Name matches are boosted 3x over description matches.

**Autocomplete** (`GET /v1/suggest?q=ngi`):

```rust
pub fn suggest(&self, prefix: &str, limit: usize) -> Result<Vec<String>>
```

Uses a regex query on the `name_exact` field: `prefix.*`. Returns matching package names for type-ahead suggestions.

## 6.12 OCI Distribution Spec v2

Remi exposes CCS packages as OCI artifacts, enabling interoperability with any OCI-compatible tool (Harbor, Zot, ORAS, `crane`, `skopeo`):

```
GET  /v2/                                  -> {"Docker-Distribution-API-Version": "registry/2.0"}
GET  /v2/_catalog                          -> list repositories
GET  /v2/conary/{distro}/{name}/manifests/{ref}  -> OCI manifest
HEAD /v2/conary/{distro}/{name}/manifests/{ref}  -> manifest exists?
GET  /v2/conary/{distro}/{name}/blobs/{digest}   -> chunk data
HEAD /v2/conary/{distro}/{name}/blobs/{digest}   -> chunk exists?
GET  /v2/conary/{distro}/{name}/tags/list        -> version tags
```

**Name mapping**: CCS package -> OCI repository `conary/{distro}/{name}`. CCS version -> OCI tag. CCS chunks -> OCI layers. The custom media type is `application/vnd.conary.package.v1`.

**Manifest generation** (`src/server/handlers/oci.rs`):

```rust
fn build_manifest(package: &ConvertedPackage) -> OciManifest {
    // Each CCS chunk becomes an OCI layer
    let layers: Vec<OciDescriptor> = chunk_hashes.iter().map(|hash| {
        OciDescriptor {
            media_type: "application/vnd.conary.chunk.v1",
            digest: format!("sha256:{}", hash),
            size: chunk_size,
        }
    }).collect();

    // Synthetic config blob with package metadata
    let config = OciDescriptor {
        media_type: "application/vnd.conary.config.v1+json",
        digest: format!("sha256:{}", config_hash),
        size: config_json.len(),
    };

    OciManifest {
        schema_version: 2,
        media_type: "application/vnd.oci.image.manifest.v1+json",
        artifact_type: Some("application/vnd.conary.package.v1"),
        config,
        layers,
    }
}
```

**Blob serving** strips the `sha256:` prefix from the OCI digest and delegates directly to the existing chunk store -- zero new storage logic. This is possible because both CCS and OCI use content-addressed storage with SHA-256.

**Path routing**: OCI repository names can contain slashes (e.g., `conary/fedora/nginx`), so the handler uses a wildcard route `/v2/*path` with `dispatch_oci_path()` that uses `rfind` to split the path at the correct segment boundary.

## 6.13 Security

### Rate Limiting

Token bucket algorithm per IP address (`src/server/security.rs`):

```rust
pub struct RateLimiter {
    buckets: DashMap<String, TokenBucket>,
    rps: u32,      // Refill rate (default: 200/sec)
    burst: u32,    // Maximum burst (default: 400)
}
```

Each IP gets a bucket that refills at `rps` tokens per second up to `burst` capacity. When empty, requests get `429 Too Many Requests`.

### Ban List

Tracks consecutive failures per IP:

```rust
pub struct BanList {
    bans: DashMap<String, BanEntry>,
    duration: Duration,
    threshold: u32,
}
```

The ban middleware (`ban_middleware`) records failures on:
- 400 Bad Request (malformed hashes)
- 401/403 Unauthorized/Forbidden
- 404 on admin endpoints (probing)

After `threshold` consecutive failures (default: 20), the IP is banned for `duration` (default: 10 minutes). Banned IPs receive `403 Forbidden` on all requests.

### Client IP Extraction

Behind Cloudflare, the connecting IP is Cloudflare's edge server. The `extract_client_ip()` function handles this with a priority chain:

1. If connection is from a Cloudflare IP range, use the `CF-Connecting-IP` header
2. If a trusted proxy header is configured (e.g., `X-Forwarded-For`), use its first value
3. Fall back to the direct connection IP

Cloudflare IP ranges are compiled into the binary and validated on each request.

### CORS

Two CORS policies:
- **Public routes** (health, metadata, search): Permissive (`Access-Control-Allow-Origin: *`)
- **Chunk and admin routes**: Restricted to configured origins (default: same-origin only)

### Middleware Stack

Applied in this order (outermost first):
1. **Audit logging** -- Logs method, path, status, client IP, latency for chunk/admin/federation endpoints
2. **Ban enforcement** -- Checks and updates the ban list
3. **Rate limiting** -- Token bucket check per client IP
4. **Body size limit** -- 16 MB max for all requests

## 6.14 Negative Cache

Caches "not found" responses to avoid repeatedly probing upstream for packages that don't exist:

```rust
// src/server/negative_cache.rs
pub struct NegativeCache {
    entries: RwLock<HashMap<String, NegativeEntry>>,
    ttl: Duration,    // Default: 15 minutes
}

struct NegativeEntry {
    inserted: Instant,
    hits: AtomicU64,    // Track how often this 404 is repeated
}
```

When a chunk or package isn't found anywhere (local disk, R2, upstream), the hash is added to the negative cache. Subsequent requests for the same hash within the TTL window return 404 immediately without any I/O.

A background cleanup task runs every 5 minutes, removing expired entries. The admin API exposes stats and a manual clear endpoint.

## 6.15 Job Management

The `JobManager` (`src/server/jobs.rs`) tracks conversion jobs with concurrency control:

```rust
pub struct JobManager {
    jobs: RwLock<HashMap<JobId, ConversionJob>>,
    semaphore: Arc<Semaphore>,    // Limits concurrent conversions
}

pub enum JobStatus {
    Pending,       // Queued, waiting for semaphore
    Converting,    // Active conversion in progress
    Ready,         // Conversion complete, package available
    Failed(String), // Conversion failed with error message
}
```

**Deduplication**: Jobs are keyed by `{distro}:{name}:{version}`. If a job for the same package already exists, the existing job ID is returned instead of creating a duplicate.

**Queue capacity**: At most `2 * max_concurrent` jobs can be pending. Beyond that, new requests are rejected with `503 Service Unavailable`.

**Cleanup**: Completed and failed jobs are removed after 1 hour by the eviction loop.

## 6.16 Analytics and Metrics

### Download Analytics

The `AnalyticsRecorder` (`src/server/analytics.rs`) buffers download events in memory and flushes to the database in batches:

```rust
pub struct AnalyticsRecorder {
    buffer: RwLock<Vec<DownloadEvent>>,
    db_path: PathBuf,
    flush_threshold: usize,    // 100 events
}
```

Events are recorded after each successful package download (not chunk downloads -- those are tracked via metrics). The buffer auto-flushes at 100 events or every 5 minutes via `run_analytics_loop()`. After flushing, it refreshes aggregate statistics used by the popularity-driven pre-warming pipeline.

### Prometheus Metrics

The `ServerMetrics` (`src/server/metrics.rs`) uses atomic counters for lock-free recording:

```rust
pub struct ServerMetrics {
    pub requests_total: AtomicU64,
    pub cache_hits: AtomicU64,
    pub cache_misses: AtomicU64,
    pub bloom_rejects: AtomicU64,
    pub bytes_served: AtomicU64,
    pub upstream_fetches: AtomicU64,
    pub upstream_errors: AtomicU64,
    pub start_time: Instant,
}
```

Exported at `GET /metrics` (public) and `GET /v1/admin/metrics/prometheus` (admin) in Prometheus text format:

```
# HELP remi_requests_total Total requests served
# TYPE remi_requests_total counter
remi_requests_total 1847293

# HELP remi_cache_hit_rate Cache hit ratio (0.0-1.0)
# TYPE remi_cache_hit_rate gauge
remi_cache_hit_rate 0.943

# HELP remi_bloom_rejects_total Requests rejected by Bloom filter
# TYPE remi_bloom_rejects_total counter
remi_bloom_rejects_total 52841
```

The admin metrics endpoint also includes negative cache stats, job queue status, and uptime.

## 6.17 Delta Manifests

For packages with multiple versions, Remi pre-computes chunk-level diffs (`src/server/delta_manifests.rs`):

```rust
pub struct DeltaManifest {
    pub from_version: String,
    pub to_version: String,
    pub new_chunks: Vec<String>,       // Chunks in 'to' but not 'from'
    pub removed_chunks: Vec<String>,   // Chunks in 'from' but not 'to'
    pub download_size: u64,            // Total bytes of new chunks
    pub full_size: u64,                // Total bytes of all chunks in 'to'
    pub savings_percent: f64,          // (1 - download_size/full_size) * 100
}
```

The delta computation is straightforward set arithmetic: query chunk hashes for both versions from `converted_packages`, compute the set difference. Because FastCDC produces stable chunk boundaries, most chunks are shared between adjacent versions of the same package.

```
GET /v1/fedora/packages/nginx/delta?from=1.24.0-3&to=1.26.0-1
```

```json
{
  "from_version": "1.24.0-3.fc43",
  "to_version": "1.26.0-1.fc43",
  "new_chunks": ["abc123...", "def456..."],
  "removed_chunks": ["old789..."],
  "download_size": 131072,
  "full_size": 924672,
  "savings_percent": 85.8
}
```

Delta manifests are cached in the `delta_manifests` database table. The `compute_deltas_for_package()` function pre-computes deltas for all adjacent version pairs of a package.

## 6.18 Pre-Warming Pipeline

Rather than waiting for the first client request to trigger conversion, Remi can proactively convert popular packages:

```toml
[prewarm]
enabled = true
metadata_sync_interval = "6h"
convert_top_n = 1000
distros = ["fedora", "arch", "ubuntu"]
```

The `run_prewarm_background()` function runs on a configurable interval (default: every 6 hours). For each distro:

1. Fetch upstream popularity data (e.g., Fedora's `popularity-contest` results)
2. Merge with local download analytics for combined ranking
3. Convert the top N packages that aren't already converted
4. Report results: packages converted, chunks stored, bytes used

This ensures the most-requested packages are always ready for instant delivery, avoiding the 202 Accepted wait on first access.

## 6.19 Federated Sparse Index

When multiple Remi instances serve different distributions (one for Fedora, one for Arch), a single client-facing instance can merge their sparse indices:

```rust
// src/server/federated_index.rs
pub struct FederatedIndexConfig {
    pub upstream_urls: Vec<String>,  // Peer Remi instances
    pub timeout: Duration,           // 10 seconds default
    pub cache_ttl: Duration,         // 300 seconds default
}
```

When a sparse index request arrives:

1. Build the local sparse entry from the database
2. Fetch remote entries from all peers in parallel (`tokio::join!`)
3. Merge all entries, deduplicating by version
4. Prefer versions where `converted = true` (one peer may have already converted it)
5. Cache the merged result in memory with TTL

```rust
pub fn merge_sparse_entries(entries: Vec<SparseIndexEntry>) -> SparseIndexEntry {
    let mut merged_versions: HashMap<String, SparseVersionEntry> = HashMap::new();
    for entry in entries {
        for ver in entry.versions {
            merged_versions
                .entry(ver.version.clone())
                .and_modify(|existing| {
                    // Prefer converted versions
                    if ver.converted && !existing.converted {
                        *existing = ver.clone();
                    }
                })
                .or_insert(ver);
        }
    }
    // ... construct merged entry
}
```

The `FederatedIndexCache` uses `RwLock<HashMap<String, (Instant, SparseIndexEntry)>>` for TTL-based caching.

## 6.20 Remi Lite (Zero-Config LAN Proxy)

For CI environments, air-gapped networks, or fleet deployments, `conary remi-proxy` provides a single-command caching proxy:

```bash
conary remi-proxy                                # Auto-discover upstream via mDNS
conary remi-proxy --upstream https://remi.example.com
conary remi-proxy --offline --cache-dir /mnt/usb  # Air-gapped mode
```

```rust
// src/server/lite.rs
pub struct ProxyConfig {
    pub port: u16,                    // Default: 7891
    pub upstream: Option<String>,     // Explicit upstream URL
    pub cache_dir: PathBuf,           // Local chunk cache
    pub discover: bool,              // mDNS discovery
    pub advertise: bool,             // mDNS advertisement
    pub offline: bool,               // No upstream fetches
    pub max_cache_bytes: u64,        // Default: 10 GB
}
```

**Zero-config startup sequence**:

1. Scan for upstream Remi instances via mDNS (`_conary-cas._tcp.local`)
2. If found, configure as a caching proxy to the discovered instance
3. Advertise itself on the LAN via mDNS so other Conary clients auto-discover it
4. Start serving with pull-through chunk caching

**Routes** (subset of full Remi):
- `GET /health` -- Liveness check
- `GET /v1/chunks/:hash` -- Pull-through chunk serving (local cache -> upstream)
- `GET /v1/index/*` -- Proxied sparse index with 60-second file cache
- `GET /v1/*/packages/*` -- Proxied package metadata

**Offline mode**: When `--offline` is set, Remi Lite serves only from its local cache. No upstream connections are made. Useful for pre-populated USB sticks or air-gapped environments.

**File-based index cache**: Unlike full Remi's database-backed cache, Lite stores index responses as flat files with 60-second TTL for simplicity.

## 6.21 Index Generation and Signing

The `generate_indices()` function (`src/server/index_gen.rs`) produces per-distro JSON repository indices:

```rust
pub struct RepositoryIndex {
    pub version: u32,
    pub distro: String,
    pub generated_at: String,
    pub packages: HashMap<String, PackageIndexEntry>,
    pub total_chunks: usize,
    pub total_bytes: u64,
}
```

Each entry includes the package's version, architecture, chunk count, chunk hashes, total size, and conversion status. The index is optionally signed with Ed25519 (`sign_index()` computes the signature over the canonical JSON bytes).

Index generation scans both the `converted_packages` table and the chunk store directory tree via `walkdir`. This provides both metadata (from the database) and chunk statistics (from the filesystem).

## 6.22 Deployment

### Cloudflare Setup

The production deployment at `packages.conary.io` uses Cloudflare for DNS, CDN caching, DDoS protection, and R2 storage. The full setup is documented in `deploy/CLOUDFLARE.md`:

1. **DNS**: A/AAAA records for `packages.conary.io` with orange cloud (proxy) enabled
2. **SSL/TLS**: Full (strict) encryption mode, TLS 1.2 minimum, Cloudflare Origin Certificate
3. **R2 bucket**: `conary-chunks` with API token scoped to that bucket
4. **Cache rules**: Immutable chunks cached forever, metadata cached briefly, admin bypassed
5. **WAF**: Managed ruleset with Skip rule for `/v1/chunks/` (prevents binary data false positives)
6. **DDoS**: Default L7 protection plus rate-limiting rule (1000 req/10s per IP)
7. **Health checks**: `GET /health` every 60 seconds, expects `200 OK`

### Admin API Access

The admin listener binds to `127.0.0.1:8081` and is never exposed externally. Access via SSH tunnel:

```bash
ssh -L 8081:localhost:8081 remi
curl http://localhost:8081/v1/admin/cache/stats
curl -X POST http://localhost:8081/v1/admin/evict
curl -X POST http://localhost:8081/v1/admin/bloom/rebuild
curl http://localhost:8081/v1/admin/metrics/prometheus
curl http://localhost:8081/v1/admin/info
```

### Health Checks

Two health endpoints on the public API:

- `GET /health` -- Simple liveness: returns `"OK"` (200)
- `GET /health/ready` -- Deep readiness: checks database accessibility, chunk directory writability, cache directory accessibility, and disk space (warns below 10 GB free). Returns 200 or 503 with details:

```json
{
  "ready": false,
  "db_accessible": true,
  "chunk_dir_ok": true,
  "cache_dir_ok": true,
  "disk_space_ok": false
}
```

### Web Frontend

Remi optionally serves a SvelteKit-based web frontend for package browsing:

```toml
[web]
enabled = true
root = "/conary/web"
```

The frontend is served as a SPA with `ServeDir` + `ServeFile` fallback to `index.html`. API routes take priority over the SPA fallback, so `/v1/*` paths are always handled by the API.

## 6.23 Complete API Reference

### Public API (port 8080)

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/health` | Liveness check |
| GET | `/health/ready` | Deep readiness check |
| GET | `/metrics` | Prometheus metrics |
| HEAD | `/v1/chunks/:hash` | Chunk existence (Bloom-protected) |
| GET | `/v1/chunks/:hash` | Chunk data (Range support, R2 redirect) |
| POST | `/v1/chunks/find-missing` | Batch existence check (up to 10K hashes) |
| POST | `/v1/chunks/batch` | Batch fetch (up to 100 chunks) |
| GET | `/v1/:distro/packages/:name` | Package metadata (triggers conversion) |
| GET | `/v1/:distro/packages/:name/download` | CCS package download |
| GET | `/v1/:distro/packages/:name/delta` | Delta manifest between versions |
| GET | `/v1/jobs/:job_id` | Conversion job status |
| GET | `/v1/index/:distro/:name` | Sparse index entry |
| GET | `/v1/index/:distro` | Paginated package list |
| GET | `/v1/search?q=...` | Full-text search |
| GET | `/v1/suggest?q=...` | Autocomplete suggestions |
| GET | `/v1/:distro/metadata` | Repository metadata |
| GET | `/v1/:distro/metadata.sig` | Metadata signature |
| GET | `/v1/federation/directory` | Federation peer directory |
| GET | `/v1/packages/:distro/:name` | Package detail |
| GET | `/v1/packages/:distro/:name/versions` | Version list |
| GET | `/v1/packages/:distro/:name/dependencies` | Dependency graph |
| GET | `/v1/packages/:distro/:name/rdepends` | Reverse dependencies |
| GET | `/v1/:distro/tuf/*.json` | TUF trust metadata |
| GET | `/v1/models/:name` | Model collection |
| GET | `/v1/stats/popular` | Popular packages |
| GET | `/v1/stats/recent` | Recently converted |
| GET | `/v1/stats/overview` | Server overview stats |
| GET | `/v2/` | OCI version check |
| GET | `/v2/_catalog` | OCI catalog |
| GET/HEAD | `/v2/*path` | OCI manifests, blobs, tags |

### Admin API (port 8081)

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/health` | Admin liveness |
| POST | `/v1/admin/convert` | Trigger package conversion |
| GET | `/v1/admin/cache/stats` | Cache statistics |
| POST | `/v1/admin/evict` | Trigger cache eviction |
| POST | `/v1/admin/bloom/rebuild` | Rebuild Bloom filter |
| GET | `/v1/admin/metrics` | Detailed JSON metrics |
| GET | `/v1/admin/metrics/prometheus` | Prometheus format |
| GET | `/v1/admin/negative-cache/stats` | Negative cache stats |
| POST | `/v1/admin/negative-cache/clear` | Clear negative cache |
| POST | `/v1/admin/recipes/build` | Trigger recipe build |
| GET | `/v1/admin/info` | Server configuration info |
| POST | `/v1/admin/refresh` | Refresh upstream metadata |
| PUT | `/v1/admin/models/:name` | Publish model collection |
| POST | `/v1/admin/tuf/refresh-timestamp` | Refresh TUF timestamp |

---

---

# 7. Security and Trust

Modern package managers distribute and execute code on millions of machines. Conary treats security as a first-class architectural concern, not an afterthought bolted onto a working system. This chapter covers the four pillars of Conary's security model: capability declarations (what a package is allowed to do), kernel enforcement (making those declarations real), provenance tracking (where a package came from and how it was built), and TUF trust (ensuring the repository hasn't been compromised).

## 7.1 Capability Declarations

Every Conary package can declare exactly what system resources it needs -- network access, filesystem paths, and system calls. These declarations live in the package's CCS metadata as a `CapabilityDeclaration` (`src/capability/declaration.rs`).

### Declaration Structure

```rust
pub struct CapabilityDeclaration {
    pub version: u32,           // Schema version (currently 1)
    pub rationale: String,      // Human-readable explanation
    pub network: Option<NetworkCapabilities>,
    pub filesystem: Option<FilesystemCapabilities>,
    pub syscalls: Option<SyscallCapabilities>,
}
```

A TOML declaration in a recipe looks like:

```toml
[capabilities]
rationale = "Web server that binds HTTP/HTTPS and writes logs"

[capabilities.network]
listen_ports = ["80", "443"]
outbound_ports = ["443"]   # For OCSP stapling

[capabilities.filesystem]
read = ["/etc/nginx", "/etc/ssl/certs", "/usr/share/nginx"]
write = ["/var/log/nginx", "/var/cache/nginx", "/run/nginx.pid"]
deny = ["/home", "/root"]

[capabilities.syscalls]
profile = "network-server"
```

### Network Capabilities

```rust
pub struct NetworkCapabilities {
    pub outbound_ports: Vec<String>,  // Ports the package connects to
    pub listen_ports: Vec<String>,    // Ports the package listens on
    pub none: bool,                   // Explicit "no network needed"
}
```

Port specs support numbers (`443`), ranges (`8000-8100`), and the wildcard `any`. Validation enforces mutual exclusivity: `none = true` conflicts with non-empty port lists.

### Filesystem Capabilities

```rust
pub struct FilesystemCapabilities {
    pub read: Vec<String>,     // Read-only paths
    pub write: Vec<String>,    // Read-write paths
    pub execute: Vec<String>,  // Executable paths
    pub deny: Vec<String>,     // Explicitly denied paths
}
```

All paths must be absolute. Deny rules take precedence -- if a package declares `read = ["/etc"]` and `deny = ["/etc/shadow"]`, the shadow file is blocked even though `/etc` is readable.

### Syscall Capabilities

```rust
pub struct SyscallCapabilities {
    pub allow: Vec<String>,   // Explicitly allowed syscalls
    pub deny: Vec<String>,    // Explicitly denied syscalls
    pub profile: Option<String>, // Named profile
}
```

Six predefined syscall profiles cover common package archetypes:

| Profile | Syscalls | Use case |
|---------|----------|----------|
| `minimal` | ~23 (read, write, mmap, etc.) | CLI tools, libraries |
| `network-server` | ~37 (adds socket, bind, listen, accept, epoll) | nginx, PostgreSQL |
| `network-client` | ~33 (adds socket, connect, send, recv) | curl, package managers |
| `gui-app` | ~42 (adds shm, ioctl, poll, memfd) | Desktop applications |
| `system-daemon` | ~45 (adds mount, chroot, setuid, prctl) | systemd, init services |
| `container` | ~50 (adds clone, unshare, pivot_root, seccomp) | Docker, Podman |

Each profile is a static array of syscall names. Explicit `allow` and `deny` lists are merged with the profile: `final = (profile + allow) - deny`.

## 7.2 Capability Inference

Most packages don't ship with explicit capability declarations. Conary's 4-tier inference engine (`src/capability/inference/`) automatically deduces what a package needs, with each tier trading speed for accuracy.

### Tier 1: Well-Known Profiles

The fastest and most reliable tier. A curated registry of 100+ well-known packages maps package names to pre-defined capability profiles (`src/capability/inference/wellknown.rs`).

```rust
static PROFILES: LazyLock<HashMap<&'static str, InferredCapabilities>> = LazyLock::new(|| {
    let mut m = HashMap::new();
    m.insert("nginx", network_server_profile("nginx",
        &["80", "443"],
        &["/etc/nginx", "/etc/ssl/certs", "/usr/share/nginx"],
        &["/var/log/nginx", "/var/cache/nginx", "/run/nginx.pid"]));
    // ... 100+ more profiles
    m
});
```

Coverage includes web servers (nginx, Apache, Caddy), databases (PostgreSQL, MySQL, Redis, MongoDB, Elasticsearch), message queues (RabbitMQ), DNS (bind9, unbound, CoreDNS), mail (Postfix, Dovecot), VPN (OpenVPN, WireGuard), monitoring (Prometheus, Grafana), Kubernetes components, container tools, shells, editors, build tools, programming languages, package managers, and more.

Lookup handles versioned names: `python3.11` matches the `python` profile by checking if the suffix after the known name starts with a digit.

### Tier 2: Heuristic Rules

Rule-based analysis using package metadata without reading binary contents (`src/capability/inference/heuristics.rs`). Four signal categories:

**Package name patterns**:
- Ends with `-server` or `d` (daemon) -> `network-server` profile
- Ends with `-client` or `-cli` -> network client
- Starts with `lib` -> library (minimal profile)

**File path analysis**:
- `/usr/sbin/` executables -> system daemon
- `/etc/<name>/` files -> adds read path
- `/var/log/<name>/` files -> adds write path
- `/var/lib/<name>/` files -> adds write path

**Systemd service analysis**: Parses `[Unit]` section for `After=network`, extracts `ListenStream=` port directives, checks for `PrivateNetwork=true` (which means no network needed).

**Dependency analysis**: Linking against `libssl` implies outbound port 443. Linking against `libpq` implies outbound port 5432. GUI libraries (GTK, Qt, X11, Wayland) trigger the `gui-app` profile.

### Tier 3: Configuration File Scanning

Regex-based scanning of package configuration files for network and filesystem hints:

```rust
// Network patterns
static PORT_RE: LazyLock<Regex> = LazyLock::new(||
    Regex::new(r"(?i)(?:listen|port|bind)[=:\s]+(\d{1,5})").unwrap());

// Filesystem patterns
static PATH_RE: LazyLock<Regex> = LazyLock::new(||
    Regex::new(r"(?i)(?:file|path|dir|log|root)[=:\s]+(/[^\s;#]+)").unwrap());
```

This catches cases where the binary is generic (e.g., a Python interpreter) but the configuration reveals the actual resource usage (e.g., `listen_port = 8080` in a config file).

### Tier 4: ELF Binary Analysis

The slowest but most accurate tier. Uses `goblin` to parse ELF binaries and extract evidence (`src/capability/inference/binary.rs`):

**Linked libraries**: `libssl.so.3` -> network + SSL. `libpq.so.5` -> database. `libgtk-3.so.0` -> GUI.

**Imported symbols**: Socket calls (`socket`, `bind`, `listen`, `accept`, `connect`) -> network. Privileged calls (`setuid`, `chroot`, `mount`) -> system daemon. Exec calls (`execve`, `fork`, `system`) -> needs execute paths.

Uses `rayon` for parallel analysis when a package contains multiple executables.

### Inference Pipeline

```rust
pub fn infer_capabilities(name: &str, ..., options: &InferenceOptions) -> InferredCapabilities {
    // Tier 1: Check well-known profiles
    if let Some(profile) = WellKnownProfiles::lookup(name) { return profile; }

    // Tier 2: Heuristic rules
    let heuristic = HeuristicInferrer::infer(&files, &metadata)?;
    if heuristic.confidence.overall() >= Confidence::Medium { return heuristic; }

    // Tier 3: Config file scanning (integrated into main infer fn)
    let config_merged = scan_config_files_and_merge(heuristic);

    // Tier 4: Binary analysis (opt-in, slow)
    if options.enable_binary_analysis {
        let binary = BinaryAnalyzer::analyze_all(&executables)?;
        return config_merged.merge(binary);
    }

    config_merged
}
```

Results are cached in a global LRU cache keyed by `name + version + file_hashes`. The `merge()` function combines results from multiple tiers, preferring higher-confidence evidence for each capability category.

### Confidence Levels

Every inference result carries a confidence score:
- **High**: Well-known profile or positive binary evidence (linked library, imported symbol)
- **Medium**: Heuristic match (name pattern, file paths, dependencies)
- **Low**: No strong evidence found (inference is a best guess)

The `InferenceOptions` struct controls behavior:
- `max_tier`: Stop at this tier (default: 2, for speed)
- `enable_binary_analysis`: Allow tier 4 (default: false)
- `min_confidence`: Reject results below this threshold

## 7.3 Capability Enforcement

Declarations without enforcement are just documentation. Conary uses two Linux kernel mechanisms to make capability declarations real: Landlock LSM for filesystem access and seccomp-BPF for system call filtering. Both are applied post-fork, pre-exec during scriptlet execution.

### Enforcement Modes

```rust
pub enum EnforcementMode {
    Audit,    // Log violations, don't block (for testing)
    Warn,     // Log and warn, don't block
    Enforce,  // Block violations, kill process on violation
}
```

The `EnforcementPolicy` bundles mode with capability rules:

```rust
pub struct EnforcementPolicy {
    pub mode: EnforcementMode,
    pub filesystem: Option<FilesystemCapabilities>,
    pub syscalls: Option<SyscallCapabilities>,
    pub network_isolation: bool,
}
```

### Landlock Filesystem Enforcement

Landlock (`src/capability/enforcement/landlock_enforce.rs`) is a Linux Security Module that restricts filesystem access for unprivileged processes. Conary uses Landlock ABI V3 (Linux 6.2+) with `CompatLevel::BestEffort` for backward compatibility with older kernels.

```rust
pub fn apply_landlock_rules(
    filesystem: &FilesystemCapabilities,
    mode: EnforcementMode,
) -> Result<RulesetStatus> {
    let mut ruleset = Ruleset::default()
        .set_compatibility(CompatLevel::BestEffort)
        .handle_access(AccessFs::from_all(ABI::V3))?
        .create()?;

    // Add read-only rules
    for path in &filesystem.read {
        if Path::new(path).exists() {
            ruleset = ruleset.add_rule(
                PathBeneath::new(PathFd::new(path)?, AccessFs::from_read(ABI::V3))
            )?;
        }
    }

    // Add read-write rules
    for path in &filesystem.write {
        if Path::new(path).exists() {
            ruleset = ruleset.add_rule(
                PathBeneath::new(PathFd::new(path)?, AccessFs::from_all(ABI::V3))
            )?;
        }
    }

    // Add execute rules
    for path in &filesystem.execute {
        if Path::new(path).exists() {
            ruleset = ruleset.add_rule(
                PathBeneath::new(PathFd::new(path)?, AccessFs::Execute)
            )?;
        }
    }

    // Restrict the calling thread
    let status = ruleset.restrict_self()?;
    Ok(status)
}
```

**Key behavior**: Landlock is deny-by-default. Only paths explicitly listed in the ruleset are accessible. Non-existent paths are silently skipped (the package might not need them on this system). Deny conflicts (a path listed in both `read` and `deny`) are detected and the deny rule wins.

**Return status**: `FullyEnforced` (all rules applied), `PartiallyEnforced` (some rules skipped on older kernels), or `NotEnforced` (Landlock not available).

**Limitation**: Landlock cannot deny sub-paths under allowed parent directories. If `/etc` is allowed for reading, `/etc/shadow` cannot be individually denied via Landlock alone. Use seccomp or namespace isolation for finer-grained control.

### Seccomp-BPF Syscall Enforcement

Seccomp (`src/capability/enforcement/seccomp_enforce.rs`) filters system calls at the kernel level using BPF programs. Conary uses the `seccompiler` crate to build allowlist filters.

```rust
pub fn apply_seccomp_filter(
    syscalls: &SyscallCapabilities,
    mode: EnforcementMode,
) -> Result<()> {
    // Merge profile + explicit allow - explicit deny
    let mut allowed: HashSet<String> = HashSet::new();

    if let Some(ref profile_name) = syscalls.profile {
        if let Some(profile) = SyscallProfile::from_name(profile_name) {
            allowed.extend(profile.allowed_syscalls().iter().map(|s| s.to_string()));
        }
    }
    allowed.extend(syscalls.allow.iter().cloned());
    for denied in &syscalls.deny {
        allowed.remove(denied);
    }

    // Expand wildcards: "epoll_*" -> epoll_create, epoll_ctl, epoll_wait, ...
    let expanded = expand_wildcards(&allowed);

    // Map names to syscall numbers (x86_64)
    let syscall_numbers = resolve_syscall_numbers(&expanded)?;

    // Build BPF filter
    let default_action = match mode {
        EnforcementMode::Enforce => SeccompAction::KillProcess,
        _ => SeccompAction::Log,
    };

    let filter = SeccompFilter::new(
        syscall_numbers.into_iter()
            .map(|nr| (nr, vec![SeccompRule::new(vec![])]))
            .collect(),
        default_action,
        AUDIT_ARCH_X86_64,
    )?;

    // Set NO_NEW_PRIVS (required for unprivileged seccomp)
    prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0)?;

    // Apply the BPF program
    let bpf: BpfProgram = filter.try_into()?;
    seccompiler::apply_filter(&bpf)?;

    Ok(())
}
```

**Wildcards**: Glob patterns like `epoll_*` are expanded against a known set of ~80 x86_64 syscall names. This resolves to `epoll_create`, `epoll_create1`, `epoll_ctl`, `epoll_wait`, `epoll_pwait`, `epoll_pwait2`.

**Default action**: In `Enforce` mode, any syscall not in the allowlist kills the process (`SECCOMP_RET_KILL_PROCESS`). In `Warn`/`Audit` mode, it logs the violation (`SECCOMP_RET_LOG`).

**`PR_SET_NO_NEW_PRIVS`**: Required by the kernel before an unprivileged process can install a seccomp filter. This also prevents the process from gaining additional privileges via setuid/setgid binaries.

### Enforcement Order

The `apply_enforcement()` orchestrator applies kernel mechanisms in the correct order:

```rust
pub fn apply_enforcement(policy: &EnforcementPolicy) -> Result<EnforcementReport> {
    // 1. Landlock first (filesystem restrictions)
    if let Some(ref fs) = policy.filesystem {
        apply_landlock_rules(fs, policy.mode)?;
    }

    // 2. Seccomp last (syscall filter -- most restrictive, cannot be removed)
    if let Some(ref sc) = policy.syscalls {
        apply_seccomp_filter(sc, policy.mode)?;
    }
}
```

Landlock must be applied before seccomp because seccomp is irreversible -- once installed, the filter cannot be removed or relaxed. If seccomp were applied first and something went wrong with Landlock setup, the process would be stuck with a restrictive syscall filter and no filesystem restrictions.

### Kernel Support Detection

```rust
pub fn check_enforcement_support() -> EnforcementSupport {
    // Check Landlock: read /sys/kernel/security/lsm for "landlock"
    // Fallback: try creating an empty ruleset
    let landlock = check_landlock_support();

    // Check seccomp: try PR_GET_SECCOMP via prctl
    let seccomp = check_seccomp_support();

    EnforcementSupport { landlock, seccomp }
}
```

## 7.4 Capability-Based Dependency Resolution

Beyond enforcing capabilities at runtime, Conary uses them for dependency resolution. Instead of depending on a specific package name, a recipe can depend on a *capability* (`src/capability/resolver.rs`).

### Capability Specs

```rust
pub enum CapabilitySpec {
    Named(String),       // "ssl" -> any package providing SSL
    Typed(String),       // "soname(libssl.so.3)" -> shared library
    Network(String),     // "network.listen:443" -> something on port 443
    Filesystem(String),  // "filesystem.read:/etc/ssl" -> reads SSL certs
}
```

Parsing is prefix-based:

```rust
pub fn parse_capability_spec(spec: &str) -> CapabilitySpec {
    if spec.starts_with("soname(") { Typed }
    else if spec.starts_with("network.") { Network }
    else if spec.starts_with("filesystem.") { Filesystem }
    else { Named }
}
```

### Match Scoring

The resolver queries both the `provides` table (traditional dependency matching) and the `capabilities` table (declared capabilities). Matches are scored:

| Match type | Score | Example |
|------------|-------|---------|
| Typed (`soname(...)`) | 95 | `soname(libssl.so.3)` -> `openssl` |
| Declared capability | 90 | `ssl` -> package with `capabilities.network.outbound_ports` containing `443` |
| Network match | 85 | `network.listen:443` -> package listening on 443 |
| Filesystem match | 80 | `filesystem.read:/etc/ssl` -> package reading SSL certs |

Filesystem matching supports both exact paths and prefix matching (path `/etc/ssl/certs` matches a capability for `/etc/ssl`).

## 7.5 Capability Audit

The audit system (`src/capability/mod.rs`) compares declared capabilities against actual behavior:

```rust
pub enum AuditStatus {
    Compliant,        // Declaration matches usage
    OverPrivileged,   // Declares more than it uses
    UnderUtilized,    // Uses capabilities it didn't declare
    Undeclared,       // No declaration at all
}

pub struct AuditViolation {
    pub category: String,      // "network", "filesystem", "syscalls"
    pub expected: String,      // What was declared
    pub observed: String,      // What was actually used
    pub severity: AuditSeverity, // Info, Warning, Error
}
```

The audit workflow:
1. Run inference (tiers 1-4) on the installed package to detect actual usage
2. Compare against the stored `CapabilityDeclaration`
3. Flag discrepancies as violations

Use `conary capability audit <package>` to check a single package, or `conary capability audit --all` to scan every installed package.

## 7.6 Container Sandboxing

Scriptlet execution (install/remove hooks) runs inside a lightweight Linux container (`src/container/mod.rs`). This protects the host from malicious or buggy scripts without requiring a full container runtime.

### Namespace Isolation

The sandbox uses five Linux namespaces:

| Namespace | Flag | Protection |
|-----------|------|------------|
| PID | `CLONE_NEWPID` | Scripts can't see or signal host processes |
| UTS | `CLONE_NEWUTS` | Hostname changes don't affect host |
| IPC | `CLONE_NEWIPC` | Shared memory and semaphores are isolated |
| Mount | `CLONE_NEWNS` | Filesystem changes don't propagate |
| Network | `CLONE_NEWNET` | Only loopback interface (no internet) |

Network isolation is **enabled by default**. When active, `/etc/resolv.conf` is not mounted into the container (useless without network). Call `config.allow_network()` to disable network isolation and add the DNS resolver mount.

### Container Configurations

```rust
ContainerConfig::default()     // Standard: all namespaces, bind-mount /usr /lib /bin, 512MB RAM, 60s timeout
ContainerConfig::strict()      // Maximum isolation, 30s timeout
ContainerConfig::minimal()     // Resource limits only, no namespaces
ContainerConfig::pristine()    // No host mounts at all (for bootstrap builds)
ContainerConfig::hermetic()    // Pristine + network isolation (BuildStream-grade)
```

**Default bind mounts** (read-only):
- `/usr`, `/lib`, `/lib64`, `/bin`, `/sbin` -- essential system directories
- `/etc/passwd`, `/etc/group`, `/etc/hosts` -- identity files

**Pristine mode** starts with zero bind mounts. You must explicitly add everything the build needs:

```rust
let mut config = ContainerConfig::pristine();
config.add_bind_mount(BindMount::readonly("/opt/stage0", "/tools")); // Toolchain
config.add_bind_mount(BindMount::readonly("/src/gcc", "/src/gcc")); // Source
config.add_bind_mount(BindMount::writable("/build/gcc", "/build/gcc")); // Build dir
config.add_bind_mount(BindMount::writable("/destdir", "/destdir")); // Install dest
```

The `pristine_for_bootstrap()` convenience method pre-configures these paths.

### Resource Limits

Applied via `setrlimit`:

| Limit | Default | Purpose |
|-------|---------|---------|
| `RLIMIT_AS` | 512 MB | Virtual memory cap |
| `RLIMIT_CPU` | 60 seconds | CPU time limit |
| `RLIMIT_FSIZE` | 100 MB | Maximum file size |
| `RLIMIT_NPROC` | 1024 | Maximum processes |

Plus a wall-clock timeout enforced by the parent process (kills child with `SIGKILL` on expiry).

### Script Risk Analysis

Before execution, scripts are analyzed for dangerous patterns (`analyze_script()`):

| Risk | Pattern | Description |
|------|---------|-------------|
| Critical | `curl ... \| sh` | Downloads and executes remote code |
| Critical | `eval $...` | Dynamic code execution |
| High | `rm -rf /` | Recursive root deletion |
| High | `dd if=... of=/dev/` | Direct device write |
| High | Fork bomb pattern | Process exhaustion |
| Medium | `chmod u+s` | Setuid bit manipulation |
| Medium | `/etc/shadow` access | Password file access |
| Medium | `crontab` modification | Persistence mechanism |
| Low | `nc`, `/dev/tcp/` | Network backdoor potential |
| Low | `base64 -d` | Obfuscation indicator |

Based on the risk level, the system recommends sandboxing strategy:
- **Safe**: No special handling
- **Low**: Consider sandboxing for untrusted packages
- **Medium**: Sandboxed execution recommended
- **High/Critical**: MUST sandbox, review script before execution

### Execution Flow

```
fork()
  |
  child:
    unshare(PID | UTS | IPC | MOUNT | NET)  // Create namespaces
    ip link set lo up                         // Bring up loopback
    sethostname("conary-sandbox")             // Set container hostname
    setup_mount_namespace()                   // Bind mounts + chroot
    apply_resource_limits()                   // setrlimit calls
    apply_enforcement()                       // Landlock + seccomp (if policy set)
    chdir(workdir)
    exec(interpreter, script)
  |
  parent:
    waitpid(child, timeout)                   // Wait with wall-clock timeout
    SIGKILL on timeout                        // Force-kill hung scripts
```

The capability enforcement policy (section 7.3) integrates with the container: if `config.capability_policy` is set, Landlock and seccomp filters are applied inside the container after namespace setup but before script execution. This provides defense-in-depth: namespaces isolate the environment, Landlock restricts filesystem access within the container, and seccomp limits available syscalls.

### Fallback Behavior

If namespace isolation isn't available (non-root without `unprivileged_userns_clone`):
- For standard containers: falls back to resource limits only (with a warning)
- For hermetic/pristine/network-isolated containers: **fails hard** rather than running unsafely

## 7.7 Package DNA (Provenance)

Conary tracks the complete provenance of every package through a 4-layer "Package DNA" system (`src/provenance/`). Each layer captures a different aspect of the package's lineage.

### Layer 1: Source Provenance

```rust
pub struct SourceProvenance {
    pub upstream_url: Option<String>,    // Original tarball URL
    pub upstream_hash: Option<String>,   // SHA-256 of upstream source
    pub git_commit: Option<String>,      // Git commit hash
    pub git_repo: Option<String>,        // Git repository URL
    pub git_tag: Option<String>,         // Release tag
    pub patches: Vec<PatchInfo>,         // Applied patches
    pub fetch_timestamp: Option<String>, // When source was downloaded
    pub verified_mirrors: Vec<String>,   // Mirrors that matched the hash
}
```

Patches track not just the diff but the reason:

```rust
pub struct PatchInfo {
    pub url: Option<String>,
    pub hash: Option<String>,
    pub author: Option<String>,
    pub reason: Option<String>,
    pub cve: Option<String>,    // CVE identifier if security fix
    pub level: Option<u32>,     // Patch strip level (-p1, -p2)
}
```

### Layer 2: Build Provenance

```rust
pub struct BuildProvenance {
    pub recipe_hash: Option<String>,     // Hash of the build recipe
    pub build_deps: Vec<BuildDependency>, // Dependencies with recursive DNA
    pub host_attestation: Option<HostAttestation>, // Build machine info
    pub reproducibility: Option<ReproducibilityInfo>,
    pub isolation_level: IsolationLevel,
    pub build_env: BTreeMap<String, String>, // Sanitized environment
}
```

**Build dependencies carry their own DNA hashes**, creating a recursive provenance chain. If any dependency was built differently, the entire chain of DNA hashes changes.

**Host attestation** records the build environment:

```rust
pub struct HostAttestation {
    pub arch: String,           // "x86_64"
    pub kernel: Option<String>, // "6.18.13-200.fc43.x86_64" (from /proc/version)
    pub distro: Option<String>, // "Fedora Linux 43" (from /etc/os-release)
    pub tpm_quote: Option<String>, // TPM 2.0 attestation quote
    pub secure_boot: Option<bool>, // EFI secure boot state
    pub hostname: Option<String>,
}
```

**Reproducibility verification** requires consensus from multiple builders:

```rust
pub struct ReproducibilityInfo {
    pub verified_by: Vec<String>,    // Builder identifiers
    pub consensus: bool,             // true if 2+ builders agree
    pub build_timestamps: Vec<String>,
    pub differing_files: Vec<String>, // Files that differed (if any)
}
```

**Isolation levels**:
- `None` -- No isolation (development builds)
- `Container` -- Default namespace isolation
- `Hermetic` -- Pristine container, network blocked (BuildStream-grade)

### Layer 3: Signature Provenance

```rust
pub struct SignatureProvenance {
    pub builder_sig: Option<Signature>,       // Build system's signature
    pub reviewer_sigs: Vec<Signature>,        // Human reviewer signatures
    pub transparency_log: Option<TransparencyLog>,
    pub sbom: Option<SbomRef>,
}
```

**Signature scopes** allow different reviewers to attest to different aspects:

```rust
pub enum SignatureScope {
    Build,       // "I built this and it compiled"
    Security,    // "I reviewed this for security issues"
    Review,      // "I reviewed the code changes"
    Audit,       // "I audited the full package"
    Performance, // "I benchmarked this version"
    Compliance,  // "This meets regulatory requirements"
}
```

**Transparency log** integration with Sigstore Rekor:

```rust
pub struct TransparencyLog {
    pub provider: String,           // "rekor.sigstore.dev"
    pub log_index: Option<u64>,     // Immutable log entry index
    pub entry_url: Option<String>,  // Direct link to the log entry
    pub inclusion_proof: Option<String>, // Merkle inclusion proof
}
```

**SBOM references** (not inline -- linked by hash):

```rust
pub struct SbomRef {
    pub spdx_hash: Option<String>,      // SHA-256 of SPDX document
    pub cyclonedx_hash: Option<String>, // SHA-256 of CycloneDX document
    pub url: Option<String>,            // URL to retrieve the SBOM
}
```

### Layer 4: Content Provenance

```rust
pub struct ContentProvenance {
    pub merkle_root: String,                        // Root of file hash tree
    pub component_hashes: BTreeMap<String, String>,  // Per-component SHA-256
    pub chunk_manifest: Vec<String>,                 // CDC chunk hashes
    pub total_size: u64,
    pub file_count: u64,
}
```

The `ContentProvenanceBuilder` computes the merkle root from sorted file hashes and per-component aggregates:

```rust
let mut builder = ContentProvenanceBuilder::new();
builder.add_file("/usr/sbin/nginx", &content, "runtime");
builder.add_file("/etc/nginx/nginx.conf", &content, "config");
let provenance = builder.build();
// provenance.merkle_root = SHA-256 of sorted file hashes
// provenance.component_hashes = {"runtime": "abc...", "config": "def..."}
```

### DNA Hash

The Package DNA hash (`src/provenance/dna.rs`) is a single SHA-256 that covers all four layers:

```rust
pub struct DnaHash([u8; 32]);

impl Provenance {
    pub fn dna_hash(&self) -> DnaHash {
        let mut hasher = Sha256::new();
        hasher.update(self.source.canonical_bytes());
        hasher.update(self.build.canonical_bytes());
        hasher.update(self.signatures.canonical_bytes());
        hasher.update(self.content.canonical_bytes());
        DnaHash(hasher.finalize().into())
    }
}
```

The `CanonicalBytes` trait ensures deterministic serialization. Build dependencies are sorted by name. Component hashes use `BTreeMap` (sorted keys). Patches are sorted by hash. This means two builds of the same package from the same source with the same recipe and dependencies will always produce the same DNA hash.

Display: `DnaHash` serializes as a 64-character hex string. The `short()` method returns the first 12 hex characters for human-readable display: `sha256:a1b2c3d4e5f6`.

### SLSA Provenance

Conary generates SLSA (Supply chain Levels for Software Artifacts) provenance in the in-toto Statement v1 format (`src/provenance/slsa.rs`):

```rust
pub fn build_slsa_statement(name: &str, version: &str, provenance: &Provenance) -> Value {
    json!({
        "_type": "https://in-toto.io/Statement/v1",
        "subject": [{
            "name": format!("pkg:conary/{}@{}", name, version),
            "digest": { "sha256": provenance.content.merkle_root }
        }],
        "predicateType": "https://slsa.dev/provenance/v1",
        "predicate": {
            "buildDefinition": {
                "buildType": "https://conary.dev/provenance/build/v1",
                "externalParameters": {
                    "source": provenance.source.upstream_url,
                    "recipe_hash": provenance.build.recipe_hash,
                }
            },
            "runDetails": {
                "builder": { "id": "https://conary.dev/builder" },
                "metadata": {
                    "invocationId": provenance.build.recipe_hash,
                }
            }
        }
    })
}
```

Materials include upstream sources, git commits, and build dependencies with their DNA hashes (using Package URL format: `pkg:conary/{name}@{version}`).

## 7.8 TUF Supply Chain Trust

Conary implements The Update Framework (TUF, spec v1.0.31) for securing repository metadata (`src/trust/`). TUF provides protection against four classes of attack:

| Attack | Protection | Mechanism |
|--------|-----------|-----------|
| Rollback | Version monotonicity | Each metadata version must be strictly greater than the stored version |
| Freeze | Expiration timestamps | Metadata has a short TTL; stale metadata is rejected |
| Arbitrary package | Signature threshold | Metadata must be signed by enough trusted keys |
| Mix-and-match | Snapshot consistency | Snapshot pins exact versions of all other metadata |

### TUF Roles

Four roles with separate keys:

```
root.json        -- Trust anchor. Contains all keys and thresholds. Self-signed.
targets.json     -- Lists all packages with their hashes and lengths.
snapshot.json    -- Records the version of every metadata file.
timestamp.json   -- Points to current snapshot. Short expiry. Updated frequently.
```

### Metadata Types

All metadata uses `BTreeMap` for deterministic JSON serialization:

```rust
pub struct RootMetadata {
    pub version: u64,
    pub expires: DateTime<Utc>,
    pub consistent_snapshot: bool,
    pub keys: BTreeMap<String, TufKey>,        // Key ID -> public key
    pub roles: BTreeMap<String, RoleDefinition>, // Role -> key IDs + threshold
}

pub struct TargetsMetadata {
    pub version: u64,
    pub expires: DateTime<Utc>,
    pub targets: BTreeMap<String, TargetDescription>, // Path -> hash + length
}
```

Every metadata document is wrapped in a `Signed<T>` envelope containing the payload and its Ed25519 signatures:

```rust
pub struct Signed<T> {
    pub signed: T,
    pub signatures: Vec<TufSignature>,  // keyid + hex-encoded sig
}
```

### Client Update Workflow

The `TufClient` (`src/trust/client.rs`) performs the full TUF update:

```
1. Fetch timestamp.json         (~200 bytes, always fetched)
2. Verify timestamp:
   - Signature threshold met?
   - Not expired?
   - Version > stored version?    (rollback protection)
3. If snapshot hash changed:
   - Fetch snapshot.json
   - Verify hash matches timestamp's reference
   - Verify signature + expiry + version monotonicity
4. If root version in snapshot is newer:
   - Walk through each intermediate root version (v2, v3, ..., vN)
   - Verify each against the PREVIOUS root's keys (trust chain)
   - Verify each is also self-signed with its own new keys
5. If targets hash changed:
   - Fetch targets.json
   - Verify hash + signature + expiry + version
6. Cross-check snapshot consistency:
   - Snapshot's root.json version matches current root
   - Snapshot's targets.json version matches fetched targets
7. Persist all verified metadata to database
```

### Verification Primitives

Five core verification functions (`src/trust/verify.rs`):

**Signature verification**: Counts valid Ed25519 signatures against trusted keys. Deduplicates by key ID. Returns error if count < threshold.

**Version monotonicity**: `new_version > stored_version`. Equal versions are also rejected (prevents replay).

**Expiration check**: `now < expires`. Detects freeze attacks where a compromised server serves old-but-valid metadata.

**Hash verification**: SHA-256 of fetched bytes must match the hash pinned in the parent metadata (timestamp pins snapshot's hash, snapshot pins targets' hash).

**Snapshot consistency**: The snapshot must pin the exact versions of root and targets that were actually verified. Prevents an attacker from mixing metadata from different points in time.

### Key Generation and Ceremony

The ceremony module (`src/trust/ceremony.rs`) handles TUF key lifecycle:

```bash
# Generate keys for each role
conary trust key-gen --role root --output /secure/keys/
conary trust key-gen --role targets --output /secure/keys/
conary trust key-gen --role snapshot --output /secure/keys/
conary trust key-gen --role timestamp --output /secure/keys/

# Bootstrap TUF for a repository
conary trust init --repo fedora --key-dir /secure/keys/ --expires 365

# Verify all metadata
conary trust verify --repo fedora

# Check status
conary trust status --repo fedora

# Rotate a compromised key
conary trust rotate-key --role targets \
    --old-key /secure/keys/targets.private \
    --new-key /secure/keys/targets-new.private \
    --root-key /secure/keys/root.private
```

Key rotation creates a new root version signed by both the old root key and the new root key. The client walks through each intermediate version during update, verifying the trust chain at every step.

For development or small repositories, `create_initial_root_single_key()` uses one key for all four roles. Production deployments should use separate keys stored on different media.

### Server-Side TUF

The Remi server serves TUF metadata via HTTP endpoints (`src/server/handlers/tuf.rs`):

```
GET /v1/:distro/tuf/timestamp.json
GET /v1/:distro/tuf/snapshot.json
GET /v1/:distro/tuf/targets.json
GET /v1/:distro/tuf/root.json
GET /v1/:distro/tuf/:version.root.json    # Versioned roots for rotation
```

Metadata is isolated per distro (each distro has its own set of TUF keys and metadata versions). The admin endpoint `POST /v1/admin/tuf/refresh-timestamp` triggers a timestamp refresh with updated snapshot hash.

All database queries use `spawn_blocking` since SQLite access is synchronous.

### Trust CLI Commands

```
conary trust key-gen    -- Generate Ed25519 key pair for a TUF role
conary trust init       -- Bootstrap TUF with initial root.json
conary trust enable     -- Enable TUF verification for a repository
conary trust disable    -- Disable TUF verification (requires --force)
conary trust status     -- Show metadata versions, expiry, key count
conary trust verify     -- Run full TUF update cycle
conary trust sign-targets -- Sign new targets (server-side, feature-gated)
conary trust rotate-key -- Rotate a role's key with old+new+root keys
```

Disabling TUF requires `--force` to prevent accidental downgrade of security guarantees.

## 7.9 Hermetic Build Security

Conary's recipe system enforces build reproducibility through a two-phase model with different security contexts:

**Fetch phase** (network allowed):
- Download sources from upstream
- Verify checksums against recipe declarations
- Cache sources locally for offline rebuilds
- No compilation or code execution

**Build phase** (network blocked):
- Extract, patch, configure, compile, install
- Container isolation with `CLONE_NEWNET` (only loopback)
- No DNS resolution (`/etc/resolv.conf` not mounted)
- All inputs must come from the fetch phase or declared dependencies

```bash
conary cook recipe.toml              # Default: fetch then build (isolated)
conary cook --fetch-only recipe.toml # Just download sources
conary cook --hermetic recipe.toml   # Maximum isolation (pristine container)
conary cook --no-isolation recipe.toml # Unsafe: disable all isolation
```

**Cache invalidation** uses `DependencyHashes` -- a hash over the content of all installed build dependencies (not just their version strings). If any dependency is rebuilt differently (even at the same version), the cache key changes and the package is rebuilt.

## 7.10 Security Architecture Summary

The security layers work together as defense-in-depth:

```
Layer 1: TUF Trust
  Repository metadata is signed and versioned.
  Rollback, freeze, and mix-and-match attacks are detected.

Layer 2: Package DNA
  Every package carries its complete provenance chain.
  Build dependencies include recursive DNA hashes.
  Tampering changes the DNA hash, which is detectable.

Layer 3: Capability Declarations
  Packages declare what they need (network, filesystem, syscalls).
  Inference fills in gaps for packages without declarations.
  Audit detects over-privileged or under-declared packages.

Layer 4: Kernel Enforcement
  Landlock restricts filesystem access (deny-by-default).
  Seccomp-BPF restricts system calls (allowlist).
  Container namespaces isolate process trees and network.
  Resource limits prevent resource exhaustion attacks.

Layer 5: Script Analysis
  Scriptlets are scanned for dangerous patterns before execution.
  High-risk scripts are force-sandboxed.
  Hermetic builds block all network access during compilation.
```

No single layer is sufficient. A compromised repository (layer 1 bypassed) still faces capability enforcement (layer 4). A package with overly broad capabilities (layer 3 insufficient) is still constrained by seccomp filters (layer 4) and its provenance is still traceable (layer 2). A malicious scriptlet (layer 5 missed) is still sandboxed in a container with resource limits.

---

## Chapter 8 -- Advanced Topics

This chapter covers the systems that make Conary more than a package manager: bootstrapping entire OS images from scratch, federating content across a mesh of nodes, computing binary deltas for bandwidth savings, declaring system state as code, building packages from recipes, and automating maintenance with AI assistance. Each system is self-contained but they compose -- a bootstrapped system uses recipes to build packages, federation to distribute them, the system model to declare the target state, and automation to keep it healthy.

### 8.1 Bootstrap: Building an OS from Nothing

The bootstrap system (`src/bootstrap/`) builds a complete Conary-managed Linux distribution from source, starting with nothing but a host compiler. It follows the LFS 12.4 methodology (binutils 2.45, GCC 15.2.0, glibc 2.42, kernel 6.16.1) but automated and resumable.

#### Target Architecture

```rust
pub enum TargetArch {
    X86_64,
    Aarch64,
    Riscv64,
}

impl TargetArch {
    pub fn gnu_triple(&self) -> &str {
        match self {
            Self::X86_64 => "x86_64-conary-linux-gnu",
            Self::Aarch64 => "aarch64-conary-linux-gnu",
            Self::Riscv64 => "riscv64-conary-linux-gnu",
        }
    }
}
```

The GNU triple uses `conary` as the vendor field. This is not cosmetic -- it creates a distinct toolchain target that prevents accidental mixing with host libraries.

#### Configuration

```rust
pub struct BootstrapConfig {
    pub target: TargetArch,
    pub gcc_version: String,       // "15.2.0"
    pub glibc_version: String,     // "2.42"
    pub binutils_version: String,  // "2.45"
    pub kernel_version: String,    // "6.16.1"
    pub musl_version: String,      // "1.2.5"
    pub sysroot: PathBuf,          // /sysroot
    pub stage_dir: PathBuf,        // /stage{0,1,2}
    pub source_cache: PathBuf,     // Cached tarballs
    pub jobs: usize,               // Parallel make jobs (default: nproc)
}
```

Source tarballs are cached locally so repeated bootstrap attempts don't re-download. All source archives require valid SHA-256 checksums -- placeholder checksums are rejected at download time.

#### The Pipeline

The bootstrap proceeds through a staged pipeline. Stage 2 and Conary are optional but recommended for production images:

```
Stage 0 --> Stage 1 --> Stage 2 (optional) --> BaseSystem --> Conary (optional) --> Image
```

```
Stage 0: Cross-Compiler
  Build a GCC cross-compiler targeting x86_64-conary-linux-gnu.
  Uses crosstool-ng methodology: binutils -> kernel headers -> glibc headers ->
  GCC (C only, no shared libs) -> full glibc -> full GCC (C/C++).
  Result: /stage0/bin/x86_64-conary-linux-gnu-gcc

Stage 1: Self-Hosted Toolchain
  Using the Stage 0 cross-compiler, build a native toolchain that runs on the
  target. This is the "Canadian cross" -- the compiler now runs on and targets
  x86_64-conary-linux-gnu.
  Result: /stage1/ with native gcc, binutils, glibc

Stage 2: Pure Rebuild (optional)
  Rebuild the entire toolchain using Stage 1's native compiler. This eliminates
  any contamination from the host system. After Stage 2, every binary was built
  by a Conary-native compiler. Optional for development iteration, recommended
  for production images.
  Result: /stage2/ -- bit-for-bit independent of host

BaseSystem:
  Build essential userspace: coreutils, bash, util-linux, findutils, grep, sed,
  gawk, make, diffutils, file, gzip, xz, tar, pkg-config, ncurses, readline,
  systemd, iproute2, openssh, kernel, boot chain, and networking stack.
  Uses the RecipeGraph for dependency ordering with automatic cycle breaking.
  Per-package checkpointing enables resume after interruption at package
  granularity (not just stage granularity). Build sandboxing uses
  ContainerConfig::pristine_for_bootstrap() for namespace isolation.
  Result: A minimal but complete bootable system

Conary (optional):
  Build Conary itself using the Rust toolchain compiled in earlier stages. This
  is the "self-hosting" step -- the system can now manage its own packages.
  Result: A self-managing system

Image:
  Produce a bootable disk image from the assembled filesystem using
  systemd-repart for rootless image generation (fallback to sfdisk/mkfs on
  systems without systemd-repart). Supports Raw (dd-able), Qcow2 (KVM/QEMU),
  and ISO (optical/USB) formats. UKI (Unified Kernel Image) support is
  available via ukify for direct-boot configurations.
  Result: A deployable OS image
```

#### Stage Management

The `StageManager` tracks progress and enables resume after interruption:

```rust
pub struct StageManager {
    state_file: PathBuf,           // bootstrap-state.json
    completed_stages: Vec<BootstrapStage>,
    current_stage: Option<BootstrapStage>,
}
```

State is persisted to `bootstrap-state.json` after each stage completes. If the process is interrupted, it resumes from the last completed stage. The `reset_from()` method invalidates a stage and all subsequent stages -- if you need to rebuild Stage 1, Stages 2 through Image are also reset.

```rust
pub fn reset_from(&mut self, stage: BootstrapStage) {
    self.completed_stages.retain(|s| (*s as u8) < (stage as u8));
    self.current_stage = None;
    self.save();
}
```

#### Prerequisites Check

Before starting, the bootstrap verifies the host has required tools:

```rust
const REQUIRED_TOOLS: &[&str] = &[
    "gcc", "g++", "make", "bison", "flex", "texinfo",
    "patch", "tar", "xz", "gzip", "wget",
];
```

Missing tools produce a clear error listing what to install.

#### Image Generation

The `ImageBuilder` produces bootable disk images using systemd-repart for rootless image generation. On systems without systemd-repart, it falls back to sfdisk/mkfs. UKI (Unified Kernel Image) support is available via ukify for direct-boot configurations without a separate bootloader.

```rust
pub enum ImageFormat {
    Raw,    // Direct dd to disk
    Qcow2,  // KVM/QEMU virtual machines
    Iso,    // USB/optical boot media
}
```

The GPT partition layout:

```
Partition 1: EFI System Partition (ESP)
  Size: 512 MB
  Filesystem: FAT32
  Contents: systemd-boot or GRUB EFI binary, kernel, initramfs
  Flags: EFI System

Partition 2: Root filesystem
  Size: Remaining disk space
  Filesystem: ext4
  Contents: Complete Conary system
  Flags: Linux root (auto-detected by systemd-gpt-auto)
```

#### CLI

```bash
conary bootstrap                      # Full bootstrap (all stages)
conary bootstrap --target aarch64     # Cross-bootstrap for ARM64
conary bootstrap --resume             # Resume from last checkpoint
conary bootstrap stage0               # Run only Stage 0
conary bootstrap stage1               # Run only Stage 1
conary bootstrap stage2               # Run optional Stage 2 (pure rebuild)
conary bootstrap base                 # Run BaseSystem stage
conary bootstrap conary               # Build Conary itself (self-hosting)
conary bootstrap image --format qcow2 # Produce a VM image
conary bootstrap dry-run              # Validate pipeline without writing
conary bootstrap --reset-from stage1  # Rebuild from Stage 1 onward
conary bootstrap --jobs 8             # Override parallelism
conary bootstrap status               # Show current progress
```

### 8.2 CAS Federation: Distributed Content Sharing

CAS (Content-Addressable Storage) federation (`src/federation/`) enables multiple Conary nodes to share chunks peer-to-peer, reducing bandwidth consumption and improving resilience. Because chunks are content-addressed, any node that has a chunk can serve it -- there's no origin authority for content.

#### Hierarchical Topology

```
             Region Hub (WAN)
            /       |        \
     Cell Hub    Cell Hub    Cell Hub  (LAN segments)
      / | \       / | \       / | \
    Leaf ...    Leaf ...    Leaf ...   (individual nodes)
```

**Leaf**: End-user machines. Request chunks from their cell hub.
**Cell Hub**: Coordinates a LAN segment. Caches chunks for its leaves. Fetches from region hub on miss.
**Region Hub**: WAN-connected central servers. mTLS required for inter-region traffic.

Chunk requests bubble up the hierarchy: leaf -> cell -> region -> upstream Remi server. Chunks flow back down and are cached at each tier.

#### Peer Discovery

Two mechanisms:

**mDNS** (LAN): Nodes advertise `_conary-cas._tcp.local` via multicast DNS. Automatic, zero-config. Cell hubs respond with their chunk count as metadata.

```rust
// Periodic mDNS scan discovers LAN peers
let peers = mdns_scanner.scan(Duration::from_secs(5)).await?;
for peer in peers {
    federation.add_peer(peer.addr, peer.tier)?;
}
```

**Static configuration**: For WAN or environments without multicast:

```toml
[federation]
peers = [
    "https://peer1.conary.io:8080",
    "https://peer2.conary.io:8080",
]
```

#### Peer Selection: Rendezvous Hashing

Given a chunk hash and a set of peers, which peer should we ask first? Conary uses rendezvous hashing (highest random weight):

```rust
fn select_peer(&self, chunk_hash: &str) -> Option<&FederationPeer> {
    self.peers.iter()
        .filter(|p| p.is_healthy())
        .max_by_key(|p| {
            let mut hasher = DefaultHasher::new();
            chunk_hash.hash(&mut hasher);
            p.id.hash(&mut hasher);
            hasher.finish()
        })
}
```

Each (chunk, peer) pair produces a deterministic score. The peer with the highest score wins. This has two properties: (1) the same chunk always maps to the same peer (cache affinity), and (2) when a peer leaves, only its chunks are redistributed (minimal disruption). This is better than consistent hashing for small peer sets.

#### Request Coalescing (Singleflight)

When multiple clients request the same chunk simultaneously, only one upstream fetch happens:

```rust
pub struct RequestCoalescer {
    in_flight: DashMap<String, Arc<Notify>>,
}
```

The first request for a chunk hash starts the fetch and inserts a `Notify` handle. Subsequent requests for the same hash wait on the notification. When the fetch completes, all waiters are notified and read from the now-populated cache.

#### Circuit Breaker

Each peer has a circuit breaker to avoid hammering failing nodes:

```
Closed (normal):  Requests flow through. Track failures.
Open (broken):    All requests immediately fail. Wait for cooldown.
Half-Open:        Allow one probe request. Success -> Closed, failure -> Open.
```

The breaker trips after consecutive failures exceed a threshold. The cooldown period starts short and doubles on each trip (exponential backoff). A peer that has been failing for hours isn't probed every few seconds.

#### Security

**mTLS**: Region hubs require mutual TLS for WAN traffic. Each hub has a certificate signed by a shared CA. Cell-to-cell traffic within a LAN can use plain HTTP.

**Chunk verification**: Every chunk fetched from a peer is verified against its SHA-256 hash before use. A peer cannot serve tampered content -- the chunk hash in the request IS the expected hash.

**Signed manifests**: A chunk manifest (list of hashes for a package) is signed with Ed25519. The signature prevents a peer from adding or removing chunks from a manifest.

**Tier allowlists**: Access can be restricted by tier. A region hub might only serve cell hubs, not individual leaves.

#### Statistics

Federation stats are tracked per-peer and aggregated daily in the `federation_stats` table:

```sql
CREATE TABLE federation_stats (
    peer_id TEXT NOT NULL,
    date TEXT NOT NULL,                -- YYYY-MM-DD
    chunks_served INTEGER DEFAULT 0,
    chunks_fetched INTEGER DEFAULT 0,
    bytes_saved INTEGER DEFAULT 0,     -- Bytes not fetched from upstream
    ...
);
```

```bash
conary federation stats --days 7
# Peer: cell-hub-1.local
#   Chunks served:  12,847
#   Chunks fetched:  3,241
#   Bytes saved:    847 MB  (73% bandwidth reduction)
```

### 8.3 Delta Updates: Binary Diffs

Delta updates (`src/delta/`) reduce download sizes by sending only the differences between package versions. Conary uses zstd dictionary compression for this -- a technique that leverages the old version's content as a compression dictionary for the new version.

#### How It Works

```rust
pub struct DeltaGenerator {
    compression_level: i32,  // zstd level (default 3)
}

impl DeltaGenerator {
    pub fn generate(&self, old_content: &[u8], new_content: &[u8]) -> Result<DeltaResult> {
        // 1. Use old_content as a zstd dictionary
        // 2. Compress new_content with that dictionary
        // 3. The result is small when old and new are similar
        let dict = zstd::dict::EncoderDictionary::copy(old_content, self.compression_level);
        let compressed = zstd::stream::encode_all_with_dict(new_content, &dict)?;
        Ok(DeltaResult {
            delta_data: compressed,
            old_hash: sha256(old_content),
            new_hash: sha256(new_content),
            old_size: old_content.len() as u64,
            new_size: new_content.len() as u64,
            delta_size: compressed.len() as u64,
        })
    }
}
```

This is different from traditional binary diff (bsdiff, xdelta). Zstd dictionary compression is:
- Faster to generate (no match-finding across large files)
- Streaming-friendly (can decompress without seeking)
- Built on a widely-deployed library (no additional dependencies)
- Good enough -- most updates change a small fraction of the content

#### Applying Deltas

```rust
pub struct DeltaApplier;

impl DeltaApplier {
    pub fn apply(&self, old_content: &[u8], delta_data: &[u8]) -> Result<Vec<u8>> {
        let dict = zstd::dict::DecoderDictionary::copy(old_content);
        let new_content = zstd::stream::decode_all_with_dict(delta_data, &dict)?;
        Ok(new_content)
    }
}
```

The client needs the old content as the dictionary. Since Conary uses CAS, the old content is retrieved by its hash from the local chunk store.

#### Worthiness Check

Not all deltas are worth generating. If the package changed completely (major version bump, different upstream), the delta might be larger than the new content:

```rust
pub struct DeltaMetrics {
    pub old_size: u64,
    pub new_size: u64,
    pub delta_size: u64,
}

impl DeltaMetrics {
    pub fn is_worthwhile(&self) -> bool {
        // Skip if delta is more than 90% of the new content
        self.delta_size < (self.new_size * 9 / 10)
    }

    pub fn savings_percent(&self) -> f64 {
        if self.new_size == 0 { return 0.0; }
        (1.0 - self.delta_size as f64 / self.new_size as f64) * 100.0
    }
}
```

Typical savings for minor version updates: 60-90%. For security patches (one or two changed files): 95-99%.

#### Integration with CAS

Deltas operate on individual chunks, not whole packages. The CDC chunking (Chapter 6) means most chunks between versions are identical. The delta system only needs to handle the chunks that actually changed:

```
nginx 1.24.0 -> 1.26.0:
  247 chunks total
  231 chunks unchanged (93.5%) -- already in local CAS
   16 chunks changed -- apply delta or download full
    Delta for 16 chunks: 142 KB (vs 1.8 MB full download)
```

#### Hash Verification

After applying a delta, the reconstructed content is verified against its expected SHA-256 hash. A corrupted delta or wrong old-content dictionary produces garbage that fails the hash check. This is a hard error -- the client falls back to a full download.

### 8.4 System Model: Declarative OS State

The system model (`src/model/`) lets you describe your desired system state in a TOML file and have Conary converge the actual state to match. Think NixOS `configuration.nix` or Ansible playbooks, but integrated into the package manager.

#### Model File

```toml
# /etc/conary/system.toml

[system]
hostname = "webserver-01"
timezone = "America/New_York"

[packages]
include = [
    "nginx", "postgresql", "redis",
    "certbot", "fail2ban",
]
exclude = ["telnet", "ftp"]

[packages.pinned]
nginx = "1.26.0"           # Don't auto-update
postgresql = "~16"          # Stay on major version 16

[[collections]]
name = "monitoring"
url = "https://packages.conary.io/collections/monitoring.toml"

[[collections]]
name = "hardened-base"
url = "https://packages.conary.io/collections/hardened-base.toml"

[automation]
security_updates = "auto"    # Apply security patches automatically
orphan_cleanup = "suggest"   # Suggest removing orphans, don't auto-remove
major_upgrades = "disabled"  # Never auto-upgrade major versions

[automation.ai]
mode = "advisory"            # AI suggestions but no auto-action

[federation]
enabled = true
tier = "leaf"
```

#### Model Resolution

The `SystemModel` parser reads the TOML and resolves all includes into a flat `ResolvedModel`:

```rust
pub struct SystemModel {
    pub system: SystemSection,
    pub packages: PackageSection,
    pub collections: Vec<CollectionRef>,
    pub automation: AutomationConfig,
    pub federation: Option<FederationConfig>,
}

pub struct ResolvedModel {
    pub packages: BTreeSet<String>,     // All packages (direct + from collections)
    pub excluded: BTreeSet<String>,     // Explicitly excluded
    pub pinned: BTreeMap<String, String>, // Version pins
    pub automation: AutomationConfig,
}
```

Resolution follows a layered composition model. Collections are fetched (with Ed25519 signature verification) and merged:

```
Base system.toml
  + collection: monitoring (adds prometheus, grafana, node-exporter)
  + collection: hardened-base (adds aide, rkhunter; excludes telnet)
  = Resolved model
```

#### Conflict Resolution

When multiple sources disagree (e.g., one collection includes `telnet`, the base model excludes it):

```rust
pub enum ConflictStrategy {
    LocalWins,   // Local system.toml takes precedence
    RemoteWins,  // Remote collection takes precedence
    Error,       // Fail with a conflict error
}
```

Default is `LocalWins` -- the local admin's `system.toml` always has the final say.

#### Cycle Detection

Collection includes can reference other collections. Conary detects circular references with a depth limit:

```rust
const MAX_INCLUDE_DEPTH: usize = 10;
```

If collection A includes B which includes A, resolution fails with a clear error at depth 10 rather than recursing infinitely.

#### Model Diff

The `ModelDiff` computes the difference between the current system state and the desired model:

```rust
pub enum DiffAction {
    Install(String),              // Package not installed, model wants it
    Remove(String),               // Package installed, model excludes it
    Upgrade(String, String, String), // Version differs (name, current, target)
    Downgrade(String, String, String),
    Pin(String, String),          // Add version pin
    Unpin(String),                // Remove version pin
    CollectionAdd(String),        // New collection to track
    CollectionRemove(String),     // Collection no longer referenced
    ConfigChange(String, String, String), // Config key changed (key, old, new)
}
```

Nine action types. The diff is a complete description of what needs to change to reach the desired state. It can be displayed for review or applied automatically.

#### System State Capture

The `capture_system_state()` function snapshots the current system from SQLite:

```rust
pub fn capture_system_state(conn: &Connection) -> Result<SystemState> {
    let packages = query_installed_packages(conn)?;
    let pinned = query_pinned_packages(conn)?;
    let collections = query_active_collections(conn)?;
    // ... hostname, timezone from system
    Ok(SystemState { packages, pinned, collections, ... })
}
```

The inverse function `snapshot_to_model()` converts a captured state back to a `SystemModel`. This is useful for exporting the current state of one machine to replicate on another.

#### Lockfile

The lockfile (`/etc/conary/system.lock`) records the exact resolved state:

```toml
[metadata]
generated = "2026-03-03T14:22:00Z"
model_hash = "sha256:abc123..."      # Hash of system.toml

[[packages]]
name = "nginx"
version = "1.26.0-1"
hash = "sha256:def456..."            # CCS package hash

[[packages]]
name = "postgresql"
version = "16.6-2"
hash = "sha256:789abc..."
```

`model_hash` is a SHA-256 of the normalized system.toml. Drift detection compares the current lockfile against the current model -- if the model changed but the lockfile didn't, you have unapplied changes.

```bash
conary model diff        # Show what would change
conary model apply       # Converge to desired state
conary model lock        # Generate/update lockfile
conary model check       # Detect drift (lockfile vs actual)
conary model export      # Export current state as system.toml
```

#### Remote Collections

Collections are published to a Remi server with Ed25519 signatures:

```rust
pub fn publish_collection(
    collection: &CollectionManifest,
    signing_key: &Ed25519SecretKey,
    endpoint: &str,
) -> Result<()> {
    let signed = sign_collection(collection, signing_key)?;
    upload_to_endpoint(&signed, endpoint)?;
    Ok(())
}
```

When fetching a remote collection, the signature is verified against the publisher's public key. A tampered collection is rejected before it can influence the resolved model.

### 8.5 Recipe System: Building from Source

The recipe system (`src/recipe/`) defines how to build packages from source using TOML-based recipe files. It draws from Arch's PKGBUILD, Gentoo's ebuilds, and BuildStream's hermetic model, with a culinary metaphor throughout.

#### Recipe Format

```toml
[recipe]
name = "nginx"
version = "1.26.0"
release = 1
description = "High-performance HTTP server and reverse proxy"
license = "BSD-2-Clause"
url = "https://nginx.org"

[source]
url = "https://nginx.org/download/nginx-%(version)s.tar.gz"
sha256 = "abc123..."

[[patches]]
url = "https://example.com/nginx-hardened.patch"
sha256 = "def456..."
level = 1

[dependencies]
makedepends = ["openssl-devel", "pcre2-devel", "zlib-devel"]
requires = ["openssl", "pcre2", "zlib"]

[build]
configure = """
./configure \
    --prefix=/usr \
    --conf-path=/etc/nginx/nginx.conf \
    --sbin-path=/usr/sbin/nginx \
    --with-http_ssl_module \
    --with-http_v2_module \
    --with-pcre-jit
"""
make = "make -j%(jobs)s"
install = "make DESTDIR=%(destdir)s install"
```

#### Variable Substitution

Recipe files support `%(variable)s` substitution:

| Variable | Value |
|----------|-------|
| `%(name)s` | Package name |
| `%(version)s` | Package version |
| `%(release)s` | Release number |
| `%(jobs)s` | Parallel build jobs |
| `%(destdir)s` | Staging directory for `make install` |
| `%(srcdir)s` | Source directory |
| `%(arch)s` | Target architecture |

#### The Kitchen

The `Kitchen` (`src/recipe/kitchen.rs`) manages the build environment:

```rust
pub struct Kitchen {
    config: KitchenConfig,
    cook: Cook,
}

pub struct KitchenConfig {
    pub work_dir: PathBuf,       // Build workspace
    pub source_cache: PathBuf,   // Cached source tarballs
    pub package_output: PathBuf, // Where built packages go
    pub isolation: IsolationConfig,
}
```

#### Cook: The Build State Machine

The `Cook` is a 5-phase state machine:

```
Prep     -> Resolve makedepends, verify source checksums, set up workspace
Unpack   -> Extract source tarball, apply patches in order
Patch    -> (Part of unpack -- patches are applied sequentially by strip level)
Simmer   -> Run configure, make (the actual compilation)
Plate    -> Run make install into destdir, package the result as CCS
```

Each phase can fail independently. The `Cook` records which phases completed, so a failed build can be resumed from the last successful phase.

```rust
pub enum CookPhase {
    Prep,
    Unpack,
    Patch,
    Simmer,
    Plate,
}
```

#### Cross-Compilation

Recipes support cross-compilation via a `[cross]` section, used during bootstrap:

```toml
[cross]
host = "x86_64-conary-linux-gnu"
target = "aarch64-conary-linux-gnu"
stage = "stage1"  # stage0, stage1, stage2, or final
```

```rust
pub enum BuildStage {
    Stage0,  // Cross-compiler build
    Stage1,  // Self-hosted first build
    Stage2,  // Pure rebuild
    Final,   // Normal package build
}
```

The `BuildStage` determines which toolchain to use, what sysroot to target, and whether to link against the host or target libraries.

#### Makedepends Resolution

The `MakedependsResolver` ensures all build dependencies are installed before compilation starts:

```rust
pub struct MakedependsResolver {
    db: Connection,
}

impl MakedependsResolver {
    pub fn resolve(&self, makedepends: &[String]) -> Result<ResolveResult> {
        let mut missing = Vec::new();
        for dep in makedepends {
            if !self.is_installed(dep)? {
                missing.push(dep.clone());
            }
        }
        Ok(ResolveResult { missing, satisfied: makedepends.len() - missing.len() })
    }
}
```

Missing makedepends are installed automatically before the build starts and can optionally be removed afterward.

#### Recipe Validation

The parser (`src/recipe/parser.rs`) validates recipes with two severity levels:

**Hard errors** (build fails):
- Missing `[recipe]` section
- Missing name or version
- Missing `[source]` section
- Missing source URL or checksum
- Invalid version format

**Soft warnings** (build proceeds):
- Missing description or license
- Empty makedepends (suspicious but valid)
- Unusual configure/make commands

#### PKGBUILD Converter

The `PkgbuildConverter` (`src/recipe/pkgbuild.rs`) translates Arch Linux PKGBUILDs into Conary recipes:

```rust
pub fn convert_pkgbuild(pkgbuild_content: &str) -> Result<Recipe> {
    let parsed = parse_pkgbuild(pkgbuild_content)?;
    Ok(Recipe {
        name: parsed.pkgname,
        version: parsed.pkgver,
        release: parsed.pkgrel.parse()?,
        description: parsed.pkgdesc,
        source: convert_sources(&parsed.source, &parsed.sha256sums)?,
        dependencies: convert_depends(&parsed)?,
        build: convert_build_function(&parsed.build, &parsed.package)?,
        ..Default::default()
    })
}
```

This handles the common cases: `source` arrays with checksums, `depends`/`makedepends` arrays, `build()` and `package()` functions. Arch-specific variables (`$srcdir`, `$pkgdir`) are mapped to Conary equivalents (`%(srcdir)s`, `%(destdir)s`).

#### CLI

```bash
conary cook recipe.toml              # Build a package from recipe
conary cook --fetch-only recipe.toml # Download sources only
conary cook --hermetic recipe.toml   # Maximum isolation
conary cook --no-isolation recipe.toml # Disable sandboxing (unsafe)
conary cook --stage simmer recipe.toml # Run up to a specific phase
conary recipe convert nginx.PKGBUILD   # Convert PKGBUILD to recipe
conary recipe validate recipe.toml     # Check recipe without building
```

### 8.6 Automated Maintenance

The automation system (`src/automation/`) monitors the system and suggests (or applies) maintenance actions. It follows a "suggest + confirm" model by default -- the system tells you what it thinks should happen and waits for approval.

#### Automation Modes

```rust
pub enum AutomationMode {
    Disabled,  // No automation
    Suggest,   // Detect issues, suggest fixes, wait for approval
    Auto,      // Apply fixes automatically (with logging)
}
```

Each category of maintenance has its own mode, configured in the system model:

```toml
[automation]
security_updates = "auto"     # Critical: auto-apply
orphan_cleanup = "suggest"    # Suggest but don't auto-remove
major_upgrades = "disabled"   # Never auto-upgrade major versions
repair = "suggest"            # Suggest repairs for corrupted files
```

#### Categories

**Security Updates**: Check for packages with known CVEs. Risk assessment is inverted -- a critical CVE gets a *low* risk score (0.2) because NOT updating is the risky action:

```rust
fn security_risk_score(severity: &str) -> f64 {
    match severity {
        "critical" => 0.2,  // Low risk to update (high risk NOT to)
        "high" => 0.3,
        "medium" => 0.5,
        "low" => 0.7,       // Higher risk to update (disruption > threat)
        _ => 0.5,
    }
}
```

**Orphan Cleanup**: Detect packages that were installed as dependencies but are no longer needed by any installed package. Uses a grace period -- recently-orphaned packages aren't immediately flagged:

```rust
pub struct OrphanDetector {
    grace_period: Duration,  // Don't flag orphans younger than this
}
```

**File Integrity**: Verify installed files against their SHA-256 hashes in the database. Detects corruption, unauthorized modifications, and missing files.

**Update Checks**: Query repositories for available updates. Separate from security updates -- these are feature/performance improvements.

#### Pending Actions

Every suggested action is wrapped in a `PendingAction`:

```rust
pub struct PendingAction {
    pub id: u64,
    pub category: ActionCategory,
    pub description: String,
    pub risk_level: f64,          // 0.0 (safe) to 1.0 (dangerous)
    pub packages: Vec<String>,    // Affected packages
    pub reversible: bool,         // Can this be rolled back?
    pub created: DateTime<Utc>,
}
```

The risk level determines the default behavior:
- `risk < 0.3` -- Auto-apply if mode is `Auto`
- `0.3 <= risk < 0.7` -- Suggest regardless of mode
- `risk >= 0.7` -- Always require explicit confirmation

#### AI-Assisted Maintenance

The automation system can optionally consult an AI model for complex decisions:

```rust
pub enum AiAssistMode {
    Disabled,   // No AI involvement
    Advisory,   // AI suggests, human decides
    Assisted,   // AI acts on low-risk, human on high-risk
    Autonomous, // AI handles everything (requires explicit opt-in)
}
```

```rust
pub struct AiSuggestion {
    pub action: String,
    pub reasoning: String,
    pub confidence: f64,
    pub risk_assessment: String,
}
```

In `Advisory` mode, the AI analyzes the situation and explains its reasoning, but takes no action. In `Assisted` mode, it can apply changes with risk < 0.3. `Autonomous` mode requires explicit opt-in in the system model and is intended for test/CI environments.

#### CLI

```bash
conary automation status           # Show pending suggestions
conary automation check            # Run all detection checks
conary automation apply <id>       # Apply a specific suggestion
conary automation apply --all      # Apply all safe suggestions
conary automation dismiss <id>     # Dismiss a suggestion
conary automation history          # Show past actions
```

### 8.7 Transaction Engine: Crash-Safe Operations

Every package operation in Conary runs inside a transaction (`src/transaction/mod.rs`). The transaction engine guarantees that the system is never left in a half-installed state, even if power is lost mid-operation.

#### State Machine

Each transaction moves through 10 phases:

```
NEW          Transaction object created
PLANNED      Dependency resolution complete, operation plan finalized
PREPARED     Packages downloaded, checksums verified
PRE_SCRIPTS  Pre-install/pre-remove scriptlets executed
BACKED_UP    Existing files backed up (for rollback)
STAGED       New files staged in temporary directory
FS_APPLIED   New files moved to final locations      <-- Point of no return
DB_APPLIED   Database updated (troves, files, deps)
POST_SCRIPTS Post-install/post-remove scriptlets run
DONE         Transaction complete, journal cleaned
```

#### Journal-Based Recovery

Every phase transition is recorded in a journal file before the operation executes:

```rust
pub struct TransactionJournal {
    journal_dir: PathBuf,      // /var/lib/conary/journal/
    tx_uuid: String,           // Unique transaction ID
}
```

If the system crashes, the next Conary operation scans the journal directory. For each incomplete transaction:

- **Before FS_APPLIED**: Roll back. Restore backups, remove staged files, revert database.
- **After FS_APPLIED**: Roll forward. The filesystem changes are committed; complete the remaining phases (database update, post-scripts, cleanup).

The FS_APPLIED phase is the point of no return because filesystem operations can't be atomically undone across multiple files. Before that point, we have complete backups. After that point, we've already moved files to their final locations and the old versions are gone.

#### Atomic File Operations

Individual file installations use atomic moves:

```rust
pub fn move_file_atomic(src: &Path, dest: &Path) -> io::Result<()> {
    match fs::rename(src, dest) {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
            // Cross-filesystem: copy + fsync + delete
            fs::copy(src, dest)?;
            let file = fs::File::open(dest)?;
            file.sync_all()?;  // fsync to ensure data on disk
            fs::remove_file(src)?;
            Ok(())
        }
        Err(e) => Err(e),
    }
}
```

On the same filesystem, `rename()` is atomic -- the file appears at the new path or doesn't. Cross-filesystem moves use copy + fsync + delete. The fsync ensures the file content is on disk before we delete the source.

#### Backup Strategy

Before overwriting any existing file, the transaction engine creates a backup:

```
/var/lib/conary/backup/{tx-uuid}/
  usr/sbin/nginx              # Original binary
  etc/nginx/nginx.conf        # Original config
  ...
```

The backup directory mirrors the filesystem hierarchy. On rollback, files are restored from backup to their original locations. After successful completion, backups are deleted.

#### Configuration

```rust
pub struct TransactionConfig {
    pub root: PathBuf,          // Filesystem root (usually /)
    pub db_path: PathBuf,       // SQLite database location
    pub txn_dir: PathBuf,       // Working directory for staging
    pub journal_dir: PathBuf,   // Journal files
    pub lock_timeout: Duration, // How long to wait for the transaction lock
}
```

Only one transaction can run at a time, enforced by a file lock. The `lock_timeout` determines how long a second transaction waits before failing with a "database locked" error.

### 8.8 Putting It All Together

These systems compose into workflows that span the entire lifecycle:

**Bootstrap a new distro**:
1. Bootstrap builds a minimal system from source (8.1)
2. Recipes define how each package is built (8.5)
3. The transaction engine ensures each package installation is atomic (8.7)
4. The system model describes the final desired state (8.4)

**Deploy to a fleet**:
1. Define the desired state in `system.toml` (8.4)
2. Include remote collections for role-specific packages (8.4)
3. Set up federation so machines share chunks on the LAN (8.2)
4. Enable automation for security updates (8.6)
5. Delta updates minimize bandwidth for ongoing maintenance (8.3)

**Update safely**:
1. Automation detects available security patches (8.6)
2. Delta generation computes minimal downloads (8.3)
3. The transaction engine creates backups before applying changes (8.7)
4. If a scriptlet fails, the transaction rolls back to the backup state (8.7)
5. The lockfile records the new resolved state (8.4)

**Air-gapped deployment**:
1. On an internet-connected machine, `conary model lock` resolves and downloads everything (8.4)
2. Copy the lockfile and chunk cache to removable media
3. On the air-gapped machine, `conary model apply --offline` installs from the local cache
4. No network access needed -- all chunks are content-addressed and pre-verified

---

## Conclusion

Conary is a ground-up rethinking of Linux package management. It doesn't patch over decades of accumulated assumptions -- it starts from first principles and builds upward.

**Content-addressable storage** means every file is stored by its hash. Deduplication is automatic, verification is intrinsic, and any node that has a chunk can serve it.

**Atomic transactions** mean the system is never half-installed. Power loss during an upgrade is a recoverable event, not a crisis.

**Multi-format ingestion** means Conary doesn't fight the existing ecosystem. RPM, DEB, and Arch packages are converted to a unified format, preserving their metadata while gaining CAS benefits.

**Declarative state** means you describe what you want, not how to get there. The system model converges reality to match your specification.

**Defense-in-depth security** means capabilities, kernel enforcement, provenance tracking, and TUF verification all work together so that no single point of compromise is fatal.

**Federation** means bandwidth scales with the network, not the origin server. Every cache is a potential source for its neighbors.

The design is intentionally modular. You can use Conary as a simple package installer and never touch the system model. You can run a single Remi server and never set up federation. You can ignore recipes and only consume pre-built packages. Each layer adds capability without requiring the layers above it.

This is what package management looks like when you don't have to be backwards-compatible with 1995.
