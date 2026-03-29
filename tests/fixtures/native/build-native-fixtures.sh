#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 2 || $# -gt 3 ]]; then
  echo "Usage: $0 <rpm|deb|arch> <output-dir> [fixture-dir]" >&2
  exit 64
fi

target="$1"
output_dir="$2"
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
fixture_dir="${3:-"${script_dir}/../phase4-runtime-fixture"}"
conary_bin="${CONARY_BIN:-conary}"

case "$target" in
  rpm) expected_suffix=".rpm" ;;
  deb) expected_suffix=".deb" ;;
  arch) expected_suffix=".pkg.tar.zst" ;;
  *)
    echo "Unsupported target: ${target}" >&2
    exit 64
    ;;
esac

mkdir -p "${output_dir}"

"${conary_bin}" ccs build "${fixture_dir}" \
  --target "${target}" \
  --output "${output_dir}" \
  --source "${fixture_dir}/stage"

if ! find "${output_dir}" -maxdepth 1 -type f -name "*${expected_suffix}" | grep -q .; then
  echo "No ${expected_suffix} artifact was generated in ${output_dir}" >&2
  exit 1
fi
