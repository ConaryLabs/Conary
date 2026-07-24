---
last_updated: 2026-07-24
revision: 26
summary: Route foreign lifecycle authority through formal shell parsing and exact helper contracts
---

# CCS Module (conary-core/src/ccs/)

Conary's native package format. Handles building, signing, policy enforcement,
declarative hooks, legacy format conversion, and OCI export.

## Data Flow: Package Build

```
ccs.toml (manifest)
     |
CcsBuilder::new(manifest, source_dir)
     |
  Walk source directory
     |
  For each file:
     +-- Compute SHA-256 hash
     +-- Apply PolicyChain (Keep / Replace / Skip / Reject)
     +-- Classify into component (explicit override or auto)
     +-- Optional: split into CDC chunks (FastCDC)
     |
  Group files by component -> ComponentData
     |
  BuildResult { manifest, components, files, blobs, chunk_stats }
     |
  Sign manifest (Ed25519) -> embed PackageSignature
     |
  Output .ccs archive (tar.gz with MANIFEST.cbor + MANIFEST.toml + objects/)
```

## Key Types

| Type | File | Purpose |
|------|------|---------|
| `CcsManifest` | manifest.rs | Root ccs.toml structure (package, provides, requires, hooks, policy, etc.) |
| `ManifestProvenance` | manifest_provenance.rs | Provenance DTOs embedded by the root manifest, including hermetic evidence, build attestations, and foreign conversion boundaries |
| `BuildAttestationEnvelope` | attestation.rs | Signed M2 release-publish attestation payload and verification helpers |
| `CcsBuilder` | builder.rs | Builds a CCS package from manifest + source directory |
| `BuildResult` | builder.rs | Output: manifest, components, files, blobs, total_size |
| `CcsPackage` | package.rs | Parsed .ccs file ready for installation via PackageFormat trait |
| `AuthorityDocumentV2` | v2/schema.rs | Signed CCS v2 native package authority |
| `BinaryManifest` | binary_manifest.rs | CBOR-encoded compact manifest (FORMAT_VERSION=1) |
| `SigningKeyPair` | signing.rs | Ed25519 key generation, signing, file I/O |
| `PackageSignature` | signing.rs | Embedded signature with algorithm, key_id, timestamp |
| `HookExecutor` | hooks/ | Runs declarative hooks with rollback tracking |
| `LegacyScriptletBundle` | legacy_scriptlets.rs | Converted RPM/DEB/Arch scriptlet decisions and local replay policy |
| `BuildPolicy` (trait) | policy.rs | Pluggable build policy (DenyPaths, StripBinaries, FixShebangs, etc.) |
| `EnhancementEngine` (trait) | enhancement/ | Exact post-conversion provenance recording |

## Submodules

**manifest.rs and manifest_provenance.rs** -- `ccs::manifest` remains the root
manifest schema and validation owner. Declarative hook schemas, capability
validation, and hook reversibility live in `ccs::manifest::hooks` and are
re-exported through that root entrypoint. The provenance DTOs live in
`ccs::manifest_provenance` and are likewise re-exported so existing imports
keep working. M2 release publish stores hermetic evidence, signed
build-attestation envelopes, and foreign conversion boundaries in manifest
provenance. Artifact-form `conary publish <pkg.ccs> <target>` is allowed only
after `repository::static_repo::publish_gate` verifies package signatures,
TOML integrity, attestation authority, output identity, command-risk evidence,
and foreign-boundary hashes.

**hooks/** -- Declarative hook executors. Pre-install order: groups, users,
directories. Post-install order: systemd, tmpfiles, sysctl, alternatives.
All operations respect a target_root parameter for bootstrap/container use.

Hook types: User, Group, Directory, Systemd, Tmpfiles, Sysctl, Alternatives.

**convert/** -- Legacy (RPM/DEB/Arch) to CCS conversion. Builds scriptlet
decisions from the adapter registry, blocked-class registry, support matrix,
replay policy, and target compatibility checks. Declarative manifest hooks are
emitted only from adapter-backed typed evidence. Shell bodies are parsed with
the tree-sitter Bash grammar into command nodes that retain command/argument
provenance and execution context. Malformed shell produces a typed parser
diagnostic and no guessed commands. Text-pattern detections remain advisory
metadata for review diagnostics and can never grant compatibility,
publication, mutation, or security authority. Remaining scripts are
preserved for guarded local replay or review when they cannot be safely
replaced. The authoritative package-manager lifecycle and helper-source map is
`docs/specs/foreign-package-lifecycle-contracts.md`.

`adapters.rs` is the thin registry and authority gate;
`adapters/builtin.rs` owns cross-distribution implementations;
`debian_adapters.rs`, `selinux_adapters.rs`, and `apparmor_adapters.rs` own
provider-specific grammars. Complete adapter results are downgraded to typed
discovery-only evidence unless the AST form is literal and unconditional or an
adapter validates an exact documented expansion grammar.

`converter.rs` remains the conversion orchestration hub.
`converter/evidence.rs` owns foreign conversion evidence and command-risk
projection, while `converter/authority.rs` owns scriptlet classification and
the projection of complete adapter effects into native manifest authority.
Tests live under `converter/tests/`.

The `dpkg-maintscript-helper/v1` adapter parses the four documented dpkg
actions and the required `-- "$@"` forwarding contract. `rm_conffile` is a
complete native replacement when the obsolete path is absent from the new
payload because Conary's generation `/etc` three-way merge removes unchanged
package configuration and preserves a user-modified orphan. The other three
actions remain typed partial evidence with the missing native transition model
named explicitly.

The `sysctl/v1` adapter projects only narrow, validated
`sysctl -w <key>=<value>` invocations into native `hooks.sysctl`; broad forms
such as `sysctl -p` and denied security-sensitive keys remain blocked. One
validated write still counts as complete native replacement evidence, but
public-ready conversion additionally requires the target profile to allow the
exact sysctl key. Missing target-profile context and supported-but-unallowed
keys stay `private-review`; the current public fixture uses `kernel.example`,
while `net.ipv4.ip_forward` remains private-review evidence.
The `setuid-mode/v1` adapter projects only payload-executable
`chmod u+s` or `chmod 4xxx` forms into native file mode authority plus an exact
`policy.allow_setuid_paths` build-policy allowlist entry. The
`file-capability/v1` adapter separately projects known Linux
`setcap cap_*=+ep <payload-executable>` grants into
`[[file_capabilities]]`. The manifest still validates `[[file_capabilities]]`
against the known Linux capability table. Public-ready conversion is narrower:
the first public allowlist is `cap_net_bind_service`; other known capability
names remain valid native manifest authority but non-public conversion
evidence. Mutable live-root installs apply that authority after file deployment
and before DB commit. Generation-aware installs preserve the same authority
only when Conary persists the installed file-capability rows, attaches
`security.capability` during generation runtime-input collection, publishes a
non-deferred generation, and emits the expected capability-xattr count through
generation inspection metadata. Setcap removal,
inheritable/process/ambient capability forms, setgid, broad `chmod +s`, unknown
capability names, and non-payload privilege mutations remain blocked/private.
Supported SELinux scriptlet forms are modeled as
`selinux-policy/v1` effects and bridged into generic `SecurityPolicyIntent`
metadata, so Fedora-origin policy declarations can be portable to Arch or
Debian targets without requiring SELinux on those targets. Generic intent
records provider, operation, scope, fallback, payload evidence, and
reconciliation state while preserving provider-specific effect evidence.
The `apparmor-policy/v1` adapter projects only payload-backed
`apparmor_parser -r|--replace /etc/apparmor.d/<profile>` reloads into generic
`SecurityPolicyIntent` metadata with dormant optional-policy fallback. AppArmor
mode changes, disable/status helpers, broad reloads, and unbacked paths remain
blocked/private and use `block-on-enforcing-target` fallback when classified as
review intent. Aggregate scriptlet fidelity is derived only from typed lifecycle
coverage in the durable scriptlet bundle; the retired regex analyzer and its
parallel guessed-hook score no longer exist.
Future LSM expansion must add target-provider facts and content semantics
before any mode change, status, disable, directory reload, or policy-store
mutation can become public-ready.

Non-default scriptlet publication summaries must include typed command
evidence plus both
`boot_security_intents` and `security_policy_intents`; rows that predate the
current conversion version are stale and must be reconverted before they can be
public-ready. The empty `{}` summary shape is reserved for native/default rows
without scriptlet evidence.

**legacy_scriptlets.rs** -- Current metadata for converted package scriptlet
semantics and local replay planning. The v1 bundle lives in the TOML manifest as
`[legacy_scriptlets]` and records source package identity, target
compatibility, per-entry decisions, effects, reserved trigger/purge metadata,
timeouts, and evidence digests. It is TOML-only in this revision; the CBOR
`BinaryManifest` remains unchanged and archive reads overlay the TOML field
when both manifest formats are present.

The large conversion surfaces are split by ownership. Adapter registry tests
live under `convert/adapters/tests/`; legacy bundle tests live under
`ccs/legacy_scriptlets/tests.rs`; Remi protocol, async-client, and client tests
live under `repository/remi/`.

**v2/** -- CCS v2 native package authority. Start in
`crates/conary-core/src/ccs/v2/` for v2 authority, validation, diagnostics,
debug projection, archive reading, and content identity. Use
`archive_reader.rs` and `package.rs` only as version-routing/adaptation
surfaces.

Native v2 authoring from `ccs.toml` starts in
`apps/conary/src/commands/ccs/{templates.rs,lint.rs,build.rs,test.rs,local_dev.rs}`
for command ergonomics and local-dev state, and
`crates/conary-core/src/ccs/v2/authoring.rs` for projection from `BuildResult`
into signed v2 authority. Debug TOML consistency checks live in
`crates/conary-core/src/ccs/v2/debug_projection.rs`; debug TOML is verified
against signed authority and never becomes install-time authority.

**enhancement/** -- Post-conversion enrichment via trait-based plugins.
Records exact conversion provenance. The retired capability and subpackage
enhancers guessed authority from package names and file paths; declared
capabilities and source package-manager metadata own those contracts instead.

**export/** -- OCI image export. Produces OCI-layout archives with gzipped
tar layers, image config, and manifest. ContainerConfig controls entrypoint,
cmd, env, ports, user.

## Architecture Context

CCS sits at the center of Conary's format pipeline. All package formats
(RPM, DEB, Arch) convert to CCS before installation. The builder produces
CAS-compatible content (SHA-256 keyed blobs), and the chunking system
enables delta-efficient distribution via the Remi server.

## Fixture Ownership

The first fixture ownership map for CCS conversion lives in
`docs/modules/test-fixtures.md`. Start there before changing golden conversion
cases, support-matrix fixture names, adapter-backed public-ready evidence, or
legacy scriptlet bundle fixtures. The fast proof for map-only or table-only
changes is:

```bash
cargo test -p conary-core golden_fixtures
cargo test -p conary-core support_matrix
```

If conversion output changes, also run:

```bash
cargo test -p conary --test conversion_integration golden_conversion
```

The golden conversion corpus includes a Fedora-to-Arch `adapter-selinux-policy`
case proving supported SELinux intent is fully replaced while unsupported
SELinux mutation remains covered by the `blocked-class-selinux` fixture.

## CCS v2 Native Authority

CCS v2 packages use the CBOR `MANIFEST` with `format_version = 2` as signed
install-time authority. `MANIFEST.toml` may be present for source/debug
visibility, but TOML-only install behavior is not native authority. The v2
implementation lives under `crates/conary-core/src/ccs/v2/`; legacy v1
`BinaryManifest` parsing remains a migration/fixture surface.

### Native CCS v2 Local Authoring Loop

The minimal native authoring loop is:

```text
conary ccs init --template minimal-file
conary ccs lint
conary ccs build --format v2 --local-dev
conary ccs verify package.ccs
conary ccs test package.ccs --dry-run
```

Config-only packages can be authored and contract-tested without a target
profile:

```text
conary ccs init --template config-noreplace
conary ccs lint
conary ccs build --format v2 --local-dev
conary ccs verify package.ccs
conary ccs test package.ccs --dry-run
```

Lifecycle-bearing packages require an explicit supported target profile during
lint, build, and dry-run test. Supported public profile IDs are `fedora-44`,
`ubuntu-26.04`, and `arch`; Remi route slugs such as `fedora` are not accepted
as CLI target profile IDs.

```text
conary ccs init --template service
conary ccs lint --target-profile fedora-44
conary ccs build --format v2 --local-dev --target-profile fedora-44
conary ccs verify package.ccs
conary ccs test package.ccs --dry-run --target-profile fedora-44
```

`--local-dev` signs with a user-local development key for iteration.
Local-dev artifacts can verify and dry-run-test locally, but static publish and
Remi release paths still require accepted release trust and build attestation.

`TargetProfileQuery` covers users, groups, directories, services, tmpfiles,
sysctl, and alternatives. Profile-backed validation accepts only explicit
per-entry policy and reports `LifecycleUnsupported` for unsupported signed
lifecycle authority. M4e's proof corpus covers the `config-noreplace` and
`service` templates, unsupported target-profile IDs, unsupported lifecycle
entries, debug TOML projection drift, and Remi lifecycle-bearing native release
upload.

## Legacy Scriptlet Bundles And Replay

Converted CCS packages may carry a `[legacy_scriptlets]` section. Local Conary
clients consume this bundle during install, update, remove, restore, batch, and
autoremove planning. Entries with `review`, `blocked`, or unknown decisions
refuse before mutation. Entries with `legacy` decisions are replayed only after
the bundle passes target, sandbox, lifecycle, timeout, and ordering preflight
and the operator explicitly provides `--allow-legacy-replay`.

Passive conversion bundle construction lives under
`crates/conary-core/src/ccs/convert/scriptlet_bundle.rs` and
`crates/conary-core/src/ccs/convert/scriptlet_bundle/`. The hub preserves the
public conversion API while child modules own public DTOs, entry decisions,
native ABI metadata projection, evidence digests, summaries, and fixtures.

Core replay planning lives in
`crates/conary-core/src/ccs/legacy_replay.rs`. The install-side adapter that
binds that planner to local install/update/remove replay execution and audit
metadata lives in `apps/conary/src/commands/install/legacy_replay.rs`.

Public-ready conversion is narrower than local replay acceptance. The supported
public source targets are `fedora-44`, `ubuntu-26.04`, and `arch`. A converted
artifact is public-ready only when the scriptlet outcome is native-free or
fully replaced by adapter/support-matrix evidence for the exact source and
target, such as validated `sysctl/v1` evidence projected into native
`hooks.sysctl` plus target-profile approval for the exact key, or validated
`setuid-mode/v1` evidence projected into payload file mode plus
`policy.allow_setuid_paths`. For the current public sysctl proof corpus,
`kernel.example` is allowed and `net.ipv4.ip_forward` stays private-review
until a supported profile explicitly allows it. Legacy replay, review-required,
blocked, malformed, or local-only scriptlet outcomes remain private conversion
results.

Common PAM stack helpers (`authselect`, `authconfig`, `pam-auth-update`, and
`pam-config`) remain `blocked-class-pam` evidence. They do not project native
manifest authority or public Remi eligibility without a future native PAM
policy adapter and target-profile PAM facts.

Remi consumes the typed command nodes emitted by conversion and never
reclassifies their strings. Its privacy normalization changes only displayed
argument values and clustering keys; it does not remove commands from the
signed conversion summary, change entry decisions, grant adapter coverage,
enable raw replay, or make an artifact public.

Live network fetches and nested package-manager calls remain blocked conversion
evidence. A scriptlet that fetches content with `curl`, `wget`, `scp`, `ssh`, or
`git clone`, or that invokes a nested package manager such as `dnf`, `apt`,
`dpkg`, `rpm`, `pacman`, `apk`, `microdnf`, or `zypper`, does not project native
manifest authority and cannot become public-ready without a future dependency
or offline-artifact authority model.

Foreign raw replay has a second gate. If the bundle source target differs from
the host target and the host is not listed in `allowed_targets`, the operation
also requires `--allow-foreign-legacy-replay` plus compatible bundle and host
mixing policy. `--no-scripts` is not a bypass for required raw replay: it
suppresses ordinary CCS hooks for replaced-only bundles, but refuses when the
selected lifecycle needs a raw legacy entry.

Converted CCS packages can carry metadata about legacy native scriptlets, but
CCS format does not make raw native scriptlets portable across distributions.
Raw replay of `family-compatible` legacy scriptlets is accepted only when an
explicit target compatibility matrix entry authorizes the source and host target
pair and the shallow compatibility preflight succeeds. The default production
matrix is empty, so Conary fails closed unless a later release ships or
configures validated compatibility evidence.

Accepted bundles are persisted with the installed trove so remove and upgrade
can replay or refuse safely even if the original `.ccs` archive is no longer in
the cache. Remi publication remains a separate gate; review, blocked, and raw
legacy replay requirements do not become public-serving approval merely because
the local client can consume the bundle.

Operators can inspect a local CCS package with:

```bash
conary query scripts ./nginx.ccs
conary query scripts ./nginx.ccs --verbose
conary query scripts ./nginx.ccs --entry rpm:%post
conary query scripts ./nginx.ccs --json
```

The CCS query output shows decisions, reasons, effects, body digests, and
reserved metadata summaries. It does not print preserved raw script bodies in
text or JSON output by default. Existing RPM/DEB/Arch package-file scriptlet
inspection keeps its current default behavior.

## Install

CCS packages are installed via `conary ccs install`. The installer verifies
signatures, evaluates capability policy, stores content in CAS, reuses the
shared composefs generation transaction, and runs declarative hooks.

```bash
conary ccs install package.ccs --yes         # Standard install
conary ccs install package.ccs --reinstall --yes # Reinstall same version (replaces files in CAS)
conary ccs install package.ccs --dry-run     # Preview without applying
```

The `--reinstall` flag forces reinstallation even when the same version is
already present. This is useful for repairing corrupted files or re-running
hooks without bumping the version.

Payload paths are normalized before capability checks, CAS storage, or
generation publication. Standard usr-merge roots and pre-existing symlink
ancestors such as Arch `/usr/lib64 -> lib` are resolved to the root-relative
deployment target only when every hop stays inside the selected install root.
An absolute symlink target is accepted only when it explicitly names a path
beneath that root. Escapes, loops, and children beneath symlinks created by the
package remain fail-closed; an existing leaf symlink also keeps the separate
replacement/collision semantics owned by the payload type.

For `[[file_capabilities]]`, the install boundary depends on the execution
path. Mutable live-root installs still apply the manifest authority with a
controlled `setcap` call after deployment. Generation-aware installs preserve
the same authority only through immediate generation publication: Conary
persists the installed file-capability rows, attaches `security.capability`
while building the runtime generation inputs, and reports the resulting xattr
count through `conary system generation info`.

Implementation routing: `apps/conary/src/commands/ccs/install.rs` is the
stable command hub. Command execution lives in
`apps/conary/src/commands/ccs/install/command.rs`; dependency/version policy
lives in `apps/conary/src/commands/ccs/install/dependency.rs`; component
selection lives in `apps/conary/src/commands/ccs/install/component_selection.rs`;
capability-policy enforcement lives in
`apps/conary/src/commands/ccs/install/capability_policy.rs`; and payload path
normalization remains in `apps/conary/src/commands/ccs/payload_paths.rs`.

CCS also exposes two package-scoped runtime helpers that are positively covered
in Phase 4:

```bash
conary ccs shell package-name          # Interactive environment with package contents
conary ccs run package-name -- cmd     # One-shot execution under that environment
conary ccs install package.ccs --components runtime,config --yes
```

Selective component installs persist only the requested components and skip
runtime hooks when a purely non-runtime slice is installed.

See also: [docs/specs/ccs-format-v1.md](/docs/specs/ccs-format-v1.md),
[docs/ARCHITECTURE.md](/docs/ARCHITECTURE.md).
