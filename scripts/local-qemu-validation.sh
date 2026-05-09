#!/usr/bin/env bash
# scripts/local-qemu-validation.sh -- Run the temporary local QEMU release gate.
set -euo pipefail

DISTRO="${CONARY_QEMU_DISTRO:-fedora44}"
LOG_ROOT="${CONARY_LOCAL_VALIDATION_LOG_DIR:-target/local-validation}"
RUN_ID="${CONARY_LOCAL_VALIDATION_RUN_ID:-qemu-$(date +%Y%m%d%H%M%S)}"
LOG_DIR="${LOG_ROOT}/${RUN_ID}"

fail() {
    echo "[local-qemu-validation] ERROR: $*" >&2
    exit 1
}

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || fail "$1 is required"
}

run_suite() {
    local suite="$1"
    local log_file="$2"
    local marker_pattern="$3"

    echo "[local-qemu-validation] running ${suite} for ${DISTRO}"
    cargo run -p conary-test -- run --distro "${DISTRO}" --suite "${suite}" | tee "${log_file}"

    if rg '"status"[[:space:]]*:[[:space:]]*"skipped"|"skipped"[[:space:]]*:[[:space:]]*[1-9][0-9]*' "${log_file}" >/dev/null; then
        fail "${suite} reported a skipped QEMU test result; see ${log_file}"
    fi

    rg "${marker_pattern}" "${log_file}" >/dev/null \
        || fail "${suite} did not emit the expected boot marker; see ${log_file}"
}

test -e /dev/kvm || fail "/dev/kvm is missing; local QEMU release validation requires KVM"
require_cmd cargo
require_cmd qemu-system-x86_64
require_cmd qemu-img
require_cmd rg

mkdir -p "${LOG_DIR}"
echo "[local-qemu-validation] logs: ${LOG_DIR}"

cargo build -p conary -p conary-test --verbose

run_suite \
    phase3-group-n-qemu \
    "${LOG_DIR}/group-n-qemu.log" \
    'kernel-deploy-ok|bls-check-done|rollback-check-done|bootloader-config-ok|boot-verified'

run_suite \
    phase3-group-o-generation-export \
    "${LOG_DIR}/group-o-generation-export.log" \
    'installed-runtime-generation-export-booted|bootstrap-run-generation-export-booted'

echo "[local-qemu-validation] ok"
