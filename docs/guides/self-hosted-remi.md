---
last_updated: 2026-07-02
revision: 2
summary: Run your own Remi conversion server in about 30 minutes
---

# Self-Hosted Remi in 30 Minutes

## What you get

Remi is Conary's conversion proxy and package server: it can index upstream
Fedora, Ubuntu, and Arch repositories, convert requested packages into CCS,
and serve the resulting metadata and chunks to Conary clients. Self-hosting
does not require a conary.io account and does not require federation; the
single Remi service is enough for a private test host.

## Requirements

- A Linux host with Rust 1.96+
- At least 8 GiB RAM for the default release build
- Disk sized to the `[storage] max_cache_size` you choose
- Outbound HTTPS access to distro mirrors
- For the final client probe, a separate host or VM with the Conary CLI
  installed that satisfies
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
base_url = "https://archive.ubuntu.com/ubuntu"
releases = ["resolute"]
arches = ["amd64"]
metadata_refresh = "6h"
priority = 100

[admin]
enabled = true
external_bind = "127.0.0.1:8082"

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
`/etc/conary`. The external admin API is loopback-only in the config above;
load its bootstrap token from a root-readable systemd environment file.

```bash
id -u conary >/dev/null 2>&1 || sudo useradd --system --home /conary --shell /usr/sbin/nologin conary
sudo chown -R conary:conary /conary
sudo cp deploy/systemd/remi.service /etc/systemd/system/remi.service
sudo install -m 0600 /dev/null /etc/conary/remi.env
TOKEN="$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n')"
printf 'REMI_ADMIN_TOKEN=%s\n' "$TOKEN" | sudo tee /etc/conary/remi.env >/dev/null
unset TOKEN
sudo install -d /etc/systemd/system/remi.service.d
sudo tee /etc/systemd/system/remi.service.d/10-admin-token.conf >/dev/null <<'EOF'
[Service]
EnvironmentFile=/etc/conary/remi.env
EOF
sudo systemctl daemon-reload
sudo systemctl enable --now remi
```

## 4. Verify the service

Check the public health endpoint on the bind address:

```bash
curl -fsS http://127.0.0.1:8080/health
```

From a Conary checkout, run the health script against your endpoint:

```bash
bash scripts/remi-health.sh --smoke --endpoint http://127.0.0.1:8080
```

The `--full` health mode is intended for a production Remi with repository
metadata and converted package indexes already populated. For a new self-host,
seed one runtime repository through the admin API before conversion testing.
Use a repository that matches the Tier 1 client you plan to test; this Fedora
44 example is the path verified for this guide. It disables package metadata
GPG checking for the first smoke conversion because the admin create route does
not import a distro key; wire signature policy before using a repo for trusted
package intake.

```bash
ADMIN_TOKEN="$(sudo sed -n 's/^REMI_ADMIN_TOKEN=//p' /etc/conary/remi.env)"
curl -fsS -X POST http://127.0.0.1:8082/v1/admin/repos \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"name":"fedora","url":"https://download.fedoraproject.org/pub/fedora/linux/releases/44/Everything/x86_64/os","enabled":true,"priority":100,"gpg_check":false,"metadata_expire":3600}'

curl -fsS -X POST 'http://127.0.0.1:8082/v1/admin/repos/fedora/sync?force=true' \
  -H "Authorization: Bearer $ADMIN_TOKEN"
unset ADMIN_TOKEN
```

Trigger and poll one conversion through the public API:

```bash
status=
for _ in $(seq 1 30); do
  status="$(curl -sS -o /tmp/remi-curl.json -w '%{http_code}' \
    http://127.0.0.1:8080/v1/fedora/packages/curl || true)"
  [ "$status" = 200 ] && break
  [ "$status" = 202 ] || { cat /tmp/remi-curl.json; exit 1; }
  sleep 2
done
test "$status" = 200
```

From a Fedora 44 client with the Conary CLI installed, point the client at the
new server and run a dry-run install through the same seeded repository:

```bash
conary repo add remi http://your-remi-host:8080
conary repo sync
conary install curl --dry-run
```

For an Ubuntu 26.04 LTS or Arch Linux client, create and sync the corresponding
runtime repository instead of the Fedora example before running the client
probe.

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

Verified on 2026-07-02 in a QEMU/KVM Ubuntu 24.04.4 LTS server VM on Remi with
Rust 1.96.1 and 8 GiB RAM. The release build completed in 9m 37s, config
validation passed, systemd started `remi`, and
`bash scripts/remi-health.sh --smoke --endpoint http://127.0.0.1:8080` passed
5/5 checks. Enabling `[admin]`, loading `REMI_ADMIN_TOKEN` from a systemd
environment file, creating the Fedora 44 runtime repo, and forcing
`/v1/admin/repos/fedora/sync?force=true` synced 76,354 packages in 27s; a
follow-up `GET /v1/fedora/packages/curl` returned `200` with converted package
metadata after the initial `202 Accepted` conversion trigger.

The first 4 GiB VM run failed during the optimized Rust build when `rustc` was
killed, so the guide now requires at least 8 GiB RAM for the default release
build. The server-host VM used Ubuntu 24.04.4 LTS; client probes remain limited
to the Tier 1 Conary client set in `docs/guides/compatibility-checklist.md`.
