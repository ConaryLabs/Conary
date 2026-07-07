# Remi

Remi is Conary's on-demand conversion and package-serving service. For the
limited public preview, its supported public source targets are Fedora 44,
Ubuntu 26.04, and Arch. It converts upstream RPM, DEB, and Arch packages into
CCS artifacts, stores converted content in the local content-addressed store,
and can write chunks through to R2 when configured.

M4d routes every `{distro}` path parameter through supported profile route
validation before DB queries, cache/key filesystem paths, or release-upload
trust gates. Public Remi route slugs remain `fedora`, `ubuntu`, and `arch`;
they are backed by profile route metadata rather than a local hard-coded
`SUPPORTED_DISTROS` list.

## Release Uploads

Remi release push is the first native CCS publication intake surface. The
route remains `POST /v1/admin/releases/{distro}` with bearer-token admin auth,
but accepted CCS v2 uploads are stored in `native_package_publications` and
projected into `repository_packages`; they are not synthetic
`converted_packages` rows. Native uploads stage privately, run the shared static
publish gate against `release_publish.trusted_build_attestation_signers`.
After structural parsing, signature/trust verification, and the shared static
publish gate pass, Remi derives the supported target profile from the release
route slug and validates any signed CCS v2 lifecycle authority against that
profile. Unsupported route slugs fail before storage or artifact verification;
supported routes with unsupported lifecycle entries fail with
`LIFECYCLE_UNSUPPORTED` before public rows, chunks, or TUF targets are written.
Package rows, native rows, chunks, and TUF targets are published only after the
gate, route-derived lifecycle validation, and metadata commit pass.

The route/staging wrapper lives in `apps/remi/src/server/release_publish.rs`.
Native CCS verification, artifact promotion, metadata persistence, supersede
behavior, and public native lookup live under
`apps/remi/src/server/native_publish/`. Failed authorization, metadata, or TUF
commits must leave the previous public native generation intact and must not
write a new public package row, chunk object, `converted_packages` row, or TUF
target for the rejected upload.

Public metadata and download lookups are release-aware for native rows:
clients should request `version`, `release`, and `arch` when selecting a native
package. If a version-only request matches multiple native releases, Remi
returns a conflict with the available releases instead of guessing.

## Passive Scriptlet Metadata

Goal 4 conversions embed a passive `legacy_scriptlets` bundle in the generated
CCS manifest and store aggregate scriptlet metadata on `converted_packages`.
Those database fields record fidelity, target compatibility, publication
status, evidence digests, blocked/review reason codes, and sanitized summary
counts for converted artifacts.

Public package detail, metadata, and generated-index responses expose public
rows through a sanitized `scriptlets` object. Local `review_artifact_path`
values remain private server state and are represented publicly only as
`review_artifact_available`.

### Scriptlet Evidence Queue

Remi maintains an admin/operator-only scriptlet evidence queue for adapter
planning. Schema v75 adds `scriptlet_evidence_*` tables that cluster blocked,
review-required, and malformed conversion evidence by stable command shape,
blocked class, distro, target profile, and lifecycle phase. Conary-core owns the
database schema and model helpers; Remi owns the normalization, aggregation,
backfill, admin routes, and packet export modules under
`apps/remi/src/server/scriptlet_evidence_queue/` and
`apps/remi/src/server/handlers/admin/scriptlet_evidence.rs`.

Incremental conversion persistence records queue samples best-effort after a
converted row is stored. Existing rows can be materialized with
`POST /v1/admin/scriptlet-evidence/backfill`, which processes a bounded batch
without startup backfill. Admins can list clusters, inspect detail, update
triage state, add private maintainer notes, and export private or
`public-sanitized` packets through the `/v1/admin/scriptlet-evidence/*` route
family. Queue samples include sanitized generic LSM `security_policy_intents`
when conversion can type SELinux or AppArmor helper behavior, and the queue
uses those records for adapter planning and public/private packet export.
Detail responses expose review-artifact availability and staleness, never raw
local review artifact paths. Private packets include sanitized artifact
references for maintainer follow-up; `public-sanitized` packets omit review
artifacts and private notes.

The queue is not publication authority. Moving a cluster to
`covered-public-ready` records maintainer workflow state only; it does not
rewrite `converted_packages.publication_status`, make blocked rows public, call
LLM services, create a public issue tracker, or otherwise make a package
public.

### Legacy Scriptlet Publication Gate

Remi treats legacy scriptlet metadata embedded during conversion as an active
serving gate. Converted rows whose scriptlet summary is valid and has
`publication_status = "public"` may be advertised, indexed, and served only
when the core conversion outcome is public-ready: native-free or fully replaced
by adapter/support-matrix evidence. Rows with `private-review`, `blocked`,
`local-only`, malformed summary JSON, or non-default scriptlet evidence without
an explicit summary are terminal review/blocked conversion outcomes and are not
public-ready.

This gate is publication-only. It does not replay scriptlets, promote reviewed
packages, or change client install/update/remove behavior.

Sparse-index and search responses use `converted=true` only for rows that do
not need reconversion and pass the same public-ready scriptlet gate. A completed
conversion row that requires legacy replay, review, or blocking remains private
server state and is not advertised as a normal converted artifact.

### Fixture Ownership

The first Remi fixture ownership map lives in `docs/modules/test-fixtures.md`.
Start there before changing scriptlet publication gates, converted package
public-ready filtering, public index metadata, review artifacts, static test
fixture uploads, or `conary-test` manifest behavior.

Fast proof for native release-publication edits:

```bash
cargo test -p remi release_upload_
cargo test -p conary --test packaging_m4c
```

Fast proof for converted publication-gate edits:

```bash
cargo test -p remi publication
```

Medium proof when public serving, conversion state, or generated metadata
changes:

```bash
cargo test -p remi
```

## Conversion Service Ownership

The conversion service now keeps `apps/remi/src/server/conversion.rs` as the
stable public hub for `ConversionService` and conversion result DTO re-exports.
Implementation ownership lives in child modules:

- `conversion/workflow.rs`: cold/hot package conversion orchestration and
  timing.
- `conversion/types.rs`: public conversion result DTOs, scriptlet package
  metadata projection, and conversion benchmark evidence records.
- `conversion/benchmark.rs`: benchmark sampling, scan-only scriptlet evidence,
  and benchmark conversion wrappers.
- `conversion/lookup.rs`: repository package selection, profile-backed
  repository hints and version scheme, upstream download, and one-shot metadata
  refresh after upstream 404s.
- `conversion/metadata.rs`: safe CCS filenames, profile-backed parser dispatch,
  metadata construction, repository identity application, and
  repository-provide merging.
- `conversion/safety.rs`: critical package and runtime capability refusal
  guards.
- `conversion/storage.rs`: local CAS writes, optional R2 write-through, and
  checksum helpers.
- `conversion/persistence.rs`: converted-package rows, cache-hit
  reconstruction, review artifact persistence, and publication outcome
  wrapping.
- `conversion/recipe.rs`: recipe URL fetch, DNS/IP validation, SSRF refusal,
  and server-side recipe builds.
- `conversion/test_support.rs`: conversion-owned test DB, repository package,
  conversion result, and scriptlet summary builders shared by child-module
  tests.

For conversion behavior changes, start with the owner module and run the
focused module tests plus `cargo test -p remi --lib conversion`. For public
listing, review artifact, or scriptlet-publication behavior changes, also run
`cargo test -p remi publication`.

## Conversion Benchmark Evidence

Remi includes a local benchmark command for measuring cold-path conversion cost
before making public latency claims:

```bash
cargo run -p remi -- conversion-benchmark \
  --db /var/lib/conary/conary.db \
  --chunk-dir /var/lib/conary/data/chunks \
  --cache-dir /var/lib/conary/data/cache \
  --distro fedora \
  --package nginx \
  --jsonl
```

When R2 flags are omitted, benchmark JSON records `r2_write_through` as skipped.
To measure cloud write-through, pass `--r2-endpoint`, `--r2-bucket`,
`--r2-prefix`, and `--r2-region` with `CONARY_R2_ACCESS_KEY` and
`CONARY_R2_SECRET_KEY` set in the environment.

Use `--scan-only` to parse package metadata and summarize scriptlet helper
commands without writing converted CCS packages:

```bash
cargo run -p remi -- conversion-benchmark \
  --db /var/lib/conary/conary.db \
  --chunk-dir /var/lib/conary/data/chunks \
  --cache-dir /var/lib/conary/data/cache \
  --distro fedora \
  --max-packages 25 \
  --scan-only \
  --jsonl
```

The scriptlet corpus summary is evidence for adapter planning only. It is not
the authority for declaring a scriptlet `replaced`; that authority belongs to
the legacy scriptlet semantics bundle decision model.

Running without `--scan-only` performs real conversions and writes CCS/CAS cache
artifacts under the supplied cache and chunk directories. Use scratch paths for
local experiments unless you intentionally want to warm a real Remi cache.
