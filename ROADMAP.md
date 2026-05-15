# Conary Roadmap

Conary already has working installs, rollback, adoption/unadoption, immutable generations, Remi conversion/serving, federation, bootstrap, generation artifact export, self-hosting VM validation, and a large integration test surface. The limited public preview target is Fedora 44, Ubuntu 26.04 LTS, and Arch Linux, with security gates and local QEMU validation treated as release criteria while remote Forge validation is paused pending a KVM-capable runner. This roadmap is intentionally forward-looking: it tracks how Conary becomes safe to try on real systems before it asks to become the primary package manager.

For the current system shape, see [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). For shipped changes, see [CHANGELOG.md](CHANGELOG.md).

---

## Current Focus

### 1. Adopt Without Regret

- Keep dnf, apt, and pacman authoritative for packages in adoption mode
- Keep `conary --allow-live-system-mutation system unadopt --all` as the one-command, non-destructive escape hatch for adopted packages on hosts without a selected Conary generation
- Prove non-destructive unadoption for RPM, DEB, and Arch systems
- Ensure Conary update paths never silently turn adopted packages into Conary-owned packages
- Make takeover an explicit opt-in beyond the risk-free adoption lane
- Design active-generation handoff back to native package-manager authority as follow-up work instead of deleting tracking rows while a Conary generation is selected

### 2. No Step Down Package Flows

- Make install, remove, update, search, list, pin, and autoremove feel boringly reliable
- Tighten unsupported-case errors so users know whether to use native PM, adoption refresh, or explicit takeover
- Keep security-update behavior honest per supported package type and distro
- Keep common package-manager expectations covered by real CLI and integration tests

### 3. Developer Experience

- Shell integration and smoother project-local workflows
- Better CLI ergonomics and troubleshooting output
- Cleaner onboarding for contributors and operators
- Fewer "special knowledge" paths for testing server-enabled code

### 4. System Validation

- Keep the `minimal-boot-v3` Fedora 44 QEMU source fixture generation-builder-ready and keep installed-runtime export green
- Broader adoption, unadoption, and explicit takeover validation on Fedora 44, Ubuntu 26.04 LTS, Arch, and real-world mixed systems
- Pristine-by-default QEMU validation for the self-hosting bootstrap VM
- More end-to-end coverage for selected next-boot generation activation and rollback under failure
- Better release-time validation of docs, trust roots, and self-update flows
- Restore scheduled remote Forge validation on a KVM-capable runner; the old VPS runner is retired because it did not expose `/dev/kvm`.

### 5. Composable Systems

- Group packages and published group definitions
- Better system-model ergonomics for large host classes
- Safer, broader-validated replatform and role-migration flows
- Stronger lockfile and remote-include workflows

### 6. Distribution and Scale

- Federation tuning for larger peer topologies
- Optional alternative chunk transports and mirror strategies
- ISO generation export as proof-of-concept follow-up on the shared generation artifact contract
- OCI export hardening and registry workflow polish on the shared generation artifact source
- Signed portable generation bundles and boot-artifact provenance
- More source-oriented workflows around recipes, factories, and remote cooking

### Preview Caveats

- The limited preview should be adoption-led and risk-free to try, not takeover-led. Native package managers remain the authority for adopted RPM, DEB, and Arch packages until the user explicitly chooses takeover.
- `conary --allow-live-system-mutation system unadopt --all` is the one-command escape hatch only when no Conary generation is selected; active-generation handoff back to native authority remains fail-closed follow-up work.
- conaryd has queue/SSE/read-route plumbing and enhance-job execution, but install/remove/update package routes intentionally return `501 Not Implemented`.
- Generation export is release-ready for x86_64 raw/qcow2 validation first; aarch64/riscv64 boot assets remain reserved follow-up work.
- ISO generation export is not part of the limited public preview core promise. OCI export uses the shared generation artifact source, but registry workflow polish remains follow-up.

---

## Near-Term Priorities

1. Prove `conary system unadopt` remains the non-destructive adoption escape for RPM, DEB, and Arch
2. Lock down adopted-package update behavior so native package managers remain authoritative unless takeover is explicit
3. Keep quick-start and preview docs current around adoption, unadoption, native PM coexistence, takeover boundaries, and security-update honesty
4. Design active-generation handoff back to native package-manager authority
5. Keep generation export and installed-runtime QEMU validation in rotation
6. Make self-host VM validation inputs pristine by default
7. Shell integration, release polish, and contributor-experience cleanup

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
