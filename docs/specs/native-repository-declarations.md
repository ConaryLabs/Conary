---
last_updated: 2026-08-11
revision: 2
summary: Pin the lossless APT, DNF5, libzypp, and ALPM native repository declaration grammars, selected-root discovery boundary, and trust-planning handoff
---

# Native Repository Declaration Contracts

Issue #377 is the first bounded Workstream 10 conformance slice. It discovers
the repository declarations already present in an explicitly selected root and
decodes their source-owned syntax without invoking a native package manager or
reading its database. This is evidence for later enrollment planning, not
permission to import trust, enable a repository, persist it, or fetch metadata.

The implementation owner is
`crates/conary-core/src/repository/declarations/`. Each ecosystem has a distinct
typed model. The private INI lexer is only shared mechanics; DNF5 and libzypp
retain different comment, continuation, quoting, option, and precedence rules.
Every UTF-8 document retains its exact source, path, ordering, comments,
duplicates, variables, disabled state, and source locations. Rendering the
preserved document is byte-for-byte identity.

## Pinned Upstream Authority

The contract was derived from these exact upstream revisions on 2026-08-11:

| Ecosystem | Upstream revision | Authoritative paths |
|---|---|---|
| APT | [`apt-team/apt@5e6dcc8d0c8bdce61e9cc7f497abadb5349d509a`](https://salsa.debian.org/apt-team/apt/-/commit/5e6dcc8d0c8bdce61e9cc7f497abadb5349d509a) | `apt-pkg/sourcelist.cc`, `apt-pkg/deb/debmetaindex.cc`, `doc/sources.list.5.xml` |
| DNF5 | [`rpm-software-management/dnf5@fcda6d3e34233ccfc93364f5efcc6e1d141ce41a`](https://github.com/rpm-software-management/dnf5/commit/fcda6d3e34233ccfc93364f5efcc6e1d141ce41a) | `libdnf5/utils/iniparser.cpp`, `libdnf5/conf/config_parser.cpp`, `libdnf5/repo/config_repo.cpp`, `include/libdnf5/conf/const.hpp` |
| libzypp | [`openSUSE/libzypp@227c6725b98dbddc86652b3f6d8f761a504796f2`](https://github.com/openSUSE/libzypp/commit/227c6725b98dbddc86652b3f6d8f761a504796f2) | `zypp/parser/RepoFileReader.cc`, `zypp/parser/ServiceFileReader.cc`, `zypp/RepoInfo.cc`, `zypp/repo/RepoInfoBase.cc`, `zypp-core/parser/iniparser.cc` |
| ALPM | [`archlinux/pacman@a6f7467d8c7c4d7e9cc846884e74c0ab7215c48d`](https://gitlab.archlinux.org/pacman/pacman/-/commit/a6f7467d8c7c4d7e9cc846884e74c0ab7215c48d) | `src/pacman/conf.c`, `doc/pacman.conf.5.asciidoc` |

The former `rpm-software-management/libdnf5` location is not the current DNF5
source repository; the pin deliberately follows the current canonical `dnf5`
repository rather than a historical URL.

## Ecosystem Models

APT discovery reads `/etc/apt/sources.list` and valid ASCII
`*.list`/`*.sources` names under `/etc/apt/sources.list.d`. The one-line and
deb822 grammars remain distinct. Typed authority includes source kinds, URIs,
suites, components, disabled entries, deb822 add/remove operations for
architectures, languages, and targets, current meta-index options, `Signed-By` including an
embedded key block, and non-authoritative deb822 annotations. Absolute suites
ending in `/` reject components; distribution suites require them.

DNF discovery reads sorted `*.repo` files from `/etc/yum.repos.d`,
`/etc/distro.repos.d`, and `/usr/share/dnf5/repos.d`, in that order. Its typed
options are the current `ConfigRepo`-bound fields and accepted aliases. Indented
continuations, whole-value quoting, repeated sections and keys, variables, and
comments remain intact. `metalink`, `mirrorlist`, and `baseurl` may coexist;
effective endpoint precedence is exactly metalink, then mirrorlist, then the
first base URL. Their coexistence is not a syntax error.

libzypp discovery reads sorted `*.repo` and `*.service` files from
`/etc/zypp/repos.d` and `/etc/zypp/services.d`. Repository URLs and keys retain
repeatable and multiline forms. libzypp preserves unknown repository extras,
so the raw model does too and marks them `Uninterpreted`; authoritative
selected-root discovery returns a source-located `unknown-authority` error
until an exact semantic is implemented. Services model their fixed keys and
the numbered `repo_N` state family and reject unknown service authority.
Parent components in a repository `path` are rejected rather than applying
libzypp's host-oriented rewrite.

ALPM discovery begins at `/etc/pacman.conf`. It models ordered `[options]` and
repository sections, `Architecture`, global and per-repository `SigLevel`,
`Server`, `CacheServer`, `Usage`, `$repo`/`$arch`, and `Include`. Includes are
expanded lexically inside the selected root, sorted like the upstream glob,
limited to ten nested files, and applied at their exact position. Included
files inherit the current section and may change the section observed by the
including file. Missing, cyclic, escaping, unreadable, invalid UTF-8, and
unknown repository authority fail with typed source attribution. Unrelated
`[options]` directives are losslessly retained but cannot become repository
authority.

## Safety And Consumer Boundary

`discover_selected_root` requires a real root path and uses `safe_join` for
every fixed path, directory entry, and include match. Symlinks and parent
components cannot escape that root. Discovery performs filesystem reads only.
It never executes `apt`, `dnf`, `zypper`, `pacman`, or their libraries and never
consults their native databases.

The declaration types are deliberately not `RepositoryTrustPolicy`, persisted
repository rows, or enabled sync inputs. Issue #379 adds the next read-only
handoff: exact declarations plus selected-root key material produce an
importable, ambiguous, or unsupported trust preview under
[`native-repository-trust-import.md`](native-repository-trust-import.md).
Follow/pin policy, persistence, and apply remain later explicit boundaries.
Distro names, file presence, URLs, extensions, guessed defaults, and diagnostic
text cannot bridge them.

## Proof

Source-derived fixtures live in
`crates/conary-core/tests/fixtures/repository_declarations/`. The declaration
contract tests prove exact rendering, disabled and duplicate preservation, all
DNF endpoint-presence combinations, ALPM include ordering and section
inheritance, unknown-field mutations, invalid UTF-8/path failures, and
selected-root symlink confinement. Run:

```bash
cargo test -p conary-core repository::declarations
```
