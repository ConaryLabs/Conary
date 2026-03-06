// docs/reviews/2026-03-06-feature-code-review.md
# Conary Codebase Feature Review -- 2026-03-06

## Executive Summary

- 16 features reviewed across 4 crates (`conary`, `conary-core`, `conary-server`, `conary-erofs`)
- 65 findings total: 10 P0, 30 P1, 25 P2
- Key security themes: shell injection, signature bypass, path traversal, SSRF
- Key correctness themes: transaction ordering, race conditions, input validation
- Key design themes: missing cleanup, non-deterministic behavior, hardcoded values

## Methodology

Feature-focused code reviews were run in parallel batches of 4, each scoped to a logical feature area. Each review examined the primary module files, their test coverage, error handling paths, and integration points with other subsystems. Findings are classified as:

- **[CRITICAL] P0**: Security vulnerabilities or data-loss bugs requiring immediate remediation.
- **[IMPORTANT] P1**: Correctness bugs, logic errors, or reliability issues that affect normal operation.
- **[MODERATE] P2**: Design issues, missing hardening, or technical debt that should be addressed in regular development cycles.

---

## Findings by Severity

### [CRITICAL] P0 -- Security / Data Loss (10)

#### P0-1. CCS: Shell injection in generated scripts

- **Location**: `conary-core/src/ccs/builder.rs`
- **Description**: Hook fields from package metadata are interpolated unquoted into shell scripts generated during CCS package builds. An attacker who controls package metadata (e.g., via a crafted recipe or upstream source) can inject arbitrary shell commands into the generated install/remove hooks.
- **Impact**: Arbitrary code execution as root during package installation. Any user installing a crafted CCS package executes attacker-controlled commands.
- **Fix**: Shell-escape all hook field values before interpolation using a dedicated escaping function. Consider switching to a structured execution model (pass arguments as arrays) rather than string interpolation into shell scripts.

#### P0-2. Repository: HTTP GPG key fetch

- **Location**: `conary-core/src/repository/gpg.rs`
- **Description**: GPG signing keys can be fetched over plain HTTP when configured with an `http://` URL. No scheme enforcement rejects non-TLS key sources.
- **Impact**: A man-in-the-middle attacker can substitute a GPG signing key during fetch, then sign and serve malicious packages that pass signature verification. This completely undermines the trust model.
- **Fix**: Enforce HTTPS for all GPG key URLs. Reject `http://` schemes with a clear error message. Add a `--allow-insecure-keys` escape hatch for air-gapped or local development environments only.

#### P0-3. Repository: Empty Remi checksums accepted

- **Location**: `conary-core/src/repository/remi.rs`
- **Description**: Packages fetched from Remi repositories with empty or missing checksum fields pass the verification step without error. The checksum comparison logic treats an empty expected value as a match.
- **Impact**: A compromised or misconfigured Remi server can serve tampered packages that bypass integrity verification. The client accepts them silently.
- **Fix**: Reject packages with empty or absent checksums. Require at least one valid hash (SHA-256 preferred) before accepting any downloaded package. Log a warning and abort the transaction.

#### P0-4. Trust: verify vs verify_strict signature bypass

- **Location**: `conary-core/src/trust/keys.rs`, `conary-core/src/trust/verify.rs`
- **Description**: The `verify` function (non-strict mode) silently accepts unsigned packages, returning success when no signature is present. Only `verify_strict` actually enforces signature presence. The default code paths use `verify`, not `verify_strict`.
- **Impact**: Signature verification is effectively disabled by default. Unsigned or stripped packages are accepted without any warning, defeating the entire TUF-based trust chain.
- **Fix**: Make `verify_strict` the default behavior. Require explicit opt-in via configuration or CLI flag (`--allow-unsigned`) to accept packages without signatures. Emit a prominent warning when unsigned acceptance is enabled.

#### P0-5. Trust: No TUF metadata size limit

- **Location**: `conary-core/src/trust/client.rs`, `conary-core/src/trust/metadata.rs`
- **Description**: TUF metadata downloads have no maximum size limit. The client reads the entire response body into memory without checking Content-Length or imposing a cap.
- **Impact**: A malicious or compromised repository can serve an arbitrarily large metadata file, causing the client to exhaust memory (OOM). This is a denial-of-service vector against any client that syncs with the repository.
- **Fix**: Add a configurable maximum metadata size (default: 10 MB). Check Content-Length before downloading and enforce the limit during streaming reads. Abort with a clear error if exceeded.

#### P0-6. Container: chroot not pivot_root

- **Location**: `conary-core/src/container/mod.rs`
- **Description**: The scriptlet sandbox uses `chroot` to isolate package scriptlets. However, `chroot` is trivially escapable by any process with `CAP_SYS_CHROOT` (which root has by default). A malicious scriptlet can call `chroot(".")` followed by `chdir("../../..")` to escape the sandbox.
- **Impact**: A malicious package scriptlet can escape the sandbox and access or modify the host filesystem, achieving full root compromise during package installation.
- **Fix**: Replace `chroot` with `pivot_root` combined with mount namespace isolation. After `pivot_root`, unmount the old root filesystem. Alternatively, use `mount` namespaces with `MS_MOVE` to ensure no reference to the host root remains accessible. Drop `CAP_SYS_CHROOT` from the capability bounding set.

#### P0-7. Container: Environment variables leak to sandboxed scriptlets

- **Location**: `conary-core/src/container/mod.rs`
- **Description**: The parent process environment is inherited wholesale by sandboxed scriptlets. No environment sanitization occurs before executing scriptlet commands.
- **Impact**: Secrets in the environment (API keys, tokens, SSH agent sockets, cloud credentials) are accessible to package scriptlets. A malicious package can exfiltrate these values.
- **Fix**: Clear the entire environment before scriptlet execution. Define an explicit allowlist of safe environment variables (e.g., `PATH`, `HOME`, `TERM`, `LANG`) and only pass those. Set `PATH` to a known-safe minimal value.

#### P0-8. Remi Server: SSRF via unvalidated recipe_url

- **Location**: `conary-server/src/server/handlers/recipes.rs:41-50`
- **Description**: The admin recipe-build endpoint accepts a `recipe_url` field and passes it directly to the build system without URL validation. Any URL scheme is accepted and the server will fetch from any address.
- **Impact**: An attacker with admin access (or who compromises admin credentials) can use the server to probe internal networks, access cloud instance metadata (e.g., `http://169.254.169.254/`), or hit internal services. This is a server-side request forgery (SSRF) vulnerability.
- **Fix**: Validate URL scheme (allow `https://` only by default). Reject URLs pointing to private/reserved IP ranges (RFC 1918, link-local, loopback). Resolve the hostname and verify the resolved IP is not private before initiating the fetch. Add an allowlist configuration for trusted recipe sources.

#### P0-9. Daemon: PID reuse race in admin group check

- **Location**: `conary-server/src/daemon/auth.rs:100-119`
- **Description**: The Unix domain socket authentication reads `SO_PEERCRED` to get the peer's PID, then reads `/proc/{pid}/status` to check group membership. Between these two operations, the original process can exit and its PID can be reused by a different process.
- **Impact**: An unprivileged process could race to occupy a recycled PID and gain admin access to the daemon's control socket. While the window is small, it is exploitable with targeted PID recycling techniques.
- **Fix**: Cross-check that the UID from `/proc/{pid}/status` matches the UID from `SO_PEERCRED`. Better yet, use `getgrouplist(3)` with the UID from `SO_PEERCRED` directly, avoiding `/proc` entirely. The UID from `SO_PEERCRED` is captured atomically at connection time and cannot be spoofed.

#### P0-10. Adopt/System: Stale DB connection after bulk adoption

- **Location**: `src/commands/generation/takeover.rs:163`
- **Description**: A database connection is opened before calling `cmd_adopt`, then reused after adoption completes to build the first system generation. The `cmd_adopt` function uses its own connection internally, so packages adopted during that call are not visible through the pre-existing connection (SQLite WAL visibility rules).
- **Impact**: The first generation built after a system takeover may be missing some or all adopted packages. Booting into this generation would produce a broken system with missing packages.
- **Fix**: Close and reopen the database connection after `cmd_adopt` returns. Alternatively, restructure to pass a single connection through the entire pipeline, or call `PRAGMA wal_checkpoint` before reusing the connection.

---

### [IMPORTANT] P1 -- Bugs / Logic Errors (30)

#### Feature 1: Package Parsing

##### P1-1. RPM TOCTOU double-read

- **Location**: `conary-core/src/packages/rpm.rs`
- **Description**: RPM parsing reads the package file twice -- once for header metadata extraction and once for payload decompression. Between reads, the file could be modified on disk (time-of-check-to-time-of-use).
- **Impact**: If the package file is replaced between reads (e.g., on a shared filesystem or via symlink manipulation), the metadata may not match the actual payload content. This could lead to installing the wrong files under a trusted package's identity.
- **Fix**: Read the entire RPM into memory once (or use a memory-mapped file with a consistent snapshot), then parse headers and payload from the single buffer. Alternatively, open the file once and seek within the same file descriptor.

##### P1-2. DEB 3-4x archive open

- **Location**: `conary-core/src/packages/deb.rs`
- **Description**: DEB parsing opens the archive file 3-4 times during a single parse operation -- once for format detection, once for control extraction, once for data extraction, and potentially once more for metadata. Each open is an independent file handle.
- **Impact**: Same TOCTOU concern as P1-1, plus unnecessary I/O overhead. On slow storage, this causes measurable performance degradation for large package sets.
- **Fix**: Open the DEB archive once and pass the file handle or buffer through all parsing stages. The `ar` archive format supports sequential reads through the three members (debian-binary, control.tar, data.tar).

##### P1-3. Semver drops 4th+ version components

- **Location**: `conary-core/src/version/`
- **Description**: The version parser truncates version strings with more than three numeric components (e.g., `1.2.3.4` becomes `1.2.3`). Many RPM packages use 4-component versions (e.g., kernel `6.18.13.200`).
- **Impact**: Packages with 4+ component versions are compared incorrectly. Version `1.2.3.4` and `1.2.3.5` appear identical, causing the resolver to make wrong upgrade decisions and potentially skipping security updates.
- **Fix**: Store the full version string and compare all components. Use the existing pre-release/build metadata fields to carry additional components, or extend the version struct to support arbitrary component counts.

##### P1-4. RPM ANY flag semantic mismatch

- **Location**: `conary-core/src/packages/rpm.rs`
- **Description**: The RPM parser treats the `RPMSENSE_ANY` flag as a literal version constraint rather than as "any version satisfies." This causes dependencies with the ANY flag to be treated as unsatisfiable when no exact version match exists.
- **Impact**: Some RPM packages with ANY-flagged dependencies fail to resolve correctly, causing spurious dependency errors during installation.
- **Fix**: When `RPMSENSE_ANY` is set, treat the dependency as satisfied by any available version of the named package, ignoring the version comparison fields.

#### Feature 2: Dependency Resolution

##### P1-5. solve_removal abandons SAT solver

- **Location**: `conary-core/src/resolver/engine.rs`
- **Description**: The `solve_removal` function bypasses the SAT solver entirely and uses a simple graph traversal to determine which packages to remove. This ignores complex dependency relationships that only the SAT solver can properly evaluate (e.g., alternative providers, virtual packages).
- **Impact**: Removing a package may leave the system in an inconsistent state where remaining packages have unsatisfied dependencies. The simplified removal logic cannot detect cases where removal breaks transitive dependencies through alternative providers.
- **Fix**: Use the SAT solver for removal planning as well. Express the removal as a constraint (package version = 0) and let resolvo compute the full impact, including any cascading removals needed.

##### P1-6. Virtual provides silently dropped

- **Location**: `conary-core/src/resolver/provider.rs`
- **Description**: When constructing the SAT problem, virtual package provides (e.g., `provides: mail-transport-agent`) are silently dropped if they don't map to a concrete package name in the database. No warning is emitted.
- **Impact**: Dependencies on virtual packages fail to resolve even when a provider is installed. This affects packages that depend on abstract capabilities rather than concrete package names (common in Debian/RPM ecosystems).
- **Fix**: Track virtual provides as first-class entries in the resolver's package universe. Map virtual names to their concrete providers before SAT solving begins. Emit a warning when a virtual dependency has no known provider.

##### P1-7. intern_version_set not deduplicated

- **Location**: `conary-core/src/resolver/sat.rs`
- **Description**: The `intern_version_set` function creates a new interned entry for every call, even when the same version set (name + constraint) has been interned before. This causes the SAT solver to treat identical constraints as distinct variables.
- **Impact**: The solver's search space grows unnecessarily, degrading performance for large dependency graphs. In pathological cases, this can cause the solver to return suboptimal solutions or time out.
- **Fix**: Add a deduplication cache (HashMap) keyed by (package_name, version_constraint). Return the existing interned ID when a duplicate is encountered.

##### P1-8. Lexicographic LanguageDep comparison

- **Location**: `conary-core/src/resolver/engine.rs`
- **Description**: Language dependencies (e.g., Python module versions, Ruby gem versions) are compared as plain strings using lexicographic ordering rather than being parsed as version numbers.
- **Impact**: Version `1.9` sorts higher than `1.10` lexicographically, causing incorrect dependency resolution for language-ecosystem packages. This can result in installing incompatible versions.
- **Fix**: Parse language dependency versions using the appropriate version comparison logic (PEP 440 for Python, semver for Node/Ruby, etc.) rather than string comparison.

#### Feature 3: Database

##### P1-9. Raw BEGIN/COMMIT bypasses rusqlite transaction API

- **Location**: `conary-core/src/db/mod.rs`
- **Description**: Several database operations use raw `EXECUTE "BEGIN"` / `EXECUTE "COMMIT"` SQL statements instead of rusqlite's transaction API (`conn.transaction()`). This bypasses rusqlite's automatic rollback-on-drop safety net.
- **Impact**: If a panic or early return occurs between `BEGIN` and `COMMIT`, the transaction is left open. This can cause the database to remain locked and subsequent operations to fail with "database is locked" errors.
- **Fix**: Replace all raw `BEGIN`/`COMMIT` pairs with `conn.transaction()` calls. The `Transaction` struct's `Drop` implementation automatically rolls back on failure, preventing leaked transactions.

##### P1-10. batch_insert no transaction

- **Location**: `conary-core/src/db/models/file_entry.rs`
- **Description**: The `batch_insert` function for file entries executes hundreds or thousands of individual INSERT statements without wrapping them in a transaction. Each INSERT is auto-committed individually.
- **Impact**: A crash partway through a batch insert leaves the database in an inconsistent state with partially-inserted file entries. Performance is also severely degraded -- individual auto-commits are 10-100x slower than batched transactions on SQLite.
- **Fix**: Wrap the entire batch_insert operation in a single transaction. Use `conn.transaction()` with a closure that performs all inserts, committing atomically on success.

##### P1-11. db::open missing WAL pragma

- **Location**: `conary-core/src/db/mod.rs`
- **Description**: The `db::open` function does not set `PRAGMA journal_mode=WAL` when opening the database. Some code paths assume WAL mode is active (e.g., concurrent reads during transactions), but it is only set in some callers, not at the open level.
- **Impact**: Without WAL mode, readers block writers and vice versa. This causes lock contention during operations that read and write concurrently (e.g., dependency resolution while installing). Performance degrades and "database is locked" errors can occur under load.
- **Fix**: Set `PRAGMA journal_mode=WAL` in `db::open` immediately after opening the connection. This ensures all callers benefit from WAL mode regardless of their individual configurations.

##### P1-12. Format-string column name injection

- **Location**: `conary-core/src/db/models/trove.rs`
- **Description**: Column names are interpolated into SQL query strings using `format!()` rather than being validated against an allowlist. While column names typically come from internal code rather than user input, any code path that passes user-derived data as a column name enables SQL injection.
- **Impact**: If any caller passes unsanitized input as a column name, an attacker can inject arbitrary SQL. Even without direct exploitation, this pattern is fragile and prone to introducing injection vulnerabilities during future refactoring.
- **Fix**: Validate column names against an explicit allowlist of known column names before interpolation. Use an enum for column selection where possible, converting to string only at the query-building layer.

#### Feature 4: Filesystem and Transactions

##### P1-13. Symlinks bypass validate_symlink_target during staging

- **Location**: `conary-core/src/filesystem/deployer.rs`
- **Description**: The `validate_symlink_target` function is called only during the final deployment phase. During the staging phase, symlinks are created without target validation. A symlink in a package can point to any path, including targets outside the intended installation prefix.
- **Impact**: A crafted package can create symlinks pointing to sensitive locations (e.g., `/etc/shadow` -> attacker-controlled path). When subsequent operations follow the symlink, they read or write the wrong file.
- **Fix**: Call `validate_symlink_target` during staging as well as deployment. Reject symlinks whose resolved target escapes the package's installation prefix. Use `Path::canonicalize` with care (it follows symlinks), or implement manual component-by-component resolution.

##### P1-14. Symlink backup encoding not rollback-safe

- **Location**: `conary-core/src/transaction/journal.rs`
- **Description**: When backing up symlinks for rollback, the journal stores the symlink target as a UTF-8 string. However, symlink targets on Linux can contain arbitrary bytes (they are not required to be valid UTF-8). Non-UTF-8 targets are silently corrupted during backup encoding.
- **Impact**: Rolling back a transaction that modified a symlink with a non-UTF-8 target restores an incorrect (corrupted) symlink. The original file or directory the symlink pointed to becomes inaccessible.
- **Fix**: Store symlink targets as raw byte sequences (`Vec<u8>`) in the journal, not as UTF-8 strings. Use `std::os::unix::ffi::OsStrExt` to handle the conversion correctly.

##### P1-15. Staged copy missing fsync

- **Location**: `conary-core/src/filesystem/cas.rs`
- **Description**: Files staged into the content-addressable store are not `fsync`'d after writing. The directory entry is also not synced (`fsync` on the parent directory).
- **Impact**: A system crash (power failure, kernel panic) during or shortly after staging can result in zero-length or partially-written files in the CAS. Since the CAS uses content hashes as filenames, a corrupted file will persist under a valid hash, causing silent data corruption during future deployments.
- **Fix**: Call `file.sync_all()` after writing each staged file, and `fsync` the parent directory after creating the new entry. Consider using `O_DSYNC` for the write if performance is critical.

##### P1-16. Delta applier no size limit

- **Location**: `conary-core/src/delta/`
- **Description**: The binary delta applier reads the entire delta payload into memory without checking its size. Deltas are fetched from remote repositories and could be arbitrarily large.
- **Impact**: A malicious or compromised repository can serve a delta that exhausts client memory (OOM), crashing the package manager. This is a denial-of-service vector.
- **Fix**: Add a configurable maximum delta size (default: 500 MB or proportional to target package size). Check Content-Length before downloading and enforce the limit during streaming application. Abort with a clear error if exceeded.

#### Feature 5: Canonical Identity

##### P1-17. Unanchored regex patterns

- **Location**: `conary-core/src/resolver/canonical.rs`
- **Description**: Regex patterns used for canonical name matching are not anchored with `^` and `$`. A pattern intended to match `^libfoo\.so$` will also match `notlibfoo.so.bak` or any string containing the pattern as a substring.
- **Impact**: Canonical identity resolution can produce false-positive matches, linking unrelated packages or files to the wrong canonical identity. This affects dependency resolution and file conflict detection.
- **Fix**: Anchor all regex patterns with `^` (start) and `$` (end). Audit all regex construction sites to ensure anchoring is applied consistently.

##### P1-18. Raw BEGIN in appstream canonical resolution

- **Location**: `conary-core/src/db/models/canonical.rs`
- **Description**: The appstream canonical resolution code uses raw `EXECUTE "BEGIN"` for transactions instead of rusqlite's transaction API, same pattern as P1-9.
- **Impact**: Same as P1-9 -- leaked transactions on panic or early return, causing database lock-up.
- **Fix**: Same as P1-9 -- replace with `conn.transaction()`.

##### P1-19. set_mixing_policy silent no-op

- **Location**: `conary-core/src/resolver/canonical.rs`
- **Description**: The `set_mixing_policy` function accepts the policy value but does not persist it or propagate it to the resolver. It silently succeeds without effect.
- **Impact**: Users who configure a mixing policy (controlling how packages from different repositories interact) believe it is active when it is not. This can lead to unexpected package combinations and version conflicts.
- **Fix**: Implement the full persistence and enforcement path for mixing policies. Store the policy in the database and read it during resolver initialization. If the feature is not yet ready, return an error indicating it is unimplemented rather than silently succeeding.

#### Feature 6: System Model

##### P1-20. Cached collections bypass signature verification

- **Location**: `conary-core/src/model/remote.rs`
- **Description**: When a collection's metadata is served from local cache, signature verification is skipped. Only fresh downloads go through the signature check path.
- **Impact**: If an attacker can poison the local cache (e.g., via a previous MITM attack or local file manipulation), the poisoned metadata is trusted indefinitely without re-verification. Signature verification becomes a one-time check rather than continuous assurance.
- **Fix**: Verify signatures on cached collection metadata at every load, not just on download. Cache the verification result with a TTL if performance is a concern, but always re-verify on fresh process startup.

##### P1-21. Content hash mismatch silently accepted

- **Location**: `conary-core/src/model/lockfile.rs`
- **Description**: When loading a model lockfile, a content hash mismatch between the stored hash and the computed hash of the lockfile contents is logged as a warning but does not cause the operation to fail.
- **Impact**: A tampered or corrupted lockfile is silently accepted, potentially causing the system to install the wrong package versions. This defeats the purpose of content hashing.
- **Fix**: Treat content hash mismatches as errors. Abort the operation and prompt the user to regenerate the lockfile. Provide a `--force` flag for override in recovery scenarios.

##### P1-22. Non-deterministic canonical_json HashMap ordering

- **Location**: `conary-core/src/model/signing.rs`
- **Description**: The `canonical_json` function serializes a `HashMap` to produce a canonical JSON representation for signing. However, `HashMap` iteration order is non-deterministic in Rust (randomized per-process by default).
- **Impact**: The same model data produces different canonical JSON on different runs, causing signature verification to fail intermittently. Signatures created on one invocation may not verify on the next.
- **Fix**: Replace `HashMap` with `BTreeMap` for all data structures that participate in canonical serialization, or sort the keys before serialization. Use `serde_json::to_string` with sorted keys, or implement a custom serializer that guarantees key ordering.

#### Feature 7: CCS

##### P1-23. Symlink/file collision via byte prefix

- **Location**: `conary-core/src/ccs/manifest.rs`
- **Description**: The CCS manifest format distinguishes files from symlinks using a single-byte prefix in the entry encoding. A crafted manifest can use an ambiguous prefix value to cause a symlink entry to be interpreted as a regular file or vice versa.
- **Impact**: A malicious CCS package can place a symlink where the manifest declares a file, or a file where a symlink is expected. This can be used to redirect file writes to attacker-controlled locations.
- **Fix**: Use distinct, non-overlapping byte ranges for file and symlink prefixes. Add validation that the entry type matches the actual filesystem object type during both build and install.

##### P1-24. Merkle root mismatch non-fatal

- **Location**: `conary-core/src/ccs/verify.rs`
- **Description**: When verifying a CCS package, a Merkle tree root hash mismatch is logged as a warning but does not cause the verification to fail. The package is still accepted for installation.
- **Impact**: A tampered CCS package with modified file contents passes verification. The Merkle tree, which is the primary integrity mechanism for CCS packages, is effectively advisory-only.
- **Fix**: Make Merkle root mismatch a hard error. Abort installation immediately when the computed root does not match the signed root. No override flag -- this is a fundamental integrity check.

##### P1-25. Chunked file hash not verified

- **Location**: `conary-core/src/ccs/chunking.rs`
- **Description**: When reassembling a file from CCS chunks, individual chunk hashes are verified but the final reassembled file's hash is not checked against the expected whole-file hash from the manifest.
- **Impact**: A correct set of chunks that are reassembled in the wrong order, or with a corrupted reassembly process, produces an incorrect file that passes chunk-level verification.
- **Fix**: After reassembling all chunks into the final file, compute the whole-file hash and compare it against the manifest's expected hash. Reject the file if they do not match.

##### P1-26. Path traversal in CCS converter

- **Location**: `conary-core/src/ccs/convert/converter.rs`
- **Description**: The CCS converter that imports legacy packages (RPM, DEB) into CCS format does not sanitize file paths extracted from the source package. Paths containing `../` components are preserved, allowing files to be placed outside the intended CCS package root.
- **Impact**: A crafted legacy package converted to CCS format can place files anywhere on the filesystem during installation, overwriting system files or installing backdoors.
- **Fix**: Normalize all paths during conversion. Strip leading `/` and reject any path containing `..` components. Use `Path::components()` to verify all components are `Normal` (not `ParentDir`, `RootDir`, or `Prefix`).

#### Feature 8: Repository

##### P1-27. FTP/HTTP metalink mirrors accepted

- **Location**: `conary-core/src/repository/metalink.rs`
- **Description**: The metalink parser accepts mirror URLs with `ftp://` and `http://` schemes without warning. These protocols transmit data in cleartext and are vulnerable to MITM attacks.
- **Impact**: Metalink files can redirect package downloads to insecure mirrors. An attacker who controls a network segment can intercept and modify package downloads from FTP/HTTP mirrors.
- **Fix**: Default to rejecting non-HTTPS mirrors from metalink files. Add a configuration option (`allow_insecure_mirrors = true`) for environments where insecure mirrors are intentionally used. Log a warning when insecure mirrors are present in metalink data.

##### P1-28. URL path injection

- **Location**: `conary-core/src/repository/client.rs`
- **Description**: Repository URL construction concatenates user-provided package names into URL paths without encoding special characters. A package name containing `/` or `..` can manipulate the request path.
- **Impact**: A crafted package name can redirect the repository client to fetch from an unintended path on the server, potentially downloading a different package than requested.
- **Fix**: URL-encode package names and version strings before path concatenation. Use the `url` crate's path segment API rather than string formatting to construct URLs.

##### P1-29. fetch_package no content hash verification

- **Location**: `conary-core/src/repository/download.rs`
- **Description**: After downloading a package file, the content hash is not verified against the expected hash from the repository metadata. The download is accepted based solely on HTTP status code and Content-Length.
- **Impact**: A MITM attacker or compromised mirror can serve a different file than expected. Without post-download hash verification, the substitution goes undetected.
- **Fix**: Compute the content hash (SHA-256) during download (streaming hash). After the download completes, compare against the expected hash from repository metadata. Reject and delete the file on mismatch.

#### Feature 9: Install/Remove/Update

##### P1-30. Dependency scriptlets bypass user sandbox mode

- **Location**: `src/commands/install/dependencies.rs:178`
- **Description**: When installing dependency packages, the sandbox mode is hardcoded to `SandboxMode::None` regardless of the user's configured sandbox preference. Only explicitly-requested packages respect the user's sandbox setting.
- **Impact**: If a user configures strict sandboxing, dependency packages' scriptlets still run unsandboxed. A malicious dependency can execute arbitrary code without sandbox restrictions.
- **Fix**: Propagate the user's configured `SandboxMode` to dependency installation. Use the same sandbox mode for all packages in a transaction, not just the explicitly-requested ones.

##### P1-31. Remove commits DB before filesystem deletion

- **Location**: `src/commands/remove.rs`
- **Description**: The remove command commits the database changes (marking the package as removed) before deleting the package's files from the filesystem. If the process crashes after the DB commit but before filesystem cleanup, the database says the package is gone but its files remain.
- **Impact**: Orphaned files consume disk space and may conflict with future installations of the same package. The package cannot be reinstalled without manual cleanup because the DB considers it already removed.
- **Fix**: Reverse the order: delete files first, then commit the DB change. Or better, use the transaction journal to record both operations and commit them atomically, with the recovery system cleaning up on crash.

##### P1-32. Blocklist exact-string match bypassable

- **Location**: `src/commands/install/blocklist.rs`
- **Description**: The package blocklist uses exact string comparison. A blocked package `foo` does not match `Foo`, `FOO`, or `foo ` (with trailing whitespace). There is no normalization of package names before comparison.
- **Impact**: Users who blocklist a package can be tricked into installing it via case variations or whitespace padding in the package name.
- **Fix**: Normalize package names (lowercase, trim whitespace) before blocklist comparison. Consider also matching against the canonical name to catch renamed packages.

##### P1-33. cmd_update treats any version difference as upgrade

- **Location**: `src/commands/update.rs`
- **Description**: The update command compares old and new version strings and proceeds with installation whenever they differ, without checking that the new version is actually newer. A "downgrade" is treated identically to an upgrade.
- **Impact**: A repository that publishes a lower version number (due to a rollback, mistake, or attack) causes clients to silently downgrade, potentially reintroducing fixed security vulnerabilities.
- **Fix**: Compare versions numerically and refuse to "update" to a lower version without explicit `--allow-downgrade` flag. Warn the user when a downgrade is detected.

##### P1-34. Batch installer pre-script failure leaves engine inconsistent

- **Location**: `src/commands/install/batch.rs`
- **Description**: When a pre-install scriptlet fails for one package in a batch, the batch installer logs the error but continues processing subsequent packages. The transaction engine's internal state still reflects the failed package as "in progress."
- **Impact**: The transaction engine's state becomes inconsistent with reality. Subsequent operations may attempt to reference the partially-installed package, causing cascading errors or incorrect rollback behavior.
- **Fix**: On pre-script failure, either abort the entire batch (safest) or properly mark the failed package as failed in the transaction engine state and skip all subsequent operations for that package (including post-scripts and DB registration).

#### Feature 10: Security and Trust

##### P1-35. Read-only remount silently dropped

- **Location**: `conary-core/src/container/mod.rs`
- **Description**: When setting up the scriptlet sandbox, the code attempts to remount certain paths as read-only. If the remount fails (e.g., due to missing mount namespace privileges), the error is silently ignored and the path remains writable.
- **Impact**: The sandbox's read-only protections are silently degraded. A scriptlet can modify paths that should be read-only, potentially altering system configuration or installing persistent backdoors.
- **Fix**: Treat remount failures as fatal errors. If read-only remounting cannot be achieved, abort the scriptlet execution with a clear error explaining that the sandbox could not be established.

##### P1-36. Landlock deny not enforced

- **Location**: `conary-core/src/container/mod.rs`
- **Description**: The Landlock filesystem access control setup constructs the ruleset but does not check the return value of the enforcement call. If Landlock is not supported by the kernel or the enforcement fails, execution continues without any filesystem access restriction.
- **Impact**: On kernels without Landlock support (pre-5.13) or when enforcement fails, scriptlets run with full filesystem access. The security boundary is silently absent.
- **Fix**: Check the Landlock enforcement return value. If it fails, fall back to a more restrictive alternative (e.g., seccomp, or refusing to run the scriptlet). At minimum, warn the user loudly that Landlock enforcement failed.

#### Feature 11: Automation and Triggers

##### P1-37. Double-wait bug on every trigger

- **Location**: `conary-core/src/trigger/`
- **Description**: The trigger execution code calls `wait()` on the child process twice -- once explicitly and once via the `Drop` implementation. The second `wait()` blocks indefinitely if the PID has been reused by another process (which will not exit on its own).
- **Impact**: Every trigger execution has a chance of hanging indefinitely if the PID is quickly recycled. On busy systems with high PID turnover, this manifests as random hangs during package installation.
- **Fix**: Use `Child::wait()` once and store the result. Set the child handle to a state that prevents the `Drop` implementation from waiting again (e.g., by calling `Child::try_wait()` or by taking ownership of the child's process handle).

##### P1-38. parse_duration panics on non-ASCII

- **Location**: `conary-core/src/automation/scheduler.rs`
- **Description**: The `parse_duration` function indexes into the input string by byte offset to split the numeric and unit parts. Non-ASCII characters (e.g., from locale-formatted numbers or UTF-8 input) cause a panic at the byte index boundary.
- **Impact**: Any automation schedule configuration containing non-ASCII characters crashes the package manager. This is triggered by user-provided configuration, making it an easily-hit denial of service.
- **Fix**: Use `char_indices()` instead of byte indexing. Parse the numeric prefix with a proper number parser that handles the split correctly regardless of character encoding.

##### P1-39. --yes flag never executes automation

- **Location**: `src/commands/automation.rs`
- **Description**: The `--yes` flag (auto-approve) is parsed and stored but never checked in the execution path. The automation command always prompts for confirmation regardless of the flag value.
- **Impact**: Non-interactive automation (cron jobs, CI pipelines) that passes `--yes` to skip prompts hangs waiting for user input that never arrives.
- **Fix**: Check the `--yes` flag before prompting. When set, skip the confirmation prompt and proceed directly to execution.

##### P1-40. Lexicographic version comparison in SQL

- **Location**: `conary-core/src/automation/check.rs`
- **Description**: The automation check for available updates compares version strings using SQL's string comparison (`WHERE version > ?`). This produces incorrect results for multi-component versions (e.g., `1.9` > `1.10` in string comparison).
- **Impact**: The automation system may miss available security updates or flag non-updates as updates, depending on version numbering. This silently degrades the automated security update feature.
- **Fix**: Perform version comparison in Rust code using the proper version comparison functions rather than in SQL. Fetch candidate versions and compare them programmatically.

#### Feature 12: Recipe and Bootstrap

##### P1-41. Stage 1/2 skip additional source checksums

- **Location**: `conary-core/src/bootstrap/stage1.rs`, `conary-core/src/bootstrap/stage2.rs`
- **Description**: Bootstrap stages 1 and 2 verify the checksum of the primary source archive but skip checksum verification for additional source files (patches, supplementary archives) listed in the recipe.
- **Impact**: A compromised mirror or MITM attacker can substitute malicious patches or supplementary sources during bootstrap. These are applied to the build without integrity verification, potentially backdooring the entire bootstrapped system.
- **Fix**: Verify checksums for all source files listed in the recipe, not just the primary archive. Reject any source file whose checksum does not match or is absent.

##### P1-42. Stage 0 seed no verification when checksum absent

- **Location**: `conary-core/src/bootstrap/stage0.rs`
- **Description**: The stage 0 bootstrap accepts seed packages without checksum verification when the seed manifest does not include a checksum field. No error or warning is emitted for missing checksums.
- **Impact**: The foundation of the entire bootstrapped system can be built from unverified binaries. A compromised seed mirror can substitute malicious seed packages.
- **Fix**: Require checksums for all seed packages. Reject seed manifests that omit checksums. Provide a `--trust-seed` override for controlled environments where seed integrity is verified through other means.

##### P1-43. Path traversal in derived patch application

- **Location**: `conary-core/src/derived/builder.rs`
- **Description**: The derived package builder applies patches from a patch directory without sanitizing the patch file paths. A patch file with `../` in its path can write to locations outside the build directory.
- **Impact**: A crafted derived package recipe can modify files outside the build sandbox, potentially altering the build system or other packages being built concurrently.
- **Fix**: Sanitize patch file paths before application. Reject paths containing `..` components. Resolve the full path and verify it remains within the build directory.

##### P1-44. Unquoted workdir in shell string

- **Location**: `conary-core/src/recipe/kitchen/cook.rs`
- **Description**: The working directory path is interpolated into shell commands without quoting. A working directory path containing spaces or shell metacharacters causes the shell command to be parsed incorrectly.
- **Impact**: Build directories with spaces in their names cause build failures or, worse, unintended command execution if the path contains shell metacharacters.
- **Fix**: Quote all path interpolations in shell commands using single quotes. Better yet, pass the working directory as an environment variable and reference it as `"$WORKDIR"` (with quotes) in the shell command.

#### Feature 13: Adopt and System Management

##### P1-45. compute_file_hash fallback produces fake placeholder hashes

- **Location**: `src/commands/adopt/packages.rs`
- **Description**: When `compute_file_hash` fails to hash a file (e.g., due to permissions, broken symlinks), it falls back to generating a placeholder hash string (e.g., `"unhashable:<path>"`) instead of propagating the error.
- **Impact**: Files that could not be hashed are recorded in the database with fake hashes. Future integrity checks silently fail for these files, and file-based deduplication in the CAS cannot detect duplicates or corruption.
- **Fix**: Propagate the error and let the caller decide how to handle unhashable files. At minimum, record the failure status explicitly in the database rather than storing a fake hash that looks like a real one.

##### P1-46. Label delegation cycle detection only depth-1

- **Location**: `src/commands/label.rs`
- **Description**: The label delegation system checks for direct cycles (A delegates to B, B delegates to A) but does not detect longer cycles (A -> B -> C -> A). The check only looks one level deep.
- **Impact**: A delegation chain with a 3+ node cycle causes infinite recursion during label resolution, eventually stack-overflowing or hanging.
- **Fix**: Implement proper cycle detection using a visited set during delegation traversal. Maintain a set of all labels seen during resolution and reject any delegation that would revisit a previously-seen label.

##### P1-47. cmd_rollback queries file list outside transaction

- **Location**: `src/commands/generation/commands.rs`
- **Description**: The rollback command queries the list of files to restore outside of the transaction that performs the actual rollback. Between the query and the rollback execution, other operations can modify the file list.
- **Impact**: A concurrent package operation during rollback can cause files to be missed or incorrectly restored. The rollback may produce an inconsistent filesystem state.
- **Fix**: Move the file list query inside the same transaction that performs the rollback. Use a single transaction for the entire read-modify-write cycle.

##### P1-48. Path traversal in copy_files_to_temp

- **Location**: `src/commands/adopt/system.rs`
- **Description**: The `copy_files_to_temp` function copies files from the system to a temporary directory for analysis. File paths from the package database are used directly without sanitization, allowing `../` traversal.
- **Impact**: A package that registered files with `../` in their paths can cause the adoption process to copy sensitive files from unintended locations into the temporary directory, potentially exposing them to less-privileged analysis steps.
- **Fix**: Sanitize file paths before constructing the destination path. Strip `../` components and verify the resolved path remains within the expected directory hierarchy.

#### Features 14: Remi Server

##### P1-49. RateLimiter/BanList HashMap never cleaned

- **Location**: `conary-server/src/server/security.rs`
- **Description**: The `RateLimiter` and `BanList` structures use `HashMap` entries keyed by client IP address. Entries are added on each request but never removed or expired. There is no eviction policy, TTL, or periodic cleanup.
- **Impact**: Over time (hours to days depending on traffic), the HashMap grows without bound, eventually exhausting server memory. This is a memory-based denial-of-service that requires only normal traffic volume, not an active attack.
- **Fix**: Add TTL-based expiration to rate limiter entries. Use a bounded data structure (e.g., `lru` crate) with a maximum capacity. Run periodic cleanup (every 60 seconds) to remove expired entries.

##### P1-50. find_latest_package prefix match crosses name boundaries

- **Location**: `conary-server/src/server/routes.rs`
- **Description**: The `find_latest_package` function uses a SQL `LIKE` prefix match (`name LIKE 'foo%'`) to find packages. This matches `foobar` when searching for `foo`, crossing package name boundaries.
- **Impact**: Queries for a short-named package may return a different, longer-named package. This can cause clients to download the wrong package when the exact name is not found.
- **Fix**: Use exact string matching (`name = ?`) instead of `LIKE` prefix matching. If prefix search is intentionally supported, document it clearly and ensure the API contract is understood by clients.

##### P1-51. Recipe download reads entire file into memory

- **Location**: `conary-server/src/server/handlers/recipes.rs`
- **Description**: Recipe download endpoints read the entire recipe file into memory before sending the response. Large recipe files (or a large number of concurrent recipe downloads) can exhaust server memory.
- **Impact**: Memory exhaustion on the Remi server, causing service degradation or OOM-kill. Exploitable by requesting many large recipe files concurrently.
- **Fix**: Use streaming file reads with `tokio::fs::File` and `hyper::Body::wrap_stream` (or equivalent) to send the file in chunks without loading it entirely into memory.

##### P1-52. Unsanitized filename in Content-Disposition header

- **Location**: `conary-server/src/server/routes.rs`
- **Description**: Package filenames are interpolated directly into the `Content-Disposition` HTTP response header without sanitization or quoting. A filename containing special characters (quotes, newlines, semicolons) can inject additional header directives.
- **Impact**: HTTP response header injection. An attacker who controls a package filename can manipulate response headers, potentially enabling cache poisoning or XSS via header injection.
- **Fix**: Sanitize the filename by removing or escaping special characters. Use RFC 6266 compliant quoting: `Content-Disposition: attachment; filename="sanitized-name"`. Reject filenames containing control characters or quotes.

##### P1-53. Missing validate_name + unescaped LIKE in reverse-deps

- **Location**: `conary-server/src/server/routes.rs`
- **Description**: The reverse dependency lookup endpoint accepts a package name from the URL path and uses it directly in a SQL `LIKE` clause without escaping `%` and `_` wildcards or validating the name format.
- **Impact**: An attacker can craft a package name containing `%` to match all packages in the database, causing a full table scan and large response. SQL wildcard injection enables information disclosure (enumerating all package names and their dependencies).
- **Fix**: Validate the package name against the allowed character set. Escape `%` and `_` characters in LIKE patterns, or use exact matching (`=`) when searching by name.

#### Features 15-16: Daemon and Federation

##### P1-54. Non-atomic check-then-insert in coalescer

- **Location**: `conary-server/src/federation/coalesce.rs`
- **Description**: The chunk coalescer checks whether a chunk exists, then inserts it if absent. These two operations are not atomic, creating a race condition when multiple federation peers submit the same chunk concurrently.
- **Impact**: Duplicate chunk entries in the database, wasting storage and potentially causing confusion during chunk routing. In the worst case, two peers simultaneously coalescing the same chunk set can corrupt the coalesced output.
- **Fix**: Use `INSERT OR IGNORE` or `INSERT ON CONFLICT DO NOTHING` to make the operation atomic. Alternatively, use a database transaction with serializable isolation for the check-and-insert sequence.

##### P1-55. Read lock held across async HTTP fetch

- **Location**: `conary-server/src/federation/peer.rs`
- **Description**: A `RwLock` read lock on the peer list is held while performing an async HTTP fetch to a remote peer. If the fetch takes a long time (network timeout), the lock blocks all write operations on the peer list for the duration.
- **Impact**: A slow or unresponsive peer causes the entire federation peer list to become read-only, preventing peer discovery updates, peer removal, and health checks from proceeding.
- **Fix**: Clone the necessary data from the peer list under the read lock, then release the lock before performing the HTTP fetch. Update the peer list with results after the fetch completes, acquiring the write lock only for the brief update.

##### P1-56. Route ordering conflict (dry-run vs :id)

- **Location**: `conary-server/src/daemon/routes.rs`
- **Description**: The daemon's route table has a conflict between the `/jobs/dry-run` route and the `/jobs/:id` route. Depending on route registration order, "dry-run" may be captured as a job ID parameter, causing the dry-run endpoint to return a "job not found" error.
- **Impact**: The dry-run functionality may be completely inaccessible depending on the router's matching order. This is a functional regression that prevents transaction preview.
- **Fix**: Register the `/jobs/dry-run` route before the `/jobs/:id` route to ensure literal paths take priority over parameterized paths. Alternatively, restructure the routes (e.g., `/jobs/actions/dry-run`) to avoid ambiguity entirely.

---

### [MODERATE] P2 -- Design Issues (25)

#### Feature 5: Canonical Identity

##### P2-1. set_mixing_policy silent no-op (design)

- **Location**: `conary-core/src/resolver/canonical.rs`
- **Description**: Beyond the P1 bug (P1-19), the mixing policy feature lacks a design for how policies should interact with the resolver. No documentation or architecture decision record exists for the intended behavior.
- **Impact**: Even after the code is fixed, the feature semantics are undefined. Different developers may implement conflicting behaviors.
- **Fix**: Write an architecture decision record (ADR) defining mixing policy semantics before implementing the feature. Define how policies interact with multi-repo resolution, pinning, and version constraints.

#### Feature 6: System Model

##### P2-2. Non-deterministic HashMap ordering (design)

- **Location**: `conary-core/src/model/signing.rs`
- **Description**: Beyond the P1 bug (P1-22), the broader model serialization uses `HashMap` in multiple places where deterministic ordering is important for reproducibility and debugging. The signing issue is the most critical instance, but the pattern exists throughout the model module.
- **Impact**: Non-deterministic serialization makes debugging difficult (different runs produce different output) and prevents reproducible model operations.
- **Fix**: Audit the entire model module and replace `HashMap` with `BTreeMap` in all serializable structures. Establish a coding guideline: any struct that implements `Serialize` must use ordered containers.

#### Feature 9: Install/Remove/Update

##### P2-3. Pre-remove scriptlet partial execution no recovery

- **Location**: `src/commands/remove.rs`
- **Description**: If a pre-remove scriptlet fails partway through execution (e.g., crashes or times out), there is no recovery mechanism. The partially-executed scriptlet's side effects are not reversed, and the removal proceeds regardless.
- **Impact**: Partial scriptlet execution can leave the system in an inconsistent state -- some cleanup done, some not. The user has no way to re-run the scriptlet or undo its partial effects.
- **Fix**: Design a scriptlet journaling system that records which scriptlet steps have been completed. On failure, either abort the removal (preserving the package) or provide a way to re-run the scriptlet from the point of failure.

##### P2-4. Changeset marked Applied on partial failure

- **Location**: `conary-core/src/transaction/mod.rs`
- **Description**: When a changeset partially fails (some packages installed, some failed), the changeset is marked as `Applied` in the database. There is no `PartiallyApplied` state to distinguish complete success from partial success.
- **Impact**: The system cannot distinguish between fully-applied and partially-applied changesets during recovery. A recovery operation may skip a partially-applied changeset, leaving the system in an inconsistent state.
- **Fix**: Add a `PartiallyApplied` changeset state. Track per-package status within the changeset. Recovery logic should identify partially-applied changesets and either complete or roll back the remaining operations.

##### P2-5. Adopted removal skips dependency check

- **Location**: `src/commands/remove.rs`
- **Description**: When removing an adopted (converted) package, the dependency check is skipped. The rationale is that adopted packages may have inaccurate dependency metadata, but this also means removing them can break dependent packages.
- **Impact**: Removing an adopted package can leave other packages with unsatisfied dependencies. The system may enter a broken state requiring manual intervention.
- **Fix**: Perform dependency checking for adopted packages using the same logic as native packages. If the dependency metadata is known to be unreliable, warn the user rather than silently skipping the check. Provide `--force` for override.

#### Features 14: Remi Server

##### P2-6. IPv6 Cloudflare ranges not checked in security middleware

- **Location**: `conary-server/src/server/security.rs`
- **Description**: The Cloudflare IP range allowlist (used to trust `X-Forwarded-For` headers) only includes IPv4 ranges. The server listens on IPv6 as well, so Cloudflare IPv6 traffic is not recognized as trusted.
- **Impact**: IPv6 clients behind Cloudflare have their rate limiting applied to the Cloudflare edge IP rather than their real IP. This can cause legitimate users to be rate-limited as a group, or allow attackers to bypass rate limiting by using IPv6.
- **Fix**: Add Cloudflare's IPv6 ranges to the allowlist. Fetch the current ranges from `https://www.cloudflare.com/ips-v6/` and keep them updated. Consider fetching ranges dynamically with a cache TTL.

##### P2-7. Two-lock TOCTOU in BanList

- **Location**: `conary-server/src/server/security.rs`
- **Description**: The `BanList` uses separate locks for the ban set and the ban count. Checking the count and then inserting into the set is not atomic, creating a race window where two concurrent requests for the same IP can both pass the count check and both be inserted.
- **Impact**: Minor: the ban count can drift from the actual set size. In extreme cases, more IPs can be banned than the configured maximum.
- **Fix**: Use a single lock (or a concurrent data structure) that covers both the ban count and the ban set. Perform the check-and-insert as a single atomic operation under one lock.

#### Features 15-16: Daemon and Federation

##### P2-8. Hardcoded GIDs for admin group check

- **Location**: `conary-server/src/daemon/auth.rs`
- **Description**: The daemon uses hardcoded GID values to identify the admin group rather than looking up the group by name. Different distributions assign different GIDs to the same group names.
- **Impact**: The daemon's admin authentication fails on distributions where the admin group has a different GID than hardcoded. This prevents legitimate administrators from accessing the daemon control socket.
- **Fix**: Look up the group GID by name at startup using `getgrnam(3)`. Make the group name configurable (default: `conary` or `wheel`). Cache the resolved GID for the process lifetime.

##### P2-9. manifest_allow_unsigned defaults true

- **Location**: `conary-server/src/federation/config.rs`
- **Description**: The federation configuration defaults `manifest_allow_unsigned` to `true`. This means federation manifest signature verification is opt-in rather than opt-out.
- **Impact**: New federation deployments accept unsigned manifests by default, trusting any peer's claims about available chunks without cryptographic verification. This undermines the federation trust model.
- **Fix**: Default `manifest_allow_unsigned` to `false`. Require explicit opt-in for unsigned manifests. Document the security implications in the configuration file comments.

##### P2-10. Body downloaded before size check in federation

- **Location**: `conary-server/src/federation/peer.rs`
- **Description**: When receiving chunk data from a federation peer, the entire body is downloaded before the size is checked against the expected chunk size. A malicious peer can send an arbitrarily large body.
- **Impact**: Memory exhaustion if a federation peer sends a very large chunk body. This is a DoS vector in the federation network.
- **Fix**: Check `Content-Length` before starting the download. Use a size-limited reader that aborts the download if the stream exceeds the expected size. Apply the limit during streaming, not after the full body is in memory.

##### P2-11. mDNS region hubs use HTTP

- **Location**: `conary-server/src/federation/mdns.rs`
- **Description**: Federation peers discovered via mDNS are contacted over plain HTTP for initial capability exchange. The mDNS discovery protocol does not include any authentication or integrity mechanism.
- **Impact**: An attacker on the local network can advertise a malicious federation peer via mDNS. The real server contacts it over HTTP, enabling MITM for the initial capability exchange and potentially poisoning the peer list.
- **Fix**: Use HTTPS for all federation peer communication, including initial mDNS-discovered connections. Require mTLS (mutual TLS) for federation peers, which is already supported for manually-configured peers but not for mDNS-discovered ones.

##### P2-12. Corrupt job kind/status defaults to valid value

- **Location**: `conary-server/src/daemon/jobs.rs`
- **Description**: When deserializing job records from the database, unrecognized `kind` or `status` values are mapped to default valid values (e.g., `JobKind::Install` and `JobStatus::Pending`) rather than causing an error.
- **Impact**: A corrupted or tampered database record is silently treated as a valid job with default parameters. This can cause unexpected package installations or status misreporting.
- **Fix**: Return an error for unrecognized job kind or status values. Log the corruption and skip the job rather than executing it with default parameters.

##### P2-13. Non-atomic check-then-insert in coalescer (design)

- **Location**: `conary-server/src/federation/coalesce.rs`
- **Description**: Beyond the P1 race condition (P1-54), the coalescer's overall design lacks idempotency. Reprocessing the same chunk set produces duplicate entries rather than being safely retriable.
- **Impact**: Federation resilience is reduced -- any retry or network partition recovery that re-sends chunks creates duplicates that must be manually cleaned up.
- **Fix**: Design the coalescer with idempotent operations. Use content-addressable keys (chunk hash) as the primary key, making duplicate inserts naturally idempotent.

#### Feature 13: Adopt and System Management

##### P2-14. check_stale_files loads all files into memory

- **Location**: `src/commands/adopt/system.rs`
- **Description**: The `check_stale_files` function loads the entire file list from the database into a `Vec` in memory before iterating. For systems with many adopted packages, this can be hundreds of thousands of file entries.
- **Impact**: High memory usage during stale file checks. On memory-constrained systems, this can cause OOM or swapping, degrading system performance.
- **Fix**: Use a streaming database cursor to iterate over file entries without loading them all into memory. Process files in batches of 1000 entries.

##### P2-15. Full system adoption as single transaction

- **Location**: `src/commands/adopt/takeover.rs`
- **Description**: The full system adoption (takeover) process runs as a single database transaction that adopts all packages. For a typical system with 500-2000 packages, this transaction can take several minutes and holds the database write lock the entire time.
- **Impact**: The database is write-locked for the entire adoption duration. Any concurrent operation that needs to write to the database (e.g., another terminal session) will block or timeout. A crash during adoption requires re-doing the entire process.
- **Fix**: Split adoption into batches of 50-100 packages per transaction. Track progress so that a crash only requires re-adopting the current batch, not the entire system. Display progress to the user.

##### P2-16. Double rollback possible

- **Location**: `src/commands/generation/commands.rs`
- **Description**: The rollback command does not check whether the target generation is the current generation or has already been rolled back to. Issuing a rollback to the current generation is a no-op that still performs filesystem operations.
- **Impact**: Unnecessary filesystem churn. In edge cases, a double rollback to a generation that is being concurrently modified can cause file conflicts.
- **Fix**: Check whether the target generation is already the active generation before proceeding. Return early with an informational message if no rollback is needed.

##### P2-17. switch_live no mount cleanup

- **Location**: `src/commands/generation/switch.rs`
- **Description**: When switching to a new live generation, the old generation's mount points (composefs overlays) are not cleaned up. They remain mounted until the next reboot.
- **Impact**: Stale mount points accumulate with each generation switch. This wastes mount table entries and can confuse system introspection tools. On long-running systems with frequent updates, the mount table grows large.
- **Fix**: Unmount the old generation's overlay after switching to the new one. Use `lazy unmount` (MNT_DETACH) to avoid blocking on open file handles. Track active mounts in a state file to clean up on next boot if the unmount fails.

##### P2-18. /proc/cmdline unsanitized in BLS entry generation

- **Location**: `src/commands/generation/boot.rs`
- **Description**: When generating Boot Loader Specification (BLS) entries, the kernel command line is read from `/proc/cmdline` and included in the generated entry without sanitization. Special characters or excessively long command lines are passed through verbatim.
- **Impact**: A compromised or malformed kernel command line is propagated to new BLS entries, potentially affecting boot behavior of new generations. Excessively long command lines may exceed bootloader limits.
- **Fix**: Parse `/proc/cmdline` into individual parameters, validate each against an allowlist of known-safe parameters, and reconstruct a sanitized command line. Limit total length to bootloader-safe bounds (typically 4096 bytes). Warn on unrecognized parameters.

##### P2-19. Path traversal in copy_files_to_temp (design)

- **Location**: `src/commands/adopt/system.rs`
- **Description**: Beyond the P1 bug (P1-48), the `copy_files_to_temp` function's design does not enforce a security boundary between the adoption analysis and the system. The temporary directory is created with default permissions and is not isolated from other processes.
- **Impact**: Other processes on the system can read or modify files in the temporary directory during adoption analysis, potentially influencing the adoption results.
- **Fix**: Create the temporary directory with restrictive permissions (0700). Use `mkdtemp` to ensure unique naming. Consider running the analysis in a mount namespace to isolate the temporary directory from other processes.

##### P2-20. Full system adoption single transaction (design)

- **Location**: `src/commands/adopt/takeover.rs`
- **Description**: Beyond the P2-15 implementation concern, the single-transaction adoption design means that any failure during adoption (even for one package) rolls back the entire adoption, losing all progress.
- **Impact**: A single problematic package (e.g., one with corrupt metadata) prevents adoption of the entire system. The user must fix the problematic package before any adoption can succeed.
- **Fix**: Implement a skip-on-failure mode that records failed packages and continues with the rest. Provide a summary of skipped packages at the end so the user can address them individually.

##### P2-21. Hardcoded GIDs (design implications)

- **Location**: `conary-server/src/daemon/auth.rs`
- **Description**: Beyond the P2-8 implementation issue, the daemon's authentication model ties authorization to Unix group membership. This is fragile and does not support more granular access control (e.g., read-only vs. admin operations).
- **Impact**: No granular authorization. Any member of the admin group has full access to all daemon operations. There is no audit trail for which user performed which operation.
- **Fix**: Implement a role-based access control (RBAC) system for the daemon. Define roles (admin, operator, viewer) with specific permission sets. Record the authenticated user identity with each operation for auditing.

##### P2-22. manifest_allow_unsigned defaults true (design implications)

- **Location**: `conary-server/src/federation/config.rs`
- **Description**: Beyond the P2-9 default value issue, the federation trust model lacks a key distribution mechanism for manifest signing. Even when `manifest_allow_unsigned` is set to `false`, there is no documented process for exchanging signing keys between federation peers.
- **Impact**: Enabling manifest verification requires manual key exchange, which is error-prone and does not scale. Federation operators may leave verification disabled because enabling it is too difficult.
- **Fix**: Implement an automated key exchange protocol for federation peers (e.g., using the mTLS certificates already required for federation communication to derive manifest signing keys). Document the key management lifecycle.

##### P2-23. Body downloaded before size check (design)

- **Location**: `conary-server/src/federation/peer.rs`
- **Description**: Beyond the P2-10 implementation issue, the federation protocol lacks a pre-flight size negotiation. Peers do not advertise chunk sizes before transmission, so the receiver cannot reject oversized chunks before any data is transmitted.
- **Impact**: Bandwidth is wasted on oversized chunks that are ultimately rejected. In a constrained-bandwidth federation (e.g., WAN links), this wastes expensive network resources.
- **Fix**: Add a chunk size field to the federation protocol's chunk advertisement message. Receivers can reject chunks that exceed their configured maximum before any data is transferred.

##### P2-24. mDNS region hubs use HTTP (design implications)

- **Location**: `conary-server/src/federation/mdns.rs`
- **Description**: Beyond the P2-11 transport issue, the mDNS discovery mechanism does not authenticate discovered peers. Any device on the local network can advertise itself as a federation hub.
- **Impact**: Rogue federation peers can join the federation network and serve malicious or outdated package data to other peers.
- **Fix**: Require discovered peers to present a valid federation certificate before being added to the peer list. Use the federation's mTLS CA as the trust anchor. Reject peers whose certificates are not signed by the federation CA.

##### P2-25. Corrupt job kind/status defaults (design implications)

- **Location**: `conary-server/src/daemon/jobs.rs`
- **Description**: Beyond the P2-12 implementation issue, the job system lacks integrity checks for the job queue. There is no checksum or version field on job records to detect corruption or schema mismatches.
- **Impact**: Database corruption or schema migration errors manifest as silent incorrect behavior rather than detectable errors.
- **Fix**: Add a schema version field to the job table. Validate job records against the expected schema version on read. Add a checksum or HMAC to job records for integrity verification.

---

## Recommendations

### Immediate (This Week)

Fix all 10 P0 findings. These are exploitable security vulnerabilities or data-loss bugs that can be triggered through normal usage or by a moderately-skilled attacker:

1. Shell-escape CCS hook fields (P0-1)
2. Enforce HTTPS for GPG key fetch (P0-2)
3. Reject empty Remi checksums (P0-3)
4. Make verify_strict the default (P0-4)
5. Add TUF metadata size limit (P0-5)
6. Replace chroot with pivot_root (P0-6)
7. Clear environment before scriptlets (P0-7)
8. Validate recipe_url in Remi (P0-8)
9. Fix PID reuse race in daemon auth (P0-9)
10. Reopen DB connection after adoption (P0-10)

### Short-Term (This Month)

Fix P1 findings related to:
- **Transaction safety**: P1-10 (batch_insert no transaction), P1-14 (symlink backup), P1-15 (missing fsync), P1-31 (remove order), P1-34 (batch engine state)
- **Signature verification**: P1-20 (cached collection bypass), P1-24 (Merkle root non-fatal), P1-29 (fetch no hash verification)
- **Input validation**: P1-12 (column injection), P1-13 (symlink staging), P1-26 (CCS path traversal), P1-28 (URL path injection), P1-43 (derived patch traversal), P1-48 (copy_files_to_temp traversal)
- **Sandbox integrity**: P1-30 (dependency sandbox bypass), P1-35 (remount silently dropped), P1-36 (Landlock not enforced)

### Medium-Term (This Quarter)

Address remaining P1 findings and all P2 design issues during regular development cycles. Prioritize:
- Version comparison correctness (P1-3, P1-8, P1-40)
- Memory management in server components (P1-49, P1-51, P2-14)
- Federation trust model (P2-9, P2-11, P2-22, P2-24)
- Transaction state tracking (P2-4, P2-15, P2-20)

---

## Feature Review Status

| # | Feature Area | P0 | P1 | P2 | Status |
|---|---|---|---|---|---|
| 1 | Package Parsing | 0 | 4 | 0 | Reviewed |
| 2 | Dependency Resolution | 0 | 4 | 0 | Reviewed |
| 3 | Database | 0 | 4 | 0 | Reviewed |
| 4 | Filesystem and Transactions | 0 | 4 | 0 | Reviewed |
| 5 | Canonical Identity | 0 | 3 | 1 | Reviewed |
| 6 | System Model | 0 | 3 | 1 | Reviewed |
| 7 | CCS | 1 | 4 | 0 | Reviewed |
| 8 | Repository | 2 | 3 | 0 | Reviewed |
| 9 | Install/Remove/Update | 0 | 5 | 3 | Reviewed |
| 10 | Security and Trust | 2 | 2 | 0 | Reviewed |
| 11 | Automation and Triggers | 0 | 4 | 0 | Reviewed |
| 12 | Recipe and Bootstrap | 0 | 4 | 0 | Reviewed |
| 13 | Adopt and System Management | 1 | 4 | 5 | Reviewed |
| 14 | Remi Server | 1 | 5 | 2 | Reviewed |
| 15-16 | Daemon and Federation | 1 | 3 | 5 | Reviewed |
| | **Totals** | **10** | **56** | **25** | |

---

*Review conducted 2026-03-06. Next review cycle: TBD after P0 remediation is complete.*
