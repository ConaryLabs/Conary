# Remi

Remi is Conary's on-demand conversion and package-serving service. It converts
upstream RPM, DEB, and Arch packages into CCS artifacts, stores converted
content in the local content-addressed store, and can write chunks through to R2
when configured.

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
