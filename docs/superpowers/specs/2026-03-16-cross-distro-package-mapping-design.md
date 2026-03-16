---
last_updated: 2026-03-16
revision: 1
summary: Wire the canonical mapping pipeline end-to-end — sync integration, dependency resolution, rdepends fix, Repology import, UX hints
---

# Cross-Distro Package Mapping — End-to-End

## Problem Statement

Conary supports Fedora, Arch, and Ubuntu simultaneously, but packages have
different names across distros (~40% of common packages). The canonical
mapping infrastructure exists (2,300+ lines, 5 discovery strategies, Repology
client, AppStream parser, YAML rules engine) but is completely disconnected
from the actual pipeline:

1. `repo sync` never calls `ingest_canonical_mappings()` — auto-discovery
   never runs
2. The SAT dependency resolver ignores canonical names — cross-distro deps
   fail with "not found"
3. The Remi packages site uses `LIKE '%name%'` for reverse dependencies —
   false positives everywhere
4. When a package isn't found, there's no suggestion of canonical alternatives
5. Repology and AppStream clients exist but have no CLI entry points

## Design

### 1. Wire canonical sync into repo sync

**Location:** `conary-core/src/repository/sync.rs` (or wherever repo sync
persists packages).

After the repository metadata is parsed and persisted, call:

```rust
canonical::sync::ingest_canonical_mappings(&tx, &repo_packages, rules_engine.as_ref())?;
```

This triggers:
- Phase 1: Apply curated YAML rules (from `/usr/share/conary/canonical-rules/`
  or `~/.config/conary/canonical-rules/`)
- Phase 2: Run auto-discovery on unmatched packages (name match, provides
  match, stem match)

**Data flow:**
```
repo sync fedora-43
  → parse 50K packages from RPM metadata
  → persist to repository_packages table
  → NEW: build RepoPackageInfo vec from parsed packages
  → NEW: call ingest_canonical_mappings()
  → canonical_packages + package_implementations populated
```

The `RepoPackageInfo` struct already exists in `canonical/sync.rs`. It needs:
`name`, `distro`, `provides` (from RPM/DEB provides), `files` (empty for
now — file lists require unpacking, which we defer).

**When multiple distros are synced**, auto-discovery fires on each sync.
The name-match strategy finds packages with the same name across distros
and creates canonical entries. After syncing both Fedora and Arch, packages
like `curl`, `gcc`, `nginx` (same name) are automatically mapped. Packages
like `kernel` (Fedora) vs `linux` (Arch) need either Repology data or
curated rules.

### 2. Fix rdepends to use normalized provides

**Location:** `conary-server/src/server/handlers/detail.rs:391-424`

Replace the LIKE substring scan:
```sql
WHERE rp.dependencies LIKE '%kernel%'
```

With an indexed join on `repository_requirements`:
```sql
SELECT DISTINCT rp.name
FROM repository_packages rp
JOIN repository_requirements rr ON rr.repository_package_id = rp.id
WHERE rr.name = ?1
  AND rp.repository_id = ?2
  AND rp.name != ?3
ORDER BY rp.name
```

The `repository_requirements` table already exists (added in schema v51)
with individual rows per dependency, indexed by name. This gives exact
matching and O(log N) performance.

**Also check:** Does the `repository_requirements` table get populated
during repo sync? If not, that's another gap to fix — the sync must
populate both `repository_packages.dependencies` (JSON blob for display)
AND `repository_requirements` (normalized rows for querying).

### 3. Canonical expansion in dependency resolution

**Location:** `conary-core/src/resolver/engine.rs` (SAT solver) and
`src/commands/install/resolve.rs` (package resolution).

Currently when the resolver needs package `libssl3` and can't find it in
the active repos, it fails. The fix:

```
For each unresolved dependency D:
  1. Exact name lookup in active repos → found? done
  2. NEW: Canonical lookup: find canonical entry for D
     → get all implementations across distros
     → filter to repos the user has configured
     → if exactly one match: use it
     → if multiple: pick by affinity (same distro preference)
  3. Provides lookup: check if any package provides D
  4. Fail with "not found" + suggestions
```

This applies to BOTH root requests (user-typed names) AND transitive
dependencies. The current code only does canonical expansion for root
requests.

**Important:** This must not slow down the common case. The canonical
lookup only fires when the exact name lookup fails — it's a fallback
path. Cache the canonical index in the resolver's `ConaryProvider` (same
pattern as the provides index we added for `solve_removal`).

### 4. "Did you mean?" suggestions for package not found

**Location:** `src/commands/install/mod.rs` (install error handling).

When a package is not found anywhere (repos, canonical, provides), instead
of a bare "Package not found" error, search for alternatives:

```
Error: Package 'ssl-library' not found.

Did you mean:
  openssl        (Fedora 43)
  libssl3        (Ubuntu Noble)
  openssl        (Arch Linux)

Use 'conary canonical search ssl' for more options.
```

Implementation: query `canonical_packages` and `repository_packages` with
a fuzzy/stem match on the failed name. Show top 5 results with their
distro source.

### 5. Repology and AppStream import commands

**Location:** `src/commands/canonical.rs` (add subcommands).

Two new CLI commands:

```bash
conary canonical import repology [--project <name>] [--batch <prefix>]
conary canonical import appstream [--distro <distro>]
```

**Repology import:**
- Single project: `conary canonical import repology --project curl`
  → calls `RepologyClient::fetch_project("curl")`
  → maps to canonical entry with implementations per distro
- Batch: `conary canonical import repology --batch a`
  → calls `RepologyClient::fetch_projects_batch("a")`
  → processes all projects starting with 'a'
- Full: `conary canonical import repology`
  → iterates all projects (rate-limited, may take hours)

**AppStream import:**
- `conary canonical import appstream --distro fedora`
  → downloads AppStream catalog from repo metadata
  → parses XML/YAML
  → calls `appstream::ingest_appstream()`

Both clients are fully implemented — this is just wiring them into CLI
commands and calling the existing functions.

### 6. Ship curated canonical rules for critical packages

Create `data/canonical-rules/00-kernel.yaml`:

```yaml
- setname: linux-kernel
  name: kernel
  repo: fedora
- setname: linux-kernel
  name: linux
  repo: arch
- setname: linux-kernel
  name: linux-image-generic
  repo: ubuntu

- setname: openssl
  name: openssl
  repo: [fedora, arch]
- setname: openssl
  name: libssl3
  repo: ubuntu

- setname: python
  name: python3
  repo: [fedora, ubuntu]
- setname: python
  name: python
  repo: arch

- setname: zlib
  name: zlib
  repo: [fedora, arch]
- setname: zlib
  name: zlib1g
  repo: ubuntu
```

Ship ~50-100 critical mappings covering kernel, SSL, Python, development
headers, common libraries, and popular tools. The auto-discovery handles
the rest (packages with identical names across distros).

### 7. Additional gaps found during audit

**7a. `repository_requirements` population during sync**

Verify that `repo sync` populates the `repository_requirements` table
with individual rows per dependency. If not, add it — this table is
required for the rdepends fix AND for dependency resolution canonical
expansion.

**7b. `canonical_id` index on `repository_packages`**

Consider adding `canonical_id INTEGER REFERENCES canonical_packages(id)`
to `repository_packages` to enable fast joins between repo packages and
their canonical equivalents. Currently requires going through
`package_implementations` which is an extra hop.

**7c. Provides normalization during sync**

The `repository_provide` table exists but may not be populated for all
distro formats. Verify that RPM provides, DEB provides, and Arch provides
are all extracted and normalized during sync. This is needed for both
rdepends and cross-distro dependency resolution.

## Implementation Order

1. **Fix rdepends query** — switch from LIKE to repository_requirements join
   (quick win, visible on packages site immediately)
2. **Wire canonical sync into repo sync** — call ingest_canonical_mappings()
   after package persistence
3. **Ship curated rules** — 50-100 critical package mappings in YAML
4. **"Did you mean?" suggestions** — fuzzy search on install failure
5. **Canonical expansion in dependency resolution** — fallback path in SAT
   resolver
6. **Repology/AppStream CLI commands** — wire existing clients
7. **Verify/fix repository_requirements population**

## Success Criteria

- `conary install kernel` works on Arch (resolves to `linux` via canonical)
- `conary install openssl-devel` works on Ubuntu (resolves to `libssl-dev`)
- The packages site shows correct reverse dependencies (no false positives)
- `cargo test` passes
- Repo sync populates canonical mappings automatically
- `conary canonical show kernel` shows implementations for all 3 distros
