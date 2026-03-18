# Text Processing Tools Recipes

These recipes build essential text processing and file manipulation
utilities found on every Unix-like system.

## Package List

| Package | Version | Description |
|---------|---------|-------------|
| grep | 3.12 | Pattern matching utility |
| sed | 4.9 | Stream editor |
| gawk | 5.3.2 | Pattern scanning language (awk) |
| less | 685 | Terminal pager |
| diffutils | 3.12 | File comparison (diff, cmp) |
| patch | 2.8 | Apply diff files |
| findutils | 4.10.0 | File finding (find, xargs, locate) |
| file | 5.46 | File type identification |

## Build Order

These tools have minimal dependencies and can mostly be built in parallel:

```
Core Libraries (must be complete)
         │
         ▼
    ┌─────────────────────────────────────────┐
    │  These can all build in parallel:       │
    │                                         │
    │  ┌──────┐  ┌─────┐  ┌──────────┐       │
    │  │ grep │  │ sed │  │ diffutils│       │
    │  └──────┘  └─────┘  └──────────┘       │
    │                                         │
    │  ┌───────┐  ┌───────────┐  ┌──────┐   │
    │  │ patch │  │ findutils │  │ file │   │
    │  └───────┘  └───────────┘  └──────┘   │
    │                                         │
    │  ┌──────┐  (needs ncurses)             │
    │  │ less │                               │
    │  └──────┘                               │
    │                                         │
    │  ┌──────┐  (needs readline, mpfr)      │
    │  │ gawk │                               │
    │  └──────┘                               │
    └─────────────────────────────────────────┘
```

## Building Text Tools

```bash
# Build all text processing tools (orchestrated)
conary bootstrap text

# Or build individual tools
conary cook recipes/core/text/grep.toml
conary cook recipes/core/text/sed.toml
conary cook recipes/core/text/gawk.toml
conary cook recipes/core/text/less.toml
conary cook recipes/core/text/diffutils.toml
conary cook recipes/core/text/patch.toml
conary cook recipes/core/text/findutils.toml
conary cook recipes/core/text/file.toml
```

## Tool Descriptions

### grep
Searches for patterns in files using regular expressions.
- Supports basic (`-G`), extended (`-E`), and Perl (`-P`) regex
- egrep and fgrep are symlinks to grep
- Moved to /bin for early boot scripts

### sed
Stream editor for filtering and transforming text.
- Non-interactive batch editing
- Supports regular expressions
- Essential for shell scripts
- Moved to /bin for early boot

### gawk (GNU awk)
Pattern scanning and processing language.
- Powerful data extraction and reporting
- POSIX 1003 compliant
- Extended with GNU features
- Creates `awk` symlink

### less
Terminal pager for viewing files.
- Scroll forward and backward
- Search with regular expressions
- Better than `more` in every way
- Moved to /bin

### diffutils
File comparison utilities.
- `diff` - compare files line by line
- `cmp` - compare files byte by byte
- `sdiff` - side-by-side merge
- `diff3` - compare three files

### patch
Applies diff files to originals.
- Supports unified and context diffs
- Essential for source code patches
- Used by package build systems

### findutils
File finding utilities.
- `find` - search for files by criteria
- `xargs` - build commands from input
- `locate` - fast file search using database
- find and xargs moved to /bin

### file
Identifies file types using magic numbers.
- Uses /usr/share/misc/magic database
- Essential for scripts that need to detect file types
- Moved to /bin

## Verification

After building, verify tools work:

```bash
# Pattern matching
echo "hello world" | grep "world"
echo "hello world" | sed 's/world/universe/'
echo "hello world" | awk '{print $2}'

# File viewing
less /etc/passwd

# File comparison
echo "a" > /tmp/a.txt
echo "b" > /tmp/b.txt
diff /tmp/a.txt /tmp/b.txt

# File finding
find /usr -name "*.so" -type f | head -5
echo /usr/lib/*.so | xargs ls -la

# File type
file /bin/bash
file /etc/passwd
```

## Common Use Cases

### grep Examples
```bash
# Search recursively
grep -r "pattern" /path/to/dir

# Show line numbers
grep -n "pattern" file.txt

# Invert match (lines NOT matching)
grep -v "pattern" file.txt

# Count matches
grep -c "pattern" file.txt
```

### sed Examples
```bash
# Replace first occurrence
sed 's/old/new/' file.txt

# Replace all occurrences
sed 's/old/new/g' file.txt

# In-place editing
sed -i 's/old/new/g' file.txt

# Delete lines matching pattern
sed '/pattern/d' file.txt
```

### awk Examples
```bash
# Print specific column
awk '{print $2}' file.txt

# Sum a column
awk '{sum += $1} END {print sum}' numbers.txt

# Filter by condition
awk '$3 > 100' data.txt

# Field separator
awk -F: '{print $1}' /etc/passwd
```

### find Examples
```bash
# Find by name
find /usr -name "*.h"

# Find by type (f=file, d=directory)
find /var -type f -name "*.log"

# Find and execute
find . -name "*.tmp" -exec rm {} \;

# Find by modification time
find /var/log -mtime +7 -type f
```

## Next Steps

After building text processing tools, you have a complete base
system with development tools. Consider building:
- Compression tools (gzip, bzip2, tar)
- Networking tools (curl, wget)
- Version control (git)
