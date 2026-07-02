---
last_updated: 2026-07-01
revision: 1
summary: Run your own Remi conversion server in about 30 minutes
---

# Self-Hosted Remi in 30 Minutes

## What you get

Remi is Conary's conversion proxy and package server: it indexes upstream
Fedora, Ubuntu, and Arch repositories, converts requested packages into CCS,
and serves the resulting metadata and chunks to Conary clients. Self-hosting
does not require a conary.io account and does not require federation; the
single Remi service is enough for a private test host.

## Requirements

- A Linux host with Rust 1.96+
- Disk sized to the `[storage] max_cache_size` you choose
- Outbound HTTPS access to distro mirrors
- Client hosts that satisfy
  [docs/guides/compatibility-checklist.md](compatibility-checklist.md) Tier 1

## 1. Build the binary

```bash
cargo build --release -p remi
sudo install -m 0755 target/release/remi /usr/local/bin/remi
```

## 2. Write /etc/conary/remi.toml

Create the storage directory and keep the admin API on localhost:

```bash
sudo install -d /conary
sudo install -d /etc/conary
sudo editor /etc/conary/remi.toml
```

Minimal single-host config:

```toml
[server]
bind = "0.0.0.0:8080"
admin_bind = "127.0.0.1:8081"
workers = 0
metrics = true
audit_log = true

[storage]
root = "/conary"
eviction_threshold = 0.90
eviction_min_age = "1h"
negative_cache_ttl = "15m"
max_cache_size = "50GB"

[upstream.fedora]
metalink = "https://mirrors.fedoraproject.org/metalink"
releases = ["44"]
arches = ["x86_64"]
metadata_refresh = "6h"
priority = 100

[upstream.arch]
base_url = "https://archive.archlinux.org"
releases = ["latest"]
arches = ["x86_64"]
metadata_refresh = "6h"
priority = 100

[upstream.ubuntu]
base_url = "http://archive.ubuntu.com/ubuntu"
releases = ["resolute"]
arches = ["amd64"]
metadata_refresh = "6h"
priority = 100

[conversion]
chunking = true
chunk_min = 16384
chunk_avg = 65536
chunk_max = 262144
strip_debug = false
max_concurrent = 4
```

Validate the config before enabling the service:

```bash
remi --config /etc/conary/remi.toml --validate
```

## 3. Install the systemd unit

The tracked unit runs `/usr/local/bin/remi --config /etc/conary/remi.toml`,
uses the `conary` user and group, writes only under `/conary`, and reads
`/etc/conary`.

```bash
id -u conary >/dev/null 2>&1 || sudo useradd --system --home /conary --shell /usr/sbin/nologin conary
sudo chown -R conary:conary /conary
sudo cp deploy/systemd/remi.service /etc/systemd/system/remi.service
sudo systemctl daemon-reload
sudo systemctl enable --now remi
```

## 4. Verify

Check the public health endpoint on the bind address:

```bash
curl -fsS http://127.0.0.1:8080/health
```

From a Conary checkout, run the health script against your endpoint:

```bash
bash scripts/remi-health.sh --smoke --endpoint http://127.0.0.1:8080
bash scripts/remi-health.sh --full --endpoint http://127.0.0.1:8080
```

Point a client at the new server and convert one package end-to-end:

```bash
conary repo add remi http://your-remi-host:8080
conary repo sync
conary install nginx --dry-run
```

## 5. Optional: S3/R2 chunk storage

`deploy/remi.toml.example` documents Cloudflare R2 chunk storage in the `[r2]`
section. Enable it only after the local path works:

```toml
[r2]
enabled = true
bucket = "conary-chunks"
prefix = "chunks/"
write_through = true
```

Provide credentials through environment variables, not the config file:

```bash
CONARY_R2_ACCESS_KEY=...
CONARY_R2_SECRET_KEY=...
```

See `deploy/CLOUDFLARE.md` for the CDN-backed origin setup.

## Verified run

Pending fresh-VM verification. Fill this with the date, host OS, elapsed time,
and any deviations folded back into the guide during Task 11 of the
2026-07-01 tester-loop plan.
