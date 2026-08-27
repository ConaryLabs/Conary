# Conary Roadmap

## Direction

Conary is pursuing a cross-distro package-installation preview: make RPM, DEB,
and Arch packages retain their source ABI while Conary installs them on any
supported Linux target with the required typed capabilities. Conary owns normal
install, update, remove, and rollback operations. Native package managers remain
authority only for packages in the explicit adoption/takeover migration path.

## Current Milestone

The first external tester milestone is ten people outside the existing project
circle completing a bounded cross-distro artifact loop on supported systems and
reporting friction. Each qualifying run installs at least one package whose
source format differs from the host's native format. An evidence-backed
maintainer pivot may close the milestone instead only when a reproducible
systemic blocker is documented with the affected attempts and chosen next
action.

W7's ordinary-package gate closed through #110 and PR #487. The active launch
lane now completes the signed immutable public universe in #598, the
pre-release daily-driver floor in #122, #534, #132, #642, and #643, the
synchronized public-artifact journey in #639, and launch proof in #121/#149.
Outreach remains at 0/10 and still waits for
the separate cached-history and venue checks. The machine-readable
[launch status](docs/roadmaps/launch-status.json) owns the current gate and
tester-pin state; release publication alone does not assign tester authority.

Detailed maturity, workstream status, proof, blockers, and longer horizons live
in the [development roadmap](docs/roadmaps/development-roadmap.md).

## Current Preview Caveats

- Public package-manager proof is scoped to Fedora 44, Ubuntu 26.04 LTS, and
  Arch Linux. Treat other distro and architecture support as unproven.
- Generation boot and export proof is x86_64-focused. Other architectures,
  signed boot authority, and broader system-builder claims remain later work.
- The local CLI package-manager flow and operator-run Remi service are the
  useful preview path. conaryd and federation are outside the reliable core
  path and must not be presented as production-ready fleet services.
- Preview releases remain unsuitable for irreplaceable daily-driver systems;
  start with a VM, snapshot, or non-critical host.

## Stable References

- [Architecture](docs/ARCHITECTURE.md)
- [Changelog](CHANGELOG.md)
- [Release artifact matrix](docs/operations/release-artifact-matrix.md)
- [Contributing](CONTRIBUTING.md)
- [Security policy](SECURITY.md)

## Not Planned

- rBuilder integration or revival of the proprietary appliance-builder model
- revival of `cvc`; normal Git workflows remain the project direction
- original-lineage appliance groups or specialized desktop package templates
- broad production, federation, or multi-architecture claims before the
  current milestone and their owning proof gates are complete
