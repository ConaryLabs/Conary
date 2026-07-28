---
last_updated: 2026-07-28
revision: 49
summary: Convert foreign packages into source-independent CCS lifecycle transactions and export CCS as native packages
---

# CCS Module (conary-core/src/ccs/)

Conary's native package format. Handles building, signing, policy enforcement,
declarative hooks, foreign package conversion, native package export, and OCI
export.

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
     +-- Apply exact path/rule component assignment, else lossless `runtime`
     +-- Optional: split into CDC chunks (FastCDC)
     |
  Group files by component -> ComponentData
     |
  BuildResult { manifest, components, files, blobs, chunk_stats }
     |
  Sign manifest (Ed25519) -> embed PackageSignature
     |
  Output .ccs archive (tar.gz with MANIFEST + MANIFEST.toml + objects/)
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
| CCS package projection | package/v2_projection.rs | Project verified signed v2 authority into install-time package data |
| `AuthorityDocumentV2` | v2/schema.rs | Signed CCS v2 native package authority |
| `CcsStructuralBudget` | budget.rs | Single owner of every CCS structural and operator-resource limit; authoring preflight and verification both admit against it |
| `AuthorityCensus` | budget.rs | Exact structural measurement of one authority document; derives that package's byte ceilings |
| Component view | v2/component_view.rs | Derives component and file views from signed authority instead of a duplicated archive projection |
| `ComponentType` | components/types.rs | Closed typed names for standard component metadata; never inferred from payload paths |
| `SigningKeyPair` | signing.rs | Ed25519 key generation, signing, file I/O |
| `PackageSignature` | signing.rs | Embedded signature with algorithm, key_id, timestamp |
| `HookExecutor` | hooks/ | Runs declarative hooks with rollback tracking |
| `HostCapabilityInventory` | hooks/capabilities.rs | Persists source-independent host lifecycle interfaces and performs typed preflight |
| `NativeLifecycleBundle` | native_lifecycle.rs | Byte-preserved RPM/DEB/Arch lifecycle ABI and typed source metadata |
| `NativeTransactionPlan` | native_transaction.rs, native_transaction/{rpm,deb,arch}.rs | Exact lifecycle events derived from typed package changes and installed state |
| `NativeExport` | manifest.rs, native_export/ | Format-specific RPM, Debian, and Arch export overrides and generators |
| `BuildPolicy` (trait) | policy.rs | Pluggable build policy (DenyPaths, StripBinaries, FixShebangs, etc.) |
| `EnhancementEngine` (trait) | enhancement/ | Exact post-conversion provenance recording |

## Submodules

**manifest.rs and manifest_provenance.rs** -- `ccs::manifest` remains the root
manifest schema and validation owner. Declarative hook schemas, capability
validation, and hook reversibility live in `ccs::manifest::hooks` and are
re-exported through that root entrypoint. The provenance DTOs live in
`ccs::manifest_provenance` and are exported through the same root schema. M2
release publish stores hermetic evidence, signed
build-attestation envelopes, and foreign conversion boundaries in manifest
provenance. Artifact-form `conary publish <pkg.ccs> <target>` is allowed only
after `repository::static_repo::publish_gate` verifies package signatures,
TOML integrity, attestation authority, output identity, command-risk evidence,
and foreign-boundary hashes.

**hooks/** -- Declarative hook executors. Pre-install order: groups, users,
directories. Post-install order: live systemd daemon reload, explicit systemd
enable or disable, generic service actions, tmpfiles, sysctl, alternatives.
All operations respect a target_root parameter for bootstrap/container use.
`hooks/capabilities.rs` owns the typed host-inventory epoch. `conary
system init` discovers and persists the active init interface plus exact
`systemctl`, `systemd-sysusers`, `systemd-tmpfiles`, `sysctl`, and target-root
`ldconfig` executable interfaces in `system.host-capability-inventory`.
Each schema-v3 interface is bound to a command/root grammar, implementation
family and version, and executable digest established by a non-mutating
functional handshake. Install loads that document, repeats the handshake and
identity check, and preflights the complete hook set before dry-run output or
payload mutation. Missing, replaced, or stale required interfaces are typed
actionable errors, never silent skips.

Tmpfiles declarations retain all seven `tmpfiles.d` columns exactly: type,
path, mode, user, group, age, and argument. Type validation parses the
documented structural grammar (one ASCII letter followed by `+`, `!`, `-`,
`=`, `~`, `^`, or `$` modifiers) without maintaining a line-type allowlist.
The persisted typed `systemd-tmpfiles` interface owns support for particular
line types, modifier combinations, specifiers, and field semantics. Live-root
installs pass the exact rendered declaration to that executable; offline-root
installs write the same declaration under the target root for its own
`systemd-tmpfiles` implementation to consume.

Systemd enable and disable use the documented `systemctl` grammar on a live
host and `systemctl --root=...` for an offline root. A generic service start,
stop, or restart is valid only against a running typed service manager; it is
not guessed or deferred for an offline root. A `[[hooks.systemd]]` entry with
`enable = false` is an explicit disable operation. Author-declared systemd and
service fields are pathless unit names; raw native lifecycle argv has a
separate exact systemctl-operand contract and is not narrowed by this
declarative safety envelope.

SELinux/AppArmor conversion adapters record discovery evidence only. CCS has no
parallel `SecurityPolicyIntent` mutation contract. During selected-root
lifecycle execution, `crates/conary-core/src/scriptlet/activation_capture.rs`
observes actual provider argv and
`crates/conary-core/src/activation/security_policy/` applies the current
upstream helper grammars. Filesystem/policy-store work stays in the selected
root through documented no-live flags; kernel-active work becomes an exact
generation request bound to the captured provider path and SHA-256.

Hook types: Capability, User, Group, Directory, Systemd, Service, Tmpfiles,
Sysctl, Alternatives.

**components/types.rs and builder.rs** -- Component correctness is
author-owned metadata. Exact `[components.files]` paths take precedence over
exact `[components].rules` glob assignments; an unmatched payload remains in
one lossless `runtime` component. Conary does not infer libraries, development
files, documentation, or configuration ownership from path spelling.
Overlapping rules that select different component names are invalid. Foreign
packages without an explicit Conary component contract likewise remain one
lossless `runtime` component.

### Exact payload authority

`crates/conary-core/src/payload.rs` is the single file-node contract shared by
native parsers, CCS, installed database rows, selected-root transactions, and
generation artifacts. Every entry carries an explicit node variant, complete
POSIX mode, source owner and group identity, mtime, xattrs, and, only for a
regular file, an exact SHA-256 and byte length. Symlink targets, hardlink
identity, device numbers, FIFOs, sockets, and directories are variant data;
consumers must not recover node kind from mode bits, path spelling, content
length, or the presence of an unrelated field.

RPM header arrays own installed metadata while the paired CPIO member owns
bytes. RPM `FILEUSERNAME` and `FILEGROUPNAME` remain named source identities
and are resolved from the selected target root's exact passwd and group
records immediately before apply. Debian and Arch payloads retain the numeric
uid and gid declared by their accepted tar grammars. Installed rows preserve
both source identity and resolved numeric identity so later generation,
rollback, query, and verification paths do not repeat or guess resolution.
Unknown names, conflicting header/payload facts, unsupported archive records,
and missing content authority reject the package before filesystem mutation.
For an RPM symlink, `FILELINKTOS` owns the target and the CPIO member must carry
exactly those target bytes at exactly that length. After the streaming parser
proves that equality, projection retains the target as symlink variant data
without inventing regular-file content authority. Any mismatched target or
length still fails, and every other non-regular node must carry zero payload
bytes.

RPM hardlink projection follows the transaction rule pinned at upstream commit
`a8f0192aee1c08bd1454ed2ac6ebaf506004b55c`.
[`rpmfilesBuildNLink`](https://github.com/rpm-software-management/rpm/blob/a8f0192aee1c08bd1454ed2ac6ebaf506004b55c/lib/rpmfi.cc#L1380-L1434)
groups non-ghost regular files with a positive inode solely by the
`FILEDEVICES`/`FILEINODES` pair and retains each set in header-index order.
[`rpmfiArchiveHasContent`](https://github.com/rpm-software-management/rpm/blob/a8f0192aee1c08bd1454ed2ac6ebaf506004b55c/lib/rpmfi.cc#L2301-L2318)
designates the last packaged header-index member as the content-bearing member;
that designation also completes an all-zero-length set even though it carries
no data bytes.
[`fsmMkfile`](https://github.com/rpm-software-management/rpm/blob/a8f0192aee1c08bd1454ed2ac6ebaf506004b55c/lib/fsm.cc#L197-L237)
creates the other names with `link(2)` and applies inode metadata once, from
that completing member's header record. A partial set permitted by
[`rpmlib(PartialHardlinkSets)`](https://github.com/rpm-software-management/rpm/blob/a8f0192aee1c08bd1454ed2ac6ebaf506004b55c/lib/rpmds.cc#L995-L997)
therefore uses the last member actually packaged, while a declared link count
smaller than the packaged set remains malformed. CCS stores the resulting one
effective inode node on every path in the set; it does not preserve a parallel
source-RPM verification database. The adopted-package `--rpm` verification
path delegates to the installed RPM database, while native Conary verification
uses the projected node and CAS authority.

**convert/** -- RPM, Debian, and Arch to CCS conversion. Source package parsers
produce a typed native ABI before conversion: exact lifecycle slots, body
bytes, interpreters, invocation contracts, trigger/control metadata, and
package-manager ordering. Conversion persists every executable entry with a
digest; entry presence is lifecycle authority. It does not decide event order
by inspecting the script body and it does not suppress an entry because a
command appears to have a declarative replacement. The resulting CCS is
executed by Conary on any supported target; conversion and install do not
delegate lifecycle planning, database mutation, or transaction completion to
`rpm`, `dpkg`, or `pacman`.
Source format and target host are orthogonal: the running system exposes typed
ABI/libc/loader, init, LSM, filesystem, boot/kernel, and helper capabilities.
The implemented hook inventory currently records init/systemd, sysusers,
tmpfiles, sysctl, and ldconfig interfaces; the remaining capability families
stay typed implementation work rather than distro-name fallbacks. Distro names
do not select pairwise converters, compatibility profiles, or string gates.

`convert/converter.rs` is the conversion orchestration hub.
`convert/scriptlet_bundle/{builder,entries,format_metadata,native_contracts}.rs`
projects package-parser ABI entries into the durable bundle, and
`convert/scriptlet_bundle/{digest,summary}.rs` owns its content identity and
minimal fidelity summary. `convert/command_evidence.rs` and
`convert/converter/evidence.rs` may parse shell bodies for private diagnostic
evidence and engineering prioritization. That evidence is not persisted in the
lifecycle bundle and cannot affect publication, compatibility, mutation,
security, event selection, or event order. The retired adapter registry,
effect projection, classification, and policy-authority modules do not have a
runtime replacement.

The authoritative package-manager surface, invocation matrices, event order,
and payload visibility contract is
`docs/specs/foreign-package-lifecycle-contracts.md`.

**native_lifecycle.rs** -- Current persisted RPM, Debian, and Arch lifecycle
ABI. The schema-revision-19 bundle lives in the TOML manifest as
`[native_lifecycle]` and records source identity and version scheme, exact
entries with typed executable/control-artifact kind, body digests,
interpreters, native invocation contracts, RPM triggers, Debian
maintainer/trigger metadata including each exact trigger declaration line,
Arch install functions and ALPM hooks, and residual lifecycle metadata. All
persisted lifecycle structs reject unknown fields; there is no arbitrary
extension map. The bundle has no reason code, effect projection, unknown-command
evidence, diagnostic-class list/count, adapter-registry digest, publication
policy, or parallel security-policy intent. Its optional `source_profile` is
one exact public supported-profile ID; the former ambiguous field name and
family/route aliases are rejected.

The typed Debian declaration list is the sole trigger authority: validation
reparses the preserved control artifact and rejects parallel superseded
trigger-body or trigger-name projections. Actual provider execution at the
selected-root process boundary is systemd, SELinux, AppArmor, boot, and kernel
runtime authority; static script classifications are not persisted authority.
Revision 18 is a hard cut: earlier artifacts and installed rows must be
reconverted, rebuilt, or discarded with the pre-alpha database rather than
migrated. `native-free` means no lifecycle entries; `native-lifecycle` means
source behavior is preserved by the Conary runtime. Only a future complete
typed lowering contract may suppress a source program. Entry presence is the
exact lifecycle authority.
Successful conversion carries or declares the interpreter/helper runtime it
needs, uses a Conary-owned compatibility implementation, or has a complete
typed lowering. A source-manager-dependent entry is a missing implementation,
not a successful conversion.
The lifecycle contract carries no scriptlet publication policy or status.
Storage and serving validate the current typed bundle and artifact integrity;
diagnostics do not create a second admission state.

**native_transaction.rs and native_transaction/** -- Plan exact transaction
events from validated lifecycle bundles, typed install/upgrade/remove changes,
installed state, and lossless old/new payload sets. The RPM module owns the
separate installed-database-owner and transaction-owner trigger passes; the
`rpm/counts.rs` child owns database and script-argument instance counts at
their exact RPM state-machine boundaries; the
Arch module derives Install, Upgrade, and Remove hook candidates from added,
retained, and removed paths. The planner owns source-ABI argv, trigger stdin,
stage order, source transaction order, and intra-stage keys. It never invokes
the source package manager or reads script bodies or diagnostic command labels
to decide whether an event runs. Debian relation transactions carry exact
deconfiguration causes and identities, schedule deconfiguration before
conflict removal and incoming `preinst`, and persist documented reverse
`abort-remove`/`abort-deconfigure` recovery without mutating deconfigured
payload ownership.

The large conversion surfaces are split by ownership. Conversion projection
tests live under `convert/converter/tests.rs`; lifecycle schema tests live under
`ccs/native_lifecycle/tests.rs`; transaction-order tests live under
`ccs/native_transaction/tests.rs`; Remi protocol, async-client, and client
tests live under `repository/remi/`.

**v2/** -- CCS v2 native package authority. Start in
`crates/conary-core/src/ccs/v2/` for v2 authority, validation, diagnostics,
debug projection, archive reading, and content identity. Use
`archive_reader.rs` and `package.rs` only as version-routing/adaptation
surfaces.

Native v2 authoring from `ccs.toml` starts in
`apps/conary/src/commands/ccs/{templates.rs,lint.rs,build.rs,test.rs,local_dev.rs}`
for command ergonomics and local-dev state, and
`crates/conary-core/src/ccs/v2/authoring.rs` for projection from `BuildResult`
into signed v2 authority. `crates/conary-core/src/ccs/v2/lifecycle.rs` owns the
exact bidirectional projection between manifest lifecycle declarations, signed
v2 lifecycle authority, and the install-interface manifest projection.
`crates/conary-core/src/ccs/v2/validation/identity.rs` owns exact package
identity and typed provider validation; the validation hub owns orchestration.
`crates/conary-core/src/ccs/v2/validation/config.rs` owns exact per-path config
and currently consumable package-policy validation.
Debug TOML consistency checks live in
`crates/conary-core/src/ccs/v2/debug_projection.rs`; debug TOML is verified
against signed authority and never becomes install-time authority.

**enhancement/** -- Post-conversion enrichment via trait-based plugins.
Records exact conversion provenance. The retired capability and subpackage
enhancers guessed authority from package names and file paths; declared
capabilities and source package-manager metadata own those contracts instead.

**native_export/** -- CCS-to-native package generation for RPM, Debian, and
Arch. This is a current output surface, not a backward-compatibility layer.
`[native_export]` in `ccs.toml` holds format-specific metadata that has no
source-independent CCS equivalent. The exporters report every lossy
translation and do not provide a `[legacy]` schema alias. Required target
metadata comes from exact manifest fields: RPM requires the package license,
Arch requires homepage and maintainer-backed packager identity, and Debian
omits its optional maintainer field when no exact value exists. Exporters do
not invent unknown identities or unconditional maintainer scriptlets. Config
export fails when the target format cannot express a declaration's exact
per-path semantics.

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
cases, formal command evidence, source ABI parsing, or native lifecycle bundle
fixtures. The fast proof for map-only or table-only
changes is:

```bash
cargo test -p conary-core native_abi
cargo test -p conary-core native_lifecycle
cargo test -p conary-core native_transaction
```

If conversion output changes, also run:

```bash
cargo test -p conary --test conversion_integration golden_conversion
```

Golden conversion cases may assert diagnostic command/effect evidence, but
exact lifecycle proof comes from source ABI, event-order, argv/stdin, and
payload-visibility tests.

## CCS v2 Native Authority

CCS v2 packages use the CBOR `MANIFEST` with `format_version = 2` as signed
install-time authority. `MANIFEST.toml` may be present for source/debug
visibility, but TOML-only install behavior is not native authority. The
implementation lives under `crates/conary-core/src/ccs/v2/`. CCS v2 is the
sole native package contract: the repository has no v1 writer, parser,
projection, fixture factory, or compatibility path. `MANIFEST.toml` is a
checked projection only; malformed, unsupported, or unsigned authority never
falls back to it. The archive carries no `components/*.json` copy of the signed
file records: component views are derived from authority in
`v2/component_view.rs`.

### Structural Budget

`crates/conary-core/src/ccs/budget.rs` owns every CCS limit. There is no fixed
serialized-byte ceiling on `MANIFEST`: limits are structural counts, per-item
string and depth bounds, aggregate variable-length pools, payload-object
bounds, and CBOR nesting depth, and byte ceilings are derived from them. The
writer admits the authority it is about to sign through the same
`admit_authority` call verification uses, so a package this repository emits is
readable by construction. `docs/specs/ccs-format-v2.md` owns the contract.

Configuration authority is exact per path. Each `[[config.files]]` declaration
and its signed v2 projection carries `noreplace`, `ghost`, and
`remove_on_upgrade`; there is no package-wide config default. Ordinary config
paths must identify one regular-file or symlink payload and repeat the same
semantics in `FileAuthorityV2`. RPM ghost paths and Debian
remove-on-upgrade paths must be absent from payload and backed by the matching
signed native source contract. `CcsPackage` projects only that verified
authority into install, update, and remove transactions. Foreign conversion
copies the native parser's exact config declarations rather than reconstructing
them from paths.

Signed file conflict replacement and host-mutation policy are rejected while
their typed transaction consumers do not exist. The reader never accepts a
non-default signed field that installation would silently ignore.

### Native CCS v2 Local Authoring Loop

The minimal native authoring loop is:

```text
conary ccs init --template minimal-file
conary ccs lint
conary ccs build --local-dev
conary ccs verify package.ccs
conary ccs test package.ccs --dry-run
```

Config-only packages can be authored and contract-tested directly:

```text
conary ccs init --template config-noreplace
conary ccs lint
conary ccs build --local-dev
conary ccs verify package.ccs
conary ccs test package.ccs --dry-run
```

Declarative lifecycle is signed source-independent package intent. Authoring
does not name a destination distro: the install transaction resolves that
intent against the destination host's typed capabilities.

```text
conary ccs init --template service
conary ccs lint
conary ccs build --local-dev
conary ccs verify package.ccs
conary ccs test package.ccs --dry-run
```

`--local-dev` signs with a user-local development key for iteration.
Local-dev artifacts can verify and dry-run-test locally, but static publish and
Remi release paths still require accepted release trust and build attestation.

Structural validation covers the complete typed values for users, groups,
directories, generic services, systemd units, tmpfiles, sysctl, alternatives,
and executable post-install/pre-remove hooks without a distro allowlist.
Executable hooks sign their interpreter, body, capabilities, reversibility,
and sandboxed-target-root execution contract; install never guesses those
fields from script text or the destination distro. Destination capability
resolution belongs to transaction planning, not package authoring or Remi
publication. The proof corpus covers typed lifecycle round trips, install hook
projection, the `config-noreplace` and `service` templates, debug TOML
projection drift, and lifecycle-bearing native release upload.

## Source-Independent Lifecycle Bundles And Execution

Converted CCS packages and directly acquired RPM/DEB/Arch artifacts use the
same `[native_lifecycle]` bundle and Conary transaction engine. Local Conary
clients consume it during install, update, remove, restore, batch, and
autoremove planning. Flattened script text may exist only as explicitly
diagnostic evidence. A source parser that reports diagnostic script evidence
without typed ABI entries is a parser defect; there is no generic-hook fallback
and no source-package-manager runtime fallback.

Bundle construction lives under
`crates/conary-core/src/ccs/convert/scriptlet_bundle.rs` and
`crates/conary-core/src/ccs/convert/scriptlet_bundle/`. Child modules own entry
projection, source-format metadata, body/evidence digests, summaries, and test
fixtures. Every entry is byte-preserved and has the single current authority of
being present in the validated bundle; there is no parallel decision tag.

Debian lifecycle service-helper argv starts in
`crates/conary-core/src/packages/deb/lifecycle_helpers.rs` and its focused
children. That module pins `init-system-helpers` `1.69~deb13u1`, owns the exact
option/action/status/policy tables for all five helpers, and provides one typed
parse/render union for a later Conary-owned broker. It is grammar only: no
helper execution, target inspection, or schema authority lives there.

Core event planning lives in `crates/conary-core/src/ccs/native_transaction.rs`.
`apps/conary/src/commands/install/native_events.rs` binds that plan to the
shared install/remove executor. Command, batch, CCS, restore, remove, and
autoremove paths call the same stage groups instead of maintaining
format-specific copies.

Applied package transactions cannot suppress lifecycle execution. Install,
update, remove, restore, batch, autoremove, CCS install, automation, and daemon
package jobs have no `--no-scripts` or request-level equivalent; only a
non-mutating dry run stops before execution after reporting the typed plan.

An upgrade exposes new payload before post-install events while retaining
old-only paths through the source ABI's pre-removal boundary. The
transaction then removes old-only paths and old ownership before post-removal
events. Mutable-root and generation-aware installs use the same boundary; a
generation cannot be deferred past an event that needs to observe it.

Bundles are persisted with the installed trove so later remove, upgrade, and
restore transactions retain source lifecycle authority even when the original
archive is no longer cached. Unknown schema revisions, source slots,
interpreter modes, invocation shapes, or trigger semantics fail typed preflight
before mutation. During pre-alpha, each such result is a required implementation
defect with upstream-source citation and conformance coverage; it is not an
operator review state or a permanent supported-format boundary.

Cross-distribution execution is the point of the bundle, not an optional
promotion. A Debian package on Fedora, an RPM on Arch, and an Arch package on
Ubuntu preserve their source ABI without reading or mutating dpkg, RPM, or ALPM
state. Interpreters and helpers come from declared dependencies, a
Conary-owned compatibility runtime, or complete typed lowerings. Diagnostic
helper/effect records do not satisfy that requirement.

Private conversion diagnostics may retain privacy-normalized command evidence
for engineering prioritization. Normalization affects display only; it cannot
remove lifecycle entries, change event order, alter publication, or authorize
mutation.

Operators can inspect a local CCS package with:

```bash
conary query scripts ./nginx.ccs --policy ./ccs-trust.toml
conary query scripts ./nginx.ccs --verbose --policy ./ccs-trust.toml
conary query scripts ./nginx.ccs --entry rpm:%post --policy ./ccs-trust.toml
conary query scripts ./nginx.ccs --json --policy ./ccs-trust.toml
```

The CCS query output shows lifecycle entries, native slots, arguments,
transaction metadata, reserved source metadata, and body digests. It does not
print preserved raw program bodies in text or JSON output by default. Existing
RPM/DEB/Arch package-file lifecycle inspection keeps its current default
behavior.

## Install

CCS packages are installed via `conary ccs install`. The installer verifies
signatures, evaluates capability policy, stores content in CAS, reuses the
shared composefs generation transaction, and runs declarative hooks.

```bash
conary ccs install package.ccs --policy ./ccs-trust.toml --yes
conary ccs install package.ccs --policy ./ccs-trust.toml --reinstall --yes
conary ccs install package.ccs --policy ./ccs-trust.toml --dry-run
```

The `--reinstall` flag forces reinstallation even when the same version is
already present. This is useful for repairing corrupted files or re-running
hooks without bumping the version.

Payload paths are normalized before capability checks, CAS storage, or
generation publication. Usr-merge roots and pre-existing symlink ancestors
such as Arch `/usr/lib64 -> lib` are resolved to the root-relative deployment
target only from exact symlinks present in the selected install root and only
when every hop stays inside it. Conary does not infer `/bin`, `/sbin`, `/lib`,
or `/lib64` rewrites from path spelling. An absolute symlink target is accepted
only when it explicitly names a path beneath that root. Escapes, loops, and
children beneath symlinks created by the package remain fail-closed; an
existing leaf symlink also keeps the separate replacement/collision semantics
owned by the payload type.

Signed v2 authority carries the complete optional package capability
declaration used by install preflight, persistence, audit, and enforcement; the
authoring and install projections do not reconstruct it from debug TOML.
Package payload paths likewise do not imply lifecycle mutations. The current
schema has no built-in trigger classification or seeded path-glob triggers;
only an explicitly user-created trigger carries that separate operator
authority.

The source-independent node contract in
`crates/conary-core/src/payload.rs` is the sole authority for payload kind,
mode, numeric ownership, timestamp, xattrs, device identity, symlink target,
hardlink identity, and content reference. Native parsers, CCS v2, installed
state, and generation manifests use that same type; they do not maintain
parallel partial file projections.

A CCS archive is transport, not necessarily its source package ABI. A
Conary-authored CCS package applies its incoming metadata when it shares an
existing directory. A converted CCS package instead retains the exact
RPM, Debian, or Arch directory and configuration behavior selected by its
validated `native_lifecycle.source_format`; that source format must agree with
the package version scheme. The installed database stores every package's
exact directory claim even when dpkg or libalpm semantics preserve the
currently visible directory metadata. See
[`docs/specs/foreign-package-lifecycle-contracts.md`](../specs/foreign-package-lifecycle-contracts.md#shared-directory-ownership-and-materialization).

For `[[file_capabilities]]`, Conary persists the signed authority in the
selected-root transaction, attaches `security.capability` while building the
runtime generation inputs, and reports the resulting xattr count through
`conary system generation info`. There is no mutable live-root application
path.

Implementation routing: `apps/conary/src/commands/ccs/install.rs` is the
stable command hub. Command execution lives in
`apps/conary/src/commands/ccs/install/command.rs`; dependency/version policy
lives in `apps/conary/src/commands/ccs/install/dependency.rs`; component
selection lives in `apps/conary/src/commands/ccs/install/component_selection.rs`;
exact target capability validation lives in
`apps/conary/src/commands/ccs/install/capability_declaration.rs`; and payload path
normalization remains in `apps/conary/src/commands/ccs/payload_paths.rs`.

```bash
conary ccs install package.ccs --policy ./ccs-trust.toml --components runtime,config --yes
```

Selective component installs persist only the requested components and skip
unselected payloads. Declared CCS hooks are package-scoped and run for every
component selection; no script-disabling bypass exists, and component names
do not infer lifecycle ownership.

See also: [docs/specs/ccs-format-v2.md](/docs/specs/ccs-format-v2.md),
[docs/ARCHITECTURE.md](/docs/ARCHITECTURE.md).
