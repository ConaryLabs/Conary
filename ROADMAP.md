# Conary Roadmap

Conary already has working installs, rollback, immutable generations, Remi conversion/serving, federation, bootstrap, generation artifact export, self-hosting VM validation, and a large integration test surface. This roadmap is intentionally forward-looking: it tracks the next areas to polish, validate, and expand rather than repeating the historical build-out.

For the current system shape, see [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). For shipped changes, see [CHANGELOG.md](CHANGELOG.md).

---

## Current Focus

### 1. Developer Experience

- Shell integration and smoother project-local workflows
- Better CLI ergonomics and troubleshooting output
- Cleaner onboarding for contributors and operators
- Fewer "special knowledge" paths for testing server-enabled code

### 2. System Validation

- Keep the Fedora 44 generation-export QEMU suite green, including installed-runtime export
- Broader takeover validation on Fedora, Ubuntu, Arch, and real-world mixed systems
- Pristine-by-default QEMU validation for the self-hosting bootstrap VM
- More end-to-end coverage for generation activation and rollback under failure
- Better release-time validation of docs, trust roots, and self-update flows

### 3. Composable Systems

- Group packages and published group definitions
- Better system-model ergonomics for large host classes
- Safer, broader-validated replatform and role-migration flows
- Stronger lockfile and remote-include workflows

### 4. Distribution and Scale

- Federation tuning for larger peer topologies
- Optional alternative chunk transports and mirror strategies
- ISO export and OCI convergence on the shared generation artifact contract
- Signed portable generation bundles and boot-artifact provenance
- More source-oriented workflows around recipes, factories, and remote cooking

---

## Near-Term Priorities

1. Keep generation export and installed-runtime QEMU validation in rotation
2. Make self-host VM validation inputs pristine by default
3. Finish ISO export and OCI convergence on the generation artifact contract
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
