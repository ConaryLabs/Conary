# Remi Scriptlet Evidence Queue Design

**Status:** Active design
**Date:** 2026-07-03
**Scope:** Admin/operator-only workflow for turning Remi scriptlet refusal evidence into adapter work

## Goal

Remi should make blocked and review-required scriptlet evidence painless for
maintainers to use. The first version is an admin/operator-only evidence queue
that aggregates non-public conversion refusals into adapter candidate clusters.
It should help maintainers see what users are trying, choose adapter work by
impact, export sanitized fixture/design packets, and track adapter progress
without exposing private review artifacts or weakening publication gates.

The longer-term goal is to project a sanitized subset of this queue into a
public tracker once Conary has real external users and enough operational
discipline to avoid confusing "we saw this" with "this is supported."

## Non-Goals

- Do not make blocked packages public-ready.
- Do not use model output as publication, install, or support authority.
- Do not expose raw scriptlet bodies, private artifact paths, local host paths,
  environment values, or maintainer-only notes publicly.
- Do not replace the adapter registry, support matrix, target-profile facts, or
  legacy scriptlet bundle decision model.
- Do not build a public roadmap surface in the first slice.

## Existing Repo Facts

- Remi already writes private scriptlet review artifacts under
  `scriptlet-review` when converted packages are review-required or blocked.
- `apps/remi/src/server/publication.rs` defines `PublicationGateReport` and
  `ScriptletReviewArtifact`.
- `apps/remi/src/server/conversion/persistence.rs` stores non-public scriptlet
  metadata and review artifact paths on converted-package rows.
- `apps/remi/src/server/handlers/admin/packages.rs` already has an
  admin-authorized review artifact lookup and validates that artifact paths stay
  under the review root.
- `apps/remi/src/server/scriptlet_corpus.rs` can summarize scriptlet command
  counts, command forms, and blocked-class hints for scan-only evidence.
- `crates/conary-core/src/ccs/convert/blocked_classes.rs`,
  `adapters.rs`, and `support_matrix.rs` remain the authority for blocked class
  policy, adapter-backed replacement, and public-ready proof.
- `docs/modules/remi.md` explicitly says scan-only corpus evidence is for
  adapter planning only, not replacement authority.

## Operator Workflow

1. A tester attempts to install or update a package through Remi.
2. Remi converts or scans the legacy package, refuses public serving when
   scriptlet policy requires review or blocking, and stores the current private
   review artifact.
3. The evidence queue aggregates the refusal into a stable candidate cluster by
   distro, package family, blocked class, command, normalized command shape,
   lifecycle phase, architecture, and target profile when available.
4. A maintainer views the queue sorted by impact: attempts, unique packages,
   recency, blocked class, and whether the command shape appears repetitive
   enough for an adapter.
5. The maintainer marks candidate state, adds internal notes, and exports a
   sanitized adapter packet.
6. Adapter work happens in the normal code path: tests, fixtures, adapter
   registry changes, support matrix promotion, target-profile facts when needed,
   Remi reconversion, and publication gate verification.
7. If a later release supports the exact shape, Remi reconversion can move
   affected packages from blocked or review-required to public-ready only when
   existing gates prove every remaining scriptlet effect is supported.

The loop is intentionally boring:

```text
attempt -> refuse safely -> aggregate evidence -> export packet
-> implement adapter with fixtures -> support matrix proof -> reconvert
```

## Candidate Clusters

The queue should present clusters rather than raw rows. A cluster represents a
maintainer-actionable shape such as:

```text
class: initramfs
command: dracut
command_shape: dracut --force <boot>/<kver>/initramfs.img
distro: fedora
target_profile: fedora-44
packages: kernel-core, akmods
architectures: x86_64, aarch64
attempts: 38
state: needs-triage
latest_seen: 2026-07-03T12:00:00Z
sample_artifacts: [review:fedorax86_64:sha256-example]
```

The normalized command shape must use sanitized arguments already suitable for
maintainer review. Kernel-version-like values stay `<kver>`, boot paths stay
`<boot>/<normalized-path>`, and raw environment values are excluded.

### Stable Cluster Key

Cluster state must not depend on volatile `converted_packages.id` rows. Remi can
delete or rebuild converted-package rows when conversion versions change or
cache artifacts go stale, but maintainer triage state should survive that churn.

The stable key for v1 is:

```text
schema_version
distro
target_profile_or_unknown
blocked_class
command
normalized_command_shape_hash
lifecycle_phase_or_unknown
```

The normalized command shape itself is display data and packet-export data. The
hash is the key component. Package names, versions, architectures, attempt
counts, timestamps, review artifact IDs, and sample evidence are derived fields
attached to the cluster, not part of the key. Architecture remains a filter and
aggregate field because the same scriptlet shape can matter across multiple
architectures without changing the adapter work item.

If normalization rules change, Remi should either migrate old keys through an
explicit compatibility mapping or mark affected clusters as `needs-triage`
under the new `schema_version`. Silent key rewrites must not discard maintainer
state.

### Normalization Rules

The first implementation should reuse the sanitized evidence already projected
into publication reports when available. Scan-only corpus evidence can use the
same normalizer once it exists; until then, scan-only command forms are planning
hints, not stable cluster authority.

Minimum v1 normalization rules:

- replace kernel-version-like segments with `<kver>`
- replace `/boot/` prefixes with `<boot>/`
- strip or placeholder shell environment references such as `$VAR` and
  `${VAR}`
- drop raw environment values entirely
- preserve only package-authored command names and normalized argument shape
- normalize non-whitelisted absolute paths to a coarse placeholder unless they
  are package payload paths or approved boot/security placeholders

## Candidate States

Candidate state is operator-maintained metadata. It is not support authority.

| State | Meaning |
| --- | --- |
| `needs-triage` | Evidence exists, but no one has decided whether it is adapter work. |
| `adapter-candidate` | The shape looks repetitive and modelable enough to design. |
| `in-design` | A maintainer is writing or reviewing an adapter design. |
| `in-implementation` | A branch or plan is actively implementing coverage. |
| `covered-partial` | Some observed shapes are supported, but the cluster still has unsupported variants. |
| `covered-public-ready` | The exact cluster shape is covered by adapter/support-matrix proof; Remi still decides per package. |
| `wont-support` | The shape is intentionally blocked for the foreseeable future. |

State transitions should be auditable and reversible. The first implementation
stores state in Remi's database, but every state must be derived from or
attached to stable cluster keys so a conversion-version bump, reconversion, or
`ConvertedPackage::delete_by_checksum` cleanup does not erase maintainer intent.

Internal cluster state names are distinct from future public status names. Do
not store public-status strings such as `tracked` or `covered-in-release` in the
internal state table; public tracking should be a later projection layer.

## Adapter Packet Export

The queue should export a packet that can seed a design, plan, or fixture work
item without forcing maintainers to dig through review JSON manually.

Packet contents:

- cluster key and state
- blocked class and reason codes
- command and normalized command shape
- distro, release/profile, architecture set
- affected packages and versions
- attempt counts and first/last seen timestamps
- sanitized sample boot/security intents
- links or IDs for private review artifacts
- suggested fixture names
- current support-matrix row, if one exists
- current blocked-class registry entry, if one exists
- maintainer notes

The packet may include private review artifact identifiers for admin use, but a
public export mode must omit those identifiers.

## LLM Assistance

LLM usage should be opt-in and advisory. A cheap model can help make the queue
less tedious, but it must never become a trust boundary.

Allowed LLM tasks:

- cluster similar command shapes
- summarize repeated patterns
- suggest adapter names and fixture group names
- draft a design note from sanitized evidence
- flag missing target-profile facts to investigate
- explain why a cluster appears related to an existing blocked class

Forbidden LLM tasks:

- mark a package, class, or command shape public-ready
- edit the support matrix without human-authored tests
- bypass review/blocked publication gates
- decide target compatibility
- expose private artifact paths or raw scriptlet bodies
- produce install authority consumed directly by Conary or Remi

LLM input must be the sanitized packet form, not raw private artifacts. LLM
output should be stored as a draft summary with provider/model metadata, prompt
version, timestamp, and "advisory only" status.

LLM calls must not run synchronously inside package-serving HTTP request paths.
The safe shapes are:

- local/operator CLI generation from an exported packet
- an out-of-process asynchronous worker or sidecar
- a disabled-by-default Remi worker queue with explicit operator configuration

The LLM configuration surface needs its own child design before implementation.
That design must cover provider selection, API-key storage, billing limits,
retry behavior, egress controls, and whether summaries are stored per cluster
or per exported packet.

## Public Tracking Later

After the first external tester loop is active, Remi can project a sanitized
subset of the evidence queue into a public status surface. That surface should
answer "is this known, tracked, and moving?" without promising support.

Public fields can include:

- package or package family
- distro and architecture family
- blocked class
- broad status such as `tracked`, `adapter-candidate`, `in-progress`,
  `partially-covered`, `covered-in-release`, or `wont-support`
- rough attempt count bands, not exact operator analytics if privacy policy
  says otherwise
- release or issue link when available

Public fields must not include review artifact paths, raw scriptlet bodies,
private maintainer notes, environment values, or package-manager host-local
paths.

This public projection is outside the first implementation milestone. It should
get a separate follow-up design once the admin queue has real data and the
external tester loop has enough usage to make public status wording useful.

## Data Boundaries

The evidence queue is an operational index over existing conversion evidence,
not a second source of truth for conversion safety. The queue may cache derived
cluster summaries, but the underlying package decision remains in the legacy
scriptlet bundle summary and Remi publication gate.

The queue requires a Remi-side database migration. At a high level, the schema
should include:

- `scriptlet_evidence_clusters`: stable cluster key, current state, first/last
  seen timestamps, and aggregate display fields
- `scriptlet_evidence_cluster_samples`: cluster key to converted-package or
  review-artifact sample references
- `scriptlet_evidence_state_events`: append-only state transition audit entries
- `scriptlet_evidence_notes`: maintainer-only notes keyed by cluster
- later, `scriptlet_evidence_llm_summaries`: advisory summaries keyed by
  cluster or exported packet

The first implementation should prefer SQL grouping/views over materialized
cached counts when practical. If it stores materialized aggregate counts, it must
update or invalidate them whenever converted-package rows are inserted, deleted,
or marked for reconversion.

Suggested ownership boundaries:

- `apps/remi/src/server/publication.rs`: refusal report and review artifact
  schema remain publication-owned.
- `apps/remi/src/server/conversion/persistence.rs`: conversion persistence
  remains responsible for creating review artifacts and recording scriptlet
  metadata.
- A future `apps/remi/src/server/scriptlet_evidence_queue/` module can own
  cluster-key generation, aggregation queries, state storage, packet export, and
  advisory LLM summary records.
- `apps/remi/src/server/handlers/admin/` can expose admin-only list/detail/export
  endpoints.
- `crates/conary-core/src/ccs/convert/` remains the only place that can promote
  a command shape from blocked/review into adapter-backed support.

## Backfill Strategy

Existing Remi deployments may already have `converted_packages` rows with
`publication_status` values such as `private-review` or `blocked`. The evidence
queue must account for those rows without slowing normal package serving.

V1 should use an operator-triggered or admin-endpoint-triggered backfill rather
than a mandatory startup full-table scan. The backfill should run in batches,
record progress, and be safe to retry. New conversions can update clusters
incrementally after the queue schema exists.

Backfill should include:

- only converted rows with non-public scriptlet publication status or malformed
  scriptlet summary evidence
- batch limits so a large conversion cache does not block Remi startup
- stale-artifact marking when `review_artifact_path` is missing or no longer
  under the review root
- a test proving pre-existing blocked/private-review rows appear in the queue
  after backfill

## Error Handling

- Missing review artifact files should make a queue row stale, not public.
- Malformed scriptlet summary JSON should remain a blocked/review signal and
  should still appear as an evidence row when enough metadata exists to cluster
  it.
- Cluster-key changes across schema versions should preserve old state through a
  compatibility migration or mark the row as needing retriage.
- LLM failures should be recorded as advisory-summary failures and must not
  affect package serving.
- Admin endpoints must keep the existing authorization and review-root path
  validation posture.

## Testing And Verification

Initial implementation should prove:

- blocked/review conversions produce or update evidence queue clusters
- queue clusters do not expose `review_artifact_path` through non-admin surfaces
- stale or missing artifacts are visible to admins but not served publicly
- packet export contains sanitized evidence and omits raw scriptlet bodies
- candidate state transitions are persisted and auditable
- advisory LLM summaries cannot affect `publication_status`
- evidence queue endpoints reject missing or insufficient admin scope with
  `401 Unauthorized` or `403 Forbidden`
- normalization strips environment references and sensitive path shapes from
  exported packets
- backfill makes pre-existing `blocked` and `private-review` converted rows
  visible in admin queue results
- reconversion after adapter support can move a previously blocked cluster to
  `covered-public-ready` without making that cluster itself publication
  authority
- existing publication gate tests still pass
- support-matrix promotion still requires adapter fixture proof

Focused proof should include:

```bash
cargo test -p remi publication
cargo test -p remi scriptlet_evidence_queue
cargo test -p conary-core support_matrix
bash scripts/check-doc-truth.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
git diff --check
```

`cargo test -p remi scriptlet_evidence_queue` is the intended focused proof
target after the evidence-queue module lands; it does not exist before the first
implementation slice.

## Open Implementation Slices

1. Add the Remi-side evidence queue schema and stable cluster-key normalizer.
2. Add batched, retryable backfill over existing converted-package scriptlet
   metadata.
3. Add incremental aggregation for new blocked/review conversion outcomes.
4. Add admin list/detail endpoints with review-root safety checks and admin-scope
   tests.
5. Add candidate state, state transition audit events, and maintainer notes.
6. Add packet export for adapter design and fixture seeding.
7. Add optional advisory LLM summary records only after a child design chooses
   the operator configuration and execution boundary.
8. Defer public projection to a separate design after external tester usage and
   privacy wording are ready.
