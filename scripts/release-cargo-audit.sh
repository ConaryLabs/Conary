#!/usr/bin/env bash
set -euo pipefail

# Release-readiness cargo audit gate.
#
# Keep this list in sync with
# docs/superpowers/release-security-waivers-2026-05-06.md. Do not add an
# ignore here without a matching waiver entry and release-owner approval.
cargo audit \
  --ignore RUSTSEC-2023-0071
