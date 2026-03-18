# Compression and Archiving Tools Recipes

These recipes build essential compression and archiving utilities.

## Package List

| Package | Version | Description |
|---------|---------|-------------|
| tar | 1.35 | GNU tape archiver |
| gzip | 1.13 | Deflate compression (.gz) |
| bzip2 | 1.0.8 | Block-sorting compression (.bz2) |
| cpio | 2.15 | Copy files to/from archives |

**Note:** Additional compression libraries are in `recipes/core/libs/`:
- xz 5.6.4 - LZMA2 compression (.xz)
- zstd 1.5.6 - Fast real-time compression (.zst)
- zlib 1.3.1 - Deflate library (used by gzip)

## Build Order

These tools have minimal dependencies:

```
Core Libraries (zlib must be complete)
         │
         ▼
    ┌─────────────────────────────────┐
    │  Can build in parallel:         │
    │                                 │
    │  ┌─────┐  ┌───────┐  ┌──────┐  │
    │  │ tar │  │ gzip  │  │ cpio │  │
    │  └─────┘  └───────┘  └──────┘  │
    │                                 │
    │  ┌───────┐                      │
    │  │ bzip2 │  (no autoconf)      │
    │  └───────┘                      │
    └─────────────────────────────────┘
```

## Building Archive Tools

```bash
# Build all archive tools (orchestrated)
conary bootstrap archive

# Or build individual tools
conary cook recipes/core/archive/tar.toml
conary cook recipes/core/archive/gzip.toml
conary cook recipes/core/archive/bzip2.toml
conary cook recipes/core/archive/cpio.toml
```

## Tool Descriptions

### tar
The standard Unix archiving tool.
- Creates `.tar` archives (uncompressed)
- Supports compression via external programs or built-in:
  - `-z` for gzip (.tar.gz, .tgz)
  - `-j` for bzip2 (.tar.bz2)
  - `-J` for xz (.tar.xz)
  - `--zstd` for zstd (.tar.zst)
- Moved to /bin for early boot

### gzip
Standard compression utility using deflate algorithm.
- Creates `.gz` files
- Includes helper scripts: gunzip, zcat, zgrep, zless, etc.
- Fast compression with good ratio
- All tools moved to /bin

### bzip2
High-quality compression using Burrows-Wheeler algorithm.
- Creates `.bz2` files
- Better compression than gzip, but slower
- Provides shared library (libbz2.so)
- Includes: bunzip2, bzcat, bzgrep, bzmore, bzdiff

### cpio
Archive format used by initramfs and RPM.
- Creates/extracts cpio archives
- Essential for building initramfs images
- Used internally by RPM package format
- Moved to /bin for initramfs scripts

## Common Operations

### Creating Archives

```bash
# tar archive (uncompressed)
tar cvf archive.tar files/

# tar.gz (gzip compressed)
tar czvf archive.tar.gz files/

# tar.bz2 (bzip2 compressed)
tar cjvf archive.tar.bz2 files/

# tar.xz (xz compressed)
tar cJvf archive.tar.xz files/

# tar.zst (zstd compressed)
tar --zstd -cvf archive.tar.zst files/

# cpio archive
find files/ | cpio -o > archive.cpio
```

### Extracting Archives

```bash
# tar (auto-detect compression)
tar xvf archive.tar.gz

# Explicit compression
tar xzvf archive.tar.gz
tar xjvf archive.tar.bz2
tar xJvf archive.tar.xz

# cpio
cpio -idv < archive.cpio

# Single file compression
gunzip file.gz
bunzip2 file.bz2
unxz file.xz
unzstd file.zst
```

### Listing Contents

```bash
# tar
tar tvf archive.tar.gz

# cpio
cpio -tv < archive.cpio
```

## Compression Comparison

| Format | Speed | Ratio | Use Case |
|--------|-------|-------|----------|
| gzip | Fast | Good | General use, web |
| bzip2 | Slow | Better | Archival |
| xz | Slower | Best | Distribution packages |
| zstd | Very Fast | Very Good | Modern replacement |

## Verification

After building, verify tools work:

```bash
# Create test file
echo "test data" > /tmp/test.txt

# Test gzip
gzip -c /tmp/test.txt > /tmp/test.txt.gz
gunzip -c /tmp/test.txt.gz

# Test bzip2
bzip2 -c /tmp/test.txt > /tmp/test.txt.bz2
bunzip2 -c /tmp/test.txt.bz2

# Test tar
tar cvf /tmp/test.tar /tmp/test.txt
tar tvf /tmp/test.tar

# Test cpio
echo /tmp/test.txt | cpio -o > /tmp/test.cpio
cpio -tv < /tmp/test.cpio

# Check versions
tar --version
gzip --version
bzip2 --version
cpio --version
```

## Integration with tar

GNU tar automatically uses compression programs when available:

```bash
# These all work if compression tools are installed:
tar xf archive.tar.gz   # auto-detects gzip
tar xf archive.tar.bz2  # auto-detects bzip2
tar xf archive.tar.xz   # auto-detects xz
tar xf archive.tar.zst  # auto-detects zstd
```

## Next Steps

After building compression tools, consider:
- Networking tools (curl, wget) for downloading
- Git for version control
