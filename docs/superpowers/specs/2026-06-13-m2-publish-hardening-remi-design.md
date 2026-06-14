# M2 Publish Hardening And Remi Push Design

**Date:** 2026-06-13
**Status:** Full release-surface design; M2a landed; M2b/M2c/M2d pre-plan
**Parent design:** `docs/superpowers/specs/2026-06-10-packaging-toolchain-design.md`

## Purpose

M2 turns the M1 packaging loop from a preview publishing path into a release
evidence path. M1a proved recipe-driven static publishing with honest
`sandboxed` provenance. M1b proved inference, `conary new`, `conary cook`, and
`conary try`. M2a has now landed the hermetic publish foundation. The remaining
M2 release surface defines the trust contract for signed build attestations,
artifact-form publish, foreign package ingestion, and Remi push.

The core invariant is:

> Artifact-form publish is allowed only for hermetic artifacts with verified,
> accepted build attestations.

`conary publish <pkg.ccs> <target>` must refuse any artifact that lacks a valid
signed build attestation. A `hermetic` artifact is useful evidence, but it is
not enough for artifact-form publish until the attestation, signer authority,
package signature, output identity, and lint gates all pass. The happy path
remains project-form publish: `conary publish <target>` performs the hermetic
rebuild, signs the attestation, verifies the result, and publishes in one flow
once M2b lands.

## Current Repo Facts

- `conary publish <pkg.ccs> <target>` is currently rejected in
  `apps/conary/src/commands/publish.rs` with an M2 attestation message.
- `conary cook --isolated` and the hidden compatibility `--hermetic` path now
  route through the M2a hermetic planner when local hermetic builder
  configuration is present.
- Project-form static publish now uses the hermetic Kitchen path with pristine
  sysroot-only build isolation, network disabled during the build, and unsigned
  M2a hermetic evidence. It still does not embed a signed build attestation.
- `apps/conary/src/commands/publish.rs` prints that M2a static publish records
  hermetic build evidence but release attestation gates arrive in M2b.
- `crates/conary-core/src/ccs/manifest.rs` already carries the orthogonal
  provenance fields `origin_class` and `hardening_level`, but the
  release surface still needs a separate build-attestation field; `attested` is
  a derived publish state, not a `hardening_level` value.
- M2a intentionally refuses unresolved build dependencies until dependency
  content locks exist. Ecosystem-specific lock/vendor/offline policy remains
  publish-gate work for the remaining M2 surface.
- Remi's TUF timestamp refresh is still a prerequisite for M2 Remi push; the
  parent packaging design calls out the current 501 stub in
  `apps/remi/src/server/handlers/tuf.rs`.
- Static artifact-form publish has no accepted-signer policy today. The M1
  project-form path signs with its own local key directory; M2b must add signer
  allowlist verification against the target publication context rather than
  trusting any self-signed artifact.
- `crates/conary-core/src/repository/static_repo/publish.rs` is already a large
  file that owns static repo layout, key rotation, package signing, TUF metadata,
  index writes, and concurrency checks. M2 gate logic should be extracted into
  small shared helpers rather than adding major eligibility policy directly to
  that module.
- Foreign package conversion is not greenfield. Existing ownership lives under
  `crates/conary-core/src/ccs/convert/`, `crates/conary-core/src/ccs/legacy/`,
  `apps/conary/src/commands/install/conversion.rs`, and
  `apps/remi/src/server/conversion/`. M2c wires that machinery into
  `conary cook <foreign-pkg>` and adds file-output plus attestation boundary
  metadata; it must not invent a parallel conversion engine.
- Runtime scriptlet sandboxing defaults to `SandboxMode::Always`, while
  `--sandbox=auto` only enters the protected sandbox when `analyze_script()`
  returns `Medium` or higher. The current analyzer names remote shell pipes,
  destructive filesystem operations, privilege edits, network backdoors,
  obfuscation patterns, package-manager fetches such as npm/Bun, and dynamic
  language execution. The remaining M2 work is shared taxonomy and report
  parity across runtime scriptlets, build commands, PKGBUILD bodies, and foreign
  conversion evidence.
- The PKGBUILD converter extracts `prepare`, `build`, `check`, and `package`
  bodies into recipe commands and warns on unsupported conversion features such
  as non-sha256 checksums, split packages, VCS packages, and dynamic `pkgver()`.
  It does not yet produce a build-body risk report for AUR-style package-manager
  fetches, obfuscation, credential-path probes, or `.install` metadata.
- Kitchen defaults to `use_isolation = true` and `allow_network = false`, but M2
  must still prove that build-time package-manager/network invocations are
  either satisfied from locked prefetched inputs or recorded as publish-blocking
  policy failures.

## Provenance Contract

M2 keeps provenance on two existing axes and adds build-attestation evidence:

- `origin_class`: `native-built`, `inferred-source`, `foreign-converted`, or
  `recorded-draft`.
- `hardening_level`: `host`, `sandboxed`, or `hermetic`.
- `build_attestation`: absent, present-but-unverified, or verified against a
  target-accepted signer.

The parent packaging design used `attested` as shorthand for "hermetic plus a
signed attestation." M2 refines that shorthand into an orthogonal attestation
gate so the execution truth remains visible: publishable artifacts are still
`hardening_level = "hermetic"`.

The artifact-form publish gate keys off all three concepts:

| Execution or attestation state | Artifact-form publish |
|--------------------------------|-----------------------|
| `hardening_level = "host"` | Refused. Host builds are iteration artifacts only. |
| `hardening_level = "sandboxed"` | Refused. M1 preview evidence is not release evidence. |
| `hardening_level = "hermetic"` with no build attestation | Refused. Hermetic evidence still needs a signed attestation. |
| `hardening_level = "hermetic"` with verified build attestation | Allowed only when signer authority, package signature, output identity, and publish lint also pass. |
| absent or unknown hardening/attestation value | Refused. Unknown provenance never falls through to a publishable state. |

| Origin state | Artifact-form publish |
|--------------|-----------------------|
| `native-built` or `inferred-source` | Allowed only through the hermetic-plus-attestation gate above. |
| `foreign-converted` | Allowed only through the hermetic-plus-attestation gate and only when conversion boundary metadata and risk reports are lint-clean. |
| `recorded-draft` | Always refused, regardless of hardening or attestation. |
| absent or unknown origin | Refused. |

This table covers artifact-form publish only:
`conary publish <pkg.ccs> <target>`. Project-form publish
(`conary publish <target>`) may start from the same project or source tree, but
it rebuilds, signs, verifies, and publishes a new hermetic artifact with a
verified build attestation.

M2b is the first point where signed build attestations may be embedded and used
for artifact-form publish. Before then, M2a may produce hermetic evidence
internally, but it must not unlock artifact-form publish.

## Architecture

M2 remains one release-surface design, executed as gated slices:

- M2b: signed build attestations and static artifact-form publish gates.
- M2c: foreign-package ingestion into attested CCS artifacts.
- M2d: Remi push with server-side gate parity.

The implementation plan may launch these under one `/goal`, but each slice must
retain its own reviewable boundary. M2 should add focused core concepts and keep
command modules thin.

### Hermetic Build Foundation

`crates/conary-core/src/recipe/hermetic/` owns:

- source prefetch
- source identity
- local tree hashing
- offline build setup
- dependency lock capture
- ecosystem dependency policy
- reproducibility controls
- host-vs-hermetic divergence diagnostics
- hermetic build diagnostics

This module feeds the existing Kitchen path. It must not become a parallel
builder. Fetching happens before the build environment starts; the hermetic
environment has no network access.

M2a also owns the reproducibility controls required before divergence diagnostics
are meaningful: set `SOURCE_DATE_EPOCH`, apply build-path mapping, record a
`host_build_record` for host iterations, and compare the hermetic output against
the last host build of the same input when such a record exists.

### Attestation

`crates/conary-core/src/ccs/attestation.rs` owns:

- `BuildInputIdentity`
- `DependencyLock`
- `BuildOutputIdentity`
- `BuildAttestationPayload`
- `BuildAttestationEnvelope`
- canonical attestation serialization
- signing and verification helpers
- attestation-to-manifest embedding/extraction helpers
- signer identity extraction

Attestation is a CCS/package property. Repository publication consumes artifacts
with build-attestation envelopes, but repositories do not create attestations.

Attestation verification has two layers plus the existing package-signature
layer:

1. **Attestation integrity:** the embedded signature verifies against the attestation
   content and the output identity matches the artifact.
2. **Attestation authority:** the signer key is accepted for the target
   publication context.
3. **Package authorization:** the CCS package signature verifies against the
   target package trust policy.

For static repositories, accepted signer policy comes from the destination
repo's active publisher/package keys after verified metadata is loaded, or from
the explicit local key directory during project-form or brand-new artifact-form
repo initialization. Artifact-form publish to a brand-new static repo is allowed
only with an explicit `--key-dir` whose active publish key verifies both the
package signature and the build attestation. There is no one-off
`--accept-artifact-signer` or equivalent bypass in M2. Retired static keys remain
historical verification material for already-published packages, but they cannot
authorize new artifact-form publish. For Remi, accepted signer policy is
enforced server-side from Remi's configured trusted publisher keys. The client
may preflight it, but the server is the authority.

Using the same active static publish/package key for static M2b v1 is a
deliberate simplification, not a long-term assertion that build and repository
release authority must always be identical. A later key-ceremony slice may add a
separate trusted-builder key list. M2b v1 does not add that ceremony; it keeps the
rule simple and auditable: the active static key is the only key that can attest
and authorize a new static artifact publish.

`AcceptedStaticSignerSet` is the precise static authority boundary. For an
existing repo, it is derived from verified, target-pinned
`keys/package-keys.json` active package-key entries after TUF metadata and
package-key consistency checks pass. Retired package-key entries, local key-dir
keys not authorized by verified destination metadata, stale active entries, and
package keys inconsistent with the current verified publishing authority fail
new publish authorization. For a brand-new repo, the explicit `--key-dir` active
publish key becomes the initial accepted static signer set only as part of repo
initialization.

Static publish therefore needs a two-phase API for M2b: prepare a verified
static publish context, then commit. The prepared context owns signer
resolution, active-key verification, package-key policy, and the key material
needed for project-form attestation signing. The commit phase owns immutable
package placement, package signing, index/TUF writes, and concurrency ordering.
This lets project-form publish sign the build attestation with the same active
key the final package signature will use without pushing key-resolution logic
back into the CLI.

`BuildAttestationEnvelope` is distinct from both existing signature layers. M2b
should embed it as a new structured field, e.g.
`ManifestProvenance.build_attestation: Option<BuildAttestationEnvelope>`,
alongside the existing `signatures: Vec<ProvenanceSignature>`. The existing
`ManifestProvenance.signatures` field is provenance-chain metadata; it is not the
package-distribution authorization gate. Package authorization is the archive
`MANIFEST.sig` / `ccs::verify::PackageSignature` path. Publish requires the
package-signature gate and the build-attestation gate.

Ownership boundaries:

| Responsibility | Owner |
|----------------|-------|
| Attestation schema, canonicalization, embedding, extraction, integrity verification, signer identity extraction | `crates/conary-core/src/ccs/attestation.rs` |
| Static target signer authority, package-key policy, and brand-new repo explicit `--key-dir` behavior | `crates/conary-core/src/repository/static_repo/` plus `apps/conary/src/commands/publish.rs` orchestration |
| Remi target signer authority and trusted build-attestation signer config/storage | `apps/remi/src/server/config.rs` and the M2d Remi push handler |
| Publish lint composition and artifact eligibility reason codes | shared core gate/lint helpers, orchestrated from `apps/conary/src/commands/publish.rs`, consumed by static publish, and rechecked by Remi |
| Command-risk evidence and classifications | `crates/conary-core/src/ccs/convert/command_evidence.rs` remains the canonical shell-command extractor; `crates/conary-core/src/recipe/hermetic/command_risk.rs` is the current build-time classifier seed; M2 must extract or share the rule vocabulary so publish lint and `container/analysis.rs` cannot drift into separate scanners |

### Publish Command

`apps/conary/src/commands/publish.rs` remains the user-facing command
orchestrator:

- Project-form publish resolves a project/source, prefetches inputs, runs the
  offline build, signs the attestation, verifies its own output, and publishes.
- Artifact-form publish verifies the embedded attestation, verifies signer
  authority for the target, runs publish lint, and publishes without rebuilding.

Static repository publication stays under
`crates/conary-core/src/repository/static_repo/`. That layer receives already
gated artifacts and keeps owning file layout, TUF metadata, index updates, and
publisher ordering. Because `static_repo/publish.rs` is already above the
large-file decomposition threshold, M2 should place static gate/admission logic
in focused helper modules such as `static_repo::publish_gate` or a shared core
publish-gate module, leaving the existing publisher as an ordering/layout owner.
M2b should create the gate module before adding artifact eligibility logic to
`publish.rs`, and the implementation plan should name the net line-count target
for keeping that file from growing beyond its current large-file warning state.

Likewise, `ccs::manifest.rs` and `recipe/kitchen/cook.rs` are near or over their
maintainability warning thresholds. Attestation schema, embedding/extraction,
and manifest mutation should live in `ccs::attestation` and
`ccs::manifest_provenance`; Kitchen should remain build orchestration and call
helpers rather than absorbing attestation logic.

### Remi Push

Remi push accepts the same CCS artifacts static artifact-form publish accepts.
Remi must not invent a separate provenance model or weaken static repo trust.
Before Remi push graduates, Remi must refresh TUF timestamp metadata rather than
leaving clients with stale or unimplemented timestamp behavior. Remi also
rechecks attestation integrity and signer authority on the server side; a
client-side preflight is helpful UX, not the trust boundary.

M2d must add a trusted build-attestation signer configuration or storage owner
for Remi. An empty trusted-signer set fails closed. The child plan must decide
whether the existing admin package route is hardened in place or whether Remi
push receives a distinct endpoint; either way, signer authority is server-side
state, not a client assertion.

Remi push has two independent trust layers:

- **Transport authentication:** the v1 Remi upload endpoint uses the parent
  design's static bearer token. Token-management UX remains deferred.
- **Artifact authorization:** the Remi server verifies attestation integrity and
  checks the build-attestation signer against configured trusted publisher keys,
  verifies the package signature/trust policy, checks output identity, and runs
  publish lint.

Both layers must pass. Bearer-token authentication does not make an untrusted
artifact acceptable, and a trusted artifact signer does not bypass upload
authentication.

Remi uploads must be staged outside public/package-index visibility. Only after
transport auth, attestation integrity, signer authority, package signature,
output identity, and publish lint all pass may Remi atomically commit the
package, index, and TUF state. Failed verification leaves no installable
artifact.

M2d release push should use a distinct release staging path or split the current
admin package upload route so release artifacts cannot reuse today's visibility
path accidentally. A failed release upload must leave no converted-package row,
no public package detail/index result, no public chunk-store object, and no TUF
target. For object stores such as R2, the order is: upload to a private staging
namespace, validate all gates, begin the database transaction, promote staged
objects to public content storage, update package/index/TUF rows, then commit.
If promotion fails, the transaction aborts and staged objects are cleaned up.

## Data Flow

Project-form publish:

1. Resolve project, recipe, or inference target.
2. Prefetch every source into the source cache.
3. Verify source identity by input kind.
4. Resolve Conary build dependencies against a named TUF snapshot.
5. Resolve or validate language-ecosystem dependencies according to the M2a
   per-ecosystem policy.
6. Scan recipe commands, converted PKGBUILD bodies, and converted legacy
   scriptlets for publish-relevant risk signals.
7. Record dependency locks and build/scriptlet policy reports.
8. Start an offline Kitchen build with network unavailable, reproducibility
   controls applied, build paths mapped, `allow_network = false`, and
   `source_download_policy = OfflineCacheOnly` asserted at the build execution
   boundary.
9. Capture output identity.
10. Compare against the last host build record when one exists.
11. Sign a build attestation with the target publisher Ed25519 key. Static
    project-form publish uses the active static publish/package key from the
    destination key directory in M2b v1.
12. Sign the target-local CCS package signature and verify it against the target
    package policy.
13. Embed and re-verify the attestation.
14. Verify that the attestation signer is accepted for the target publication
    context.
15. Run publish lint.
16. Recheck the final staged artifact bytes before metadata or index visibility.
17. Publish to a static repo or Remi target.

Artifact-form publish:

1. Read the existing `.ccs` artifact.
2. Extract and verify the embedded attestation.
3. Ensure the attestation's output identity matches the artifact.
4. Evaluate the incoming package-signature policy. A target may require the
   incoming package signature to verify before import; otherwise the incoming
   signature is not the final publication authorization.
5. Verify that the attestation signer is accepted for the target publication
   context.
6. Run publish lint using the shared reason-code vocabulary.
7. Attach or replace the target-local package signature in the CCS signature
   layer without rewriting canonical package content, manifest data covered by
   output identity, or the build-attestation payload.
8. Verify the final CCS package signature against the target package trust
   policy and recheck that output identity stayed unchanged.
9. Recheck the final staged artifact bytes before metadata or index visibility.
10. Publish to a static repo or Remi target without rebuilding.

Static artifact-form publish to a brand-new repo is allowed only when the caller
supplies an explicit `--key-dir`; the active publish key from that directory is
the accepted attestation signer and final package signer. An existing static
repo loads accepted active package keys from verified destination metadata and
local key-dir reconciliation. Retired package keys may verify historical
artifacts but do not authorize new artifact-form publish.

## AUR-Style Supply-Chain Threat Model

M2 treats the June 2026 Atomic Arch reports as a design input for M2a, M2b, and
M2c. Sonatype reported orphaned AUR package ownership changes where PKGBUILD or
install instructions invoked npm or Bun to fetch packages such as
`atomic-lockfile`, `js-digest`, and `lockfile-js`; analysis of the native payload
identified credential-harvesting, exfiltration, anti-debugging, and eBPF/stealth
indicators. Arch maintainers later reported that known malicious commits had
been removed while the public affected-package list was still not exhaustive.

The relevant Conary distinction is:

- Runtime scriptlets are an install-time containment problem.
- PKGBUILD and recipe command bodies are a build-time provenance problem.
- Already-built payload bytes are an artifact trust problem.

The default live-root scriptlet sandbox should blunt many runtime effects by
removing outbound network access, isolating filesystem mutations, and enforcing
the scriptlet syscall profile. That is containment, not publish evidence. M2
must not let a sandboxed install path imply that a package is safe to publish.

M2a and M2c therefore need a shared command-risk classifier for recipe commands,
converted PKGBUILD bodies, foreign-package scriptlet metadata, and legacy
scriptlet bundles. At minimum, these inputs must classify the following as
publish-relevant risk signals:

- external package-manager commands: `npm`, `npx`, `pnpm`, `yarn`, `bun`, `pip`,
  `gem`, `cargo install`, and `go install`
- network acquisition commands: `git clone`, `curl`, `wget`, `aria2c`, and
  `fetch`
- language one-liners that execute dynamic code: `node -e`, `python -c`,
  `perl -e`, and `ruby -e`
- credential-path references, shell obfuscation, base64 decode flows,
  persistence hooks, eBPF/BPF probes, proc-hiding indicators, and debugger
  attachment attempts

The shared classifier should reuse the current M2a building blocks rather than
creating an unrelated third scanner: `ccs::convert::command_evidence` already
extracts shell invocations, `recipe::hermetic::command_risk` already reports
build-command package-manager/network/dynamic-exec risks, and the conversion
blocked-class model already owns review/block reason codes for legacy
scriptlets. The remaining work is extracting a shared risk taxonomy/report DTO
that hermetic build commands, PKGBUILD body reports, runtime `--sandbox=auto`,
and foreign conversion scriptlets all consume. Runtime scriptlet auto-sandboxing
can then map the same classification vocabulary into `ScriptRisk`.

For hermetic project-form publish, a classified package-manager/network command
is allowed only when the command is satisfied from a lock/vendor/prefetch input
recorded in `BuildInputIdentity` and the actual build runs offline. Otherwise it
is a publish-blocking policy failure. For artifact-form publish, the artifact
must carry the previously generated build-command and scriptlet risk reports, and
publish lint must refuse absent, unknown, or unclean reports. For
`--sandbox=auto`, runtime scriptlet analysis should classify package-manager
fetches and dynamic language execution as at least `Medium` so they never run
directly on a live root through the auto path.

The scanner's static job is to require evidence, not to prove that a tool would
never try the network. Acceptable evidence includes ecosystem lock, vendor, or
cache identities in `BuildInputIdentity` and, where the tool supports it,
explicit offline/no-index/vendor mode in the generated command or config (for
example Cargo `--offline`, Go vendor mode, npm offline/cache configuration, or a
Python wheelhouse with no-index install policy). The offline Kitchen build is
the dynamic enforcement layer. If static evidence is missing or ambiguous,
hermetic publish fails before the build; if static evidence exists but the
command still attempts network access, the offline build fails and publish lint
records the attempted network access.

This is intentionally not a malware scanner. M2 attestation says which inputs,
commands, policy decisions, and output identity produced an artifact. It does not
declare arbitrary payload bytes benign.

External references for this threat model: Sonatype's
[Atomic Arch write-up](https://www.sonatype.com/blog/atomic-arch-npm-campaign-adds-malicious-dependency),
Arch's
[aur-general cleanup update](https://lists.archlinux.org/archives/list/aur-general%40lists.archlinux.org/message/FCH7TT6IOVT7D477JKSVJALBKADAARSW/),
and Phoronix's
[incident summary](https://www.phoronix.com/news/Arch-Linux-AUR-More-Than-1500).
These references were consulted on 2026-06-14 and may evolve; Conary fixtures
must use inert synthetic commands, stubbed tools, and disabled network rather
than contacting real package registries or incident artifacts.

## Command Behavior

### `conary cook --isolated <target>`

`--isolated` continues to mean "use the strongest available isolation for this
milestone." In M2a and later, the command may emit
`hardening_level = "hermetic"` only when all inputs are pinned and the build runs
offline. If a target cannot be made hermetic, the command refuses with
actionable diagnostics rather than falling back silently or overclaiming.

Plain `conary cook --isolated` may produce hermetic artifacts after M2a, but it
does not sign them and therefore does not make them artifact-form publishable.
Project-form publish remains the path that combines hermetic rebuild,
attestation signing, and publication.

The hidden compatibility `--hermetic` surface routes to the same M2a hermetic
behavior for compatibility. The public contract stays `--isolated`; the
provenance field tells the truth.

### `conary publish <target>`

Project-form publish is the release happy path. It performs the hermetic rebuild
and signing automatically. After M2b, a successful project-form publish emits a
hermetic artifact with a verified build-attestation envelope and publishes it.
Project-form publish must use release-grade dirty-tree enforcement regardless of
the caller's ambient `CI` environment; local dirty git worktrees are refused
instead of becoming release artifacts with weaker local diagnostics.

### `conary publish <pkg.ccs> <target>`

Artifact-form publish unlocks only after M2b. It verifies an existing artifact
and refuses unless:

- the embedded build attestation verifies
- the attestation signer key is accepted for the target publication context
- the final CCS package signature verifies against the target package trust policy
- the attestation output identity matches the package contents
- `origin_class` and `hardening_level` are recognized and publishable
- publish lint passes

No rebuild happens in artifact-form publish. M2b may attach or replace the
target-local CCS package signature after the artifact's attestation, output
identity, incoming-signature policy, and publish lint checks pass. That signature
operation is limited to the package signature layer; it must not rewrite payload
files, manifest data covered by `BuildOutputIdentity`, the attestation payload,
or the attestation envelope. The final staged artifact must verify under the
target package trust policy and must still match the attested output identity
before any metadata or index entry becomes visible.

### `conary cook <foreign-pkg>`

M2c adds `.rpm`, `.deb`, and `.pkg.tar.zst` routing through `conary cook` by
reusing the existing `ccs/convert` and `ccs/legacy` conversion machinery.
Foreign package conversion stamps `origin_class = "foreign-converted"`. The
attestation must describe the conversion boundary: upstream binary identity,
conversion tool/version, extracted metadata, existing legacy provenance and
scriptlet bundle/classification output, build-command/scriptlet risk reports, and
output identity.

Foreign-converted artifacts must not be laundered into `native-built`. They are
publishable only when hermetic, backed by a verified accepted build attestation,
and lint-clean.

The existing conversion stack is currently oriented around install-time CAS
insertion and Remi server conversion. M2c adds a client-side file-output path
that produces a `.ccs` artifact for `conary cook <foreign-pkg>` while reusing
the same extraction, analysis, safety, manifest-generation, legacy provenance,
and scriptlet bundle logic.

### Remi Targets

M2d adds authenticated Remi upload for the same CCS artifact static
artifact-form publish accepts. If an artifact would fail static publish gates,
Remi push refuses too. Remi stages the upload outside public/package-index
visibility, rechecks attestation integrity, configured signer authority, package
signature policy, output identity, foreign conversion boundaries, and publish
lint server-side, then atomically commits metadata, chunk/index visibility, and
TUF timestamp state. Transport authentication is necessary but never sufficient
for artifact authorization.

M2d must also split the current local-static destination guard from Remi target
routing. The existing M1a filesystem-only static publish refusal remains valid
for local static repo writes, but authenticated Remi targets need a distinct path
that allows HTTP(S) only after transport authentication and artifact
authorization are both configured.

## Data Model

### `BuildInputIdentity`

Records what was built:

- recipe hash
- source target kind
- archive checksums
- git commit identities
- local tree hash
- patch identities
- inferred-source trace hash when inference is used
- foreign binary identity when conversion is used
- ecosystem dependency identity when a build-system-specific lock or vendor tree
  is part of the input

Local source hashing follows the parent design: inside a git repository, hash
exactly tracked files; ignored files, `.git/`, `dist/`, `target/`, and generated
outputs are excluded. Outside git, hash all files minus a documented default
ignore set and warn that identity is weaker. CI refuses dirty local trees; the
remaining M2 release surface must preserve the M2a CI-mode detection contract
and keep dirty-tree behavior fail-closed for publish.

Hermetic local-source materialization must use the same canonical file list that
was hashed. M2a must not let ignored or untracked files influence a build while
being absent from `BuildInputIdentity`. The remaining release-surface plan must
either preserve M2a's hashed-file-list materialization guarantee or refuse when
excluded files could affect the build.

### `DependencyLock`

Records the Conary repository dependency universe:

- repository URL
- TUF snapshot version
- resolved package name, version, release, and architecture
- immutable package content identity: package hash or TUF target hash

Re-resolution uses the snapshot version as provenance and re-fetches the exact
recorded package or target hashes from append-only package storage. A moving
latest view is not a hermetic dependency lock. If an immutable content identity
is unavailable for any build dependency, the artifact cannot claim `hermetic` and
publish lint must fail.

The remaining M2 release surface must extend M2a's fail-closed dependency
posture into a language-ecosystem dependency policy. At minimum, each supported
inferred ecosystem needs one of these outcomes:

- accepted offline mode with a lock/vendor input recorded in
  `BuildInputIdentity`
- clear refusal with a diagnostic naming the missing lock/vendor input
- explicit deferral from hermetic publish support for that ecosystem

Cargo, Go, npm, and Python are the first ecosystems to classify because Conary's
source inference already warns that they may resolve dependencies over the
network.

Provisional remaining-M2 defaults:

| Ecosystem | Hermetic publish default |
|-----------|----------------------|
| Cargo | Accept only with `Cargo.lock` plus a vendored or cached crate source set that is part of `BuildInputIdentity`; otherwise refuse. |
| Go | Accept only with `go.sum` plus `vendor/` or a pinned module cache recorded in `BuildInputIdentity`; otherwise refuse. |
| npm | Accept only with a lockfile (`package-lock.json` or equivalent) plus vendored `node_modules` or a pinned npm cache recorded in `BuildInputIdentity`; otherwise refuse. |
| Python | Defer hermetic support unless the M2a plan chooses a lockfile/wheelhouse strategy; refuse publish-hermetic for ambiguous pip/network resolution. |

The remaining implementation plan should implement the Cargo path first if it
extends ecosystem support, because Rust workspace packaging is the closest
dogfood path for Conary itself. The plan should prefer explicit offline
invocation or config over relying only on the network namespace. Other
ecosystems may begin as fail-closed classifications with diagnostics if their
offline strategy is not ready.

When Go or npm move from fail-closed to accepted, the implementation plan must
name concrete static checks. Examples: Go builds must use a recorded `vendor/`
tree with `-mod=vendor` or an explicitly pinned local module cache/proxy, and
npm builds must use a lockfile plus explicit offline/cache mode such as
`npm ci --offline`. A command that merely has a lockfile nearby but can still
resolve from the network is not sufficient publish evidence.

### `BuildOutputIdentity`

Records what was produced:

- file-manifest Merkle root
- package name, version, release, and architecture
- origin class
- hardening level
- hermetic evidence hash
- canonical CCS content identity, excluding package signatures and the
  attestation envelope

The attestation signs both input identity and output identity. Output identity is
computed over canonical package content excluding signature and attestation
blocks, so the signature is not self-referential. The M2b plan must align this
with the existing CCS manifest layering, where package signatures are added
after manifest/content identity is computed.

The existing `dna_hash` is not a build output identity. It identifies a
provenance chain that includes source/build/dependency data and, in the newer
provenance module, signatures. M2b must add or compute a new explicit
content-only identity for `BuildOutputIdentity`; it must not use `dna_hash` as a
shortcut. The content identity must remain stable when package signatures or
attestation envelopes are added, removed, or replaced.

### `BuildAttestationPayload`

Versioned signed payload schema:

- schema version
- origin class
- hardening level
- build input identity
- dependency lock
- hermetic evidence hash
- build output identity
- build-command risk report hash
- scriptlet risk report hash when install hooks or converted legacy metadata are
  present
- conversion-boundary hash when `origin_class = "foreign-converted"`
- publish policy/ruleset digest
- command-risk classifier version
- sandbox and seccomp profile identity
- builder identity and Conary tool version
- issued-at signing timestamp

For artifact-form publish, the payload's `hardening_level` must be `hermetic`.
The policy/ruleset digest and classifier version let verifiers reject old or
unknown attestations after policy changes. Targets may define a minimum accepted
policy version or an explicit compatibility window, but the default is
fail-closed for stale or unknown policy identities.

### `BuildAttestationEnvelope`

Versioned envelope schema:

- payload
- build-attestation signer key ID
- Ed25519 signature over the canonical serialized payload bytes

The signature is made by a publisher Ed25519 key. In project-form static publish
this is the destination repo's active publisher/package key from the local key
directory. In artifact-form publish, the build-attestation signer and the target
publication authority are distinct concepts: the artifact carries who built it,
while the destination target decides whether that signer is accepted. Target
package authorization is represented by the CCS package signature layer, which
artifact-form publish may re-sign without changing the attested content
identity. M2 does not introduce a separate machine key ceremony.

The signature field is not part of the signed payload. The output identity is
also computed over canonical package content excluding package signatures and
attestation envelopes, so neither signature layer is self-referential.

### Attestation-Carrying Manifest Projection

M2b must define the exact canonical manifest projection that carries
attestation-relevant provenance. Today package signatures verify the raw CBOR
manifest bytes, while TOML provenance is protected through the binary
manifest's TOML integrity hash. M2b has two acceptable implementation choices:

- promote attestation-bearing provenance into a binary-manifest v2 projection
  that is directly covered by the package signature; or
- keep the attestation in `ManifestProvenance` and require artifact-form publish
  to verify the CBOR package signature, the binary manifest's TOML integrity
  hash, and the embedded build-attestation envelope before any lint or signer
  authority decision.

The implementation plan must pick one. Either way, tampering with
`MANIFEST.toml` attestation/provenance while leaving CBOR and `MANIFEST.sig`
unchanged must be detected before publish, and re-signing the package must not
change `BuildOutputIdentity`.

### `ForeignConversionBoundary`

Foreign-converted artifacts need a concrete boundary DTO rather than prose-only
evidence. M2c should add `ForeignConversionBoundary` under `ccs::attestation` or
`ccs::convert` and include at least:

- source package format, distro/release when known, architecture, checksum, and
  package filename or source identity
- converter name/version and conversion policy version
- extracted legacy provenance digest
- `LegacyScriptletBundle` evidence digest and sanitized summary digest
- build-command and scriptlet risk report digests
- conversion fidelity/publication status
- output identity

The build-attestation payload signs the boundary hash when
`origin_class = "foreign-converted"`. Missing, malformed, or mismatched boundary
data fails static artifact-form publish and Remi release push.

### `PublishLintReport`

Publish lint produces a machine-testable result and human-readable diagnostics.
It must identify the exact failed gate:

- missing attestation
- build-attestation signature mismatch
- package signature mismatch or untrusted package signer
- output identity mismatch
- unaccepted signer key
- retired signer key used for new publish authorization
- absent or unknown provenance class
- non-hermetic hardening level
- network access attempted during hermetic build
- unpinned source
- dirty local tree in CI
- missing dependency snapshot
- missing dependency content identity
- missing ecosystem lock/vendor identity
- missing explicit offline command or config evidence required by an ecosystem
  policy
- stale or unknown publish policy/ruleset identity
- unclassified package-manager or network command in a hermetic build path
- package-manager or network command without matching prefetch/lock evidence
- credential-path, persistence, obfuscation, eBPF/BPF, proc-hiding, or
  anti-debugging indicators that require review or refusal
- absent, unknown, or unclean build-command/scriptlet risk report
- foreign conversion missing boundary metadata
- foreign conversion boundary metadata hash mismatch
- Remi transport authenticated but artifact authorization failed
- recorded-draft artifact
- non-reversible or disallowed install hooks

## Error Handling

M2 errors fail closed and name the corrective action. Once M2 work starts
landing, user-facing errors should avoid generic unsupported-feature text for
paths that have partial M2 support. Examples:

- "artifact is sandboxed, not hermetic with a verified build attestation; run
  project-form publish to rebuild and sign"
- "source archive is missing a checksum; add one to the recipe before publish"
- "dependency lock entry lacks a package hash or TUF target hash"
- "npm dependencies are not vendored or locked for offline build"
- "cargo publish build is missing explicit offline mode; add `--offline` or an
  equivalent hermetic config before publish"
- "PKGBUILD build body invokes npm without a recorded lock/vendor/prefetch input"
- "scriptlet invokes bun in auto sandbox mode; protected sandbox required"
- "build attestation was made under an unknown publish policy version"
- "package signature does not verify against the target package trust policy"
- "artifact was signed by a retired package key; rebuild or re-sign with the
  active publish key before publishing"
- "Remi upload authenticated, but artifact authorization failed: unaccepted
  build-attestation signer"
- "cargo build requires network access; vendor dependencies or provide a
  lock-compatible offline source before publish"
- "local tree is dirty; commit changes or rerun outside CI with dirty-tree
  recording"
- "dependency lock is missing TUF snapshot version; sync and rebuild"
- "foreign conversion is missing scriptlet policy metadata"

## Implementation Slices

### M2a: Hermetic Publish Foundation

Landed before this full release-surface lock-in. It implements source prefetch,
source identity, local tree hashing, offline Kitchen execution, initial
fail-closed dependency policy, hashed-file-list local-source materialization,
build-command/scriptlet risk classification, reproducibility controls,
divergence diagnostics, and `hardening_level = "hermetic"`.

M2a does not enable artifact-form publish. The project-form publish pipeline can
produce hermetic evidence without overclaiming attestation. Before M2b, release
publishability does not graduate; project-form publish records M2a evidence but
does not emit a verified build-attestation envelope.

### M2b: Attestation And Publish Gates

Adds signed build attestations, verification helpers, publish lint, and the
artifact-form publish gate. This is the first slice that may embed
`BuildAttestationEnvelope` and unlock `conary publish <pkg.ccs> <target>` for
artifacts with `hardening_level = "hermetic"`.

Project-form publish performs hermetic rebuild plus signing automatically.
Artifact-form publish refuses artifacts whose build-command or scriptlet risk
reports are absent, unknown, or unclean.

M2b must first choose the attestation-carrying manifest projection, add the
static prepared-publish context, and extract publish-gate/lint helpers before
placing eligibility logic in the existing static publisher. It must also force
project-form publish through release-grade dirty-tree refusal rather than
ambient CI detection.

M2b must also define the explicit accepted-signer UX for artifact-form publish
to a brand-new static repo as explicit `--key-dir` use. The active key in that
directory is the accepted build-attestation signer and final package signer. M2b
does not add a one-off accepted-signer flag.

### M2c: Foreign Package Ingestion

Adds `.rpm`, `.deb`, and `.pkg.tar.zst` routing through `conary cook` by reusing
the existing conversion modules. Conversion outputs use
`origin_class = "foreign-converted"` and must carry boundary metadata before
they are publishable. A foreign-converted artifact must also carry a scriptlet
and build-body risk report when the source package format can express those
hooks or command bodies.

M2c owns the `ForeignConversionBoundary` contract and the tests proving existing
legacy provenance, scriptlet bundle evidence, conversion fidelity, and command
risk reports flow into the attestation boundary rather than living only in
Remi/database-local state.

### M2d: Remi Push

Adds authenticated Remi upload for CCS artifacts that pass the static
artifact-form gate and completes the Remi TUF timestamp refresh prerequisite.
Remi push uses the same artifact gate as static artifact-form publish, plus the
parent design's bearer-token upload authentication. M2d must add Remi trusted
build-attestation signer configuration or storage, define empty-list behavior as
fail-closed, and prove uploads stage privately until all artifact gates pass.
Remi rechecks the shared gate server-side; client preflight is UX only.
M2d also splits Remi release push from the current admin-upload visibility path
or hardens that route so release failures leave no package row, no public chunk,
no package detail/index response, and no TUF target.

## Non-Goals

- Record mode.
- Watch mode.
- MCP packaging tools.
- Ecosystem refs such as `crate:` or `pypi:`.
- Delta-only publishing.
- Broader TUF delegation work.
- Replacing the M1 static repository format.
- Making Remi a distinct package trust model.
- Declaring arbitrary payload bytes malware-free.

## Verification Expectations

Each child implementation plan must name focused checks. The umbrella-level
expectations are:

- Unit tests for source identity, local tree hashing, dependency lock
  serialization, dependency content identity, language-ecosystem offline policy,
  hashed-file-list materialization, reproducibility controls, attestation
  payload/envelope signing and verification, accepted-signer policy, and publish
  lint.
- Unit tests proving the chosen attestation-carrying manifest projection detects
  tampered TOML attestation/provenance even when CBOR and `MANIFEST.sig` are
  unchanged.
- CLI tests proving project-form publish produces hermetic artifacts with
  verified build-attestation envelopes after M2b.
- CLI tests proving project-form publish refuses dirty git worktrees regardless
  of ambient `CI` environment.
- CLI tests proving artifact-form publish rejects `host`, `sandboxed`,
  `hermetic` without attestation, missing-attestation, mismatched-attestation,
  unaccepted-signer, package-signature failure, absent/unknown provenance,
  stale/unknown policy version, recorded-draft, and unclean foreign-converted
  artifacts.
- Static publish tests proving `AcceptedStaticSignerSet` is derived from
  verified active package keys, rejects retired keys, rejects local key-dir
  mismatches, rejects tampered package-key metadata, and keeps project-form
  attestation and final package signature on the same active key through key
  rotation.
- Regression tests proving `--isolated` and any hidden `--hermetic`
  compatibility path never overclaim.
- Regression tests proving attestation output identity excludes
  signature/attestation bytes and never uses `dna_hash` as the output identity.
- Regression tests proving `hardening_level` remains `hermetic` for publishable
  artifacts and that attestation is represented by the separate envelope.
- Regression tests proving ignored or untracked local files cannot affect a
  hermetic build unless they are included in the hashed input identity.
- Tests proving host-vs-hermetic divergence diagnostics are stable enough to be
  useful and not a nondeterminism firehose.
- AUR-style regression fixtures using inert synthetic package names and stubbed
  `npm`/`bun` tools, for example a `.INSTALL` scriptlet with a synthetic package
  install command, a Bun variant, and a simulated `bpf()` attempt. Expected
  results: default `always` sandbox contains network/syscall behavior, `auto`
  classifies the scriptlet as `Medium` or higher, and `--sandbox=never` remains
  an explicit live-root escape hatch rather than a publishable trust signal.
- PKGBUILD/foreign-conversion fixtures with the same npm/Bun package-manager
  commands in `prepare`, `build`, `check`, or `package` bodies. Expected
  results: the M2 command-risk scanner reports the command, missing static
  lock/vendor/prefetch or explicit offline-mode evidence fails before build,
  Kitchen default isolation denies unprefetched network access when static
  evidence exists but the tool still attempts a fetch, and any explicit network
  override is recorded and prevents `hermetic` claims or verified-attestation
  publishability.
- Shared-risk tests proving runtime `--sandbox=auto`, hermetic build-command
  reports, PKGBUILD body reports, and foreign conversion scriptlet reports agree
  on npm/Bun/network/dynamic-exec reason codes.
- Foreign-conversion tests proving missing or mismatched
  `ForeignConversionBoundary` data fails static artifact-form publish and Remi
  release push.
- Remi tests proving upload gate parity with static publish, including
  accepted/unaccepted build-attestation signer, package-signature failure, and
  failed-upload staging cases where no artifact becomes installable.
- Remi negative tests proving failed release push leaves no converted-package
  row, no public chunk object, no package detail/index response, and no TUF
  target.
- Remi tests proving TUF timestamp refresh no longer returns the current 501
  stub after M2d starts publishing artifacts.
- M2d CLI/routing tests proving the old local-static destination guard still
  protects static repo writes while authenticated Remi HTTP(S) targets route
  through the Remi transport/artifact authorization path.
- `cargo run -p conary-test -- list` when integration manifests are touched.
- Owning package tests: `cargo test -p conary-core`, `cargo test -p conary`, and
  `cargo test -p remi` for slices that touch their owned behavior.
- Workspace finish gates before lock-in: `cargo fmt --check` and
  `cargo clippy --workspace --all-targets -- -D warnings`.
- Documentation gates for public claim changes:
  `scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete`
  and `scripts/check-doc-truth.sh`.

## Review And Due Diligence Gate

Before launching the remaining release surface as a `/goal`, run and archive the
review loop against this spec and the eventual implementation plan:

- Local agentic review for repository-context sanity and hidden prerequisites.
- `scripts/agentic-plan-review.sh <spec-or-plan>` for DeepSeek Reasonix and
  Gemini/Antigravity review when those CLIs are available.
- Optional Claude Opus review as a generated review artifact only. The repo no
  longer tracks active Claude-specific guidance or `.claude/` harness files.

Review artifacts belong under `docs/superpowers/reviews/` or an archive
subdirectory. Review-derived fixes must be patched into the spec or plan before
the `/goal` starts.

## Readiness Gate

M2 is ready for implementation planning when this umbrella design has completed
review and review-derived fixes are committed. The next implementation plan
should cover the remaining full M2 release surface: M2b, M2c, and M2d. It may be
launched as one `/goal`, but the plan must preserve internal checkpoints for
static attestation gates, foreign ingestion, and Remi push so each gate can be
verified before the next one expands the trust surface.
