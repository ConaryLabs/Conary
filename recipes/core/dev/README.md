# Development Tools Recipes

These recipes build the development tools needed to compile software
from source. They should be built after the core libraries.

## Package Categories

### Build Systems
| Package | Version | Description |
|---------|---------|-------------|
| make | 4.4.1 | GNU Make |
| cmake | 4.2.1 | Cross-platform build generator |
| ninja | 1.13.1 | Fast build system |
| meson | 1.7.0 | High productivity build system |

### GNU Autotools
| Package | Version | Description |
|---------|---------|-------------|
| m4 | 1.4.20 | Macro processor |
| autoconf | 2.72 | Configure script generator |
| automake | 1.18.1 | Makefile generator |
| libtool | 2.5.4 | Portable library builder |
| pkgconf | 2.3.0 | Package metadata toolkit |

### Parser/Lexer Generators
| Package | Version | Description |
|---------|---------|-------------|
| bison | 3.8.2 | Parser generator (yacc) |
| flex | 2.6.4 | Lexical analyzer generator (lex) |

### Programming Languages
| Package | Version | Description |
|---------|---------|-------------|
| perl | 5.42.0 | Practical Extraction and Report Language |
| python | 3.14.2 | Python programming language |

### Internationalization
| Package | Version | Description |
|---------|---------|-------------|
| gettext | 0.26 | GNU internationalization framework |

## Build Order

Development tools should be built in this order:

```
Core Libraries (must be complete)
         │
         ▼
    ┌─────────┐
    │  make   │  (no deps beyond glibc)
    └─────────┘
         │
    ┌────┴────────┐
    ▼             ▼
┌──────┐     ┌──────────┐
│  m4  │     │  gettext │  (parallel, both just need glibc)
└──────┘     └──────────┘
    │
    ▼
┌─────────┐     ┌──────┐
│  bison  │────▶│ flex │  (flex needs bison)
└─────────┘     └──────┘
    │
    ▼
┌─────────┐
│ libtool │
└─────────┘
    │
    ▼
┌──────────┐
│   perl   │  (needed by autoconf/automake)
└──────────┘
    │
    ├─────────────────┐
    ▼                 ▼
┌──────────┐     ┌──────────┐
│ autoconf │────▶│ automake │
└──────────┘     └──────────┘

┌──────────┐
│  python  │  (can build in parallel with autotools)
└──────────┘
    │
    ▼
┌─────────┐
│  ninja  │  (needs python to bootstrap)
└─────────┘
    │
    ▼
┌─────────┐
│  meson  │  (pure python, needs ninja)
└─────────┘

┌─────────┐
│ pkgconf │  (needs meson/ninja)
└─────────┘

┌─────────┐
│  cmake  │  (can build with just make)
└─────────┘
```

## Building Development Tools

```bash
# Build all development tools (orchestrated)
conary bootstrap dev

# Or build individual tools
conary cook recipes/core/dev/make.toml
conary cook recipes/core/dev/m4.toml
conary cook recipes/core/dev/bison.toml
conary cook recipes/core/dev/flex.toml
conary cook recipes/core/dev/libtool.toml
conary cook recipes/core/dev/perl.toml
conary cook recipes/core/dev/autoconf.toml
conary cook recipes/core/dev/automake.toml
conary cook recipes/core/dev/gettext.toml
conary cook recipes/core/dev/python.toml
conary cook recipes/core/dev/ninja.toml
conary cook recipes/core/dev/meson.toml
conary cook recipes/core/dev/pkgconf.toml
conary cook recipes/core/dev/cmake.toml
```

## Tool Notes

### GNU Make
- Foundation of most build systems
- Required by almost everything

### M4
- Macro processor used by autoconf
- Must be built before autoconf

### Autotools (autoconf, automake, libtool)
- Traditional GNU build system
- Many packages still use ./configure scripts
- autoconf requires m4 and perl
- automake requires autoconf and perl
- libtool integrates with both

### pkgconf
- Modern replacement for pkg-config
- Creates `pkg-config` symlink for compatibility
- Used to find library compilation flags

### Bison & Flex
- Parser and lexer generators
- Needed by many packages (kernel, bash, etc.)
- flex requires bison to build

### Perl
- Required by autotools and many build scripts
- Thread support enabled
- Shared library enabled

### Python
- Required by meson and many modern tools
- LTO and optimizations enabled
- pip included via ensurepip

### CMake
- Modern build system generator
- Self-bootstraps using make
- System libraries used where possible

### Ninja
- Fast build executor
- Default backend for CMake and Meson
- Bootstraps using Python

### Meson
- Modern, fast build system
- Pure Python package
- Requires ninja as backend

### gettext
- Internationalization framework
- Provides libintl and tools
- Many GNU packages require it

## Verification

After building, verify tools work:

```bash
# Build systems
make --version
cmake --version
ninja --version
meson --version

# Autotools
m4 --version
autoconf --version
automake --version
libtool --version
pkg-config --version

# Parser/Lexer
bison --version
flex --version

# Languages
perl --version
python3 --version

# Internationalization
gettext --version
```

## Common Issues

### Circular Dependencies
The autotools have circular dependencies during bootstrap:
- autoconf needs m4
- automake needs autoconf
- Some packages need all three

Solution: Build m4 first, then autoconf, then automake.

### Perl Location
Many scripts hardcode `/usr/bin/perl`. Ensure perl is installed
there or create a symlink.

### Python Version
Meson requires Python 3.7+. Our python recipe builds 3.14.

## Next Steps

After building development tools, you can build any package that
uses standard build systems (autotools, cmake, meson).
