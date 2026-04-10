# Cloudflare Setup for Conary Remi

This document covers the Cloudflare configuration for `remi.conary.io`, including DNS, R2, caching, and security.

## Prerequisites

- A Cloudflare account (Free plan works, Pro recommended for advanced WAF rules)
- The `remi.conary.io` domain managed by Cloudflare DNS
- A running Remi server (see `deploy/setup-remi.sh`)

## 1. DNS Setup

Create an A record pointing to your Hetzner server:

| Type | Name       | Content         | Proxy | TTL  |
|------|------------|-----------------|-------|------|
| A    | remi       | YOUR_SERVER_IP  | Yes   | Auto |

The orange cloud (proxy) must be enabled for caching and DDoS protection to work.

If using IPv6 (recommended on Hetzner):

| Type | Name       | Content         | Proxy | TTL  |
|------|------------|-----------------|-------|------|
| AAAA | remi       | YOUR_IPV6_ADDR  | Yes   | Auto |

If you are preserving `packages.conary.io` as a legacy alias, keep matching
proxied `packages` records as well and redirect or otherwise treat them as a
compatibility hostname rather than the canonical public endpoint.

### Admin and MCP proxying

When `remi.conary.io` is orange-cloud proxied, do not expect the public
hostname to expose arbitrary origin ports such as `:8082`.

- Keep the Remi admin origin listener on loopback or another non-public bind.
- Publish the authenticated MCP surface on the standard HTTPS hostname, for
  example `https://remi.conary.io/mcp`.
- Expose REST admin routes on the public hostname only if you explicitly proxy
  them; otherwise reach them through a direct origin URL or an SSH tunnel.

Minimal nginx example:

```nginx
location /mcp {
    proxy_pass http://127.0.0.1:8082/mcp;
    proxy_http_version 1.1;
    proxy_set_header Host $host;
    proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    proxy_set_header X-Forwarded-Proto $scheme;
}
```

## 2. SSL/TLS

Under SSL/TLS settings:

1. Set encryption mode to **Full (strict)**
2. Enable **Always Use HTTPS**
3. Set minimum TLS version to **TLS 1.2**
4. Enable **Opportunistic Encryption**

For the origin certificate (on the Remi server):

```bash
# Option A: Cloudflare Origin Certificate (recommended, 15-year validity)
# Generate at: SSL/TLS > Origin Server > Create Certificate
# Save to /etc/conary/ssl/origin.pem and /etc/conary/ssl/origin-key.pem

# Option B: Let's Encrypt (if you need direct access without Cloudflare)
certbot certonly --standalone -d remi.conary.io
```

## 3. R2 Bucket Setup

### Create the bucket

```bash
# Install wrangler if not already available
npm install -g wrangler

# Authenticate
wrangler login

# Create the bucket
wrangler r2 bucket create conary-chunks
```

### Generate R2 API token

1. Go to: My Profile > API Tokens > Create Token
2. Select "Create Custom Token"
3. Permissions: **Account > R2 Storage > Edit**
4. Restrict to the `conary-chunks` bucket if possible
5. Save the Access Key ID and Secret Access Key

### Configure Remi server

Add the credentials as environment variables (not in the config file):

```bash
# /etc/conary/r2-credentials (mode 0400, owned by root)
CONARY_R2_ACCESS_KEY=your-access-key-id
CONARY_R2_SECRET_KEY=your-secret-access-key
```

For systemd, add to the service override:

```bash
systemctl edit remi
```

```ini
[Service]
EnvironmentFile=/etc/conary/r2-credentials
```

If you are not using systemd, use the equivalent secret-file or environment-file
mechanism for your service manager instead of putting credentials directly in
`remi.toml`.

Update `remi.toml`:

```toml
[r2]
enabled = true
account_id = "your-cloudflare-account-id"
endpoint = "https://your-account-id.r2.cloudflarestorage.com"
bucket = "conary-chunks"
prefix = "chunks/"
write_through = true
```

### R2 Custom Domain (optional)

To serve chunks directly from R2 via a custom domain:

1. Go to R2 > conary-chunks > Settings > Public Access
2. Add custom domain: `chunks.conary.io`
3. Add a CNAME record: `chunks` -> R2 bucket URL

This bypasses the origin server for chunk reads, reducing server load.

## 4. Cache Rules

Configure cache rules under **Caching > Cache Rules**. Create rules in this priority order:

### Rule 1: Immutable Chunks (highest priority)

- **When**: URI path starts with `/v1/chunks/`
- **Cache eligibility**: Eligible for cache
- **Edge TTL**: Override - 365 days
- **Browser TTL**: Override - 365 days
- **Cache Key**: Default (URI path is the chunk hash, so it is naturally unique)
- **Respect Origin**: No

Chunks are content-addressed (the URL contains the hash), so they are immutable and safe to cache indefinitely.

### Rule 2: Hashed Web Assets

- **When**: URI path starts with `/_app/`
- **Cache eligibility**: Eligible for cache
- **Edge TTL**: Override - 365 days
- **Browser TTL**: Override - 365 days

SvelteKit hashed assets (`/_app/immutable/...`) are safe to cache forever.

### Rule 3: Package Metadata

- **When**: URI path starts with `/v1/packages/`
- **Cache eligibility**: Eligible for cache
- **Edge TTL**: Override - 300 seconds (5 minutes)
- **Browser TTL**: Override - 60 seconds

### Rule 4: Package Index

- **When**: URI path starts with `/v1/index/`
- **Cache eligibility**: Eligible for cache
- **Edge TTL**: Override - 60 seconds
- **Browser TTL**: Override - 30 seconds

The index updates when new packages are converted, so keep TTL short.

### Rule 5: Search Results

- **When**: URI path starts with `/v1/search/`
- **Cache eligibility**: Eligible for cache
- **Edge TTL**: Override - 30 seconds
- **Browser TTL**: Override - 15 seconds

### Rule 6: No Cache for Admin/MCP/Mutations

- **When**: URI path starts with `/v1/admin/` OR URI path equals `/mcp` OR HTTP method is POST
- **Cache eligibility**: Bypass cache

## 5. Transform Rules

Under **Rules > Transform Rules > Modify Response Headers**, add:

### Add security headers

- **When**: All incoming requests
- **Set headers**:
  - `X-Content-Type-Options`: `nosniff`
  - `X-Frame-Options`: `DENY`
  - `Referrer-Policy`: `strict-origin-when-cross-origin`

### Add CORS for chunk endpoints

- **When**: URI path starts with `/v1/chunks/`
- **Set headers**:
  - `Access-Control-Allow-Origin`: `*`
  - `Access-Control-Allow-Methods`: `GET, HEAD`
  - `Access-Control-Max-Age`: `86400`

Chunks are public, content-addressed data -- open CORS is safe.

## 6. Security Settings

### WAF (Web Application Firewall)

Under **Security > WAF**:

1. Enable **Managed Rules** (Cloudflare Managed Ruleset)
2. Set the OWASP Core Ruleset sensitivity to **Medium**
3. Create a custom rule to skip WAF for chunk downloads:
   - **When**: URI path starts with `/v1/chunks/`
   - **Action**: Skip all remaining WAF rules
   - This prevents false positives on binary chunk data

### DDoS Protection

Under **Security > DDoS**:

1. Leave L7 DDoS protection at default (auto)
2. Consider adding a rate-limiting rule:
   - **When**: URI path starts with `/v1/`
   - **Rate**: 1000 requests per 10 seconds per IP
   - **Action**: Challenge

### Bot Management (Pro plan)

If on Pro plan or higher:

1. Enable **Bot Fight Mode**
2. Create a custom rule for the API:
   - **When**: URI path starts with `/v1/` AND bot score < 30
   - **Action**: Challenge
3. Exempt known package manager user agents with a Skip rule:
   - **When**: User agent contains `conary/`
   - **Action**: Skip

### IP Access Rules

Block known bad ranges if needed:

```
Security > WAF > Tools > IP Access Rules
```

The Remi server also maintains its own ban list (configured via `ban_threshold` and `ban_duration` in `remi.toml`).

## 7. Page Rules (Legacy, optional)

If using Page Rules instead of Cache Rules:

| URL Pattern                          | Setting              | Value        |
|--------------------------------------|----------------------|--------------|
| `remi.conary.io/v1/chunks/*`    | Cache Level          | Cache Everything |
|                                      | Edge Cache TTL       | 1 month      |
|                                      | Browser Cache TTL    | 1 year       |
| `remi.conary.io/v1/index/*`     | Cache Level          | Cache Everything |
|                                      | Edge Cache TTL       | 1 minute     |
| `remi.conary.io/v1/admin/*`     | Cache Level          | Bypass       |

Note: Page Rules are being deprecated in favor of Cache Rules. Use Cache Rules (Section 4) for new deployments.

## 8. Origin Configuration

Ensure the Remi server is configured to trust Cloudflare's connecting IP header:

```toml
# remi.toml
[security]
trusted_proxy_header = "CF-Connecting-IP"
```

Optionally validate that requests actually come from Cloudflare IPs:

```bash
# Download Cloudflare IP ranges
curl -o /etc/conary/cloudflare-ips.txt https://www.cloudflare.com/ips-v4
curl https://www.cloudflare.com/ips-v6 >> /etc/conary/cloudflare-ips.txt
```

```toml
[security]
cloudflare_ips_file = "/etc/conary/cloudflare-ips.txt"
```

## 9. Monitoring

### Cloudflare Analytics

- Check **Analytics > Traffic** for request volume and cache hit ratios
- Target: >95% cache hit rate for `/v1/chunks/` after warm-up period
- Monitor **Security > Overview** for blocked threats

### Origin Health

- Set up a **Health Check** (under Traffic > Health Checks):
  - URL: `https://remi.conary.io/health`
  - Interval: 60 seconds
  - Expected status: 200
  - Expected body contains: `OK`

### Notifications

Configure alerts under **Notifications**:

- Origin health check failure
- DDoS attack detected
- SSL certificate expiring
- R2 storage usage threshold

## Quick Verification Checklist

After completing setup, verify:

1. `curl -fsS https://remi.conary.io/health` returns `OK`
2. `curl -I https://remi.conary.io/v1/chunks/HASH` returns `cf-cache-status: HIT` on second request
3. `curl -I https://remi.conary.io/v1/index/fedora/43/x86_64` returns short-TTL cache headers
4. R2 dashboard shows objects being written (if write-through is enabled)
5. WAF dashboard shows no false positives on legitimate traffic
