# M2 Publish Hardening And Remi Push Design

**Date:** 2026-06-13
**Status:** Review-tightened design, pre-implementation
**Parent design:** `docs/superpowers/specs/2026-06-10-packaging-toolchain-design.md`

## Purpose

M2 turns the M1 packaging loop from a preview publishing path into a release
evidence path. M1a proved recipe-driven static publishing with honest
`sandboxed` provenance. M1b proved inference, `conary new`, `conary cook`, and
`conary try`. M2 now defines the trust contract for hermetic publish, signed
build attestations, artifact-form publish, foreign package ingestion, and Remi
push.

The core invariant is:

> Artifact-form publish is allowed only for hermetic artifacts with verified,
> accepted build attestations.

`conary publish <pkg.ccs> <target>` must refuse any artifact that lacks a valid
signed build attestation. A `hermetic` artifact is useful evidence, but it is
not enough for artifact-form publish until the attestation, signer authority,
package signature, output identity, and lint gates all pass. The happy path
remains project-form publish: `conary publish <target>` performs the hermetic
rebuild, signs the attestation, verifies the result, and publishes in one flow.

## Current Repo Facts

- `conary publish <pkg.ccs> <target>` is currently rejected in
  `apps/conary/src/commands/publish.rs` with an M2 attestation message.
- `conary cook --hermetic` is currently rejected in
  `apps/conary/src/commands/cook.rs`; M1b supports host or `--isolated` builds.
- Project-form publish currently forces Kitchen isolation but keeps network
  access enabled, so it correctly produces `hardening_level = "sandboxed"`, not
  `hermetic`, and does not embed a build attestation.
- `publish_kitchen_config` in `apps/conary/src/commands/publish.rs` currently
  hardcodes `allow_network = true`. M2a must plumb hermetic network policy
  through project-form publish so network is disabled after prefetch before any
  build claims `hardening_level = "hermetic"`.
- `crates/conary-core/src/ccs/manifest.rs` already carries the orthogonal
  provenance fields `origin_class` and `hardening_level`, but the
  `hardening_level` field comment still describes the M1a-only values. M2 must
  update the recognized value documentation when `hermetic` becomes a real
  emitted state and add a separate build-attestation field; `attested` is a
  derived publish state, not a `hardening_level` value.
- Source inference warns that npm, Python, and Go may still resolve over the
  network in M1b; offline/reproducible handling is explicitly M2 work.
- Remi's TUF timestamp refresh is still a prerequisite for M2 Remi push; the
  parent packaging design calls out the current 501 stub in
  `apps/remi/src/server/handlers/tuf.rs`.
- Static artifact-form publish has no accepted-signer policy today. The M1
  project-form path signs with its own local key directory; M2b must add signer
  allowlist verification against the target publication context rather than
  trusting any self-signed artifact.
- Foreign package conversion is not greenfield. Existing ownership lives under
  `crates/conary-core/src/ccs/convert/`, `crates/conary-core/src/ccs/legacy/`,
  `apps/conary/src/commands/install/conversion.rs`, and
  `apps/remi/src/server/conversion/`. M2c wires that machinery into
  `conary cook <foreign-pkg>` and adds file-output plus attestation boundary
  metadata; it must not invent a parallel conversion engine.
- Runtime scriptlet sandboxing defaults to `SandboxMode::Always`, while
  `--sandbox=auto` only enters the protected sandbox when `analyze_script()`
  returns `Medium` or higher. The current analyzer names remote shell pipes,
  destructive filesystem operations, privilege edits, network backdoors, and
  obfuscation patterns; it does not yet classify language package-manager fetch
  commands such as npm or Bun as a medium-risk trigger.
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

M2 should add focused core concepts and keep command modules thin.

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
the local key directory during project-form repo initialization. Artifact-form
publish to a brand-new static repo must require an explicit accepted-signer
decision rather than trusting any self-signed artifact. For Remi, accepted signer
policy is enforced server-side from Remi's configured trusted publisher keys.
The client may preflight it, but the server is the authority.

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
| Static target signer authority, package-key policy, and brand-new repo accepted-signer UX | `crates/conary-core/src/repository/static_repo/` plus `apps/conary/src/commands/publish.rs` orchestration |
| Remi target signer authority and trusted build-attestation signer config/storage | `apps/remi/src/server/config.rs` and the M2d Remi push handler |
| Publish lint composition | shared core lint helpers, orchestrated from `apps/conary/src/commands/publish.rs` and rechecked by Remi |
| Command-risk evidence and classifications | existing `crates/conary-core/src/ccs/convert/command_evidence.rs` / blocked-class model or a small core module extracted from it; `container/analysis.rs` consumes that model for `--sandbox=auto` rather than growing a third scanner |

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
publisher ordering.

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
   controls applied, and build paths mapped.
9. Capture output identity.
10. Compare against the last host build record when one exists.
11. Sign a build attestation with the publisher Ed25519 key.
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
6. Run publish lint.
7. Attach or replace the target-local package signature in the CCS signature
   layer without rewriting canonical package content, manifest data covered by
   output identity, or the build-attestation payload.
8. Verify the final CCS package signature against the target package trust
   policy and recheck that output identity stayed unchanged.
9. Recheck the final staged artifact bytes before metadata or index visibility.
10. Publish to a static repo or Remi target without rebuilding.

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

The shared classifier should reuse or extract from the existing conversion
command-evidence and blocked-class machinery rather than creating an unrelated
third scanner. Runtime scriptlet auto-sandboxing can then map the same
classification vocabulary into `ScriptRisk`.

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
milestone." After M2a, the command may emit `hardening_level = "hermetic"` only
when all inputs are pinned and the build runs offline. If a target cannot be
made hermetic, the command refuses with actionable diagnostics rather than
falling back silently or overclaiming.

Plain `conary cook --isolated` may produce hermetic artifacts after M2a, but it
does not sign them and therefore does not make them artifact-form publishable.
Project-form publish remains the path that combines hermetic rebuild,
attestation signing, and publication.

The hidden compatibility `--hermetic` surface should either route to the M2
hermetic behavior after the CLI contract is reviewed or remain hidden/rejected.
The public contract stays `--isolated`; the provenance field tells the truth.

### `conary publish <target>`

Project-form publish is the release happy path. It performs the hermetic rebuild
and signing automatically. After M2b, a successful project-form publish emits a
hermetic artifact with a verified build-attestation envelope and publishes it.

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
Remi push refuses too.

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
ignore set and warn that identity is weaker. CI refuses dirty local trees; M2a
must define the CI-mode detection mechanism, such as an explicit flag or
environment contract.

Hermetic local-source materialization must use the same canonical file list that
was hashed. The current Kitchen local-source path copies the resolved source
directory recursively; that is acceptable for M1 iteration, but M2a must not let
ignored or untracked files influence a build while being absent from
`BuildInputIdentity`. The child plan must either materialize from the hashed file
list or refuse when excluded files could affect the build.

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

M2a must also define a language-ecosystem dependency policy. At minimum, each
supported inferred ecosystem needs one of these outcomes:

- accepted offline mode with a lock/vendor input recorded in
  `BuildInputIdentity`
- clear refusal with a diagnostic naming the missing lock/vendor input
- explicit deferral from hermetic support for that ecosystem in the M2a plan

Cargo, Go, npm, and Python are the first ecosystems to classify because M1b
already warns that they may resolve dependencies over the network.

Provisional M2a defaults:

| Ecosystem | M2a hermetic default |
|-----------|----------------------|
| Cargo | Accept only with `Cargo.lock` plus a vendored or cached crate source set that is part of `BuildInputIdentity`; otherwise refuse. |
| Go | Accept only with `go.sum` plus `vendor/` or a pinned module cache recorded in `BuildInputIdentity`; otherwise refuse. |
| npm | Accept only with a lockfile (`package-lock.json` or equivalent) plus vendored `node_modules` or a pinned npm cache recorded in `BuildInputIdentity`; otherwise refuse. |
| Python | Defer hermetic support unless the M2a plan chooses a lockfile/wheelhouse strategy; refuse publish-hermetic for ambiguous pip/network resolution. |

M2a should implement the Cargo path first because Rust workspace packaging is
the closest dogfood path for Conary itself. The child plan should prefer
explicit offline invocation or config over relying only on the network namespace.
Other ecosystems may begin as fail-closed classifications with diagnostics if
their offline strategy is not ready.

### `BuildOutputIdentity`

Records what was produced:

- canonical CCS content identity
- file-manifest Merkle root
- package name, version, release, and architecture

The attestation signs both input identity and output identity. Output identity is
computed over canonical package content excluding signature and attestation
blocks, so the signature is not self-referential. The M2b plan must align this
with the existing CCS manifest layering, where package signatures are added
after manifest/content identity is computed.

The existing `dna_hash` is not a build output identity. It identifies a
provenance chain that includes source/build/dependency data and, in the newer
provenance module, signatures. M2b must not sign an attestation over a hash that
would change when the attestation or signatures are embedded. If a stronger
content-only identity is needed, add a new explicit content identity instead of
reusing `dna_hash`.

### `BuildAttestationPayload`

Versioned signed payload schema:

- schema version
- origin class
- hardening level
- build input identity
- dependency lock
- build output identity
- build-command risk report
- scriptlet risk report when install hooks or converted legacy metadata are
  present
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

### `PublishLintReport`

Publish lint produces a machine-testable result and human-readable diagnostics.
It must identify the exact failed gate:

- missing attestation
- build-attestation signature mismatch
- package signature mismatch or untrusted package signer
- output identity mismatch
- unaccepted signer key
- absent or unknown provenance class
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
- "cargo build requires network access; vendor dependencies or provide a
  lock-compatible offline source before publish"
- "local tree is dirty; commit changes or rerun outside CI with dirty-tree
  recording"
- "dependency lock is missing TUF snapshot version; sync and rebuild"
- "foreign conversion is missing scriptlet policy metadata"

## Implementation Slices

### M2a: Hermetic Publish Foundation

Implements source prefetch, source identity, local tree hashing, offline Kitchen
execution, Conary dependency lock capture, language-ecosystem dependency policy,
hashed-file-list local-source materialization, build-command/scriptlet risk
classification, reproducibility controls, divergence diagnostics, and
`hardening_level = "hermetic"`.

M2a does not enable artifact-form publish. Success means the project-form
publish pipeline can internally produce hermetic evidence without overclaiming
attestation. Before M2b, release publishability does not graduate; project-form
publish either remains preview-labeled or emits local hermetic artifacts for the
next slice to sign and gate.

### M2b: Attestation And Publish Gates

Adds signed build attestations, verification helpers, publish lint, and the
artifact-form publish gate. This is the first slice that may embed
`BuildAttestationEnvelope` and unlock `conary publish <pkg.ccs> <target>` for
artifacts with `hardening_level = "hermetic"`.

Project-form publish performs hermetic rebuild plus signing automatically.
Artifact-form publish refuses artifacts whose build-command or scriptlet risk
reports are absent, unknown, or unclean.

M2b must also define the explicit accepted-signer UX for artifact-form publish
to a brand-new static repo, such as a policy file or
`--accept-build-signer <key-id>` flow, and how that decision is persisted.

### M2c: Foreign Package Ingestion

Adds `.rpm`, `.deb`, and `.pkg.tar.zst` routing through `conary cook` by reusing
the existing conversion modules. Conversion outputs use
`origin_class = "foreign-converted"` and must carry boundary metadata before
they are publishable. A foreign-converted artifact must also carry a scriptlet
and build-body risk report when the source package format can express those
hooks or command bodies.

### M2d: Remi Push

Adds authenticated Remi upload for CCS artifacts that pass the static
artifact-form gate and completes the Remi TUF timestamp refresh prerequisite.
Remi push uses the same artifact gate as static artifact-form publish, plus the
parent design's bearer-token upload authentication. M2d must add Remi trusted
build-attestation signer configuration or storage, define empty-list behavior as
fail-closed, and prove uploads stage privately until all artifact gates pass.

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
- CLI tests proving project-form publish produces hermetic artifacts with
  verified build-attestation envelopes after M2b.
- CLI tests proving artifact-form publish rejects `host`, `sandboxed`,
  `hermetic` without attestation, missing-attestation, mismatched-attestation,
  unaccepted-signer, package-signature failure, absent/unknown provenance,
  stale/unknown policy version, recorded-draft, and unclean foreign-converted
  artifacts.
- Regression tests proving `--isolated` and any hidden `--hermetic`
  compatibility path never overclaim.
- Regression tests proving attestation output identity excludes
  signature/attestation bytes.
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
- Remi tests proving upload gate parity with static publish, including
  accepted/unaccepted build-attestation signer, package-signature failure, and
  failed-upload staging cases where no artifact becomes installable.
- `cargo run -p conary-test -- list` when integration manifests are touched.
- Owning package tests: `cargo test -p conary-core`, `cargo test -p conary`, and
  `cargo test -p remi` for slices that touch their owned behavior.
- Workspace finish gates before lock-in: `cargo fmt --check` and
  `cargo clippy --workspace --all-targets -- -D warnings`.
- Documentation gates for public claim changes:
  `scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete`
  and `scripts/check-doc-truth.sh`.

## Readiness Gate

M2 is ready for implementation planning when this umbrella design has completed
review and review-derived fixes are committed. The next child plan should be M2a
only. M2b, M2c, and M2d plans should be written after the preceding slice has
landed or at a reviewed checkpoint, not opportunistically folded into M2a.
