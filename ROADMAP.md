# Conary Roadmap

## Direction

Conary is pursuing an adoption-led limited preview: make the reversible path
on an existing supported Linux system safe, understandable, and useful before
expanding the product surface or support claims. Native package managers remain
authoritative for adopted packages until a user explicitly chooses otherwise.

## Current Milestone

The first external tester milestone is ten people outside the existing project
circle completing the bounded preview loop on supported systems and reporting
friction. An evidence-backed maintainer pivot may close the milestone instead
only when a reproducible systemic blocker is documented with the affected
attempts and the chosen next action.

The enabling sequence is:

1. W0 Neutral Planning Migration
2. W1 Integrated Release-Green Baseline
3. W2 Preview Release and Remi Readiness
4. W3 First External Tester Loop

W3 is active, but outreach has not started. Its W3a public-readiness gate must
publish and verify the current onboarding release before the manual tester
posts begin.

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
