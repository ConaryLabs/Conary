# Conary Roadmap

Conary already has working installs, rollback, immutable generations, Remi conversion/serving, federation, bootstrap, and a large integration test surface. This roadmap is intentionally forward-looking: it tracks the next areas to polish, validate, and expand rather than repeating the historical build-out.

For the current system shape, see [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). For shipped changes, see [CHANGELOG.md](CHANGELOG.md).

---

## Current Focus

### 1. Developer Experience

- Shell integration and smoother project-local workflows
- Better CLI ergonomics and troubleshooting output
- Cleaner onboarding for contributors and operators
- Fewer "special knowledge" paths for testing server-enabled code

### 2. System Validation

- Broader takeover validation on Fedora, Ubuntu, Arch, and real-world mixed systems
- QEMU boot verification for bootstrap-generated images
- More end-to-end coverage for generation switching and rollback under failure
- Better release-time validation of docs, trust roots, and self-update flows

### 3. Composable Systems

- Group packages and published group definitions
- Better system-model ergonomics for large host classes
- Safer migration flows for changing system roles
- Stronger lockfile and remote-include workflows

### 4. Distribution and Scale

- Federation tuning for larger peer topologies
- Optional alternative chunk transports and mirror strategies
- More source-oriented workflows around recipes, factories, and remote cooking

---

## Near-Term Priorities

1. QEMU-backed bootstrap verification
2. Shell integration for project and dev environments
3. Group/package composition workflows
4. Release and operational polish for Remi and conaryd
5. Documentation and contributor-experience cleanup

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
