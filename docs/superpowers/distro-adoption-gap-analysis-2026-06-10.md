# Distro Adoption Gap Analysis — 2026-06-10

**Question:** Beyond the adopt/try-before-you-commit feature, how viable is Conary as the
package manager for someone starting a new Linux distro, or migrating an existing
system/fleet off apt/dnf/pacman? What gaps exist, and what would make that entry point
as easy as possible?

**Verdict:** The architecture is not the blocker. conary-core has no hard dependency on
Remi or conary.io, repository sources are pluggable (HTTP, static files, local), and the
runtime root/db paths are configurable. The story fails today on **missing tooling and
docs at the edges** — chiefly, no third-party path to create and publish packages.

## Where the architecture already helps

- `RepositoryClient` is a trait with format-agnostic metadata parsing (RPM repodata,
  Debian Packages, Arch DB). Chunk fetchers exist for HTTP (`HttpChunkFetcher`), local
  files (`LocalCacheFetcher`), and composites — a plain static file server is already a
  valid repo backend on the *client* side.
- Remi is optional. It is a conversion proxy, not a required hub. No conary.io URLs are
  hardcoded in core code; they appear only in docs/examples and test fixtures.
- Native `.rpm` / `.deb` / `.pkg.tar.zst` packages install directly; CCS conversion is
  transparent. Adopt/unadopt is reversible and tested.
- Runtime layout (`/conary`, `/var/lib/conary/conary.db`) is constructor-parameterized,
  not hardcoded.
- Signing is Ed25519 (CCS) + sigstore, both configurable.

## Persona A: starting a new distro (wants Conary as their pacman)

Gaps in priority order:

1. **No general-purpose package building toolchain.** Biggest gap. CCS packages can only
   be built via the library API (`CcsBuilder`) or the bootstrap recipe system, and
   `recipes/` is scoped entirely to the conaryOS LFS build. There is no
   `conary build ./recipe.toml` → `.ccs` workflow a third party can use. A distro cannot
   exist without a way to make packages.
2. **No way to publish a repo without running Remi.** Remi is a heavyweight answer
   (conversion pipeline, tantivy search, S3/CDN integration) when a small distro needs
   `conary repo publish ./packages/ ./out/` producing static metadata + chunks hostable
   on any dumb HTTP server or GitHub Pages. The client side already supports consuming
   this; only the publisher side is missing.
3. **Distro identity is hardcoded.** `SUPPORTED_USER_DISTROS` in
   `crates/conary-core/src/repository/distro.rs` is a 3-entry const (fedora-44,
   ubuntu-26.04, arch). A new distro should be a data-driven profile (TOML), not a code
   change — plus optional scriptlet-conversion rules in `support_matrix.rs` if Remi
   conversion is wanted.
4. **Signing/key UX.** Ed25519 + sigstore are wired in, but there is no operator-facing
   keygen / trust-setup flow, scripted or documented.

## Persona B: migrating an existing system/fleet off apt/dnf/pacman

This path is in much better shape — adopt/unadopt, direct native package install,
generations, and rollback all work. Remaining gaps are operational:

1. **No self-hosting tutorial for Remi.** `docs/operations/infrastructure.md` documents
   *our* deployment, not a stranger's. A "run your own Remi in 30 minutes" guide
   (single binary + `remi.toml` + optional R2) would unlock org-level adoption.
2. **No per-source-distro migration runbooks** ("coming from apt: habit mapping, cutover
   sequence, escape hatch").
3. **Generation-model requirements are not stated up front.** composefs (kernel 6.5+,
   mounted via overlayfs), systemd, UEFI boot stack. Needs a published compatibility
   checklist so people on older stacks hit a doc, not a wall.

## Structural recommendation: split Remi's two jobs

Remi currently carries two jobs — **conversion proxy** and **repo server** — and the
repo-server job should be commoditized:

- Extract a **static repo format + publisher tool** (thin CLI over conary-core, or a
  small `conary-repo` crate). Remi becomes one *producer* of that format; a static
  publisher becomes another. Clients don't care which.
- Promote the recipe/build machinery **out of `bootstrap/` into a first-class packaging
  toolchain**. Bootstrap becomes a *consumer* of the general build system rather than
  its owner. conaryOS then dogfoods the same tooling third parties would use, which
  keeps it honest.
- Make the distro catalog **data-driven** so "add your distro" is a config file, not a
  fork.

## Keystone

`conary build` (recipe → .ccs) plus static repo publishing serves **both** personas: a
new distro needs it to exist, and a migrator needs it the day they want one internal
package that isn't in upstream repos. If one investment is chosen first, it is that
pair, with operator docs (self-hosted Remi tutorial, migration runbooks, compatibility
checklist) as the cheap fast-follow.

## Effort sketch (from structural survey)

| Scenario | Rough effort |
|----------|--------------|
| Individual tries Conary on a supported distro via public Remi | days |
| Org self-hosts Remi | weeks–2 months (mostly ops + missing docs) |
| Third party builds a native CCS package ecosystem | months (blocked on build/publish toolchain) |
| New distro: bootstrap + ecosystem + branding | 6–12 months |

*Survey basis: workspace exploration on 2026-06-10 (schema v71, 8 workspace members,
limited-preview status). See `docs/ARCHITECTURE.md` and `docs/conaryopedia-v2.md` for
the underlying architecture reference.*
