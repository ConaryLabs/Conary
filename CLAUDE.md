# CLAUDE.md

This project uses **Mira** for persistent memory and code intelligence.

## Session Start

```
project(action="start", project_path="/home/peter/Conary")
```

Then `recall("architecture")` and `recall("progress")` before making changes.

---

## CRITICAL: Tool Selection

STOP before using Grep or Glob. Use Mira tools instead.

### When to Use Mira Tools

Use Mira tools proactively in these scenarios:

1. **Searching for code by intent** - Use `semantic_code_search` instead of Grep
2. **Understanding file structure** - Use `get_symbols` instead of grepping for definitions
3. **Tracing call relationships** - Use `find_callers` / `find_callees` instead of grepping function names
4. **Checking if a feature exists** - Use `check_capability` instead of exploratory grep
5. **Recalling past decisions** - Use `recall` before making architectural changes
6. **Storing decisions for future sessions** - Use `remember` after important choices

### When NOT to Use Mira Tools

Use Grep/Glob directly only when:

1. Searching for **literal strings** (error messages, UUIDs, specific constants)
2. Finding files by **exact filename pattern** when you know the name
3. The search is a simple one-off that doesn't need semantic understanding

### Wrong vs Right

| Task | Wrong | Right |
|------|-------|-------|
| Find authentication code | `grep -r "auth"` | `semantic_code_search("authentication")` |
| What calls this function? | `grep -r "function_name"` | `find_callers("function_name")` |
| List functions in file | `grep "fn " file.rs` | `get_symbols(file_path="file.rs")` |
| Check if feature exists | `grep -r "feature"` | `check_capability("feature description")` |
| Use external library | Guess from training data | Context7: `resolve-library-id` → `query-docs` |
| Find config files | `find . -name "*.toml"` | `glob("**/*.toml")` - OK, exact pattern |
| Find error message | `semantic_code_search("error 404")` | `grep "error 404"` - OK, literal string |

---

## External Documentation (Context7)

### CRITICAL: Use Context7 for Library Questions

Before guessing at library APIs or using potentially outdated knowledge, check Context7.

**Proactive triggers - use Context7 when:**
1. **Implementing with external libraries** - Check current API before writing code
2. **Debugging library errors** - Verify correct usage patterns
3. **User asks "how do I use X"** - Get up-to-date examples
4. **Uncertain about library API** - Don't guess, look it up
5. **Library version matters** - Context7 has version-specific docs

**Workflow:**
```
resolve-library-id(libraryName="tokio", query="async runtime spawn tasks")
query-docs(libraryId="/tokio-rs/tokio", query="how to spawn async tasks")
```

### When NOT to Use Context7

- Standard library features (Rust std, Python builtins, etc.)
- You're confident in the API from recent experience
- Simple operations with well-known patterns

---

## Task and Goal Management

### Session Workflow: Use Claude's Built-in Tasks

For current session work, use Claude Code's native task system:
- `TaskCreate` - Create tasks for multi-step work
- `TaskUpdate` - Mark in_progress/completed, set dependencies
- `TaskList` - View current session tasks

These are session-scoped and optimized for real-time workflow tracking.

### Cross-Session Planning: Use Mira Goals

For work spanning multiple sessions, use Mira's `goal` tool with milestones:

```
goal(action="create", title="Implement auth system", priority="high")
goal(action="add_milestone", goal_id="1", milestone_title="Design API", weight=2)
goal(action="add_milestone", goal_id="1", milestone_title="Implement endpoints", weight=3)
goal(action="complete_milestone", milestone_id="1")  # Auto-updates progress
goal(action="list")  # Shows goals with progress %
goal(action="get", goal_id="1")  # Shows goal details with milestones
```

**When to use goals:**
- Multi-session objectives (features, refactors, migrations)
- Tracking progress over time
- Breaking large work into weighted milestones

**Goal statuses:** planning, in_progress, blocked, completed, abandoned

**Priorities:** low, medium, high, critical

### Quick Reference

| Need | Tool |
|------|------|
| Track work in THIS session | Claude's `TaskCreate` |
| Track work across sessions | Mira's `goal` |
| Add sub-items to goal | `goal(action="add_milestone")` |
| Check long-term progress | `goal(action="list")` |

---

## Memory System

Use `remember` to store decisions and context. Use `recall` to retrieve them.

### Evidence Threshold

**Don't store one-off observations.** A pattern seen once is not yet a pattern. Only use `remember` for:
- Patterns observed **multiple times** across sessions
- Decisions **explicitly requested** by the user to remember
- Mistakes that caused **real problems** (not hypothetical issues)

When uncertain, don't store it. Memories accumulate and dilute recall quality.

### When to Use Memory

1. **After architectural decisions** - Store the decision and reasoning
2. **User preferences discovered** - Store for future sessions
3. **Mistakes made and corrected** - Remember to avoid repeating
4. **Before making changes** - Recall past decisions in that area
5. **Workflows that worked** - Store successful patterns

---

## Sub-Agent Context Injection

When spawning sub-agents (Task tool with Explore, Plan, etc.), they do NOT automatically have access to Mira memories. You must inject relevant context into the prompt.

### Pattern: Recall Before Task

Before launching a sub-agent for significant work:

1. Use `recall()` to get relevant context
2. Include key information in the Task prompt
3. Be explicit about project conventions

---

## Expert Consultation

Use the unified `consult_experts` tool for second opinions before major decisions:

```
consult_experts(roles=["architect"], context="...", question="...")
consult_experts(roles=["code_reviewer", "security"], context="...")  # Multiple experts
```

**Available expert roles:**
- `architect` - system design, patterns, tradeoffs
- `plan_reviewer` - validate plans before coding
- `code_reviewer` - find bugs, quality issues
- `security` - vulnerabilities, hardening
- `scope_analyst` - missing requirements, edge cases

### When to Consult Experts

1. **Before major refactoring** - `consult_experts(roles=["architect"], ...)`
2. **After writing implementation plan** - `consult_experts(roles=["plan_reviewer"], ...)`
3. **Before merging significant changes** - `consult_experts(roles=["code_reviewer"], ...)`
4. **When handling user input or auth** - `consult_experts(roles=["security"], ...)`
5. **When requirements seem incomplete** - `consult_experts(roles=["scope_analyst"], ...)`

---

## Code Navigation Quick Reference

| Need | Tool |
|------|------|
| Search by meaning | `semantic_code_search` |
| File structure | `get_symbols` |
| What calls X? | `find_callers` |
| What does X call? | `find_callees` |
| Past decisions | `recall` |
| Feature exists? | `check_capability` |
| Codebase overview | `project(action="start")` output |
| External library API | Context7: `resolve-library-id` → `query-docs` |
| Literal string search | `Grep` (OK for this) |
| Exact filename pattern | `Glob` (OK for this) |

---

## Consolidated Tools Reference

Mira uses action-based tools. Here are the key ones:

### `project` - Project/Session Management
```
project(action="start", project_path="...", name="...")  # Initialize session
project(action="set", project_path="...", name="...")    # Change active project
project(action="get")                                     # Show current project
```

### `goal` - Cross-Session Goals
```
goal(action="create", title="...", priority="high")       # Create goal
goal(action="list")                                       # List goals
goal(action="add_milestone", goal_id="1", milestone_title="...", weight=2)
goal(action="complete_milestone", milestone_id="1")       # Mark done
```

### `finding` - Code Review Findings
```
finding(action="list", status="pending")                  # List findings
finding(action="review", finding_id=123, status="accepted", feedback="...")
finding(action="stats")                                   # Get statistics
```

### `documentation` - Documentation Tasks
```
documentation(action="list", status="pending")            # List doc tasks
documentation(action="skip", task_id=123, reason="...")   # Skip a task
documentation(action="inventory")                         # Show doc inventory
```

### `consult_experts` - Expert Consultation
```
consult_experts(roles=["architect"], context="...", question="...")
consult_experts(roles=["code_reviewer", "security"], context="...")
```

## Build & Test

```bash
cargo build --release                    # Client-only (default)
cargo build --release --features server  # With Remi server
cargo build --release --features daemon  # With conaryd daemon
cargo test
cargo test --features daemon             # Include daemon tests
cargo clippy -- -D warnings
```

## Core Principles

**Database-First**: All state lives in SQLite. No config files. No INI/TOML/YAML/JSON for runtime state.

**File Headers**: Every Rust source file starts with its path as a comment:
```rust
// src/main.rs
```

**No Emojis**: Use text markers instead:
- `[COMPLETE]` not checkmarks
- `[IN PROGRESS]` not spinners
- `[FAILED]` not X marks

**Rust Standards**:
- Edition 2024, Rust 1.92
- `thiserror` for error types
- Clippy-clean (pedantic encouraged)
- Tests in same file as code

## Architecture Quick Reference

- **Trove**: Core unit (package, component, collection)
- **Changeset**: Atomic transaction (install/remove/rollback)
- **Flavor**: Build variations (arch, features)
- **CAS**: Content-addressable storage for files

## Key Modules

| Module | Purpose |
|--------|---------|
| `src/db/` | SQLite schema, models, migrations |
| `src/packages/` | RPM/DEB/Arch parsers (unified via `common.rs` PackageMetadata) |
| `src/compression/` | Unified decompression (Gzip, Xz, Zstd) with format detection |
| `src/repository/` | Remote repos, metadata sync |
| `src/resolver/` | Dependency graph, topological sort |
| `src/filesystem/` | CAS, file deployment |
| `src/delta/` | Binary delta updates |
| `src/version/` | Version parsing, constraints |
| `src/container/` | Scriptlet sandboxing, namespace isolation |
| `src/trigger/` | Post-install trigger system |
| `src/scriptlet/` | Scriptlet execution, cross-distro support |
| `src/label/` | Package provenance labels |
| `src/flavor/` | Build variation specs |
| `src/components/` | Component classification |
| `src/transaction/` | Crash-safe atomic operations, journal-based recovery |
| `src/model/` | System Model - declarative OS state (parser, diff, state capture, remote includes, publishing) |
| `src/ccs/` | CCS native package format, builder, policy engine, OCI export, lockfile, redirects |
| `src/server/` | Remi server - on-demand CCS conversion proxy (feature-gated: `--features server`) |
| `src/cli/` | CLI definitions (primary commands at root; system/query with nested state/trigger/redirect/label) |
| `src/commands/` | Command implementations |
| `src/commands/install/` | Package installation (resolve, prepare, execute submodules) |
| `src/recipe/` | Recipe system for building packages from source (kitchen, parser, format, pkgbuild converter, hermetic builds) |
| `src/capability/` | Capability declarations for packages (network, filesystem, syscalls) - audit, enforcement, inference, and resolver |
| `src/provenance/` | Package DNA / full provenance tracking (source, build, signatures, content) |
| `src/automation/` | Automated maintenance (security updates, orphan cleanup, AI-assisted operations) |
| `src/bootstrap/` | Bootstrap a complete Conary system from scratch |
| `src/federation/` | CAS federation - peer discovery, chunk routing, manifests, mTLS, mDNS |
| `src/daemon/` | conaryd daemon - REST API, SSE events, job queue, systemd integration (feature-gated: `--features daemon`) |

## Database Schema

Currently v36. Tables: troves, changesets, files, flavors, provenance, dependencies, repositories, repository_packages, file_contents, file_history, package_deltas, delta_stats, provides, scriptlets, components, component_dependencies, component_provides, collection_members, triggers, trigger_dependencies, changeset_triggers, system_states, state_members, labels, label_path, config_files, config_backups, converted_packages, derived_packages, chunk_access, redirects, package_resolution, provenance_sources, provenance_builds, provenance_signatures, provenance_content, provenance_verifications, capabilities, capability_audits, federation_peers, federation_stats, daemon_jobs, subpackage_relationships.

Key schema additions:
- v8: `provides` - capability tracking for dependency resolution
- v9: `scriptlets` - package install/remove hooks
- v11: `components`, `component_dependencies`, `component_provides` - component model
- v12: `install_reason` column on troves - for autoremove support
- v13: `collection_members` - package group/collection support
- v14: `flavor_spec` column on troves - Conary-style flavor specifications
- v15: `pinned` column on troves - package pinning support
- v16: `selection_reason` column on troves - for tracking why packages were installed
- v17: `triggers`, `trigger_dependencies`, `changeset_triggers` - trigger system
- v18: `system_states`, `state_members` - system state snapshots
- v19: `kind` column on provides and dependencies - typed dependency matching
- v20: `labels`, `label_path` tables, `label_id` on troves - package provenance tracking
- v21: `config_files`, `config_backups` tables - configuration file tracking and backup
- v22: security columns on `repository_packages` - security update tracking
- v23: `tx_uuid` column on changesets - transaction engine crash recovery correlation
- v24: `content_url` column on repositories - reference mirrors for split metadata/content
- v25: `converted_packages` table - track legacy→CCS conversions with fidelity
- v26: `derived_packages` table - packages derived from base packages via model-apply
- v27: `chunk_access` table - LRU cache tracking for Remi chunk store
- v28: `redirects` table - package redirect/rename/obsolete tracking
- v29: `package_resolution` table - per-package routing strategies (binary, remi, recipe, delegate)
- v30: `repository_id`, `delegate_to_label_id` columns on labels - label federation support
- v31: `default_strategy`, `default_strategy_endpoint`, `default_strategy_distro` columns on repositories - repo-level default resolution strategy
- v32: `provenance_sources`, `provenance_builds`, `provenance_signatures`, `provenance_content`, `provenance_verifications` - Package DNA / full provenance tracking
- v33: `capabilities`, `capability_audits` - package capability declarations (network, filesystem, syscalls)
- v34: `federation_peers`, `federation_stats` - CAS federation peers and daily stats
- v35: `daemon_jobs` - persistent job queue for conaryd daemon
- v36: enhancement columns on `converted_packages`, `subpackage_relationships` - retroactive CCS enhancement framework

## Testing

```bash
cargo test                    # All tests
cargo test --lib             # Library tests only
cargo test --test '*'        # Integration tests only
cargo test --test database   # Run specific test module
```

1150+ tests total (with --features daemon).

Integration tests are organized in `tests/`:
- `database.rs` - DB init, transactions (6 tests)
- `workflow.rs` - Install/remove/rollback (4 tests)
- `query.rs` - Queries, dependencies, provides (9 tests)
- `component.rs` - Component classification (7 tests)
- `features.rs` - Language deps, collections, state, config (9 tests)
- `conversion_integration.rs` - Legacy to CCS conversion (19 tests)
- `enhancement_integration.rs` - Retroactive enhancement (26 tests)
- `common/mod.rs` - Shared test helpers

## Hermetic Builds

Conary provides BuildStream-grade hermetic builds for reproducibility:

**Build Phases:**
1. **Fetch Phase** (network allowed): Download sources, verify checksums, cache locally
2. **Build Phase** (network blocked): Extract, patch, configure, make, install

**Container Isolation** (on by default):
- PID, UTS, IPC, mount, and network namespaces
- Network isolation via `CLONE_NEWNET` (only loopback available)
- No `/etc/resolv.conf` mount when network isolated

**CLI Flags:**
```bash
conary cook recipe.toml              # Default: isolated, network blocked during build
conary cook --fetch-only recipe.toml # Pre-fetch sources for offline build
conary cook --hermetic recipe.toml   # Maximum isolation (no host mounts)
conary cook --no-isolation recipe.toml # Unsafe: disable all isolation
```

**Cache Invalidation:**
- `DependencyHashes` tracks installed dependency content hashes
- Cache key changes when any dependency is updated (not just version bump)
- Use `cache_key_with_deps()` for BuildStream-grade reproducibility

## CAS Federation

Distributed chunk sharing across Conary nodes for bandwidth savings.

**Architecture:**
- **Region Hub**: WAN-connected central servers (mTLS required)
- **Cell Hub**: LAN segment coordinators
- **Leaf**: Individual client nodes

**Key Features:**
- Hierarchical peer selection (cell → region → upstream)
- mDNS discovery for LAN peers (`_conary-cas._tcp.local`)
- Request coalescing (dedupe concurrent identical requests)
- Circuit breaker pattern for failing peers
- Signed manifests (Ed25519) for chunk list integrity
- Per-tier allowlists for access control

**Server Security (Phase 4):**
- CORS restrictions for chunk/admin endpoints
- Token-bucket rate limiting per IP
- Audit logging for federation requests
- Configurable ban list for misbehaving IPs

**Observability (Phase 5):**
- Prometheus metrics export (`/v1/admin/metrics/prometheus`)
- Federation stats command (`conary federation stats`)
- Per-peer success rates and latency tracking

**CLI Commands:**
```bash
conary federation status              # Show federation overview
conary federation peers               # List configured peers
conary federation add-peer URL --tier cell_hub
conary federation test                # Test peer connectivity
conary federation scan                # mDNS discovery (server feature)
conary federation stats --days 7      # Show bandwidth savings
```

## conaryd Daemon

Local daemon providing REST API for package operations, acting as the "Guardian of State" with exclusive transaction lock ownership.

**Architecture:**
- Unix socket primary (`/run/conary/conaryd.sock`) with optional TCP
- SO_PEERCRED for peer authentication
- SQLite job persistence (survives daemon restart)
- SSE for real-time progress streaming
- Systemd socket activation and watchdog support

**Daemon Modules:**
| Module | Purpose |
|--------|---------|
| `mod.rs` | DaemonConfig, DaemonState, run_daemon() |
| `routes.rs` | Axum router with all REST endpoints |
| `jobs.rs` | DaemonJob model, OperationQueue with priority |
| `client.rs` | CLI forwarding client with SSE support |
| `socket.rs` | Unix socket + optional TCP listener |
| `lock.rs` | System-wide flock wrapper |
| `systemd.rs` | Socket activation, watchdog, idle timeout |
| `auth.rs` | Peer credentials, permission checking, audit logging |

**REST API:**
```
GET  /health                        # Health check
GET  /v1/version                    # API version info
GET  /v1/metrics                    # Prometheus format metrics
GET  /v1/packages                   # List installed packages
GET  /v1/packages/:name             # Package details
GET  /v1/packages/:name/files       # Package file list
GET  /v1/search?q=pattern           # Search packages
GET  /v1/depends/:name              # Dependencies
GET  /v1/rdepends/:name             # Reverse dependencies
GET  /v1/history                    # Transaction history
GET  /v1/transactions               # List jobs
GET  /v1/transactions/:id           # Job details
GET  /v1/transactions/:id/stream    # SSE progress stream
GET  /v1/events                     # Global SSE event stream
POST /v1/transactions               # Create transaction
POST /v1/packages/install           # Install packages
POST /v1/packages/remove            # Remove packages
POST /v1/packages/update            # Update packages
DELETE /v1/transactions/:id         # Cancel job
```

**CLI Forwarding:**
```rust
// CLI checks for daemon, forwards if available
if let Ok(client) = DaemonClient::connect() {
    client.install(&["nginx"], Default::default())?;
} else {
    // Fallback to direct execution
}
```

**Systemd Integration:**
```ini
# conaryd.socket
[Socket]
ListenStream=/run/conary/conaryd.sock
SocketMode=0660

# conaryd.service
[Service]
Type=notify
ExecStart=/usr/bin/conary daemon
WatchdogSec=60s
```
