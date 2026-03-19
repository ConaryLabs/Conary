# Canonical Registry Redesign: Server-Authoritative with Multi-Source Pipeline

## Problem

Clients currently populate canonical package mappings by reading local YAML rules
files (`data/canonical-rules/`). This fails in containers and test environments
where the rules files aren't bundled. Meanwhile, Remi already serves a canonical
map API (`GET /v1/canonical/map`) but no client fetches from it. Two built data
sources (Repology, AppStream) are fully implemented but never called.

## Goal

Make Remi the single source of truth for canonical mappings. Clients pull from
Remi. Remi builds its canonical map from all available data sources in priority
order: curated rules, Repology, AppStream, auto-discovery.

## Architecture Overview

Two independent server-side cycles feed the canonical map. Clients fetch the
finished map via HTTP with ETag caching.

```
Daily Background Job                    Post-Sync Rebuild
  Repology API ──┐                        repo sync ──┐
  AppStream URLs ─┤── cache in DB ──┐       (5 min cooldown)
                  │                 │                  │
                  └─── trigger ─────┼──────────────────┘
                                    │
                              Rebuild Pipeline
                         ┌─────────────────────┐
                         │ 1. Curated YAML rules│
                         │ 2. Repology cache    │
                         │ 3. AppStream cache   │
                         │ 4. Auto-discovery    │
                         └─────────┬───────────┘
                                   │
                           canonical_packages +
                         package_implementations
                                   │
                         GET /v1/canonical/map
                           (ETag cached)
                                   │
                    ┌──────────────┼──────────────┐
                    │              │              │
              repo sync       registry        MCP tool
             (automatic)    update (CLI)     (manual)
```

## Server-Side Design

### Daily Background Job (`canonical_fetch.rs`, new)

Started at server boot via `tokio::spawn`. Waits 60 seconds, then runs
immediately and repeats on a configurable interval (default 24 hours).

**Sequence:**

1. Fetch Repology data for configured distros (fedora, arch, ubuntu) via the
   existing `RepologyClient`. The client uses paginated batch fetch
   (`/api/v1/projects/{start}/`, 200 projects per page) with a mandatory
   1 req/sec rate limit (Repology blocks non-compliant clients). User-Agent
   must identify the client and provide a contact URL (currently
   `conary/0.1 (https://conary.io)` — update to current version). Fetch
   ~5000 projects (~25 requests, ~25 seconds). Write results to
   `repology_cache` table. Note: Repology recommends database dumps for
   bulk access, but the dumps are 10GB+ PostgreSQL SQL — the paginated API
   is appropriate for our ~5000-project scope.

2. Fetch AppStream catalogs from well-known distro URLs:
   - Fedora: parse `repomd.xml` from the release repo to locate the
     `<data type="appstream">` entry, then fetch the referenced
     `{hash}-appstream.xml.gz` file. URL pattern:
     `https://dl.fedoraproject.org/pub/fedora/linux/releases/{ver}/Everything/x86_64/os/repodata/repomd.xml`
   - Ubuntu: DEP-11 YAML catalog at
     `http://archive.ubuntu.com/ubuntu/dists/{codename}/main/dep11/Components-amd64.yml.gz`
   - Arch: no standard AppStream catalog (Arch does not ship one — skip)

   Decompress, parse with existing `parse_appstream_xml()` (Fedora) /
   `parse_appstream_yaml()` (Ubuntu DEP-11). Write results to
   `appstream_cache` table. Note: AppStream 1.0 moved local paths from
   `/usr/share/app-info/` to `/usr/share/swcatalog/` — this only affects
   local file reads, not our HTTP fetching.

3. Trigger a canonical rebuild.

On fetch failure (network, rate limit), log a warning and continue. The rebuild
runs with whatever cached data is available.

### Post-Sync Rebuild (`canonical_job.rs`, rewrite)

Triggered after successful repo sync on Remi, with a 5-minute cooldown. Also
triggered by the daily job and the MCP `canonical_rebuild` tool (which bypasses
cooldown).

**Debounce:** `server_metadata` table stores `last_canonical_rebuild` timestamp.
Skip if within cooldown window.

**4-Phase Pipeline (commit per phase, first match wins via `INSERT OR IGNORE`):**

Each phase commits independently. This avoids holding a single write lock for
the entire pipeline (which could take minutes on large package sets). The
"first match wins" invariant is enforced by `INSERT OR IGNORE` on the
`canonical_packages.name` unique constraint — later phases cannot overwrite
earlier ones.

**Phase 1 -- Curated Rules:**
Load YAML rules from the configured rules directory (default:
`/usr/share/conary/canonical-rules`, configurable in `[canonical]`). Upsert
`canonical_packages`, insert `package_implementations` with `source='curated'`.
These are authoritative and never overwritten by later phases.

**Phase 2 -- Repology:**
Read `repology_cache`. Group by project name (Repology's canonical identity).
For projects with 2+ distro implementations where the canonical name doesn't
already exist from Phase 1: insert with `source='repology'`. Only include
entries with `status` in (`newest`, `devel`, `unique`, `outdated`) — skip
`legacy`, `incorrect`, `untrusted`, `noscheme`, `rolling` entries. If the
Repology cache is empty (first run, or fetch failed), Phase 2 silently
produces no entries and logs a warning.

**Phase 3 -- AppStream:**
Read `appstream_cache`. For components matching an existing canonical entry
(from Phase 1 or 2): enrich by updating the `appstream_id` field (matched via
`pkgname` → `package_implementations.distro_name`). For new components not
yet mapped: insert with `source='appstream'`. This phase enriches rather than
competes — a package gets its cross-distro mapping from Repology and its
AppStream metadata from AppStream.

**Phase 4 -- Auto-Discovery:**
Query `repository_packages` for all enabled repos. Run existing discovery
strategies (`discovery.rs`) on packages not yet mapped: name match, provides
match, binary path match. Insert with `source='auto'`.

After rebuild: bump `canonical_map_version` counter in `server_metadata`
(via `INSERT OR REPLACE`), update ETag.

### ETag Support

`GET /v1/canonical/map` reads `canonical_map_version` from `server_metadata`
and uses it for both the response JSON `version` field (replacing the current
hardcoded `1`) and the ETag header (`W/"v{version}"`). Client sends
`If-None-Match`; server returns 304 if unchanged. Cache-Control header set to
300 seconds (existing behavior). The v53 migration seeds the initial value:
`INSERT INTO server_metadata VALUES ('canonical_map_version', '0')`.

### Configuration

New `[canonical]` section in `remi.toml`:

```toml
[canonical]
fetch_interval_hours = 24
rebuild_cooldown_minutes = 5
repology_batch_size = 5000
```

### MCP Tools

- `canonical_rebuild` (existing): triggers rebuild, bypasses cooldown
- `canonical_fetch` (new): triggers Repology + AppStream fetch cycle on demand

## Client-Side Design

### `repo sync` Integration

After metadata sync completes for any repo, the client makes a single
`GET /v1/canonical/map` request to the Remi endpoint (derived from the repo's
metadata URL). Sends `If-None-Match` with the locally stored ETag.

- **200:** Full JSON map received. Transaction: clear `canonical_packages` +
  `package_implementations`, insert new map, store new ETag. Full replace.
- **304:** Map is current, nothing to do.
- **Error:** Silently skip. Canonical sync is best-effort -- never blocks or
  fails the repo sync.

ETag stored in a `client_metadata` table (`key TEXT PRIMARY KEY, value TEXT`).

### `registry update` Command

Explicit command. Derives the Remi endpoint by iterating configured repos in
priority order, stripping the path from each repo's metadata URL, and appending
`/v1/canonical/map`. Tries each until a 200/304 is received. If all fail or no
repos are configured: falls back to local YAML rules at
`/usr/share/conary/canonical-rules` or `data/canonical-rules`.

Prints what it did: "Fetched N canonical mappings from packages.conary.io" or
"Loaded N curated rules from local files (server unreachable)".

## Database Changes (Schema v52 to v53)

All four tables are created on every database by the v53 migration (client and
server share the same migration runner). The server-only tables
(`repology_cache`, `appstream_cache`, `server_metadata`) are harmless on
clients; `client_metadata` is harmless on Remi. This avoids conditional
migration logic.

```sql
CREATE TABLE IF NOT EXISTS repology_cache (
    project_name TEXT NOT NULL,
    distro TEXT NOT NULL,
    distro_name TEXT NOT NULL,
    version TEXT,
    status TEXT,
    fetched_at TEXT NOT NULL,
    PRIMARY KEY (project_name, distro)
);

CREATE TABLE IF NOT EXISTS appstream_cache (
    appstream_id TEXT NOT NULL,
    pkgname TEXT NOT NULL,
    display_name TEXT,
    summary TEXT,
    distro TEXT NOT NULL,
    fetched_at TEXT NOT NULL,
    PRIMARY KEY (appstream_id, distro)
);

CREATE TABLE IF NOT EXISTS server_metadata (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- Seed initial version counter for ETag support
INSERT OR IGNORE INTO server_metadata (key, value)
    VALUES ('canonical_map_version', '0');

CREATE TABLE IF NOT EXISTS client_metadata (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
```

Existing `canonical_packages` and `package_implementations` tables are unchanged.

## Files Changed

### Server (conary-server)

| File | Change |
|------|--------|
| `server/canonical_job.rs` | Rewrite: 4-phase pipeline, debounce logic |
| `server/canonical_fetch.rs` | New: daily background job (Repology + AppStream fetch, scheduling) |
| `server/handlers/canonical.rs` | Add ETag support to `canonical_map()` |
| `server/mod.rs` | Register module, start background job at boot |
| `server/mcp.rs` | Add `canonical_fetch` MCP tool, update `canonical_rebuild` signature (rules dir from config) |

### Core (conary-core)

| File | Change |
|------|--------|
| `canonical/sync.rs` | Refactor to accept Repology + AppStream cache data |
| `canonical/repology.rs` | Add cache read/write methods |
| `canonical/appstream.rs` | Add fetch-from-URL, cache read/write |
| `canonical/client.rs` | New: HTTP fetch + ETag + local DB ingestion |
| `db/schema.rs` | Add cache tables, server_metadata, client_metadata (v53) |
| `db/models/` | New models for cache and metadata tables |

### Client (conary root)

| File | Change |
|------|--------|
| `commands/registry.rs` | Try Remi fetch first, fall back to local YAML |
| `commands/repo.rs` | After sync: fetch canonical map with ETag |

## Priority Order Summary

```
Curated YAML rules  >  Repology  >  AppStream  >  Auto-discovery
  (hand-verified)     (cross-distro)  (enriches)    (heuristic)
```

## Not In Scope

- Changing existing canonical API endpoints (they work as-is)
- Changing `canonical_packages` / `package_implementations` schema
- Client-side Repology or AppStream fetching (server handles all external APIs)
