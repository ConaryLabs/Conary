# Conary Roadmap

Conary already has working installs, rollback, adoption/unadoption, immutable generations, Remi conversion/serving, federation, bootstrap, generation artifact export, self-hosting VM validation, and a large integration test surface. The limited public preview target is Fedora 44, Ubuntu 26.04 LTS, and Arch Linux, with security gates and local QEMU validation treated as release criteria while remote Forge validation is paused pending a KVM-capable runner. As of 2026-05-21, the local Group O QEMU generation-export gate is green for both installed-runtime and bootstrap-run raw/qcow2 exports, and the focused Group P QEMU run is green for x86_64 ISO generation-carrier export with output provenance, host copy-back, readonly-carrier boot, and writable `/etc` overlay proof. This roadmap is intentionally forward-looking: it tracks how Conary becomes safe to try on real systems before it asks to become the primary package manager.

For the current system shape, see [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). For shipped changes, see [CHANGELOG.md](CHANGELOG.md).

---

## Current Focus

### 1. Adopt Without Regret

- Keep dnf, apt, and pacman authoritative for packages in adoption mode
- Keep `conary --allow-live-system-mutation system unadopt --all` as the one-command, non-destructive escape hatch for adopted packages on hosts without a selected Conary generation
- Prove non-destructive unadoption for RPM, DEB, and Arch systems
- Ensure Conary update paths never silently turn adopted packages into Conary-owned packages
- Make takeover an explicit opt-in beyond the risk-free adoption lane
- Keep selected-generation native-authority handoff explicit, recoverable, and covered across Fedora 44, Ubuntu 26.04 LTS, and Arch

### 2. No Step Down Package Flows

- Make install, remove, update, search, list, pin, and autoremove feel boringly reliable
- Tighten unsupported-case errors so users know whether to use native PM, adoption refresh, or explicit takeover
- Keep security-update behavior honest per supported package type and distro
- Keep common package-manager expectations covered by real CLI and integration tests

### 3. Developer Experience

- Shell integration and smoother project-local workflows, backed by rendered bash/zsh completion checks
- Better CLI ergonomics and troubleshooting output, tracked in the daily-driver UX matrix
- Cleaner onboarding for contributors and operators
- Fewer "special knowledge" paths for testing server-enabled code

### 4. System Validation

- Keep the `minimal-boot-v3` Fedora 44 QEMU source fixture generation-builder-ready and keep Group O installed-runtime/bootstrap-run generation export green after the 2026-05-21 refresh
- Keep the Group P ISO generation export manifest in the local KVM gate now that its focused 2026-05-21 pass is recorded
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
- OCI export hardening and registry workflow polish on the shared generation artifact source
- Signed portable generation bundles and signed boot-artifact provenance
- More source-oriented workflows around recipes, factories, and remote cooking

### Preview Caveats

- The limited preview should be adoption-led and risk-free to try, not takeover-led. Native package managers remain the authority for adopted RPM, DEB, and Arch packages until the user explicitly chooses takeover.
- `conary --allow-live-system-mutation system unadopt --all` is the one-command escape hatch only when no Conary generation is selected; after a generation is selected, use `conary system native-handoff --dry-run` and then `conary --allow-live-system-mutation system native-handoff --yes`. `--recover --yes` resumes an interrupted handoff record.
- conaryd has queue/SSE/read-route plumbing plus install/remove/update and enhance-job execution. Package mutation jobs still require the same explicit live-host mutation acknowledgement as the CLI.
- Generation export has x86_64 raw/qcow2/ISO support. The 2026-05-21 Group O QEMU run passed installed-runtime and bootstrap-run raw/qcow2 boot proof. The focused 2026-05-21 Group P QEMU run passed ISO export, provenance sidecar, copy-back, readonly-carrier boot, and writable `/etc` overlay proof. Keep generation export as supporting evidence for the preview rather than the headline ask. aarch64/riscv64 boot assets remain reserved follow-up work.
- The former `tough`/Sigstore trust-root dependency path has been removed from `Cargo.lock`; the remaining `rsa` RustSec advisory is covered by the dated limited-preview waiver until a compatible fixed dependency path exists.
- ISO generation export is implemented as a generation-carrier artifact, not installer media. OCI export uses the shared generation artifact source, but registry workflow polish remains follow-up.

---

## Near-Term Priorities

1. Prove `conary system unadopt` remains the non-destructive adoption escape for RPM, DEB, and Arch
2. Lock down adopted-package update behavior so native package managers remain authoritative unless takeover is explicit
3. Keep quick-start and preview docs current around adoption, unadoption, native PM coexistence, takeover boundaries, and security-update honesty
4. Keep selected-generation native-authority handoff validation green and document any recovery caveats from real use
5. Keep generation export, installed-runtime QEMU validation, and Group P ISO evidence green in rotation
6. Keep self-host VM validation inputs fresh by default and finish pristine rerun isolation
7. Keep the daily-driver UX matrix, shell completions, release polish, and contributor/operator diagnostics current

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
