# Cross-Distro Package Mapping Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire the canonical mapping pipeline end-to-end so Remi builds a cross-distro package map, clients consume it, and `conary install kernel` works on any distro.

**Architecture:** Remi is the canonical mapping authority. It runs auto-discovery + Repology + curated rules across all indexed distros, serves the map via a public API, and clients fetch it during repo sync. The SAT resolver uses the map as a fallback when exact name lookup fails. The rdepends query switches from LIKE scans to indexed joins.

**Tech Stack:** Rust 1.94, SQLite (rusqlite), Axum, reqwest, serde, resolvo.

**Spec:** `docs/superpowers/specs/2026-03-16-cross-distro-package-mapping-design.md` (rev 3)

---

## File Map

### Chunk 1: Server-side (rdepends fix + canonical job + API)

| Action | File | Responsibility |
|--------|------|----------------|
| Modify | `conary-server/src/server/handlers/detail.rs:391-424` | Fix rdepends query |
| Create | `conary-server/src/server/canonical_job.rs` | Scheduled canonical map builder |
| Modify | `conary-server/src/server/handlers/canonical.rs` | Add `/v1/canonical/map` endpoint |
| Modify | `conary-server/src/server/routes.rs:414-417` | Register new route |
| Modify | `conary-server/src/server/mod.rs` | Wire canonical_job module |

### Chunk 2: Curated rules + client sync + resolver

| Action | File | Responsibility |
|--------|------|----------------|
| Create | `data/canonical-rules/00-critical.yaml` | 50-100 critical package mappings |
| Modify | `conary-core/src/canonical/rules.rs:33` | Extend `repo` field to accept arrays |
| Modify | `conary-core/src/repository/sync.rs` | Fetch canonical map from Remi during sync |
| Modify | `conary-core/src/resolver/provider.rs` | Add canonical index to ConaryProvider |
| Modify | `conary-core/src/resolver/sat.rs` | Canonical fallback in dependency resolution |

### Chunk 3: UX (suggestions + MCP + packages site)

| Action | File | Responsibility |
|--------|------|----------------|
| Modify | `src/commands/install/mod.rs` | "Did you mean?" on package not found |
| Modify | `conary-server/src/server/mcp.rs` | canonical_rebuild MCP tool |
| Modify | `web/src/routes/packages/[distro]/[name]/+page.svelte` | Cross-distro links panel |

---

## Chunk 1: Server-Side Fixes

### Task 1: Fix rdepends to use indexed requirements

**Files:**
- Modify: `conary-server/src/server/handlers/detail.rs:391-424`

- [ ] **Step 1: Replace the LIKE query**

Find `query_reverse_dependencies()` at line 391. Replace the LIKE scan:

```rust
fn query_reverse_dependencies(
    db_path: &std::path::Path,
    distro: &str,
    name: &str,
) -> anyhow::Result<Vec<String>> {
    let conn = conary_core::db::open(db_path)?;

    let repo_id = match resolve_repo_id(&conn, distro)? {
        Some(id) => id,
        None => return Ok(Vec::new()),
    };

    // Use indexed join on repository_requirements instead of LIKE scan.
    // The `capability` column is the normalized dependency name.
    let mut stmt = conn.prepare(
        "SELECT DISTINCT rp.name
         FROM repository_packages rp
         JOIN repository_requirements rr ON rr.repository_package_id = rp.id
         WHERE rr.capability = ?1
           AND rp.repository_id = ?2
           AND rp.name != ?3
         ORDER BY rp.name",
    )?;

    let rows = stmt.query_map(rusqlite::params![name, repo_id, name], |row| {
        row.get::<_, String>(0)
    })?;

    Ok(rows.filter_map(|r| r.ok()).collect())
}
```

- [ ] **Step 2: Build and test**

Run: `cargo build --features server && cargo clippy --features server -- -D warnings`

- [ ] **Step 3: Commit**

```
fix(server): use indexed repository_requirements for rdepends query

Replaced LIKE '%name%' substring scan on dependencies text column
with an indexed join on repository_requirements.capability. Fixes
false positives (e.g., "kernel" matching "kernel-headers") and
improves performance from O(N) scan to O(log N) lookup.
```

---

### Task 2: Remi canonical map builder job

**Files:**
- Create: `conary-server/src/server/canonical_job.rs`
- Modify: `conary-server/src/server/mod.rs`

- [ ] **Step 1: Create the canonical job module**

Create `conary-server/src/server/canonical_job.rs`:

```rust
// conary-server/src/server/canonical_job.rs
//! Scheduled job that builds the canonical package mapping from all
//! indexed distros. Runs after mirror sync or on demand.

use anyhow::Result;
use conary_core::canonical::sync::{RepoPackageInfo, ingest_canonical_mappings};
use conary_core::canonical::rules::RulesEngine;
use conary_core::db;
use tracing::info;

/// Build the canonical map from all repository packages.
///
/// Loads packages from all enabled repos, applies curated rules,
/// runs auto-discovery (name match, provides match, stem match),
/// and persists to canonical_packages + package_implementations.
pub fn rebuild_canonical_map(db_path: &std::path::Path, rules_dir: Option<&std::path::Path>) -> Result<u64> {
    let conn = db::open(db_path)?;

    // Load curated rules if available
    let rules = rules_dir
        .and_then(|dir| {
            if dir.is_dir() {
                RulesEngine::load_from_dir(dir).ok()
            } else {
                None
            }
        });

    // Build RepoPackageInfo from all enabled repos
    let packages = build_repo_package_list(&conn)?;
    info!(
        "Canonical rebuild: {} packages across {} repos",
        packages.len(),
        packages.iter().map(|p| &p.distro).collect::<std::collections::HashSet<_>>().len()
    );

    // Run the canonical mapping pipeline
    let count = ingest_canonical_mappings(&conn, &packages, rules.as_ref())?;
    info!("Canonical rebuild complete: {} new mappings", count);

    Ok(count)
}

fn build_repo_package_list(conn: &rusqlite::Connection) -> Result<Vec<RepoPackageInfo>> {
    let mut stmt = conn.prepare(
        "SELECT rp.name, r.name as repo_name,
                COALESCE(r.default_strategy_distro, r.name) as distro
         FROM repository_packages rp
         JOIN repositories r ON rp.repository_id = r.id
         WHERE r.enabled = 1"
    )?;

    let rows = stmt.query_map([], |row| {
        Ok(RepoPackageInfo {
            name: row.get(0)?,
            distro: row.get::<_, String>(2)?,
            provides: Vec::new(), // TODO: join with repository_provide
            files: Vec::new(),    // Not available without unpacking
        })
    })?;

    Ok(rows.filter_map(|r| r.ok()).collect())
}
```

NOTE: The `provides` field should be populated from `repository_provide`
for better auto-discovery. Add a follow-up query:

```sql
SELECT rp2.provider_name
FROM repository_provide rp2
WHERE rp2.repository_package_id = ?1
```

Or batch-load all provides and join in memory. The implementer should
read `build_repo_package_list` and decide the most efficient approach.

- [ ] **Step 2: Register module**

In `conary-server/src/server/mod.rs`, add `pub mod canonical_job;`

- [ ] **Step 3: Build and test**

Run: `cargo build --features server`

- [ ] **Step 4: Commit**

```
feat(server): add canonical map builder job

rebuild_canonical_map() loads all repo packages, applies curated
rules, runs auto-discovery, and persists canonical mappings. Called
after mirror sync or on demand via MCP.
```

---

### Task 3: Canonical map API endpoint

**Files:**
- Modify: `conary-server/src/server/handlers/canonical.rs`
- Modify: `conary-server/src/server/routes.rs`

- [ ] **Step 1: Add the /v1/canonical/map endpoint**

In `canonical.rs`, add a handler that returns the full canonical map:

```rust
#[derive(Debug, Serialize)]
pub struct CanonicalMapEntry {
    pub canonical: String,
    pub implementations: HashMap<String, String>, // distro -> distro_name
}

#[derive(Debug, Serialize)]
pub struct CanonicalMapResponse {
    pub version: u32,
    pub generated_at: String,
    pub entries: Vec<CanonicalMapEntry>,
}

pub async fn canonical_map(
    State(state): State<Arc<RwLock<ServerState>>>,
) -> Result<Response, Response> {
    let db_path = state.read().await.config.db_path.clone();

    let map = run_blocking("canonical_map", move || {
        query_canonical_map(&db_path)
    }).await?;

    Ok((
        StatusCode::OK,
        [(header::CACHE_CONTROL, "public, max-age=300")],
        Json(map),
    ).into_response())
}

fn query_canonical_map(db_path: &std::path::Path) -> anyhow::Result<CanonicalMapResponse> {
    let conn = conary_core::db::open(db_path)?;

    let mut stmt = conn.prepare(
        "SELECT cp.name, pi.distro, pi.distro_name
         FROM canonical_packages cp
         JOIN package_implementations pi ON pi.canonical_id = cp.id
         ORDER BY cp.name, pi.distro"
    )?;

    let mut entries: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let canonical: String = row.get(0)?;
        let distro: String = row.get(1)?;
        let distro_name: String = row.get(2)?;
        entries.entry(canonical).or_default().insert(distro, distro_name);
    }

    Ok(CanonicalMapResponse {
        version: 1,
        generated_at: chrono::Utc::now().to_rfc3339(),
        entries: entries.into_iter().map(|(canonical, implementations)| {
            CanonicalMapEntry { canonical, implementations }
        }).collect(),
    })
}
```

- [ ] **Step 2: Register the route**

In `routes.rs`, add near line 414 with the other canonical routes:

```rust
.route("/v1/canonical/map", get(canonical::canonical_map))
```

- [ ] **Step 3: Build and test**

Run: `cargo build --features server`

- [ ] **Step 4: Commit**

```
feat(server): add /v1/canonical/map endpoint

Returns the full canonical package map as JSON. Cached for 5 minutes.
Clients fetch this during repo sync to populate their local
canonical tables.
```

---

## Chunk 2: Curated Rules + Client Sync + Resolver

### Task 4: Ship curated canonical rules

**Files:**
- Create: `data/canonical-rules/00-critical.yaml`
- Modify: `conary-core/src/canonical/rules.rs` (extend repo field)

- [ ] **Step 1: Extend Rule struct to accept repo arrays**

In `rules.rs`, change the `repo` field from `Option<String>` to accept
both strings and arrays. Use a custom deserializer or `StringOrVec`:

```rust
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum StringOrVec {
    Single(String),
    Multiple(Vec<String>),
}

// In the Rule struct, change:
pub repo: Option<StringOrVec>,
```

Update the `matches()` method to check if the distro matches any entry
in the vec. If `StringOrVec::Single(s)`, check `distro == s`. If
`StringOrVec::Multiple(v)`, check `v.contains(&distro)`.

- [ ] **Step 2: Create curated rules file**

Create `data/canonical-rules/00-critical.yaml` with mappings for the
most common cross-distro packages. Use one entry per distro for now
(avoid the array syntax until the parser extension is tested):

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

# SSL/TLS
- setname: openssl
  name: openssl
  repo: fedora
- setname: openssl
  name: openssl
  repo: arch
- setname: openssl
  name: libssl3
  repo: ubuntu

# Python
- setname: python
  name: python3
  repo: fedora
- setname: python
  name: python
  repo: arch
- setname: python
  name: python3
  repo: ubuntu

# zlib
- setname: zlib
  name: zlib
  repo: fedora
- setname: zlib
  name: zlib
  repo: arch
- setname: zlib
  name: zlib1g
  repo: ubuntu

# Development header patterns
- setname: $1-devel
  namepat: "^(.+)-devel$"
  repo: fedora
- setname: $1-devel
  namepat: "^lib(.+)-dev$"
  repo: ubuntu
```

Add ~50 more entries covering: curl, wget, git, nginx, httpd/apache,
postgresql, sqlite, ncurses, readline, glibc/libc6, systemd, dbus,
util-linux, coreutils, bash, grep, sed, gawk, findutils, tar, gzip,
xz, bzip2, file, less, man-db, sudo, shadow, pam.

- [ ] **Step 3: Build and test**

Run: `cargo test -p conary-core canonical::rules -- --nocapture`

- [ ] **Step 4: Commit**

```
feat(canonical): ship curated rules for 50+ critical cross-distro packages

Kernel, SSL, Python, zlib, development headers, and common tools.
Extended rules.rs to accept repo arrays (StringOrVec). Rules are
loaded by Remi's canonical job during map rebuild.
```

---

### Task 5: Client fetches canonical map during repo sync

**Files:**
- Modify: `conary-core/src/repository/sync.rs`

- [ ] **Step 1: Add canonical map fetch after sync**

After the package metadata sync completes (after the batch inserts),
check if the repo has a Remi endpoint configured. If so, fetch the
canonical map:

```rust
// After package sync completes, fetch canonical map from Remi
if let Some(ref remi_endpoint) = repo.default_strategy_endpoint {
    match fetch_canonical_map(remi_endpoint) {
        Ok(map) => {
            let count = persist_canonical_map(&tx, &map)?;
            info!("Synced {} canonical mappings from Remi", count);
        }
        Err(e) => {
            // Non-fatal: canonical map is a nice-to-have
            debug!("Failed to fetch canonical map: {}", e);
        }
    }
}
```

The `fetch_canonical_map()` function does a blocking HTTP GET to
`{remi_endpoint}/v1/canonical/map` and parses the JSON.

`persist_canonical_map()` iterates the entries and does
`INSERT OR REPLACE` into `canonical_packages` and
`package_implementations`.

- [ ] **Step 2: Build and test**

Run: `cargo build -p conary-core`

- [ ] **Step 3: Commit**

```
feat(sync): fetch canonical map from Remi during repo sync

After syncing package metadata, clients fetch the canonical map
from the Remi endpoint and persist it locally. Non-fatal if Remi
is unreachable. Enables cross-distro name resolution on the client.
```

---

### Task 6: Canonical fallback in dependency resolution

**Files:**
- Modify: `conary-core/src/resolver/provider.rs`
- Modify: `conary-core/src/resolver/sat.rs` (or `engine.rs`)

- [ ] **Step 1: Add canonical index to ConaryProvider**

In `provider.rs`, add a field to ConaryProvider:

```rust
/// Canonical name -> Vec<(distro, distro_name)> for cross-distro resolution
canonical_index: HashMap<String, Vec<(String, String)>>,
```

Populate it in a new `load_canonical_index()` method:

```rust
pub fn load_canonical_index(&mut self) -> Result<()> {
    let mut stmt = self.conn.prepare(
        "SELECT pi.distro_name, pi.distro, cp.name
         FROM package_implementations pi
         JOIN canonical_packages cp ON cp.id = pi.canonical_id"
    )?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let distro_name: String = row.get(0)?;
        let distro: String = row.get(1)?;
        let canonical: String = row.get(2)?;

        // Index by distro-specific name -> canonical equivalents
        self.canonical_index
            .entry(distro_name)
            .or_default()
            .push((distro, canonical));
    }
    Ok(())
}

/// Find canonical equivalents for a package name.
/// Returns (distro, distro_name) pairs for packages that are
/// canonically equivalent to the given name.
pub fn find_canonical_equivalents(&self, name: &str) -> Vec<(String, String)> {
    // Step 1: Is this name a distro-specific name with a canonical entry?
    if let Some(entries) = self.canonical_index.get(name) {
        // Find the canonical name
        if let Some((_, canonical)) = entries.first() {
            // Find ALL implementations of this canonical name
            let mut result = Vec::new();
            for (dn, impls) in &self.canonical_index {
                for (distro, cn) in impls {
                    if cn == canonical && dn != name {
                        result.push((distro.clone(), dn.clone()));
                    }
                }
            }
            return result;
        }
    }
    Vec::new()
}
```

- [ ] **Step 2: Use canonical fallback in solve_install**

In the dependency resolution path (where a dependency name is looked up
in the solvable list), add a fallback:

```rust
// After exact name lookup fails:
if candidates.is_empty() {
    let equivalents = provider.find_canonical_equivalents(dep_name);
    // Filter to distros the user has configured
    for (distro, equiv_name) in equivalents {
        let equiv_candidates = lookup_by_name(&equiv_name);
        if !equiv_candidates.is_empty() {
            debug!("Canonical fallback: {} -> {} ({})", dep_name, equiv_name, distro);
            candidates = equiv_candidates;
            break;
        }
    }
}
```

- [ ] **Step 3: Call load_canonical_index in solve_install**

After `load_installed_packages()`, add:

```rust
provider.load_canonical_index()?;
```

- [ ] **Step 4: Build and test**

Run: `cargo build -p conary-core && cargo test -p conary-core resolver`

- [ ] **Step 5: Commit**

```
feat(resolver): canonical fallback for cross-distro dependency resolution

When a dependency name isn't found by exact lookup, the resolver
checks the canonical index for cross-distro equivalents. Filters
to configured distros to prevent mixed-distro chains. Pre-loads
the canonical index as a HashMap for O(1) lookup performance.
```

---

## Chunk 3: UX + MCP + Packages Site

### Task 7: "Did you mean?" suggestions

**Files:**
- Modify: `src/commands/install/mod.rs`

- [ ] **Step 1: Add suggestion search on package not found**

Find where the install command reports "Package not found" (search for
the error message). Before returning the error, search for alternatives:

```rust
// When package resolution fails with "not found":
let suggestions = find_package_suggestions(&conn, &package_name)?;
if !suggestions.is_empty() {
    eprintln!("Error: Package '{}' not found.\n", package_name);
    eprintln!("Did you mean:");
    for (name, distro) in suggestions.iter().take(5) {
        eprintln!("  {:<20} ({})", name, distro);
    }
    eprintln!("\nUse 'conary canonical search {}' for more options.",
        package_name.split('-').next().unwrap_or(&package_name));
    std::process::exit(1);
}
```

The `find_package_suggestions()` function:
1. Search `canonical_packages` by substring match
2. Search `repository_packages` by prefix match
3. Combine, deduplicate, return top 5 with distro info

- [ ] **Step 2: Build and test**

Run: `cargo build`

- [ ] **Step 3: Commit**

```
feat(cli): show 'did you mean?' suggestions when package not found

Searches canonical names and repository packages for alternatives.
Shows top 5 matches with distro provenance.
```

---

### Task 8: canonical_rebuild MCP tool

**Files:**
- Modify: `conary-server/src/server/mcp.rs`

- [ ] **Step 1: Add MCP tool**

Add to the `#[tool_router]` impl:

```rust
#[tool(description = "Rebuild the canonical package mapping from all indexed distros. Runs auto-discovery, applies curated rules, and updates the canonical map served to clients.")]
async fn canonical_rebuild(&self) -> Result<CallToolResult, McpError> {
    let state = self.state.read().await;
    let db_path = state.config.db_path.clone();
    let rules_dir = std::path::PathBuf::from("data/canonical-rules");

    drop(state); // Release lock before blocking

    let count = tokio::task::spawn_blocking(move || {
        crate::server::canonical_job::rebuild_canonical_map(
            &db_path,
            Some(&rules_dir),
        )
    })
    .await
    .map_err(|e| McpError::internal(e.to_string()))?
    .map_err(service_err_to_mcp)?;

    to_json_text(&serde_json::json!({
        "status": "ok",
        "new_mappings": count,
    }))
}
```

- [ ] **Step 2: Update tool count test**

- [ ] **Step 3: Commit**

```
feat(server): add canonical_rebuild MCP tool

Triggers a full rebuild of the canonical package map on demand.
Returns count of new mappings created.
```

---

### Task 9: Cross-distro links on packages site

**Files:**
- Modify: `web/src/routes/packages/[distro]/[name]/+page.svelte`
- Modify: `web/src/lib/api.ts`

- [ ] **Step 1: Add canonical lookup API call**

In `api.ts`, add:

```typescript
export async function getCanonicalInfo(name: string) {
    return fetchJson(`/v1/canonical/${encodeURIComponent(name)}`);
}
```

- [ ] **Step 2: Add cross-distro panel to package detail page**

In the package detail page, after loading package info, also fetch
canonical info. If the package has cross-distro equivalents, show:

```svelte
{#if canonicalInfo && canonicalInfo.implementations.length > 1}
    <div class="cross-distro">
        <h3>Also available as</h3>
        {#each canonicalInfo.implementations.filter(i => i.distro !== distro) as impl}
            <a href="/packages/{impl.distro}/{impl.distro_name}">
                {impl.distro_name} ({impl.distro})
            </a>
        {/each}
    </div>
{/if}
```

- [ ] **Step 3: Commit**

```
feat(web): show cross-distro equivalents on package detail page

When viewing a package like /packages/fedora/kernel, shows links
to equivalent packages on other distros (linux on Arch,
linux-image-generic on Ubuntu).
```

---

## Final Verification

- [ ] `cargo test && cargo test --features server`
- [ ] `cargo clippy -- -D warnings && cargo clippy --features server -- -D warnings`
- [ ] Deploy to Remi, trigger `canonical_rebuild` via MCP
- [ ] Verify `/v1/canonical/map` returns populated map
- [ ] Verify `/packages/fedora/kernel` rdepends is accurate (no false positives)
- [ ] Verify `/packages/fedora/kernel` shows Arch/Ubuntu equivalents
- [ ] Test `conary install kernel` on Arch-configured client resolves to `linux`
