# Scriptlet Verification Test Harness

End-to-end testing for Conary's cross-distro scriptlet execution.

## Overview

This test harness generates minimal test packages (RPM, DEB, Arch) with
scriptlets that log their execution arguments, then verifies Conary executes
them with the correct arguments for each scenario.

## Requirements

- Python 3.10+
- `rpmbuild` (for RPM tests) - install via `dnf install rpm-build`
- `dpkg-deb` (for DEB tests) - install via `dnf install dpkg`
- `tar`, `zstd` (for Arch tests) - usually pre-installed
- Conary binary (built from this repo)

## Usage

```bash
# Build conary first
cargo build --release

# Run all tests (requires root for scriptlet execution on /)
sudo python3 tests/scriptlet_harness/test_scriptlets.py

# Run specific test
sudo python3 tests/scriptlet_harness/test_scriptlets.py --test "rpm_upgrade"

# Keep artifacts for debugging
sudo python3 tests/scriptlet_harness/test_scriptlets.py --keep-artifacts

# Specify conary binary path
sudo python3 tests/scriptlet_harness/test_scriptlets.py --conary ./target/debug/conary
```

## Test Matrix

| Test | Format | Scenario | Verifies |
|------|--------|----------|----------|
| RPM Install | RPM | Fresh install | `$1=1` for pre/post |
| RPM Upgrade | RPM | v1 -> v2 | NEW: `$1=2`, OLD: `$1=1` |
| RPM Remove | RPM | Uninstall | `$1=0` for preun/postun |
| DEB Install | DEB | Fresh install | `install`, `configure` |
| DEB Upgrade | DEB | v1 -> v2 | `upgrade <ver>`, `configure <ver>` |
| DEB Remove | DEB | Uninstall | `remove` for prerm/postrm |
| Arch Install | Arch | Fresh install | `post_install(version)` |
| Arch Upgrade | Arch | v1 -> v2 | `pre/post_upgrade(new, old)`, OLD scripts SKIP |
| Arch Remove | Arch | Uninstall | `pre/post_remove(version)` |
| --no-scripts | Any | Skip flag | No scriptlets run |

## Expected Output

```
=== RPM Install ===
[INFO] Installing testpkg-rpm-1.0.0
[INFO] RPM install: pre=$1=1, post=$1=1
[PASS] RPM Install

=== RPM Upgrade ===
[INFO] Installing testpkg-rpm-upg-1.0.0
[INFO] Upgrading to testpkg-rpm-upg-2.0.0
[INFO] RPM upgrade: NEW pre=$1=2, OLD preun=$1=1, OLD postun=$1=1, NEW post=$1=2
[PASS] RPM Upgrade

=== Arch Upgrade (OLD scripts skipped) ===
[INFO] Arch upgrade: NEW pre/post_upgrade('2.0.0', '1.0.0'), OLD remove scripts SKIPPED
[PASS] Arch Upgrade (OLD scripts skipped)
```

## Troubleshooting

### "Skipping - RPM generation not available"

Install rpmbuild: `sudo dnf install rpm-build`

### "Skipping - DEB generation not available"

Install dpkg tools: `sudo dnf install dpkg`

### Tests fail with permission errors

Scriptlet execution requires root. Run with `sudo`.

### Tests pass but scriptlets didn't actually run

Check that root is `/` (not a chroot). Conary skips scriptlets for non-root installs.
