# Remi

Remi is Conary's on-demand conversion and package-serving service. For the
limited public preview, its supported public source targets are Fedora 44,
Ubuntu 26.04, and Arch. It converts upstream RPM, DEB, and Arch packages into
CCS artifacts, stores converted content in the local content-addressed store,
and can write chunks through to R2 when configured.

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
