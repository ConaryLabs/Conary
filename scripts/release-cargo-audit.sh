#!/usr/bin/env bash
set -euo pipefail

# Release-readiness cargo audit gate.
#
# Keep this list in sync with
# docs/operations/release-security-waivers.md. Do not add an
# ignore here without a matching waiver entry and release-owner approval.
python3 scripts/check-third-party-divergence.py --check-upstream-exits

cargo audit \
  --ignore RUSTSEC-2023-0071
