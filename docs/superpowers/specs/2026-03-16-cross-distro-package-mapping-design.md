---
last_updated: 2026-03-16
revision: 3
summary: Remi-centric canonical mapping — server builds the map, clients consume it
---

# Cross-Distro Package Mapping — End-to-End

## Problem Statement

Conary supports Fedora, Arch, and Ubuntu simultaneously, but packages have
different names across distros (~40% of common packages). The canonical
mapping infrastructure exists (2,300+ lines, 5 discovery strategies, Repology
client, AppStream parser, YAML rules engine) but is completely disconnected
from the pipeline. None of it is called anywhere.

## Architectural Decision: Remi Holds the Map

**Remi is the canonical mapping authority**, not the client.

Why:
- Remi has ALL distro metadata already (it indexes Fedora, Arch, Ubuntu)
- Remi can see cross-distro equivalences that no single client can (a client
  configured with only Fedora repos has no data to discover Arch equivalents)
- Remi can run Repology/AppStream imports on a schedule without user action
- A single authoritative map means all clients get the same consistent view
- The map is computed once on the server, consumed many times by clients

**Data flow:**
```
Remi (server)                              Client
┌─────────────────────────────────┐        ┌──────────────────────┐
│ 1. Index all distro repos       │        │                      │
│ 2. Run auto-discovery:          │        │ conary repo sync     │
│    - name match across distros  │ ──────→│   fetches packages   │
│    - provides match             │        │   + canonical map    │
│    - stem match                 │        │                      │
│ 3. Import Repology (scheduled)  │        │ conary install curl  │
│ 4. Import AppStream (scheduled) │        │   looks up canonical │
│ 5. Apply curated rules          │        │   resolves per-distro│
│ 6. Serve canonical map via API  │        │                      │
└─────────────────────────────────┘        └──────────────────────┘
```

## Design

### 1. Remi canonical mapping pipeline (server-side)

**New scheduled job on Remi** that runs after repo indexing completes
(or on a configurable interval, default: after each mirror sync).

Steps:
1. Load all `repository_packages` across all enabled repos
2. Build `RepoPackageInfo` vec (name, distro, provides) — source provides
   from the already-parsed `RepositoryProvide` objects in the sync pipeline
   (`normalized_repository_capabilities()` at sync.rs:283), not by re-parsing
   metadata
3. Apply curated YAML rules (shipped with Remi in `data/canonical-rules/`)
4. Run auto-discovery on unmatched packages:
   - Name match: same name in 2+ distros → canonical
   - Provides match: shared virtual capability → canonical
   - Stem match: strip `-dev`/`-devel`/`-lib` suffixes → match stems
5. Optionally run Repology batch import (configurable, default: weekly)
6. Optionally run AppStream import per distro
7. Persist results to `canonical_packages` + `package_implementations`

**Location:** New module `conary-server/src/server/canonical_job.rs` or
add to existing job queue infrastructure.

**Trigger:** After mirror sync completes (Forgejo CI pushes, or the
10-minute GitHub mirror poll). Also available as a manual MCP tool:
`canonical_rebuild`.

### 2. Remi canonical map API

**New endpoints** on the Remi server (public, no auth needed):

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/v1/canonical/map` | Full canonical map (JSON, cached 5 min) |
| GET | `/v1/canonical/search?q=<term>` | Search canonical names |
| GET | `/v1/canonical/resolve?name=<pkg>&distro=<distro>` | Resolve one name |

**`/v1/canonical/map` response format:**

```json
{
  "version": 1,
  "generated_at": "2026-03-16T00:00:00Z",
  "entries": [
    {
      "canonical": "linux-kernel",
      "implementations": {
        "fedora": "kernel",
        "arch": "linux",
        "ubuntu": "linux-image-generic"
      }
    },
    {
      "canonical": "openssl",
      "implementations": {
        "fedora": "openssl",
        "arch": "openssl",
        "ubuntu": "libssl3"
      }
    }
  ]
}
```

Lightweight — just names and distro mappings. The client downloads this
once during `repo sync` and caches it locally.

### 3. Client: fetch canonical map during repo sync

**Location:** The repo sync flow in `conary-core/src/repository/sync.rs`.

After syncing package metadata from a Remi-backed repo, also fetch:
```
GET {remi_endpoint}/v1/canonical/map
```

Persist to the local DB's `canonical_packages` + `package_implementations`
tables (same schema, just populated from server data instead of local
discovery). Use `INSERT OR REPLACE` to keep the local map fresh.

For non-Remi repos (direct Fedora/Arch/Ubuntu mirrors), the canonical
map isn't available — the client falls back to exact name matching only.
This is acceptable because most users will have at least one Remi repo.

### 4. Fix rdepends to use normalized requirements

**Location:** `conary-server/src/server/handlers/detail.rs:391-424`

Replace the LIKE substring scan with an indexed join on the
`repository_requirements` table (column is `capability`, not `name`):

```sql
SELECT DISTINCT rp.name
FROM repository_packages rp
JOIN repository_requirements rr ON rr.repository_package_id = rp.id
WHERE rr.capability = ?1
  AND rp.repository_id = ?2
  AND rp.name != ?3
ORDER BY rp.name
```

**Confirmed:** `repository_requirements` IS populated during repo sync.
`sync.rs` calls `RepositoryRequirement::batch_insert()` at lines 257 and
272. No additional work needed here.

### 5. Canonical expansion in dependency resolution

**Location:** `conary-core/src/resolver/engine.rs` and
`src/commands/install/resolve.rs`.

When the resolver can't find a dependency by exact name:

```
1. Exact name lookup → found? done
2. NEW: Check local canonical map for this name
   → find implementations for the configured distros
   → retry exact lookup with each implementation name
   → use the first hit
3. Provides lookup (existing)
4. Fail with "not found" + suggestions
```

This applies to BOTH root requests AND transitive dependencies. The
canonical map is already in the local DB (synced from Remi), so this
is a fast local lookup — no network call in the hot path.

**Performance:** Pre-load the canonical index into a HashMap at resolver
construction time (same pattern as `provides_index` in
`ConaryProvider::load_removal_data()`). Do NOT do per-dependency DB
queries — that would be too slow for packages with 500+ transitive deps.

**Mixing policy for transitive canonical expansion:** When expanding a
dependency canonically, only consider implementations from the SAME
distro family as the parent package. If a Fedora package depends on
`libssl3` (a Debian name), canonical expansion finds `openssl` (Fedora)
and `libssl3` (Ubuntu). The resolver picks `openssl` because it matches
the active distro. This prevents mixed-distro dependency chains.
Concretely: filter canonical implementations by the repos the user has
configured + the distro of the package being installed.

### 6. "Did you mean?" on package not found

**Location:** `src/commands/install/mod.rs` (error handling).

When a package isn't found anywhere:

```
Error: Package 'ssl-library' not found.

Did you mean:
  openssl        (Fedora 43, Arch Linux)
  libssl3        (Ubuntu Noble)

Use 'conary canonical search ssl' for more options.
```

Search strategy:
1. Check canonical entries with substring/stem match
2. Check repository_packages with LIKE prefix match
3. Show top 5 results with distro provenance

### 7. Ship curated rules for critical packages

Create `data/canonical-rules/00-critical.yaml` shipped with Remi:

```yaml
# Kernel
- setname: linux-kernel
  name: kernel
  repo: fedora
- setname: linux-kernel
  name: linux
  repo: arch
- setname: linux-kernel
  name: linux-image-generic
  repo: ubuntu

# SSL
- setname: openssl
  name: openssl
  repo: [fedora, arch]
- setname: openssl
  name: libssl3
  repo: ubuntu

# Python
- setname: python
  name: python3
  repo: [fedora, ubuntu]
- setname: python
  name: python
  repo: arch

# zlib
- setname: zlib
  name: zlib
  repo: [fedora, arch]
- setname: zlib
  name: zlib1g
  repo: ubuntu

# Development headers (pattern-based)
- setname: $1-devel
  namepat: "^(.+)-devel$"
  repo: fedora
- setname: $1-devel
  namepat: "^lib(.+)-dev$"
  repo: ubuntu
```

Ship ~50-100 rules covering kernel, SSL, Python, zlib, development headers,
and other high-profile naming differences. Auto-discovery handles the rest.

**Note on `repo` field:** The existing `rules.rs` parser accepts
`repo: Option<String>` (single string). The `repo: [fedora, arch]` array
syntax above requires extending the parser to accept `StringOrVec`. This
is a minor serde change. Alternatively, use one rule per distro for
multi-distro mappings (more verbose but requires no parser change).

**First-sync edge case:** Auto-discovery strategies require packages from
2+ distros to create mappings (the `distros.len() >= 2` check). On Remi
this is not an issue — Remi indexes all 3 distros simultaneously. For
clients that only have one Remi repo, the canonical map comes from the
server (which sees all distros). Curated rules additionally apply to
single-distro setups since they match by repo name, not cross-distro
comparison.

### 8. MCP tools + CLI commands

**New remi-admin MCP tool:**
- `canonical_rebuild` — trigger canonical map rebuild on demand

**New conary CLI commands:**
- `conary canonical import repology [--project <name>]` — manual import
  (for bootstrapping or one-off lookups). Full import covers ~100K projects
  in batches of 200 at ~1 request/sec (Repology rate limit) = ~8-10 min.
- `conary canonical import appstream --distro <distro>` — manual import
- These are convenience commands; the primary flow is Remi-automatic

### 9. Remi packages site: use canonical for cross-distro links

On the package detail page (`/packages/fedora/kernel`), if the package
has canonical equivalents, show a cross-distro panel:

```
Also available as:
  linux (Arch Linux)
  linux-image-generic (Ubuntu Noble)
```

This uses the canonical map that Remi already has — just a UI addition.

## Implementation Order

1. **Fix rdepends query** — LIKE → indexed join (visible immediately)
2. **Remi canonical job** — build the map server-side after indexing
3. **Canonical map API** — serve the map to clients
4. **Ship curated rules** — 50-100 critical mappings
5. **Client sync** — fetch canonical map during repo sync
6. **Dependency resolution expansion** — canonical fallback in SAT solver
7. **"Did you mean?"** — suggestions on package not found
8. **MCP tool + CLI** — canonical_rebuild, import commands
9. **Packages site** — cross-distro links panel

## Success Criteria

- `conary install kernel` works on Arch (resolves to `linux` via canonical)
- `conary install openssl-devel` on Ubuntu resolves to `libssl-dev`
- Packages site shows correct reverse dependencies (no false positives)
- Packages site shows cross-distro equivalents
- Remi rebuilds canonical map automatically after mirror sync
- `conary canonical show kernel` shows implementations for all 3 distros
- `cargo test` passes
