# Remi

Remi is Conary's on-demand conversion and package-serving service. For the
limited public preview, its supported public source targets are Fedora 44,
Ubuntu 26.04, and Arch. It converts upstream RPM, DEB, and Arch packages into
CCS artifacts, stores converted content in the local content-addressed store,
and can write chunks through to R2 when configured.

## Release Uploads

Remi release push is the first native CCS publication intake surface. The
route remains `POST /v1/admin/releases/{distro}` with bearer-token admin auth,
but accepted CCS v2 uploads are stored in `native_package_publications` and
projected into `repository_packages`; they are not synthetic
`converted_packages` rows. Native uploads stage privately, run the shared static
publish gate against `release_publish.trusted_build_attestation_signers`, and
publish package rows, native rows, chunks, and TUF targets only after the gate
and metadata commit pass.

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
- `conversion/lookup.rs`: repository package selection, supported distro
  mapping, upstream download, and one-shot metadata refresh after upstream
  404s.
- `conversion/metadata.rs`: safe CCS filenames, package parsing, metadata
  construction, repository identity application, and repository-provide merging.
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
