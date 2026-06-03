# Legacy Scriptlet Publication Gate Design

## Summary

Goal 5 turns the passive scriptlet publication metadata from Goal 4 into Remi
serving policy. A converted package whose legacy scriptlet bundle is fully
portable may continue to appear as ready. A converted package whose bundle is
`private-review`, `blocked`, `local-only`, malformed, or otherwise not exactly
public must not be advertised or served as a public-ready artifact.

The work is still not replay. Goal 5 does not make clients execute legacy
scriptlets, does not implement sandboxed replay, does not promote packages that
need raw native scriptlet execution, and does not change install/update/remove
behavior. It only prevents Remi from presenting incomplete or unsafe conversions
as ready packages, while preserving operator-visible evidence for review.

## Source Context

Read these first when implementing:

- `docs/superpowers/plans/2026-05-27-legacy-scriptlet-semantics-bundle-goal-queue.md`
- `docs/superpowers/specs/2026-05-27-legacy-scriptlet-semantics-bundle-design.md`
- `docs/superpowers/specs/2026-05-28-legacy-scriptlet-passive-remi-bundle-embedding-design.md`
- `docs/superpowers/specs/2026-05-28-legacy-scriptlet-bootstrap-adapters-design.md`
- `docs/modules/remi.md`
- `crates/conary-core/src/ccs/legacy_scriptlets.rs`
- `crates/conary-core/src/ccs/convert/scriptlet_bundle.rs`
- `crates/conary-core/src/db/models/converted.rs`
- `apps/remi/src/server/conversion.rs`
- `apps/remi/src/server/jobs.rs`
- `apps/remi/src/server/handlers/jobs.rs`
- `apps/remi/src/server/handlers/packages.rs`
- `apps/remi/src/server/handlers/index.rs`
- `apps/remi/src/server/handlers/detail.rs`
- `apps/remi/src/server/handlers/sparse.rs`
- `apps/remi/src/server/handlers/chunks.rs`
- `apps/remi/src/server/handlers/oci.rs`
- `apps/remi/src/server/handlers/admin/packages.rs`
- `apps/remi/src/server/index_gen.rs`
- `apps/remi/src/server/search.rs`
- `apps/remi/src/server/federated_index.rs`
- `apps/remi/src/server/delta_manifests.rs`
- `apps/remi/src/server/prewarm.rs`

Relevant current code facts:

- Goal 4 already embeds `CcsManifest.legacy_scriptlets` in converted CCS
  archives and stores compact scriptlet summary fields on `converted_packages`.
- `ConvertedPackage::scriptlet_summary()` reconstructs a
  `ScriptletBundleSummary` from the database row and hides private review paths
  from public API projection through `ScriptletPackageMetadata`.
- `publication_status` is currently informational only. Package, download,
  metadata, generated-index, and job paths can still treat `private-review` and
  `blocked` rows as ready.
- `ConvertedPackage::new()` and `new_server()` default legacy rows to
  `publication_status = "public"` so existing non-scriptlet test fixtures keep
  their behavior unless a test intentionally sets scriptlet metadata.
- `JobStatus` currently has `Pending`, `Converting`, `Ready`, and `Failed`.
  `Ready` is the only terminal success state exposed through job polling.

## Scope

Goal 5 includes:

- a single Remi publication gate that converts scriptlet summary metadata into
  allow/review/blocked serving decisions;
- public package and download endpoints that refuse to serve non-public
  converted artifacts as ready;
- job states and job responses that distinguish ready, review-required, and
  blocked outcomes;
- generated repository indexes and public metadata endpoints that do not
  advertise non-public conversion rows as ready;
- private review artifacts for review-required and blocked conversions;
- admin-only retrieval of review artifacts;
- structured negative responses with operator-visible reason codes, unknown
  commands, blocked classes, evidence digests, and review-artifact availability;
- tests for public, review-required, blocked, local-only, stale, and malformed
  summary flows.

Goal 5 excludes:

- database migrations;
- legacy scriptlet replay or sandbox execution;
- client-side enforcement;
- installer changes;
- publication promotion of raw legacy replay packages;
- operator approval flows that turn private-review rows into public-ready rows;
- changing native package parsing or adapter classification;
- exposing local filesystem paths through public APIs.

## Policy Model

Add a Remi-local module, for example `apps/remi/src/server/publication.rs`, that
owns all serving policy derived from scriptlet metadata.

Use exact string matching on the normalized Goal 4 summary values, plus an
explicit summary-health bit. Only a valid summary with the literal
`publication_status = "public"` is public-ready. Everything else is not
public-ready:

| publication_status | Serving outcome | Public package manifest | Public download | Job terminal status | Generated index |
| --- | --- | --- | --- | --- | --- |
| valid `public` | ready | allowed | allowed | `ready` | listed as converted |
| `private-review` | review required | structured refusal | structured refusal | `review-required` | not listed as converted-ready |
| `blocked` | blocked | structured refusal | structured refusal | `blocked` | not listed as converted-ready |
| `local-only` | review required | structured refusal | structured refusal | `review-required` | not listed as converted-ready |
| unknown/missing/malformed summary | review required | structured refusal | structured refusal | `review-required` | not listed as converted-ready |

`local-only` remains reserved for future local-file workflows. If a row ever
contains it in Goal 5, public Remi must treat it as non-public.

The gate should be conservative when summary JSON is malformed. Goal 4's
`ConvertedPackage::scriptlet_summary()` intentionally recovers a best-effort
summary for display, but Goal 5 needs to know whether that recovery path was
used. Add a helper that returns both the summary and its parse health so a row
with malformed `scriptlet_summary_json` cannot be treated as public-ready just
because its explicit `publication_status` column says `public`.

`summary_valid` is true only when `scriptlet_summary_json` deserializes cleanly,
the deserialized `publication_status` matches the scalar `publication_status`
column, and the row passes one of these shape checks:

1. the JSON object is an explicit Goal 4 summary, containing at least
   `scriptlet_fidelity`, `target_compatibility`, `publication_status`,
   `decision_counts`, `blocked_reason_codes`, `review_reason_codes`,
   `unknown_commands`, and `blocked_classes`; or
2. the JSON object is the constructor default `{}` and every scalar/list
   scriptlet field is also at its constructor default
   (`scriptlet_fidelity = "unknown"`, `target_compatibility = "unknown"`,
   `publication_status = "public"`, no evidence digests, empty blocked reason
   codes, and no review artifact path).

The second case is a narrow no-bundle/default compatibility path for existing
admin uploads or native CCS rows that do not carry a `legacy_scriptlets` bundle.
Any row that carries scriptlet evidence, a review artifact path, non-empty reason
lists, or a non-default scalar field must have an explicit summary object. A row
whose summary JSON is malformed, empty, non-object, or missing required explicit
keys must have `summary_valid = false`; the scalar `publication_status` may still
hydrate the best-effort display summary, but it cannot make the row public-ready.

### Public-Ready Predicate

Use a narrow predicate:

```rust
pub fn is_public_ready(summary: &ScriptletBundleSummary, summary_valid: bool) -> bool {
    summary_valid && summary.publication_status == "public"
}
```

Do not infer public readiness from `scriptlet_fidelity`, `target_compatibility`,
`conversion_fidelity`, `decision_counts`, or absence of blocked reason codes.
Those fields are explanatory evidence. `publication_status` is the active gate.

This intentionally means that a package with fully preserved but unreplayable
native scriptlets remains private-review until a later goal implements replay
or an explicit future curation workflow. Until Goal 6 client and replay
enforcement exists, packages requiring raw legacy replay are not public-ready.

### Gate Types

Use typed structures rather than sprinkling string checks through handlers:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicationDecision {
    Ready,
    ReviewRequired(PublicationGateReport),
    Blocked(PublicationGateReport),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicationRefusal {
    ReviewRequired(PublicationGateReport),
    Blocked(PublicationGateReport),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublicationGateReport {
    pub publication_status: String,
    pub scriptlet_fidelity: String,
    pub target_compatibility: String,
    pub summary_valid: bool,
    pub message: String,
    pub reason_codes: Vec<String>,
    pub blocked_reason_codes: Vec<String>,
    pub review_reason_codes: Vec<String>,
    pub unknown_commands: Vec<String>,
    pub blocked_classes: Vec<String>,
    pub evidence_digest: Option<String>,
    pub curation_evidence_digest: Option<String>,
    pub review_artifact_available: bool,
}
```

This report is the refusal/job/admin detail shape. It is separate from the
existing `ScriptletPackageMetadata` projection used by public metadata and
generated-index entries. Public metadata should keep using
`ScriptletPackageMetadata` for public-ready rows only; refusal responses may use
the richer `PublicationGateReport`.

`reason_codes` is the compact ordered union of blocked reason codes, review
reason codes, unknown command names encoded as `unknown-command:<name>`, and
blocked-class IDs. It exists so job and refusal responses can show actionable
reasons without clients knowing every detail field.

Ordering must be deterministic:

1. blocked reason codes;
2. review reason codes;
3. unknown command names in sorted order;
4. blocked class IDs in sorted order.

Deduplicate while preserving that group order.

### Summary Conversion Helper

Admin CCS upload and tests need to project an already-embedded
`LegacyScriptletBundle` back into the compact Remi summary shape. Goal 4's
`summary_from_bundle()` helper is currently private to
`crates/conary-core/src/ccs/convert/scriptlet_bundle.rs`. Goal 5 should expose a
public conversion API from that module, for example:

```rust
impl ScriptletBundleSummary {
    pub fn from_bundle(
        bundle: &LegacyScriptletBundle,
        evidence_digest: Option<String>,
    ) -> Self {
        ...
    }
}
```

The public API should preserve the same aggregation behavior as Goal 4:
decision counts, blocked reason codes, review reason codes, unknown commands,
blocked classes, fidelity, target compatibility, and publication status come
from the bundle. `review_artifact_path` remains `None` until Remi writes a
private review artifact.

## Structured Negative Responses

Public package and download endpoints currently return plain text for many
errors. Goal 5 may add JSON for publication refusals because the goal requires
structured negative conversion results. Keep other existing error shapes
unchanged.

Use a response shape like:

```json
{
  "status": "review-required",
  "message": "Converted package requires scriptlet review before public serving",
  "distro": "fedora",
  "package": "pkg",
  "version": "1.0",
  "architecture": "x86_64",
  "scriptlets": {
    "publication_status": "private-review",
    "scriptlet_fidelity": "review-required",
    "target_compatibility": "review-required",
    "summary_valid": true,
    "reason_codes": ["review-class-debconf"],
    "blocked_reason_codes": [],
    "review_reason_codes": ["review-class-debconf"],
    "unknown_commands": [],
    "blocked_classes": [],
    "evidence_digest": "sha256:...",
    "curation_evidence_digest": null,
    "review_artifact_available": true
  }
}
```

Use these status codes:

- `409 Conflict` for `review-required` and `local-only` rows;
- `403 Forbidden` for `blocked` rows.

The response must not contain `review_artifact_path`, CCS filesystem paths,
temporary directories, repository credentials, or host-local cache paths.

## Review Artifacts

Goal 5 should persist a private review artifact for every non-public conversion
result. The artifact is operator evidence, not a public package asset.

Store artifacts under a private cache subtree such as:

```text
<cache_dir>/scriptlet-review/<distro>/<package>/<version>/<arch-or-noarch>/<evidence-digest>.json
```

Use sanitized path components and write atomically through a temporary file in
the review root plus rename. The path is stored only in
`converted_packages.review_artifact_path` and never appears in public JSON.

The artifact schema should be explicit and versioned:

```rust
pub struct ScriptletReviewArtifact {
    pub schema: String, // "conary.remi.scriptlet-review.v1"
    pub distro: String,
    pub package: String,
    pub version: String,
    pub architecture: Option<String>,
    pub original_format: String,
    pub publication: PublicationGateReport,
    pub conversion_fidelity: String,
    pub conversion_version: i32,
    pub ccs_content_hash: String,
    pub ccs_total_size: u64,
    pub created_at: String,
}
```

The artifact may include public package identity, digests, structured scriptlet
reasons, and conversion metadata. It should not include private filesystem
paths except inside the database column that points to the artifact itself.

`curation_evidence_digest` remains reserved for explicit operator curation.
Goal 5 review artifacts are evidence for curation, but they are not themselves
operator curation decisions. Therefore:

- if a future/manual row already has `curation_evidence_digest`, preserve and
  expose it as a digest;
- do not fabricate `curation_evidence_digest` for an unreviewed conversion;
- tests should prove the field remains absent unless explicitly set.

### Admin Retrieval

Add an admin-only route for review artifact retrieval. The exact route can
follow existing admin handler conventions, for example:

```text
GET /v1/admin/packages/:distro/:package/scriptlet-review?version=...&arch=...
```

Rules:

- require the existing admin scope check;
- require `version` as a query parameter, not a path segment, because native
  package versions may contain characters such as `:`, `~`, or `+`;
- locate the current non-stale converted row by package identity and optional
  architecture through the same architecture-aware lookup used by package
  serving;
- if `arch` is omitted and more than one row matches the package/version, return
  `409` with an ambiguity message instead of picking the newest row;
- return `404` when no converted row exists or the row has no artifact;
- return `404` when the stored artifact path points to a file that no longer
  exists;
- return `409` when the row needs reconversion;
- return `200 application/json` with artifact bytes when present;
- never read arbitrary paths from user input;
- canonicalize or otherwise verify the stored artifact path stays under the
  configured private review root before reading.

Goal 5 does not add an admin approval or promotion endpoint. That belongs to a
future curation goal because promoting packages that need legacy replay before
Goal 6 would contradict the safety boundary.

## Conversion Flow

`LegacyConverter` already returns `scriptlet_metadata`. Remi should classify
the server-side result after persistence metadata is known:

1. convert and write the CCS archive as in Goal 4;
2. compute final content hash and total size;
3. create the `ConvertedPackage`;
4. copy scriptlet metadata from `ConversionResult.scriptlet_metadata`;
5. evaluate the publication gate;
6. for non-public rows, write the private review artifact and set
   `review_artifact_path` before inserting the row;
7. insert the row;
8. return a terminal conversion outcome with sanitized scriptlet metadata and a
   publication decision.

The CCS file may remain in the private package cache for operator inspection.
Public handlers must not stream it unless the row is public-ready.

Writing the review artifact is part of successful persistence for non-public
native conversions. If Remi cannot write that artifact, the conversion should
fail before inserting the row rather than leaving a review-required package
without private operator evidence.

Hot-cache conversion results must evaluate the same gate from the stored row.
Do not let `build_result_from_existing()` or in-memory job results bypass the
publication decision.

Publication refusals are successful terminal conversion outcomes, not
`anyhow::Error` failures. Do not encode a review-required or blocked package as
`Err(...)`, because that would collapse policy refusals into generic failed jobs.
Use one outer type for both fresh conversions and hot-cache hits, for example:

```rust
#[derive(Debug)]
pub enum ServerConversionOutcome {
    Ready(ServerConversionResult),
    ReviewRequired(ServerConversionResult),
    Blocked(ServerConversionResult),
}
```

`ServerConversionResult` may carry the same `PublicationDecision` internally for
callers that need the report details, but the outer outcome is what callers use
to pick the job terminal state and HTTP response path.

`build_result_from_existing()` must evaluate `is_scriptlet_public_ready()` and
return a refusal when false. This requires expanding the hot-cache lookup return
path beyond `Option<ServerConversionResult>`, for example:

```rust
pub enum HotCacheLookup {
    Hit(ServerConversionOutcome),
    Miss,
}
```

`cached_conversion_result_async()` should propagate that distinction so callers
do not reconvert a current private-review or blocked row in a loop and do not
return a ready manifest from hot cache.

Manual CCS uploads through admin package publishing are already CCS artifacts,
not native-package conversion results. They may keep the constructor default
`publication_status = "public"` only when the uploaded CCS manifest has no
`legacy_scriptlets` bundle. If the uploaded manifest has a bundle, the upload
path must project its summary into the existing scriptlet metadata fields and
apply the same publication gate. Admin upload must not become a bypass for
serving a CCS archive whose own manifest says it is private-review or blocked.

Implementation requirements for `apps/remi/src/server/handlers/admin/packages.rs`:

1. use the existing `InspectedPackage::from_file()` result and read
   `inspected.manifest.legacy_scriptlets`; the current
   `conary_core::ccs::inspector::InspectedPackage` exposes `pub manifest:
   CcsManifest`, so no archive re-reader is needed for this lookup;
2. if the bundle is present, validate it through the manifest/archive validation
   path before trusting its fields;
3. call the public bundle-summary conversion helper;
4. derive `package.platform.arch` when present, pass it through the
   architecture-aware converted-package identity lookup, and store it on
   `ConvertedPackage.package_architecture` so review-artifact lookup and
   replacement semantics stay aligned for uploaded multilib CCS artifacts;
5. call `ConvertedPackage::set_scriptlet_metadata()` before inserting the row;
6. if the summary is not public-ready, write the private review artifact under
   the configured review root and populate `review_artifact_path` before the DB
   transaction commits;
7. expand `atomic_replace_record()` to accept the optional architecture and the
   scriptlet summary after `review_artifact_path` has been populated, then set
   architecture and metadata inside the transaction before `insert()`;
8. if review artifact writing succeeds but the DB transaction fails, delete the
   newly written review artifact or leave it only as an unreachable temporary
   file under the review root; it must not be referenced by any committed row;
9. preserve the existing staged-file and atomic replacement behavior so the DB
   row never points at bytes that were not successfully staged.

## Jobs

Extend job state so conversion can finish without becoming public-ready:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobStatus {
    Pending,
    Converting,
    Ready,
    ReviewRequired,
    Blocked,
    Failed(String),
}
```

`ConversionJob.result` should still store
`apps/remi/src/server/jobs::ConversionResult` for terminal success-like
outcomes so operators can inspect metadata. This is the job-scoped result type,
not `conary_core::ccs::convert::ConversionResult`. Extend the job result with
sanitized scriptlet metadata and an optional publication report:

```rust
use crate::server::conversion::ScriptletPackageMetadata;
use crate::server::publication::PublicationGateReport;

#[derive(Debug, Clone)]
pub struct ConversionResult {
    pub chunk_hashes: Vec<String>,
    pub total_size: u64,
    pub content_hash: String,
    pub ccs_path: PathBuf,
    pub actual_version: String,
    pub scriptlets: ScriptletPackageMetadata,
    pub publication: Option<PublicationGateReport>,
}
```

The public job response must not include a downloadable manifest unless the
status is `ready`.

`GET /v1/jobs/:job_id` should return:

- `status = "ready"` and a manifest with sanitized scriptlet metadata only for
  public-ready results;
- `status = "review-required"` plus the publication report for private-review
  or local-only results;
- `status = "blocked"` plus the publication report for blocked results;
- existing failed behavior for hard conversion failures.

Job summary counts should treat `ReviewRequired` and `Blocked` as terminal
states, but not as ready. If summary fields are expanded, name them explicitly
instead of folding them into `failed`.

JobStatus migration checklist:

| File | Site | Required change |
| --- | --- | --- |
| `apps/remi/src/server/jobs.rs` | `evict_terminal_jobs_for_capacity()` | Treat `ReviewRequired` and `Blocked` as terminal evictable states. |
| `apps/remi/src/server/jobs.rs` | `update_status()` terminal check | Mark `completed_at` for `ReviewRequired` and `Blocked`. |
| `apps/remi/src/server/jobs.rs` | `complete_with_result()` | Choose `Ready`, `ReviewRequired`, or `Blocked` from the publication decision instead of always setting `Ready`. |
| `apps/remi/src/server/jobs.rs` | `stats()` | Count review and blocked terminal outcomes separately or as explicit non-ready terminal states. |
| `apps/remi/src/server/handlers/jobs.rs` | `get_job_status()` | Render `review-required` and `blocked`, include publication reports, and omit ready manifests. |
| `apps/remi/src/server/handlers/packages.rs` | `download_package()` job-status match | Refuse review/blocked job results and do not stream files for them. |
| `apps/remi/src/server/handlers/packages.rs` | `run_conversion()` / `complete_with_result()` caller | Populate the extended job result with scriptlet metadata and publication report. |

## Public Package And Download Endpoints

### Package Manifest

`GET /v1/:distro/packages/:name` currently returns a manifest when
`check_converted()` finds a current converted row. Goal 5 changes that lookup
to return a typed result:

```rust
enum ConvertedManifestLookup {
    Ready(PackageManifest),
    ReviewRequired(PublicationRefusal),
    Blocked(PublicationRefusal),
    Missing,
}
```

Rules:

- stale rows still behave as missing and may trigger reconversion;
- public-ready rows return the existing manifest shape with sanitized
  `scriptlets` metadata;
- review-required or blocked rows return structured refusal and must not start
  another conversion loop;
- non-public rows with missing CCS files should still be treated as stale or
  missing according to existing file-existence rules.

### Download

`download_package()` has two bypass risks:

1. a just-finished in-memory job result can stream directly;
2. a stored database row can stream through `converted_ccs_path_for_download()`.

Both paths must apply the same publication gate. Return structured refusal
instead of a file stream for review-required or blocked results.

`converted_ccs_path_for_download()` should return a typed lookup rather than
`Option<PathBuf>` so callers cannot ignore the refusal path.

### Raw Chunk And OCI Blob Streaming

Content-addressed chunk endpoints can also expose converted package bytes when a
client already knows a hash. Goal 5 must make those endpoints converted-row
reachability-aware without turning Remi's general CAS into a converted-package
only store:

- `/v1/chunks/{hash}` `GET` and `HEAD` may serve or acknowledge a local chunk
  when that hash is not referenced by any current converted row, or when it is
  reachable from at least one current public-ready converted row;
- `/v1/chunks/batch` must omit or mark unavailable any hash that is only
  reachable from stale or non-public rows;
- `/v1/chunks/find-missing` must not report non-public-only local chunks as
  available;
- OCI `GET`/`HEAD /v2/{name}/blobs/{digest}` must apply the same reachability
  check before streaming chunk bytes.

If a hash is shared by a public-ready row and a non-public row, serving is
allowed because the bytes are already publicly reachable through the public row.
If the hash is reachable only from non-public or stale rows, public endpoints
should return the same shape they use for missing chunks/blobs, preferably `404`,
so the response does not leak that a private artifact exists.

Chunks with no converted-package reference, or chunks explicitly protected by the
generic `chunk_access` cache bookkeeping for non-converted workflows, remain
servable according to the existing CAS rules. The gate is about preventing
non-public converted-package bytes from leaking, not about reclassifying every
CAS object as a package artifact.

The chunk/blob reachability helper must be cheap enough for hot public paths. It
should query only candidate rows whose `chunk_hashes_json` appears to contain the
requested hash and only select the minimal columns needed to evaluate the
health-aware public-ready predicate. A SQL text prefilter is only a narrowing
hint; the helper must still parse `chunk_hashes_json` and perform exact hash
matching in Rust before deciding that a converted row references the chunk.

This rule is stricter than simply filtering manifests and indexes: public
metadata must not advertise non-public hashes, and public blob/chunk handlers
must not serve non-public-only hashes even when a caller guessed or retained the
digest.

## Metadata And Generated Indexes

Goal 5 must remove "ready" implications from non-public converted rows across
public metadata surfaces.

### `/v1/:distro/metadata`

Repository-backed packages should remain visible because they exist upstream,
but a non-public converted row must not mark them as `converted: true`.

Required behavior:

- public-ready converted row: `converted: true`, include sanitized
  `metadata.scriptlets`;
- review-required or blocked converted row: `converted: false`, omit
  `metadata.scriptlets` from the public metadata entry and rely on package,
  job, and admin responses for the structured refusal details;
- converted-only row with no repository package:
  - public-ready: include it;
  - review-required or blocked: omit it from public metadata.

`converted_count` must count only public-ready converted rows.

### Generated Index

Generated public indexes are client discovery inputs. They must list only
public-ready converted versions as converted.

Required behavior:

- when a repository package has a non-public conversion row, emit the version as
  pending/unconverted rather than converted;
- do not add converted-only private-review or blocked rows to the generated
  public index;
- keep public-ready scriptlet metadata in the index;
- never include `review_artifact_path`.

### Package Detail, OCI, Sparse, Delta

Goal 4 hardened stale-row filtering. Goal 5 should add publication filtering to
any endpoint that presents converted rows as ready artifacts, including package
detail, OCI manifests, sparse metadata, and delta manifest generation when they
use `converted_packages`.

The rule is simple: stale or non-public rows are not ready artifact inputs.

Search and federated/sparse index builders that expose a `converted` boolean
must apply the same ready predicate. A non-public row may remain terminal in the
database, but public discovery surfaces must not count it as converted-ready.
OCI tag/catalog and delta-manifest queries must likewise exclude non-public
rows because they point clients toward retrievable converted artifacts.
Prewarm code may treat non-public rows as terminal to avoid an infinite
reconversion loop, but its reporting should distinguish terminal review/blocked
rows from public-ready conversions when practical.

Concrete query gates:

- `apps/remi/src/server/search.rs::SearchEngine::rebuild_from_db`: the
  `LEFT JOIN converted_packages cp` used to compute `is_converted` must include
  `cp.publication_status = 'public'` and must not mark rows converted when the
  summary parse-health helper rejects the row.
- `apps/remi/src/server/handlers/sparse.rs::build_sparse_entry`: the converted
  lookup query must filter to public rows and must not expose `content_hash`
  for non-public or malformed-summary rows.
- `apps/remi/src/server/federated_index.rs`: the local converted lookup used
  by federated sparse entries must apply the same public-ready predicate before
  setting `converted: true` or returning a content hash.
- `apps/remi/src/server/delta_manifests.rs::get_version_chunks`,
  `versions_have_current_conversions`, and `compute_deltas_for_package`: only
  public-ready rows may provide chunk lists, participate in version-pair
  eligibility, or appear in delta version enumeration.
- `apps/remi/src/server/handlers/oci.rs::build_manifest`,
  `build_tags_list`, and `build_catalog`: non-public rows must not build OCI
  manifests, appear as tags, or appear as repositories in the catalog.

Full current query-site inventory:

| File | Function or query | Required publication change |
| --- | --- | --- |
| `apps/remi/src/server/handlers/packages.rs` | `check_converted()` | Return `Ready`, `ReviewRequired`, `Blocked`, or `Missing`; do not treat non-public current rows as missing. |
| `apps/remi/src/server/handlers/packages.rs` | `converted_ccs_path_for_download()` | Return a typed ready/refusal/missing lookup; do not return a path for non-public rows. |
| `apps/remi/src/server/handlers/index.rs` | `load_converted_metadata_rows()` | Load only public-ready rows for public metadata, or mark non-public repo-backed rows unconverted and omit converted-only non-public rows. |
| `apps/remi/src/server/index_gen.rs` | `get_packages_for_distro()` | Filter non-public rows out of converted lookups and converted-only additions. |
| `apps/remi/src/server/delta_manifests.rs` | `get_version_chunks()` | Require public-ready rows before returning chunk lists. |
| `apps/remi/src/server/delta_manifests.rs` | `versions_have_current_conversions()` | Count only public-ready versions. |
| `apps/remi/src/server/delta_manifests.rs` | `compute_deltas_for_package()` | Enumerate only public-ready converted versions. |
| `apps/remi/src/server/federated_index.rs` | local sparse converted lookup | Do not set converted/content hash for non-public rows. |
| `apps/remi/src/server/handlers/sparse.rs` | `build_sparse_entry()` | Do not set converted/content hash for non-public rows. |
| `apps/remi/src/server/handlers/oci.rs` | `build_manifest()` | Reject non-public lookup matches before building a manifest. |
| `apps/remi/src/server/handlers/oci.rs` | `build_tags_list()` | List only public-ready versions. |
| `apps/remi/src/server/handlers/oci.rs` | `build_catalog()` | List only repositories with at least one public-ready version. |
| `apps/remi/src/server/handlers/oci.rs` | `get_blob_inner()` / `head_blob_inner()` | Serve or acknowledge only blobs reachable from a public-ready converted row. |
| `apps/remi/src/server/handlers/chunks.rs` | `get_chunk()` / `head_chunk()` | Serve or acknowledge only chunks reachable from a public-ready converted row. |
| `apps/remi/src/server/handlers/chunks.rs` | `find_missing()` / `batch_fetch()` | Treat non-public-only chunks as unavailable to public clients. |
| `apps/remi/src/server/handlers/detail.rs` | `query_package_detail()` | Count only public-ready converted rows. |
| `apps/remi/src/server/handlers/detail.rs` | `query_versions_internal()` | Mark only public-ready versions as converted. |
| `apps/remi/src/server/handlers/detail.rs` | `query_overview()` | Count only public-ready conversions in public overview stats. |
| `apps/remi/src/server/search.rs` | `SearchEngine::rebuild_from_db()` | Add publication-ready filtering to the `LEFT JOIN` before setting `is_converted`. |
| `apps/remi/src/server/prewarm.rs` | `is_already_converted()` | Treat current non-public rows as terminal to avoid loops, but distinguish them from public-ready rows in reporting when practical. |

For SQL queries, `publication_status = 'public'` is only a prefilter. Every
public `converted: true`, count, ready statistic, content hash, chunk list, OCI
manifest, delta, package manifest, chunk stream, or blob stream must be computed
from rows that pass a health-aware public-ready check. If a query cannot express
the full `scriptlet_summary_for_publication()` validity rule in SQL, fetch enough
candidate row data and filter/count in Rust, or add a focused DB helper that
returns only health-checked public-ready identities. Do not rely on scalar
`publication_status = 'public'` alone, even for counts and booleans.

## Database Model

Goal 5 should not add a migration. Use existing Goal 4 columns:

- `publication_status`
- `scriptlet_fidelity`
- `target_compatibility`
- `evidence_digest`
- `curation_evidence_digest`
- `blocked_reason_codes_json`
- `scriptlet_summary_json`
- `review_artifact_path`

Add helpers to `ConvertedPackage` if they reduce duplication:

```rust
pub struct ScriptletSummaryForPublication {
    pub summary: ScriptletBundleSummary,
    pub valid: bool,
}

impl ConvertedPackage {
    pub fn scriptlet_publication_status(&self) -> &str { ... }
    pub fn scriptlet_summary_for_publication(&self) -> ScriptletSummaryForPublication { ... }
    pub fn is_scriptlet_public_ready(&self) -> bool { ... }
}
```

Keep `ConvertedPackage::new()` and `new_server()` signatures stable. Existing
tests and admin upload paths should continue to compile without passing
scriptlet-specific arguments.

`scriptlet_summary()` can keep its current best-effort display behavior.
Publication gating must use the health-aware helper.

`scriptlet_summary_for_publication()` should parse `scriptlet_summary_json` twice
or otherwise preserve shape information: once as JSON `Value` to verify required
explicit keys or the narrow `{}` default shape, and once as
`ScriptletBundleSummary` to reuse the Goal 4 summary projection. The helper must
not call `scriptlet_summary()` and infer validity afterward, because
`scriptlet_summary()` intentionally hides malformed or partial JSON by recovering
display defaults.

If a future implementation needs to update only the review artifact path after
insert, prefer inserting the final path with the row in Goal 5. A separate DB
update helper is acceptable only if it remains scoped to the existing
`review_artifact_path` column.

## Reason Codes

Goal 5 does not mint new parser or adapter reason codes for package content.
It may introduce Remi serving reason codes for top-level gate outcomes:

- `publication-gate-review-required`
- `publication-gate-blocked`
- `publication-gate-local-only`
- `publication-gate-unknown-status`
- `publication-gate-malformed-summary`

These reason codes describe why Remi refused public serving. They do not replace
the underlying scriptlet reason codes from the bundle summary.

## Security And Privacy

Required invariants:

- public endpoints never expose `review_artifact_path`;
- public endpoints never stream non-public CCS artifacts;
- public chunk/blob endpoints never stream hashes that are reachable only from
  non-public or stale converted rows;
- admin review-artifact retrieval never accepts a raw filesystem path;
- review artifact paths are sanitized and verified under the private review
  root;
- blocked packages are structured negative conversion results, not generic 500
  failures;
- review-required packages are terminal conversion outcomes, not conversion
  failures;
- logs may include package identity and reason codes, but should avoid printing
  local artifact paths at info level.

## Testing Strategy

Unit tests:

- publication gate maps `public` to ready;
- publication gate maps `private-review` to review-required;
- publication gate maps `blocked` to blocked;
- publication gate maps `local-only`, unknown, and malformed values to
  review-required;
- `scriptlet_summary_for_publication()` treats malformed, empty, non-object, and
  partial summary JSON as invalid;
- `scriptlet_summary_for_publication()` treats `{}` as valid only for the narrow
  no-bundle/default row shape and rejects `{}` when any scriptlet scalar/list
  field indicates real scriptlet evidence;
- reason-code aggregation is deterministic, deduplicated, and ordered as
  blocked reason codes, review reason codes, unknown commands, then blocked
  classes;
- public `ScriptletBundleSummary::from_bundle()` preserves bundle status,
  decision counts, reason codes, unknown commands, and blocked classes;
- review artifact writer sanitizes path components and writes valid JSON;
- admin artifact path validation rejects paths outside the private review root.

Remi conversion tests:

- public/native-free or fully-replaced conversion completes with job status
  `ready`;
- private-review conversion completes with job status `review-required` and no
  ready manifest;
- blocked conversion completes with job status `blocked` and no ready manifest;
- hot-cache lookup returns ready for public rows and a refusal for
  private-review or blocked rows;
- review-required and blocked rows create private review artifacts and set only
  `review_artifact_available` in public metadata.

Package and download handler tests:

- public rows still return manifests and downloads;
- private-review rows return `409` structured refusal for manifest and
  download;
- blocked rows return `403` structured refusal for manifest and download;
- non-public rows do not trigger infinite reconversion loops;
- current non-public rows return refusals instead of reconversion jobs, while
  stale rows still trigger reconversion;
- stale rows still trigger existing reconversion behavior.

Metadata/index tests:

- `/v1/:distro/metadata` marks only public-ready rows as converted;
- converted-only non-public rows are omitted from public metadata;
- generated index omits converted-only non-public rows;
- repository-backed non-public rows appear as pending/unconverted;
- sparse and federated sparse entries report non-public rows as not converted;
- search indexing reports non-public rows as not converted;
- OCI manifests, tags, and catalog ignore non-public rows;
- OCI blob and raw chunk endpoints return missing for hashes reachable only from
  non-public rows and allow hashes shared with at least one public-ready row;
- delta-manifest chunk, eligibility, and version enumeration ignore non-public
  rows;
- `converted_count` excludes non-public rows;
- serialized public metadata and indexes never contain `review_artifact_path`
  or private path fragments.

Admin tests:

- admin review-artifact endpoint requires admin scope;
- admin endpoint returns artifact JSON for a current non-public row;
- admin endpoint returns `404` when no artifact exists;
- admin endpoint rejects stale rows;
- admin endpoint refuses artifact paths outside the private review root;
- admin CCS upload with no `legacy_scriptlets` bundle remains public by default;
- admin CCS upload with a private-review or blocked bundle stores scriptlet
  metadata, writes a private review artifact, and is not served as public-ready.

Verification commands:

```bash
cargo test -p remi jobs
cargo test -p remi package_publication
cargo test -p remi conversion
cargo test -p remi index
cargo test -p remi detail
cargo test -p remi sparse
cargo test -p remi federated_index
cargo test -p remi chunks
cargo test -p remi oci
cargo test -p remi delta_manifests
cargo test -p remi search
cargo test -p remi prewarm
cargo test -p remi admin
cargo test -p conary-core converted
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
git diff --check
```

## Non-Goals And Deferred Work

Deferred to Goal 6 or later:

- safe replay engine;
- compatibility checks that allow legacy replay on source-native targets;
- client-side install/update/remove enforcement;
- Remi publication promotion after operator curation;
- public serving of packages that still require raw native scriptlet replay;
- bundle sidecars or external evidence stores;
- authenticated UI workflows for editing curation notes;
- new database schema for curation history.

## Review Questions

Ask reviewers to focus on these points:

1. Is `summary_valid && publication_status == "public"` the right active allow
   predicate?
2. Should public metadata omit non-public scriptlet summaries entirely, leaving
   details to package/job and admin responses?
3. Are `409` for review-required and `403` for blocked the right public HTTP
   statuses?
4. Is the private review artifact sufficient curation evidence for Goal 5, or
   should explicit operator curation remain entirely deferred?
5. Does the design cover every path that can currently stream or advertise a
   converted CCS artifact as ready?
6. Does keeping `ConvertedPackage::new()` and `new_server()` signatures stable
   avoid unnecessary test churn?
7. Are admin review-artifact retrieval rules tight enough to avoid path leaks?
8. Are generated index rules conservative enough for existing clients?
9. Are `ReviewRequired` and `Blocked` job states preferable to encoding those
   outcomes as failed jobs?
10. Are any Goal 5 behaviors accidentally implementing replay, promotion, or
    install-time enforcement?
