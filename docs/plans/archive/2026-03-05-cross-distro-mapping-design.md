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
Remi serves as the canonical authority. Multi-strategy auto-discovery handles
the bulk of mappings; Repology data bootstraps the registry; a curated overlay
covers edge cases.

## Prior Art

This design draws on research into existing cross-distro mapping efforts:

- **Repology** (repology.org): Tracks 120+ repos with a YAML-based rules
  system for package normalization (renames, merges, splits). The
  `/tools/project-by` API translates distro-specific names to unified project
  names. We bootstrap our canonical registry from Repology data.
- **AppStream** (freedesktop.org): Cross-distro component IDs using reverse-DNS
  (`org.mozilla.Firefox`). Already shipped by most desktop apps. We use
  AppStream IDs as canonical names where available.
- **distromatch** (Debian): Multi-strategy matching -- stem matching,
  binary-to-package mapping, popcon data. Service is down but the approach
  (especially binary path matching) is proven.
- **Nix pname**: 90,000+ packages each get a canonical `pname` --
  lowercase, hyphenated, matching upstream name. Proves the concept works at
  scale. We follow the same naming convention.
- **libsolv** (openSUSE): SAT solver used by DNF/Zypper/Conda. Natively
  handles Provides, Conflicts, Obsoletes, Supplements as first-class dependency
  types. We add Conflicts/Obsoletes to our resolver for canonical equivalents.

## Constraints

- Canonical resolution must be transparent -- `conary install curl` just works
- Users can pin to a distro or run unpinned (distro-agnostic)
- Remi is the primary source of truth; clients cache locally for offline use
- No new "group" concept -- groups are canonical packages with kind = "group"
- Database-first: all state in SQLite (migration v45)
- Canonical names: lowercase-hyphenated, match upstream project name (Nix convention)
- AppStream component IDs used as canonical names where available

## Design

### Section 1: Canonical Package Identity

Every package gets a canonical identity -- a distro-neutral name representing
"what this thing is." Distro packages are implementations of a canonical
identity.

```
Canonical: "apache-httpd"
  +-- AppStream: org.apache.httpd (if available)
  +-- Fedora:    httpd
  +-- Ubuntu:    apache2
  +-- Arch:      apache
  +-- CCS:       apache-httpd (native build)

Canonical: "which"
  +-- Fedora:  which
  +-- Ubuntu:  debianutils (provides: which)
  +-- Arch:    which
```

**Naming convention** (following Nix pname):
- Lowercase, hyphenated: `apache-httpd`, `lib-archive`, `python-requests`
- Match upstream project name where possible
- No distro-specific prefixes (`lib` prefix only when upstream uses it)

**How canonical names are established (priority order):**
1. AppStream component IDs -- reverse-DNS, globally unique, already shipped by
   most GUI apps. Gives free interop with Flatpak/Snap ecosystem.
2. Repology project names -- bootstraps thousands of mappings on day one from
   their 120+ repo database via API sync.
3. Multi-strategy auto-discovery during repo sync (see Section 5).
4. Curated overlay in Repology-compatible YAML rules format for edge cases.
5. User/admin custom mappings via overrides file.

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

The SAT resolver gains canonical awareness plus Conflicts/Obsoletes support
(following libsolv's model).

**Resolution pipeline:**

```
User: "conary install curl"
  |
  +- 1. Lookup: canonical name? -> get all implementations
  |     OR: distro-specific name? -> resolve to canonical -> implementations
  |     OR: AppStream ID? -> resolve to canonical -> implementations
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
  |     +- Conflicts: canonical equivalents conflict with each other
  |     |  (httpd and apache2 can't both be installed)
  |     +- Obsoletes: newer canonical implementation obsoletes older
  |
  +- 5. Mixing policy check (if pinned + cross-distro candidate won):
        +- Strict: reject, suggest pinned alternative
        +- Guarded: warn, show what's happening, proceed
        +- Permissive: proceed silently
```

**Conflicts and Obsoletes** (inspired by libsolv):
- When `httpd` and `apache2` are implementations of the same canonical
  `apache-httpd`, installing one implicitly conflicts with the other. The
  resolver treats them as mutually exclusive providers of the same canonical.
- When a package is renamed across distro versions (e.g., `python3.11` →
  `python3.12`), the newer implementation obsoletes the older one during
  upgrades.

Source affinity is computed from installed packages -- if 80% are Ubuntu,
Ubuntu gets a boost. Tracked in `system_affinity` table, recalculated per
transaction.

Key change: `ConaryProvider` gains a `CanonicalResolver` that expands package
names to canonical identities before feeding candidates to the SAT solver. The
SAT solver itself doesn't change -- we change which candidates it sees and how
they're ranked.

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

**Multi-strategy auto-discovery during repo sync** (inspired by distromatch):

1. **Provides fields** -- RPM `Provides:`, DEB virtual packages, Arch
   `provides()` arrays ingested into package_implementations
2. **Name matching** -- identical names across distros assumed same canonical
   (covers ~80% of packages)
3. **Binary path matching** -- `/usr/bin/which` owned by different packages
   across distros maps them to the same canonical. Powerful for utilities
   bundled into different packages per distro.
4. **Library soname matching** -- `libssl.so.3` from different distros maps to
   same canonical even if package names differ (`openssl-libs` vs `libssl3`)
5. **Stem matching** -- strip common prefixes/suffixes (`lib`, `-dev`, `-devel`,
   `-libs`, `-common`) to match base package identity
6. **Conflict detection** -- curated registry wins over auto-discovery
   contradictions; ambiguous matches logged for manual review

**Repology bootstrap:**

On first sync (or `conary registry update`), Conary fetches canonical mappings
from Repology's API:
- `/api/v1/project/<name>` returns all implementations across 120+ repos
- `/tools/project-by?repo=<repo>&name=<pkg>` translates distro names to
  Repology project names
- This bootstraps thousands of canonical mappings before any local repo sync

The Repology data is cached locally and refreshed periodically. It serves as
the baseline; local auto-discovery and curated rules refine it.

**AppStream integration:**

During repo sync, AppStream catalog XML/YAML is parsed. Packages with
AppStream component IDs (`org.mozilla.Firefox`) get that ID stored as an
alternate canonical identifier. This enables:
- `conary install org.mozilla.Firefox` works
- Cross-reference with Flatpak/Snap: same component ID across packaging formats

**Curated overlay (`/usr/share/conary/canonical-rules/`):**

Follows Repology's proven YAML rules format for familiarity and potential
rule sharing:

```yaml
# 800.renames-and-merges/apache.yaml
- { setname: apache-httpd, name: httpd, repo: fedora_41 }
- { setname: apache-httpd, name: apache2, repo: ubuntu_24_04 }
- { setname: apache-httpd, name: apache, repo: arch }

# 800.renames-and-merges/groups.yaml
- { setname: dev-tools, kind: group, name: build-essential, repo: ubuntu_24_04 }
- { setname: dev-tools, kind: group, name: "@development-tools", repo: fedora_41 }
- { setname: dev-tools, kind: group, name: base-devel, repo: arch }
```

Rules are processed in numbered order (matching Repology convention):
- `500.wildcard.yaml` -- broad pattern rules
- `800.renames-and-merges/*.yaml` -- specific name mappings
- `850.split-ambiguities/*.yaml` -- disambiguation for shared names
- `900.version-fixes/*.yaml` -- version normalization

**User overrides:** `/etc/conary/canonical-overrides.yaml` for local additions.
`conary registry update` pulls updated rules from Remi between releases.

**Scale estimate:** Repology bootstrap covers tens of thousands of packages.
Curated rules handle ~200-500 known problem cases. Auto-discovery fills
gaps. The long tail of obscure packages may not have canonical mappings --
they're still installable by distro-specific name.

### Section 6: Remi as Canonical Authority

Remi is central -- it syncs all distro repos, converts to CCS, and has the
full cross-distro picture.

**Remi's role:**
1. Syncs all distro repos, builds canonical mapping server-side with full
   cross-distro visibility (more accurate than any single client)
2. Syncs Repology data and merges with local auto-discovery results
3. Serves canonical metadata -- client asks "what implementations exist for X?"
4. CCS conversion is canonically-aware -- converted packages carry
   `canonical = "apache-httpd"` tag and AppStream ID if available
5. Curated rules and Repology cache live on Remi, clients sync them

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
    appstream_id TEXT,                     -- e.g., 'org.mozilla.Firefox'
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
    source TEXT NOT NULL DEFAULT 'auto',   -- 'auto' | 'repology' | 'appstream' | 'curated' | 'user'
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
- `canonical_packages` gains `appstream_id` for AppStream component ID lookup
- `package_implementations.source` expanded: 'auto' | 'repology' | 'appstream'
  | 'curated' | 'user' -- tracks where the mapping came from
- `provides` gains `canonical_id INTEGER REFERENCES canonical_packages(id)`
- `repository_packages` gains `distro TEXT`
- `repositories` gains `distro TEXT`

**Indexes:** `canonical_packages(name)`, `canonical_packages(appstream_id)`,
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
conary install org.mozilla.Firefox   # AppStream component ID
conary install httpd                 # distro name, resolves to canonical
conary install build-essential       # group
conary install mesa --from fedora-41 # explicit cross-distro override

conary canonical curl                # show identity + all implementations
conary canonical --search http       # search canonical registry
conary canonical --unmapped          # installed packages without mapping

conary groups                        # list available groups
conary groups dev-tools              # show members
conary groups --distro fedora-41     # distro-specific view

conary registry update               # sync from Remi (includes Repology data)
conary registry stats                # mapping coverage + source breakdown
```

### Section 9: Error Handling and Testing

**Error cases:**
- Ambiguous name (maps to multiple canonicals): disambiguation prompt
- No implementation for pin: suggest --from or available alternatives
- Mixing policy violation: clear error with remediation steps
- Conflicts between canonical equivalents: resolver rejects, explains why
- Circular canonical resolution: detect during sync, warn, skip
- Stale registry: prompt for `conary registry update`
- Repology API unavailable: fall back to local auto-discovery + curated rules

**Testing:**
- Unit: canonical lookup, ranking, affinity, mixing policy, auto-discovery,
  Repology data parsing, AppStream ID resolution, conflict/obsoletes handling
- Integration: full resolution with mock multi-distro repos, pinned/unpinned,
  multi-strategy auto-discovery (provides + binary path + stem matching)
- Registry: YAML rule parsing, Repology format compatibility, rule precedence,
  user overrides
- Remi: canonical metadata in CCS, registry sync endpoint, query API,
  Repology sync
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
| Resolver | conary-core/src/resolver/conflicts.rs (NEW) |
| Resolver | conary-core/src/resolver/provider.rs |
| Resolver | conary-core/src/resolver/sat.rs |
| Repo sync | conary-core/src/repository/sync.rs |
| Repo sync | conary-core/src/repository/appstream.rs (NEW) |
| Repo sync | conary-core/src/repository/repology.rs (NEW) |
| Registry | conary-core/src/canonical/ (NEW module) |
| Registry | conary-core/src/canonical/rules.rs (YAML rule engine) |
| Registry | conary-core/src/canonical/discovery.rs (multi-strategy) |
| Remi | conary-server/src/server/canonical.rs (NEW) |
| CCS | conary-core/src/ccs/legacy/mod.rs (replace hardcoded mappings) |
| CLI | src/cli/pin.rs (NEW) |
| CLI | src/cli/canonical.rs (NEW) |
| CLI | src/cli/groups.rs (NEW) |
| CLI | src/commands/pin.rs (NEW) |
| CLI | src/commands/canonical.rs (NEW) |
| CLI | src/commands/groups.rs (NEW) |
| CLI | src/commands/install.rs (--from flag) |
| Data | data/canonical-rules/ (NEW, Repology-format YAML) |
| Data | data/distros.toml (NEW) |
| Model | conary-core/src/model/parser.rs (distro/mixing fields) |

## References

- Repology API: https://repology.org/api
- Repology rules: https://github.com/repology/repology-rules
- AppStream spec: https://www.freedesktop.org/software/appstream/docs/
- distromatch: https://github.com/spanezz/distromatch
- Debian wiki (package name mapping): https://wiki.debian.org/Mapping%20package%20names%20across%20distributions
- Nix pname convention: https://github.com/NixOS/nixpkgs/blob/master/pkgs/by-name/README.md
- libsolv: https://github.com/openSUSE/libsolv
- resolvo: https://github.com/prefix-dev/resolvo
- ecosyste-ms resolver reference: https://github.com/ecosyste-ms/package-manager-resolvers

## Success Criteria

- `conary install apache-httpd` resolves to httpd/apache2/apache based on pin
- `conary install httpd` on Ubuntu-pinned system resolves via canonical mapping
- `conary install org.mozilla.Firefox` resolves via AppStream component ID
- `conary pin ubuntu-noble` constrains resolution to Ubuntu packages
- Mixing policies (strict/guarded/permissive) enforced correctly
- Conflicts prevent installing two implementations of same canonical
- Package groups resolve to distro-appropriate metapackage/comps group
- Repology bootstrap populates thousands of canonical mappings on first sync
- Multi-strategy auto-discovery (provides, binary path, stem, soname) works
- Curated YAML rules override auto-discovery for known problem cases
- Remi serves CCS packages tagged with canonical identity + AppStream ID
- Source affinity influences unpinned resolution
- All existing tests pass + new tests for canonical resolution
