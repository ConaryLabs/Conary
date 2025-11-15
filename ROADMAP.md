# Conary Package Manager - Phased Roadmap

## Current State
**Phase 9B Complete** - Delta updates with zstd compression implemented with 90 tests passing. Core package management functionality is working: install, remove, rollback, verify, query, dependency resolution, repository management, and delta updates all functional with full filesystem deployment, atomic transactions, remote package discovery, and bandwidth-efficient updates.

**Completed:** Phases 0-8, Phase 9A-9B (Database, schema, core models, RPM support, changeset transactions, file operations with CAS, dependency resolution, CLI polish, repository management, delta updates)
**Next:** Phase 10 (Multi-Format Support) and beyond

See PROGRESS.md for detailed session-by-session implementation notes.

---

## Phase 0: Project Bootstrap ✓ (COMPLETE)
- Vision and architecture documentation
- Core concepts finalized (troves, changesets, flavors)
- Technology stack decided (Rust 1.91.1, SQLite)
- Development standards established (CLAUDE.md)
- Progress tracking framework (PROGRESS.md)

---

## Phase 1: Foundation & Project Setup ✓ (COMPLETE)
**Goal**: Get a minimal Rust project running with database connectivity

**Deliverables**:
- `Cargo.toml` with core dependencies (rusqlite, thiserror, anyhow)
- Basic project structure: `src/main.rs`, `src/lib.rs`, `src/db/mod.rs`
- File header conventions in place
- Database connection management
- Basic CLI skeleton (just help/version)
- Integration test framework setup
- CI configuration (cargo test, clippy, rustfmt checks)

**Success Criteria**: `cargo build` works, can open/close SQLite database

---

## Phase 2: Database Schema & Core Layer ✓ (COMPLETE)
**Goal**: Design and implement the foundational database schema

**Deliverables**:
- SQLite schema with core tables:
  - `troves` - package/component/collection metadata
  - `changesets` - transactional operations history
  - `files` - file-level tracking with hashes
  - `flavors` - build-time variations
  - `provenance` - supply chain tracking
  - `dependencies` - trove relationships
- Schema migration system
- Database initialization logic
- CRUD operations for core entities
- Transaction wrapper utilities
- Comprehensive database tests

**Success Criteria**: Can create database, run migrations, perform basic CRUD operations

---

## Phase 3: Core Abstractions & Data Models ✓ (COMPLETE)
**Goal**: Build the Rust types and abstractions that represent core concepts

**Deliverables**:
- `Trove` struct with variants (Package, Component, Collection)
- `Changeset` struct with state machine (Pending → Applied → Rolled Back)
- `FileEntry` with content hashing (SHA-256)
- `Flavor` representation and parsing
- `Provenance` chain tracking
- Error types with thiserror
- Serialization/deserialization from database
- Unit tests for all core types

**Success Criteria**: Can create, persist, and retrieve core entities from database

---

## Phase 4: Package Format Support (First Format) ✓ (COMPLETE)
**Goal**: Implement support for ONE package format (choose RPM, DEB, or Arch)

**Decision**: **RPM** chosen and implemented (most complex, good test case)

**Deliverables**:
- RPM file parser (header, payload extraction)
- Metadata extraction (name, version, arch, dependencies)
- File list extraction
- Conversion to Trove representation
- Integration tests with real RPM files
- Error handling for malformed packages

**Success Criteria**: Can parse RPM, extract metadata, store as Trove in database

---

## Phase 5: Changeset Transaction Model ✓ (COMPLETE)
**Goal**: Implement atomic operations with rollback capability

**Deliverables**:
- Changeset creation (install, remove, update operations)
- Pre-transaction validation (dependency checks, conflict detection)
- Atomic application of changesets
- Rollback mechanism (restore previous state)
- File conflict detection and resolution
- Transaction logging and history
- Comprehensive transaction tests

**Success Criteria**: Can install/remove packages atomically, rollback on failure

---

## Phase 6: File-Level Operations ✓ (COMPLETE)
**Goal**: Implement file installation, tracking, and integrity checking

**Deliverables**:
- File extraction and installation
- SHA-256 hashing for all installed files
- File ownership and permissions handling
- Conflict detection (file already exists)
- File verification against hashes
- Orphan file detection
- File-level rollback support

**Success Criteria**: Can install files to filesystem, verify integrity, detect changes

---

## Phase 7: Dependency Resolution
**Goal**: Implement dependency graph building and resolution

**Deliverables**:
- Dependency graph construction
- Topological sorting for install order
- Conflict detection (version incompatibilities)
- Optional dependency handling
- Circular dependency detection
- "What breaks if I remove X?" queries
- Efficient SQLite queries for dep resolution

**Success Criteria**: Can resolve dependencies for complex package installations

---

## Phase 8: CLI Interface (Basic Commands) - ✓ COMPLETE
**Goal**: Build user-facing command-line interface

**Deliverables**:
- ✓ `conary install <trove>` - install packages (DONE in Phase 4-6)
- ✓ `conary remove <trove>` - remove packages with dependency checking (DONE in Phase 5+7)
- ✓ `conary rollback <changeset-id>` - rollback to previous state (DONE in Phase 5-6)
- ✓ `conary query <pattern>` - search installed packages (DONE in Phase 5)
- ✓ `conary verify [package]` - check file integrity (DONE in Phase 6)
- ✓ `conary history` - show changeset history (DONE in Phase 5)
- ✓ `conary depends <package>` - show dependencies (DONE in Phase 7)
- ✓ `conary rdepends <package>` - show reverse dependencies (DONE in Phase 7)
- ✓ `conary whatbreaks <package>` - show breaking packages (DONE in Phase 7)
- ✓ `conary completions <shell>` - generate shell completions (DONE in Phase 8)
- ✓ Shell completion scripts for bash, zsh, fish, powershell (DONE in Phase 8)
- ✓ Man pages via clap_mangen (DONE in Phase 8)
- ⏰ `conary update` - update packages (DEFERRED - requires repository management from Phase 9+)

**Success Criteria**: Can perform basic package operations from command line ✓ COMPLETE

---

## Phase 9A: Repository Management - ✓ COMPLETE
**Goal**: Implement remote package repository support

**Deliverables:**
- ✓ Database schema v4 with repositories and repository_packages tables
- ✓ Repository and RepositoryPackage models with full CRUD operations
- ✓ HTTP client with retry and timeout support (reqwest)
- ✓ JSON-based repository metadata format
- ✓ Repository metadata synchronization with expiry checking
- ✓ Package checksum verification (SHA-256)
- ✓ CLI commands: repo-add, repo-list, repo-remove, repo-enable, repo-disable, repo-sync
- ✓ Search command for repository package discovery
- ✓ Basic update command stub (full implementation in future)
- ✓ 67 tests passing with comprehensive repository test coverage

**Success Criteria**: Can add repositories, sync metadata, search packages ✓ COMPLETE

---

## Phase 9B: Delta Updates ✓ (COMPLETE)
**Goal**: Implement efficient binary delta updates

**Implementation**: zstd dictionary compression (using old file as dictionary)

**Deliverables**:
- Delta generation between file versions using zstd ✓
- Delta application and verification with hash checking ✓
- Bandwidth usage metrics tracked in database ✓
- Fallback to full file download on delta failure ✓
- Tests with comprehensive delta scenarios ✓
- CLI command for viewing delta statistics ✓

**Success Criteria**: Updates download only changed portions of files ✓

---

## Phase 10: Multi-Format Support
**Goal**: Add support for DEB and Arch package formats

**Deliverables**:
- DEB parser and metadata extraction
- Arch (pkg.tar.zst) parser
- Format auto-detection
- Unified Trove representation across formats
- Cross-format dependency resolution
- Tests for all three formats

**Success Criteria**: Can install RPM, DEB, and Arch packages in same system

---

## Phase 11: Flavor System
**Goal**: Implement build-time variation tracking

**Deliverables**:
- Flavor syntax parser (`package[feature,!other]`)
- Flavor storage and querying
- Flavor-aware dependency resolution
- Build flag tracking (arch, toolchain, features)
- Flavor conflict detection

**Success Criteria**: Can track and query packages by build-time features

---

## Phase 12: Component Auto-Split
**Goal**: Implement automatic package component splitting

**Deliverables**:
- Component detection (`:runtime`, `:devel`, `:doc`, `:locale`)
- Automatic file classification
- Component-level dependency resolution
- Smart defaults (install :runtime by default)
- Component querying and selection

**Success Criteria**: Can install just development headers without runtime files

---

## Phase 13: System Integration
**Goal**: Integrate with system package managers and boot process

**Deliverables**:
- systemd integration (if needed)
- Boot-time verification
- Integration with existing package managers (as fallback)
- System recovery mode
- Secure boot considerations
- Performance optimization for production use

**Success Criteria**: Can use as primary system package manager safely

---

## Phase 14: Advanced Features & Polish
**Goal**: Add nice-to-have features and optimize

**Deliverables**:
- Repository management
- Package signing and verification
- Mirror support
- Parallel downloads
- Progress indicators and UX improvements
- Performance profiling and optimization
- Memory usage optimization
- Comprehensive documentation
- Tutorial and examples

**Success Criteria**: Production-ready package manager

---

## Maintenance & Future Phases
- Bug fixes and stability improvements
- Security updates
- Community feature requests
- Performance tuning
- Additional package format support
- Plugin system (if needed)

---

## Decision Points

**Resolved:**
1. ✓ **Phase 4**: RPM chosen as first format (implemented)
2. ✓ **CI/CD**: GitHub Actions (implemented with test, clippy, rustfmt, security audit)
3. ✓ **Logging**: `tracing` with `tracing-subscriber` (implemented)
4. ✓ **CLI**: `clap` v4.5 with derive macros (implemented)

**To Be Decided:**
1. **Phase 9**: Which delta algorithm to use? (Recommendation: zstd)
2. **Phase 10**: Order for additional formats (DEB vs Arch next?)

---

## Notes

- Each phase should update PROGRESS.md upon completion
- Tests are mandatory at every phase
- Database schema changes require migration scripts
- Keep dependency tree lean throughout
- Review and refine roadmap as we learn
- All code must follow standards in CLAUDE.md
- Database-first architecture: NO config files for runtime state
