# Conary Roadmap

Conary already has working installs, rollback, adoption/unadoption, immutable generations, Remi conversion/serving, federation, bootstrap, generation artifact export, self-hosting VM validation, and a large integration test surface. The limited public preview target is Fedora 44, Ubuntu 26.04 LTS, and Arch Linux, with security gates and local QEMU validation treated as release criteria while remote Forge validation is paused pending a KVM-capable runner. As of 2026-05-21, the local Group O QEMU generation-export gate is green for both installed-runtime and bootstrap-run raw/qcow2 exports, and the focused Group P QEMU run is green for x86_64 ISO generation-carrier export with output provenance, host copy-back, readonly-carrier boot, and writable `/etc` overlay proof. This roadmap is intentionally forward-looking: it tracks how Conary becomes safe to try on real systems before it asks to become the primary package manager.

For the current system shape, see [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). For shipped changes, see [CHANGELOG.md](CHANGELOG.md).
For limited-preview artifact, checksum, signature, SBOM, provenance, and source-build expectations, see [docs/operations/release-artifact-matrix.md](docs/operations/release-artifact-matrix.md).

---

## Current Focus

### 1. Adopt Without Regret

- Keep dnf, apt, and pacman authoritative for packages in adoption mode
- Keep `conary --allow-live-system-mutation system unadopt --all` as the one-command, non-destructive escape hatch for adopted packages on hosts without a selected Conary generation
- Write SQLite rollback and post-success checkpoint backups around
  adoption/unadoption and other first-wave live DB mutations, with
  non-generation recovery commands so the escape hatch remains recoverable even
  before a generation exists
- Prove non-destructive unadoption for RPM, DEB, and Arch systems
- Ensure Conary update paths never silently turn adopted packages into Conary-owned packages
- Make takeover an explicit opt-in beyond the risk-free adoption lane
- Keep selected-generation native-authority handoff explicit, recoverable, and covered across Fedora 44, Ubuntu 26.04 LTS, and Arch

### 2. No Step Down Package Flows

- Make install, remove, update, search, list, pin, and autoremove feel boringly reliable
- Tighten unsupported-case errors so users know whether to use native PM, adoption refresh, or explicit takeover
- Keep security-update behavior honest per supported package type and distro
- Keep common package-manager expectations covered by real CLI and integration tests
- Treat Remi cold-start conversion latency as a preview UX risk: document it in
  the quickstart and pre-warm a small tested package set before broader tester
  outreach.

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

## Review-Derived Release Hardening Queue

The 2026-05-26 multi-model review pass agrees on the main direction: Conary's
near-term advantage is not "replace every package manager now." The useful
wedge is Nix-like safety on the Linux distribution a tester already uses,
with adoption and unadoption as the low-risk entry point. The active planning
queue therefore protects the preview promise before adding more ecosystem
breadth.

Umbrella design:
[Limited Preview Release Hardening](docs/superpowers/specs/2026-05-26-limited-preview-release-hardening-design.md).

1. **Preview truth and onboarding** -
   [plan](docs/superpowers/plans/2026-05-26-preview-truth-and-onboarding.md).
   Fix public truth drift, make the bounded adoption/unadoption path impossible
   to miss, sharpen the Nix comparison, include the site in truth checks,
   explain the live-mutation acknowledgement flag, account for Remi cold-start
   latency, and either implement or route single-package adoption dry-runs.
2. **Scriptlet trust assurance** -
   [plan](docs/superpowers/plans/2026-05-26-scriptlet-trust-assurance.md).
   Resolve the protected-sandbox `chroot`/root-transition ambiguity, make
   namespace failures readable on hardened kernels and containers, make direct
   scriptlet failure semantics structured and visible, and design
   capability-scoped integration hooks before relying on unsandboxed fallbacks.
3. **Release evidence and supportability** -
   [plan](docs/superpowers/plans/2026-05-26-release-evidence-and-supportability.md).
   Publish the minimum artifact/source expectation matrix before first testers,
   keep local KVM evidence honest while remote validation is paused, add beta
   support-bundle/privacy guidance, and turn the limited preview into a
   contributor funnel.
4. **Generation state resilience** -
   [plan](docs/superpowers/plans/2026-05-26-generation-state-resilience.md).
   Add live-mutation inventory plus adoption-lane rollback/post-success SQLite
   backups and non-generation recovery before the first tester wave, then add
   generation-bound SQLite-native backups and recovery commands so a damaged
   live SQLite DB does not blind the manager when generation artifacts and
   metadata are still intact.

Strategic follow-up after the preview queue:

- define a CCS-native corpus strategy with a small signed reference set;
- add aarch64 generation boot assets before claiming broad developer reach;
- add openSUSE/Tumbleweed after Fedora/RPM behavior is boringly repeatable;
- polish recipe, bootstrap, and model flows before selling Conary as a system
  builder target;
- keep conaryd remote/fleet management and signed portable generation bundles
  behind the release-hardening work.

---

## Near-Term Priorities

1. Land the preview truth/onboarding plan before inviting new public testers.
2. Add adoption-lane rollback and post-success DB checkpoints plus
   non-generation recovery before first-wave tester adoption/unadoption becomes
   part of the support story.
3. Land the minimum Plan C supportability slice before the first public tester
   post if binaries or shared tester feedback are involved: the artifact matrix,
   allowlist support bundle, beta feedback template, and release evidence
   command block should stay current together.
4. Land the scriptlet trust-assurance plan before widening beyond careful VM or
   non-critical-machine testers.
5. Keep `conary system unadopt` and selected-generation `native-handoff` proof
   green for Fedora 44, Ubuntu 26.04 LTS, and Arch.
6. Keep generation export, installed-runtime QEMU validation, and Group P ISO
   evidence green in rotation while remote KVM validation is paused.
7. Add release evidence, support-bundle, and contributor-onboarding polish
   before treating the limited preview as more than a small tester program.
8. Add full generation-state DB backup and recovery before generation switching
   becomes the headline public ask.
9. Keep daily-driver UX, shell completions, release polish, and operator
   diagnostics current as preview feedback arrives.

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
