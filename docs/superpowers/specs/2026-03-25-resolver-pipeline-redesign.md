# Resolver Pipeline Redesign: Enriched Package Identity

## Problem

The resolver pipeline loses package identity at every subsystem boundary. Rich data from AppStream, Repology, and repository sync (repository_id, architecture, version_scheme, distro, canonical mapping) is progressively flattened into bare strings by the time the SAT solver sees it. This causes:

- **Cross-distro resolution failures**: Debian `amd64` packages filtered out on `x86_64` hosts because architecture aliases aren't carried through. Canonical equivalents only tried when exact-name candidates are empty pre-filter, missing cases where version constraints eliminate exact matches.
- **Policy enforcement gaps**: `RequestScope::Repository` can't actually scope to a repository because candidates don't carry repository identity. Mixing policies compare inferred strings, not real fields.
- **Version scheme mismatches**: `VersionScheme` is inferred from `Repository.default_strategy_distro` instead of read from the explicit `repository_packages.version_scheme` column. Debian provide-version checks evaluated with RPM comparator.
- **Multi-arch collisions**: Graph resolver keyed by plain name; SAT provider's `ConaryPackage` has no architecture field. Multilib installs overwrite each other or pick wrong candidates.

### Root cause

There is no unified type that carries package identity through the resolution pipeline. Each subsystem reinvents identity from whatever fields it has, losing context at every boundary.

Additionally, the canonical system (AppStream, Repology) provides rich cross-distro data that is underused:

- **AppStream `origin`** identifies the exact repository source (e.g., "debian-unstable-main") but we don't link it to our repos table.
- **AppStream `<provides>`** carries cross-distro capability data (sonames, binaries, python3 modules) but we only use name mappings.
- **Repology rules** are the gold standard for cross-distro name equivalence but we only query the rate-limited API instead of ingesting the curated ruleset.
- **libsolv's Solvable identity** is `(name, evr, arch, repo)` -- the proven model we should match.

### Data loss junctures

| Juncture | What's lost | Consequence |
|----------|-------------|-------------|
| Canonical -> Resolver | repo_id, version_scheme, arch, version | ResolverCandidate is three strings |
| PackageSelector -> ConaryPackage | architecture, distro, repository_id | SAT candidates are name + version only |
| Version scheme inference | Explicit package-level scheme | Wrong comparator for Debian/Arch |
| Graph node insertion | Architecture (keyed by name only) | Multi-arch overwrites |
| Policy enforcement | Repository identity | String matching instead of real fields |
| AppStream ingestion | origin, provides (libs/bins/py3) | Cross-distro capabilities unused |
| Repology ingestion | Rule-based equivalences | API-only, rate-limited, incomplete |

## Research findings

### AppStream (freedesktop.org, spec 1.0)

AppStream catalog metadata carries more than we use:

- **`origin` attribute**: Identifies the repository source per catalog file (e.g., "debian-unstable-main", "fedora-41-updates"). We should capture this during ingestion and link to our `repositories` table.
- **`<provides>` section**: Cross-distro by definition. Supported types:
  - `<library/>` -- shared library sonames (e.g., `libappstream.so.1`)
  - `<binary/>` -- executables in PATH
  - `<python3/>` -- Python 3 modules
  - `<dbus/>` -- D-Bus service names (type: system/user)
  - `<firmware/>` -- firmware components
  - `<mediatype/>` -- MIME types
  - `<modalias/>` -- hardware device patterns
  - `<font/>` -- font names
  - `<id/>` -- other component IDs
- **`<pkgname>`**: Per-distro by design (each catalog comes from a specific origin). Multiple `<pkgname>` tags are supported but discouraged in favor of metapackages.
- **DEP-11 YAML**: Debian's equivalent format carries identical semantics. Both XML and YAML are fully supported by AppStream libraries.

**Implication for our design**: Ingest AppStream `origin` to establish repo links. Ingest `<provides>` as cross-distro capability data that feeds into the provides index, not just the RPM/DEB metadata parsers.

### Repology

- **API**: Rate-limited (1 req/sec), bulk use explicitly discouraged. Returns `repo`, `version`, `srcname`, `binname`, `binnames`, `visiblename`, `origversion`, `status`, `summary`, `categories`, `licenses`, `maintainers`. Does NOT provide architecture or version scheme.
- **Database dumps**: PostgreSQL SQL compressed with zstd at `dumps.repology.org`. Intended for bulk consumers. Requires postgresql-libversion and postgresql-trgm extensions. This is the right path for us.
- **Rules repository** (`github.com/repology/repology-rules`): YAML rulesets organized by function:
  - `100.prefix-suffix/` -- strip repo-specific prefixes/suffixes
  - `800.renames-and-merges/` -- cross-distro name mapping (e.g., `{name: httpd, setname: apache}`)
  - `850.split-ambiguities/` -- split same-name different-project packages
  - `900.version-fixes/` -- normalize version schemes
  - Rules match on: name, namepat, version, vergt/verge, repo/ruleset, homepage (wwwpart/wwwpat), category, maintainer
  - Actions: `setname`, `setver`, `devel`, `ignore`, `incorrect`, `altver`, `altscheme`

**Implication for our design**: Ingest Repology rules YAML directly for name mapping instead of (or in addition to) the API. The rules are the canonical source of truth and are freely available. Consider periodic Repology dump ingestion for version tracking.

### libsolv (openSUSE, the gold standard)

libsolv's Pool architecture validates our design direction:

- **Solvable identity**: `(name, evr, arch, repo)` -- exactly our `PackageIdentity`. Every solvable carries its full provenance.
- **Architecture handling**: `pool_setdisttype()` sets package comparison semantics (RPM vs Debian). `pool_setarch()` sets installable architectures. Multilib uses "colors" (32-bit vs 64-bit) as a pool-level concept, not per-query logic.
- **Provides index**: `pool_createwhatprovides()` builds an indexed map from dependency IDs to provider sets. Zero-terminated arrays in `whatprovidesdata`. O(1) lookup after construction. Our `ProvideEntry` queries are the naive version.
- **Vendor matching**: `pool_addvendorclass()` defines vendor equivalence groups. Only replaces packages within the same vendor class. Our mixing policy is the equivalent concept.
- **Repository management**: Pool maintains `Repo **repos` array. Each Repo is identified separately. An explicit `installed` pointer designates the system state.

**Implication for our design**: Model `PackageIdentity` after libsolv's Solvable. Build a provides index (like `whatprovides`) at resolution start instead of per-dep queries. Treat architecture as a pool-level policy, not per-candidate filtering.

### resolvo (prefix-dev, our SAT solver, v0.10.2)

resolvo's `DependencyProvider` trait is well-suited:

- **`get_candidates(name)`**: Returns all candidates for a name. This is where canonical equivalents should be injected.
- **`filter_candidates(version_set, solvables, inverse)`**: Version filtering happens here, inside resolvo. We cannot intercept post-filter, confirming that canonical equivalents must be in the candidate pool pre-filter.
- **`sort_candidates(solvables)`**: Ranking hook. Exact-name matches rank above canonical fallbacks here.
- **`get_dependencies(solvable)`**: Per-solvable dependency loading. With `PackageIdentity`, the version scheme is on the identity, not inferred.
- **Async support**: `DependencyProvider` methods are async, enabling concurrent metadata fetching.
- **Conditional requirements**: Supported via `ConditionalRequirement` type.

**Implication for our design**: Always include canonical equivalents in `get_candidates` (confirmed by API design -- filtering is internal to resolvo). Use `sort_candidates` for ranking. Use async loading if we need lazy candidate fetching.

## Design

### Approach: Enriched identity on repository_packages

Add `canonical_id` to the `repository_packages` table (set during sync). Create a `PackageIdentity` struct in Rust loaded from a single join query. Replace `ConaryPackage` and `ResolverCandidate` with `PackageIdentity`. Delete the graph resolver. Policy enforcement operates on real identity fields. Build a provides index at resolution start. Ingest AppStream provides and Repology rules for richer cross-distro data.

### 1. Data model

**DB change (migration v59):** Add `canonical_id INTEGER REFERENCES canonical_packages(id) ON DELETE SET NULL` to `repository_packages` with index. Backfill from existing `package_implementations` data.

**Rust type (`conary-core/src/resolver/identity.rs`):**

```rust
/// Full package identity for resolution, modeled after libsolv's Solvable.
/// Loaded from a single join across repository_packages + repositories + canonical_packages.
pub struct PackageIdentity {
    // From repository_packages -- the Solvable core
    pub repo_package_id: i64,
    pub name: String,
    pub version: String,
    pub architecture: Option<String>,
    pub version_scheme: VersionScheme,     // Required, not Optional

    // From repositories (via join) -- the Repo context
    pub repository_id: i64,
    pub repository_name: String,
    pub repository_distro: Option<String>, // default_strategy_distro
    pub repository_priority: i32,

    // From canonical (via canonical_id join, nullable) -- cross-distro identity
    pub canonical_id: Option<i64>,
    pub canonical_name: Option<String>,

    // Installed state (set when this identity matches an installed trove)
    pub installed_trove_id: Option<i64>,
}
```

**Loading query:**

```sql
SELECT rp.id, rp.name, rp.version, rp.architecture, rp.version_scheme,
       rp.repository_id, r.name, r.default_strategy_distro, r.priority,
       rp.canonical_id, cp.name
FROM repository_packages rp
JOIN repositories r ON rp.repository_id = r.id
LEFT JOIN canonical_packages cp ON rp.canonical_id = cp.id
WHERE rp.name = ?1
```

`version_scheme` defaults to the repo's inferred scheme only when the column is NULL. This is the fallback exception, not the rule.

### 2. Provides index

Modeled after libsolv's `pool_createwhatprovides()`. Built once at the start of resolution, not queried per-dependency.

**`ProvidesIndex` (`conary-core/src/resolver/provides_index.rs`):**

```rust
/// Pre-built index mapping capability names to provider PackageIdentity IDs.
/// Built once at resolution start from repository_provides + installed provides.
pub struct ProvidesIndex {
    /// capability_name -> Vec<(repo_package_id, provide_version, version_scheme)>
    providers: HashMap<String, Vec<ProviderEntry>>,
}

pub struct ProviderEntry {
    pub repo_package_id: i64,
    pub provide_version: Option<String>,
    pub version_scheme: VersionScheme,
    pub installed_trove_id: Option<i64>,
}

impl ProvidesIndex {
    /// Build the index from all repository_provides + installed provides.
    pub fn build(conn: &Connection) -> Result<Self> { ... }

    /// Find all providers for a capability, optionally filtered by version constraint.
    pub fn find_providers(&self, capability: &str, constraint: Option<&RepoVersionConstraint>, scheme: VersionScheme) -> Vec<&ProviderEntry> { ... }
}
```

**Data sources for the index:**
1. `repository_provides` table (from repo sync) -- per-distro provides
2. `provides` table (installed packages) -- local provides
3. AppStream `<provides>` data (libraries, binaries, python3 modules) -- cross-distro provides

### 3. Sync populates canonical links

During `sync_repository()`, after inserting each `repository_package`, look up its canonical identity:

```rust
let canonical_id = conn.query_row(
    "SELECT pi.canonical_id FROM package_implementations pi
     WHERE pi.distro_name = ?1 AND pi.distro = ?2",
    params![pkg.name, repo_distro],
    |row| row.get(0),
).optional()?;
```

If found, set `repository_packages.canonical_id`. If not (no canonical data yet, unknown package), leave NULL.

**Re-link:** `conary canonical rebuild` re-links all existing `repository_packages.canonical_id` after refreshing AppStream/Repology data. Covers the "canonical data arrived after sync" case.

### 4. AppStream enrichment

Enhance AppStream ingestion (`conary-core/src/canonical/appstream.rs`) to capture data we currently discard:

**Origin linking:** When parsing AppStream catalog XML/YAML, capture the `origin` attribute and match it to a repository in our `repositories` table. Store the link on each `package_implementations` row so canonical mappings know which repo they came from.

**Cross-distro provides ingestion:** Parse `<provides>` entries and store them as capability data:

| AppStream type | Maps to | Use |
|---|---|---|
| `<library/>` | soname provide | Cross-distro lib dependency resolution |
| `<binary/>` | binary provide | Cross-distro binary dependency resolution |
| `<python3/>` | python module provide | Python dependency resolution |
| `<dbus/>` | dbus service provide | Service dependency resolution |

These feed into the `ProvidesIndex` alongside per-distro RPM/DEB metadata provides, giving the resolver cross-distro capability data that doesn't depend on having synced a specific distro's repo.

**New table (migration v59):**

```sql
CREATE TABLE appstream_provides (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    canonical_id INTEGER NOT NULL REFERENCES canonical_packages(id),
    provide_type TEXT NOT NULL,  -- 'library', 'binary', 'python3', 'dbus'
    capability TEXT NOT NULL,    -- 'libssl.so.3', 'nginx', 'requests'
    UNIQUE(canonical_id, provide_type, capability)
);
CREATE INDEX idx_appstream_provides_cap ON appstream_provides(capability);
```

### 5. Repology rules ingestion

Replace (or supplement) the Repology API path with direct ingestion of the `repology-rules` YAML ruleset:

**Rule types we consume:**
- `800.renames-and-merges/` -- the primary source for `{name: httpd, setname: apache}` mappings
- `100.prefix-suffix/` -- strip distro-specific prefixes (e.g., `python3-` on Fedora vs `python3-` on Debian -- same prefix, different package names underneath)

**Ingestion flow:**
1. Clone/update `repology-rules` repo (or vendor a snapshot)
2. Parse YAML rules matching our supported distros
3. Apply `setname` rules to build/update `canonical_packages` + `package_implementations` entries
4. Run during `conary canonical rebuild`

**Advantages over API-only:**
- No rate limiting (local data)
- Complete coverage (API returns only projects, not the mapping rules themselves)
- Deterministic (same rules = same mappings, no API drift)
- The rules are the upstream source of truth -- the API is derived from them

**Coexistence:** AppStream provides the component-level metadata (descriptions, icons, screenshots, provides). Repology rules provide the name-mapping layer. Both feed into `canonical_packages` / `package_implementations`. They complement each other.

### 6. SAT provider restructured

**`ConaryPackage` replaced by `PackageIdentity`.** The provider's `solvables: Vec<ConaryPackage>` becomes `solvables: Vec<PackageIdentity>`.

**Candidate loading simplified:**

```rust
fn load_candidates(&mut self, name: &str) -> Result<()> {
    let identities = PackageIdentity::find_all_by_name(self.conn, name)?;
    for identity in identities {
        let sid = self.register_solvable(identity);
        // Dependencies loaded using identity.version_scheme (explicit)
    }
    Ok(())
}
```

All viable candidates loaded (all versions, all repos). Architecture filtering uses `normalize_arch()`. The solver handles backtracking across the full candidate set.

**Canonical equivalents via SQL:**

```sql
SELECT DISTINCT rp2.name FROM repository_packages rp1
JOIN repository_packages rp2 ON rp1.canonical_id = rp2.canonical_id
WHERE rp1.name = ?1 AND rp2.name != ?1 AND rp1.canonical_id IS NOT NULL
```

One query returns cross-distro equivalent names. Those names load as additional `PackageIdentity` solvables. `sort_candidates` ranks exact-name above canonical by comparing the solvable's `name` against the requested name.

**Provides resolution uses the index:** Instead of per-dependency `ProvideEntry` queries, the provider queries `ProvidesIndex::find_providers()` which returns results in O(1) from the pre-built HashMap. The index includes both per-distro provides (from sync) and cross-distro provides (from AppStream).

**Dependency loading uses explicit scheme:** `loading.rs` reads `identity.version_scheme` directly instead of calling `infer_version_scheme(&repo)`.

### 7. Policy enforcement uses real identity

**`CandidateOrigin` deleted.** `accepts_candidate()` takes `&PackageIdentity`:

```rust
pub fn accepts_candidate(&self, identity: &PackageIdentity, is_root: bool) -> bool {
    match &self.request_scope {
        RequestScope::Repository(repo) => identity.repository_name == *repo,
        RequestScope::DistroFlavor(flavor) => scheme_matches_flavor(identity.version_scheme, *flavor),
        RequestScope::Any => true,
    }
    // SourceSelectionProfile.allowed_repositories checked against identity.repository_name
    // Mixing policy checked against identity.repository_distro
}
```

Exact field comparisons replace all string inference, substring matching, and flavor guessing.

**Deleted:**
- `CandidateOrigin` struct
- `infer_repo_flavor()` in `selector.rs`
- All substring/contains matching in policy

### 8. Graph resolver deletion

**Deleted files:**
- `conary-core/src/resolver/graph.rs`
- `conary-core/src/resolver/engine.rs`

**Kept files:**
- `conary-core/src/resolver/sat.rs` -- sole resolution entry point
- `conary-core/src/resolver/provider/` -- restructured around `PackageIdentity`
- `conary-core/src/resolver/canonical.rs` -- simplified, no more `ResolverCandidate`
- `conary-core/src/resolver/conflict.rs`
- `conary-core/src/resolver/component_resolver.rs`

**Caller migration:** All callers of `Resolver::resolve_install()` / `resolve()` switch to `solve_install()`. The SAT path already provides install ordering, missing dependency detection, and cycle detection.

`plan.rs` types (`ResolutionPlan`, `MissingDependency`) are used extensively in the install CLI pipeline (~15 references across `dependencies.rs`, `dep_resolution.rs`, `conversion.rs`). They must be kept and populated from `SatResolution` results. `plan.rs` is NOT deleted.

### 9. Migration and backwards compatibility

**Schema migration (v59):**

```sql
-- canonical_id on repository_packages
ALTER TABLE repository_packages ADD COLUMN canonical_id INTEGER
    REFERENCES canonical_packages(id) ON DELETE SET NULL;
CREATE INDEX idx_repo_packages_canonical ON repository_packages(canonical_id);

-- Backfill from existing data.
-- Use COALESCE to handle repos where default_strategy_distro is NULL
-- (common for repos added before v38 or unconfigured repos).
UPDATE repository_packages SET canonical_id = (
    SELECT pi.canonical_id FROM package_implementations pi
    JOIN repositories r ON repository_packages.repository_id = r.id
    WHERE pi.distro_name = repository_packages.name
      AND pi.distro = COALESCE(r.default_strategy_distro, r.name)
    LIMIT 1
) WHERE canonical_id IS NULL;

-- AppStream cross-distro provides
CREATE TABLE appstream_provides (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    canonical_id INTEGER NOT NULL REFERENCES canonical_packages(id),
    provide_type TEXT NOT NULL,
    capability TEXT NOT NULL,
    UNIQUE(canonical_id, provide_type, capability)
);
CREATE INDEX idx_appstream_provides_cap ON appstream_provides(capability);
```

**Graceful degradation:** `canonical_id` is nullable. Resolution works without it. Canonical links enable cross-distro resolution and richer policy; systems without canonical data resolve by name as today.

**No breaking CLI changes.** Inputs and outputs stay the same. Internal resolution path changes.

**Testing:**
- Cross-distro canonical resolution with correct version scheme
- Multi-arch candidates don't collapse
- Policy enforcement with real repository identity
- Missing canonical data degrades gracefully to name matching
- Provides index returns same results as direct queries
- AppStream provides feed into capability resolution
- Existing unit tests continue to pass (graph resolver tests deleted, others updated)

## Files touched

### New files
- `conary-core/src/resolver/identity.rs` -- `PackageIdentity` type + loading queries
- `conary-core/src/resolver/provides_index.rs` -- `ProvidesIndex` pre-built capability index

### Modified files
- `conary-core/src/db/migrations/v41_current.rs` -- v59 migration (canonical_id + appstream_provides)
- `conary-core/src/db/schema.rs` -- bump to v59
- `conary-core/src/repository/sync.rs` -- set canonical_id during sync
- `conary-core/src/canonical/appstream.rs` -- ingest origin + provides
- `conary-core/src/canonical/sync.rs` -- re-link command, Repology rules ingestion
- `conary-core/src/resolver/mod.rs` -- re-exports, remove graph resolver
- `conary-core/src/resolver/sat.rs` -- use PackageIdentity + ProvidesIndex
- `conary-core/src/resolver/provider/mod.rs` -- load PackageIdentity instead of ConaryPackage
- `conary-core/src/resolver/provider/types.rs` -- remove ConaryPackage
- `conary-core/src/resolver/provider/traits.rs` -- operate on PackageIdentity, always include canonical
- `conary-core/src/resolver/provider/loading.rs` -- use explicit version_scheme
- `conary-core/src/resolver/provider/matching.rs` -- match with real scheme
- `conary-core/src/resolver/canonical.rs` -- remove ResolverCandidate, simplify
- `conary-core/src/repository/resolution_policy.rs` -- delete CandidateOrigin, accept PackageIdentity
- `conary-core/src/repository/selector.rs` -- remove infer_repo_flavor
- `conary-core/src/repository/dependencies.rs` -- update resolver type usage
- `src/commands/install/mod.rs` -- switch from Resolver to solve_install()
- `src/commands/install/dependencies.rs` -- switch to SatResolution
- `src/commands/install/dep_resolution.rs` -- switch to SatResolution
- `src/commands/install/conversion.rs` -- switch to SatResolution
- `src/commands/remove.rs` -- use solve_removal
- `src/commands/query/dependency.rs` -- use solve_removal instead of Resolver::check_removal
- `conary-core/tests/canonical.rs` -- update for CanonicalResolver changes

### Deleted files
- `conary-core/src/resolver/graph.rs`
- `conary-core/src/resolver/engine.rs`

## Non-goals

- Changing the resolvo SAT solver itself
- Changing CLI flags or user-facing behavior
- RPM rich dependency parsing (separate effort)

## Future work: Repology database dump ingestion

After this redesign lands, the next phase is ingesting Repology's full PostgreSQL database dump (~2GB compressed, updated weekly at `dumps.repology.org`) for **version intelligence**. This is orthogonal to resolution but builds on the canonical_id infrastructure:

- **`repology_versions` table**: Cross-reference `repository_packages.canonical_id` against Repology's per-project version status (newest, outdated, devel, legacy)
- **`conary outdated`**: Show which installed packages have newer versions available across distros
- **`conary update --check`**: Preview available updates with cross-distro version context
- **Security advisories**: Flag packages with versions marked as vulnerable
- **Requires**: PostgreSQL dump parsing (SQL format, zstd compressed), `postgresql-libversion` semantics for version comparison

The `canonical_id` column added in this redesign is the join key that makes dump ingestion useful -- without it, Repology's project-level data can't be correlated to our repository_packages rows.

## References

- [AppStream Catalog Metadata Spec](https://www.freedesktop.org/software/appstream/docs/chap-CatalogData.html)
- [AppStream Upstream Metadata (Provides)](https://www.freedesktop.org/software/appstream/docs/chap-Metadata.html)
- [AppStream DEP-11 YAML Format](https://www.freedesktop.org/software/appstream/docs/sect-AppStream-YAML.html)
- [Repology API](https://repology.org/api)
- [Repology Rules Repository](https://github.com/repology/repology-rules)
- [Repology Database Dumps](https://dumps.repology.org/README.txt)
- [resolvo SAT Solver](https://github.com/prefix-dev/resolvo)
- [resolvo DependencyProvider Trait](https://docs.rs/resolvo/latest/resolvo/trait.DependencyProvider.html)
- [libsolv Pool Architecture](https://github.com/openSUSE/libsolv/blob/master/doc/libsolv-pool.txt)
