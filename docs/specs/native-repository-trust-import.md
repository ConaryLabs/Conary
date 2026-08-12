---
last_updated: 2026-08-11
revision: 3
summary: Define deterministic fail-closed trust-import planning for selected-root APT, DNF5, libzypp, and ALPM repository declarations
---

# Native Repository Trust-Import Planning

Issue #379 is the second bounded Workstream 10 slice. It consumes the lossless
declarations from
[`native-repository-declarations.md`](native-repository-declarations.md) and
produces strict, serializable preview data. Planning does not fetch keys,
persist trust, enable repositories, invoke a native manager, or mutate the
selected root.

The implementation owner is
`crates/conary-core/src/repository/declarations/trust_import/`. Every planned
repository retains its native declaration identity, enabled state, exact source
locations, role-separated evidence, and one closed disposition:

- `importable`: every required role has an exact local or embedded OpenPGP
  certificate fingerprint, or RPM metadata is authenticated by the exact
  declared metalink root;
- `ambiguous`: native configuration inherits global trust, refers to an unpinned
  remote key, or otherwise lacks an exact binding that Conary can import
  without guessing;
- `unsupported`: verification is disabled or weakened, a declared source is
  missing or malformed, a selector does not match its key material, or a path
  escapes the selected root.

An ambiguous or unsupported disposition is preview evidence, never an enabled
`RepositoryTrustPolicy`. The later takeover/apply slice must resolve every
enabled repository to exact authority before mutation.

## Pinned Source Semantics

Trust planning uses the same upstream revisions pinned by the declaration
contract:

| Ecosystem | Revision | Trust-bearing source contracts |
|---|---|---|
| APT | `apt-team/apt@5e6dcc8d0c8bdce61e9cc7f497abadb5349d509a` | `Signed-By`, embedded deb822 keys, `Trusted`, insecure/weak allowances, and Release verification in `apt-pkg/deb/debmetaindex.cc`, `methods/gpgv.cc`, and `doc/sources.list.5.xml` |
| DNF5 | `rpm-software-management/dnf5@fcda6d3e34233ccfc93364f5efcc6e1d141ce41a` | `gpgkey`, `gpgcheck`/`pkg_gpgcheck`, `repo_gpgcheck`, and metalink precedence in `libdnf5/repo/` and `libdnf5/repo/config/` |
| libzypp | `openSUSE/libzypp@227c6725b98dbddc86652b3f6d8f761a504796f2` | `gpgkey`, general/repository/package GPG policy, and metalink repository authority in `zypp/RepoInfo.cc`, `zypp/repo/RepoInfoBase.cc`, and repository workflow code |
| ALPM | `archlinux/pacman@a6f7467d8c7c4d7e9cc846884e74c0ab7215c48d` | ordered global/per-repository `SigLevel` plus pacman-key populated trust in `src/pacman/conf.c`, `lib/libalpm/signing.c`, and `doc/pacman.conf.5.asciidoc` |

The source package manager can accept broader native trust state than Conary.
Import preserves that fact as a disposition; it does not reproduce permissive
or implicit authority.

## Ecosystem Rules

APT `Signed-By` paths are resolved only below the selected root. Full
fingerprint selectors filter the combined certificate set from every exact
source named by that entry, matching APT's role-global selector behavior; the
trailing `!` exact-key restriction remains typed preview
evidence, and a missing selector is an unsupported mismatch. Deb822 embedded
key blocks are decoded with their dot-escaped blank lines and parsed directly.
A fingerprint without a declared source still refers to global native trust and is ambiguous.
`Trusted=yes` and insecure, weak, or downgrade allowances are unsupported.

DNF and Zypper keep RPM metadata and package roles separate. Repeated DNF
`gpgkey` declarations append in declaration order and retain their individual
locations. Explicit strict
GPG checks plus selected-root `file:` key material can be imported for either
role. An exact `https:` or `file:` metalink without credentials or a fragment
may instead authenticate RPM metadata, but never package bytes; local metalink
paths are confined below the selected root. A declared
metalink satisfies that role even when inherited `gpgcheck_policy` could add a
second native metadata check; an explicit `repo_gpgcheck=true` instead selects
the declared OpenPGP keys. HTTPS key URLs are preserved as ambiguous evidence
because their content does not pin its own certificate fingerprint. Missing
inherited check policy is otherwise ambiguous; an explicit false or permissive
value is unsupported.

Enabled state retains each manager's native boolean grammar. In particular,
libzypp treats its true tokens and nonzero integers as true and other values as
false; the planner does not replace that upstream rule with a new grammar.

Libzypp service declarations and repositories generated dynamically by those
services are not trust-enrollment inputs in this slice. The later enrollment
slice must surface them explicitly rather than treating an empty repository
plan as complete.

ALPM `Never`, `TrustAll`, and optional package signatures are unsupported.
Strict `SigLevel` remains ambiguous in preview because declarations cannot bind
the selected root's populated keyring snapshot to exact master fingerprints
and a certification threshold. Takeover may resolve exactly that one typed
ambiguity from an explicit enrollment policy with an identical effective
`SigLevel`; repository names and keyring filenames cannot create the binding,
and unsafe or otherwise ambiguous native policy remains non-overridable.

## Selected-Root And Serialization Contract

Every local key path passes through `safe_join`. Existing symlinks are resolved
against the canonical selected root, and escapes become typed unsupported
findings without reading the external file. OpenPGP streams must contain at
least one complete certificate; fingerprints are uppercase, deduplicated, and
sorted. Repository and evidence ordering follows declaration order so repeated
planning produces identical JSON.

All preview types deny unknown fields. This is a versioned implementation input,
not an extensible diagnostic bag: future authority must be added deliberately
and rejected by older consumers.

## Proof

Focused tests generate real OpenPGP certificates and cover APT local and
embedded keys, global and remote ambiguity, repeated DNF key declarations,
metalink/package separation and transport rejection, Zypper metadata/package
separation, ALPM strict and unsafe modes, deterministic serialization, unknown
JSON, and selected-root key and metalink symlink escape:

```bash
cargo test -p conary-core repository::declarations::trust_import
```
