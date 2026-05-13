# Conary Roadmap

Conary already has working installs, rollback, immutable generations, Remi conversion/serving, federation, bootstrap, generation artifact export, self-hosting VM validation, and a large integration test surface. The limited public preview target is Fedora 44, Ubuntu 26.04 LTS, and Arch Linux, with security gates and local QEMU validation treated as release criteria while remote Forge validation is paused pending a KVM-capable runner. This roadmap is intentionally forward-looking: it tracks the next areas to polish, validate, and expand rather than repeating the historical build-out.

For the current system shape, see [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). For shipped changes, see [CHANGELOG.md](CHANGELOG.md).

---

## Current Focus

### 1. Developer Experience

- Shell integration and smoother project-local workflows
- Better CLI ergonomics and troubleshooting output
- Cleaner onboarding for contributors and operators
- Fewer "special knowledge" paths for testing server-enabled code

### 2. System Validation

- Refresh the Fedora 44 QEMU source image so generation-export validation starts from a generation-builder-ready fixture, then keep installed-runtime export green
- Broader takeover validation on Fedora 44, Ubuntu 26.04 LTS, Arch, and real-world mixed systems
- Pristine-by-default QEMU validation for the self-hosting bootstrap VM
- More end-to-end coverage for generation activation and rollback under failure
- Better release-time validation of docs, trust roots, and self-update flows
- Restore scheduled remote Forge validation on a KVM-capable runner; the old VPS runner is retired because it did not expose `/dev/kvm`.

### 3. Composable Systems

- Group packages and published group definitions
- Better system-model ergonomics for large host classes
- Safer, broader-validated replatform and role-migration flows
- Stronger lockfile and remote-include workflows

### 4. Distribution and Scale

- Federation tuning for larger peer topologies
- Optional alternative chunk transports and mirror strategies
- ISO generation export on the shared generation artifact contract
- OCI export hardening and registry workflow polish on the shared generation artifact source
- Signed portable generation bundles and boot-artifact provenance
- More source-oriented workflows around recipes, factories, and remote cooking

### Preview Caveats

- conaryd has queue/SSE/read-route plumbing and enhance-job execution, but install/remove/update package routes intentionally return `501 Not Implemented`.
- Generation export is release-ready for x86_64 raw/qcow2 validation first; aarch64/riscv64 boot assets remain reserved follow-up work.
- ISO generation export is not part of the limited public preview. OCI export uses the shared generation artifact source, but registry workflow polish remains follow-up.

---

## Near-Term Priorities

1. Keep generation export and installed-runtime QEMU validation in rotation
2. Make self-host VM validation inputs pristine by default
3. Finish ISO generation export on the generation artifact contract
4. Shell integration for project and dev environments
5. Release and operational polish for Remi and conaryd
6. Documentation and contributor-experience cleanup

---

## Not Planned

These features from the original Conary lineage are still out of scope:

- **rBuilder integration** -- proprietary appliance builder
- **cvc tool revival** -- standard Git workflows are preferred
- **Appliance groups as originally designed** -- superseded by more general composition work
- **Highly specialized desktop package templates** -- general templates are preferred

---

## Contributing

If you want to help, the most useful work right now is in validation, developer experience, and documentation quality. See [CONTRIBUTING.md](CONTRIBUTING.md) for setup and [SECURITY.md](SECURITY.md) for reporting policy.
