# Audit Hardening Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the highest-confidence code-audit findings around critical package blocking, composefs/fs-verity trust behavior, conaryd API honesty, and distro/version inference drift.

**Architecture:** Make safety decisions single-source and explicit. Shared policy moves into `conary-core`; app crates become thin consumers. Runtime activation paths fail closed when integrity metadata says fs-verity should be enforced, and incomplete daemon APIs report their incomplete status directly.

**Tech Stack:** Rust, Cargo workspace tests, rusqlite-backed unit tests, existing source-contract tests under `crates/conary-core/tests/`.

---

## Scope

This plan covers four shippable chunks:

1. Shared critical package and runtime capability blocklist.
2. composefs/fs-verity fail-closed behavior and visible `/etc` overlay warnings.
3. conaryd package-operation route honesty.
4. Shared distro/flavor/version-scheme inference.

Supported distro scope for this plan is Fedora 44, Ubuntu LTS 26.04, and Arch. Do not broaden user-facing distro support while doing inference cleanup. Short family labels such as `fedora`, `ubuntu`, and `arch` may remain only where existing repository/Remi internals already use family-level identifiers.

This plan deliberately does not refactor Remi's `spawn_blocking` + `Handle::block_on` architecture. That should be a separate design because it touches repository sync, HTTP, database ownership, and admin API concurrency.

## File Map

- Create: `crates/conary-core/src/critical_packages.rs`
  - Canonical critical package names and runtime capability guards.
- Modify: `crates/conary-core/src/lib.rs`
  - Export `critical_packages`.
- Modify: `apps/conary/src/commands/install/blocklist.rs`
  - Replace local constants with thin wrappers around `conary_core::critical_packages`.
- Modify: `apps/remi/src/server/conversion.rs`
  - Remove duplicate blocklist, call shared core policy, and reject parsed packages that provide critical runtime capabilities.
- Modify: `crates/conary-core/tests/generation_composefs_runtime_contract.rs`
  - Add source-contract coverage for no silent verity downgrade in generation switching and stderr-visible `/etc` overlay warnings in both activation paths.
- Modify: `apps/conary/src/commands/generation/switch.rs`
  - Fail closed on verity mount failure instead of retrying plain composefs, and print `/etc` overlay warnings to stderr.
- Modify: `apps/conary/src/commands/composefs_ops.rs`
  - Print `/etc` overlay warning to stderr as well as logs.
- Modify: `apps/conaryd/src/daemon/mod.rs`
  - Update module docs to describe current daemon support accurately.
- Modify: `apps/conaryd/src/daemon/routes/transactions.rs`
  - Return `501 Not Implemented` directly for install/remove/update package endpoints while retaining enhance support.
- Modify: `apps/conaryd/src/daemon/routes.rs`
  - Update tests and response expectations.
- Create or Modify: `crates/conary-core/src/repository/distro.rs`
  - Shared distro-name, repository, flavor, and version-scheme inference helpers.
- Modify: `crates/conary-core/src/repository/mod.rs`
  - Export distro inference helpers.
- Modify call sites:
  - `apps/conary/src/commands/install/mod.rs`
  - `apps/remi/src/server/conversion.rs`
  - `apps/remi/src/server/delta_manifests.rs`
  - `crates/conary-core/src/repository/effective_policy.rs`
  - `crates/conary-core/src/resolver/canonical.rs`
  - `crates/conary-core/src/repository/selector.rs`
  - `crates/conary-core/src/automation/check.rs`

---

## Chunk 1: Shared Critical Blocklist

**Files:**
- Create: `crates/conary-core/src/critical_packages.rs`
- Modify: `crates/conary-core/src/lib.rs`
- Modify: `apps/conary/src/commands/install/blocklist.rs`
- Modify: `apps/remi/src/server/conversion.rs`

- [ ] **Step 1: Add failing core tests for shared policy**

Add `pub mod critical_packages;` to `crates/conary-core/src/lib.rs`, then add tests in `crates/conary-core/src/critical_packages.rs` covering:

- package-name blocking for `glibc`, `bash`, `filesystem`, `setup`
- case-insensitive package-name matching
- runtime capability blocking for `libc.so.6`, `ld-linux`, `libssl.so.`, `group(`
- normal packages like `nginx`, `curl`, `vim` are not blocked

Run:

```bash
cargo test -p conary-core critical_packages
```

Expected before implementation: compile failure because the exported module points at a file that does not exist, or test failure while the module is still stubbed.

- [ ] **Step 2: Implement shared policy module**

Create `crates/conary-core/src/critical_packages.rs` with:

- `pub const CRITICAL_PACKAGES: &[&str]`
- `pub const CRITICAL_RUNTIME_CAPABILITY_PREFIXES: &[&str]`
- `pub fn is_critical_package_name(name: &str) -> bool`
- `pub fn is_critical_runtime_capability(name: &str) -> bool`
- `pub fn is_blocked(name: &str) -> bool`
- `pub fn blocked_packages() -> &'static [&'static str]`

Export from `crates/conary-core/src/lib.rs`:

```rust
pub mod critical_packages;
```

- [ ] **Step 3: Rewire CLI blocklist**

Replace local constants in `apps/conary/src/commands/install/blocklist.rs` with wrappers:

```rust
pub fn is_critical_runtime_capability(name: &str) -> bool {
    conary_core::critical_packages::is_critical_runtime_capability(name)
}

pub fn is_blocked(name: &str) -> bool {
    conary_core::critical_packages::is_blocked(name)
}

pub fn blocked_packages() -> &'static [&'static str] {
    conary_core::critical_packages::blocked_packages()
}
```

Keep existing CLI tests, adjusting imports only if needed.

- [ ] **Step 4: Rewire Remi conversion guard**

Remove `is_critical_system_package` from `apps/remi/src/server/conversion.rs`.

Before download, reject package names with:

```rust
if conary_core::critical_packages::is_critical_package_name(package_name) {
    anyhow::bail!("Refusing to convert critical system package '{}'", package_name);
}
```

Before checking `ConvertedPackage::find_by_checksum`, also reject repository metadata that already declares critical runtime provides for the selected `RepositoryPackage`. This prevents a previously cached conversion from bypassing the runtime-capability policy. Use `RepositoryProvide::find_by_repository_package` when `repo_pkg.id` is available.

After parsing package metadata and merging repository provides, reject critical runtime provides from `PackageMetadata.provides`. Add small helpers local to `conversion.rs`:

```rust
fn metadata_provides_critical_runtime(metadata: &PackageMetadata) -> Option<&str> {
    metadata
        .provides
        .iter()
        .map(|provide| provide.name.as_str())
        .find(|name| conary_core::critical_packages::is_critical_runtime_capability(name))
}

fn repository_package_provides_critical_runtime(
    conn: &rusqlite::Connection,
    repo_pkg: &RepositoryPackage,
) -> Result<Option<String>> {
    let Some(repository_package_id) = repo_pkg.id else {
        return Ok(None);
    };
    let provides = RepositoryProvide::find_by_repository_package(conn, repository_package_id)?;
    Ok(provides
        .into_iter()
        .map(|provide| provide.capability)
        .find(|name| conary_core::critical_packages::is_critical_runtime_capability(name)))
}
```

- [ ] **Step 5: Add/adjust Remi tests**

Extend existing tests around `apps/remi/src/server/conversion.rs` to assert:

- `bash`, `filesystem`, and `setup` are refused
- case variants like `GLIBC` are refused
- the helper detects a metadata provide such as `libc.so.6()(64bit)` without requiring network/package downloads
- an existing cached conversion is still refused when its `repository_packages` row has a critical `repository_provides` capability

Run:

```bash
cargo test -p remi critical_packages
cargo test -p conary install::blocklist
```

- [ ] **Step 6: Verify chunk**

Run:

```bash
cargo test -p conary-core critical_packages
cargo test -p conary install::blocklist
cargo test -p remi critical_packages
cargo fmt --check
```

Commit:

```bash
git add crates/conary-core/src/critical_packages.rs crates/conary-core/src/lib.rs apps/conary/src/commands/install/blocklist.rs apps/remi/src/server/conversion.rs
git commit -m "security: share critical package blocklist"
```

---

## Chunk 2: composefs/fs-verity Trust And `/etc` Warnings

**Files:**
- Modify: `crates/conary-core/tests/generation_composefs_runtime_contract.rs`
- Modify: `apps/conary/src/commands/generation/switch.rs`
- Modify: `apps/conary/src/commands/composefs_ops.rs`

- [ ] **Step 1: Add failing source-contract tests**

Add tests to `generation_composefs_runtime_contract.rs`:

- `generation_switch_does_not_retry_requested_verity_as_plain_composefs`
- `composefs_apply_prints_etc_overlay_failures_to_stderr`
- `generation_switch_prints_etc_overlay_failures_to_stderr`

The first test should inspect the requested-verity branch precisely. It should fail if that branch contains the current downgrade retry shape, such as `or_else` plus `retrying without`, while still allowing the non-verity branch to call `mount_generation(&opts_plain)`.

The `/etc` warning tests should require both activation files to contain the existing log warning and an `eprintln!` for `/etc` overlay failure.

Run:

```bash
cargo test -p conary-core --test generation_composefs_runtime_contract
```

Expected before implementation: new tests fail.

- [ ] **Step 2: Fail closed in generation switch**

In `apps/conary/src/commands/generation/switch.rs`, remove the plain composefs retry when `requested_verity` is true.

Desired behavior:

- checksum mismatch remains fatal with a digest-specific message
- any other verity mount failure is fatal with a message that fs-verity was requested and no downgrade was attempted
- plain composefs is used only when persisted metadata says fs-verity is unavailable

- [ ] **Step 3: Surface `/etc` overlay failures to users**

In `apps/conary/src/commands/composefs_ops.rs` and `apps/conary/src/commands/generation/switch.rs`, keep the existing `warn!`, and add:

```rust
eprintln!("Warning: Failed to mount /etc overlay: {e}; /etc may be stale");
```

- [ ] **Step 4: Verify chunk**

Run:

```bash
cargo test -p conary-core --test generation_composefs_runtime_contract
cargo test -p conary-core generation::mount
cargo check -p conary
cargo fmt --check
```

Commit:

```bash
git add crates/conary-core/tests/generation_composefs_runtime_contract.rs apps/conary/src/commands/generation/switch.rs apps/conary/src/commands/composefs_ops.rs
git commit -m "security: fail closed on requested verity mounts"
```

---

## Chunk 3: conaryd API Honesty

**Files:**
- Modify: `apps/conaryd/src/daemon/mod.rs`
- Modify: `apps/conaryd/src/daemon/routes/transactions.rs`
- Modify: `apps/conaryd/src/daemon/routes.rs`

- [ ] **Step 1: Add failing conaryd route tests**

Add or rename a focused route test called `test_package_routes_return_not_implemented`. It should call install/remove/update package routes with root credentials and assert `501 Not Implemented` plus a direct message such as:

```text
Daemon package install jobs are not implemented yet. Use the CLI directly.
```

Run:

```bash
cargo test -p conaryd test_package_routes_return_not_implemented
```

Expected before implementation: status-code assertion fails.

- [ ] **Step 2: Return direct 501 from package routes**

In `apps/conaryd/src/daemon/routes/transactions.rs`, introduce a helper that preserves the existing auth check but removes JSON-body parsing and job-kind forwarding for unimplemented package routes:

```rust
fn package_jobs_not_implemented(operation: &str) -> ApiError
```

Use it in:

- `install_packages_handler`
- `remove_packages_handler`
- `update_packages_handler`

The handlers should no longer extract `Json<...>` request bodies. They should authenticate the requested action, then return `501`. Do not call `create_transaction_handler` for these routes until execution exists.

After replacing the handlers, remove or repurpose any now-unused forwarding helper such as `forward_package_operation` so `cargo clippy -D warnings` does not fail on dead code.

- [ ] **Step 3: Keep generic transaction guard**

Leave the `create_transaction_handler` non-Enhance guard in place as defense in depth for clients that post package operations to `/transactions`.

- [ ] **Step 4: Update daemon docs**

In `apps/conaryd/src/daemon/mod.rs`, change the module docs from "REST API for package operations (install, remove, update)" to the current truth:

- daemon owns lock, SSE, auth, and job queue scaffolding
- only enhance jobs execute today
- install/remove/update should use CLI directly until daemon executors are implemented

- [ ] **Step 5: Verify chunk**

Run:

```bash
cargo test -p conaryd test_package_routes_return_not_implemented
cargo test -p conaryd
cargo fmt --check
```

Commit:

```bash
git add apps/conaryd/src/daemon/mod.rs apps/conaryd/src/daemon/routes/transactions.rs apps/conaryd/src/daemon/routes.rs
git commit -m "fix(conaryd): report unimplemented package jobs directly"
```

---

## Chunk 4: Shared Distro And Version-Scheme Inference

**Files:**
- Create or Modify: `crates/conary-core/src/repository/distro.rs`
- Modify: `crates/conary-core/src/repository/mod.rs`
- Modify: `apps/conary/src/commands/install/mod.rs`
- Modify: `apps/remi/src/server/conversion.rs`
- Modify: `apps/remi/src/server/delta_manifests.rs`
- Modify: `crates/conary-core/src/repository/effective_policy.rs`
- Modify: `crates/conary-core/src/resolver/canonical.rs`
- Modify: `crates/conary-core/src/repository/selector.rs`
- Modify: `crates/conary-core/src/automation/check.rs`

- [ ] **Step 1: Add failing shared inference tests**

Add `pub mod distro;` to `crates/conary-core/src/repository/mod.rs`, then add table-driven tests in the new core module for:

- supported user-facing distro names only: `fedora-44`, `ubuntu-26.04`, and `arch`
- internal family labels already used by repository/Remi code: `fedora`, `ubuntu`, and `arch`
- `ubuntu-26.04` and the internal `ubuntu` family label map to `VersionScheme::Debian` because Ubuntu packages use Debian version semantics
- unknown distro returns `None` for name-only inference
- repository name/URL inference preserves current metadata-format detection for supported repositories, without treating extra distro families as user-facing support
- explicit DB strings `rpm`, `debian`, `arch` parse correctly
- invalid DB string returns `None`

Run:

```bash
cargo test -p conary-core repository::distro
```

Expected before implementation: compile failure because the exported module points at a file that does not exist, or test failure while the module is still stubbed.

- [ ] **Step 2: Implement shared inference helpers**

Create helpers such as:

```rust
pub fn flavor_from_distro_name(name: &str) -> Option<RepositoryDependencyFlavor>
pub fn flavor_from_repository(repo: &Repository) -> Option<RepositoryDependencyFlavor>
pub fn flavor_from_repository_name_url(name: &str, url: &str) -> Option<RepositoryDependencyFlavor>
pub fn version_scheme_from_distro_name(name: &str) -> Option<VersionScheme>
pub fn version_scheme_from_repository(repo: &Repository) -> Option<VersionScheme>
pub fn version_scheme_from_db(value: Option<&str>) -> Option<VersionScheme>
pub fn version_scheme_or_rpm(value: Option<&str>) -> VersionScheme
pub fn flavor_matches_distro_name(name: &str, flavor: RepositoryDependencyFlavor) -> bool
pub fn flavor_to_version_scheme(flavor: RepositoryDependencyFlavor) -> VersionScheme
```

Repository-based helpers should delegate to existing format detection where callers already depend on repository metadata shape, but supported distro-name helpers should stay limited to Fedora 44, Ubuntu LTS 26.04, and Arch. Keep "default to RPM" behavior explicit through `version_scheme_or_rpm` or local `unwrap_or(VersionScheme::Rpm)`.

- [ ] **Step 3: Replace duplicate call sites**

Replace local implementations in the files listed above. Preserve behavior unless a caller was plainly wrong.

Important preservation rules:

- places that currently return `None` for unknown distro should still return `None`
- places that currently default unknown values to RPM should call an explicit helper or use `unwrap_or(VersionScheme::Rpm)`
- places that infer from repository name and URL must keep URL-based inference through the repository helper
- do not add new supported distro aliases such as Linux Mint, Manjaro, Debian, RHEL, CentOS, or SUSE as part of this cleanup

- [ ] **Step 4: Verify chunk**

Run:

```bash
cargo test -p conary-core repository::distro
cargo test -p conary install::tests::distro_name_to_flavor_known
cargo test -p remi find_package
cargo test -p conary-core resolver::canonical
cargo fmt --check
```

Commit:

```bash
git add crates/conary-core/src/repository/distro.rs crates/conary-core/src/repository/mod.rs apps/conary/src/commands/install/mod.rs apps/remi/src/server/conversion.rs apps/remi/src/server/delta_manifests.rs crates/conary-core/src/repository/effective_policy.rs crates/conary-core/src/resolver/canonical.rs crates/conary-core/src/repository/selector.rs crates/conary-core/src/automation/check.rs
git commit -m "refactor: centralize distro version inference"
```

---

## Final Verification

Run:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p conary-core
cargo test -p conary
cargo test -p remi
cargo test -p conaryd
git status --short --branch
```

If all pass, push the branch:

```bash
git push -u origin audit-hardening
```

## Follow-Up Plan Candidates

- Remi async/blocking refactor: split repository sync into DB-only and async HTTP/CAS phases.
- Production panic audit: enable `clippy::unwrap_used` in targeted modules after filtering test-only code.
- conaryd package executor implementation: make daemon install/remove/update real instead of stubbed.
- Dynamic distro list: drive `conary distro list` from registry or configured repositories.
