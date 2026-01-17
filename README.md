# Conary

**Package management rebuilt for reliability.**

Package management is fundamentally broken. You're locked to one distro's format, updates are coin flips that might brick your system, and when things go wrong you're reinstalling from scratch. We're stuck with tools designed when "rollback" meant restore from backup.

Conary is a modern package manager inspired by the [original Conary](https://en.wikipedia.org/wiki/Conary_(package_manager)) that was criminally ahead of its time. Written in Rust for safety. Built on SQLite for queryability. Designed for atomic operations from day one.

---

## Key Features

### Atomic Transactions
Every operation is a changeset - a transactional move from one system state to another. It works completely or not at all. No half-configured systems.

```bash
conary install nginx           # Creates changeset, applies atomically
conary system state rollback 5 # Full rollback - database AND filesystem
```

### Multi-Format, One Tool
RPM, DEB, Arch packages - Conary speaks all of them. Stop letting package format dictate your OS choice.

```bash
conary install ./package.rpm
conary install ./package.deb
conary install ./package.pkg.tar.zst
```

### Component Model
Packages are automatically split into components. Install only what you need.

```bash
conary install nginx:runtime   # Just the binaries
conary install openssl:devel   # Just headers and libs for building
conary install package:all     # Everything
```

### Declarative System Model
Define your desired system state in TOML. Conary computes and applies the diff.

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
```

```bash
conary model diff    # What needs to change?
conary model apply   # Make it so
conary model check   # CI/CD drift detection
```

### Collections & Groups
Group packages together. Install entire stacks with one command.

```bash
conary collection create web-stack --members nginx,postgresql,redis
conary install @web-stack
```

### Time Travel
Every system state is tracked. Rollback isn't an afterthought - it's core functionality.

```bash
conary system state list       # See all snapshots
conary system state diff 5 8   # Compare states
conary system state rollback 5 # Go back
```

### Native CCS Package Format
Build reproducible, signed packages with automatic quality enforcement.

```bash
conary ccs build .             # Build from ccs.toml
conary ccs sign pkg.ccs        # Ed25519 signatures
conary ccs export pkg --format oci  # Export to container
```

### Dev Shells (Nix-style)
Temporary environments without permanent install.

```bash
conary ccs shell python,nodejs  # Spawn shell with packages
conary ccs run gcc -- make      # One-shot command execution
```

### Content-Addressable Storage
Git-style file storage with deduplication, integrity verification, and delta updates.

```bash
conary system verify nginx     # SHA-256 verification
conary system restore nginx    # Restore from CAS
conary query delta-stats       # Bandwidth savings
```

### Sandboxed Scriptlets
Install scripts run in namespace isolation with resource limits.

```bash
conary install pkg --sandbox=always  # Force sandboxing
conary install pkg --sandbox=never   # Trust the scripts
```

---

## Quick Start

```bash
# Initialize
conary system init

# Add a repository
conary repo add fedora https://example.com/fedora/packages
conary repo sync

# Install packages
conary install nginx --dry-run  # Preview
conary install nginx            # Do it

# Query your system
conary list                     # All packages
conary query depends nginx      # Dependencies
conary query whatprovides libc.so.6

# Adopt existing packages
conary system adopt --system    # Track everything installed by RPM/APT
```

---

## Technical Foundation

- **Rust** - Memory-safe, no segfaults in your package manager
- **SQLite** - Queryable state, transactional operations
- **Ed25519** - Package signatures
- **SHA-256 + XXH128** - Integrity verification and fast CAS
- **CBOR** - Binary manifests with Merkle tree verification
- **Namespace isolation** - Sandboxed scriptlet execution

---

## Status

**Core architecture complete.** Component model, collections, multi-format support, dependency resolution, atomic transactions, and the full CCS pipeline (build, sign, verify, install, export) are working. 558 tests passing.

---

## Documentation

| Document | Description |
|----------|-------------|
| [Conaryopedia](docs/conaryopedia.md) | Original Conary concepts and architecture |
| [CCS Format Spec](docs/specs/ccs-format-v1.md) | Native package format specification |
| [Scriptlet Security](docs/SCRIPTLET_SECURITY.md) | Sandboxing and isolation |

For CLI reference, run `conary --help` or `man conary`.

---

## Building

```bash
cargo build --release
cargo test
```

---

## License

MIT License
