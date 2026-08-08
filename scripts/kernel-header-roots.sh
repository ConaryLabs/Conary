#!/usr/bin/env bash
# scripts/kernel-header-roots.sh
#
# Locate the Linux UAPI headers that libseccomp needs when it is compiled
# against musl, which supplies no kernel headers of its own.
#
# This file is meant to be sourced. Running it directly prints the roots
# resolved for this host, which is the fastest way to diagnose a static build
# that failed its header preflight.
#
# Distributions disagree about where these headers live, and they do not even
# keep them in one place. Arch and Fedora put both under /usr/include. Debian
# and Ubuntu ship linux/ under /usr/include but the architecture-specific asm/
# only under the multiarch directory, so no single root holds both. Each
# header is therefore located on its own and every distinct directory that
# supplied one is returned, in candidate order.

KERNEL_HEADERS_REQUIRED=("linux/audit.h" "asm/unistd.h")

# The directories searched, most exact first.
kernel_header_candidates() {
    printf '%s\n' "/usr/include" "/usr/include/$(uname -m)-linux-gnu"
}

# Usage: kernel_header_roots CANDIDATE_DIR...
#
# Prints one include root per line, deduplicated and in candidate order.
# Returns 1 and explains which header was missing from where when any required
# header cannot be found at all.
kernel_header_roots() {
    local candidates=("$@")
    local header candidate entry found
    local -a resolved=()
    local -a missing=()
    local -a roots=()

    if [[ ${#candidates[@]} -eq 0 ]]; then
        echo "error: kernel_header_roots needs at least one candidate directory" >&2
        return 1
    fi

    for header in "${KERNEL_HEADERS_REQUIRED[@]}"; do
        found=""
        for candidate in "${candidates[@]}"; do
            if [[ -f "$candidate/$header" ]]; then
                found="$candidate"
                break
            fi
        done
        if [[ -n "$found" ]]; then
            resolved+=("$header=$found")
        else
            missing+=("$header")
        fi
    done

    if [[ ${#missing[@]} -gt 0 ]]; then
        {
            echo "error: required Linux UAPI headers are missing"
            for header in "${missing[@]}"; do
                printf '  %s not found under any of: %s\n' "$header" "${candidates[*]}"
            done
            for entry in ${resolved[@]+"${resolved[@]}"}; do
                printf '  %s found under %s\n' "${entry%%=*}" "${entry#*=}"
            done
            echo "libseccomp cannot be built for musl without them; install your"
            echo "distribution's kernel header package (linux-libc-dev on Debian/Ubuntu,"
            echo "kernel-headers on Fedora, linux-api-headers on Arch)."
        } >&2
        return 1
    fi

    # Emit in candidate order, so the most exact directory comes first, and
    # only once even when it supplied several headers.
    for candidate in "${candidates[@]}"; do
        for entry in "${resolved[@]}"; do
            if [[ "${entry#*=}" == "$candidate" ]]; then
                roots+=("$candidate")
                break
            fi
        done
    done

    printf '%s\n' "${roots[@]}"
}

# Usage: kernel_header_include_flags ROOT...
#
# Renders one -idirafter per root. -idirafter appends to the end of the search
# path, so these directories are consulted only after musl's own headers and
# cannot shadow them.
kernel_header_include_flags() {
    local root flags=""
    for root in "$@"; do
        flags="${flags:+$flags }-idirafter $root"
    done
    printf '%s\n' "$flags"
}

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    set -euo pipefail
    mapfile -t _candidates < <(kernel_header_candidates)
    mapfile -t _roots < <(kernel_header_roots "${_candidates[@]}")
    printf 'roots: %s\n' "${_roots[*]}"
    printf 'flags: %s\n' "$(kernel_header_include_flags "${_roots[@]}")"
fi
