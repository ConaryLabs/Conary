# M4d Supported Distro Adapter Profiles Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status:** Locked implementation plan after DeepSeek, Gemini, and local agentic review.

**Goal:** Replace scattered supported-distro policy with one compile-time embedded profile catalog for Fedora 44, Ubuntu 26.04, and Arch, then route CLI, lifecycle validation, Remi, conversion, and sync behavior through that catalog.

**Architecture:** Add `crates/conary-core/src/repository/supported_profiles/` as the profile owner, with `catalog.toml` embedded by `include_str!` and parsed into typed Rust data through `LazyLock`. Keep parser backends and family-specific package parsing in Rust, but move public IDs, route slugs, dependency flavor, version scheme, replay target facts, repository hints, lifecycle policy, and supported-target lists into profile-backed APIs. Extend the existing CCS v2 `TargetProfileQuery` hook and cut Remi route validation over to profile route metadata before database, filesystem, key, or publish-gate work.

**Tech Stack:** Rust 2024, serde/toml, std `LazyLock`, rusqlite, Axum, existing CCS v2 validation and diagnostics, existing Remi route handlers, existing M2 publish-gate tests, Cargo test.

---

## Design Inputs

Read these before executing:

- `AGENTS.md`
- `docs/llms/README.md`
- `docs/superpowers/specs/2026-06-18-m4d-supported-distro-adapter-profiles-design.md`
- `docs/superpowers/specs/2026-06-17-m4-ccs-native-ecosystem-design.md`
- `docs/superpowers/specs/2026-06-17-m4a-ccs-v2-native-package-contract-design.md`
- `docs/superpowers/specs/2026-06-18-m4b-native-authoring-build-lint-test-design.md`
- `docs/superpowers/specs/2026-06-18-m4c-remi-native-ccs-publication-design.md`
- `docs/superpowers/plans/2026-06-05-ccs-native-ecosystem-roadmap.md`
- `docs/modules/source-selection.md`
- `docs/modules/remi.md`
- `docs/modules/ccs.md`
- `docs/modules/test-fixtures.md`
- `docs/modules/feature-ownership.md`
- `docs/llms/subsystem-map.md`
- `data/distros.toml`
- `crates/conary-core/src/repository/distro.rs`
- `crates/conary-core/src/ccs/v2/validation.rs`
- `crates/conary-core/src/ccs/v2/schema.rs`
- `apps/conary/src/commands/distro.rs`
- `apps/conary/src/commands/install/source_policy.rs`
- `apps/remi/src/server/handlers/mod.rs`
- `apps/remi/src/server/routes/public.rs`
- `apps/remi/src/server/routes/admin.rs`
- `apps/remi/src/server/handlers/index.rs`
- `apps/remi/src/server/handlers/packages.rs`
- `apps/remi/src/server/handlers/sparse.rs`
- `apps/remi/src/server/handlers/tuf.rs`
- `apps/remi/src/server/handlers/detail.rs`
- `apps/remi/src/server/handlers/admin/packages.rs`
- `apps/remi/src/server/native_publish/verify.rs`
- `apps/remi/src/server/conversion/lookup.rs`
- `apps/remi/src/server/conversion/metadata.rs`
- `crates/conary-core/src/repository/sync/remi.rs`

## Scope Locks

M4d includes:

- One embedded supported-target profile catalog with exactly `fedora-44`, `ubuntu-26.04`, and `arch`.
- Deletion of the old `data/distros.toml` duplicate public catalog. M4d does not keep a generated alias unless a later implementation step proves a current non-test consumer still needs it.
- Typed profile data for display names, release facts, family slugs, Remi route slugs, package formats, dependency flavors, version schemes, replay targets, repository hints, and lifecycle policies.
- Public profile lookup by public ID.
- Route lookup by Remi route slug, returning route metadata plus matching public profile IDs.
- Exact public-profile lookup semantics: trim only, case-sensitive, no public-ID fallback through family slug lookup. Callers that want aliases must add explicit alias APIs in a future slice.
- Exact string-domain validation: `package_format = rpm|deb|arch`, `dependency_flavor = rpm|deb|arch`, `version_scheme = rpm|debian|arch`, and `replay_target.format = rpm|deb|arch`.
- `conary distro list` and `conary distro set` backed by public profile IDs.
- Source-policy and resolver helpers backed by profile-derived dependency flavor and version scheme.
- Public-profile replay target mapping only.
- Fail-closed deletion or quarantine of old generic/unsupported replay normalization such as `debian-13`, generic `fedora`, and generic `ubuntu`.
- CCS v2 lifecycle validation for every signed lifecycle vector: users, groups, directories, services, tmpfiles, sysctl, and alternatives.
- Per-entry lifecycle policy, using explicit allow lists, reviewed patterns, or `unsupported`.
- Remi route validation for every `{distro}` public/admin route before DB, filesystem/cache, key-path, or publish-gate work.
- Remi conversion lookup, parser dispatch, native release upload validation, and sync version-scheme derivation backed by profiles.
- Focused unsupported-route proof for sparse index, TUF metadata, TUF refresh, and admin package upload.
- M2 publish-gate regression proof because M4d changes Remi route validation and release-upload support checks.
- Docs-audit, inventory, feature-coherency, and workspace verification gates.

M4d excludes:

- Adding public Debian, Linux Mint, Fedora next, Ubuntu noble, or derivative support.
- Runtime-loaded or user-editable profile catalogs.
- Plugin-provided distro profiles.
- Making package parser backends declarative.
- Host I/O in core validation or profile lookups.
- A database migration/backfill for persisted `version_scheme` rows.
- Changing the existing Remi route slugs `fedora`, `ubuntu`, and `arch`.
- M4e proof corpus work.
- New package-manager execution authority semantics beyond flavor, scheme, parser, and repository-selection facts.

## File Map

Create:

- `crates/conary-core/src/repository/supported_profiles/catalog.toml` - embedded profile catalog.
- `crates/conary-core/src/repository/supported_profiles/mod.rs` - public API, cached catalog parsing, lookup functions, and compatibility exports.
- `crates/conary-core/src/repository/supported_profiles/types.rs` - typed profile structs and enums.
- `crates/conary-core/src/repository/supported_profiles/lifecycle.rs` - per-entry lifecycle policy matcher and `TargetProfileQuery` implementation.
- `crates/conary-core/src/repository/supported_profiles/tests.rs` - catalog, lookup, replay, lifecycle, and unsupported-target tests.
- `apps/conary/tests/packaging_m4d.rs` - CLI-facing profile smoke proof and unsupported public ID proof.

Modify:

- `data/distros.toml` - delete; supported public distro data moves to the embedded profile catalog.
- `crates/conary-core/src/repository/mod.rs` - export `supported_profiles`.
- `crates/conary-core/src/repository/distro.rs` - reduce to profile-backed compatibility shims or delete after callers move.
- `crates/conary-core/src/ccs/v2/validation.rs` - extend `TargetProfileQuery` and validator loops.
- `apps/conary/src/commands/distro.rs` - use public profile list and validate pins.
- `apps/conary/src/commands/install/source_policy.rs` - derive request flavor from profile public ID or route/family lookup.
- `apps/conary/src/commands/update/source_policy.rs` and nearby update selection helpers if they still call `repository::distro`.
- `apps/conary/src/commands/install/legacy_replay.rs` and remove/update replay callers if they still normalize unsupported distro IDs.
- `apps/remi/src/server/handlers/mod.rs` - replace `SUPPORTED_DISTROS` with profile-backed route validation helpers.
- `apps/remi/src/server/handlers/index.rs` - validate route slugs before metadata DB work.
- `apps/remi/src/server/handlers/packages.rs` - validate route slugs before package DB/download/delta work.
- `apps/remi/src/server/handlers/sparse.rs` - validate route slugs before sparse DB/federated work.
- `apps/remi/src/server/handlers/tuf.rs` - validate route slugs before metadata/key-path work.
- `apps/remi/src/server/handlers/detail.rs` - validate route slugs before detail DB work.
- `apps/remi/src/server/handlers/admin/mod.rs` - validate release-upload route slugs before release publish work and expose admin JSON validation wrapper.
- `apps/remi/src/server/handlers/admin/packages.rs` - validate route slugs before upload cache path or review artifact work.
- `apps/remi/src/server/native_publish/verify.rs` - validate release upload route slugs through profiles.
- `apps/remi/src/server/conversion/lookup.rs` - use profile repository hints and profile version scheme.
- `apps/remi/src/server/conversion/metadata.rs` - use profile package format/backend selection.
- `crates/conary-core/src/repository/sync/remi.rs` - derive route version scheme from profiles without unknown-to-RPM fallback at route derivation sites.
- `docs/modules/source-selection.md`, `docs/modules/remi.md`, `docs/modules/ccs.md`, `docs/modules/test-fixtures.md`, `docs/modules/feature-ownership.md`, and `docs/llms/subsystem-map.md` - update after implementation passes.
- `docs/superpowers/feature-coherency-ledger.tsv` and `docs/superpowers/feature-coherency-wave-scopes.tsv` - update route/public-claim rows when implementation changes public behavior.
- `docs/superpowers/documentation-accuracy-audit-inventory.tsv` and `docs/superpowers/documentation-accuracy-audit-ledger.tsv` - update after plan lock and implementation docs.

Maintainability boundaries:

- `repository/supported_profiles/` owns profile parsing, lookup, lifecycle policy, and string-domain validation.
- `ccs/v2/validation.rs` owns validation loops and diagnostics. It may define `TargetProfileQuery`; profile-backed types implement it without moving profile data into `ccs/v2`.
- `apps/remi/src/server/handlers/mod.rs` owns shared route validation helpers. Individual handlers call the helper before DB/filesystem/key/trust work.
- `apps/remi/src/server/handlers/admin/mod.rs` owns the admin JSON wrapper around supported-route validation because admin endpoints use `json_error` responses.
- Large files and orchestrators such as `crates/conary-core/src/ccs/manifest.rs` and `crates/conary-core/src/recipe/kitchen/cook.rs` must delegate to profile APIs and must not embed new distro policy.
- `crates/conary-core/src/repository/distro.rs` is migration scaffolding only after M4d. Keep it as small re-export/shim if it reduces churn; do not preserve duplicate logic there.
- Existing persisted-row RPM defaults in resolver/automation stay out of M4d. Only new route/profile derivation sites fail closed.

## Checkpoints

- Checkpoint 1 after Task 1: profile catalog tests pass.
- Checkpoint 2 after Task 2: CLI/source-policy/replay tests pass.
- Checkpoint 3 after Task 3: CCS v2 lifecycle profile tests pass.
- Checkpoint 4 after Task 4: Remi route, conversion, native release validation, and sync tests pass.
- Checkpoint 5 after Task 5: focused M4d integration proof plus M2 regression gates pass.
- Checkpoint 6 after Task 6: docs/audit/coherency/workspace verification passes.

## Review Lock Mapping

| Design concern | Plan owner |
| --- | --- |
| Compile-time embedded catalog | Task 1 |
| Exactly three public IDs | Task 1 and Task 5 |
| Public/internal route split | Task 1, Task 2, Task 4 |
| `deb` vs `debian` string domains | Task 1 |
| No runtime profile file I/O | Task 1 and Task 3 |
| Delete old `data/distros.toml` duplicate catalog | Task 1 and Task 5 |
| `conary distro set` fail-closed validation | Task 2 |
| Non-public replay normalization deletion/quarantine | Task 2 |
| Profile-derived dependency flavor and version scheme | Task 1, Task 2, Task 4 |
| Full lifecycle vector coverage | Task 3 |
| Per-entry lifecycle policy | Task 1 and Task 3 |
| Remi route inventory and unsupported-route proof | Task 4 and Task 5 |
| Remi native release trust gates preserved | Task 4 and Task 5 |
| Parser backend mapping stays Rust-owned | Task 4 |
| Stored-row RPM defaults intentionally preserved | Task 4 and Task 5 |
| Docs-audit and coherency updates | Task 6 |

---

### Task 1: Add The Embedded Supported Profile Catalog

**Files:**
- Create: `crates/conary-core/src/repository/supported_profiles/catalog.toml`
- Create: `crates/conary-core/src/repository/supported_profiles/types.rs`
- Create: `crates/conary-core/src/repository/supported_profiles/mod.rs`
- Create: `crates/conary-core/src/repository/supported_profiles/tests.rs`
- Modify: `crates/conary-core/src/repository/mod.rs`
- Modify: `crates/conary-core/src/repository/distro.rs`
- Test: `cargo test -p conary-core supported_profiles`

- [ ] **Step 1: Write failing catalog tests**

Add `crates/conary-core/src/repository/supported_profiles/tests.rs`:

```rust
use super::*;
use crate::repository::dependency_model::RepositoryDependencyFlavor;
use crate::repository::versioning::VersionScheme;

#[test]
fn catalog_contains_exact_public_profiles() {
    let ids: Vec<_> = public_profiles().iter().map(|profile| profile.id()).collect();
    assert_eq!(ids, vec!["fedora-44", "ubuntu-26.04", "arch"]);
}

#[test]
fn catalog_rejects_unsupported_public_ids() {
    for id in ["debian", "debian-13", "linux-mint", "ubuntu-noble", "fedora-45", "fedora"] {
        assert!(profile_by_public_id(id).is_none(), "{id} must not be public");
    }
}

#[test]
fn ubuntu_profile_uses_deb_flavor_and_debian_version_scheme() {
    let profile = profile_by_public_id("ubuntu-26.04").expect("ubuntu profile");
    assert_eq!(profile.package_format(), ProfilePackageFormat::Deb);
    assert_eq!(profile.dependency_flavor(), RepositoryDependencyFlavor::Deb);
    assert_eq!(profile.version_scheme(), VersionScheme::Debian);
    assert_eq!(profile.replay_target_for_arch("x86_64").to_id(), "deb/ubuntu/26.04/x86_64");
}

#[test]
fn route_lookup_returns_route_metadata_and_matching_profile_ids() {
    let fedora = route_by_slug("fedora").expect("fedora route");
    assert_eq!(fedora.slug(), "fedora");
    assert_eq!(fedora.public_profile_ids(), &["fedora-44"]);

    let ubuntu = route_by_slug("ubuntu").expect("ubuntu route");
    assert_eq!(ubuntu.public_profile_ids(), &["ubuntu-26.04"]);

    let arch = route_by_slug("arch").expect("arch route");
    assert_eq!(arch.public_profile_ids(), &["arch"]);

    assert!(route_by_slug("debian").is_none());
}

#[test]
fn family_slug_lookup_does_not_accept_public_ids() {
    assert!(profile_by_family_slug("fedora-44").is_none());
    assert!(profile_by_family_slug("ubuntu-26.04").is_none());
    assert!(profile_by_family_slug("fedora").is_some());
    assert!(profile_by_family_slug("ubuntu").is_some());
    assert!(profile_by_family_slug("arch").is_some());
}

#[test]
fn repository_hints_are_profile_owned() {
    assert_eq!(
        profile_by_public_id("fedora-44").unwrap().repository_name_patterns(),
        &["fedora%"]
    );
    assert_eq!(
        profile_by_public_id("ubuntu-26.04").unwrap().repository_name_patterns(),
        &["ubuntu%"]
    );
    assert_eq!(
        profile_by_public_id("arch").unwrap().repository_name_patterns(),
        &["arch%"]
    );
}
```

- [ ] **Step 2: Run the failing profile tests**

Run:

```bash
cargo test -p conary-core supported_profiles
```

Expected: FAIL because `supported_profiles` does not exist yet.

- [ ] **Step 3: Add the embedded catalog**

Create `crates/conary-core/src/repository/supported_profiles/catalog.toml`:

```toml
[[profiles]]
id = "fedora-44"
display_name = "Fedora 44"
release = "44"
release_date = "2026-04-28"
eol = "2027-06-02"

[profiles.identity]
family_slug = "fedora"
remi_route_slug = "fedora"
package_format = "rpm"
dependency_flavor = "rpm"
version_scheme = "rpm"

[profiles.replay_target]
format = "rpm"
distro = "fedora"
release = "44"

[profiles.repository]
name_patterns = ["fedora%"]

[profiles.lifecycle]
service_manager = "systemd"
default_shell = "/bin/sh"
path_dirs = ["/usr/bin", "/bin"]

# M4d uses minimal fixture-backed allow-list entries to prove the policy
# mechanism. M4e replaces these placeholders with proof-corpus entries.
[profiles.lifecycle.services]
mode = "allow-list"
entries = ["example.service"]

[profiles.lifecycle.tmpfiles]
mode = "allow-list"
entries = ["example.conf"]

[profiles.lifecycle.sysctl]
mode = "allow-list"
keys = ["kernel.example"]

[profiles.lifecycle.users]
mode = "unsupported"

[profiles.lifecycle.groups]
mode = "unsupported"

[profiles.lifecycle.directories]
mode = "unsupported"

[profiles.lifecycle.alternatives]
mode = "unsupported"

[[profiles]]
id = "ubuntu-26.04"
display_name = "Ubuntu 26.04 LTS"
release = "26.04"
release_date = "2026-04-23"
eol = "2031-05-31"

[profiles.identity]
family_slug = "ubuntu"
remi_route_slug = "ubuntu"
package_format = "deb"
dependency_flavor = "deb"
version_scheme = "debian"

[profiles.replay_target]
format = "deb"
distro = "ubuntu"
release = "26.04"

[profiles.repository]
name_patterns = ["ubuntu%"]

[profiles.lifecycle]
service_manager = "systemd"
default_shell = "/bin/sh"
path_dirs = ["/usr/bin", "/bin"]

# M4d uses minimal fixture-backed allow-list entries to prove the policy
# mechanism. M4e replaces these placeholders with proof-corpus entries.
[profiles.lifecycle.services]
mode = "allow-list"
entries = ["example.service"]

[profiles.lifecycle.tmpfiles]
mode = "allow-list"
entries = ["example.conf"]

[profiles.lifecycle.sysctl]
mode = "allow-list"
keys = ["kernel.example"]

[profiles.lifecycle.users]
mode = "unsupported"

[profiles.lifecycle.groups]
mode = "unsupported"

[profiles.lifecycle.directories]
mode = "unsupported"

[profiles.lifecycle.alternatives]
mode = "unsupported"

[[profiles]]
id = "arch"
display_name = "Arch Linux (rolling)"
release = "rolling"

[profiles.identity]
family_slug = "arch"
remi_route_slug = "arch"
package_format = "arch"
dependency_flavor = "arch"
version_scheme = "arch"

[profiles.replay_target]
format = "arch"
distro = "arch"
release = "rolling"

[profiles.repository]
name_patterns = ["arch%"]

[profiles.lifecycle]
service_manager = "systemd"
default_shell = "/bin/sh"
path_dirs = ["/usr/bin", "/bin"]

# M4d uses minimal fixture-backed allow-list entries to prove the policy
# mechanism. M4e replaces these placeholders with proof-corpus entries.
[profiles.lifecycle.services]
mode = "allow-list"
entries = ["example.service"]

[profiles.lifecycle.tmpfiles]
mode = "allow-list"
entries = ["example.conf"]

[profiles.lifecycle.sysctl]
mode = "allow-list"
keys = ["kernel.example"]

[profiles.lifecycle.users]
mode = "unsupported"

[profiles.lifecycle.groups]
mode = "unsupported"

[profiles.lifecycle.directories]
mode = "unsupported"

[profiles.lifecycle.alternatives]
mode = "unsupported"
```

- [ ] **Step 4: Add typed profile structs and value enums**

Create `crates/conary-core/src/repository/supported_profiles/types.rs` with:

```rust
// conary-core/src/repository/supported_profiles/types.rs

use serde::Deserialize;

use crate::repository::dependency_model::RepositoryDependencyFlavor;
use crate::repository::distro::ReplayTargetOwned;
use crate::repository::versioning::VersionScheme;

#[derive(Debug, Clone, Deserialize)]
pub(super) struct CatalogDocument {
    pub profiles: Vec<ProfileDocument>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct ProfileDocument {
    pub id: String,
    pub display_name: String,
    pub release: String,
    #[serde(default)]
    pub release_date: Option<String>,
    #[serde(default)]
    pub eol: Option<String>,
    pub identity: ProfileIdentityDocument,
    pub replay_target: ReplayTargetDocument,
    pub repository: RepositoryHintsDocument,
    pub lifecycle: LifecycleDocument,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct ProfileIdentityDocument {
    pub family_slug: String,
    pub remi_route_slug: String,
    pub package_format: ProfilePackageFormat,
    pub dependency_flavor: DependencyFlavorValue,
    pub version_scheme: VersionSchemeValue,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) struct ReplayTargetDocument {
    pub format: ReplayFormat,
    pub distro: String,
    pub release: String,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct RepositoryHintsDocument {
    pub name_patterns: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct LifecycleDocument {
    pub service_manager: String,
    pub default_shell: String,
    #[serde(default)]
    pub path_dirs: Vec<String>,
    pub services: LifecyclePolicyDocument,
    pub tmpfiles: LifecyclePolicyDocument,
    pub sysctl: LifecyclePolicyDocument,
    pub users: LifecyclePolicyDocument,
    pub groups: LifecyclePolicyDocument,
    pub directories: LifecyclePolicyDocument,
    pub alternatives: LifecyclePolicyDocument,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) struct LifecyclePolicyDocument {
    pub mode: LifecyclePolicyMode,
    #[serde(default)]
    pub entries: Vec<String>,
    #[serde(default)]
    pub keys: Vec<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ProfilePackageFormat {
    Rpm,
    Deb,
    Arch,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(super) enum DependencyFlavorValue {
    Rpm,
    Deb,
    Arch,
}

impl From<DependencyFlavorValue> for RepositoryDependencyFlavor {
    fn from(value: DependencyFlavorValue) -> Self {
        match value {
            DependencyFlavorValue::Rpm => RepositoryDependencyFlavor::Rpm,
            DependencyFlavorValue::Deb => RepositoryDependencyFlavor::Deb,
            DependencyFlavorValue::Arch => RepositoryDependencyFlavor::Arch,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(super) enum VersionSchemeValue {
    Rpm,
    Debian,
    Arch,
}

impl From<VersionSchemeValue> for VersionScheme {
    fn from(value: VersionSchemeValue) -> Self {
        match value {
            VersionSchemeValue::Rpm => VersionScheme::Rpm,
            VersionSchemeValue::Debian => VersionScheme::Debian,
            VersionSchemeValue::Arch => VersionScheme::Arch,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(super) enum ReplayFormat {
    Rpm,
    Deb,
    Arch,
}

impl ReplayFormat {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            ReplayFormat::Rpm => "rpm",
            ReplayFormat::Deb => "deb",
            ReplayFormat::Arch => "arch",
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum LifecyclePolicyMode {
    AllowList,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportedProfile {
    document: ProfileDocument,
}

impl SupportedProfile {
    pub(super) fn new(document: ProfileDocument) -> Self {
        Self { document }
    }

    pub fn id(&self) -> &str {
        &self.document.id
    }

    pub fn display_name(&self) -> &str {
        &self.document.display_name
    }

    pub fn family_slug(&self) -> &str {
        &self.document.identity.family_slug
    }

    pub fn remi_route_slug(&self) -> &str {
        &self.document.identity.remi_route_slug
    }

    pub fn package_format(&self) -> ProfilePackageFormat {
        self.document.identity.package_format
    }

    pub fn dependency_flavor(&self) -> RepositoryDependencyFlavor {
        self.document.identity.dependency_flavor.into()
    }

    pub fn version_scheme(&self) -> VersionScheme {
        self.document.identity.version_scheme.into()
    }

    pub fn repository_name_patterns(&self) -> &[String] {
        &self.document.repository.name_patterns
    }

    pub fn replay_target_for_arch(&self, arch: &str) -> ReplayTargetOwned {
        ReplayTargetOwned {
            format: self.document.replay_target.format.as_str().to_string(),
            distro: self.document.replay_target.distro.clone(),
            release: self.document.replay_target.release.clone(),
            arch: arch.trim().to_string(),
        }
    }

    pub(super) fn lifecycle(&self) -> &LifecycleDocument {
        &self.document.lifecycle
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportedRoute {
    slug: String,
    public_profile_ids: Vec<String>,
}

impl SupportedRoute {
    pub(super) fn new(slug: String, public_profile_ids: Vec<String>) -> Self {
        Self {
            slug,
            public_profile_ids,
        }
    }

    pub fn slug(&self) -> &str {
        &self.slug
    }

    pub fn public_profile_ids(&self) -> &[String] {
        &self.public_profile_ids
    }
}
```

If `PartialEq` cannot derive because of nested private structs, add the missing derives to private document types. Do not make document structs public unless a caller needs them.

- [ ] **Step 5: Add the profile API with embedded parsing**

Create `crates/conary-core/src/repository/supported_profiles/mod.rs`:

```rust
// conary-core/src/repository/supported_profiles/mod.rs

mod lifecycle;
#[cfg(test)]
mod tests;
mod types;

use std::sync::LazyLock;

use crate::repository::dependency_model::RepositoryDependencyFlavor;
use crate::repository::distro::ReplayTargetOwned;
use crate::repository::versioning::VersionScheme;

pub use types::{LifecyclePolicyMode, ProfilePackageFormat, SupportedProfile, SupportedRoute};

use types::{CatalogDocument, SupportedProfile as Profile};

const CATALOG_TOML: &str = include_str!("catalog.toml");

static PUBLIC_PROFILES: LazyLock<Vec<SupportedProfile>> = LazyLock::new(|| {
    let parsed: CatalogDocument =
        toml::from_str(CATALOG_TOML).expect("embedded supported profile catalog must parse");
    validate_catalog(parsed.profiles)
});

fn validate_catalog(profiles: Vec<types::ProfileDocument>) -> Vec<SupportedProfile> {
    let supported = profiles
        .into_iter()
        .map(Profile::new)
        .collect::<Vec<SupportedProfile>>();

    let ids = supported.iter().map(SupportedProfile::id).collect::<Vec<_>>();
    assert_eq!(ids, ["fedora-44", "ubuntu-26.04", "arch"]);

    for profile in &supported {
        assert!(!profile.id().trim().is_empty(), "profile id must not be empty");
        assert!(
            !profile.remi_route_slug().trim().is_empty(),
            "profile route slug must not be empty"
        );
        assert!(
            !profile.repository_name_patterns().is_empty(),
            "profile must include repository hints"
        );
    }

    supported
}

pub fn public_profiles() -> &'static [SupportedProfile] {
    PUBLIC_PROFILES.as_slice()
}

pub fn profile_by_public_id(id: &str) -> Option<&'static SupportedProfile> {
    let id = id.trim();
    public_profiles().iter().find(|profile| profile.id() == id)
}

pub fn profile_by_family_slug(slug: &str) -> Option<&'static SupportedProfile> {
    let slug = slug.trim();
    public_profiles()
        .iter()
        .find(|profile| profile.family_slug() == slug)
}

pub fn route_by_slug(slug: &str) -> Option<SupportedRoute> {
    let slug = slug.trim();
    let public_profile_ids = public_profiles()
        .iter()
        .filter(|profile| profile.remi_route_slug() == slug)
        .map(|profile| profile.id().to_string())
        .collect::<Vec<_>>();
    if public_profile_ids.is_empty() {
        None
    } else {
        Some(SupportedRoute::new(slug.to_string(), public_profile_ids))
    }
}

pub fn dependency_flavor_for_name(name: &str) -> Option<RepositoryDependencyFlavor> {
    profile_by_public_id(name)
        .or_else(|| profile_by_family_slug(name))
        .map(SupportedProfile::dependency_flavor)
}

pub fn version_scheme_for_name(name: &str) -> Option<VersionScheme> {
    profile_by_public_id(name)
        .or_else(|| profile_by_family_slug(name))
        .map(SupportedProfile::version_scheme)
}

pub fn replay_target_for_public_id(id: &str, arch: &str) -> Option<ReplayTargetOwned> {
    if arch.trim().is_empty() {
        return None;
    }
    profile_by_public_id(id).map(|profile| profile.replay_target_for_arch(arch))
}
```

Add the module export in `crates/conary-core/src/repository/mod.rs`:

```rust
pub mod supported_profiles;
```

- [ ] **Step 6: Add temporary distro compatibility shims**

In `crates/conary-core/src/repository/distro.rs`, replace hard-coded public catalog constants and mapping bodies with profile-backed shims. Keep the existing `SupportedDistro`, `ReplayTarget`, and `ReplayTargetOwned` public types for call-site churn control.

Use this shape:

```rust
pub fn supported_user_distros() -> Vec<SupportedDistro> {
    crate::repository::supported_profiles::public_profiles()
        .iter()
        .map(|profile| SupportedDistro {
            id: profile.id().to_string(),
            display_name: profile.display_name().to_string(),
        })
        .collect()
}

pub fn supported_distro(id: &str) -> Option<SupportedDistro> {
    crate::repository::supported_profiles::profile_by_public_id(id).map(|profile| {
        SupportedDistro {
            id: profile.id().to_string(),
            display_name: profile.display_name().to_string(),
        }
    })
}

pub fn flavor_from_distro_name(name: &str) -> Option<RepositoryDependencyFlavor> {
    crate::repository::supported_profiles::dependency_flavor_for_name(name)
}

pub fn version_scheme_from_distro_name(name: &str) -> Option<VersionScheme> {
    crate::repository::supported_profiles::version_scheme_for_name(name)
}

pub fn replay_target_from_distro_id(distro_id: &str, arch: &str) -> Option<ReplayTargetOwned> {
    crate::repository::supported_profiles::replay_target_for_public_id(distro_id, arch)
}
```

Adjust `SupportedDistro` fields from `&'static str` to `String` if needed. Then update callers that format `distro.id` and `distro.display_name` to borrow as strings.

- [ ] **Step 7: Run catalog tests**

Run:

```bash
cargo test -p conary-core supported_profiles
```

Expected: PASS. If shims break existing `repository::distro` tests, update those tests in the same task to assert the new hard-cutover behavior: `debian` and `debian-13` are not replay targets, generic `fedora` and `ubuntu` are not public replay pins, and route slugs are internal lookup facts.

- [ ] **Step 8: Delete the old duplicate distro catalog**

Remove `data/distros.toml` in the same task that adds `crates/conary-core/src/repository/supported_profiles/catalog.toml`:

```bash
git rm data/distros.toml
rg -n "data/distros\\.toml|supported-target-profiles\\.toml|distros\\.toml" crates apps docs scripts
```

Expected: no runtime readers remain. Documentation references may remain only when they explain the historical hard cutover or say the file was deleted.

- [ ] **Step 9: Commit Task 1**

```bash
git add data/distros.toml \
    crates/conary-core/src/repository/mod.rs \
    crates/conary-core/src/repository/distro.rs \
    crates/conary-core/src/repository/supported_profiles
git commit -m "feat(repository): add supported target profiles"
```

---

### Task 2: Cut CLI, Source Policy, And Replay Callers Over To Profiles

**Files:**
- Modify: `apps/conary/src/commands/distro.rs`
- Modify: `apps/conary/src/commands/install/source_policy.rs`
- Modify: `apps/conary/src/commands/update/source_policy.rs`
- Modify: `apps/conary/src/commands/install/legacy_replay.rs`
- Modify: other callers found by `rg "repository::distro|flavor_from_distro_name|replay_target_from_distro_id|supported_user_distros"`
- Create: `apps/conary/tests/packaging_m4d.rs`
- Test: `cargo test -p conary --lib commands::distro`
- Test: `cargo test -p conary --lib commands::install::source_policy`
- Test: `cargo test -p conary --test packaging_m4d`

- [ ] **Step 1: Write failing `distro set` tests**

In `apps/conary/src/commands/distro.rs`, replace the compatibility-pin test with:

```rust
#[tokio::test]
async fn test_cmd_distro_set_persists_supported_public_pin() {
    let (_temp, db_path, conn) = create_test_db();

    cmd_distro_set(&db_path, "arch", "strict").await.unwrap();

    let pin = DistroPin::get_current(&conn).unwrap().unwrap();
    let source_pin = pin.as_source_pin();
    assert_eq!(source_pin.distro, "arch");
    assert_eq!(source_pin.strength.as_deref(), Some("strict"));
}

#[tokio::test]
async fn test_cmd_distro_set_rejects_unsupported_public_id() {
    let (_temp, db_path, conn) = create_test_db();

    let err = cmd_distro_set(&db_path, "debian-13", "strict")
        .await
        .unwrap_err();

    assert!(err.to_string().contains("Unsupported distro"));
    assert!(DistroPin::get_current(&conn).unwrap().is_none());
}

#[tokio::test]
async fn test_cmd_distro_set_rejects_internal_only_route_slug() {
    let (_temp, db_path, conn) = create_test_db();

    let err = cmd_distro_set(&db_path, "fedora", "strict")
        .await
        .unwrap_err();

    assert!(err.to_string().contains("Unsupported distro"));
    assert!(DistroPin::get_current(&conn).unwrap().is_none());
}
```

Keep `arch` accepted because it is an explicit public profile ID.

- [ ] **Step 2: Run CLI tests and verify failure**

Run:

```bash
cargo test -p conary --lib commands::distro
```

Expected: FAIL because `cmd_distro_set` still stores arbitrary strings.

- [ ] **Step 3: Validate `distro set` through profiles**

Update `cmd_distro_set`:

```rust
pub async fn cmd_distro_set(db_path: &str, distro: &str, mixing: &str) -> Result<()> {
    validate_mixing_policy(mixing)?;
    let profile = conary_core::repository::supported_profiles::profile_by_public_id(distro)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Unsupported distro: {distro}. Use 'conary distro list' to see supported targets."
            )
        })?;
    let conn = open_db(db_path)?;
    DistroPin::set_from_source_pin(
        &conn,
        &SourcePinConfig {
            distro: profile.id().to_string(),
            strength: Some(mixing.to_string()),
        },
    )?;
    println!("Pinned to {} (mixing: {mixing})", profile.id());
    Ok(())
}
```

Update `render_distro_list_for_repos` for `supported_user_distros()` returning owned values if Task 1 made that compatibility shim owned:

```rust
for distro in supported_user_distros() {
    let matching_repos: Vec<_> = repos
        .iter()
        .filter(|repo| {
            repo.name == distro.id || repo.default_strategy_distro.as_deref() == Some(distro.id.as_str())
        })
        .collect();
    output.push_str(&format!(
        "  {:<15} {:<24} {}\n",
        distro.id, distro.display_name, status
    ));
}
```

- [ ] **Step 4: Write source-policy tests for route and public ID handling**

In `apps/conary/src/commands/install/source_policy.rs`, extend tests:

```rust
#[test]
fn distro_name_to_flavor_accepts_public_ids_and_route_slugs() {
    assert_eq!(
        distro_name_to_flavor("fedora-44"),
        Some(RepositoryDependencyFlavor::Rpm)
    );
    assert_eq!(
        distro_name_to_flavor("fedora"),
        Some(RepositoryDependencyFlavor::Rpm)
    );
    assert_eq!(
        distro_name_to_flavor("ubuntu-26.04"),
        Some(RepositoryDependencyFlavor::Deb)
    );
    assert_eq!(
        distro_name_to_flavor("ubuntu"),
        Some(RepositoryDependencyFlavor::Deb)
    );
    assert_eq!(
        distro_name_to_flavor("arch"),
        Some(RepositoryDependencyFlavor::Arch)
    );
}

#[test]
fn distro_name_to_flavor_rejects_unsupported_derivatives() {
    for name in ["debian", "debian-13", "linux-mint", "ubuntu-noble", "fedora-45"] {
        assert_eq!(distro_name_to_flavor(name), None, "{name}");
    }
}
```

- [ ] **Step 5: Update source-policy helpers to use profiles**

Keep `distro_name_to_flavor` as a tiny wrapper:

```rust
fn distro_name_to_flavor(distro: &str) -> Option<RepositoryDependencyFlavor> {
    conary_core::repository::supported_profiles::dependency_flavor_for_name(distro)
}
```

Repeat the same profile-backed replacement for update source-policy helpers discovered by:

```bash
rg "flavor_from_distro_name|version_scheme_from_distro_name|repository::distro" apps/conary/src/commands/update apps/conary/src/commands/install crates/conary-core/src
```

- [ ] **Step 6: Write replay hard-cutover tests**

In `crates/conary-core/src/repository/distro.rs`, update replay tests:

```rust
#[test]
fn replay_target_only_accepts_public_profile_ids() {
    assert_eq!(
        replay_target_from_distro_id("fedora-44", "x86_64")
            .expect("fedora")
            .to_id(),
        "rpm/fedora/44/x86_64"
    );
    assert_eq!(
        replay_target_from_distro_id("ubuntu-26.04", "x86_64")
            .expect("ubuntu")
            .to_id(),
        "deb/ubuntu/26.04/x86_64"
    );
    assert_eq!(
        replay_target_from_distro_id("arch", "x86_64")
            .expect("arch")
            .to_id(),
        "arch/arch/rolling/x86_64"
    );
}

#[test]
fn replay_target_rejects_non_public_legacy_normalization() {
    for name in ["fedora", "ubuntu", "debian", "debian-13", "linux-mint"] {
        assert_eq!(replay_target_from_distro_id(name, "x86_64"), None, "{name}");
    }
}
```

Audit legacy replay callers and tests before implementing any private shim:

```bash
rg -n '"fedora"|"ubuntu"|debian|replay_target_from_distro_id' crates/conary-core/src/ccs/legacy_replay.rs apps/conary/src/commands/legacy_replay_policy.rs
```

Update public profile pin tests in those files to `fedora-44` and `ubuntu-26.04`. Keep source-family tests that intentionally model bundle source metadata, but do not let generic `fedora`, generic `ubuntu`, or `debian` survive as public replay pins.

If a current install replay fixture fails because it truly needs non-public normalization, implement a private helper named `legacy_replay_target_from_non_public_source` in the owning replay module, add a leak-guard test proving it is not used by `supported_profiles`, `conary distro list`, Remi help, or route validation, and document the shim in the task summary.

- [ ] **Step 7: Add an integration smoke test**

Create `apps/conary/tests/packaging_m4d.rs`:

```rust
mod common;

use std::process::Command;

#[test]
fn packaging_m4d_distro_list_exposes_only_supported_profiles() {
    let (_db_temp, db_path) = common::setup_command_test_db();
    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(["distro", "list", "--db-path", &db_path])
        .output()
        .expect("run conary distro list");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    assert!(stdout.contains("fedora-44"));
    assert!(stdout.contains("ubuntu-26.04"));
    assert!(stdout.contains("arch"));
    assert!(!stdout.contains("debian"));
    assert!(!stdout.contains("linux-mint"));
}

#[test]
fn packaging_m4d_distro_set_rejects_unsupported_target() {
    let (_db_temp, db_path) = common::setup_command_test_db();
    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(["distro", "set", "debian-13", "--db-path", &db_path])
        .output()
        .expect("run conary distro set");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Unsupported distro"));
}
```

- [ ] **Step 8: Run Task 2 tests**

Run:

```bash
cargo test -p conary-core repository::distro
cargo test -p conary --lib commands::distro
cargo test -p conary --lib commands::install::source_policy
cargo test -p conary --test packaging_m4d
```

Expected: PASS.

- [ ] **Step 9: Commit Task 2**

```bash
git add crates/conary-core/src/repository/distro.rs \
    apps/conary/src/commands/distro.rs \
    apps/conary/src/commands/install/source_policy.rs \
    apps/conary/src/commands/update \
    apps/conary/src/commands/install/legacy_replay.rs \
    apps/conary/tests/packaging_m4d.rs
git commit -m "feat(cli): validate distro pins with supported profiles"
```

---

### Task 3: Extend CCS v2 Lifecycle Validation Through Profiles

**Files:**
- Modify: `crates/conary-core/src/ccs/v2/validation.rs`
- Modify: `crates/conary-core/src/repository/supported_profiles/lifecycle.rs`
- Modify: `crates/conary-core/src/repository/supported_profiles/mod.rs`
- Test: `cargo test -p conary-core ccs::v2::validation`
- Test: `cargo test -p conary-core supported_profiles::tests::profile_backed_lifecycle_query`

- [ ] **Step 1: Write failing full-vector lifecycle tests**

In `crates/conary-core/src/ccs/v2/validation.rs`, add tests near the existing validation tests:

```rust
#[derive(Debug, Clone, Copy)]
struct AcceptOnlyNamedService;

impl TargetProfileQuery for AcceptOnlyNamedService {
    fn service_status(&self, service: &str) -> ProfileConstraintStatus {
        if service == "allowed.service" {
            ProfileConstraintStatus::Accepted
        } else {
            ProfileConstraintStatus::Unsupported
        }
    }

    fn tmpfiles_status(&self, _entry: &str) -> ProfileConstraintStatus {
        ProfileConstraintStatus::Unsupported
    }

    fn sysctl_status(&self, _key: &str) -> ProfileConstraintStatus {
        ProfileConstraintStatus::Unsupported
    }

    fn user_status(&self, _user: &str) -> ProfileConstraintStatus {
        ProfileConstraintStatus::Unsupported
    }

    fn group_status(&self, _group: &str) -> ProfileConstraintStatus {
        ProfileConstraintStatus::Unsupported
    }

    fn directory_status(&self, _directory: &str) -> ProfileConstraintStatus {
        ProfileConstraintStatus::Unsupported
    }

    fn alternative_status(&self, _alternative: &str) -> ProfileConstraintStatus {
        ProfileConstraintStatus::Unsupported
    }
}

#[test]
fn target_profile_rejects_all_signed_lifecycle_vectors() {
    let mut authority = test_package_authority("lifecycle-target");
    authority.lifecycle.services = vec!["blocked.service".to_string()];
    authority.lifecycle.tmpfiles = vec!["blocked.conf".to_string()];
    authority.lifecycle.sysctl = vec!["kernel.blocked".to_string()];
    authority.lifecycle.users = vec!["blocked-user".to_string()];
    authority.lifecycle.groups = vec!["blocked-group".to_string()];
    authority.lifecycle.directories = vec!["/var/lib/blocked".to_string()];
    authority.lifecycle.alternatives = vec!["blocked-alternative".to_string()];

    let err = validate_authority_with_profile(&authority, &AcceptOnlyNamedService).unwrap_err();
    let fields = err
        .diagnostics
        .iter()
        .filter_map(|diagnostic| diagnostic.field.as_deref())
        .collect::<Vec<_>>();

    assert!(fields.contains(&"lifecycle.services"));
    assert!(fields.contains(&"lifecycle.tmpfiles"));
    assert!(fields.contains(&"lifecycle.sysctl"));
    assert!(fields.contains(&"lifecycle.users"));
    assert!(fields.contains(&"lifecycle.groups"));
    assert!(fields.contains(&"lifecycle.directories"));
    assert!(fields.contains(&"lifecycle.alternatives"));
}
```

Use the existing test authority helper name if it differs from `test_package_authority`.

- [ ] **Step 2: Run lifecycle tests and verify failure**

Run:

```bash
cargo test -p conary-core ccs::v2::validation::target_profile_rejects_all_signed_lifecycle_vectors
```

Expected: FAIL because `TargetProfileQuery` lacks user/group/directory/alternative methods and validation loops.

- [ ] **Step 3: Extend `TargetProfileQuery` and default fail-closed implementation**

Update `crates/conary-core/src/ccs/v2/validation.rs`:

```rust
pub trait TargetProfileQuery {
    fn service_status(&self, service: &str) -> ProfileConstraintStatus;
    fn tmpfiles_status(&self, entry: &str) -> ProfileConstraintStatus;
    fn sysctl_status(&self, key: &str) -> ProfileConstraintStatus;
    fn user_status(&self, user: &str) -> ProfileConstraintStatus;
    fn group_status(&self, group: &str) -> ProfileConstraintStatus;
    fn directory_status(&self, directory: &str) -> ProfileConstraintStatus;
    fn alternative_status(&self, alternative: &str) -> ProfileConstraintStatus;
}

impl TargetProfileQuery for M4aNoProfileFacts {
    fn service_status(&self, _service: &str) -> ProfileConstraintStatus {
        ProfileConstraintStatus::Unsupported
    }

    fn tmpfiles_status(&self, _entry: &str) -> ProfileConstraintStatus {
        ProfileConstraintStatus::Unsupported
    }

    fn sysctl_status(&self, _key: &str) -> ProfileConstraintStatus {
        ProfileConstraintStatus::Unsupported
    }

    fn user_status(&self, _user: &str) -> ProfileConstraintStatus {
        ProfileConstraintStatus::Unsupported
    }

    fn group_status(&self, _group: &str) -> ProfileConstraintStatus {
        ProfileConstraintStatus::Unsupported
    }

    fn directory_status(&self, _directory: &str) -> ProfileConstraintStatus {
        ProfileConstraintStatus::Unsupported
    }

    fn alternative_status(&self, _alternative: &str) -> ProfileConstraintStatus {
        ProfileConstraintStatus::Unsupported
    }
}
```

- [ ] **Step 4: Add validation loops for every lifecycle vector**

In `validate_authority_with_profile`, add loops after existing service/tmpfiles/sysctl loops:

```rust
for user in &authority.lifecycle.users {
    if profile.user_status(user) == ProfileConstraintStatus::Unsupported {
        diagnostics.push(V2Diagnostic::error(
            V2DiagnosticCode::LifecycleUnsupported,
            format!("user {user} is not supported by the target profile"),
            Some("lifecycle.users".to_string()),
            "remove the user declaration or choose a target profile that supports it",
        ));
    }
}
for group in &authority.lifecycle.groups {
    if profile.group_status(group) == ProfileConstraintStatus::Unsupported {
        diagnostics.push(V2Diagnostic::error(
            V2DiagnosticCode::LifecycleUnsupported,
            format!("group {group} is not supported by the target profile"),
            Some("lifecycle.groups".to_string()),
            "remove the group declaration or choose a target profile that supports it",
        ));
    }
}
for directory in &authority.lifecycle.directories {
    if profile.directory_status(directory) == ProfileConstraintStatus::Unsupported {
        diagnostics.push(V2Diagnostic::error(
            V2DiagnosticCode::LifecycleUnsupported,
            format!("directory {directory} is not supported by the target profile"),
            Some("lifecycle.directories".to_string()),
            "remove the directory declaration or choose a target profile that supports it",
        ));
    }
}
for alternative in &authority.lifecycle.alternatives {
    if profile.alternative_status(alternative) == ProfileConstraintStatus::Unsupported {
        diagnostics.push(V2Diagnostic::error(
            V2DiagnosticCode::LifecycleUnsupported,
            format!("alternative {alternative} is not supported by the target profile"),
            Some("lifecycle.alternatives".to_string()),
            "remove the alternative declaration or choose a target profile that supports it",
        ));
    }
}
```

Update existing service/tmpfiles/sysctl diagnostics to use the same target-profile wording instead of any stale future-support placeholder text. The suggestion for each unsupported lifecycle vector must be:

```rust
format!("remove the {kind} declaration or choose a target profile that supports it")
```

Use the concrete kind strings `service`, `tmpfiles`, `sysctl`, `user`, `group`, `directory`, and `alternative` so diagnostics stay actionable and do not imply a future compatibility path.

- [ ] **Step 5: Implement profile-backed lifecycle policy**

Create `crates/conary-core/src/repository/supported_profiles/lifecycle.rs`:

```rust
// conary-core/src/repository/supported_profiles/lifecycle.rs

use crate::ccs::v2::validation::{ProfileConstraintStatus, TargetProfileQuery};

use super::types::{LifecyclePolicyDocument, LifecyclePolicyMode, SupportedProfile};

fn match_policy(policy: &LifecyclePolicyDocument, value: &str, use_keys: bool) -> ProfileConstraintStatus {
    match policy.mode {
        LifecyclePolicyMode::Unsupported => ProfileConstraintStatus::Unsupported,
        LifecyclePolicyMode::AllowList => {
            let values = if use_keys { &policy.keys } else { &policy.entries };
            if values.iter().any(|entry| entry == value) {
                ProfileConstraintStatus::Accepted
            } else {
                ProfileConstraintStatus::Unsupported
            }
        }
    }
}

impl TargetProfileQuery for SupportedProfile {
    fn service_status(&self, service: &str) -> ProfileConstraintStatus {
        match_policy(&self.lifecycle().services, service, false)
    }

    fn tmpfiles_status(&self, entry: &str) -> ProfileConstraintStatus {
        match_policy(&self.lifecycle().tmpfiles, entry, false)
    }

    fn sysctl_status(&self, key: &str) -> ProfileConstraintStatus {
        match_policy(&self.lifecycle().sysctl, key, true)
    }

    fn user_status(&self, user: &str) -> ProfileConstraintStatus {
        match_policy(&self.lifecycle().users, user, false)
    }

    fn group_status(&self, group: &str) -> ProfileConstraintStatus {
        match_policy(&self.lifecycle().groups, group, false)
    }

    fn directory_status(&self, directory: &str) -> ProfileConstraintStatus {
        match_policy(&self.lifecycle().directories, directory, false)
    }

    fn alternative_status(&self, alternative: &str) -> ProfileConstraintStatus {
        match_policy(&self.lifecycle().alternatives, alternative, false)
    }
}
```

- [ ] **Step 6: Add profile lifecycle tests**

In `crates/conary-core/src/repository/supported_profiles/tests.rs`, add:

```rust
#[test]
fn profile_backed_lifecycle_query_accepts_only_explicit_entries() {
    use crate::ccs::v2::validation::{ProfileConstraintStatus, TargetProfileQuery};

    let profile = profile_by_public_id("fedora-44").unwrap();

    assert_eq!(
        profile.service_status("example.service"),
        ProfileConstraintStatus::Accepted
    );
    assert_eq!(
        profile.service_status("anything.service"),
        ProfileConstraintStatus::Unsupported
    );
    assert_eq!(
        profile.tmpfiles_status("example.conf"),
        ProfileConstraintStatus::Accepted
    );
    assert_eq!(
        profile.sysctl_status("kernel.example"),
        ProfileConstraintStatus::Accepted
    );
    assert_eq!(
        profile.user_status("example"),
        ProfileConstraintStatus::Unsupported
    );
    assert_eq!(
        profile.alternative_status("editor"),
        ProfileConstraintStatus::Unsupported
    );
}
```

- [ ] **Step 7: Run Task 3 tests**

Run:

```bash
cargo test -p conary-core ccs::v2::validation
cargo test -p conary-core supported_profiles::tests::profile_backed_lifecycle_query
```

Expected: PASS.

- [ ] **Step 8: Commit Task 3**

```bash
git add crates/conary-core/src/ccs/v2/validation.rs \
    crates/conary-core/src/repository/supported_profiles
git commit -m "feat(ccs): validate lifecycle authority with supported profiles"
```

---

### Task 4: Cut Remi Route, Conversion, Native Publish, And Sync Logic Over To Profiles

**Files:**
- Modify: `apps/remi/src/server/handlers/mod.rs`
- Modify: `apps/remi/src/server/handlers/index.rs`
- Modify: `apps/remi/src/server/handlers/packages.rs`
- Modify: `apps/remi/src/server/handlers/sparse.rs`
- Modify: `apps/remi/src/server/handlers/tuf.rs`
- Modify: `apps/remi/src/server/handlers/detail.rs`
- Modify: `apps/remi/src/server/handlers/admin/mod.rs`
- Modify: `apps/remi/src/server/handlers/admin/packages.rs`
- Modify: `apps/remi/src/server/native_publish/verify.rs`
- Modify: `apps/remi/src/server/conversion/lookup.rs`
- Modify: `apps/remi/src/server/conversion/metadata.rs`
- Modify: `crates/conary-core/src/repository/sync/remi.rs`
- Test: `cargo test -p remi route`
- Test: `cargo test -p remi conversion`
- Test: `cargo test -p remi release_upload_`
- Test: `cargo test -p conary-core remi_sync`

- [ ] **Step 1: Write route validation tests for unsupported slugs**

Add tests in the owning Remi handler test modules:

```rust
use std::path::PathBuf;
use std::sync::Arc;

use crate::server::auth::TokenScopes;
use crate::server::{ServerConfig, ServerState};
use tokio::sync::RwLock;

fn remi_empty_db_state() -> (tempfile::TempDir, PathBuf, Arc<RwLock<ServerState>>) {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("remi-test.db");
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conary_core::db::schema::migrate(&conn).unwrap();
    }

    let mut config = ServerConfig::default();
    config.db_path = db_path;
    config.chunk_dir = temp.path().join("chunks");
    config.cache_dir = temp.path().join("cache");
    let cache_dir = config.cache_dir.clone();
    std::fs::create_dir_all(&config.chunk_dir).unwrap();
    std::fs::create_dir_all(&config.cache_dir).unwrap();

    let state = Arc::new(RwLock::new(ServerState::new(config).unwrap()));
    (temp, cache_dir, state)
}

#[tokio::test]
async fn sparse_index_rejects_unsupported_distro_before_db_lookup() {
    let (_temp, _cache_dir, state) = remi_empty_db_state();
    let response = crate::server::handlers::sparse::list_packages(
        axum::extract::State(state),
        axum::extract::Path("debian".to_string()),
        axum::extract::Query(crate::server::handlers::sparse::ListQuery {
            page: None,
            per_page: None,
        }),
    )
    .await;

    assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn sparse_entry_rejects_unsupported_distro_before_db_lookup() {
    let (_temp, _cache_dir, state) = remi_empty_db_state();
    let response = crate::server::handlers::sparse::get_sparse_entry(
        axum::extract::State(state),
        axum::extract::Path(("debian".to_string(), "bash".to_string())),
    )
    .await;

    assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn tuf_metadata_rejects_unsupported_distro_before_db_lookup() {
    let (_temp, _cache_dir, state) = remi_empty_db_state();
    let response = crate::server::handlers::tuf::get_timestamp(
        axum::extract::State(state),
        axum::extract::Path("debian".to_string()),
    )
    .await;

    assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn tuf_refresh_rejects_unsupported_distro_before_key_config_lookup() {
    let (_temp, _cache_dir, state) = remi_empty_db_state();
    let response = crate::server::handlers::tuf::refresh_timestamp(
        axum::extract::State(state),
        axum::extract::Path("debian".to_string()),
    )
    .await;

    assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = String::from_utf8_lossy(&body);
    assert!(
        !body.contains("repository_keys_dir"),
        "route validation must happen before release_publish.repository_keys_dir lookup: {body}"
    );
}

#[tokio::test]
async fn admin_package_upload_rejects_unsupported_distro_before_cache_paths() {
    let (_temp, cache_dir, state) = remi_empty_db_state();
    let response = crate::server::handlers::admin::packages::upload_package(
        axum::extract::State(state),
        axum::extract::Path("debian".to_string()),
        Some(axum::Extension(TokenScopes("admin".to_string()))),
        axum::http::Request::builder()
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;

    assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
    assert!(
        !cache_dir.join("packages").join("debian").exists(),
        "unsupported distro route must not create cache/packages/debian"
    );
}
```

Use existing Remi test-state helpers if they already provide the same empty migrated DB and temp cache/key directories. Keep the five behavior assertions identical: unsupported route slug returns `400`, sparse and metadata paths do not need populated package rows, TUF refresh does not read release key configuration, and admin package upload does not create unsupported cache directories.

- [ ] **Step 2: Run route tests and verify failure**

Run:

```bash
cargo test -p remi unsupported_distro
```

Expected: FAIL for sparse, TUF, or admin upload paths that currently validate path syntax only.

- [ ] **Step 3: Add profile-backed route validation helpers**

Update `apps/remi/src/server/handlers/mod.rs`:

```rust
pub fn supported_route_slugs() -> Vec<String> {
    conary_core::repository::supported_profiles::public_profiles()
        .iter()
        .map(|profile| profile.remi_route_slug().to_string())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[allow(clippy::result_large_err)]
pub fn validate_supported_distro_route(distro: &str) -> Result<(), Response> {
    validate_name(distro)?;
    if conary_core::repository::supported_profiles::route_by_slug(distro).is_some() {
        Ok(())
    } else {
        Err((StatusCode::BAD_REQUEST, "Unknown distribution").into_response())
    }
}

#[allow(clippy::result_large_err)]
pub fn validate_distro_and_name(distro: &str, name: &str) -> Result<(), Response> {
    validate_supported_distro_route(distro)?;
    validate_name(name)?;
    Ok(())
}
```

Remove the local `SUPPORTED_DISTROS` constant after every caller has moved. If a temporary constant is required by tests in the same commit, derive it from `supported_route_slugs()` rather than hard-coding route strings.

In `apps/remi/src/server/handlers/admin/mod.rs`, add an admin-specific JSON wrapper that can be called from `admin/mod.rs` directly and from `admin/packages.rs` through `super::`:

```rust
pub(crate) fn validate_supported_admin_distro_route(distro: &str) -> Option<Response> {
    if let Some(err) = validate_path_param(distro, "distro") {
        return Some(err);
    }
    if conary_core::repository::supported_profiles::route_by_slug(distro).is_none() {
        return Some(json_error(400, "Unknown distribution", "UNKNOWN_DISTRIBUTION"));
    }
    None
}
```

- [ ] **Step 4: Call route validation before DB/filesystem/key/trust work**

Patch every `{distro}` route handler named in the M4d design:

```rust
if let Err(e) = super::validate_supported_distro_route(&distro) {
    return e;
}
```

For `(distro, name)` or `(distro, package)` routes, use:

```rust
if let Err(e) = super::validate_distro_and_name(&distro, &name) {
    return e;
}
```

For admin handlers under `apps/remi/src/server/handlers/admin/`, use the JSON wrapper:

```rust
if let Some(err) = validate_supported_admin_distro_route(&distro) {
    return err;
}
```

Inside `apps/remi/src/server/handlers/admin/packages.rs`, call it as:

```rust
if let Some(err) = super::validate_supported_admin_distro_route(&distro) {
    return err;
}
```

For admin upload and TUF refresh routes, place supported-route validation after auth/scope checks but before cache-path creation, DB queries, key-path access, or release publish verification. TUF refresh lives in `apps/remi/src/server/handlers/tuf.rs`, so it should call the root helper, not the admin wrapper.

Patch these concrete functions and keep validation as the first distro-related operation after path extraction, or after auth/scope checks for admin routes:

- `apps/remi/src/server/handlers/index.rs`: `get_metadata`, `get_metadata_sig`
- `apps/remi/src/server/handlers/packages.rs`: `get_package`, `download_package`, `get_delta`
- `apps/remi/src/server/handlers/sparse.rs`: `get_sparse_entry`, `list_packages`
- `apps/remi/src/server/handlers/detail.rs`: `get_package_detail`, `get_versions`, `get_dependencies`, `get_reverse_dependencies`
- `apps/remi/src/server/handlers/tuf.rs`: `get_timestamp`, `get_snapshot`, `get_targets`, `get_root`, `get_versioned_root`, `refresh_timestamp`
- `apps/remi/src/server/handlers/admin/mod.rs`: `upload_release_package`
- `apps/remi/src/server/handlers/admin/packages.rs`: `upload_package`, `get_scriptlet_review_artifact`

The supported-route check must happen before `state.read().await`, `tokio::task::spawn_blocking`, DB opens/queries, cache or chunk path construction, TUF key path reads, review-artifact lookup, and native publish-gate verification.

- [ ] **Step 5: Cut native release upload validation over to profiles**

Update `apps/remi/src/server/native_publish/verify.rs`:

```rust
pub(crate) fn validate_supported_release_distro(distro: &str) -> Result<(), NativePublishError> {
    if conary_core::repository::supported_profiles::route_by_slug(distro).is_some() {
        Ok(())
    } else {
        Err(NativePublishError::unprocessable(
            NativePublishErrorCode::UnsupportedDistro,
            format!("unsupported release distro {distro}"),
        ))
    }
}
```

Keep the existing `UnsupportedDistro` error code. Do not change accepted-signer checks, local-dev rejection, recorded-draft refusal, policy digest checks, or static publish-gate behavior.

- [ ] **Step 6: Update conversion lookup to use repository hints**

In `apps/remi/src/server/conversion/lookup.rs`, replace local flavor-to-pattern logic:

```rust
let route = conary_core::repository::supported_profiles::route_by_slug(distro)
    .ok_or_else(|| anyhow!("Unknown distribution: {}", distro))?;
let profile_id = route
    .public_profile_ids()
    .first()
    .ok_or_else(|| anyhow!("No public profile for route: {}", distro))?;
let profile = conary_core::repository::supported_profiles::profile_by_public_id(profile_id)
    .ok_or_else(|| anyhow!("Profile disappeared for route: {}", distro))?;
let repo_patterns = profile.repository_name_patterns();
let scheme = profile.version_scheme();
```

Change SQL building to use all `repo_patterns`. For M4d each route has one pattern, so a simple loop that tries each pattern in stable order is enough. Do not reintroduce local `fedora%`, `ubuntu%`, or `arch%` matches outside tests.

- [ ] **Step 7: Update parser dispatch to use profile package format**

In `apps/remi/src/server/conversion/metadata.rs`, replace route-string parser dispatch with:

```rust
let route = conary_core::repository::supported_profiles::route_by_slug(distro)
    .ok_or_else(|| anyhow!("Unsupported distribution: {}", distro))?;
let profile_id = route
    .public_profile_ids()
    .first()
    .ok_or_else(|| anyhow!("No public profile for route: {}", distro))?;
let profile = conary_core::repository::supported_profiles::profile_by_public_id(profile_id)
    .ok_or_else(|| anyhow!("Profile disappeared for route: {}", distro))?;

match profile.package_format() {
    ProfilePackageFormat::Arch => { /* existing Arch parser branch */ }
    ProfilePackageFormat::Rpm => { /* existing RPM parser branch */ }
    ProfilePackageFormat::Deb => { /* existing DEB parser branch */ }
}
```

Import `ProfilePackageFormat` from `conary_core::repository::supported_profiles`. Delete the `debian` parser route from public route dispatch. Only keep a Debian conversion branch if a private conversion-only test proves a non-route parser helper still needs it; if retained, the helper must be unreachable from public Remi routes, and the test name must state `private_debian_parser_is_not_a_supported_route`.

- [ ] **Step 8: Update Remi sync route version scheme derivation**

In `crates/conary-core/src/repository/sync/remi.rs`, change `remi_sync_row` to return `Result<SyncedPackageRow>` so unsupported route names can fail closed instead of silently falling back:

```rust
pub(super) fn remi_sync_row(
    repo_id: i64,
    endpoint: String,
    distro: String,
    entry: RemiPackageEntry,
) -> Result<SyncedPackageRow> {
    // Keep the existing package/download URL/metadata construction above the
    // version-scheme block unchanged.
```

Remove the current `crate::repository::distro` version-scheme lookup and its unknown-route RPM fallback, then replace it with:

```rust
    let route = crate::repository::supported_profiles::route_by_slug(&distro)
        .ok_or_else(|| crate::Error::Repository(format!("unsupported Remi distro route: {distro}")))?;
    let profile_id = route
        .public_profile_ids()
        .first()
        .ok_or_else(|| crate::Error::Repository(format!("no public profile for Remi distro route: {distro}")))?;
    let profile = crate::repository::supported_profiles::profile_by_public_id(profile_id)
        .ok_or_else(|| crate::Error::Repository(format!("profile disappeared for Remi distro route: {distro}")))?;
    let scheme = profile.version_scheme();
    let scheme_str = Some(match scheme {
        crate::repository::versioning::VersionScheme::Rpm => "rpm".to_string(),
        crate::repository::versioning::VersionScheme::Debian => "debian".to_string(),
        crate::repository::versioning::VersionScheme::Arch => "arch".to_string(),
    });
```

Change the final row return from bare value to `Ok(...)`:

```rust
    Ok(SyncedPackageRow {
        package,
        provides,
        requirements,
        requirement_groups: Vec::new(),
        requirement_group_clauses: Vec::new(),
    })
```

Add sync boundary tests in `crates/conary-core/src/repository/sync/remi.rs`:

```rust
#[test]
fn remi_sync_row_rejects_public_profile_id_as_route_slug() {
    for public_id in ["fedora-44", "ubuntu-26.04"] {
        let err = remi_sync_row(
            1,
            "https://remi.example.test".to_string(),
            public_id.to_string(),
            remi_entry_for_tests("bash", "5.2.0"),
        )
        .unwrap_err();

        assert!(err.to_string().contains("unsupported Remi distro route"));
    }
}

#[test]
fn remi_sync_row_accepts_route_slug_and_uses_profile_scheme() {
    let row = remi_sync_row(
        1,
        "https://remi.example.test".to_string(),
        "ubuntu".to_string(),
        remi_entry_for_tests("bash", "5.2.0"),
    )
    .unwrap();

    assert_eq!(row.package.distro.as_deref(), Some("ubuntu"));
    assert_eq!(row.package.version_scheme.as_deref(), Some("debian"));
}
```

Reuse or add a tiny `remi_entry_for_tests(name, version) -> RemiPackageEntry` helper in the existing `#[cfg(test)]` module.

Update `fetch_remi_sync_rows` so duplicate filtering and `?` work together:

```rust
let mut synced_packages = Vec::new();
for entry in response.packages {
    let key = (
        entry.name.clone(),
        entry.version.clone(),
        entry.release.clone(),
        entry.architecture.clone(),
    );
    if !seen.insert(key) {
        continue;
    }

    synced_packages.push(remi_sync_row(
        repo_id,
        endpoint.to_string(),
        distro.to_string(),
        entry,
    )?);
}
```

Keep `version_scheme_or_rpm` behavior for stored-row resolver/automation reads. Do not add a DB migration in M4d.

Prove sync derivation has no unknown-route RPM fallback left:

```bash
rg -n "version_scheme_from_distro_name|version_scheme_or_rpm" crates/conary-core/src/repository/sync/
```

Expected: no matches in `crates/conary-core/src/repository/sync/`. Matches in stored-row resolver or automation code outside `repository/sync/` may remain because M4d intentionally avoids a DB migration.

- [ ] **Step 9: Run Task 4 tests**

Run:

```bash
cargo test -p remi route
cargo test -p remi conversion
cargo test -p remi release_upload_
cargo test -p conary-core remi_sync
```

Expected: PASS.

- [ ] **Step 10: Commit Task 4**

```bash
git add apps/remi/src/server/handlers \
    apps/remi/src/server/native_publish/verify.rs \
    apps/remi/src/server/conversion/lookup.rs \
    apps/remi/src/server/conversion/metadata.rs \
    crates/conary-core/src/repository/sync/remi.rs
git commit -m "feat(remi): validate distro routes with supported profiles"
```

---

### Task 5: Add Focused M4d Regression And Trust-Gate Proof

**Files:**
- Modify: `apps/conary/tests/packaging_m4d.rs`
- Modify: Remi test modules touched in Task 4
- Test: `cargo test -p conary-core supported_profiles`
- Test: `cargo test -p conary-core ccs::v2`
- Test: `cargo test -p conary --lib commands::distro`
- Test: `cargo test -p conary --lib commands::install::source_policy`
- Test: `cargo test -p conary --test packaging_m2a`
- Test: `cargo test -p conary --lib commands::publish`
- Test: `cargo test -p conary-core repository::static_repo::publish_gate`
- Test: `cargo test -p conary-core remi_sync`
- Test: `cargo test -p remi route`
- Test: `cargo test -p remi conversion`
- Test: `cargo test -p remi release_upload_`

- [ ] **Step 1: Add one end-to-end M4d smoke assertion**

Extend `apps/conary/tests/packaging_m4d.rs`:

```rust
#[test]
fn packaging_m4d_supported_profiles_stay_narrow() {
    let (_db_temp, db_path) = common::setup_command_test_db();
    let list = Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(["distro", "list", "--db-path", &db_path])
        .output()
        .expect("run conary distro list");
    assert!(list.status.success());
    let stdout = String::from_utf8_lossy(&list.stdout);
    for supported in ["fedora-44", "ubuntu-26.04", "arch"] {
        assert!(stdout.contains(supported), "{supported}");
    }
    for unsupported in ["debian", "linux-mint", "ubuntu-noble", "fedora-45"] {
        assert!(!stdout.contains(unsupported), "{unsupported}");
    }
}
```

- [ ] **Step 2: Run M4d-focused tests**

Run:

```bash
cargo test -p conary-core supported_profiles
cargo test -p conary-core ccs::v2
cargo test -p conary --lib commands::distro
cargo test -p conary --lib commands::install::source_policy
cargo test -p conary --test packaging_m4d
cargo test -p conary-core remi_sync
cargo test -p remi route
cargo test -p remi conversion
cargo test -p remi release_upload_
```

Expected: PASS.

Then prove the embedded catalog did not grow unsupported targets by accident:

```bash
test ! -e data/distros.toml
rg -n "debian|linux-mint|ubuntu-noble|fedora-45" crates/conary-core/src/repository/supported_profiles/catalog.toml
```

Expected: `data/distros.toml` is gone, and the unsupported public targets do not appear in `crates/conary-core/src/repository/supported_profiles/catalog.toml`.

- [ ] **Step 3: Run M2 publish-gate regression proof**

Run:

```bash
cargo test -p conary --test packaging_m2a
cargo test -p conary --lib commands::publish
cargo test -p conary-core repository::static_repo::publish_gate
```

Expected: PASS. These tests prove the profile route cutover did not weaken artifact-form publish refusal, static publish trust, accepted signer policy, or recorded-draft refusal.

- [ ] **Step 4: Run broad workspace proof**

Run:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS.

- [ ] **Step 5: Commit Task 5**

```bash
git add apps/conary/tests/packaging_m4d.rs apps/remi/src/server
git commit -m "test(packaging): prove supported profile cutover"
```

---

### Task 6: Update Docs, Audit, And Coherency Ledgers

**Files:**
- Modify: `docs/modules/source-selection.md`
- Modify: `docs/modules/remi.md`
- Modify: `docs/modules/ccs.md`
- Modify: `docs/modules/test-fixtures.md`
- Modify: `docs/modules/feature-ownership.md`
- Modify: `docs/llms/subsystem-map.md`
- Modify: `docs/superpowers/feature-coherency-ledger.tsv`
- Modify: `docs/superpowers/feature-coherency-wave-scopes.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Test: `bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete`
- Test: `bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv`
- Test: `bash scripts/check-coherency-wave-scopes.sh docs/superpowers/feature-coherency-ledger.tsv docs/superpowers/feature-coherency-wave-scopes.tsv`
- Test: `bash scripts/check-doc-truth.sh`
- Test: `git diff --check`

- [ ] **Step 1: Update source-selection docs**

In `docs/modules/source-selection.md`, add the M4d profile owner near the source-policy or supported-distro section:

```markdown
M4d supported-target profiles make `crates/conary-core/src/repository/supported_profiles/`
the source of truth for public distro IDs, dependency flavor, version scheme,
and Remi route-family mapping. Fedora 44, Ubuntu 26.04, and Arch are the only
public targets. Internal route slugs such as `fedora` and `ubuntu` are not
public IDs. The older `data/distros.toml` catalog was deleted in M4d.
```

- [ ] **Step 2: Update Remi docs**

In `docs/modules/remi.md`, add:

```markdown
M4d routes every `{distro}` path parameter through supported profile route
validation before DB queries, cache/key filesystem paths, or release-upload
trust gates. Public Remi route slugs remain `fedora`, `ubuntu`, and `arch`;
they are backed by profile route metadata rather than a local hard-coded
`SUPPORTED_DISTROS` list.
```

- [ ] **Step 3: Update CCS docs**

In `docs/modules/ccs.md`, add:

```markdown
M4d completes the CCS v2 target-profile hook. `TargetProfileQuery` now covers
users, groups, directories, services, tmpfiles, sysctl, and alternatives.
Profile-backed validation accepts only explicit per-entry policy and reports
`LifecycleUnsupported` for unsupported signed lifecycle authority.
```

- [ ] **Step 4: Update fixture docs**

In `docs/modules/test-fixtures.md`, add:

```markdown
The M4d supported-profile fixture family proves exactly three public IDs
(`fedora-44`, `ubuntu-26.04`, and `arch`), route/profile agreement for
`fedora`, `ubuntu`, and `arch`, unsupported derivative refusal, and
profile-backed lifecycle diagnostics.
```

- [ ] **Step 5: Update feature ownership and assistant routing docs**

In `docs/modules/feature-ownership.md` and `docs/llms/subsystem-map.md`, route M4d work through:

```markdown
Start M4d supported-target profile work in
`crates/conary-core/src/repository/supported_profiles/`. CLI distro commands,
Remi route validation, conversion lookup/parser dispatch, Remi sync, and CCS
v2 lifecycle validation should delegate to that profile API instead of adding
new hard-coded distro matches.
```

- [ ] **Step 6: Update feature coherency rows**

Add or update rows for:

- `conary distro list` and `conary distro set` supported-target claims.
- Remi `{distro}` route validation.
- CCS v2 lifecycle validation.
- Remi conversion parser/source lookup.

Use the current ledger format and include focused commands from Task 5 as evidence. If a path is not already covered by the coherency ledger, document that no row was required in the task summary.

- [ ] **Step 7: Update docs-audit inventory and ledger**

Regenerate inventory:

```bash
bash scripts/docs-audit-inventory.sh > /tmp/conary-docs-audit-inventory.tsv
diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv /tmp/conary-docs-audit-inventory.tsv
```

Apply the inventory diff to `docs/superpowers/documentation-accuracy-audit-inventory.tsv`.

Add or update ledger rows for every tracked doc changed in this task. The M4d plan row should use:

```text
docs/superpowers/plans/2026-06-18-m4d-supported-distro-adapter-profiles-implementation-plan.md	docs/superpowers/plans/2026-06-18-m4d-supported-distro-adapter-profiles-implementation-plan.md	planning	maintainer	ccs-native; m4d; supported-target-profiles; implementation-plan; distro-adapters; lifecycle-validation; remi-route-slugs	docs/superpowers/specs/2026-06-18-m4d-supported-distro-adapter-profiles-design.md; docs/superpowers/specs/2026-06-17-m4-ccs-native-ecosystem-design.md; crates/conary-core/src/repository/supported_profiles/; data/distros.toml; crates/conary-core/src/repository/distro.rs; crates/conary-core/src/ccs/v2/validation.rs; apps/conary/src/commands/distro.rs; apps/conary/src/commands/install/source_policy.rs; apps/remi/src/server/handlers/mod.rs; apps/remi/src/server/handlers/sparse.rs; apps/remi/src/server/handlers/tuf.rs; apps/remi/src/server/handlers/admin/mod.rs; apps/remi/src/server/handlers/admin/packages.rs; apps/remi/src/server/conversion/lookup.rs; apps/remi/src/server/conversion/metadata.rs; crates/conary-core/src/repository/sync/remi.rs	verified	corrected	Locked the M4d implementation plan after DeepSeek, Gemini, and local agentic review; covers compile-time embedded supported-target profiles, old data/distros.toml deletion, public ID and route slug boundaries, explicit profile string domains, profile-backed CLI/source-policy/replay behavior, full CCS v2 lifecycle target-profile validation, Remi route inventory validation, route-only Remi sync derivation, M2 publish-gate regression proof, and docs/coherency gates before implementation.
```

- [ ] **Step 8: Run docs and coherency checks**

Run:

```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
bash scripts/check-coherency-wave-scopes.sh docs/superpowers/feature-coherency-ledger.tsv docs/superpowers/feature-coherency-wave-scopes.tsv
bash scripts/check-doc-truth.sh
git diff --check
```

Expected: PASS.

- [ ] **Step 9: Commit Task 6**

```bash
git add docs/modules/source-selection.md \
    docs/modules/remi.md \
    docs/modules/ccs.md \
    docs/modules/test-fixtures.md \
    docs/modules/feature-ownership.md \
    docs/llms/subsystem-map.md \
    docs/superpowers/feature-coherency-ledger.tsv \
    docs/superpowers/feature-coherency-wave-scopes.tsv \
    docs/superpowers/documentation-accuracy-audit-inventory.tsv \
    docs/superpowers/documentation-accuracy-audit-ledger.tsv
git commit -m "docs(m4d): document supported profile cutover"
```

---

## Final Verification

Before merging M4d, run:

```bash
cargo test -p conary-core supported_profiles
cargo test -p conary-core ccs::v2
cargo test -p conary --lib commands::distro
cargo test -p conary --lib commands::install::source_policy
cargo test -p conary --test packaging_m4d
cargo test -p conary --test packaging_m2a
cargo test -p conary --lib commands::publish
cargo test -p conary-core repository::static_repo::publish_gate
cargo test -p conary-core remi_sync
cargo test -p remi route
cargo test -p remi conversion
cargo test -p remi release_upload_
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
bash scripts/check-coherency-wave-scopes.sh docs/superpowers/feature-coherency-ledger.tsv docs/superpowers/feature-coherency-wave-scopes.tsv
bash scripts/check-doc-truth.sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

Expected: all commands pass.

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-06-18-m4d-supported-distro-adapter-profiles-implementation-plan.md`.

Two execution options:

1. Subagent-Driven (recommended) - dispatch a fresh subagent per task, review between tasks, fast iteration.
2. Inline Execution - execute tasks in this session using executing-plans, batch execution with checkpoints.
