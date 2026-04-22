---
last_updated: 2026-04-21
revision: 2
summary: Operator flow for building and validating the x86_64 self-hosting bootstrap VM
---

# Bootstrap Self-Hosting VM

## Purpose

This is the first truthful self-hosting VM path for Conary bootstrap.

It is intentionally:

- `x86_64` only for the first milestone
- QEMU-first, with a `qcow2` artifact as the source of truth
- explicit about real remote inputs for package operations
- explicit that VMware conversion/import is follow-up work, not the current
  artifact contract

Use this path when you want to prove that a bootstrap-built guest can query,
install, remove, cook, and rebuild `conary` inside itself instead of only
building a base image on the host.

## Exact Operator Flow

Build the Tier-2-complete image:

```bash
scripts/bootstrap-vm/build-selfhost-qcow2.sh \
  --work-dir /tmp/conary-selfhost-vm \
  --image-size 32G
```

Validate the guest against explicit remote inputs:

```bash
scripts/bootstrap-vm/validate-selfhost-vm.sh \
  --work-dir /tmp/conary-selfhost-vm \
  --repo-name fedora-remi \
  --repo-url "$REPO_URL" \
  --remi-endpoint "$REMI_ENDPOINT" \
  --remi-distro fedora \
  --root-json /absolute/path/to/root.json
```

If the target repository does not require TUF bootstrap for the first run, omit
`--root-json`.

The build wrapper is the supported single-entry path. It creates the staged
workspace tarball, runs bootstrap in this order, and then emits the final
self-hosting image:

1. `conary bootstrap init`
2. `conary bootstrap cross-tools`
3. `conary bootstrap temp-tools`
4. `conary bootstrap system`
5. `conary bootstrap config`
6. `conary bootstrap tier2`
7. `conary bootstrap guest-profile --public-key <work_dir>/vm-selfhost/keys/selfhost_ed25519.pub`
8. `conary bootstrap image --format qcow2 --size 32G`

The wrapper keeps Phase 1 and the deterministic input generation unprivileged,
but it deliberately routes the chroot-owning phases through a rootful runner so
the bootstrap core, not the shell wrapper, still owns `/dev`, `/proc`, `/sys`,
`/run`, and `chroot` setup:

- `conary bootstrap temp-tools`
- `conary bootstrap system`
- `conary bootstrap tier2`

By default that rootful runner is `sudo`. After each privileged phase, the
wrapper restores ownership of `<work_dir>` back to the invoking user so the
later unprivileged steps (`config`, `guest-profile`, `image`, and VM
validation) can still mutate and read the generated artifacts normally. For
harness/testing scenarios, the wrapper also honors
`CONARY_BOOTSTRAP_ROOTFUL_RUNNER` as an override for the rootful command.

The validation wrapper then:

1. boots the finished `qcow2` under QEMU
2. waits for SSH using a host-generated ephemeral keypair
3. copies the staged workspace tarball and optional `root.json` into the guest
4. runs the checked-in guest validation script inside the VM

## Artifact Layout

The wrappers reserve a self-hosting area under `<work_dir>/vm-selfhost/`:

```text
<work_dir>/
  vm-selfhost/
    inputs/
      conary-workspace.tar.gz
      conary-workspace.tar.gz.sha256
    keys/
      selfhost_ed25519
      selfhost_ed25519.pub
    logs/
      qemu-serial.log
      ssh-probe.log
      guest-validate.log
    output/
      conaryos-selfhost-x86_64.qcow2
```

The validation script copies the same checked host inputs into:

```text
/var/lib/conary/bootstrap-inputs/
  conary-workspace.tar.gz
  conary-workspace.tar.gz.sha256
  guest-validate.sh
  root.json              # optional
  conary-workspace/      # unpacked in guest
```

## Deterministic Workspace Tarball Contract

The host-side `conary` Tier 2 build and the in-guest rebuild must use the same
workspace snapshot.

The checked-in wrapper therefore creates:

- `conary-workspace.tar.gz` from the current working tree via:
  `git ls-files -z --cached --modified --others --exclude-standard | sort -z | tar --null --no-recursion --files-from=- --transform='s,^,conary-workspace/,' --mtime='UTC 1970-01-01' --owner=0 --group=0 --numeric-owner -cf - | gzip -n`
- `conary-workspace.tar.gz.sha256` as a plain `sha256sum` digest sidecar

The host Tier 2 implementation validates that sidecar before it extracts the
staged `conary` source, and the guest validation script repeats the same check
before rebuilding `conary` inside the VM.

## Remote Inputs

The guest validation path is intentionally explicit about infrastructure:

- `--repo-name`: local Conary repository name inside the guest
- `--repo-url`: repository metadata URL used by `conary repo add`
- `--remi-endpoint`: Remi conversion endpoint for the default strategy
- `--remi-distro`: Remi distro name, for example `fedora`
- `--root-json`: optional absolute path to initial TUF root metadata copied
  into the guest after boot

Inside the guest the checked-in validation script performs:

1. `conary repo add`
2. optional `conary trust init <repo-name> --root /var/lib/conary/bootstrap-inputs/root.json`
3. `conary repo sync`
4. `conary query label list`
5. `conary install tree`
6. `conary remove tree`
7. `conary cook /var/lib/conary/bootstrap-inputs/conary-workspace/recipes/bootstrap-smoke/simple-hello.toml`
8. `cargo build --locked` from the unpacked Conary workspace
9. rebuilt-binary smoke commands with `target/debug/conary`

The guest validation also fails closed if it finds a reusable operator/test
private SSH key under `/root/.ssh`.

## Tier 2 Audit: 2026-04-16

The first self-hosting milestone uses this audited package set:

| Component | Version | Notes |
|-----------|---------|-------|
| `linux-pam` | `1.7.2` | Matches current BLFS systemd page |
| `openssh` | `10.3p1` | Matches current BLFS systemd page |
| `make-ca` | `1.16.1` | Matches current BLFS systemd page |
| `curl` | `8.19.0` | Matches current BLFS systemd page |
| `sudo` | `1.9.17p2` | Matches current BLFS systemd page |
| `nano` | `9.0` | Updated to the current BLFS page |
| `rust` | `1.94.0` | Version matches current BLFS page |
| `conary` | tracked workspace | Built from the staged `conary-workspace.tar.gz` snapshot |

Additional prerequisite note:

- `sqlite` is required by the Tier 2 `conary` recipe but is owned by Phase 3,
  not counted as a ninth Tier 2 package
- the self-hosting path closes that gap by building `sqlite` in the final
  system before `python`

Rust diverges from BLFS install method: the self-hosting path installs the
official `x86_64-unknown-linux-gnu` Rust binary distribution instead of
building `rustc` from source. Keep that divergence explicit in recipe comments
and in future audit updates.

## Why The Wrapper Defaults To 32G

The generic `conary bootstrap image` default is `4G`, which is too small for a
truthful self-hosting validation run once the guest contains:

- the Tier 2 package closure
- the staged Conary workspace tarball
- a writable Cargo target directory
- temporary build outputs for the in-guest `cargo build --locked`

The first full self-hosting validation run on the checked-in workspace
exhausted a `16G` image during the in-guest `cargo build --locked`, so the
checked-in wrapper now defaults to `32G`. Treat smaller images as debug-only
experiments unless a later measurement proves a lower floor is still truthful.

## Follow-Up: VMware

The current milestone ends at a truthful QEMU-validated `qcow2`.

VMware conversion and import are follow-up work after this path is stable. That
follow-up may document `qemu-img` conversion, OVF packaging, or manual import
steps, but those artifacts are intentionally not part of the first self-hosting
acceptance contract.
