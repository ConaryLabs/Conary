# Networking Tools Recipes

These recipes build essential networking utilities for downloading
files and making HTTP/HTTPS requests.

## Package List

| Package | Version | Description |
|---------|---------|-------------|
| curl | 8.18.0 | Command line data transfer tool and library |
| wget2 | 2.2.1 | GNU network file retriever |
| ca-certificates | 2026.01 | Mozilla CA certificate bundle |

## Build Order

ca-certificates must be built first:

```
OpenSSL (from libs/, must be complete)
         │
         ▼
┌─────────────────────┐
│  ca-certificates    │  (build first - needed by curl/wget)
└─────────────────────┘
         │
    ┌────┴────┐
    ▼         ▼
┌──────┐  ┌───────┐
│ curl │  │ wget2 │  (can build in parallel)
└──────┘  └───────┘
```

## Building Networking Tools

```bash
# Build all networking tools (orchestrated)
conary bootstrap net

# Or build individual tools
conary cook recipes/core/net/ca-certificates.toml
conary cook recipes/core/net/curl.toml
conary cook recipes/core/net/wget2.toml
```

## Tool Descriptions

### curl
The Swiss Army knife of data transfer.
- Supports 25+ protocols (HTTP, HTTPS, FTP, SFTP, etc.)
- Provides libcurl library for applications
- Highly configurable with many options
- Used by countless applications and scripts

### wget2
Modern replacement for classic GNU wget.
- Parallel downloads for faster retrieval
- HTTP/2 support
- Better performance than wget 1.x
- Creates `wget` symlink for compatibility

### ca-certificates
Mozilla's trusted root certificates.
- Installed to `/etc/ssl/certs/ca-certificates.crt`
- Hash symlinks for OpenSSL compatibility
- Required for HTTPS verification
- Updated from Mozilla's certificate program

## Certificate Locations

After installing ca-certificates:

```
/etc/ssl/certs/ca-certificates.crt    # Main bundle
/etc/ssl/cert.pem                      # Symlink to bundle
/etc/pki/tls/certs/ca-bundle.crt       # RHEL-style symlink
/etc/ssl/certs/*.pem                   # Individual certs
/etc/ssl/certs/*.0                     # OpenSSL hash symlinks
```

## Common Operations

### curl Examples

```bash
# Simple GET request
curl https://example.com

# Save to file
curl -o file.txt https://example.com/file.txt

# Follow redirects
curl -L https://example.com/redirect

# POST data
curl -X POST -d "key=value" https://api.example.com

# With headers
curl -H "Authorization: Bearer token" https://api.example.com

# Download with progress bar
curl -# -O https://example.com/large-file.tar.gz

# Resume interrupted download
curl -C - -O https://example.com/large-file.tar.gz

# Check SSL certificate
curl -vI https://example.com 2>&1 | grep -A5 "Server certificate"
```

### wget Examples

```bash
# Simple download
wget https://example.com/file.txt

# Save with different name
wget -O output.txt https://example.com/file.txt

# Download entire website (mirror)
wget -m -p -E -k https://example.com

# Continue interrupted download
wget -c https://example.com/large-file.tar.gz

# Download in background
wget -b https://example.com/large-file.tar.gz

# Limit download speed
wget --limit-rate=1m https://example.com/large-file.tar.gz

# Download from file list
wget -i urls.txt
```

### Checking Certificates

```bash
# List certificates in bundle
awk '/BEGIN CERTIFICATE/,/END CERTIFICATE/{if(/BEGIN/)n++}END{print n}' \
    /etc/ssl/certs/ca-certificates.crt

# View certificate details
openssl x509 -in /etc/ssl/certs/ca-certificates.crt -text -noout | head -20

# Test HTTPS connection
openssl s_client -connect example.com:443 -CAfile /etc/ssl/certs/ca-certificates.crt
```

## Configuration Files

### /etc/wgetrc
Global wget configuration:
```
ca_certificate = /etc/ssl/certs/ca-certificates.crt
timeout = 60
tries = 3
```

### ~/.curlrc
Per-user curl configuration (optional):
```
# Use certificate bundle
cacert = /etc/ssl/certs/ca-certificates.crt

# Follow redirects
location

# Show error on HTTP errors
fail
```

## Verification

After building, verify tools work:

```bash
# Check curl
curl --version
curl -I https://www.google.com

# Check wget
wget --version
wget -q -O /dev/null https://www.google.com && echo "wget works"

# Verify certificates
curl -s https://curl.se/ca/cacert.pem | head -1
```

## Troubleshooting

### Certificate Errors
If you get certificate verification errors:

1. Ensure ca-certificates is installed
2. Check certificate bundle exists: `ls -la /etc/ssl/certs/ca-certificates.crt`
3. Verify OpenSSL can find certs: `openssl version -d`
4. Update ca-certificates if outdated

### Connection Issues
```bash
# Test with verbose output
curl -v https://example.com

# Skip certificate verification (NOT recommended for production)
curl -k https://example.com
wget --no-check-certificate https://example.com
```

## Integration Notes

### For Package Building
Both curl and wget are commonly used in recipe source downloads:
- curl is often preferred for its flexibility
- wget is simpler for basic downloads
- Both require ca-certificates for HTTPS

### For System Services
- systemd-networkd doesn't need these (uses systemd-resolved)
- Many services use libcurl for HTTP operations

## Next Steps

After building networking tools, consider:
- Git for version control
- System utilities (procps, psmisc, shadow, sudo)
