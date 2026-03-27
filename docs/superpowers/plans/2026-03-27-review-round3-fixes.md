# Review Round 3 Fixes -- Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix all 160+ findings from the round 3 adversarial + invariant review (5 independent reviewers), organized into 4 phases with ~45 implementation tasks.

> **IMPORTANT:** The appendix at the end contains "Fold into Task N" items from Minimax, Gemini, and Codex reviews. Check the appendix for additional sub-steps when working on any task.

**Architecture:** Fixes are grouped by subsystem and severity. Phase 1 addresses the most dangerous exploitation chains (sandbox, self-update, metadata signing). Later phases address defense-in-depth, DoS, and test coverage. Tasks within each phase can be parallelized via `emerge`.

**Tech Stack:** Rust 2024 edition, rusqlite, composefs-rs, ed25519-dalek, axum, nix (namespace/seccomp), libc

**Spec:** `docs/superpowers/specs/2026-03-27-review-round3-design.md`
**Findings:** Summarized in conversation from 14 parallel lintian reviews (A1-A10, I1-I4)

---

## Phase 1: Critical Exploitation Chains (P0)

These are directly exploitable with significant impact. Fix before any release.

### Task 0: Fix CCS symlink-following arbitrary write (Codex C1)

**Findings:** A CCS package can create a symlink (`usr/lib/link -> /etc`) then write a child file (`usr/lib/link/cron.d/persist`). `sanitize_package_relative_path()` only rejects `..`, not paths whose ancestors are symlinks created by the same package. The write follows the symlink to the host. **This is an arbitrary write as root from any CCS package.**

**Files:**
- Modify: `src/commands/ccs/install.rs`

**Fix:** Before writing each file during CCS install, check that no ancestor component in the destination path is a symlink (either pre-existing or created earlier in the same install). Use `O_NOFOLLOW` semantics or `openat2` with `RESOLVE_NO_SYMLINKS`, or track created symlinks and reject child writes under them.

- [ ] Read `src/commands/ccs/install.rs` -- find the file deployment loop
- [ ] Track all symlinks created during the install in a `HashSet<PathBuf>`
- [ ] Before each file write, check if any ancestor of the destination path is in the symlink set
- [ ] If a symlink ancestor is found, reject the file with `Error::PathTraversal`
- [ ] Add regression test: package with symlink then child file write-through
- [ ] Run: `cargo test`
- [ ] Commit: `security(ccs): prevent symlink-following arbitrary write during install`

> **File conflict:** This task and Task 6 both modify `src/commands/ccs/install.rs`. Do Task 0 first, then Task 6.

---

### Task 1: Sandbox CCS hook scripts through container isolation (A1-C1, A6-C2)

**Findings:** CCS post_install/pre_remove scripts execute via raw `Command::new("/bin/sh").arg("-c").arg(script)` with zero sandbox isolation. Trivially exploitable RCE as root.

**Files:**
- Modify: `conary-core/src/ccs/hooks/mod.rs:383-416`

**Fix:** Route `execute_script()` through the existing `Sandbox` infrastructure or `ScriptletExecutor`. The `container::Sandbox` already provides PID/mount/network namespace isolation.

- [ ] Read `conary-core/src/ccs/hooks/mod.rs` and `conary-core/src/container/mod.rs` to understand current hook execution and Sandbox API
- [ ] Replace `Command::new("/bin/sh").arg("-c").arg(script)` with `Sandbox::new(ContainerConfig::default()).execute("/bin/sh", &["-c", script], &[], &env)`
- [ ] Add test verifying hook scripts run in isolated namespace
- [ ] Run: `cargo test -p conary-core ccs::hooks`
- [ ] Commit: `security(ccs): route hook scripts through container sandbox`

---

### Task 2: Set seccomp to Enforce mode (A6-C1)

**Findings:** Seccomp hardcoded to `EnforcementMode::Warn` in chroot scriptlets. Dangerous syscalls logged but still execute, enabling complete sandbox escape.

**Files:**
- Modify: `conary-core/src/scriptlet/mod.rs:427`

**Fix:** Change `Warn` to `Enforce`. Make configurable but default to Enforce.

- [ ] Read `conary-core/src/scriptlet/mod.rs` around line 427
- [ ] Change `seccomp_mode = EnforcementMode::Warn` to `seccomp_mode = EnforcementMode::Enforce`
- [ ] Also remove `setuid` and `setgid` from the seccomp allowlist in `conary-core/src/capability/enforcement/seccomp_enforce.rs` (Minimax A6-C4: these syscalls enable UID/GID escalation inside sandbox)
- [ ] Add `--seccomp-warn` CLI flag for development/debugging that overrides to Warn
- [ ] Run: `cargo test -p conary-core scriptlet`
- [ ] Commit: `security(scriptlet): set seccomp to Enforce mode by default`

---

### Task 3: Add mount namespace to chroot scriptlet path (A6-H1)

**Findings:** `execute_with_chroot()` uses plain chroot without mount namespace -- classic escape via `chroot("."); chdir("../..")`.

**Files:**
- Modify: `conary-core/src/scriptlet/mod.rs:413-454`

**Fix:** Add `unshare(CLONE_NEWNS | CLONE_NEWPID)` in the `pre_exec` closure before `chroot()`. Or route through `Sandbox::execute_isolated()`.

- [ ] Read `execute_with_chroot()` in scriptlet/mod.rs
- [ ] Add `nix::sched::unshare(CloneFlags::CLONE_NEWNS)` before the `chroot()` call
- [ ] Add `mount(None::<&str>, "/", None::<&str>, MsFlags::MS_PRIVATE | MsFlags::MS_REC, None::<&str>)` after unshare to prevent mount propagation
- [ ] Run: `cargo test -p conary-core scriptlet`
- [ ] Commit: `security(scriptlet): add mount namespace to chroot path`

---

### Task 4: Add CLONE_NEWUSER to sandbox (A6-H4)

**Findings:** No user namespace anywhere in the codebase. Sandbox runs as real UID 0.

**Files:**
- Modify: `conary-core/src/container/mod.rs:688-711`

**Fix:** Add `CLONE_NEWUSER` to unshare flags. Write uid_map/gid_map to map root inside to unprivileged UID outside. If `CLONE_NEWUSER` fails (kernel restriction), log a warning and continue with existing namespace isolation -- do not hard-fail. Ensure the seccomp filter permits `unshare` during sandbox initialization.

- [ ] Read `child_setup_and_execute()` in container/mod.rs
- [ ] Add `CLONE_NEWUSER` to the `unshare()` flags
- [ ] After `unshare()`, write `/proc/self/uid_map` and `/proc/self/gid_map` to map container root to host nobody
- [ ] Add `deny` to `/proc/self/setgroups` before writing gid_map
- [ ] Test: verify sandboxed process sees uid 0 but cannot write to root-owned files outside sandbox
- [ ] Run: `cargo test -p conary-core container`
- [ ] Commit: `security(container): add user namespace for privilege isolation`

---

### Task 5: Fix self-update MITM chain (A3-C1, A3-C2, A3-C3)

**Findings:** MITM can supply arbitrary download_url + matching hash. `verify_binary` executes the downloaded binary as root before installation. Combined with `--no-verify`, this is full RCE.

**Files:**
- Modify: `conary-core/src/self_update.rs`
- Modify: `src/commands/self_update.rs`

**Fix:** (a) Validate `download_url` is from the same origin as the channel URL. (b) Do NOT execute the binary before installation -- verify via hash only. (c) Remove or gate `verify_binary` execution.

- [ ] Read `conary-core/src/self_update.rs` -- find `verify_binary` and `download_url` usage
- [ ] Add origin validation: `download_url` must have the same scheme+host as the channel URL
- [ ] Remove or gate `verify_binary` execution (the version check by running the binary is the RCE vector)
- [ ] Verify via hash comparison only -- the SHA-256 streaming verification is already correct
- [ ] Run: `cargo test -p conary-core self_update`
- [ ] Commit: `security(self-update): validate download origin, remove pre-install binary execution`

---

### Task 6: CCS install transaction lock + atomic CAS (A7-C1, A7-C2, A7-C3)

**Findings:** CCS install bypasses transaction lock entirely, uses non-atomic `std::fs::write` for CAS, and uses `unchecked_transaction()`.

**Files:**
- Modify: `src/commands/ccs/install.rs`

**Fix:** (a) Acquire `conary.lock` via TransactionEngine. (b) Replace `std::fs::write` with `CasStore::atomic_store()`. (c) Replace `unchecked_transaction()` with `db::transaction()`.

- [ ] Read `src/commands/ccs/install.rs` -- find lock acquisition, CAS write, and transaction patterns
- [ ] Add `TransactionEngine::new()` and `engine.begin()` before any mutation
- [ ] Replace `std::fs::write(&cas_path, &content)` at ~line 596 with `cas.store(&content)?`
- [ ] Replace `unchecked_transaction()` at ~line 614 with `db::transaction()`
- [ ] Add `engine.release_lock()` after all mutations complete
- [ ] Run: `cargo test`
- [ ] Commit: `fix(ccs): add transaction lock, atomic CAS writes, safe DB transactions`

---

### Task 7: Add transaction lock to remove path (A7-C2)

**Findings:** `cmd_remove` has no file lock. Concurrent install + remove can interleave.

**Files:**
- Modify: `src/commands/remove.rs`

**Fix:** Acquire `conary.lock` before first mutation, release after `rebuild_and_mount`.

- [ ] Read `src/commands/remove.rs`
- [ ] Add `TransactionEngine::new()` and lock acquisition before the DB transaction
- [ ] Release after `rebuild_and_mount` completes
- [ ] Run: `cargo test`
- [ ] Commit: `fix(remove): add transaction lock for concurrent operation safety`

---

### Task 8: Fix dracut boot /etc overlay path (A8-C1)

**Findings:** Dracut initrd uses shared `/conary/etc-state/upper` while runtime uses per-generation `/conary/etc-state/{N}`. Every reboot-based rollback is defeated.

**Files:**
- Modify: `packaging/dracut/90conary/conary-generator.sh`

**Fix:** Align dracut to use per-generation scheme. Read generation number from `current` symlink or kernel cmdline.

- [ ] Read `packaging/dracut/90conary/conary-generator.sh` -- find the /etc overlay upper dir
- [ ] Change to read the current generation number from `/conary/current` symlink target
- [ ] Use `etc-state/{N}` and `etc-state/{N}-work` matching the runtime code
- [ ] Create the directory if it doesn't exist (first boot of a new generation)
- [ ] Test: verify dracut script produces correct mount options
- [ ] Commit: `security(dracut): align /etc overlay to per-generation scheme`

---

### Task 9: Validate hook inputs in CCS manifest (A1-H6)

**Findings:** `CcsManifest::validate()` only checks name/version. No hook field validation. Root cause of all hook-based attacks.

**Files:**
- Modify: `conary-core/src/ccs/manifest.rs:93-101`
- Modify: `conary-core/src/ccs/hooks/user_group.rs`
- Modify: `conary-core/src/ccs/hooks/tmpfiles.rs`
- Modify: `conary-core/src/ccs/hooks/sysctl.rs`
- Modify: `conary-core/src/ccs/hooks/systemd.rs`

**Fix:** Extend `validate()` to check all hook fields. Add per-hook validation functions.

- [ ] Add `validate_username()`: POSIX rules (`^[a-z_][a-z0-9_-]*$`), max 32 chars, require `system = true`
- [ ] Add `validate_shell()`: allowlist (`/usr/sbin/nologin`, `/bin/false`, `/sbin/nologin`)
- [ ] Restrict tmpfiles `entry_type` to allowlist: `d`, `D`, `e`, `v`, `q`, `Q` only
- [ ] Add tmpfiles `path` validation via `sanitize_path()`
- [ ] Add sysctl key denylist: `kernel.randomize_va_space`, `kernel.kptr_restrict`, `kernel.modules_disabled`, etc.
- [ ] Apply `is_safe_unit_name()` to systemd unit name in live enable path
- [ ] Wire all validations into `CcsManifest::validate()`
- [ ] Add tests for each validation
- [ ] Run: `cargo test -p conary-core ccs`
- [ ] Commit: `security(ccs): validate all hook inputs in manifest`

---

### Task 10: GPG verification on repository metadata (A2-C1)

**Findings:** No signature verification on repomd.xml, Packages.gz, or .db.tar.gz during sync. MITM can inject arbitrary packages.

**Files:**
- Modify: `conary-core/src/repository/sync.rs`
- Modify: `conary-core/src/repository/gpg.rs`

**Fix:** When `repo.gpg_check` is true, verify GPG signatures on downloaded metadata before parsing. For repos without GPG keys, warn prominently.

- [ ] Read `sync.rs` to find where metadata is downloaded and parsed
- [ ] After downloading metadata (repomd.xml, Packages.gz, .db.tar.gz), check for detached signature (.asc/.sig)
- [ ] When `gpg_check = true` and signature exists, verify before parsing
- [ ] When `gpg_check = true` and no signature available, emit `warn!` (not silent skip)
- [ ] Run: `cargo test -p conary-core repository`
- [ ] Commit: `security(repository): verify GPG signatures on repository metadata`

---

### Task 11: Make create_snapshot_at transactional (I2-H1)

**Findings:** 4 non-transactional writes in `create_snapshot_at()`. Crash between steps leaves empty `state_cas_hashes`, causing GC data loss.

**Files:**
- Modify: `conary-core/src/db/models/state.rs:391-438`

**Fix:** Wrap all 4 operations in `conn.unchecked_transaction()`.

- [ ] Read `create_snapshot_at()` in state.rs
- [ ] Add `let tx = self.conn.unchecked_transaction()?;` at the start
- [ ] Change all operations to use `&tx` instead of `self.conn`
- [ ] Add `tx.commit()?;` before returning
- [ ] Fold `set_active()` savepoint logic into the wrapping transaction
- [ ] Run: `cargo test -p conary-core`
- [ ] Commit: `fix(state): wrap create_snapshot_at in transaction for atomicity`

---

### Task 12: Fix federation empty trusted_keys bypass (I3-H2, A5-H2)

**Findings:** When `trusted_keys` is empty (default), ANY signed manifest passes the key check. Attacker-signed manifests accepted as trusted.

**Files:**
- Modify: `conary-server/src/federation/manifest.rs:184`

**Fix:** When `trusted_keys` is empty AND `allow_unsigned` is false, reject ALL signed manifests.

- [ ] Read `manifest.rs` around line 184
- [ ] Change: when `trusted_keys.is_empty() && !policy.allow_unsigned`, return `Err(ManifestError::UntrustedKey)`
- [ ] Add test: empty trusted_keys + signed manifest = rejection
- [ ] Run: `cargo test -p conary-server federation`
- [ ] Commit: `security(federation): reject signed manifests when no trusted keys configured`

---

### Task 12b: Require authentication for mDNS-discovered peers (Minimax CRITICAL-A5)

**Findings:** Any device on the LAN can register as a federation peer via mDNS. `conary-server/src/federation/mdns.rs` adds discovered peers without checking the allowlist or requiring mTLS verification. Zero authentication.

**Files:**
- Modify: `conary-server/src/federation/mdns.rs`
- Modify: `conary-server/src/federation/mod.rs`

- [ ] Read mDNS discovery callback in `mdns.rs` and `mod.rs`
- [ ] Before adding a discovered peer, check against the configured peer allowlist
- [ ] If no allowlist configured, require mTLS for all federation connections (not just WAN)
- [ ] At minimum, log a prominent warning when unauthenticated peers are accepted
- [ ] Run: `cargo test -p conary-server federation`
- [ ] Commit: `security(federation): require allowlist or mTLS for mDNS-discovered peers`

---

### Task 12c: Enforce daemon socket authentication regardless of connection type (Minimax CRITICAL-A9)

**Findings:** Socket mode `0o660` allows any user in the group to connect. `RequireAuth` is not enforced for Unix socket connections, giving group members full admin access.

**Files:**
- Modify: `conary-server/src/daemon/routes.rs`
- Modify: `conary-server/src/daemon/auth.rs`

- [ ] Read auth middleware in daemon routes -- find where Unix socket connections bypass auth
- [ ] Enforce `RequireAuth` for ALL connection types, including Unix socket
- [ ] Existing `PeerCredentials` from `SO_PEERCRED` should be checked against an authorization policy (at minimum: uid == 0 or uid == daemon uid)
- [ ] Run: `cargo test -p conary-server daemon`
- [ ] Commit: `security(daemon): enforce auth for Unix socket connections`

---

### Task 12d: Add MCP auth integration test (Minimax CRITICAL-A4)

**Findings:** MCP router's `route_layer` checks `TokenScopes` from request extensions. If parent auth middleware extensions don't propagate (axum layer ordering), the check sees `None` and returns 403 (currently fails closed). But this is fragile with no test coverage.

**Files:**
- Modify: `conary-server/src/server/routes.rs` (add re-validation)
- Create: test in `conary-server/src/server/` (integration test)

- [ ] Add explicit `check_scope()` call inside the MCP handler itself (defense-in-depth, don't rely solely on route_layer)
- [ ] Add integration test: unauthenticated request to `/mcp` returns 401/403
- [ ] Add integration test: token with non-admin scope to `/mcp` returns 403
- [ ] Run: `cargo test -p conary-server`
- [ ] Commit: `security(server): explicit MCP auth re-validation with integration tests`

---

## Phase 2: High-Severity Security Hardening (P1)

### Task 13: Scriptlet env_clear + host /tmp isolation (A6-C3, A6-H2)

**Files:** `conary-core/src/scriptlet/mod.rs`, `conary-core/src/container/mod.rs`

- [ ] Add `env_clear()` to `execute_direct()` -- only set safe env vars explicitly
- [ ] Replace host `/tmp` bind-mount in `pristine_for_bootstrap()` with private `TempDir`
- [ ] Commit: `security(sandbox): env_clear in direct exec, private /tmp in bootstrap`

---

### Task 14: Generation switch runs no removal scriptlets (A8-C3)

**Files:** `src/commands/generation/switch.rs`, `src/commands/generation/commands.rs`

- [ ] Add warning after generation switch listing packages removed that had side effects (users, systemd units, cron)
- [ ] Document that generation rollback does not undo scriptlet side effects
- [ ] Consider `--undo-scriptlets` flag that diffs package lists and runs removal scriptlets
- [ ] Commit: `fix(generation): warn about unreversed scriptlet side effects on rollback`

---

### Task 15: EROFS verity digest computation + verification (A8-H2)

**Files:** `conary-core/src/generation/builder.rs`, `conary-core/src/generation/mount.rs`, `conary-core/src/generation/metadata.rs`

- [ ] Compute fs-verity digest at EROFS build time
- [ ] Store in `GenerationMetadata.erofs_verity_digest`
- [ ] Before mounting, verify digest matches if stored; refuse to mount mismatched images
- [ ] Commit: `security(generation): compute and verify EROFS verity digest`

---

### Task 16: GC cleanup of etc-state directories (A8-H1)

**Files:** `src/commands/generation/commands.rs`

- [ ] In GC loop, after removing generation dir, also `remove_dir_all` for `etc-state/{N}/` and `etc-state/{N}-work/`
- [ ] Commit: `fix(gc): remove orphaned etc-state directories during generation GC`

---

### Task 17: Remove double state snapshots (A7-H1)

**Files:** `src/commands/install/batch.rs`, `src/commands/remove.rs`

- [ ] Remove standalone `create_state_snapshot` calls after `rebuild_and_mount` (which already creates one)
- [ ] Verify no other callers duplicate snapshots
- [ ] Commit: `fix: remove duplicate state snapshot creation in install/remove`

---

### Task 18: CAS integrity -- verify on all retrieval paths (I1-H1 through I1-H4)

**Files:** `conary-core/src/derivation/substituter.rs`, `src/commands/update.rs`, `conary-core/src/repository/chunk_fetcher.rs`, `conary-core/src/ccs/chunking.rs`

- [ ] `substituter.rs:337`: compare `cas.store(&bytes)?` return to `file.hash`, error on mismatch
- [ ] `update.rs:559`: replace `retrieve_unchecked` with `cas.retrieve(&new_hash)`
- [ ] `chunk_fetcher.rs:446`: add `verify_sha256(&data, hash)` to `LocalCacheFetcher::fetch()`
- [ ] `chunking.rs:242`: add re-hash verification to `ChunkStore::get_chunk()`
- [ ] Commit: `fix(cas): verify integrity on all retrieval paths`

---

### Task 19: Daemon auth-gate GET endpoints (A9-H1, A9-H2, A9-H3)

**Files:** `conary-server/src/daemon/routes.rs`

- [ ] Extend `auth_gate_middleware` to check all HTTP methods, not just POST/PUT/DELETE
- [ ] Filter SSE events by `requested_by_uid` matching caller's UID (or root)
- [ ] Filter transaction list/detail by ownership
- [ ] Commit: `security(daemon): auth-gate all endpoints, filter events/transactions by user`

---

### Task 20: Canonical mapping authentication (A2-C2)

**Files:** `conary-core/src/canonical/sync.rs`, `conary-core/src/canonical/client.rs`

- [ ] Add signature verification or checksum validation for canonical mapping data
- [ ] Reject mappings that redirect to suspiciously different package names
- [ ] Commit: `security(canonical): authenticate canonical mapping data`

---

### Task 20b: Additional trust model fixes (Minimax HIGHs)

**Findings from Minimax not covered elsewhere:**

- [ ] **PeerId from URL not cert-bound (Minimax HIGH-A5):** In `conary-server/src/federation/peer.rs`, bind PeerId to TLS certificate fingerprint instead of URL hash. A DNS hijack currently allows peer impersonation.
- [ ] **TrustPolicy::permissive() accepts self-signed packages (Minimax HIGH-I3):** In `conary-core/src/ccs/verify.rs`, `TrustPolicy::Permissive` returns success for `Untrusted` signature status. Add warning or refuse in non-development contexts.
- [ ] **Model remote.rs empty keys returns Ok(false) (Minimax HIGH-I3):** In `conary-core/src/model/remote.rs`, when `trusted_keys` is empty and `require_signatures` is true, silently returns `Ok(false)`. Should error, matching federation manifest fix in Task 12.
- [ ] **EROFS image written outside DB transaction (Minimax HIGH-I2):** In `conary-core/src/generation/builder.rs`, the EROFS image is written and persisted before the DB snapshot transaction. Add a pending marker or wrap in the snapshot transaction.
- [ ] Commit per-subsystem as appropriate.

---

### ~~Task 21: MERGED INTO TASK 5~~ (download_url validation is part of Task 5)

---

### Task 22: Remove cfg!(test) bypass from verify_update_signature (I3-H1)

**Files:** `conary-core/src/self_update.rs:79-81`

- [ ] Remove the `cfg!(test)` bypass entirely
- [ ] Verify tests still pass (they call `verify_update_signature_with_keys` directly)
- [ ] Commit: `security(self-update): remove cfg!(test) signature bypass`

---

### Task 23: Compression bomb protection (A10-C1, A10-C2)

**Files:** `conary-core/src/compression/mod.rs`, `conary-core/src/packages/deb.rs`, `conary-core/src/packages/arch.rs`, `conary-core/src/repository/client.rs`

- [ ] Add `create_decoder_limited()` that wraps output in `Read::take()` with cumulative budget
- [ ] Add `MAX_ENTRIES` (500,000) limit to all tar/archive iteration loops
- [ ] Reduce `MAX_DECOMPRESS_SIZE` for metadata contexts to 512 MB
- [ ] Commit: `security: add compression bomb protection with cumulative size and entry limits`

---

### Task 24: SSRF protection for stored URLs (A4-M1, A4-M2)

**Files:** `conary-server/src/server/handlers/admin/repos.rs`, `conary-server/src/server/admin_service.rs`

- [ ] Create `validate_external_url()` that resolves hostname and rejects loopback, private (RFC 1918), link-local, and cloud metadata IPs
- [ ] Apply to `create_repo`, `update_repo`, and `add_peer`
- [ ] Commit: `security(server): add SSRF protection for stored URLs`

---

## Phase 3: Medium-Severity Hardening (P2)

### Task 25: Federation canonical JSON + model require_signatures (I3-M1, I3-M3)

**Files:** `conary-server/src/federation/manifest.rs:136`, `conary-core/src/model/parser.rs`

- [ ] Switch federation `canonical_bytes()` to use `conary_core::json::canonical_json()`
- [ ] Change `require_signatures` default to `true` for remote collections, or emit warning when false
- [ ] Commit: `fix: federation canonical JSON, warn on unsigned remote collections`

---

### Task 26: Sandbox /etc + /var protection + read-only remount fix (A6-H3, A6-M5)

**Files:** `conary-core/src/scriptlet/mod.rs`, `conary-core/src/container/mod.rs`

- [ ] In sandbox live mode, mount `/etc/passwd`, `/etc/shadow`, `/etc/sudoers` read-only even when /etc writable
- [ ] Fix `.ok()` on read-only remount -- log error and fail in Enforce mode
- [ ] Commit: `security(sandbox): protect critical /etc files, fix read-only remount error handling`

---

### Task 27: Bootstrap /dev minimization + chroot teardown (A6-M3, A6-L3)

**Files:** `conary-core/src/bootstrap/chroot_env.rs`

- [ ] Replace full host /dev bind-mount with minimal devtmpfs (null, zero, random, urandom, tty, full)
- [ ] Kill processes inside chroot before teardown, use non-lazy umount first
- [ ] Commit: `security(bootstrap): minimize /dev mount, kill processes before teardown`

---

### Task 28: Setuid bit stripping + component auditing (A1-M1, A1-M2)

**Files:** `conary-core/src/ccs/policy.rs`, `conary-core/src/ccs/builder.rs`

- [ ] Add `StripSetuidPolicy` to default policy chain -- mask mode with `& !0o6000`
- [ ] Flag executable files in `:doc`/`:config`/`:data` components as suspicious
- [ ] Commit: `security(ccs): strip setuid/setgid bits, audit component classification`

---

### Task 29: DoS -- SAT solver limits + upload body limit + CCS archive limits (A10-H2, A10-H3, A10-H5)

**Files:** `conary-core/src/resolver/sat.rs`, `conary-core/src/repository/remi.rs`, `conary-server/src/server/routes.rs`, `conary-core/src/ccs/archive_reader.rs`

- [ ] Add `MAX_LOADED_NAMES` (50,000) and timeout to SAT solver transitive loading loop
- [ ] Add `MAX_TOTAL_CHUNK_BYTES` to chunk download accumulation in RemiClient
- [ ] Add `DefaultBodyLimit` middleware to daemon router
- [ ] Wrap GzDecoder in archive_reader with `.take(MAX_TOTAL_EXTRACTION_SIZE)`
- [ ] Commit: `fix: add resource limits for SAT solver, chunk downloads, uploads, archives`

---

### Task 30: GC coordination + grace period + gc-roots protection (A7-M1, A8-H3, A8-H4)

**Files:** `conary-core/src/generation/gc.rs`, `src/commands/generation/commands.rs`

- [ ] GC should acquire transaction lock (or skip objects with mtime < 1 hour)
- [ ] Protect gc-roots with DB-backed tracking instead of raw filesystem presence
- [ ] Commit: `fix(gc): coordinate with transactions, protect gc-roots`

---

### Task 31: Daemon socket permission TOCTOU + body limit + error sanitization (A9-M1, A9-M2, A9-M3)

**Files:** `conary-server/src/daemon/socket.rs`, `conary-server/src/daemon/routes.rs`

- [ ] Set umask to `0o077` before `UnixListener::bind()`, restore after
- [ ] Add `DefaultBodyLimit::max(2 * 1024 * 1024)` to daemon router
- [ ] Sanitize `DaemonError::internal` messages -- log full error, return generic text to client
- [ ] Commit: `security(daemon): fix socket TOCTOU, add body limit, sanitize errors`

---

### Task 32: Remaining CAS integrity mediums (I1-M1 through I1-M5)

**Files:** `conary-core/src/filesystem/cas.rs`, `conary-core/src/repository/chunk_fetcher.rs`, `src/commands/generation/commands.rs`

- [ ] Align `hash_to_path()` validation to match `object_path()` (require len >= 4 + hex)
- [ ] Use unique temp names in `LocalCacheFetcher::store()` (PID+counter)
- [ ] Acquire transaction lock during GC for in-flight transaction safety
- [ ] Add hex validation to `LocalCacheFetcher::chunk_path()`
- [ ] Make `object_path()` return `Result<PathBuf>` instead of silent fallback
- [ ] Commit: `fix(cas): align validation, unique temps, GC coordination`

---

### Task 33: Recipe env var filtering + LD_PRELOAD protection (A6-M1)

**Files:** `conary-core/src/recipe/kitchen/cook.rs`

- [ ] Add denylist for dangerous env vars: `LD_PRELOAD`, `LD_LIBRARY_PATH`, `LD_AUDIT`, `LD_BIND_NOT`
- [ ] In `run_build_step_direct`, start from `env_clear()` state
- [ ] Commit: `security(recipe): filter dangerous environment variables`

---

### Task 34: Remaining transaction atomicity + update orphans (I2-M1 through I2-M4)

**Files:** `src/commands/system.rs`, `src/commands/update.rs`, `src/commands/mod.rs`, `src/commands/adopt/packages.rs`

- [ ] Move `println!()` out of rollback transaction closures -- collect in Vec, print after commit
- [ ] Add orphan changeset cleanup for `cmd_update` (mark as RolledBack on abnormal exit)
- [ ] Propagate `create_state_snapshot` errors instead of swallowing
- [ ] Replicate empty-trove guard from adopt/system.rs to adopt/packages.rs
- [ ] Commit: `fix: transaction atomicity improvements (println, orphans, error propagation)`

---

### Task 35: Metadata signing + /etc merge security (A8-H5, A8-M2, A8-M3)

**Files:** `conary-core/src/generation/metadata.rs`, `src/commands/composefs_ops.rs`, `conary-core/src/generation/mount.rs`

- [ ] Sign generation metadata with a key from the keyring
- [ ] Store .base-gen in database instead of unsigned file
- [ ] Emit user-visible warning on verity-to-non-verity fallback during mount
- [ ] Commit: `security(generation): sign metadata, secure base-gen, warn on verity downgrade`

---

## Phase 4: Low-Severity Hardening (P3)

### Task 36: Test coverage -- panic paths + dead code (I4-C1, I4-C2, I4-H1 through I4-H4)

**Files:** Multiple across resolver, federation, container, adopt, bootstrap

- [ ] Replace 4 `expect()` calls in resolver/provider/mod.rs with proper error handling
- [ ] Replace `.unwrap()` on mDNS state in federation/mod.rs with error handling
- [ ] Replace `expect()` in federation/manifest.rs `sign()` with `?`
- [ ] Replace `expect()` on HTTP client creation in server/mod.rs
- [ ] Replace `CString::new().unwrap()` in daemon/socket.rs
- [ ] Add boundary tests for 6 `unsafe` blocks in container/mod.rs
- [ ] Commit: `fix: replace panic paths with error handling, add unsafe boundary tests`

---

### Task 37: Remaining DoS protections (A10-M1 through A10-M5, A10-L)

**Files:** Multiple

- [ ] DEB control member: lower limit to 16 MB (from 2 GB)
- [ ] Self-update JSON response: add 1 MB limit before deserializing
- [ ] Job queue: track total count against max capacity
- [ ] RequestCoalescer: add `MAX_INFLIGHT` cap
- [ ] Metadata body-size: use streaming with running total instead of Content-Length trust
- [ ] RPM file read: reduce limit from 4 GB to 2 GB
- [ ] RateLimiter eviction: consider governor replacement (per existing TODO)
- [ ] Commit: `fix: add resource limits for DoS protection across all input boundaries`

---

### Task 38: Daemon improvements (A9-L1 through A9-L3)

**Files:** `conary-server/src/daemon/routes.rs`, `conary-server/src/daemon/socket.rs`

- [ ] Add ownership check to cancel endpoint
- [ ] Remove PID and schema version from unauthenticated health/version responses
- [ ] Validate socket parent directory permissions after `create_dir_all`
- [ ] Add SSE connection cap (e.g., 64 per daemon)
- [ ] Commit: `fix(daemon): ownership checks, info hiding, socket validation, SSE limits`

---

### Task 39: Remaining security LOWs

**Files:** Multiple

- [ ] `safe_join()`: return error when root cannot be canonicalized instead of silent pass
- [ ] Script risk analysis: default `SandboxMode` to `Always` instead of `Auto`
- [ ] Chroot teardown: kill processes before lazy umount
- [ ] Bootstrap `--skip-verify`: add prominent runtime warning
- [ ] Document TUF TOFU bootstrap assumptions
- [ ] `--no-verify` audit trail after keys are configured
- [ ] Booted generation: validate against actual generation directory
- [ ] `ContainerConfig::for_untrusted()`: enforce minimum isolation levels
- [ ] Commit: `fix: address remaining security LOWs (safe_join, sandbox defaults, cleanup)`

---

### Task 40: Test coverage -- unreachable code + dead APIs (I4-M, I4-L)

**Files:** Multiple

- [ ] Replace wildcard `unreachable!()` in daemon/routes.rs and adopt/hooks.rs with explicit arms
- [ ] Remove `#[cfg(test)]` methods on production structs in conary-test
- [ ] Remove 4 dead error variants (TransactionError, TransactionConflict, JournalCorrupted, CircuitOpen)
- [ ] Add seccomp failure path test in capability/enforcement
- [ ] Commit: `fix: remove dead code, replace unreachable with explicit arms`

---

## Execution Notes

**Parallelization:** Within each phase, most tasks touch different files and can be dispatched in parallel via `emerge`. Exceptions:
- Task 0 and Task 6 both modify `src/commands/ccs/install.rs` -- do Task 0 first (symlink write), then Task 6 (transaction lock)
- Task 1 and Task 2 both modify sandbox-related code -- do Task 1 first (hooks), then Task 2 (seccomp)
- Task 3 depends on Task 2 (both modify scriptlet/mod.rs)
- Task 5 and Task 22 both modify self_update.rs -- coordinate
- Task 21 merged into Task 5 (duplicate)

**Testing:** After each phase, run:
```bash
cargo test                          # Unit tests
cargo clippy -- -D warnings         # Lint
cargo build --features server       # Server build check
```

**Integration test gate between Phase 1 and Phase 2:**
```bash
cargo run -p conary-test -- run --distro fedora43 --phase 1
cargo run -p conary-test -- run --distro fedora43 --suite phase2-group-a --phase 2
```

**Verification:** After all phases, dispatch `sbuild` for full release verification.

---

## Minimax-Unique Findings (folded into plan)

3 findings from the Minimax independent review not covered by existing tasks:

### Fold into Task 2 (seccomp Enforce): Remove setuid/setgid from seccomp allowlist (Minimax A6-C4)

**Finding:** The default seccomp allowlist includes `setuid` (syscall #146) and `setgid` (#149). Combined with writable /etc and Warn-mode seccomp, enables UID/GID escalation inside the sandbox.

**Fix:** When setting seccomp to Enforce in Task 2, also remove `setuid` and `setgid` from the allowlist in `conary-core/src/capability/enforcement/seccomp_enforce.rs`. Add to Task 2's steps.

### New Task (Phase 2): Daemon PID reuse defense (Minimax A9-C1)

**Finding:** SO_PEERCRED captures PID at connection time. After daemon crash, a malicious local user can spawn a process with the same PID and inherit root credentials. Race window is several seconds.

**Fix:** Re-validate peer credentials on each request (not just connection time). Add to Task 19 alongside daemon auth-gating. Modify: `conary-server/src/daemon/auth.rs`

### Fold into Task 6 (CCS transaction lock): Fix lockfile truncation (Gemini H3)

**Finding:** `File::create` truncates the lockfile even if another process holds a lock. The error message encourages users to delete the lockfile, which can cause two processes to hold "exclusive" locks on different inodes.

**Fix:** Use `OpenOptions::new().create(true).truncate(false).write(true).open()` instead of `File::create`. Update error message to NOT suggest deleting the lockfile. Modify: `conary-core/src/transaction/mod.rs`

### Fold into Task 9 (hook validation): Unicode normalization in path sanitization (Gemini H4)

**Finding:** `sanitize_path()` doesn't normalize Unicode. Homoglyphs like U+FF0F (fullwidth solidus) could bypass `/` detection on normalizing filesystems (macOS HFS+, potentially future Linux).

**Fix:** Add Unicode NFC normalization before component validation in `sanitize_path()`. Or reject any non-ASCII characters in paths from untrusted sources. Modify: `conary-core/src/filesystem/path.rs`

### NEW Task (Phase 1): CCS symlink-following arbitrary write (Codex C1)

**Finding:** A CCS package can create a symlink (`usr/lib/link -> /etc`) then write a child file (`usr/lib/link/cron.d/persist`). `sanitize_package_relative_path()` only rejects `..`, not paths whose ancestors are symlinks created earlier in the same install. The write follows the symlink to the host.

**Fix:** Before writing each file during CCS install, check that no ancestor in the destination path is a symlink created by the same package. Or stage all files into a temp tree (no symlink following) then move atomically.
Modify: `src/commands/ccs/install.rs`, add regression test for "symlink first, child file second" attack.

**Priority:** P0 -- this is an arbitrary write as root from any CCS package.

### Fold into Task 9 (hook validation): Call revert_pre_hooks on failure (Codex M1)

**Finding:** `revert_pre_hooks()` exists in `ccs/hooks/mod.rs` but is never called. Failed installs leave created users/groups/directories behind.

**Fix:** Call `revert_pre_hooks()` on every post-hook/deployment/DB failure path after pre-hooks have run. Add end-to-end test.
Modify: `src/commands/ccs/install.rs`

### Fold into Task 10 (GPG verification): Make gpg_strict the default (Codex H4)

**Finding:** `gpg_check = true` but `gpg_strict = false` is the default. Missing `.sig` just logs a warning and passes. Attacker omits signature file to bypass GPG.

**Fix:** Change `gpg_strict` default to `true`. Non-strict mode should require explicit opt-in with visible warning.
Modify: `conary-core/src/db/models/repository.rs`

### Fold into Task 14 (rollback scriptlets): Post-install deliberate failure (Codex H2)

**Finding:** Malicious post_install plants persistence then exits non-zero. DB is already committed, CLI reports failure, but package remains installed. Operators misled into thinking install failed cleanly.

**Fix:** Either run post-install before commit (rollback-safe) or treat post-commit script failure as degraded-success, never as "failed install." Add changeset status `PostHooksFailed`.
Modify: `src/commands/ccs/install.rs`

### Fold into Task 23 (compression bombs): CPIO cumulative size limit (Gemini M3)

**Finding:** CPIO parser limits individual file sizes (512 MB) but not cumulative extracted size. A crafted RPM with many entries can fill disk.

**Fix:** Add cumulative size tracking in `conary-core/src/packages/cpio.rs` matching the pattern in `archive_reader.rs`.

### New Task (Phase 2): MCP auth integration test (Minimax A4-C1)

**Finding:** MCP router's `route_layer` checks `TokenScopes` from request extensions. If parent auth middleware extensions don't propagate (axum layer ordering), the check sees `None` and returns 403 (fails closed). Currently safe but fragile -- no integration test verifies this.

**Fix:** Add integration test that sends unauthenticated request to `/mcp` and verifies 401/403 rejection. Add to Task 19 or create standalone test task.
