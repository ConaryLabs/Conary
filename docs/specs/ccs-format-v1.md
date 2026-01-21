# CCS Package Format Specification v1

## Overview

CCS (Conary Component Specification) is a package format designed for:
- Content-addressable storage and delta-efficient updates
- Declarative hooks instead of shell scripts
- Component-based installation (install only what you need)
- Cross-distro compatibility via capability-based dependencies
- Cryptographic verification via Merkle tree structure

## File Extensions

- `.ccs` - Packed package file (self-contained, for distribution)
- `ccs.toml` - Source manifest (human-authored, in project root)

---

## Part 1: ccs.toml Manifest Schema

The `ccs.toml` file is the human-authored package definition.

### Minimal Example

```toml
[package]
name = "myapp"
version = "1.2.3"
description = "My application"

[provides]
capabilities = ["cli-tool"]
```

### Complete Example

```toml
[package]
name = "myapp"
version = "1.2.3"
description = "A sample application demonstrating CCS features"
license = "MIT"
homepage = "https://github.com/example/myapp"
repository = "https://github.com/example/myapp.git"

# Platform targeting (optional - defaults to current platform)
[package.platform]
os = "linux"
arch = "x86_64"
libc = "gnu"  # or "musl"
# abi = "x86-64-v2"  # optional CPU baseline

# Package maintainer/author
[package.authors]
maintainers = ["Jane Doe <jane@example.com>"]
upstream = "Upstream Project <upstream@example.com>"

# What this package provides (capability-based)
[provides]
capabilities = [
    "cli-tool",
    "json-parsing",
]
# Auto-detected (filled in by ccs build):
# sonames = ["libmyapp.so.1"]
# binaries = ["/usr/bin/myapp"]

# What this package requires (capability-based)
[requires]
capabilities = [
    { name = "tls", version = ">=1.2" },
    { name = "glibc", version = ">=2.31" },
]
# Fallback to package names when capabilities don't exist
packages = [
    { name = "openssl", version = ">=3.0" },
]

# Optional dependencies
[suggests]
capabilities = ["shell-completion"]

# Component configuration
[components]
# Override auto-classification for specific paths (glob patterns)
overrides = [
    { path = "/usr/share/myapp/plugins/*", component = "runtime" },
    { path = "/usr/lib/myapp/*.a", component = "devel" },
]
# Override auto-classification for specific files (exact paths)
# Use this when the classifier gets it wrong
[components.files]
"/usr/bin/my-weird-helper" = "lib"      # Force into :lib instead of :runtime
"/usr/share/myapp/required.txt" = "runtime"  # Not doc, actually needed at runtime

# Which components install by default
default = ["runtime", "lib", "config"]

# Declarative hooks
[hooks]

# System users/groups (sysusers-style)
[[hooks.users]]
name = "myapp"
system = true
home = "/var/lib/myapp"
shell = "/sbin/nologin"
# group = "myapp"  # defaults to same as name

[[hooks.groups]]
name = "myapp-admin"
system = true

# Directories with ownership (tmpfiles-style)
[[hooks.directories]]
path = "/var/lib/myapp"
mode = "0750"
owner = "myapp"
group = "myapp"

[[hooks.directories]]
path = "/var/log/myapp"
mode = "0755"
owner = "myapp"
group = "myapp"
# cleanup = "on-uninstall"  # optional: remove on package removal

# Systemd integration
[[hooks.systemd]]
unit = "myapp.service"
enable = false  # install but don't enable by default
# enable = true  # auto-enable on install

# tmpfiles.d entries (for runtime directories)
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
# only_if_lower = true  # only set if current value is lower

# Alternatives system
[[hooks.alternatives]]
name = "editor"
path = "/usr/bin/myapp-edit"
priority = 50

# Configuration files (noreplace behavior)
[config]
files = [
    "/etc/myapp/config.toml",
    "/etc/myapp/users.conf",
]
# noreplace = true  # default: preserve user modifications

# Build provenance (filled in by ccs build, or manually specified)
[build]
source = "https://github.com/example/myapp.git"
commit = "abc123def456"
timestamp = "2026-01-15T10:30:00Z"
# For reproducible builds:
# environment = { CC = "gcc", CFLAGS = "-O2" }
# commands = ["cargo build --release"]

# Build policies for quality enforcement
[policy]
reject_paths = ["/home/*", "/tmp/*", "*.pyc"]  # Reject these paths
strip_binaries = true                           # Strip debug symbols from ELF binaries
normalize_timestamps = true                     # Set mtimes to SOURCE_DATE_EPOCH or fixed value
compress_manpages = true                        # Gzip man pages
fix_shebangs = { "/usr/bin/env python" = "/usr/bin/python3" }

# Metadata for legacy format generation
[legacy]
# RPM-specific
rpm.group = "Applications/System"
rpm.requires = ["systemd"]  # additional RPM-only deps

# DEB-specific
deb.section = "utils"
deb.priority = "optional"
deb.depends = ["systemd"]  # additional DEB-only deps

# Arch-specific
arch.groups = ["base-devel"]
```

### Schema Reference

#### [package] Section (Required)

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| name | string | yes | Package name (alphanumeric, hyphens) |
| version | string | yes | Semantic version (X.Y.Z) |
| description | string | yes | Short description |
| type | string | no | Package type: "package" (default), "group", or "redirect" |
| license | string | no | SPDX license identifier |
| homepage | string | no | Project homepage URL |
| repository | string | no | Source repository URL |

#### Package Types

**package** (default): A normal package containing files.

**group**: A composition package containing only references to other packages. Groups are the building blocks for OS composition (inspired by Foresight Linux's group-* recipes).

```toml
[package]
name = "group-server"
version = "1.0.0"
type = "group"
description = "Minimal server installation"

[members]
required = [
    { name = "group-base", version = ">=1.0" },
    { name = "openssh", version = ">=9.0" },
    { name = "systemd" },
]
optional = [
    { name = "nginx", version = ">=1.24" },
    { name = "postgresql", version = ">=15" },
]
```

**redirect**: A transition package that redirects to another package. Used for package renames, splits, or replacements.

```toml
[package]
name = "mysql"
version = "999.0.0"  # High version ensures upgrade
type = "redirect"
description = "Transitional package - mysql has been replaced by mariadb"

[redirect]
to = "mariadb"
version = ">=10.11"
reason = "MySQL replaced by MariaDB in this distribution"
```

#### Groups and System Models

Groups can be created in two ways:

1. **From ccs.toml** - Build directly with `conary ccs build` using `type = "group"`
2. **From system.toml** - Publish a system model with `conary model publish`

The system model (`/etc/conary/system.toml`) is a declarative specification of desired system state. When published, it becomes a versioned group that other systems can subscribe to:

```toml
# /etc/conary/system.toml
[model]
version = 1
install = ["nginx", "postgresql", "redis"]
exclude = ["sendmail"]

[pin]
openssl = "3.0.*"

[include]
models = ["group-base-server@corp:production"]
on_conflict = "local"  # local | remote | error
```

```bash
# Publish as a group
conary model publish --name webserver --version 1.0.0 --repo local

# Other systems can now include it
# [include]
# models = ["group-webserver@local:stable"]
```

The `[include]` directive enables composable system definitions - a base server group can be extended with application-specific packages, and upstream changes propagate automatically.

#### [redirects] Section (Optional)

Declare package evolution - renames, deprecations, merges, and splits:

```toml
[redirects]
# Package renames (old name → this package)
[[redirects.renames]]
old_name = "python3-foo"
version = ">=2.0"  # optional: only applies to versions >= 2.0
message = "Renamed for consistency"

# Obsoleted packages (should be removed, optionally replaced)
[[redirects.obsoletes]]
name = "deprecated-tool"
version = "<3.0"  # optional: only obsoletes versions before 3.0
replaced_by = "modern-tool"  # optional: suggest replacement
message = "This package is deprecated"

# Merge from multiple packages into this one
[[redirects.merges]]
packages = ["libfoo", "libfoo-utils"]
since_version = "2.0.0"
message = "Libraries consolidated"

# Split from a monolithic package
[[redirects.splits]]
from_package = "monolithic-app"
since_version = "3.0.0"
components = ["core", "plugins"]  # which parts this package contains
message = "Split for modularity"
```

#### [package.platform] Section (Optional)

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| os | string | "linux" | Target OS |
| arch | string | detected | CPU architecture (x86_64, aarch64, etc.) |
| libc | string | "gnu" | C library (gnu, musl) |
| abi | string | none | CPU feature baseline (x86-64-v2, etc.) |

#### [provides] Section

| Field | Type | Description |
|-------|------|-------------|
| capabilities | string[] | Semantic capabilities this package provides |
| sonames | string[] | Shared library sonames (auto-detected) |
| binaries | string[] | Executable paths (auto-detected) |
| pkgconfig | string[] | pkg-config files (auto-detected) |

#### [requires] Section

| Field | Type | Description |
|-------|------|-------------|
| capabilities | object[] | Required capabilities with version constraints |
| packages | object[] | Fallback package dependencies (name-based) |

#### [hooks] Section

##### hooks.users / hooks.groups

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| name | string | required | User/group name |
| system | bool | false | Create as system user (low UID) |
| home | string | /nonexistent | Home directory |
| shell | string | /sbin/nologin | Login shell |
| group | string | same as name | Primary group |

##### hooks.directories

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| path | string | required | Directory path |
| mode | string | "0755" | Octal permissions |
| owner | string | "root" | Owner user |
| group | string | "root" | Owner group |
| cleanup | string | none | "on-uninstall" to remove on package removal |

##### hooks.systemd

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| unit | string | required | Unit file name |
| enable | bool | false | Enable on install |

##### hooks.alternatives

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| name | string | required | Alternative group name |
| path | string | required | Path to this package's implementation |
| priority | int | 50 | Priority (higher wins) |

---

## Part 2: .ccs Binary Package Layout

A `.ccs` file is a **gzip-compressed tar archive** with the following structure:

```
myapp-1.2.3.ccs (gzipped tar)
├── MANIFEST              # Binary manifest (CBOR-encoded)
├── MANIFEST.sig          # Ed25519 signature (optional, created by ccs-sign)
├── MANIFEST.toml         # Human-readable manifest (for debugging)
├── components/
│   ├── runtime.json      # File list for :runtime component
│   ├── devel.json        # File list for :devel component
│   ├── doc.json          # File list for :doc component
│   └── config.json       # File list for :config component
└── objects/
    ├── ab/
    │   └── cdef1234567890...  # Content blob (SHA-256 named)
    └── 12/
        └── 34567890abcdef...  # Content blob
```

### MANIFEST Structure

The MANIFEST file is CBOR-encoded for compact binary representation:

```rust
struct Manifest {
    // Format version
    format_version: u8,  // = 1

    // Package metadata (from ccs.toml [package])
    name: String,
    version: String,
    description: String,
    license: Option<String>,

    // Platform
    platform: Platform,

    // Dependencies
    provides: Vec<Capability>,
    requires: Vec<Requirement>,

    // Components (hashes of component file lists)
    components: HashMap<String, ComponentRef>,

    // Hooks
    hooks: Hooks,

    // Build provenance
    build: Option<BuildInfo>,

    // Merkle root of all content
    content_root: Hash,
}

struct ComponentRef {
    hash: Hash,           // SHA-256 of component JSON file
    file_count: u32,
    total_size: u64,
    default: bool,        // Install by default?
}

struct Hash {
    algorithm: String,    // "sha256"
    value: [u8; 32],
}
```

### Component File Lists (JSON)

Each `components/*.json` file contains the file list for that component:

```json
{
  "component": "runtime",
  "files": [
    {
      "path": "/usr/bin/myapp",
      "hash": "sha256:abcdef1234567890...",
      "size": 1048576,
      "mode": 493,
      "owner": "root",
      "group": "root",
      "type": "file"
    },
    {
      "path": "/usr/lib/libmyapp.so.1",
      "hash": "sha256:1234567890abcdef...",
      "size": 524288,
      "mode": 493,
      "owner": "root",
      "group": "root",
      "type": "file"
    },
    {
      "path": "/usr/lib/libmyapp.so",
      "target": "libmyapp.so.1",
      "type": "symlink"
    }
  ]
}
```

### File Entry Schema

**IMPORTANT**: The CAS stores only content blobs. All metadata (permissions, ownership,
symlink targets) MUST be stored in the component manifest, not derived from the CAS.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| path | string | yes | Absolute path |
| type | string | yes | "file", "symlink", "directory" |
| hash | string | if file | Content hash (sha256:...) of full file |
| size | u64 | if file | File size in bytes |
| mode | u32 | **yes** | Unix permissions as integer (e.g., 493 = 0o755) |
| owner | string | yes | Owner username |
| group | string | yes | Group name |
| target | string | if symlink | Symlink target (required for symlinks) |
| chunks | string[] | optional | Ordered list of chunk hashes (if CDC-chunked) |

**Mode field is mandatory** - without it, executables install as non-executable (0644)
and the package breaks. Installers MUST apply the mode from the manifest, never infer
from content.

**Symlink target is mandatory for symlinks** - the CAS does not store symlink metadata.
The target field contains the literal symlink target string.

**Chunks field** - When present, the file content is stored as Content-Defined Chunks
instead of a single blob. Each chunk is stored by its SHA-256 hash in `objects/`.
To reassemble the file, concatenate chunks in the order listed.

### Objects Directory

Content blobs stored by hash, using 2-character prefix directories:

```
objects/
├── ab/
│   └── cdef1234567890abcdef1234567890abcdef1234567890abcdef12345678
└── 12/
    └── 34567890abcdef1234567890abcdef1234567890abcdef1234567890ab
```

- Filename is the full SHA-256 hash (64 hex characters)
- Content is stored uncompressed (compression happens at transport layer)
- Symlinks stored as text file containing target path
- With CDC chunking, objects may be either whole files OR chunks

### Content-Defined Chunking (CDC)

When packages are built with `--chunked`, large files (>16KB) are split into
variable-size chunks using the FastCDC algorithm. This enables efficient delta
updates: when a file changes, only the affected chunks need to be downloaded.

**Algorithm Parameters:**
- Minimum chunk size: 16 KB
- Average chunk size: 64 KB
- Maximum chunk size: 256 KB
- Hash algorithm: SHA-256

**Key Properties:**
- Chunk boundaries are determined by content, not position
- A small change affects only 1-2 chunks, not the entire file
- Identical content across packages shares the same chunk hash

**Example: CDC-chunked File Entry**

```json
{
  "path": "/usr/bin/myapp",
  "hash": "fac3398b9ed3a3b737bd3a05b2ffa78dcf0ff756ee2cd3afa3dbb2bd695c7bc3",
  "size": 15646328,
  "mode": 493,
  "type": "file",
  "chunks": [
    "fe0380ee0b2bfa97d9f42193fcdd5b4312ebf152c46e2b86e42021e4f17c827f",
    "97cae74371638ddbb2d93fb6a59917c88d4c01662c7e4a8475b606fe48f32084",
    "1dbaf7668fa3b20f593ff51a80a623a388875a7865635cf3d69650ef78c239f2"
  ]
}
```

**Reassembly:**
To reconstruct the file, fetch each chunk from `objects/` by its hash and
concatenate them in order. Verify the result matches the file's `hash` field.

**Delta Update Flow:**
1. Client has version 1.0.0 with chunks [A, B, C, D, E]
2. Server publishes version 1.0.1 with chunks [A, B, X, D, E]
3. Client only downloads chunk X (B and C changed to become X)
4. Bandwidth savings: ~80% for typical updates

---

## Part 3: Verification

### Signature Verification

1. `MANIFEST.sig` contains an Ed25519 signature (raw bytes or base64)
2. Verify signature over `MANIFEST` file bytes using the public key
3. Public keys are managed via `ccs-keygen` (generates Ed25519 keypair)
4. Sign packages with `ccs-sign --key <private.pem>`
5. Verify with `ccs-verify <package.ccs>` (checks Merkle tree and optional signature)

### Content Verification

1. Parse `MANIFEST`, extract `content_root`
2. For each component in `components/`:
   - Verify JSON file hash matches `ComponentRef.hash`
   - For each file entry, verify content matches `file.hash`
3. Reject if any hash mismatch

### Merkle Root Calculation

```
content_root = SHA256(
    sorted([
        SHA256(component_name || component_hash)
        for each component
    ])
)
```

---

## Part 4: Installation Flow

```
1. Download .ccs file
2. Verify MANIFEST.sig against trust policy
3. Parse MANIFEST
4. Select components to install (default or user-specified)
5. For each selected component:
   a. Parse component JSON
   b. For each file:
      - Check if hash exists in local CAS
      - If not, extract from objects/ to CAS
   c. Deploy files from CAS to filesystem
6. Execute declarative hooks:
   a. Create users/groups
   b. Create directories
   c. Install systemd units
   d. Update alternatives
7. Run system triggers (ldconfig, etc.)
8. Record in database
```

---

## Part 5: Legacy Export

When generating `.deb`, `.rpm`, or `.pkg.tar.zst`:

1. Parse MANIFEST and component files
2. Generate format-specific metadata
3. Convert declarative hooks to maintainer scripts
4. Package files from objects/
5. Report lossiness (what couldn't be translated)

### Hook Translation

| CCS Hook | DEB | RPM | Arch |
|----------|-----|-----|------|
| hooks.users | postinst: useradd | %pre: useradd | .INSTALL: post_install useradd |
| hooks.directories | postinst: mkdir/chown | %post: mkdir/chown | .INSTALL: post_install |
| hooks.systemd.enable=true | postinst: systemctl enable | %post: systemctl enable | .INSTALL: post_install |
| hooks.systemd.enable=false | (just install file) | (just install file) | (just install file) |
| hooks.alternatives | postinst: update-alternatives | %post: alternatives | (manual) |

---

## Appendix A: MIME Types and Magic

- MIME type: `application/vnd.conary.ccs`
- File extension: `.ccs`
- Magic bytes: First file in tar is always `MANIFEST`

## Appendix B: Reserved Component Names

| Name | Description |
|------|-------------|
| runtime | Executables and runtime libraries |
| lib | Shared libraries only |
| devel | Headers, static libs, pkg-config |
| doc | Documentation, man pages |
| config | Configuration file templates |
| debuginfo | Debug symbols |
| test | Test suites |

## Appendix C: Capability Namespace (V1)

For V1, capabilities use simple names. Future versions will use URIs.

### Named Capabilities

| Namespace | Example | Description |
|-----------|---------|-------------|
| (none) | `tls-1.3` | General capability |
| `abi:` | `abi:glibc-2.31` | ABI requirement |
| `soname:` | `soname:libssl.so.3` | Shared library |
| `bin:` | `bin:python3` | Executable |
| `pkgconfig:` | `pkgconfig:openssl` | pkg-config module |

### Network Capabilities

Declare what network access a package requires:

| Capability | Example | Description |
|------------|---------|-------------|
| `network.listen` | `network.listen:443` | Can bind/listen on port |
| `network.outbound` | `network.outbound:443` | Can connect to remote port |

Port ranges supported: `network.listen:8000-9000`

### Filesystem Capabilities

Declare what filesystem access a package requires:

| Capability | Example | Description |
|------------|---------|-------------|
| `filesystem.read` | `filesystem.read:/etc/ssl` | Can read from path |
| `filesystem.write` | `filesystem.write:/var/cache/nginx` | Can write to path |
| `filesystem.execute` | `filesystem.execute:/usr/lib/cgi-bin` | Can execute from path |

Glob patterns supported: `/var/cache/*`

### Capability Resolution

The capability resolver matches requirements to providers:

```
ssl                      → Named capability lookup in provides table
soname(libssl.so.3)      → Typed capability lookup (kind=soname)
network.listen:443       → Network capability from [capabilities] section
filesystem.read:/etc/ssl → Filesystem capability from [capabilities] section
```

## Appendix D: ccs.lock Lockfile Format

The `ccs.lock` file records exact dependency versions for reproducible builds.

### Location

- `ccs.lock` in the same directory as `ccs.toml`

### Format

```toml
# Auto-generated lockfile - do not edit manually
# Generated: 2026-01-21T15:30:00Z

[metadata]
version = 1
generated_by = "conary 0.2.0"
manifest_hash = "sha256:abc123..."  # Hash of ccs.toml

[[dependencies]]
name = "openssl"
version = "3.0.12"
content_hash = "sha256:def456..."
source = "https://repo.example.com/packages"
kind = "runtime"  # runtime, build, optional, dev
dna_hash = "sha256:789abc..."  # Full provenance hash (optional)

[[dependencies]]
name = "zlib"
version = "1.3.1"
content_hash = "sha256:ghi789..."
kind = "runtime"

# Platform-specific dependencies
[platform_deps.x86_64-linux-gnu]
[[platform_deps.x86_64-linux-gnu.dependencies]]
name = "glibc"
version = "2.38"
content_hash = "sha256:..."
kind = "runtime"
```

### Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| name | string | yes | Package name |
| version | string | yes | Exact version |
| content_hash | string | yes | SHA-256 of package content |
| source | string | no | Repository URL |
| kind | string | yes | runtime, build, optional, dev |
| dna_hash | string | no | Full provenance (Package DNA) hash |

### Usage

```bash
# Generate lockfile from resolved dependencies
conary lock

# Install using lockfile (reproducible)
conary install --locked

# Update lockfile
conary lock --update

# Verify lockfile matches resolved deps
conary lock --verify
```
