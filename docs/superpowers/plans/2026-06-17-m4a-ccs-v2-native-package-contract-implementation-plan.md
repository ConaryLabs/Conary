# M4a CCS v2 Native Package Contract Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the first CCS v2 native package contract so validated v2 packages carry signed binary install-time authority, fail closed when authority is missing, and stop relying on v1 CBOR-to-TOML default reconstruction for native behavior.

**Architecture:** Add `crates/conary-core/src/ccs/v2/` as the owner of v2 schema, validation, authority reading, identity projection, and diagnostics. Keep existing v1 `BinaryManifest`, `CcsManifest`, `archive_reader.rs`, and `package.rs` as transition surfaces: archive/package code routes by format version, but v2 parsing and validation live in `ccs/v2`. M4a proves end-to-end v2 package authority for ordinary file-owning packages first, with group/redirect schema and validation covered but not installed.

**Tech Stack:** Rust 2024, serde, ciborium CBOR, serde_json canonical JSON, tar/gzip CCS archives, Ed25519 `PackageSignature`, existing `CcsBuilder`/package writer, existing `conary-core::diagnostics`, existing static-repo publish gate tests, Cargo test.

---

## Design Inputs

Read these before executing:

- `AGENTS.md`
- `docs/superpowers/specs/2026-06-17-m4a-ccs-v2-native-package-contract-design.md`
- `docs/superpowers/specs/2026-06-17-m4-ccs-native-ecosystem-design.md`
- `docs/superpowers/plans/2026-06-05-ccs-native-ecosystem-roadmap.md`
- `docs/modules/ccs.md`
- `docs/modules/test-fixtures.md`
- `docs/specs/ccs-format-v1.md`
- `crates/conary-core/src/ccs/binary_manifest.rs`
- `crates/conary-core/src/ccs/package.rs`
- `crates/conary-core/src/ccs/archive_reader.rs`
- `crates/conary-core/src/ccs/attestation.rs`
- `crates/conary-core/src/ccs/signing.rs`
- `crates/conary-core/src/ccs/verify.rs`
- `crates/conary-core/src/repository/static_repo/publish_gate.rs`

## Scope Locks

M4a includes:

- A v2-owned CBOR authority document with `format_version = 2`.
- Detached `MANIFEST.sig` verification over the exact archived v2 `MANIFEST` bytes.
- v2 package kind modeling for `package`, `group`, and `redirect`.
- v2 validation for package identity, dependency/provide entries, file/component authority, lifecycle declarations, TOML integrity, provenance role split, and kind-specific forbidden fields.
- v2 diagnostics for `missing-authority`, `legacy-v1-package`, `toml-only-authority`, `kind-contract-violation`, `component-authority-mismatch`, `lifecycle-unsupported`, `identity-unstable`, and `conversion-not-native`.
- v2 content identity projection that serializes positive authority fields rather than clearing old `CcsManifest` fields by name.
- Archive routing that rejects v1 as native v2 and never calls `convert_binary_to_ccs_manifest` for v2 native reads.
- Ordinary `package` end-to-end build/write/read/verify coverage.
- `group` and `redirect` parse/serialize/validate coverage.
- M2 publish-gate regression coverage.

M4a excludes:

- M4b maintainer-facing authoring UX.
- Remi native package intake, indexing, staging, or promotion.
- M4d target profile facts for Fedora 44, Ubuntu 26.04, or Arch.
- Distro expansion beyond Fedora 44, Ubuntu 26.04, and Arch.
- Native install proof for group and redirect packages.
- Converting foreign packages into native v2 unless the converted artifact satisfies the full v2 authority contract.
- Replacing the public TOML authoring format.
- Keeping CCS v1 installability as a product feature.

## File Map

Create:

- `crates/conary-core/src/ccs/v2/mod.rs` - v2 module hub and public exports.
- `crates/conary-core/src/ccs/v2/schema.rs` - `AuthorityDocumentV2`, package kind enum, typed dependencies/provides, files, components, lifecycle, provenance, policy, and TOML debug hash fields.
- `crates/conary-core/src/ccs/v2/diagnostics.rs` - `V2Diagnostic`, stable codes, severity, field/path hints, and conversion to packaging diagnostics.
- `crates/conary-core/src/ccs/v2/validation.rs` - fail-closed validation for authority completeness, kind contracts, component/file consistency, lifecycle representability, legacy metadata rejection, and TOML-only authority.
- `crates/conary-core/src/ccs/v2/reader.rs` - v2 raw-CBOR decode, raw-byte retention, signature verification, TOML debug hash verification, attestation/conversion metadata parsing, and archive result construction.
- `crates/conary-core/src/ccs/v2/identity.rs` - positive `ContentIdentityProjectionV2` and canonical content identity hashing.
- `crates/conary-core/src/ccs/v2/legacy.rs` - v1/legacy/foreign classification helpers and conversion rejection reasons.
- `crates/conary-core/src/ccs/v2/test_support.rs` - crate-private v2 authority fixtures and archive mutation helpers.
- `apps/conary/tests/packaging_m4a.rs` - CLI-level regression coverage where publish/verify behavior crosses app boundaries.

Modify:

- `crates/conary-core/src/ccs/mod.rs` - export `v2`.
- `crates/conary-core/src/ccs/archive_reader.rs` - route `MANIFEST` by format version, delegate v2 to `ccs/v2/reader.rs`, and preserve v1 logic as legacy only.
- `crates/conary-core/src/ccs/package.rs` - consume verified v2 package data through an explicit trust boundary; keep `convert_binary_to_ccs_manifest` out of v2 native paths.
- `crates/conary-core/src/ccs/builder/package_writer.rs` - add a narrow production v2 package writer path for complete M4a fixtures and project-form attestation refresh; keep v1 writer behavior unchanged unless the caller explicitly asks for v2.
- `crates/conary-core/src/ccs/verify.rs` - verify v2 signatures over raw archived CBOR bytes and surface v2 diagnostics.
- `crates/conary-core/src/ccs/attestation.rs` - route v2 content identity to `ccs/v2/identity.rs`; preserve old function behavior for legacy/v1 package tests.
- `crates/conary-core/src/repository/static_repo/publish_gate.rs` - keep M2 active emitted failures passing for v2 packages and preserve reserved diagnostic mappings.
- `crates/conary-core/src/repository/static_repo/publish_context.rs` - keep project-form attestation attachment writing v2 packages as v2 instead of round-tripping them through v1 `BinaryManifest`.
- `apps/conary/src/commands/ccs/install/command.rs` - keep native v2 install behind strict signature verification; do not let the legacy `--allow-unsigned` bypass admit unsigned v2 authority.
- `apps/conary/src/commands/diagnostics.rs` - map v2 validation diagnostics into packaging diagnostics when CLI-visible.
- `docs/modules/ccs.md` - document v2 as native authority after implementation passes.
- `docs/modules/test-fixtures.md` - record regenerated v2 native fixtures and legacy-rejection fixtures after implementation passes.
- `docs/llms/subsystem-map.md` and `docs/modules/feature-ownership.md` - route future CCS v2 work to `ccs/v2/`.
- `docs/superpowers/documentation-accuracy-audit-inventory.tsv` and `docs/superpowers/documentation-accuracy-audit-ledger.tsv` - register this plan and any touched docs.

Maintainability boundaries:

- `crates/conary-core/src/ccs/manifest.rs` is at the AGENTS threshold. M4a must not add v2 authority there unless a type is genuinely shared and the implementation review accepts the size impact.
- `crates/conary-core/src/ccs/archive_reader.rs` remains a routing facade. Do not bury v2 validation or field reconstruction there.
- `crates/conary-core/src/ccs/package.rs` remains the install-facing adapter. Do not extend `convert_binary_to_ccs_manifest` to support v2.
- `PackageFormat::parse` has no `TrustPolicy` argument, so it must not silently construct installable native v2 packages. M4a must add an explicit verified-v2 parse path and update install/publish callers to use it after signature/content verification.
- `crates/conary-core/src/ccs/verify.rs` may route verification, but v2-specific authority semantics live in `ccs/v2`.
- `crates/conary-core/src/repository/static_repo/publish_gate.rs` is above 1000 lines. M4a may add focused regression tests there because they are append-only proof that existing gates survive, not new gate behavior.

## Checkpoints

- Checkpoint 1 after Task 3: v2 schema, diagnostics, and validation unit tests pass.
- Checkpoint 2 after Task 6: v2 archive read/write, raw-byte signatures, and TOML debug hash behavior pass.
- Checkpoint 3 after Task 8: v2 package adapter and content identity behavior pass.
- Checkpoint 4 after Task 10: legacy classification, publish-gate regression, fixture/doc updates, fmt, and clippy pass.

## Review Lock Mapping

| Design concern | Plan owner |
| --- | --- |
| v2 must not extend v1 `BinaryManifest` | Task 2 `AuthorityDocumentV2`; Task 5 reader routing |
| Exact archived CBOR bytes are signed | Task 5 reader tests; Task 6 writer tests |
| TOML debug hash is drift proof only | Task 5 validation and verify tests |
| `package`/`group`/`redirect` field pollution | Task 3 validation tests |
| v2 identity is positive projection | Task 7 identity projection tests |
| `convert_binary_to_ccs_manifest` is not v2 native | Task 8 adapter and legacy tests |
| native v2 cannot bypass trust through `PackageFormat::parse` or `--allow-unsigned` | Task 8 verified-parse boundary and Task 9 install regression |
| v2 payload tampering cannot bypass signatures | Task 5 `verify_v2_archive_payload` and Task 9 tamper tests |
| build attestation and conversion boundary survive v2 adapter | Task 5 archive metadata parsing and Task 8 compatibility manifest bridge |
| Fixture classification starts implementation | Task 1 inventory |
| M2 publish gates survive | Task 9 regression tests |
| CLI diagnostics are stable and fail closed | Task 4 diagnostics and Task 10 CLI checks |

---

### Task 1: Inventory V1-Dependent Paths And Seed V2 Module

**Files:**
- Create: `crates/conary-core/src/ccs/v2/mod.rs`
- Create: `crates/conary-core/src/ccs/v2/legacy.rs`
- Modify: `crates/conary-core/src/ccs/mod.rs`
- Test: `crates/conary-core/src/ccs/v2/legacy.rs`

- [ ] **Step 1: Write the v1 classification tests**

Create `crates/conary-core/src/ccs/v2/legacy.rs` with a path comment and these tests:

```rust
// conary-core/src/ccs/v2/legacy.rs

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestFormatClassification {
    V2Native,
    LegacyV1,
    Unknown,
}

pub fn classify_manifest_format(format_version: Option<u64>) -> ManifestFormatClassification {
    match format_version {
        Some(2) => ManifestFormatClassification::V2Native,
        Some(1) => ManifestFormatClassification::LegacyV1,
        _ => ManifestFormatClassification::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_v2_v1_and_unknown_formats() {
        assert_eq!(
            classify_manifest_format(Some(2)),
            ManifestFormatClassification::V2Native
        );
        assert_eq!(
            classify_manifest_format(Some(1)),
            ManifestFormatClassification::LegacyV1
        );
        assert_eq!(
            classify_manifest_format(Some(0)),
            ManifestFormatClassification::Unknown
        );
        assert_eq!(
            classify_manifest_format(None),
            ManifestFormatClassification::Unknown
        );
    }
}
```

- [ ] **Step 2: Wire the v2 module**

Create `crates/conary-core/src/ccs/v2/mod.rs`:

```rust
// conary-core/src/ccs/v2/mod.rs
//! CCS v2 native package authority.

pub mod legacy;

pub use legacy::{ManifestFormatClassification, classify_manifest_format};
```

Add this to `crates/conary-core/src/ccs/mod.rs`:

```rust
pub mod v2;
```

- [ ] **Step 3: Run the focused test**

Run:

```bash
cargo test -p conary-core ccs::v2::legacy
```

Expected: pass.

- [ ] **Step 4: Record the implementation inventory in the commit message**

Before committing, run:

```bash
rg -n "convert_binary_to_ccs_manifest|FORMAT_VERSION|toml_integrity_hash|MANIFEST\\.toml|MANIFEST\\.sig" crates/conary-core/src/ccs crates/conary-core/src/repository/static_repo/publish_gate.rs apps/conary/src/commands/publish.rs
```

Expected: hits in `binary_manifest.rs`, `package.rs`, `archive_reader.rs`, `builder/package_writer.rs`, `verify.rs`, `publish_gate.rs`, and publish/verify surfaces. Use the output to confirm the Task 2 through Task 9 ownership before continuing.

- [ ] **Step 5: Commit**

```bash
git add crates/conary-core/src/ccs/mod.rs crates/conary-core/src/ccs/v2
git commit -m "feat(ccs): seed v2 authority module"
```

---

### Task 2: Define V2 Authority Schema

**Files:**
- Create: `crates/conary-core/src/ccs/v2/schema.rs`
- Modify: `crates/conary-core/src/ccs/v2/mod.rs`
- Test: `crates/conary-core/src/ccs/v2/schema.rs`

- [ ] **Step 1: Add schema tests first**

Create `schema.rs` with the path comment, then add tests that define the required shape:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_kind_serializes_as_tagged_enum() {
        let authority = AuthorityDocumentV2::package_for_tests("hello");
        let bytes = authority.to_cbor().unwrap();
        let decoded = AuthorityDocumentV2::from_cbor(&bytes).unwrap();
        assert_eq!(decoded.format_version, FORMAT_VERSION_V2);
        assert!(matches!(decoded.kind, PackageKindV2::Package(_)));
    }

    #[test]
    fn group_requires_members_and_has_no_payload_fields() {
        let group = PackageKindV2::Group(GroupDataV2 {
            members: vec![GroupMemberV2 {
                requirement: DependencyEntryV2::package("hello"),
                strength: GroupMemberStrengthV2::Required,
            }],
            provides: Vec::new(),
            conflicts: Vec::new(),
            policy: PackagePolicyV2::default(),
        });
        assert!(matches!(group, PackageKindV2::Group(_)));
    }

    #[test]
    fn redirect_has_minimum_authority_fields() {
        let redirect = RedirectDataV2 {
            to: "new-name".to_string(),
            version_constraint: Some(">=1.0".to_string()),
            reason: Some("package renamed".to_string()),
        };
        assert_eq!(redirect.to, "new-name");
        assert_eq!(redirect.version_constraint.as_deref(), Some(">=1.0"));
    }
}
```

- [ ] **Step 2: Implement the v2 schema**

Add this initial API. Keep it inside `ccs/v2/schema.rs`; do not add it to `manifest.rs`.

```rust
// conary-core/src/ccs/v2/schema.rs

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const FORMAT_VERSION_V2: u16 = 2;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthorityDocumentV2 {
    pub format_version: u16,
    pub identity: PackageIdentityV2,
    pub kind: PackageKindV2,
    #[serde(default)]
    pub provides: Vec<DependencyEntryV2>,
    #[serde(default)]
    pub requires: Vec<DependencyEntryV2>,
    #[serde(default)]
    pub components: BTreeMap<String, ComponentAuthorityV2>,
    #[serde(default)]
    pub lifecycle: LifecycleAuthorityV2,
    #[serde(default)]
    pub provenance: ProvenanceAuthorityV2,
    #[serde(default)]
    pub debug_toml_sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackageIdentityV2 {
    pub name: String,
    pub version: String,
    pub release: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub architecture: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    pub kind: PackageKindTagV2,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PackageKindTagV2 {
    Package,
    Group,
    Redirect,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", tag = "type", content = "data")]
pub enum PackageKindV2 {
    Package(PackageDataV2),
    Group(GroupDataV2),
    Redirect(RedirectDataV2),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PackageDataV2 {
    #[serde(default)]
    pub files: Vec<FileAuthorityV2>,
    #[serde(default)]
    pub config: Vec<ConfigAuthorityV2>,
    #[serde(default)]
    pub policy: PackagePolicyV2,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GroupDataV2 {
    pub members: Vec<GroupMemberV2>,
    #[serde(default)]
    pub provides: Vec<DependencyEntryV2>,
    #[serde(default)]
    pub conflicts: Vec<DependencyEntryV2>,
    #[serde(default)]
    pub policy: PackagePolicyV2,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RedirectDataV2 {
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_constraint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GroupMemberV2 {
    pub requirement: DependencyEntryV2,
    pub strength: GroupMemberStrengthV2,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum GroupMemberStrengthV2 {
    Required,
    Recommended,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DependencyEntryV2 {
    pub kind: DependencyKindV2,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_constraint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DependencyKindV2 {
    Package,
    Capability,
    File,
    Path,
    Binary,
    Soname,
    PkgConfig,
    Conflict,
    Replace,
    Obsolete,
    Break,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileAuthorityV2 {
    pub path: String,
    pub sha256: String,
    pub size: u64,
    pub file_type: FileTypeV2,
    pub mode: u32,
    pub owner: String,
    pub group: String,
    pub component: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symlink_target: Option<String>,
    #[serde(default)]
    pub config: Option<ConfigPolicyV2>,
    #[serde(default)]
    pub conflict: ConflictPolicyV2,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum FileTypeV2 {
    Regular,
    Directory,
    Symlink,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ComponentAuthorityV2 {
    pub name: String,
    pub default: bool,
    pub file_count: u32,
    pub total_size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfigAuthorityV2 {
    pub path: String,
    pub policy: ConfigPolicyV2,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigPolicyV2 {
    Replace,
    NoReplace,
    Merge,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ConflictPolicyV2 {
    #[default]
    Error,
    Replace,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LifecycleAuthorityV2 {
    /// M4a carries opaque install-time authority references. Structured
    /// lifecycle authoring and target-specific validation are deferred to M4b
    /// and M4d; these strings must still be signed v2 authority.
    #[serde(default)]
    pub users: Vec<String>,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default)]
    pub directories: Vec<String>,
    #[serde(default)]
    pub services: Vec<String>,
    #[serde(default)]
    pub tmpfiles: Vec<String>,
    #[serde(default)]
    pub sysctl: Vec<String>,
    #[serde(default)]
    pub alternatives: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ProvenanceAuthorityV2 {
    /// Install-time provenance facts. Build attestation envelopes live in
    /// `MANIFEST.attestation.json`; the build-time classifier version remains
    /// in that attestation envelope, not in v2 package authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hardening_level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_input_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hermetic_evidence_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreign_conversion_boundary_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PackagePolicyV2 {
    /// M4a carries only host-mutation policy. Required-capability,
    /// public-serving, and trust-metadata policy fields are reserved for M4b+.
    #[serde(default)]
    pub allow_host_mutation: bool,
}

impl DependencyEntryV2 {
    pub fn package(name: impl Into<String>) -> Self {
        Self {
            kind: DependencyKindV2::Package,
            name: name.into(),
            version_constraint: None,
            target: None,
            component: None,
        }
    }
}

impl AuthorityDocumentV2 {
    pub fn to_cbor(&self) -> Result<Vec<u8>, ciborium::ser::Error<std::io::Error>> {
        let mut buf = Vec::new();
        ciborium::into_writer(self, &mut buf)?;
        Ok(buf)
    }

    pub fn from_cbor(bytes: &[u8]) -> Result<Self, ciborium::de::Error<std::io::Error>> {
        ciborium::from_reader(bytes)
    }

    #[cfg(test)]
    pub(crate) fn package_for_tests(name: &str) -> Self {
        let mut authority = Self::empty_package_for_tests(name);
        authority.components = BTreeMap::from([(
            "main".to_string(),
            ComponentAuthorityV2 {
                name: "main".to_string(),
                default: true,
                file_count: 1,
                total_size: 12,
            },
        )]);
        if let PackageKindV2::Package(data) = &mut authority.kind {
            data.files.push(FileAuthorityV2 {
                path: "/usr/bin/hello".to_string(),
                sha256: crate::hash::sha256(b"hello world\n"),
                size: 12,
                file_type: FileTypeV2::Regular,
                mode: 0o755,
                owner: "root".to_string(),
                group: "root".to_string(),
                component: "main".to_string(),
                symlink_target: None,
                config: None,
                conflict: ConflictPolicyV2::Error,
            });
        }
        authority
    }

    #[cfg(test)]
    pub(crate) fn empty_package_for_tests(name: &str) -> Self {
        Self {
            format_version: FORMAT_VERSION_V2,
            identity: PackageIdentityV2 {
                name: name.to_string(),
                version: "1.0.0".to_string(),
                release: "1".to_string(),
                architecture: Some("x86_64".to_string()),
                platform: Some("linux".to_string()),
                kind: PackageKindTagV2::Package,
            },
            kind: PackageKindV2::Package(PackageDataV2::default()),
            provides: Vec::new(),
            requires: Vec::new(),
            components: BTreeMap::new(),
            lifecycle: LifecycleAuthorityV2::default(),
            provenance: ProvenanceAuthorityV2 {
                origin_class: Some("native-built".to_string()),
                hardening_level: Some("hermetic".to_string()),
                build_input_identity: Some("sha256:build-input".to_string()),
                hermetic_evidence_hash: Some("sha256:evidence".to_string()),
                foreign_conversion_boundary_hash: None,
            },
            debug_toml_sha256: None,
        }
    }
}
```

- [ ] **Step 3: Export schema types**

Update `v2/mod.rs`:

```rust
pub mod legacy;
pub mod schema;

pub use legacy::{ManifestFormatClassification, classify_manifest_format};
pub use schema::{
    AuthorityDocumentV2, DependencyEntryV2, FORMAT_VERSION_V2, PackageKindTagV2, PackageKindV2,
};
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p conary-core ccs::v2::schema
cargo fmt --check
```

Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/conary-core/src/ccs/v2 crates/conary-core/src/ccs/mod.rs
git commit -m "feat(ccs): add v2 authority schema"
```

---

### Task 3: Add V2 Diagnostics And Validation

**Files:**
- Create: `crates/conary-core/src/ccs/v2/diagnostics.rs`
- Create: `crates/conary-core/src/ccs/v2/validation.rs`
- Modify: `crates/conary-core/src/ccs/v2/mod.rs`
- Test: `crates/conary-core/src/ccs/v2/validation.rs`

- [ ] **Step 1: Add diagnostic types**

Create `diagnostics.rs`:

```rust
// conary-core/src/ccs/v2/diagnostics.rs

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum V2DiagnosticCode {
    MissingAuthority,
    LegacyV1Package,
    TomlOnlyAuthority,
    KindContractViolation,
    ComponentAuthorityMismatch,
    LifecycleUnsupported,
    IdentityUnstable,
    ConversionNotNative,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum V2DiagnosticSeverity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct V2Diagnostic {
    pub code: V2DiagnosticCode,
    pub severity: V2DiagnosticSeverity,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub invalid: bool,
    pub suggestion: String,
}

impl V2Diagnostic {
    pub fn error(
        code: V2DiagnosticCode,
        message: impl Into<String>,
        field: impl Into<Option<String>>,
        suggestion: impl Into<String>,
    ) -> Self {
        Self {
            code,
            severity: V2DiagnosticSeverity::Error,
            message: message.into(),
            field: field.into(),
            path: None,
            invalid: true,
            suggestion: suggestion.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct V2ValidationError {
    pub diagnostics: Vec<V2Diagnostic>,
}

impl std::fmt::Display for V2ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(first) = self.diagnostics.first() {
            write!(f, "{}", first.message)
        } else {
            write!(f, "v2 validation failed")
        }
    }
}

impl std::error::Error for V2ValidationError {}
```

- [ ] **Step 2: Write failing validation tests**

Create `validation.rs` tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::v2::diagnostics::V2DiagnosticCode;
    use crate::ccs::v2::schema::{
        AuthorityDocumentV2, DependencyEntryV2, GroupDataV2, PackageKindTagV2, PackageKindV2,
        RedirectDataV2,
    };

    #[test]
    fn rejects_missing_package_files_as_missing_authority() {
        let authority = AuthorityDocumentV2::empty_package_for_tests("empty-package");
        let error = validate_authority(&authority).unwrap_err();
        assert!(error.diagnostics.iter().any(|d| d.code == V2DiagnosticCode::MissingAuthority));
    }

    #[test]
    fn rejects_group_without_members() {
        let mut authority = AuthorityDocumentV2::empty_package_for_tests("empty-group");
        authority.identity.kind = PackageKindTagV2::Group;
        authority.kind = PackageKindV2::Group(GroupDataV2 {
            members: Vec::new(),
            provides: Vec::new(),
            conflicts: Vec::new(),
            policy: Default::default(),
        });
        let error = validate_authority(&authority).unwrap_err();
        assert!(error.diagnostics.iter().any(|d| d.code == V2DiagnosticCode::KindContractViolation));
    }

    #[test]
    fn accepts_redirect_with_target() {
        let mut authority = AuthorityDocumentV2::empty_package_for_tests("old-name");
        authority.identity.kind = PackageKindTagV2::Redirect;
        authority.kind = PackageKindV2::Redirect(RedirectDataV2 {
            to: "new-name".to_string(),
            version_constraint: None,
            reason: Some("renamed".to_string()),
        });
        validate_authority(&authority).unwrap();
    }

    #[test]
    fn rejects_kind_tag_payload_mismatch() {
        let mut authority = AuthorityDocumentV2::package_for_tests("mismatch");
        authority.identity.kind = PackageKindTagV2::Group;
        let error = validate_authority(&authority).unwrap_err();
        assert!(error.diagnostics.iter().any(|d| d.code == V2DiagnosticCode::KindContractViolation));
    }

    #[test]
    fn dependency_entries_need_name() {
        let mut authority = AuthorityDocumentV2::package_for_tests("bad-dep");
        authority.requires.push(DependencyEntryV2::package(""));
        let error = validate_authority(&authority).unwrap_err();
        assert!(error.diagnostics.iter().any(|d| d.field.as_deref() == Some("requires.name")));
    }

    #[test]
    fn rejects_incomplete_identity_provenance_and_component_totals() {
        let mut authority = AuthorityDocumentV2::package_for_tests("bad-authority");
        authority.identity.release.clear();
        authority.provenance.origin_class = None;
        authority.provenance.hardening_level = None;
        authority
            .components
            .get_mut("main")
            .unwrap()
            .file_count = 2;
        let error = validate_authority(&authority).unwrap_err();
        assert!(error.diagnostics.iter().any(|d| d.field.as_deref() == Some("identity.release")));
        assert!(error.diagnostics.iter().any(|d| d.field.as_deref() == Some("provenance.origin_class")));
        assert!(error.diagnostics.iter().any(|d| d.code == V2DiagnosticCode::ComponentAuthorityMismatch));
    }

    #[test]
    fn rejects_symlink_without_signed_target() {
        let mut authority = AuthorityDocumentV2::package_for_tests("bad-link");
        let PackageKindV2::Package(data) = &mut authority.kind else {
            panic!("fixture should be package");
        };
        data.files[0].file_type = FileTypeV2::Symlink;
        data.files[0].symlink_target = None;
        let error = validate_authority(&authority).unwrap_err();
        assert!(error.diagnostics.iter().any(|d| d.field.as_deref() == Some("kind.package.files.symlink_target")));
    }
}
```

- [ ] **Step 3: Implement validation**

Implement:

```rust
// conary-core/src/ccs/v2/validation.rs

use super::diagnostics::{V2Diagnostic, V2DiagnosticCode, V2ValidationError};
use super::schema::*;

pub fn validate_authority(authority: &AuthorityDocumentV2) -> Result<(), V2ValidationError> {
    let mut diagnostics = Vec::new();

    if authority.format_version != FORMAT_VERSION_V2 {
        diagnostics.push(V2Diagnostic::error(
            V2DiagnosticCode::LegacyV1Package,
            format!("unsupported CCS authority format {}", authority.format_version),
            Some("format_version".to_string()),
            "rebuild or regenerate the package as CCS v2",
        ));
    }
    if authority.identity.name.trim().is_empty() {
        diagnostics.push(V2Diagnostic::error(
            V2DiagnosticCode::MissingAuthority,
            "v2 package identity name is required",
            Some("identity.name".to_string()),
            "set identity.name in signed v2 authority",
        ));
    }
    if authority.identity.version.trim().is_empty() {
        diagnostics.push(V2Diagnostic::error(
            V2DiagnosticCode::MissingAuthority,
            "v2 package identity version is required",
            Some("identity.version".to_string()),
            "set identity.version in signed v2 authority",
        ));
    }
    if authority.identity.release.trim().is_empty() {
        diagnostics.push(V2Diagnostic::error(
            V2DiagnosticCode::MissingAuthority,
            "v2 package identity release is required",
            Some("identity.release".to_string()),
            "set identity.release in signed v2 authority",
        ));
    }
    validate_provenance(&authority.provenance, &mut diagnostics);
    validate_dependencies("requires", &authority.requires, &mut diagnostics);
    validate_dependencies("provides", &authority.provides, &mut diagnostics);
    validate_component_defaults(authority, &mut diagnostics);

    match (&authority.identity.kind, &authority.kind) {
        (PackageKindTagV2::Package, PackageKindV2::Package(data)) => {
            if data.files.is_empty() {
                diagnostics.push(V2Diagnostic::error(
                    V2DiagnosticCode::MissingAuthority,
                    "v2 package kind requires at least one file authority entry",
                    Some("kind.package.files".to_string()),
                    "write file path/hash/component authority into v2 MANIFEST",
                ));
            }
            validate_files(data, authority, &mut diagnostics);
            validate_component_totals(data, authority, &mut diagnostics);
            validate_lifecycle(&authority.lifecycle, &mut diagnostics);
        }
        (PackageKindTagV2::Group, PackageKindV2::Group(data)) => {
            reject_group_redirect_payload_authority(authority, &mut diagnostics);
            if data.members.is_empty() {
                diagnostics.push(V2Diagnostic::error(
                    V2DiagnosticCode::KindContractViolation,
                    "v2 group packages require at least one member",
                    Some("kind.group.members".to_string()),
                    "add required or recommended group member requirements",
                ));
            }
        }
        (PackageKindTagV2::Redirect, PackageKindV2::Redirect(data)) => {
            reject_group_redirect_payload_authority(authority, &mut diagnostics);
            if data.to.trim().is_empty() {
                diagnostics.push(V2Diagnostic::error(
                    V2DiagnosticCode::KindContractViolation,
                    "v2 redirect packages require redirect.to",
                    Some("kind.redirect.to".to_string()),
                    "set redirect target package name",
                ));
            }
        }
        _ => diagnostics.push(V2Diagnostic::error(
            V2DiagnosticCode::KindContractViolation,
            "v2 package kind tag does not match payload",
            Some("identity.kind".to_string()),
            "make identity.kind match the package/group/redirect payload",
        )),
    }

    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(V2ValidationError { diagnostics })
    }
}

fn validate_provenance(provenance: &ProvenanceAuthorityV2, diagnostics: &mut Vec<V2Diagnostic>) {
    for (field, value) in [
        ("provenance.origin_class", provenance.origin_class.as_deref()),
        ("provenance.hardening_level", provenance.hardening_level.as_deref()),
        ("provenance.build_input_identity", provenance.build_input_identity.as_deref()),
        ("provenance.hermetic_evidence_hash", provenance.hermetic_evidence_hash.as_deref()),
    ] {
        if value.is_none_or(|value| value.trim().is_empty()) {
            diagnostics.push(V2Diagnostic::error(
                V2DiagnosticCode::MissingAuthority,
                format!("v2 authority requires {field}"),
                Some(field.to_string()),
                "write complete provenance authority into signed v2 MANIFEST",
            ));
        }
    }
}

fn validate_component_defaults(authority: &AuthorityDocumentV2, diagnostics: &mut Vec<V2Diagnostic>) {
    let default_count = authority.components.values().filter(|component| component.default).count();
    if default_count != 1 {
        diagnostics.push(V2Diagnostic::error(
            V2DiagnosticCode::ComponentAuthorityMismatch,
            "v2 package authority requires exactly one default component",
            Some("components.default".to_string()),
            "mark one and only one component as default",
        ));
    }
}

fn validate_dependencies(prefix: &str, entries: &[DependencyEntryV2], diagnostics: &mut Vec<V2Diagnostic>) {
    for entry in entries {
        if entry.name.trim().is_empty() {
            diagnostics.push(V2Diagnostic::error(
                V2DiagnosticCode::MissingAuthority,
                format!("{prefix} entry requires a name"),
                Some(format!("{prefix}.name")),
                "write typed dependency/provide name into signed v2 authority",
            ));
        }
    }
}

fn validate_files(
    data: &PackageDataV2,
    authority: &AuthorityDocumentV2,
    diagnostics: &mut Vec<V2Diagnostic>,
) {
    for file in &data.files {
        if file.path.trim().is_empty() || file.sha256.trim().is_empty() || file.component.trim().is_empty() {
            diagnostics.push(V2Diagnostic::error(
                V2DiagnosticCode::MissingAuthority,
                "v2 file authority requires path, sha256, size, and component",
                Some("kind.package.files".to_string()),
                "write complete file authority into signed v2 authority",
            ));
        }
        match file.file_type {
            FileTypeV2::Regular => {
                if file.symlink_target.is_some() {
                    diagnostics.push(V2Diagnostic::error(
                        V2DiagnosticCode::KindContractViolation,
                        format!("regular file {} must not carry symlink target", file.path),
                        Some("kind.package.files.symlink_target".to_string()),
                        "clear symlink_target for regular files",
                    ));
                }
            }
            FileTypeV2::Directory => {
                if file.size != 0 || file.symlink_target.is_some() {
                    diagnostics.push(V2Diagnostic::error(
                        V2DiagnosticCode::KindContractViolation,
                        format!("directory {} must have size 0 and no symlink target", file.path),
                        Some("kind.package.files".to_string()),
                        "encode directory authority without blob size or symlink target",
                    ));
                }
            }
            FileTypeV2::Symlink => {
                if file.symlink_target.as_deref().is_none_or(str::is_empty) {
                    diagnostics.push(V2Diagnostic::error(
                        V2DiagnosticCode::MissingAuthority,
                        format!("symlink {} requires signed target authority", file.path),
                        Some("kind.package.files.symlink_target".to_string()),
                        "write symlink target into signed v2 authority",
                    ));
                }
            }
        }
        if !authority.components.contains_key(&file.component) {
            diagnostics.push(V2Diagnostic::error(
                V2DiagnosticCode::ComponentAuthorityMismatch,
                format!("file {} references unknown component {}", file.path, file.component),
                Some("kind.package.files.component".to_string()),
                "add matching component authority for every file component",
            ));
        }
    }
}

fn validate_component_totals(
    data: &PackageDataV2,
    authority: &AuthorityDocumentV2,
    diagnostics: &mut Vec<V2Diagnostic>,
) {
    for (name, component) in &authority.components {
        let files = data.files.iter().filter(|file| file.component == *name).collect::<Vec<_>>();
        let total_size: u64 = files.iter().map(|file| file.size).sum();
        if component.file_count as usize != files.len() || component.total_size != total_size {
            diagnostics.push(V2Diagnostic::error(
                V2DiagnosticCode::ComponentAuthorityMismatch,
                format!("component {name} count or size does not match signed file authority"),
                Some("components".to_string()),
                "make component file_count and total_size match package file authority",
            ));
        }
    }
}

fn validate_lifecycle(lifecycle: &LifecycleAuthorityV2, diagnostics: &mut Vec<V2Diagnostic>) {
    // M4a accepts local user/group/directory/alternative declarations, but
    // profile-bound service/tmpfiles/sysctl checks must fail closed until M4d
    // provides target facts.
    if !lifecycle.services.is_empty() || !lifecycle.tmpfiles.is_empty() || !lifecycle.sysctl.is_empty() {
        diagnostics.push(V2Diagnostic::error(
            V2DiagnosticCode::LifecycleUnsupported,
            "v2 lifecycle services, tmpfiles, and sysctl declarations require target profile facts",
            Some("lifecycle".to_string()),
            "defer profile-bound lifecycle declarations until M4d target profiles are available",
        ));
    }
}

fn reject_group_redirect_payload_authority(
    authority: &AuthorityDocumentV2,
    diagnostics: &mut Vec<V2Diagnostic>,
) {
    if !authority.components.is_empty() || authority.lifecycle != LifecycleAuthorityV2::default() {
        diagnostics.push(V2Diagnostic::error(
            V2DiagnosticCode::KindContractViolation,
            "v2 group and redirect packages must not carry file components or lifecycle payload authority",
            Some("components".to_string()),
            "move file/lifecycle authority to package kind payloads only",
        ));
    }
}
```

- [ ] **Step 4: Export diagnostics and validation**

Update `v2/mod.rs`:

```rust
pub mod diagnostics;
pub mod legacy;
pub mod schema;
pub mod validation;

pub use diagnostics::{V2Diagnostic, V2DiagnosticCode, V2ValidationError};
pub use legacy::{ManifestFormatClassification, classify_manifest_format};
pub use schema::{
    AuthorityDocumentV2, DependencyEntryV2, FORMAT_VERSION_V2, PackageKindTagV2, PackageKindV2,
};
pub use validation::validate_authority;
```

- [ ] **Step 5: Run validation tests**

```bash
cargo test -p conary-core ccs::v2::validation
cargo fmt --check
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add crates/conary-core/src/ccs/v2
git commit -m "feat(ccs): validate v2 authority contract"
```

---

### Task 4: Add Profile Hook And Packaging Diagnostic Mapping

**Files:**
- Modify: `crates/conary-core/src/ccs/v2/validation.rs`
- Modify: `crates/conary-core/src/ccs/v2/diagnostics.rs`
- Modify: `crates/conary-core/src/diagnostics/mod.rs`
- Modify: `apps/conary/src/commands/diagnostics.rs`
- Test: `crates/conary-core/src/ccs/v2/validation.rs`
- Test: `apps/conary/src/commands/diagnostics.rs`

- [ ] **Step 1: Define the M4a to M4d hook point**

In `validation.rs`, add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileConstraintStatus {
    Accepted,
    Unsupported,
}

pub trait TargetProfileQuery {
    fn service_status(&self, service: &str) -> ProfileConstraintStatus;
    fn tmpfiles_status(&self, entry: &str) -> ProfileConstraintStatus;
    fn sysctl_status(&self, key: &str) -> ProfileConstraintStatus;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct M4aNoProfileFacts;

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
}

pub fn validate_authority_with_profile(
    authority: &AuthorityDocumentV2,
    profile: &impl TargetProfileQuery,
) -> Result<(), V2ValidationError> {
    let mut diagnostics = validate_authority(authority)
        .err()
        .map(|error| error.diagnostics)
        .unwrap_or_default();

    for service in &authority.lifecycle.services {
        if profile.service_status(service) == ProfileConstraintStatus::Unsupported {
            diagnostics.push(V2Diagnostic::error(
                V2DiagnosticCode::LifecycleUnsupported,
                format!("service {service} is not supported by the target profile"),
                Some("lifecycle.services".to_string()),
                "remove the service declaration or wait for M4d target profile support",
            ));
        }
    }
    for entry in &authority.lifecycle.tmpfiles {
        if profile.tmpfiles_status(entry) == ProfileConstraintStatus::Unsupported {
            diagnostics.push(V2Diagnostic::error(
                V2DiagnosticCode::LifecycleUnsupported,
                format!("tmpfiles entry {entry} is not supported by the target profile"),
                Some("lifecycle.tmpfiles".to_string()),
                "remove the tmpfiles declaration or wait for M4d target profile support",
            ));
        }
    }
    for key in &authority.lifecycle.sysctl {
        if profile.sysctl_status(key) == ProfileConstraintStatus::Unsupported {
            diagnostics.push(V2Diagnostic::error(
                V2DiagnosticCode::LifecycleUnsupported,
                format!("sysctl key {key} is not supported by the target profile"),
                Some("lifecycle.sysctl".to_string()),
                "remove the sysctl declaration or wait for M4d target profile support",
            ));
        }
    }

    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(V2ValidationError { diagnostics })
    }
}
```

Then make `validate_authority` call `validate_authority_with_profile(authority, &M4aNoProfileFacts)`, while sharing the common validation body so the wrapper does not recurse. M4a must fail closed for non-empty service/tmpfiles/sysctl lifecycle authority until M4d supplies real target-profile facts.

- [ ] **Step 2: Add the profile rejection test**

```rust
#[test]
fn profile_hook_can_reject_lifecycle_without_target_facts() {
    struct RejectServices;
    impl TargetProfileQuery for RejectServices {
        fn service_status(&self, _service: &str) -> ProfileConstraintStatus {
            ProfileConstraintStatus::Unsupported
        }
        fn tmpfiles_status(&self, _entry: &str) -> ProfileConstraintStatus {
            ProfileConstraintStatus::Unsupported
        }
        fn sysctl_status(&self, _key: &str) -> ProfileConstraintStatus {
            ProfileConstraintStatus::Unsupported
        }
    }

    let mut authority = AuthorityDocumentV2::package_for_tests("svc");
    authority.lifecycle.services.push("svc.service".to_string());
    let error = validate_authority_with_profile(&authority, &RejectServices).unwrap_err();
    assert!(error.diagnostics.iter().any(|d| d.code == V2DiagnosticCode::LifecycleUnsupported));
}
```

- [ ] **Step 3: Add core diagnostic enum entries**

In `crates/conary-core/src/diagnostics/mod.rs`, add:

```rust
    CcsV2ValidationFailed,
    CcsV2LegacyRejected,
```

to `PackagingDiagnosticCode`.

- [ ] **Step 4: Map v2 diagnostics to packaging diagnostics**

In `apps/conary/src/commands/diagnostics.rs`, add:

```rust
pub(crate) fn ccs_v2_diagnostic_to_packaging(
    diagnostic: &conary_core::ccs::v2::V2Diagnostic,
) -> conary_core::diagnostics::PackagingDiagnostic {
    use conary_core::ccs::v2::V2DiagnosticCode;
    use conary_core::diagnostics::{
        PackagingDiagnostic, PackagingDiagnosticCode, PackagingPhase, PackagingSuggestion,
    };

    let code = match diagnostic.code {
        V2DiagnosticCode::LegacyV1Package => PackagingDiagnosticCode::CcsV2LegacyRejected,
        _ => PackagingDiagnosticCode::CcsV2ValidationFailed,
    };
    let mut rendered = PackagingDiagnostic::error(
        PackagingPhase::RecipeValidation,
        code,
        diagnostic.message.clone(),
    );
    rendered
        .suggestions
        .push(PackagingSuggestion::new(diagnostic.suggestion.clone()));
    rendered
}
```

Add a unit test:

```rust
#[test]
fn ccs_v2_diagnostics_map_to_packaging_diagnostics() {
    let diagnostic = conary_core::ccs::v2::V2Diagnostic::error(
        conary_core::ccs::v2::V2DiagnosticCode::LegacyV1Package,
        "legacy package",
        Some("format_version".to_string()),
        "rebuild as v2",
    );
    let rendered = ccs_v2_diagnostic_to_packaging(&diagnostic);
    assert_eq!(
        rendered.code,
        conary_core::diagnostics::PackagingDiagnosticCode::CcsV2LegacyRejected
    );
    assert_eq!(rendered.suggestions[0].message, "rebuild as v2");
}
```

- [ ] **Step 5: Run tests**

```bash
cargo test -p conary-core ccs::v2::validation
cargo test -p conary --lib commands::diagnostics
cargo fmt --check
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add crates/conary-core/src/ccs/v2 crates/conary-core/src/diagnostics/mod.rs apps/conary/src/commands/diagnostics.rs
git commit -m "feat(ccs): surface v2 validation diagnostics"
```

---

### Task 5: Implement V2 Archive Reader

**Files:**
- Create: `crates/conary-core/src/ccs/v2/reader.rs`
- Modify: `crates/conary-core/src/ccs/v2/mod.rs`
- Modify: `crates/conary-core/src/ccs/archive_reader.rs`
- Modify: `crates/conary-core/src/ccs/verify.rs`
- Test: `crates/conary-core/src/ccs/v2/reader.rs`
- Test: `crates/conary-core/src/ccs/archive_reader.rs`
- Test: `crates/conary-core/src/ccs/verify.rs`

- [ ] **Step 1: Add reader tests for raw-byte signatures and TOML hash**

In `reader.rs`, add tests that build a v2 authority, sign its exact bytes, and verify those bytes:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::signing::SigningKeyPair;
    use crate::ccs::verify::TrustPolicy;

    #[test]
    fn verifies_signature_against_exact_archived_manifest_bytes() {
        let authority = crate::ccs::v2::schema::AuthorityDocumentV2::package_for_tests("signed");
        let raw = authority.to_cbor().unwrap();
        let key = SigningKeyPair::generate();
        let signature = key.sign(&raw);
        let policy = TrustPolicy::strict(vec![signature.public_key.clone()]);

        read_authority_document(
            &raw,
            Some(&serde_json::to_string(&signature).unwrap()),
            None,
            None,
            None,
            &policy,
        )
            .unwrap();

        let reserialized = authority.to_cbor().unwrap();
        let mut drifted = reserialized.clone();
        drifted.push(0);
        assert!(read_authority_document(
            &drifted,
            Some(&serde_json::to_string(&signature).unwrap()),
            None,
            None,
            None,
            &policy,
        )
        .is_err());
    }

    #[test]
    fn rejects_toml_debug_drift() {
        let mut authority = crate::ccs::v2::schema::AuthorityDocumentV2::package_for_tests("debug");
        authority.debug_toml_sha256 = Some(crate::hash::sha256(b"original"));
        let raw = authority.to_cbor().unwrap();
        let key = SigningKeyPair::generate();
        let signature = key.sign(&raw);
        let policy = TrustPolicy::strict(vec![signature.public_key.clone()]);

        let error = read_authority_document(
            &raw,
            Some(&serde_json::to_string(&signature).unwrap()),
            Some(b"modified"),
            None,
            None,
            &policy,
        )
        .unwrap_err();
        assert!(error.to_string().contains("TOML"));
    }
}
```

- [ ] **Step 2: Implement v2 authority reading**

Add:

```rust
// conary-core/src/ccs/v2/reader.rs

use super::schema::AuthorityDocumentV2;
use super::validation::validate_authority;
use crate::ccs::verify::{
    PackageSignature, SignatureStatus, TrustPolicy, verify_manifest_signature,
};
use anyhow::{Context, Result, bail};

#[derive(Debug, Clone)]
pub struct ReadAuthorityV2 {
    pub authority: AuthorityDocumentV2,
    pub raw_manifest: Vec<u8>,
    pub signature: PackageSignature,
    pub build_attestation: Option<crate::ccs::attestation::BuildAttestationEnvelope>,
    pub foreign_conversion_boundary: Option<crate::ccs::attestation::ForeignConversionBoundary>,
}

pub fn read_authority_document(
    raw_manifest: &[u8],
    signature_raw: Option<&str>,
    toml_raw: Option<&[u8]>,
    build_attestation_raw: Option<&str>,
    foreign_conversion_boundary_raw: Option<&str>,
    policy: &TrustPolicy,
) -> Result<ReadAuthorityV2> {
    let authority = AuthorityDocumentV2::from_cbor(raw_manifest).context("decode CCS v2 MANIFEST")?;
    validate_authority(&authority).map_err(|error| anyhow::anyhow!("{error}"))?;
    let signature_raw = signature_raw.context("CCS v2 MANIFEST.sig is required")?;
    let signature: PackageSignature = serde_json::from_str(signature_raw).context("parse MANIFEST.sig")?;
    verify_v2_signature(raw_manifest, &signature, policy)?;
    verify_debug_toml_hash(&authority, toml_raw)?;
    reject_install_authority_toml(toml_raw)?;
    let build_attestation = build_attestation_raw
        .map(serde_json::from_str)
        .transpose()
        .context("parse MANIFEST.attestation.json")?;
    let foreign_conversion_boundary = foreign_conversion_boundary_raw
        .map(serde_json::from_str)
        .transpose()
        .context("parse MANIFEST.conversion-boundary.json")?;
    verify_conversion_boundary_hash(&authority, foreign_conversion_boundary.as_ref())?;
    Ok(ReadAuthorityV2 {
        authority,
        raw_manifest: raw_manifest.to_vec(),
        signature,
        build_attestation,
        foreign_conversion_boundary,
    })
}

fn verify_v2_signature(
    raw_manifest: &[u8],
    package_signature: &PackageSignature,
    policy: &TrustPolicy,
) -> Result<()> {
    match verify_manifest_signature(raw_manifest, Some(package_signature), policy)? {
        SignatureStatus::Valid { .. } => Ok(()),
        SignatureStatus::Unsigned => bail!("CCS v2 MANIFEST.sig is required"),
        SignatureStatus::Invalid(reason) => bail!("invalid CCS v2 signature: {reason}"),
        SignatureStatus::Untrusted { key_id } => {
            bail!("CCS v2 package signature key is not trusted: {key_id:?}")
        }
    }
}

fn verify_debug_toml_hash(authority: &AuthorityDocumentV2, toml_raw: Option<&[u8]>) -> Result<()> {
    if let Some(expected) = &authority.debug_toml_sha256 {
        let toml_raw = toml_raw.context("v2 debug TOML hash present but MANIFEST.toml is missing")?;
        let actual = crate::hash::sha256(toml_raw);
        if &actual != expected {
            bail!("v2 MANIFEST.toml integrity check failed: expected {expected}, got {actual}");
        }
    }
    Ok(())
}

fn reject_install_authority_toml(toml_raw: Option<&[u8]>) -> Result<()> {
    let Some(toml_raw) = toml_raw else {
        return Ok(());
    };
    let toml_manifest = crate::ccs::manifest::CcsManifest::parse(
        std::str::from_utf8(toml_raw).context("decode v2 MANIFEST.toml as UTF-8")?,
    )
    .context("parse v2 MANIFEST.toml debug projection")?;
    if !toml_manifest.requires.packages.is_empty()
        || !toml_manifest.requires.capabilities.is_empty()
        || !toml_manifest.config.files.is_empty()
        || toml_manifest.hooks.has_script_hooks()
        || toml_manifest.hooks.has_declarative_hooks()
        || toml_manifest.scriptlets.has_capability_declarations()
        || toml_manifest.legacy_scriptlets.is_some()
        || !toml_manifest.components.overrides.is_empty()
        || !toml_manifest.components.files.is_empty()
    {
        bail!(
            "v2 MANIFEST.toml contains install-affecting fields; signed CBOR authority is required"
        );
    }
    Ok(())
}

fn verify_conversion_boundary_hash(
    authority: &AuthorityDocumentV2,
    boundary: Option<&crate::ccs::attestation::ForeignConversionBoundary>,
) -> Result<()> {
    if let Some(expected) = &authority.provenance.foreign_conversion_boundary_hash {
        let boundary = boundary.context(
            "v2 foreign conversion boundary hash present but MANIFEST.conversion-boundary.json is missing",
        )?;
        let actual = crate::ccs::attestation::canonical_json_hash(boundary)?;
        if &actual != expected {
            bail!("v2 foreign conversion boundary hash mismatch: expected {expected}, got {actual}");
        }
    }
    Ok(())
}
```

- [ ] **Step 3: Export the reader**

Before importing `verify_manifest_signature`, factor the existing private raw-manifest verifier in `ccs/verify.rs` into a `pub(crate)` helper. The helper must preserve `TrustPolicy::require_timestamp`, `max_signature_age`, algorithm checks, and trusted-key handling. V2 must call that helper with the exact archived `MANIFEST` bytes; do not reserialize `AuthorityDocumentV2` before verification. Add negative tests for modified `MANIFEST.sig` and unsupported signature algorithms.

In `v2/mod.rs`:

```rust
pub mod reader;
pub use reader::{ReadAuthorityV2, read_authority_document};
```

- [ ] **Step 4: Route archive reader by format version without v2 defaulting**

In `archive_reader.rs`, extend `CcsArchiveContents` with explicit v2 fields:

```rust
pub v2_authority: Option<crate::ccs::v2::AuthorityDocumentV2>,
pub v2_manifest_raw: Option<Vec<u8>>,
pub v2_build_attestation_raw: Option<String>,
pub v2_foreign_conversion_boundary_raw: Option<String>,
pub v2_build_attestation: Option<crate::ccs::attestation::BuildAttestationEnvelope>,
pub v2_foreign_conversion_boundary: Option<crate::ccs::attestation::ForeignConversionBoundary>,
```

Add a helper near manifest parsing:

```rust
fn cbor_format_version(raw: &[u8]) -> Option<u64> {
    #[derive(serde::Deserialize)]
    struct Header {
        format_version: u64,
    }
    ciborium::from_reader::<Header, _>(raw).ok().map(|header| header.format_version)
}
```

When `MANIFEST` is read, use `cbor_format_version(&content)`:

- `Some(2)`: decode `AuthorityDocumentV2` into a new `v2_authority` variable; do not call `BinaryManifest::from_cbor`.
- `Some(1)`: keep existing `BinaryManifest::from_cbor` behavior.
- any other version: keep raw bytes and fail during manifest resolution with a clear unsupported-format error.

Also recognize these v2 metadata entries:

- `MANIFEST.attestation.json`: parse as `BuildAttestationEnvelope`. This is a
  publication/verification envelope, not content identity authority. Retain the
  raw JSON string as `v2_build_attestation_raw` for `verify.rs`.
- `MANIFEST.conversion-boundary.json`: parse as `ForeignConversionBoundary` and,
  when `AuthorityDocumentV2.provenance.foreign_conversion_boundary_hash` is
  present, verify the canonical JSON hash matches that signed authority field.
  Retain the raw JSON string as `v2_foreign_conversion_boundary_raw` for
  `verify.rs`.

For M4a, `CcsArchiveContents.manifest` can remain the legacy `CcsManifest` for caller compatibility, but v2 callers must use `v2_authority`. In the v2 branch, do not reconstruct missing truth with `convert_binary_to_ccs_manifest`; instead create the minimal compatibility manifest only from v2 package identity for legacy callers and mark it as non-authoritative in comments.

`archive_reader.rs` does not own trust policy. It may decode and retain v2 authority for routing, but it must not claim the package is trusted. `verify.rs` is responsible for calling `ccs::v2::read_authority_document` with the caller's `TrustPolicy` before v2 authority can pass verification or feed a verified package parse.

- [ ] **Step 5: Add v2 payload verification in `verify.rs`**

Add a v2 content verifier in `crates/conary-core/src/ccs/verify.rs` and call it from `verify_package` whenever `contents.v2_authority` is present. The helper must verify archive payloads against signed v2 authority, not against unsigned `components/*.json` alone:

```rust
fn verify_v2_archive_payload(
    authority: &crate::ccs::v2::AuthorityDocumentV2,
    components: &std::collections::HashMap<String, crate::ccs::builder::ComponentData>,
    blobs: &std::collections::HashMap<String, Vec<u8>>,
) -> Result<ContentStatus> {
    use crate::ccs::builder::FileType as LegacyFileType;
    use crate::ccs::v2::schema::{FileTypeV2, PackageKindV2};

    let PackageKindV2::Package(data) = &authority.kind else {
        return Ok(ContentStatus::Skipped);
    };

    let mut errors = Vec::new();
    let signed_files: std::collections::HashSet<(&str, &str)> = data
        .files
        .iter()
        .map(|file| (file.component.as_str(), file.path.as_str()))
        .collect();
    for (component_name, component) in components {
        if !authority.components.contains_key(component_name) {
            errors.push(format!("v2 archive carries unsigned component {component_name}"));
        }
        for component_file in &component.files {
            if !signed_files.contains(&(component_name.as_str(), component_file.path.as_str())) {
                errors.push(format!(
                    "v2 archive carries unsigned file {} in component {}",
                    component_file.path, component_name
                ));
            }
        }
    }
    for file in &data.files {
        let component = match components.get(&file.component) {
            Some(component) => component,
            None => {
                errors.push(format!(
                    "v2 file {} references missing component {}",
                    file.path, file.component
                ));
                continue;
            }
        };
        let Some(component_file) = component.files.iter().find(|item| item.path == file.path) else {
            errors.push(format!(
                "v2 signed file {} missing from component {}",
                file.path, file.component
            ));
            continue;
        };
        let expected_type = match file.file_type {
            FileTypeV2::Regular => LegacyFileType::Regular,
            FileTypeV2::Directory => LegacyFileType::Directory,
            FileTypeV2::Symlink => LegacyFileType::Symlink,
        };
        if component_file.hash != file.sha256
            || component_file.size != file.size
            || component_file.mode != file.mode
            || component_file.file_type != expected_type
            || component_file.component != file.component
            || component_file.target != file.symlink_target
        {
            errors.push(format!(
                "v2 file authority mismatch for {} in component {}",
                file.path, file.component
            ));
        }
        if matches!(file.file_type, FileTypeV2::Regular) {
            match blobs.get(&file.sha256) {
                Some(content)
                    if crate::hash::sha256(content) == file.sha256
                        && content.len() as u64 == file.size => {}
                Some(content) if content.len() as u64 != file.size => {
                    errors.push(format!("v2 blob size mismatch for {}", file.path))
                }
                Some(_) => errors.push(format!("v2 blob hash mismatch for {}", file.path)),
                None => errors.push(format!("v2 blob missing for {}", file.path)),
            }
        } else if matches!(file.file_type, FileTypeV2::Symlink) && file.symlink_target.is_none() {
            errors.push(format!("v2 symlink target missing for {}", file.path));
        }
    }

    if errors.is_empty() {
        Ok(ContentStatus::Valid {
            files_checked: data.files.len(),
        })
    } else {
        Ok(ContentStatus::Invalid { errors })
    }
}
```

In `verify_package`, branch before the existing v1 Merkle-root path:

```rust
let verified_v2 = if let Some(raw_manifest) = contents.v2_manifest_raw.as_deref() {
    Some(crate::ccs::v2::read_authority_document(
        raw_manifest,
        contents.signature_raw.as_deref(),
        contents.toml_raw.as_deref(),
        contents.v2_build_attestation_raw.as_deref(),
        contents.v2_foreign_conversion_boundary_raw.as_deref(),
        policy,
    )?)
} else {
    None
};

let mut content_status = if let Some(verified) = verified_v2.as_ref() {
    verify_v2_archive_payload(&verified.authority, &contents.components, &contents.blobs)?
} else {
    verify_content_hashes(&files, &contents.blobs)?
};
```

Then run v1 Merkle-root verification only when `contents.binary_manifest` is present. A v2 package with a tampered blob, missing component, wrong component assignment, wrong mode, wrong path, extra unsigned component, or extra unsigned component file must produce `ContentStatus::Invalid`.

- [ ] **Step 6: Run reader tests**

```bash
cargo test -p conary-core ccs::v2::reader
cargo test -p conary-core ccs::archive_reader
cargo test -p conary-core ccs::verify
cargo fmt --check
```

Expected: pass. Existing v1 archive-reader tests should still pass.

- [ ] **Step 7: Commit**

```bash
git add crates/conary-core/src/ccs/archive_reader.rs crates/conary-core/src/ccs/verify.rs crates/conary-core/src/ccs/v2
git commit -m "feat(ccs): read signed v2 authority"
```

---

### Task 6: Add V2 Package Writer Path

**Files:**
- Create: `crates/conary-core/src/ccs/v2/test_support.rs`
- Modify: `crates/conary-core/src/ccs/builder.rs`
- Modify: `crates/conary-core/src/ccs/builder/package_writer.rs`
- Modify: `crates/conary-core/src/ccs/v2/mod.rs`
- Test: `crates/conary-core/src/ccs/builder.rs`
- Test: `crates/conary-core/src/ccs/builder/package_writer.rs`
- Test: `crates/conary-core/src/ccs/v2/test_support.rs`

- [ ] **Step 1: Add v2 fixture helper**

Create `test_support.rs`:

```rust
// conary-core/src/ccs/v2/test_support.rs

use super::schema::*;
use std::collections::BTreeMap;

pub(crate) fn package_authority_with_one_file(name: &str) -> AuthorityDocumentV2 {
    AuthorityDocumentV2::package_for_tests(name)
}

pub(crate) fn one_file_payloads_for_tests() -> BTreeMap<String, Vec<u8>> {
    BTreeMap::from([("/usr/bin/hello".to_string(), b"hello world\n".to_vec())])
}
```

Export only in tests:

```rust
#[cfg(test)]
pub(crate) mod test_support;
```

- [ ] **Step 2: Add writer test**

In `package_writer.rs`, add a test that writes a signed v2 fixture and reads it back through `read_ccs_archive`:

```rust
#[test]
fn write_v2_package_preserves_signed_authority() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("hello-v2.ccs");
    let authority = crate::ccs::v2::test_support::package_authority_with_one_file("hello-v2");
    let payloads = crate::ccs::v2::test_support::one_file_payloads_for_tests();
    let key = crate::ccs::signing::SigningKeyPair::generate();

    write_v2_ccs_package(
        &authority,
        &payloads,
        &path,
        &key,
        Some("[package]\nname = \"hello-v2\"\n"),
        None,
        None,
    )
    .unwrap();

    let contents = crate::ccs::archive_reader::read_ccs_archive(std::fs::File::open(path).unwrap())
        .unwrap();
    assert_eq!(
        contents.v2_authority.as_ref().unwrap().identity.name,
        "hello-v2"
    );
    assert!(contents.binary_manifest.is_none());
    assert_eq!(contents.components["main"].files.len(), 1);
    assert!(contents.blobs.contains_key(&crate::hash::sha256(b"hello world\n")));
}
```

- [ ] **Step 3: Implement a narrow production v2 writer**

In `package_writer.rs`, add a narrow production writer and re-export it from `ccs::builder` if needed by `publish_context.rs`. The writer must create a complete archive: signed v2 `MANIFEST`, `MANIFEST.sig`, optional metadata files, `components/*.json`, and `objects/{prefix}/{suffix}` payload blobs. It must verify every supplied regular-file payload against signed `FileAuthorityV2.sha256` and `size` before writing.

Update `crates/conary-core/src/ccs/builder.rs`:

```rust
pub use package_writer::{
    print_build_summary, write_ccs_package, write_signed_ccs_package, write_v2_ccs_package,
};
```

```rust
pub(crate) fn write_v2_ccs_package(
    authority: &crate::ccs::v2::AuthorityDocumentV2,
    payloads_by_path: &std::collections::BTreeMap<String, Vec<u8>>,
    output_path: &std::path::Path,
    signing_key: &super::super::signing::SigningKeyPair,
    debug_toml: Option<&str>,
    build_attestation: Option<&crate::ccs::attestation::BuildAttestationEnvelope>,
    foreign_conversion_boundary: Option<&crate::ccs::attestation::ForeignConversionBoundary>,
) -> anyhow::Result<()> {
    use anyhow::Context;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::fs;
    use tar::Builder;

    crate::ccs::v2::validate_authority(authority)
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    let temp_dir = tempfile::tempdir()?;
    let manifest_cbor = authority.to_cbor()?;
    fs::write(temp_dir.path().join("MANIFEST"), &manifest_cbor)?;
    if let Some(debug_toml) = debug_toml {
        fs::write(temp_dir.path().join("MANIFEST.toml"), debug_toml)?;
    }
    if let Some(build_attestation) = build_attestation {
        fs::write(
            temp_dir.path().join("MANIFEST.attestation.json"),
            serde_json::to_string_pretty(build_attestation)?,
        )?;
    }
    if let Some(foreign_conversion_boundary) = foreign_conversion_boundary {
        fs::write(
            temp_dir.path().join("MANIFEST.conversion-boundary.json"),
            serde_json::to_string_pretty(foreign_conversion_boundary)?,
        )?;
    }
    let signature = signing_key.sign(&manifest_cbor);
    fs::write(
        temp_dir.path().join("MANIFEST.sig"),
        serde_json::to_string_pretty(&signature)?,
    )?;
    let crate::ccs::v2::PackageKindV2::Package(data) = &authority.kind else {
        anyhow::bail!("M4a v2 writer only writes package payloads");
    };
    let mut components: std::collections::BTreeMap<String, crate::ccs::builder::ComponentData> =
        authority
            .components
            .keys()
            .map(|name| {
                (
                    name.clone(),
                    crate::ccs::builder::ComponentData {
                        name: name.clone(),
                        files: Vec::new(),
                        hash: String::new(),
                        size: 0,
                    },
                )
            })
            .collect();
    for file in &data.files {
        let entry = crate::ccs::builder::FileEntry {
            path: file.path.clone(),
            hash: file.sha256.clone(),
            size: file.size,
            mode: file.mode,
            component: file.component.clone(),
            file_type: match file.file_type {
                crate::ccs::v2::FileTypeV2::Regular => crate::ccs::builder::FileType::Regular,
                crate::ccs::v2::FileTypeV2::Directory => crate::ccs::builder::FileType::Directory,
                crate::ccs::v2::FileTypeV2::Symlink => crate::ccs::builder::FileType::Symlink,
            },
            target: file.symlink_target.clone(),
            chunks: None,
        };
        if matches!(file.file_type, crate::ccs::v2::FileTypeV2::Regular) {
            let payload = payloads_by_path
                .get(&file.path)
                .with_context(|| format!("missing payload for {}", file.path))?;
            if crate::hash::sha256(payload) != file.sha256 || payload.len() as u64 != file.size {
                anyhow::bail!("payload for {} does not match signed v2 authority", file.path);
            }
            let object_path = format!("objects/{}/{}", &file.sha256[..2], &file.sha256[2..]);
            fs::create_dir_all(temp_dir.path().join("objects").join(&file.sha256[..2]))?;
            fs::write(temp_dir.path().join(object_path), payload)?;
        }
        let component = components
            .get_mut(&file.component)
            .with_context(|| format!("missing component {}", file.component))?;
        component.size += file.size;
        component.files.push(entry);
    }
    for (name, component) in &mut components {
        component.hash = crate::hash::sha256_prefixed(
            &crate::ccs::attestation::canonical_json_bytes(&component.files)?,
        );
        fs::create_dir_all(temp_dir.path().join("components"))?;
        fs::write(
            temp_dir.path().join("components").join(format!("{name}.json")),
            serde_json::to_vec_pretty(component)?,
        )?;
    }
    let output_file = fs::File::create(output_path)?;
    let encoder = GzEncoder::new(output_file, Compression::default());
    let mut archive = Builder::new(encoder);
    archive.append_dir_all(".", temp_dir.path())?;
    archive.into_inner()?.finish()?;
    Ok(())
}
```

- [ ] **Step 4: Run writer and reader tests**

```bash
cargo test -p conary-core ccs::builder::package_writer::tests::write_v2_package_preserves_signed_authority
cargo test -p conary-core ccs::v2
cargo fmt --check
```

Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/conary-core/src/ccs/builder.rs crates/conary-core/src/ccs/builder/package_writer.rs crates/conary-core/src/ccs/v2
git commit -m "test(ccs): add signed v2 package fixture writer"
```

---

### Task 7: Implement Positive V2 Content Identity Projection

**Files:**
- Create: `crates/conary-core/src/ccs/v2/identity.rs`
- Modify: `crates/conary-core/src/ccs/v2/mod.rs`
- Modify: `crates/conary-core/src/ccs/attestation.rs`
- Test: `crates/conary-core/src/ccs/v2/identity.rs`
- Test: `crates/conary-core/src/ccs/attestation.rs`

- [ ] **Step 1: Write identity tests**

Create `identity.rs` tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resigning_does_not_change_identity() {
        let authority = crate::ccs::v2::test_support::package_authority_with_one_file("id");
        let first = compute_v2_content_identity(&authority).unwrap();
        let second = compute_v2_content_identity(&authority).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn authority_changes_change_identity() {
        let mut authority = crate::ccs::v2::test_support::package_authority_with_one_file("id");
        let first = compute_v2_content_identity(&authority).unwrap();
        authority.requires.push(crate::ccs::v2::schema::DependencyEntryV2::package("openssl"));
        let second = compute_v2_content_identity(&authority).unwrap();
        assert_ne!(first, second);
    }
}
```

- [ ] **Step 2: Implement positive projection**

```rust
// conary-core/src/ccs/v2/identity.rs

use super::schema::*;
use anyhow::Result;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct ContentIdentityProjectionV2<'a> {
    pub identity: &'a PackageIdentityV2,
    pub kind: &'a PackageKindV2,
    pub provides: &'a [DependencyEntryV2],
    pub requires: &'a [DependencyEntryV2],
    pub components: &'a std::collections::BTreeMap<String, ComponentAuthorityV2>,
    pub lifecycle: &'a LifecycleAuthorityV2,
    pub provenance: &'a ProvenanceAuthorityV2,
}

pub fn compute_v2_content_identity(authority: &AuthorityDocumentV2) -> Result<String> {
    let projection = ContentIdentityProjectionV2 {
        identity: &authority.identity,
        kind: &authority.kind,
        provides: &authority.provides,
        requires: &authority.requires,
        components: &authority.components,
        lifecycle: &authority.lifecycle,
        provenance: &authority.provenance,
    };
    let bytes = crate::ccs::attestation::canonical_json_bytes(&projection)?;
    Ok(crate::hash::sha256_prefixed(&bytes))
}

pub fn compute_v2_file_merkle_root(authority: &AuthorityDocumentV2) -> Result<String> {
    let PackageKindV2::Package(data) = &authority.kind else {
        return Ok(crate::hash::sha256_prefixed(
            &crate::ccs::attestation::canonical_json_bytes(&authority.kind)?,
        ));
    };
    let bytes = crate::ccs::attestation::canonical_json_bytes(&(
        &authority.components,
        &data.files,
    ))?;
    Ok(crate::hash::sha256_prefixed(&bytes))
}
```

This projection intentionally excludes `MANIFEST.sig`, `debug_toml_sha256`, upload/staging metadata, and serving metadata.

- [ ] **Step 3: Export identity**

In `v2/mod.rs`:

```rust
pub mod identity;
pub use identity::{
    ContentIdentityProjectionV2, compute_v2_content_identity, compute_v2_file_merkle_root,
};
```

- [ ] **Step 4: Route production build-output identity through v2 authority**

In `attestation.rs`, keep the legacy projection for v1 packages, but branch at the top of `compute_build_output_identity` when `package.v2_authority()` is present:

```rust
pub fn compute_v2_content_identity(
    authority: &crate::ccs::v2::AuthorityDocumentV2,
) -> Result<String> {
    crate::ccs::v2::compute_v2_content_identity(authority)
}

pub fn compute_v2_file_merkle_root(
    authority: &crate::ccs::v2::AuthorityDocumentV2,
) -> Result<String> {
    crate::ccs::v2::compute_v2_file_merkle_root(authority)
}

pub fn compute_build_output_identity(
    package: &crate::ccs::package::CcsPackage,
) -> Result<BuildOutputIdentity> {
    if let Some(authority) = package.v2_authority() {
        let provenance = &authority.provenance;
        return Ok(BuildOutputIdentity {
            file_merkle_root: compute_v2_file_merkle_root(authority)?,
            package_name: authority.identity.name.clone(),
            package_version: authority.identity.version.clone(),
            package_release: authority.identity.release.clone(),
            architecture: authority.identity.architecture.clone(),
            origin_class: provenance
                .origin_class
                .clone()
                .context("v2 build output identity requires origin_class")?,
            hardening_level: provenance
                .hardening_level
                .clone()
                .context("v2 build output identity requires hardening_level")?,
            hermetic_evidence_hash: provenance
                .hermetic_evidence_hash
                .clone()
                .context("v2 build output identity requires hermetic_evidence_hash")?,
            canonical_content_identity: compute_v2_content_identity(authority)?,
        });
    }

    // Existing v1/legacy implementation remains below.
}
```

Do not remove `compute_content_identity_excluding_signatures` yet; it remains the legacy/v1 projection until all callers move. The v2 positive publish-gate test in Task 9 must pass without a v1 `BinaryManifest`, without stuffing fake v1 Merkle or provenance fields into the compatibility manifest, and without weakening `verify_static_attestation`.

- [ ] **Step 5: Run tests**

```bash
cargo test -p conary-core ccs::v2::identity
cargo test -p conary-core ccs::attestation
cargo fmt --check
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add crates/conary-core/src/ccs/v2 crates/conary-core/src/ccs/attestation.rs
git commit -m "feat(ccs): add v2 content identity projection"
```

---

### Task 8: Route Install-Facing Package Adapter Away From V1 Defaults

**Files:**
- Modify: `crates/conary-core/src/ccs/package.rs`
- Modify: `crates/conary-core/src/ccs/archive_reader.rs`
- Modify: `crates/conary-core/src/repository/static_repo/publish_gate.rs`
- Test: `crates/conary-core/src/ccs/package.rs`
- Test: `crates/conary-core/src/ccs/archive_reader.rs`
- Test: `crates/conary-core/src/repository/static_repo/publish_gate.rs`

- [ ] **Step 1: Add regression tests proving v2 does not call v1 default reconstruction**

In `package.rs` tests, add:

```rust
#[test]
fn v2_packages_do_not_use_binary_manifest_default_reconstruction() {
    let authority = crate::ccs::v2::test_support::package_authority_with_one_file("adapter-v2");
    let package = CcsPackage::from_v2_authority_for_tests(authority.clone(), None, None).unwrap();
    assert_eq!(package.manifest().package.name, "adapter-v2");
    assert!(package.binary_manifest().is_none());
    assert!(package.v2_authority().is_some());
}

#[test]
fn v2_compatibility_manifest_preserves_attestation_metadata() {
    let authority = crate::ccs::v2::test_support::package_authority_with_one_file("attested-v2");
    let key = crate::ccs::signing::SigningKeyPair::generate().with_key_id("publish");
    let envelope = crate::ccs::attestation::test_support::sample_envelope_for_tests(&key);
    let package =
        CcsPackage::from_v2_authority_for_tests(authority, Some(envelope.clone()), None).unwrap();
    let provenance = package.manifest().provenance.as_ref().unwrap();
    assert_eq!(provenance.build_attestation.as_ref(), Some(&envelope));
}
```

In `archive_reader.rs` tests, add:

```rust
#[test]
fn v1_manifest_is_legacy_not_v2_authority() {
    let (_temp, path) = build_test_package();
    let file = std::fs::File::open(&path).unwrap();
    let contents = read_ccs_archive(file).unwrap();
    assert!(contents.binary_manifest.is_some());
    assert!(contents.v2_authority.is_none());
}
```

- [ ] **Step 2: Add v2 compatibility adapter**

In `package.rs`, extend `CcsPackage` so v2 authority is retained outside the legacy compatibility manifest:

```rust
/// Parsed v2 authority, when this package is native CCS v2.
v2_authority: Option<crate::ccs::v2::AuthorityDocumentV2>,
/// Parsed v2 build attestation envelope from MANIFEST.attestation.json.
v2_build_attestation: Option<crate::ccs::attestation::BuildAttestationEnvelope>,
/// Parsed v2 foreign conversion boundary from MANIFEST.conversion-boundary.json.
v2_foreign_conversion_boundary: Option<crate::ccs::attestation::ForeignConversionBoundary>,
```

Add accessors:

```rust
pub fn v2_authority(&self) -> Option<&crate::ccs::v2::AuthorityDocumentV2> {
    self.v2_authority.as_ref()
}
```

Then add a private conversion from v2 authority into the compatibility fields still required by `PackageFormat` and the existing M2 publish gate:

```rust
fn compatibility_manifest_from_v2(
    authority: &crate::ccs::v2::AuthorityDocumentV2,
    build_attestation: Option<crate::ccs::attestation::BuildAttestationEnvelope>,
    foreign_conversion_boundary: Option<crate::ccs::attestation::ForeignConversionBoundary>,
) -> crate::Result<CcsManifest> {
    let mut manifest = CcsManifest::new_minimal(
        &authority.identity.name,
        &authority.identity.version,
    );
    manifest.package.description = format!("CCS v2 {}", authority.identity.name);
    let provenance = manifest.provenance.get_or_insert_with(Default::default);
    provenance.origin_class = authority.provenance.origin_class.clone();
    provenance.hardening_level = authority.provenance.hardening_level.clone();
    provenance.build_attestation = build_attestation;
    provenance.foreign_conversion_boundary = foreign_conversion_boundary;
    Ok(manifest)
}
```

Add the test-only constructor used above:

```rust
#[cfg(test)]
impl CcsPackage {
    pub(crate) fn from_v2_authority_for_tests(
        authority: crate::ccs::v2::AuthorityDocumentV2,
        build_attestation: Option<crate::ccs::attestation::BuildAttestationEnvelope>,
        foreign_conversion_boundary: Option<crate::ccs::attestation::ForeignConversionBoundary>,
    ) -> crate::Result<Self> {
        let manifest = compatibility_manifest_from_v2(
            &authority,
            build_attestation.clone(),
            foreign_conversion_boundary.clone(),
        )?;
        let files = files_from_v2_authority(&authority)?;
        let dependencies = dependencies_from_v2_authority(&authority);
        let package_files = Self::convert_files(&files);
        Ok(Self {
            package_path: std::path::PathBuf::from("v2-test.ccs"),
            manifest,
            binary_manifest: None,
            v2_authority: Some(authority),
            v2_build_attestation: build_attestation,
            v2_foreign_conversion_boundary: foreign_conversion_boundary,
            files,
            components: std::collections::HashMap::new(),
            package_files,
            dependencies,
            config_files_cache: Vec::new(),
        })
    }
}
```

- [ ] **Step 3: Add concrete v2 file and dependency mapping**

In `package.rs`, add mapping helpers. `FileAuthorityV2.path`, `sha256`, `size`, `mode`, `component`, `file_type`, and `symlink_target` map into legacy `FileEntry`; `owner`, `group`, `config`, and `conflict` remain in retained v2 authority and must not be silently discarded from v2 validation. The verified parse path below only calls these helpers after `verify_v2_archive_payload` has checked regular blob hash/size and symlink metadata against signed v2 authority. `DependencyEntryV2::Package` and `DependencyEntryV2::Capability` map into existing runtime package/capability dependency entries; other typed entries remain v2 authority for later resolver work.

```rust
fn files_from_v2_authority(
    authority: &crate::ccs::v2::AuthorityDocumentV2,
) -> crate::Result<Vec<crate::ccs::builder::FileEntry>> {
    use crate::ccs::builder::{FileEntry, FileType};
    use crate::ccs::v2::schema::{FileTypeV2, PackageKindV2};

    let PackageKindV2::Package(data) = &authority.kind else {
        return Err(crate::error::Error::ParseError(
            "group and redirect v2 packages are not installable in M4a".to_string(),
        ));
    };

    Ok(data
        .files
        .iter()
        .map(|file| FileEntry {
            path: file.path.clone(),
            hash: file.sha256.clone(),
            size: file.size,
            mode: file.mode,
            component: file.component.clone(),
            file_type: match file.file_type {
                FileTypeV2::Regular => FileType::Regular,
                FileTypeV2::Directory => FileType::Directory,
                FileTypeV2::Symlink => FileType::Symlink,
            },
            target: file.symlink_target.clone(),
            chunks: None,
        })
        .collect())
}

fn dependencies_from_v2_authority(
    authority: &crate::ccs::v2::AuthorityDocumentV2,
) -> Vec<crate::packages::traits::Dependency> {
    use crate::ccs::v2::schema::DependencyKindV2;
    use crate::packages::traits::{Dependency, DependencyType};

    authority
        .requires
        .iter()
        .filter_map(|dependency| match dependency.kind {
            DependencyKindV2::Package => Some(Dependency {
                name: dependency.name.clone(),
                version: dependency.version_constraint.clone(),
                dep_type: DependencyType::Runtime,
                description: None,
            }),
            DependencyKindV2::Capability => Some(Dependency {
                name: format!("capability:{}", dependency.name),
                version: dependency.version_constraint.clone(),
                dep_type: DependencyType::Runtime,
                description: None,
            }),
            _ => None,
        })
        .collect()
}
```

- [ ] **Step 4: Add an explicit verified-v2 parse boundary**

Inside the `PackageFormat::parse` implementation, immediately after `let contents = read_ccs_archive(file)...`, reject native v2 before `let manifest = &contents.manifest`:

```rust
if contents.v2_authority.is_some() {
    return Err(crate::error::Error::ParseError(
        "native CCS v2 packages require verified parsing; call CcsPackage::parse_verified_v2"
            .to_string(),
    ));
}
```

Then add an explicit constructor for callers that have already verified the archive:

```rust
pub fn parse_verified_v2(
    path: &str,
    verification: &crate::ccs::verify::VerificationResult,
) -> crate::Result<Self> {
    if !verification.valid || !matches!(
        verification.content_status,
        crate::ccs::verify::ContentStatus::Valid { .. }
    ) {
        return Err(crate::error::Error::ParseError(
            "native CCS v2 package did not pass signature and payload verification".to_string(),
        ));
    }
    let package_path = std::path::PathBuf::from(path);
    let file = std::fs::File::open(&package_path)?;
    let contents = crate::ccs::archive_reader::read_ccs_archive(file)
        .map_err(|error| crate::error::Error::ParseError(error.to_string()))?;
    let Some(authority) = contents.v2_authority.as_ref() else {
        return <Self as crate::packages::traits::PackageFormat>::parse(path);
    };
    let manifest = compatibility_manifest_from_v2(
        authority,
        contents.v2_build_attestation.clone(),
        contents.v2_foreign_conversion_boundary.clone(),
    )?;
    let files = files_from_v2_authority(authority)?;
    let dependencies = dependencies_from_v2_authority(authority);
    let package_files = Self::convert_files(&files);
    return Ok(Self {
        package_path,
        manifest,
        binary_manifest: None,
        v2_authority: Some(authority.clone()),
        v2_build_attestation: contents.v2_build_attestation,
        v2_foreign_conversion_boundary: contents.v2_foreign_conversion_boundary,
        files,
        components: contents.components,
        package_files,
        dependencies,
        config_files_cache: Vec::new(),
    })
}
```

Do not call `convert_binary_to_ccs_manifest` in the verified-v2 branch. Group/redirect install attempts return the `ParseError` from `files_from_v2_authority`.

Update `verify_static_artifact_publish_eligibility` so it verifies first, then calls `CcsPackage::parse_verified_v2(...)` when `read_ccs_archive` reports `v2_authority`, and otherwise keeps the legacy `CcsPackage::parse` flow. Add a unit test proving plain `CcsPackage::parse` rejects a signed v2 package and `parse_verified_v2` accepts the same package only with a successful `VerificationResult`.

- [ ] **Step 5: Run adapter tests**

```bash
cargo test -p conary-core ccs::package
cargo test -p conary-core ccs::archive_reader
cargo fmt --check
```

Expected: pass.

- [ ] **Step 6: Verify `convert_binary_to_ccs_manifest` is still v1-only**

Run:

```bash
rg -n "convert_binary_to_ccs_manifest" crates/conary-core/src/ccs
```

Expected: no use from `ccs/v2`; any remaining uses are legacy v1 archive routing or tests.

- [ ] **Step 7: Commit**

```bash
git add crates/conary-core/src/ccs/package.rs crates/conary-core/src/ccs/archive_reader.rs crates/conary-core/src/repository/static_repo/publish_gate.rs
git commit -m "feat(ccs): route package adapter through v2 authority"
```

---

### Task 9: Preserve Publish-Gate And Verification Regressions

**Files:**
- Modify: `crates/conary-core/src/repository/static_repo/publish_gate.rs`
- Modify: `crates/conary-core/src/repository/static_repo/publish_context.rs`
- Modify: `crates/conary-core/src/ccs/verify.rs`
- Modify: `apps/conary/src/commands/ccs/install/command.rs`
- Create: `apps/conary/tests/packaging_m4a.rs`
- Test: `crates/conary-core/src/repository/static_repo/publish_gate.rs`
- Test: `crates/conary-core/src/ccs/verify.rs`
- Test: `apps/conary/tests/packaging_m4a.rs`

- [ ] **Step 1: Add publish-gate regression assertions**

In `publish_gate.rs` tests, add or extend tests so active emitted failure codes remain covered:

```rust
#[test]
fn m4a_preserves_active_publish_gate_failure_codes() {
    let active = active_failure_codes_for_tests();
    for expected in [
        PublishGateFailureCode::MissingAttestation,
        PublishGateFailureCode::BuildAttestationSignatureMismatch,
        PublishGateFailureCode::PackageSignatureMismatch,
        PublishGateFailureCode::TomlIntegrityMismatch,
        PublishGateFailureCode::OutputIdentityMismatch,
        PublishGateFailureCode::UnacceptedSignerKey,
        PublishGateFailureCode::NonHermeticHardeningLevel,
        PublishGateFailureCode::StaleOrUnknownPolicy,
        PublishGateFailureCode::UncleanCommandRiskReport,
        PublishGateFailureCode::ForeignConversionMissingBoundary,
        PublishGateFailureCode::ForeignConversionBoundaryHashMismatch,
        PublishGateFailureCode::RecordedDraftArtifact,
    ] {
        assert!(active.contains(&expected), "missing active publish gate code {expected:?}");
    }
}

#[test]
fn m4a_preserves_reserved_publish_gate_mappings() {
    let reserved = [
        PublishGateFailureCode::RetiredSignerKey,
        PublishGateFailureCode::AbsentOrUnknownProvenanceClass,
    ];
    assert_eq!(reserved.len(), 2);
}
```

If no helper exists, create `active_failure_codes_for_tests()` inside the test module by constructing the existing fixture cases that already emit those codes. Do not claim `RetiredSignerKey` or `AbsentOrUnknownProvenanceClass` are actively emitted unless implementation adds real emission.

- [ ] **Step 2: Add positive v2 publish-gate proof**

In `publish_gate.rs`, add a v2 equivalent of `artifact_gate_accepts_attested_hermetic_package`:

```rust
#[test]
fn artifact_gate_accepts_attested_v2_package() {
    let signer = SigningKeyPair::generate().with_key_id("publish");
    let temp = tempfile::tempdir().unwrap();
    let package_path = temp.path().join("attested-v2.ccs");
    let authority = crate::ccs::v2::test_support::package_authority_with_one_file("attested-v2");
    let payloads = crate::ccs::v2::test_support::one_file_payloads_for_tests();
    let envelope =
        crate::ccs::attestation::test_support::sample_v2_envelope_for_tests(
            &authority,
            &signer,
            STATIC_PUBLISH_POLICY_DIGEST_V1,
        );
    crate::ccs::builder::write_v2_ccs_package(
        &authority,
        &payloads,
        &package_path,
        &signer,
        None,
        Some(&envelope),
        None,
    )
    .unwrap();

    let report = verify_static_artifact_publish_eligibility(
        &package_path,
        &accepted_signers_for_key(&signer),
        STATIC_PUBLISH_POLICY_DIGEST_V1,
    )
    .unwrap();

    assert!(report.is_passed(), "{report:?}");
}
```

Add `sample_v2_envelope_for_tests` in `ccs/attestation.rs` test support. It must build `BuildOutputIdentity` from signed v2 authority using `compute_v2_content_identity(authority)` and `compute_v2_file_merkle_root(authority)`, copy name/version/release/architecture/origin/hardening/hermetic evidence from v2 authority, and sign with the requested policy digest. Do not weaken the publish gate to make this test pass.

- [ ] **Step 3: Add v2 verification regression**

Create `apps/conary/tests/packaging_m4a.rs` with a focused integration test:

```rust
use conary_core::ccs::v2::schema::{
    AuthorityDocumentV2, ComponentAuthorityV2, ConflictPolicyV2, FileAuthorityV2, FileTypeV2,
    LifecycleAuthorityV2, PackageDataV2, PackageIdentityV2, PackageKindTagV2, PackageKindV2,
    ProvenanceAuthorityV2, FORMAT_VERSION_V2,
};
use conary_core::ccs::verify::{TrustPolicy, verify_package};
use flate2::Compression;
use flate2::write::GzEncoder;
use std::collections::BTreeMap;
use std::fs;
use tar::Builder;

#[test]
fn v2_package_verification_rejects_unsigned_manifest() {
    let temp = tempfile::tempdir().unwrap();
    let package_path = temp.path().join("unsigned-v2.ccs");

    let file = FileAuthorityV2 {
        path: "/usr/bin/hello".to_string(),
        sha256: conary_core::hash::sha256(b"hello world\n"),
        size: 12,
        file_type: FileTypeV2::Regular,
        mode: 0o755,
        owner: "root".to_string(),
        group: "root".to_string(),
        component: "main".to_string(),
        symlink_target: None,
        config: None,
        conflict: ConflictPolicyV2::Error,
    };
    let authority = AuthorityDocumentV2 {
        format_version: FORMAT_VERSION_V2,
        identity: PackageIdentityV2 {
            name: "unsigned-v2".to_string(),
            version: "1.0.0".to_string(),
            release: "1".to_string(),
            architecture: Some("x86_64".to_string()),
            platform: Some("linux".to_string()),
            kind: PackageKindTagV2::Package,
        },
        kind: PackageKindV2::Package(PackageDataV2 {
            files: vec![file],
            config: Vec::new(),
            policy: Default::default(),
        }),
        provides: Vec::new(),
        requires: Vec::new(),
        components: BTreeMap::from([(
            "main".to_string(),
            ComponentAuthorityV2 {
                name: "main".to_string(),
                default: true,
                file_count: 1,
                total_size: 12,
            },
        )]),
        lifecycle: LifecycleAuthorityV2::default(),
        provenance: ProvenanceAuthorityV2 {
            origin_class: Some("native-built".to_string()),
            hardening_level: Some("hermetic".to_string()),
            build_input_identity: Some("sha256:build-input".to_string()),
            hermetic_evidence_hash: Some("sha256:evidence".to_string()),
            foreign_conversion_boundary_hash: None,
        },
        debug_toml_sha256: None,
    };

    let manifest_cbor = authority.to_cbor().unwrap();
    let output = fs::File::create(&package_path).unwrap();
    let encoder = GzEncoder::new(output, Compression::default());
    let mut archive = Builder::new(encoder);
    let mut header = tar::Header::new_gnu();
    header.set_size(manifest_cbor.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    archive
        .append_data(&mut header, "MANIFEST", manifest_cbor.as_slice())
        .unwrap();
    archive.into_inner().unwrap().finish().unwrap();

    let error = verify_package(&package_path, &TrustPolicy::strict(Vec::new())).unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("MANIFEST.sig") || message.contains("not signed"),
        "unexpected unsigned v2 verification error: {message}"
    );
}
```

This test is expected to fail until `verify.rs` routes v2 packages through `read_authority_document`. The failure proves `verify_package` does not silently accept unsigned v2 manifests.

Also update `apps/conary/src/commands/ccs/install/command.rs` so native v2 cannot use the legacy unsigned bypass:

- if `--allow-unsigned` is set, read the archive header/manifest with `read_ccs_archive`; when `v2_authority` is present, return an error that native v2 requires strict signature verification;
- when verification is required and `verify_package` succeeds, construct native v2 packages with `CcsPackage::parse_verified_v2(package, &result)` instead of plain `CcsPackage::parse`;
- keep the legacy `CcsPackage::parse` path only for non-v2 packages.

Add an integration test in `apps/conary/tests/packaging_m4a.rs` named `v2_install_refuses_allow_unsigned_bypass`. It should build the unsigned v2 fixture above and assert `conary ccs install --allow-unsigned ...` fails before parsing or extraction with a message naming native v2 signature verification.

- [ ] **Step 4: Add v2 payload tamper regression**

In `crates/conary-core/src/ccs/verify.rs` tests, add a v2 package tamper test:

```rust
#[test]
fn verify_v2_package_rejects_payload_not_in_signed_authority() {
    let temp = tempfile::tempdir().unwrap();
    let package_path = temp.path().join("tampered-v2.ccs");
    let signer = crate::ccs::signing::SigningKeyPair::generate();
    let authority = crate::ccs::v2::test_support::package_authority_with_one_file("tampered-v2");
    let payloads = crate::ccs::v2::test_support::one_file_payloads_for_tests();
    crate::ccs::builder::write_v2_ccs_package(
        &authority,
        &payloads,
        &package_path,
        &signer,
        None,
        None,
        None,
    )
    .unwrap();

    let tampered_path = temp.path().join("tampered-v2-extra.ccs");
    crate::ccs::v2::test_support::rewrite_v2_archive_for_tests(
        &package_path,
        &tampered_path,
        |entries| {
            entries.insert(
                "components/extra.json".to_string(),
                br#"{"name":"extra","files":[],"hash":"sha256:extra","size":0}"#.to_vec(),
            );
        },
    )
    .unwrap();

    let policy = TrustPolicy::strict(vec![signer.public_key_base64()]);
    let result = verify_package(&tampered_path, &policy).unwrap();
    assert!(!result.valid);
    assert!(matches!(result.content_status, ContentStatus::Invalid { .. }));
}
```

Add `rewrite_v2_archive_for_tests` to `ccs/v2/test_support.rs` as a small tar.gz read/rewrite helper modeled after `ccs/builder/test_support.rs::rewrite_manifest_toml_for_tests`, but operating on a mutable `BTreeMap<String, Vec<u8>>` of archive entries.

- [ ] **Step 5: Update `publish_context.rs` v2 attestation path**

Update `attach_project_form_attestation` and `build_result_from_package_with_attestation` so v2 packages are re-emitted as v2 when attaching project-form attestations. The v2 branch must:

- read `package.v2_authority()`;
- compute the v2 content identity with `compute_v2_content_identity`;
- build the `BuildAttestationEnvelope` with output identity matching the v2 authority;
- convert `package.extract_all_content()`'s hash-keyed blobs back into `payloads_by_path` by walking signed `FileAuthorityV2` entries and looking up each regular file's `sha256`;
- write the package through the Task 6 `write_v2_ccs_package` production API, including `MANIFEST.attestation.json` and the original verified payload bytes;
- never rebuild the artifact through the v1 `BinaryManifest` writer.

If `attach_project_form_attestation` cannot preserve the original v2 payload blobs directly from `package.extract_all_content()`, stop and add that read path explicitly; do not round-trip through `BuildResult`/v1 package writing.

- [ ] **Step 6: Run focused regression tests**

```bash
cargo test -p conary-core repository::static_repo::publish_gate
cargo test -p conary-core ccs::verify
cargo test -p conary --test packaging_m4a
cargo test -p conary --test packaging_m2a
cargo fmt --check
```

Expected: pass.

- [ ] **Step 7: Commit**

```bash
git add crates/conary-core/src/repository/static_repo/publish_gate.rs crates/conary-core/src/repository/static_repo/publish_context.rs crates/conary-core/src/ccs/verify.rs apps/conary/src/commands/ccs/install/command.rs apps/conary/tests/packaging_m4a.rs
git commit -m "test(ccs): preserve v2 publish and verify gates"
```

---

### Task 10: Docs, Fixture Classification, And Final Verification

**Files:**
- Modify: `docs/modules/ccs.md`
- Modify: `docs/modules/test-fixtures.md`
- Modify: `docs/modules/feature-ownership.md`
- Modify: `docs/llms/subsystem-map.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`

- [ ] **Step 1: Update CCS docs after behavior lands**

In `docs/modules/ccs.md`, add a short v2 section that states:

```markdown
## CCS v2 Native Authority

CCS v2 packages use the CBOR `MANIFEST` with `format_version = 2` as signed
install-time authority. `MANIFEST.toml` may be present for source/debug
visibility, but TOML-only install behavior is not native authority. The v2
implementation lives under `crates/conary-core/src/ccs/v2/`; legacy v1
`BinaryManifest` parsing remains a migration/fixture surface.
```

- [ ] **Step 2: Update fixture docs**

In `docs/modules/test-fixtures.md`, add a CCS v2 fixture row that distinguishes:

```markdown
- v2 native fixtures: signed `format_version = 2` authority with complete file,
  component, dependency, provenance, TOML-debug-hash, and content-identity
  coverage.
- legacy rejection fixtures: v1 `BinaryManifest` packages and CBOR-only
  default-reconstruction packages that prove fail-closed diagnostics.
```

- [ ] **Step 3: Update assistant routing docs**

In `docs/modules/feature-ownership.md` and `docs/llms/subsystem-map.md`, route future CCS v2 package contract work to:

```markdown
Start in `crates/conary-core/src/ccs/v2/` for v2 authority, validation,
diagnostics, archive reading, and content identity. Use `archive_reader.rs` and
`package.rs` only as version-routing/adaptation surfaces.
```

- [ ] **Step 4: Check feature-coherency ledger rows before editing public claims**

Run:

```bash
rg -n "docs/modules/ccs.md|docs/modules/test-fixtures.md|docs/modules/feature-ownership.md|docs/llms/subsystem-map.md|crates/conary-core/src/ccs|apps/conary/src/commands/ccs/install/command.rs|crates/conary-core/src/repository/static_repo" docs/superpowers/feature-coherency-ledger.tsv
```

If rows match touched paths or public claims, update the relevant claim evidence and rerun the coherency checks below. If no rows match, record that in the Task 10 commit notes.

- [ ] **Step 5: Refresh docs-audit metadata**

Run:

```bash
bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
```

If the diff shows added/changed rows, update `docs/superpowers/documentation-accuracy-audit-inventory.tsv` to match the generated output and add or update ledger rows for every public doc touched.

- [ ] **Step 6: Run full focused verification**

```bash
cargo test -p conary-core ccs
cargo test -p conary-core repository::static_repo::publish_gate
cargo test -p conary --lib commands::publish
cargo test -p conary --test packaging_m2a
cargo test -p conary --test packaging_m4a
cargo test -p remi
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/check-doc-truth.sh
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
bash scripts/check-coherency-wave-scopes.sh docs/superpowers/feature-coherency-ledger.tsv docs/superpowers/feature-coherency-wave-scopes.tsv
git diff --check
cargo fmt --check
```

Expected: all pass.

- [ ] **Step 7: Run merge-gate verification**

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: pass.

- [ ] **Step 8: Commit**

```bash
git add docs/modules/ccs.md docs/modules/test-fixtures.md docs/modules/feature-ownership.md docs/llms/subsystem-map.md docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv
git commit -m "docs(ccs): document v2 authority routing"
```

## Final Acceptance Gate

Before merging the implementation branch, prove:

```bash
cargo test -p conary-core ccs
cargo test -p conary-core repository::static_repo::publish_gate
cargo test -p conary --lib commands::publish
cargo test -p conary --test packaging_m2a
cargo test -p conary --test packaging_m4a
cargo test -p remi
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/check-doc-truth.sh
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
bash scripts/check-coherency-wave-scopes.sh docs/superpowers/feature-coherency-ledger.tsv docs/superpowers/feature-coherency-wave-scopes.tsv
git diff --check
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: all commands pass. Any failure blocks merge until the plan or implementation is corrected.

## Self-Review Notes

- The plan starts with a concrete v1 inventory and module seed, as required by the design.
- v2 authority stays under `ccs/v2/`; existing `manifest.rs`, `binary_manifest.rs`, `archive_reader.rs`, and `package.rs` are transition surfaces.
- Detached signatures cover exact archived CBOR bytes, not reserialized CBOR.
- `PackageFormat::parse` does not silently construct native v2 packages; verified v2 construction requires a successful verification result.
- TOML debug hash is checked for drift but never becomes install-time authority.
- v2 fixtures and project-form attestation refresh use a complete v2 writer with component JSON and object blobs.
- `package`, `group`, and `redirect` have explicit validation paths.
- `convert_binary_to_ccs_manifest` remains legacy/v1-only.
- M2 publish-gate regression and reserved diagnostic mappings are covered.
- M4b authoring UX, Remi intake, M4d profile facts, and distro expansion remain out of scope.
