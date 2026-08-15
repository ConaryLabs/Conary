---
last_updated: 2026-08-15
revision: 5
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

- A Linux host with Rust 1.97.1+
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
repository_manifest = "/etc/conary/remi-repositories.toml"

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

[admin]
enabled = true
external_bind = "127.0.0.1:8082"

[conversion]
strip_debug = false
max_concurrent = 4

[release_publish]
repository_keys_dir = "/conary/repository-keys"
```

Install the typed source manifest. Each source declares its package-manager
grammar and all parser construction data; Remi does not derive any of these
values from a display name or URL.

```bash
sudo cp deploy/remi-repositories.toml /etc/conary/remi-repositories.toml
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
sudo install -d -m 0700 -o conary -g conary /conary/repository-keys
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
44 example is the path verified for this guide. The trust contract keeps the
two RPM authorities separate: Fedora's metalink authenticates the exact
`repomd.xml`, while the official Fedora OpenPGP keyring authenticates each RPM
package.

```bash
ADMIN_TOKEN="$(sudo sed -n 's/^REMI_ADMIN_TOKEN=//p' /etc/conary/remi.env)"
curl -fsS -X POST http://127.0.0.1:8082/v1/admin/repos \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"name":"fedora","url":"https://download.fedoraproject.org/pub/fedora/linux/releases/44/Everything/x86_64/os","enabled":true,"priority":100,"metadata_expire":3600,"parser":{"package_format":"rpm","architecture":"x86_64"},"trust":{"ecosystem":"rpm","metadata":{"kind":"metalink","url":"https://mirrors.fedoraproject.org/metalink?repo=fedora-44&arch=x86_64"},"package_keys":[{"url":"https://fedoraproject.org/fedora.gpg","fingerprint":"36F612DCF27F7D1A48A835E4DBFCF71C6D9F90A6"}]}}'

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
new server and run a dry-run install through the same seeded repository.
First copy the profile's public package-authority key over an independently
authenticated administrative channel. For example, after verifying the SSH
host identity:

```bash
ssh root@your-remi-host \
    'cat /conary/repository-keys/fedora-44/targets.public' \
    > ./remi-fedora-44-targets.public

conary repo add remi http://your-remi-host:8080 \
    --package-format json \
    --default-strategy remi \
    --remi-endpoint http://your-remi-host:8080 \
    --ccs-package-key ./remi-fedora-44-targets.public \
    --source-profile fedora-44
conary repo sync
conary install curl --dry-run
```

Do not download `targets.public` from the same unauthenticated Remi endpoint
you are trying to trust. `repo add` validates the key and commits it in the
same transaction as the repository row; a malformed, duplicate, or missing
key leaves no partial repository.

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

Before relying on R2 for durable retrieval, inventory it against local CAS and
the persisted package-object authority. The authenticated admin operation
defaults to a read-only plan:

```bash
curl --fail-with-body \
  -H "Authorization: Bearer $REMI_ADMIN_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"mode":"plan"}' \
  "$REMI_ADMIN_ENDPOINT/v1/admin/r2-durability"
```

Review `planned_uploads`, `planned_upload_bytes`, `unrepairable_samples`, and
`missing_from_both_samples` before applying. Unrepairable samples distinguish
missing-from-both objects from local or R2 size contradictions. Apply is
explicit and concurrency is bounded between 1 and 64:

```bash
curl --fail-with-body \
  -H "Authorization: Bearer $REMI_ADMIN_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"mode":"apply","concurrency":16}' \
  "$REMI_ADMIN_ENDPOINT/v1/admin/r2-durability"
```

Archive the schema-v1 response. Only `outcome: "applied_complete"` together
with `r2_complete: true` proves the required set was present in the fresh R2
listing after upload. This command does not itself enable R2 redirects or local
eviction; those remain separate serving-policy changes.

Host-local automation can submit the same request to
`http://127.0.0.1:8081/v1/admin/r2-durability` without copying an external
admin token onto the host. That route is owned by the loopback-only internal
admin listener; do not proxy or bind it to a non-loopback address. The Conary
production workflow `.github/workflows/remi-r2-durability.yml` uses this path
only for an exact commit already merged into `main` and deployed through the
protected production environment. Its retained artifact removes diagnostic
samples and contains aggregate, public-sanitized evidence only.

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
