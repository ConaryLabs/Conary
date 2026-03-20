# Lintian Memory

## Codebase Patterns

### Server handler conventions (conary-server/src/server/handlers/)
- `mod.rs` has shared helpers: `validate_name`, `serialize_json`, `json_response`, `human_bytes`, `find_repository_for_distro`
- `admin/` is a directory (mod.rs, tokens.rs, ci.rs, events.rs, repos.rs, federation.rs, audit.rs)
- Handler pattern: scope check -> path param validation -> get db_path from state -> spawn_blocking for DB ops -> match on Result<Result<T>> (join error + db error)
- Write operations publish events via `state.read().await.publish_event(event_type, json_data)`
- `json_error(status_code, message, error_code)` from `auth.rs` for consistent error responses
- External admin router on :8082 uses auth_middleware; localhost admin on :8081 has no auth
- `admin_service.rs` provides shared business logic used by both HTTP handlers and MCP tools
- `forgejo.rs` provides shared Forgejo/CI client (get, post, get_text)
- `Scope` enum replaces raw string scopes; `validate_scopes()` rejects unknown scope strings
- Rate limiters stored as axum Extension (not in RwLock ServerState)
- `extract_ip()` in rate_limit.rs is the single source for client IP extraction
- Federation peer management uses `federation_peer` DB model in conary-core

### Testing patterns
- `test_app()` creates temp DB, seeds a bootstrap token, returns `(Router, PathBuf)`
- `rebuild_app()` helper for multi-step tests (oneshot consumes the router)

### Known issues (still open)
- Body field validation sometimes weaker than path param validation (e.g., create_repo validates length but not character set)
- Forgejo repo path `/repos/peter/Conary` hardcoded 12+ times -- should be constant or config
- `is_enabled` vs `enabled` field naming inconsistency between REST and MCP APIs for peers
- `forgejo_url` and `forgejo_token` redundantly stored in both AdminSection config and ServerState
- `validate_path_param` exists in both admin/mod.rs and mcp.rs (different error types, acceptable)
- `content_url` not validated as URL in create_repo or update_repo
- `update_repo` handler does not validate `url` via url::Url::parse (create_repo does)

## Architecture Notes
- `conary_core::db::models::Repository` has: new(), insert(), update(), delete(), find_by_name(), list_all(), list_enabled()
- Repository.id is Option<i64> -- None before insert, Some after
- `ServerState` behind `Arc<RwLock<>>`, config.db_path used to open fresh connections per request
- External admin API scopes: admin, ci:read, ci:trigger, repos:read, repos:write, federation:read, federation:write
- Schema version is v54 (derivation_index table added in v54; uses function dispatch pattern migrate_v{N})
- `audit_log` model uses free functions (insert/query/purge) not struct methods -- differs from Trove/Repository pattern
- MCP endpoint at /mcp on :8082 requires admin scope
- SSE broadcast channel bounded at 1024 -- adequate for admin API volume

### Generation module (composefs-native branch, 2026-03-17)
- `conary-core/src/generation/` extracted from CLI `src/commands/generation/`
- builder.rs: `build_erofs_image` (composefs-rs feature-gated) + `build_generation_from_db` (high-level)
- composefs.rs: runtime kernel capability detection (anyhow::Result, not crate::Result -- inconsistent)
- metadata.rs: GenerationMetadata, EXCLUDED_DIRS, ROOT_SYMLINKS, path helpers (anyhow::Result)
- mount.rs: MountOptions, mount/unmount, symlink management (crate::Result -- consistent)
- New modules: etc_merge.rs (three-way /etc merge), gc.rs (CAS GC), delta.rs (zstd dictionary deltas)
- composefs_rs_eval.rs: proof-of-concept tests only (test-only module)
- EROFS images written as `root.erofs` (builder.rs line 218)
- DB table is `system_states` not `generations` -- recovery code references wrong table
- `composefs-rs` feature gate on conary-core, not on root crate -- `cargo check --features composefs-rs` won't work from root
- composefs-rs is now a default feature of conary-core (Cargo.toml line 72)
- `composefs_ops.rs` `rebuild_and_mount()` called after every install/remove/restore -- full EROFS rebuild every time
- `build_generation_from_db` uses N+1 queries: list_all troves + find_by_trove per trove (should be single bulk SELECT)
- `detect_kernel_version_from_db` calls `Trove::list_all` redundantly (builder already has the list)
- `rebuild_and_mount` calls `collect_etc_files` twice on same DB state -- etc merge is always a no-op
- `build_erofs_image` double-parses hex: `hex_to_digest` then `Sha256HashValue::from_hex` on same string
- OCI export (`export.rs`) includes ALL CAS objects, not just generation's; builds full tar in memory
- `is_excluded()` allocates via `format!` per comparison per file (hot inner loop)

### Code quality issues (2026-03-17 review)
- String literals need constants: "root.erofs" (10 sites), "composefs" (5 sites), ".conary-gen.json" (7 sites), EROFS magic 0xE0F5_E1E2 (9 sites)
- CAS two-level walk duplicated: export.rs `collect_generation_cas_hashes` and gc.rs `gc_cas_objects` -- should share iterator
- `hex_to_digest` tests copy-pasted from conary-core to src/commands/generation/builder.rs
- `dir_stat`/`default_stat` Stat construction duplicated between builder.rs and composefs_rs_eval.rs
- `walk_sysroot_to_cas` in image.rs uses `Vec<(String,String,u64,u32)>` tuple instead of `FileEntryRef`
- `ImageBuilder.log: String` field is write-only (never read/returned) -- dead state
- `accept_package_paths` has `let _ = a;` instead of `_` pattern binding
- anyhow::Result vs crate::Result inconsistency: composefs.rs and metadata.rs use anyhow, rest uses crate

### conary-test patterns
- `TestSuite` tracks failed IDs via HashMap but recomputes passed/failed/skipped counts via linear scan each time
- `StepType` enum owns cloned strings from `TestStep` fields -- could borrow
- `expand_vars()` always iterates all vars; no early-out for strings without `${`
- `to_sse()` serializes tagged enum but also manually extracts event name -- double work, plus data payload includes redundant tag wrapper
- `runs` HashMap in AppState grows without bound (no eviction)
- `list_runs` uses serde_json round-trip to stringify RunStatus
- `build_image()` always tars full context dir (Docker API requirement), no image-exists check
- **Duplication (2026-03-09)**: 5 test fixture constructors (test_state/test_config), handlers+mcp duplicate 5 operations (no service layer), Assertion lacks Default, DistroConfig uses numbered fields, MockBackend not shared, RunStatus stringified 3 ways

## Performance Notes
- `db::open_fast()` skips migrations for server hot paths (open_fast vs open)
- Governor rate limiter DashMaps cleaned every 5 min via `run_limiter_cleanup()` (retain_recent + shrink_to_fit)
- auth_middleware spawns background `touch()` on every authenticated request (open + write) -- no debouncing

## Dead Code Audit (2026-03-16)
- 27 `#[allow(dead_code)]` markers audited across the codebase
- Common pattern: serde deserialization structs with unused fields (sync.rs) -- annotation needed to match wire format
- Common pattern: RAII fields kept alive for side effects (SystemLock::file, tool_router via macro)
- rmcp `#[tool_router]` macro generates code that reads `tool_router` field -- compiler can't see through it
- `BenchmarkResult` in tests/inference_benchmark.rs is truly dead (never instantiated)
- `CapturedPatch` in provenance_capture.rs has unnecessary annotation (struct IS used)
- federation.rs PeerRow/StatsRow have struct-level annotations but most fields are used -- should be field-level
- provenance_capture.rs has 3 public methods (with_recipe_hash, record_git_commit, record_build_deps) that are planned API but not wired up yet
- Transaction module: old dead fields (deployer, description) removed by composefs-native branch

## Review Findings (2026-03-08 full audit)
- No SQL injection via format strings -- all DB queries use parameterized ?1 bindings
- No `unwrap()`/`expect()` in non-test server handler code (all are in #[cfg(test)] blocks)
- File headers compliant across all checked files
- Path traversal properly guarded via safe_join() and sanitize_path() in filesystem/transaction modules
- CAS atomic_store uses PID+counter temp names -- race-safe across threads/processes
- Transaction journal CRC32 integrity checks are solid (journal.rs now deleted in composefs-native)
- Bootstrap pipeline has 6+ expect() calls in production paths (base.rs) -- should be errors
- `expand_env_vars()` leaks host env into sandboxed bootstrap builds (design issue, has TODO)
- `num_milliseconds() as u64` in transaction finish can wrap on clock skew (P0) -- module rewritten
- Resolver uses `as u32` for pool indices -- safe at current scale but fragile
- CPIO parser has proper MAX_FILE_SIZE and MAX_NAME_SIZE guards
- Recovery module's symlink validation is more permissive than staging validation (recovery.rs now deleted)

## Code Reuse Findings (composefs-native, 2026-03-17)
- [composefs-native code reuse findings](code_reuse_composefs_native.md) -- 8 duplication issues across hashing, CAS walks, fsverity, kernel detection

## Bootstrap Pipeline Patterns (2026-03-16 bootable image spec review)
- `populate_sysroot()` exists and is unit tested but never called from the build pipeline
- `generate_initramfs()` in image.rs is a busybox-based fallback; hardcodes `/dev/vda2`
- `dracut` appears in `package_phase()` classifier but NOT in any PACKAGES constant -- will not be installed
- Duplicate fstab creation: `populate_sysroot()` in base.rs AND `create_fstab()` in image.rs
- ESP mount path `/boot/efi` in both fstabs conflicts with repart `CopyFiles=/boot:/` semantics
- Partition labels "ESP"/"root" in repart.rs mismatch fstab/boot config refs "CONARY_ESP"/"CONARY_ROOT"
- GRUB code in image.rs (`setup_efi_boot`, `create_grub_config`, `create_stub_efi`) writes nothing when EFI binary not found
- No network daemon enabled in sysroot (no systemd-networkd, no DHCP)
- QEMU test SSH uses `BatchMode=yes` -- incompatible with password-based auth, needs key-based auth
- `ssh-keygen -A -f <prefix>` does NOT re-root key generation; `-f` is ignored by `-A`
- systemd-boot EFI binary (`systemd-bootx64.efi`) requires systemd built with `-Dbootloader=true`

## Documentation Audit (2026-03-18)
- [doc_audit_2026_03_18](doc_audit_2026_03_18.md) -- systemic doc staleness after composefs-native and LFS 13 alignment

## Bootstrap v2 Spec Review (2026-03-19)
- [bootstrap_v2_spec_review](bootstrap_v2_spec_review.md) -- Rev 2 APPROVED: 0 HIGH (all 5 fixed), 6 MEDIUM (recipe format examples, stage naming, diverse-verification hash contradiction), 5 LOW

## Bootstrap v2 Implementation Plan Review (2026-03-19)
- [bootstrap_v2_plan_review](bootstrap_v2_plan_review.md) -- 19 findings: 7 HIGH (API mismatches, type errors), 8 MEDIUM (missing mount integration, spec gaps), 4 LOW

## Derivation Module Audits (2026-03-20)
- [derivation_code_reuse](derivation_code_reuse.md) -- 8 findings: 10 sites bypass crate::hash, erofs_image_hash OOM risk, duplicated test helpers
- [derivation_efficiency](derivation_efficiency.md) -- 12 findings (2 HIGH, 6 MEDIUM, 4 LOW): streaming file I/O, allocation reduction, SQLite batching for 114-package pipeline
- [derivation_code_quality](derivation_code_quality.md) -- 16 findings (5 P1): symlink data loss in compose, output_hash excludes mode/size, canonical_string duplication, profile hash injection, glibc dual membership
