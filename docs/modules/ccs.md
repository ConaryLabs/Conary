---
last_updated: 2026-08-16
revision: 65
summary: Convert foreign packages through lossless source authority and typed native relation, lifecycle, and export contracts
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
Apply typed install prefix (default `/`) to source-root children
     |
  Walk source directory
     |
  For each file:
     +-- Compute SHA-256 hash
     +-- Apply PolicyChain (Keep / Replace / Skip / Reject)
     +-- Apply exact path/rule component assignment, else lossless `runtime`
     +-- By default, split files >= 16 KiB into canonical FastCDC v2020 chunks
     |
  Group files by component -> ComponentData
     |
  BuildResult { manifest, components, files, payloads, chunk_stats }
     |
  Project required whole-object or signed ordered-chunk layout
     |
  Sign MANIFEST authority (Ed25519)
     |
  Output .ccs archive (tar.gz with MANIFEST + MANIFEST.toml + objects/)
```

## Key Types

| Type | File | Purpose |
|------|------|---------|
| `CcsManifest` | manifest.rs | Root ccs.toml structure (package, provides, requires, hooks, policy, etc.) |
| `ManifestProvenance` | manifest_provenance.rs | Provenance DTOs embedded by the root manifest, including hermetic evidence, build attestations, and foreign conversion boundaries |
| `BuildAttestationEnvelope` | attestation.rs | Signed M2 release-publish attestation payload and verification helpers |
| `CcsBuilder` | builder.rs + builder/source.rs | Describes a CCS package from filesystem metadata and reopenable payload sources |
| `CcsInstallPrefix` | builder.rs | Validated absolute mapping for source-root children; the prefix and its ancestors are not package entries |
| `BuildResult` | builder.rs | Output: manifest, components, files, reopenable payload sources, total_size |
| `CcsPackage` | package.rs | Parsed .ccs file ready for installation via PackageFormat trait |
| CCS package projection | package/v3_projection.rs | Project verified signed v3 authority into install-time package data |
| `AuthorityDocumentV3` | v3/schema.rs | Signed CCS v3 native package authority |
| `FileContentLayoutV3` | v3/schema.rs + v3/validation/content_layout.rs | Required whole-object/no-content or canonical FastCDC v2020 reconstruction authority |
| `CcsTransportEnvelopeV1` | transport.rs | Versioned repository envelope carrying exact signed controls and their canonical ordered object set |
| v3 manifest identity projection | v3/manifest_projection.rs | Project one exact signed identity into install and untrusted-inspection compatibility manifests |
| `CcsStructuralBudget` | budget.rs | Single owner of every CCS structural and operator-resource limit; authoring preflight and verification both admit against it |
| `AuthorityCensus` | budget.rs | Exact structural measurement of one authority document; derives that package's byte ceilings |
| Verified object sink | verify/object_sink.rs + filesystem/cas/verified_batch.rs | Streams signed objects to a read-only spool or transaction-owned permanent CAS batch and returns typed committed object authority |
| `ReopenablePayload` | packages/payload.rs | Reopens one object or lazily concatenates authenticated ordered chunks without whole-file buffering |
| Component view | v3/component_view.rs | Derives component and file views from signed authority instead of a duplicated archive projection |
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

The `local-dev` signing authority is initialized under one process- and
thread-safe file lock. Its private key is the sole persisted authority; every
successful load atomically regenerates the public projection from that private
key before signing or constructing a trust policy. Concurrent first use cannot
replace an established signer, and a missing or mismatched public file never
becomes independent trust authority.

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
`systemctl`, `rc-service`, `systemd-sysusers`, `systemd-tmpfiles`, `sysctl`, and target-root
`ldconfig` executable interfaces in `system.host-capability-inventory`.
The version-5 document also records the target `/usr` node's exact opaque
`security.selinux` value when present. Generation artifacts authenticate that
target fact separately from each logical composefs node and use it only for
carrier CAS backing. `hooks/capabilities/filesystem_security.rs` owns its
discovery, validation, and application without a distro or label table. Each
executable interface remains bound to a command/root grammar, implementation
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

Systemd enable and disable use the documented `systemctl` grammar against the
selected root. OpenRC enable and disable own the exact `default` runlevel
symlink and require an executable, nonsymlinked init script. Generic service
start, stop, reload, or restart resolves through the typed active systemd or
OpenRC interface and is deferred as generation activation work. A
`[[hooks.systemd]]` entry with `enable = false` is an explicit disable
operation. Author-declared systemd and service fields are pathless names; raw
native lifecycle argv has a separate exact provider-operand contract and is
not narrowed by this declarative safety envelope.

Signed CCS lifecycle scripts share the native lifecycle boot-runtime capture
boundary. Mutation forms from the closed bootloader, initramfs, kernel, and
module-maintenance grammars become exact generation activation requests only
after a successful hook; information and status forms execute immediately and
remain non-persisted.

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

Native payload content is source-backed throughout authoring.
`builder/source.rs` owns filesystem enumeration and reopenable source
descriptors, `policy/content.rs` owns disk-backed streaming transforms, and
`builder/package_writer.rs` reopens those sources for signed CCS emission.
Authoring hashes and chunks fixed-buffer streams; it does not retain complete
files, chunks, or package payloads in memory.

### Exact payload authority

`crates/conary-core/src/payload.rs` is the single file-node contract shared by
native parsers, CCS, installed database rows, selected-root transactions, and
generation artifacts. Every entry carries an explicit node variant, complete
POSIX mode, source owner and group identity, mtime, xattrs, and, only for a
regular file, an exact SHA-256 and byte length. Symlink targets, hardlink
identity, device numbers, FIFOs, sockets, and directories are variant data;
consumers must not recover node kind from mode bits, path spelling, content
length, or the presence of an unrelated field.

`FileContentLayoutV3` is the signed storage contract layered beneath that
whole-file authority. Non-regular nodes explicitly carry `no-content`.
Unchunked regular files carry `whole-object`; default authoring for files at
least 16 KiB carries the canonical FastCDC v2020 16/64/256 KiB profile and an
ordered digest/length list. The writer re-chunks the reopened source and emits
each unique address once. Verification authenticates those objects, lazily
concatenates them, reruns the signed boundary profile, and proves the final
whole-file digest and size before exposing payload authority. Generation and
composefs materialization, not archive verification, owns the persistent
whole-file CAS object required to publish a root.

Repository transport preserves that same authority. `ccs/transport.rs` carries
the exact signed control bytes plus the canonical object identities and sizes
derived from them; authentication must reproduce the declared sequence before
any object is fetched. Remi publication, local/R2 storage, client acquisition,
delta accounting, federation, reference accounting, and GC all consume those
identities instead of rechunking the compressed `.ccs` carrier. A client reuses
trusted permanent-CAS hits, fetches only misses, verifies the complete set, and
reconstructs a deterministic temporary carrier for the existing install
boundary. Update uses the same resolver path, while rollback reuses generation
authority and never reacquires repository objects.

Remi durably publishes one converted transport with at most 16 object tasks in
flight. Each task checks R2, and a miss reads and re-verifies the canonical
local CAS object before its PUT. Remi waits for the complete bounded set and
publishes chunk-size/cache bookkeeping only after every required object is
durable; any HEAD, read, hash, or PUT failure rejects the conversion before
that bookkeeping boundary. The bound limits retained upload data to at most 16
canonical chunks while removing per-object network round-trip serialization.

RPM header arrays own installed metadata while the paired CPIO member owns
bytes. RPM `FILEUSERNAME` and `FILEGROUPNAME` remain source identities.
Matching pinned RPM
[`rpmugUid()` and `rpmugGid()`](https://github.com/rpm-software-management/rpm/blob/a8f0192aee1c08bd1454ed2ac6ebaf506004b55c/lib/rpmug.cc#L164-L220),
the distinguished RPM `root` user and group resolve directly to numeric zero
before any account database exists. Every other named RPM identity is resolved
exactly once from the selected target root's passwd or group records
immediately before apply; author-native CCS does not inherit RPM's root rule.
Debian and Arch payloads retain the numeric uid and gid declared by their
accepted tar grammars. Installed rows preserve both source identity and
resolved numeric identity so later generation, rollback, query, and
verification paths do not repeat or guess resolution.
Unknown names, conflicting header/payload facts, unsupported archive records,
and missing content authority reject the package before filesystem mutation.
For an RPM symlink, `FILELINKTOS` owns the target and the CPIO member must carry
exactly those target bytes at exactly that length. After the streaming parser
proves that equality, projection retains the target as symlink variant data
without inventing regular-file content authority. Any mismatched target or
length still fails, and every other non-regular node must carry zero payload
bytes.

RPM may encode the filesystem root itself as `DIRNAMES="/"` plus an empty
`BASENAMES` value. At the pinned upstream revision,
[`fsmFsPath`](https://github.com/rpm-software-management/rpm/blob/a8f0192aee1c08bd1454ed2ac6ebaf506004b55c/lib/fsm.cc#L71-L82)
handles that empty basename as `/`, while
[`rpmfnFindFN`](https://github.com/rpm-software-management/rpm/blob/a8f0192aee1c08bd1454ed2ac6ebaf506004b55c/lib/rpmfi.cc#L388-L435)
associates the corresponding standard CPIO entry after RPM's exact optional
`./` and `/` prefix removal. Conary preserves that entry through header/CPIO
association and completeness validation as a typed source ownership anchor.
It accepts only an unflagged directory with zero declared size, no
content-bearing metadata, and a zero-content CPIO member. The selected root is
the transaction container, not a deployable package path, so conversion
consumes the anchor without emitting a CCS payload node or install/remove claim
for `/`.

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

**convert/** -- RPM, Debian, Arch, and eopkg to CCS conversion. Source package parsers
produce a typed native ABI before conversion: exact lifecycle slots, body
bytes, interpreters, invocation contracts, trigger/control metadata, and
package-manager ordering. Source-defined strong requirement order, including
Debian `Pre-Depends` and RPM install prerequisites, is preserved as
`PreDepends` through repository metadata, signed CCS v3 authority, and
installed state. Conversion persists every executable entry with a digest;
entry presence is lifecycle authority. It does not decide event order by
inspecting the script body and it does not suppress an entry because a command
appears to have a declarative replacement. The resulting CCS is executed by
Conary on any supported target; conversion and install do not delegate
lifecycle planning, database mutation, or transaction completion to `rpm`,
`dpkg`, `pacman`, or `eopkg`.
Source format and target host are orthogonal: the running system exposes typed
ABI/libc/loader, init, LSM, filesystem, boot/kernel, and helper capabilities.
The implemented hook inventory currently records init/systemd/OpenRC, sysusers,
tmpfiles, sysctl, and ldconfig interfaces; the remaining capability families
stay typed implementation work rather than distro-name fallbacks. Distro names
do not select pairwise converters, compatibility profiles, or string gates.

The adopted-package entrypoint is
`apps/conary/src/commands/adopt/convert.rs`. It does not reconstruct a package
from live files or installed database metadata. It re-resolves the exact
authenticated RPM, Debian, Arch, or eopkg source artifact from an enrolled repository
or verified Conary cache, proves exact installed identity and payload
equivalence, and then calls the same direct native converter above. The signed
CCS is verified in same-directory staging before an installed-conversion row
and durable path commit together. Conversion failure, archive verification
failure, or SQLite failure removes the new output. Current installed
conversion records are reusable only while their conversion version, source
format/checksum, signed identity, architecture, version scheme, and lifecycle
source checksum remain exact.

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

**native_lifecycle.rs** -- Current persisted RPM, Debian, Arch, and eopkg lifecycle
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
selected-root process boundary is systemd, OpenRC, SELinux, AppArmor, boot, and kernel
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

**v3/** -- CCS v3 native package authority. Start in
`crates/conary-core/src/ccs/v3/` for v3 authority, validation, diagnostics,
debug projection, archive reading, and content identity. Use
`archive_reader.rs` and `package.rs` only as version-routing/adaptation
surfaces.

Native v3 authoring from `ccs.toml` starts in
`apps/conary/src/commands/ccs/{templates.rs,lint.rs,build.rs,test.rs,local_dev.rs}`
for command ergonomics and local-dev state,
`crates/conary-core/src/ccs/{builder.rs,builder/source.rs,policy/content.rs,builder/package_writer.rs}`
for source-backed collection, policy transforms, and archive emission, and
`crates/conary-core/src/ccs/v3/authoring.rs` for projection from `BuildResult`
into signed v3 authority. `crates/conary-core/src/ccs/v3/lifecycle.rs` owns the
exact bidirectional projection between manifest lifecycle declarations, signed
v3 lifecycle authority, and the install-interface manifest projection.
`crates/conary-core/src/ccs/v3/manifest_projection.rs` is the sole projection
of signed package identity into both install and untrusted-inspection
compatibility manifests, so authoring defaults cannot overwrite native
architecture or Debian `Multi-Arch` authority.
`crates/conary-core/src/ccs/v3/validation/identity.rs` owns exact package
identity and typed provider validation; the validation hub owns orchestration.
`crates/conary-core/src/ccs/v3/validation/config.rs` owns exact per-path config
and currently consumable package-policy validation.
Debug TOML consistency checks live in
`crates/conary-core/src/ccs/v3/debug_projection.rs`; debug TOML is verified
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
not invent unknown identities or unconditional maintainer scriptlets. RPM
requires and provides share one typed name/relation/version declaration;
unversioned relations cannot carry a version. Exporter/parser round trips keep
a source-declared same-name compatibility provide distinct from the package's
exact identity. Config
export fails when the target format cannot express a declaration's exact
per-path semantics. Shared hardlink preflight rejects missing or cyclic
targets, multiple or absent anchors, alias-side content, and member metadata
conflicts before any native artifact is published. Debian and Arch exporters
write explicit tar link records; RPM export uses an explicit package-builder
set identity and rejects ownership or timestamp authority that RPM cannot
encode. RPM root-child directories use the canonical `/` dirname with their
exact basename; default shared parent directories remain implicit when a
descendant already causes the native package manager to create them.
Exporter/parser round trips prove the canonical anchor, alias target,
shared content authority, and effective inode metadata for all three formats.

**export/** -- OCI image export. Produces OCI-layout archives with gzipped
tar layers, image config, and manifest. ContainerConfig controls entrypoint,
cmd, env, ports, user.

## Architecture Context

CCS sits at the center of Conary's format pipeline. All package formats
(RPM, DEB, Arch) convert to CCS before installation. The builder produces
CAS-compatible content (SHA-256 keyed blobs), and the chunking system
enables delta-efficient distribution via the Remi server.

### Source authority design target

Identity and declared provisions now use format-specific authority records and
the closed `SourcePackageAuthority` dispatch. Resolution, signing, persistence,
and publication consume provenance-tagged projections. Configuration records
carry their exact RPM, Debian, ALPM, or CCS declaration plus an explicit
matched/absent payload association. `ForeignConversionInput` is the named
conversion consumer, and the retired common metadata/config bridge is gone.
The canonical contract is
[`docs/specs/source-package-authority.md`](../specs/source-package-authority.md):
RPM, Debian, and ALPM parsers retain their native ontologies, and named
fallible projections serve dependency resolution, CCS authoring, and native
transaction planning. Exact identity is separate from declared capabilities,
and source config declarations are separate from materialized payload nodes.

Issues #104 and #105 complete the two W5 implementation halves;
[`docs/specs/ccs-format-v3.md`](../specs/ccs-format-v3.md) is the current
signed-format contract.

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

## CCS v3 Native Authority

CCS v3 packages use the CBOR `MANIFEST` with `format_version = 3` as signed
install-time authority. `MANIFEST.toml` may be present for source/debug
visibility, but TOML-only install behavior is not native authority. The
implementation lives under `crates/conary-core/src/ccs/v3/`. CCS v3 is the
sole native package contract: the repository has no v1 writer, parser,
projection, fixture factory, or compatibility path. `MANIFEST.toml` is a
checked projection only; malformed, unsupported, or unsigned authority never
falls back to it. The archive carries no `components/*.json` copy of the signed
file records: component views are derived from authority in
`v3/component_view.rs`.

Archive-envelope signer authority is established before that signed native
authority is projected into a transaction. A repository-acquired CCS uses its
exact persisted repository provenance and only active
`repository_package_keys`; a static repository may derive those keys from its
verified TUF targets metadata. Canonical Remi repository setup seeds a
release-tracked, exact-endpoint/exact-profile key set, while self-hosted Remi
requires an independently authenticated public-key file. A binary JSON
repository that serves already-built CCS packages enrolls its independently
authenticated public-key file through the same `--ccs-package-key` input; it
does not acquire authority from package metadata. An unknown,
cross-profile, malformed, or retired signer fails closed before payload or
lifecycle mutation. Local authoring and explicit file workflows retain their
separate exact-key or policy authority.

### Structural Budget

`crates/conary-core/src/ccs/budget.rs` owns every CCS limit. There is no fixed
serialized-byte ceiling on `MANIFEST`: limits are structural counts, per-item
string and depth bounds, aggregate variable-length pools, payload-object
bounds, ordered-reference bounds, and CBOR nesting depth, and byte ceilings are derived from them. The
writer admits the authority it is about to sign through the same
`admit_authority` call verification uses, so a package this repository emits is
readable by construction.

Source-package archive decoding derives its entry-count, cumulative payload,
metadata, decompressed-stream, and spool bounds from that same owner. One
payload object may consume the full 64 GiB total-payload allowance because
native authoring, conversion, FastCDC chunking, CAS ingestion, and package
writing all use fixed-buffer or reopenable streams rather than whole-object
buffers. Tar decoders enforce declared size per entry, then independently bound
cumulative payload, archive entries, metadata, and framing overhead; those
dimensions replace a guessed global decompression ceiling without weakening
decompression-bomb resistance. `docs/specs/ccs-format-v3.md` owns the contract.

Verification-only and dry-run callers use a temporary payload spool and do not
create permanent CAS state. Mutating CCS install, restore, and repository-batch
preparation instead stream the signed complete layout-object set into one
permanent SHA-256 `VerifiedObjectBatch`. Missing bytes are hashed while written once,
data-synced, published without replacement, and followed by shard/root
directory durability barriers. Exact-size canonical hits write no payload
bytes and are not reread solely for insertion; a concurrent publication winner
is reread and must match its signed size and digest. Only the committed batch
can create `ReopenablePayload` object sources carrying `VerifiedObjectSet`
authority. A whole-object source can transfer its canonical identity directly
to installer storage for the same CAS root; a chunk layout yields a lazy
concatenated source that installer storage ingests once into the whole-file CAS
representation generation publication requires. Ordinary sources retain the
existing bounded reader-ingestion path.

Configuration authority is exact per path. Each signed declaration retains
its source-specific record and a `matched` or `absent` payload association;
there is no package-wide config default. Matched declarations identify one
regular-file or symlink payload and repeat effective semantics in
`FileAuthorityV3`. RPM ghosts, Debian remove-on-upgrade declarations, and
unmatched ALPM backup declarations are absent for distinct source-owned
reasons. An unmatched ALPM declaration remains signed and persisted with
`materialized = false`; it never creates, removes, or backs up a user-created
path until a later version supplies a matching payload node. `CcsPackage`
projects only verified declaration authority into transactions. A manifest can
author an absent declaration directly with `payload = "absent"` and
`noreplace = true`; native export encodes that one declaration through each
format's own mechanism (`%ghost %config(noreplace)`, a
`remove-on-upgrade` conffile, or a backup entry without a payload member), and
no exporter invents payload bytes for it. Authoring an absent declaration with
`noreplace = false` is rejected before any export runs.

Signed file conflict replacement and host-mutation policy are rejected while
their typed transaction consumers do not exist. The reader never accepts a
non-default signed field that installation would silently ignore.

### Native CCS v3 Local Authoring Loop

The minimal native authoring loop is:

```text
conary ccs init --template minimal-file
conary ccs lint
conary ccs build --local-dev
conary ccs verify package.ccs
conary ccs test package.ccs --dry-run
```

`ccs init` writes `[package.platform]` with the explicit Conary `noarch`
architecture token. That token is signed architecture-independent authority,
not an omitted value for the target to infer. Authors of architecture-specific
payloads must replace it with the exact target architecture token before
building. Authoring lint and signed-v3 verification reject missing or blank
architecture authority before installation.

`ccs build --source <directory> --install-prefix /usr/bin` maps each child of
the source directory below `/usr/bin`. It does not author the source directory,
the target prefix, or any prefix ancestor as a package entry. The prefix is a
typed normalized absolute POSIX path; relative paths, empty components, `.`,
`..`, and trailing separators are rejected by the builder authority before
output is created. The default prefix is `/`.

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
format-specific copies. RPM's pre-payload sysusers event is executed through
the persisted target interface in
`crates/conary-core/src/scriptlet/sysusers.rs`; generic native commands remain
owned by `crates/conary-core/src/scriptlet/native_command.rs` inside the
selected root.

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
signatures, ingests authenticated missing objects directly into permanent CAS,
evaluates capability policy, reuses the shared composefs generation
transaction, and runs declarative hooks. Dry-run verification remains
filesystem-read-only with respect to permanent CAS.

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

Signed v3 authority carries the complete optional package capability
declaration used by install preflight, persistence, audit, and enforcement; the
authoring and install projections do not reconstruct it from debug TOML.
Package payload paths likewise do not imply lifecycle mutations. The current
schema has no built-in trigger classification or seeded path-glob triggers;
only an explicitly user-created trigger carries that separate operator
authority.

The source-independent node contract in
`crates/conary-core/src/payload.rs` is the sole authority for payload kind,
mode, numeric ownership, timestamp, xattrs, device identity, symlink target,
hardlink identity, and content reference. Native parsers, CCS v3, installed
state, and generation manifests use that same type; they do not maintain
parallel partial file projections.

A CCS archive is transport, not necessarily its source package ABI. A
Conary-authored CCS package applies its incoming metadata when it shares an
existing directory. A converted CCS package instead retains the exact
RPM, Debian, or Arch directory and configuration behavior selected by its
validated `native_lifecycle.source_format`; that source format must agree with
the package version scheme. The installed database stores every package's
exact payload claim even when dpkg or libalpm semantics preserve the
currently visible directory metadata. See
[`docs/specs/foreign-package-lifecycle-contracts.md`](../specs/foreign-package-lifecycle-contracts.md#shared-payload-ownership-and-materialization).

For `[[file_capabilities]]`, the v3 writer canonicalizes the declarations into
signed authority and verification proves each unique path names an exact
regular signed payload file. The verified install projection never consults
debug TOML. The selected-root transaction applies `security.capability` before
capturing the publication candidate and persists the same declaration for
database-derived generation and rollback authority. Generation metadata
reports the resulting xattr count through `conary system generation info`.
There is no mutable live-root application path.

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

See also: [docs/specs/ccs-format-v3.md](/docs/specs/ccs-format-v3.md),
[docs/specs/source-package-authority.md](/docs/specs/source-package-authority.md),
[docs/ARCHITECTURE.md](/docs/ARCHITECTURE.md).
