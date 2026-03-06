# Design: Cross-Distro Package Mapping and Canonical Identity

*2026-03-05*

## Problem

Packages have different names, groupings, and availability across distros.
`which` is a standalone package on Fedora but lives inside `debianutils` on
Ubuntu. NVIDIA drivers come from main repos on Arch, RPMFusion on Fedora, and
different PPAs on Ubuntu. Package groups like `build-essential` (Debian) and
`@development-tools` (Fedora) serve the same purpose but have no shared
identity. Conary needs to handle all of this transparently.

## Approach

Full Abstraction Layer (Approach 3). Every package gets a canonical
distro-neutral identity. Distro packages are implementations of that identity.
Remi serves as the canonical authority. Auto-discovery from repo metadata
handles the bulk of mappings; a curated registry covers edge cases.

## Constraints

- Canonical resolution must be transparent -- `conary install curl` just works
- Users can pin to a distro or run unpinned (distro-agnostic)
- Remi is the primary source of truth; clients cache locally for offline use
- No new "group" concept -- groups are canonical packages with kind = "group"
- Database-first: all state in SQLite (migration v45)

## Design

### Section 1: Canonical Package Identity

Every package gets a canonical identity -- a distro-neutral name representing
"what this thing is." Distro packages are implementations of a canonical
identity.

```
Canonical: "apache-httpd"
  +-- Fedora:  httpd
  +-- Ubuntu:  apache2
  +-- Arch:    apache
  +-- CCS:     apache-httpd (native build)

Canonical: "which"
  +-- Fedora:  which
  +-- Ubuntu:  debianutils (provides: which)
  +-- Arch:    which
```

How canonical names are established:
- Auto-discovered during repo sync from Provides metadata (bulk)
- Curated registry shipped with Conary for known problem cases (~200-500)
- User/admin can add custom mappings via overrides file

The canonical name is what users interact with. `conary install apache-httpd`
works regardless of pin. Distro-native names also work --
`conary install httpd` on a Fedora-pinned system resolves transparently.

### Section 2: Distro Pinning

A distro pin declares "this system follows distro X's package set." Stored in
the system model (source of truth), manageable via CLI.

**System model:**
```toml
[system]
distro = "ubuntu-noble"
mixing = "guarded"
```

**Mixing policies:**
- **Strict**: only pinned distro's packages. Cross-distro blocked unless
  per-package override.
- **Guarded** (default): cross-distro allowed if resolver proves dependencies
  satisfiable. Warns.
- **Permissive**: install anything, just warn about conflicts.

**Per-package overrides:**
```toml
[overrides]
mesa = { from = "fedora-41" }
nvidia-driver = { from = "rpmfusion-41" }
```

**Unpinned mode** (no distro set): Conary is fully distro-agnostic. All repos
are equal candidates. Resolution order: repo priority > source affinity >
newest version.

**Distro registry:** Conary ships `distros.toml` defining known distros (name,
label format, repo URLs, release cadence). Users can add custom definitions.

### Section 3: Resolution Order and Resolver Changes

The SAT resolver gains canonical awareness. Resolution pipeline:

```
User: "conary install curl"
  |
  +- 1. Lookup: canonical name? -> get all implementations
  |     OR: distro-specific name? -> resolve to canonical -> implementations
  |
  +- 2. Filter by context:
  |     +- Pinned?  -> prefer pinned distro's implementation
  |     +- Unpinned? -> all implementations are candidates
  |
  +- 3. Rank candidates:
  |     +- 1st: Repo priority (user-configured)
  |     +- 2nd: Source affinity (prefer distro of installed packages)
  |     +- 3rd: Newest version
  |     +- 4th: CCS native preferred over distro packages (tiebreaker)
  |
  +- 4. SAT solve with ranked candidates
  |
  +- 5. Mixing policy check (if pinned + cross-distro candidate won):
        +- Strict: reject, suggest pinned alternative
        +- Guarded: warn, show what's happening, proceed
        +- Permissive: proceed silently
```

Source affinity is computed from installed packages -- if 80% are Ubuntu,
Ubuntu gets a boost. Tracked in `system_affinity` table, recalculated per
transaction.

Key change: `ConaryProvider` gains a `CanonicalResolver` that expands package
names to canonical identities before feeding candidates to the SAT solver. The
SAT solver itself doesn't change.

### Section 4: Package Groups as Virtual Provides

Groups are canonical packages whose implementations are metapackages or comps
groups. No new concept needed.

```
Canonical: "dev-tools" (kind = "group")
  +-- Ubuntu:  build-essential
  +-- Fedora:  @development-tools
  +-- Arch:    base-devel
  +-- CCS:     dev-tools
```

During repo sync:
- DEB: metapackages (deps only, no files) detected, linked to canonical
- RPM: comps.xml parsed, groups become virtual provides with member deps
- Arch: meta packages detected same as DEB
- CCS: groups are recipes with `[package] kind = "group"`

### Section 5: Auto-Discovery and Curated Registry

**Auto-discovery during repo sync:**
1. Provides fields (RPM Provides:, DEB virtual packages, Arch provides())
   ingested into package_implementations
2. Name matching -- identical names across distros assumed same canonical
   (covers ~80% of packages)
3. Library soname matching -- libssl.so.3 from different distros maps to same
   canonical even if package names differ
4. Conflict detection -- curated registry wins over auto-discovery contradictions

**Curated registry (`/usr/share/conary/canonical-registry.toml`):**
```toml
[packages.apache-httpd]
description = "Apache HTTP Server"
category = "net/http"
implementations = [
    { distro = "fedora", name = "httpd" },
    { distro = "ubuntu", name = "apache2" },
    { distro = "arch", name = "apache" },
]

[groups.dev-tools]
description = "Core development tools"
implementations = [
    { distro = "fedora", name = "@development-tools" },
    { distro = "ubuntu", name = "build-essential" },
    { distro = "arch", name = "base-devel" },
]
```

**User overrides:** `/etc/conary/canonical-overrides.toml` for local additions.
Future `conary registry update` pulls updates from Remi between releases.

### Section 6: Remi as Canonical Authority

Remi is central -- it syncs all distro repos, converts to CCS, and has the
full cross-distro picture.

**Remi's role:**
1. Syncs all distro repos, builds canonical mapping server-side
2. Serves canonical metadata -- client asks "what implementations exist for X?"
3. CCS conversion is canonically-aware -- converted packages carry
   `canonical = "apache-httpd"` tag
4. Curated registry lives on Remi, clients sync it

**Revised flow:**
```
Client: "conary install apache-httpd"
  |
  +- Local canonical_packages has it?
  |   +- Yes -> check local implementations, resolve
  |   +- No -> ask Remi
  |
  +- Remi returns implementations:
  |     +- CCS (pre-converted, chunked, ready) <- preferred
  |     +- Fedora: httpd-2.4.62
  |     +- Ubuntu: apache2-2.4.58
  |
  +- Client resolver picks based on pin + policy
  |
  +- Remi serves the CCS package (already converted)
      or converts on-demand if not cached
```

In most cases, Remi has already converted the package to CCS. The client gets
a CCS package tagged with its canonical identity. The distro-specific details
are Remi's problem, not the client's.

Client still needs local tables for offline resolution, pinning logic, and
standalone mode (no Remi).

### Section 7: Database Schema (Migration v45)

**New tables:**
```sql
CREATE TABLE canonical_packages (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    description TEXT,
    kind TEXT NOT NULL DEFAULT 'package',  -- 'package' | 'group'
    category TEXT
);

CREATE TABLE package_implementations (
    id INTEGER PRIMARY KEY,
    canonical_id INTEGER NOT NULL REFERENCES canonical_packages(id),
    distro TEXT NOT NULL,
    distro_name TEXT NOT NULL,
    repo_id INTEGER REFERENCES repositories(id),
    source TEXT NOT NULL DEFAULT 'auto',  -- 'auto' | 'curated' | 'user'
    UNIQUE(canonical_id, distro, distro_name)
);

CREATE TABLE distro_pin (
    id INTEGER PRIMARY KEY,
    distro TEXT NOT NULL,
    mixing_policy TEXT NOT NULL DEFAULT 'guarded',
    created_at TEXT NOT NULL
);

CREATE TABLE package_overrides (
    id INTEGER PRIMARY KEY,
    canonical_id INTEGER NOT NULL REFERENCES canonical_packages(id),
    from_distro TEXT NOT NULL,
    reason TEXT
);

CREATE TABLE system_affinity (
    distro TEXT PRIMARY KEY,
    package_count INTEGER NOT NULL DEFAULT 0,
    percentage REAL NOT NULL DEFAULT 0.0,
    updated_at TEXT NOT NULL
);
```

**Changes to existing tables:**
- `provides` gains `canonical_id INTEGER REFERENCES canonical_packages(id)`
- `repository_packages` gains `distro TEXT`
- `repositories` gains `distro TEXT`

**Indexes:** `canonical_packages(name)`,
`package_implementations(distro, distro_name)`,
`package_implementations(canonical_id)`.

### Section 8: CLI Surface

```
conary pin ubuntu-noble              # pin to distro
conary pin --list                    # show available distros
conary pin --info                    # current pin + affinity stats
conary pin --mixing permissive       # change mixing policy
conary pin --remove                  # unpin

conary install curl                  # canonical or distro name
conary install apache-httpd          # canonical name
conary install httpd                 # distro name, resolves to canonical
conary install build-essential       # group
conary install mesa --from fedora-41 # explicit cross-distro override

conary canonical curl                # show identity + all implementations
conary canonical --search http       # search canonical registry
conary canonical --unmapped          # installed packages without mapping

conary groups                        # list available groups
conary groups dev-tools              # show members
conary groups --distro fedora-41     # distro-specific view

conary registry update               # sync canonical registry from Remi
conary registry stats                # mapping coverage
```

### Section 9: Error Handling and Testing

**Error cases:**
- Ambiguous name (maps to multiple canonicals): disambiguation prompt
- No implementation for pin: suggest --from or available alternatives
- Mixing policy violation: clear error with remediation steps
- Circular canonical resolution: detect during sync, warn, skip
- Stale registry: prompt for `conary registry update`

**Testing:**
- Unit: canonical lookup, ranking, affinity, mixing policy, auto-discovery
- Integration: full resolution with mock multi-distro repos, pinned/unpinned
- Registry: TOML parsing, curated vs auto precedence, user overrides
- Remi: canonical metadata in CCS, registry sync endpoint, query API
- End-to-end: forge server with real distro repos (not CI)

## Files Changed

| Area | Files |
|------|-------|
| Schema | conary-core/src/db/migrations.rs (v45) |
| Models | conary-core/src/db/models/canonical.rs (NEW) |
| Models | conary-core/src/db/models/distro_pin.rs (NEW) |
| Models | conary-core/src/db/models/package_impl.rs (NEW) |
| Models | conary-core/src/db/models/system_affinity.rs (NEW) |
| Resolver | conary-core/src/resolver/canonical.rs (NEW) |
| Resolver | conary-core/src/resolver/provider.rs |
| Resolver | conary-core/src/resolver/sat.rs |
| Repo sync | conary-core/src/repository/sync.rs |
| Registry | conary-core/src/canonical/ (NEW module) |
| Remi | conary-server/src/server/canonical.rs (NEW) |
| CCS | conary-core/src/ccs/legacy/mod.rs (replace hardcoded mappings) |
| CLI | src/cli/pin.rs (NEW) |
| CLI | src/cli/canonical.rs (NEW) |
| CLI | src/cli/groups.rs (NEW) |
| CLI | src/commands/pin.rs (NEW) |
| CLI | src/commands/canonical.rs (NEW) |
| CLI | src/commands/groups.rs (NEW) |
| CLI | src/commands/install.rs (--from flag) |
| Data | data/canonical-registry.toml (NEW) |
| Data | data/distros.toml (NEW) |
| Model | conary-core/src/model/parser.rs (distro/mixing fields) |

## Success Criteria

- `conary install apache-httpd` resolves to httpd/apache2/apache based on pin
- `conary install httpd` on Ubuntu-pinned system resolves via canonical mapping
- `conary pin ubuntu-noble` constrains resolution to Ubuntu packages
- Mixing policies (strict/guarded/permissive) enforced correctly
- Package groups resolve to distro-appropriate metapackage/comps group
- Auto-discovery populates canonical mappings during repo sync
- Curated registry overrides auto-discovery for known problem cases
- Remi serves CCS packages tagged with canonical identity
- Source affinity influences unpinned resolution
- All existing tests pass + new tests for canonical resolution
