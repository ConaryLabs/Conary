#!/usr/bin/env bash
# scripts/release.sh -- Prepare one synchronized Conary workspace release.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MATRIX="${REPO_ROOT}/scripts/release-matrix.sh"
cd "$REPO_ROOT"

die() {
    printf '%s\n' "$1" >&2
    exit 1
}

usage() {
    cat <<'EOF'
Usage: scripts/release.sh suite (--dry-run | --prepare-only) [--target VERSION]

Analyze or prepare the one synchronized Conary workspace release. The suite
version covers every workspace package and all four artifact products.

Options:
  --dry-run          Show the inferred or explicit release without changing files.
  --prepare-only     Update and stage release files without committing or tagging.
  --target VERSION   Use one explicit exact MAJOR.MINOR.PATCH suite version.

Release commits are reviewed and merged before the exact merge commit receives
its annotated v* tag. This script intentionally does not create commits or tags.
EOF
    exit 1
}

is_release_version() {
    [[ "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]
}

version_max() {
    printf '%s\n%s\n' "$1" "$2" | sort -V | tail -n1
}

version_lt() {
    local first="$1"
    local second="$2"
    [[ "$(printf '%s\n%s\n' "$first" "$second" | sort -V | head -n1)" == "$first" && "$first" != "$second" ]]
}

bump_version() {
    local version="$1"
    local level="$2"
    local major minor patch

    IFS='.' read -r major minor patch <<< "$version"
    case "$level" in
        major) printf '%s\n' "$((major + 1)).0.0" ;;
        minor) printf '%s\n' "${major}.$((minor + 1)).0" ;;
        patch) printf '%s\n' "${major}.${minor}.$((patch + 1))" ;;
        *) die "unknown bump level: $level" ;;
    esac
}

matching_suite_tags() {
    git tag --list 'v[0-9]*'
}

history_baseline_version() {
    local -a tags=()
    mapfile -t tags < <(matching_suite_tags)
    if [[ ${#tags[@]} -eq 0 ]]; then
        printf '%s\n' '0.0.0'
        return
    fi
    bash "$MATRIX" latest-version-from-list suite "${tags[@]}"
}

history_baseline_tag() {
    local version="$1"
    local tag="v${version}"
    git rev-parse --verify --quiet "refs/tags/${tag}" >/dev/null || return 1
    printf '%s\n' "$tag"
}

commits_for_suite() {
    local since_ref="$1"
    local -a scope_paths=()
    mapfile -t scope_paths < <(bash "$MATRIX" field suite bump_scope_paths)

    if [[ -n "$since_ref" ]]; then
        git log --no-merges "${since_ref}..HEAD" --oneline -- "${scope_paths[@]}" 2>/dev/null || true
    else
        git log --no-merges --oneline -- "${scope_paths[@]}" 2>/dev/null || true
    fi
}

determine_bump() {
    local since_ref="$1"
    local level="none"
    local line subject

    while IFS= read -r line; do
        [[ -n "$line" ]] || continue
        subject="${line#* }"

        if [[ "$subject" =~ ^(feat|fix|refactor|perf)(\(.+\))?!: ]]; then
            printf '%s\n' 'major'
            return
        fi
        if [[ "$subject" =~ ^feat(\(.+\))?: ]]; then
            level="minor"
        elif [[ "$subject" =~ ^(fix|security|perf)(\(.+\))?: ]] && [[ "$level" == "none" ]]; then
            level="patch"
        fi
    done < <(commits_for_suite "$since_ref")

    printf '%s\n' "$level"
}

generate_changelog() {
    local since_ref="$1"
    local new_version="$2"
    local date line subject description
    local -a features=()
    local -a changed=()
    local -a fixes=()
    local -a security=()
    local -a perf=()
    local -a other=()

    date="$(date +%Y-%m-%d)"
    printf '\n## [v%s] - %s\n\n' "$new_version" "$date"

    while IFS= read -r line; do
        [[ -n "$line" ]] || continue
        subject="${line#* }"
        description="${subject#*: }"

        if [[ "$subject" =~ ^feat(\(.+\))?!?: ]]; then
            features+=("- ${description}")
        elif [[ "$subject" =~ ^refactor(\(.+\))?!?: ]]; then
            changed+=("- ${description}")
        elif [[ "$subject" =~ ^fix(\(.+\))?!?: ]]; then
            fixes+=("- ${description}")
        elif [[ "$subject" =~ ^security(\(.+\))?!?: ]]; then
            security+=("- ${description}")
        elif [[ "$subject" =~ ^perf(\(.+\))?!?: ]]; then
            perf+=("- ${description}")
        elif [[ "$subject" =~ ^(test|chore|docs|release)(\(.+\))?!?: ]]; then
            :
        else
            other+=("- ${subject}")
        fi
    done < <(commits_for_suite "$since_ref")

    if [[ ${#features[@]} -gt 0 ]]; then
        printf '### Added\n%s\n\n' "$(printf '%s\n' "${features[@]}")"
    fi
    if [[ ${#changed[@]} -gt 0 ]]; then
        printf '### Changed\n%s\n\n' "$(printf '%s\n' "${changed[@]}")"
    fi
    if [[ ${#fixes[@]} -gt 0 ]]; then
        printf '### Fixed\n%s\n\n' "$(printf '%s\n' "${fixes[@]}")"
    fi
    if [[ ${#security[@]} -gt 0 ]]; then
        printf '### Security\n%s\n\n' "$(printf '%s\n' "${security[@]}")"
    fi
    if [[ ${#perf[@]} -gt 0 ]]; then
        printf '### Performance\n%s\n\n' "$(printf '%s\n' "${perf[@]}")"
    fi
    if [[ ${#other[@]} -gt 0 ]]; then
        printf '### Other\n%s\n\n' "$(printf '%s\n' "${other[@]}")"
    fi
}

update_workspace_version() {
    local new_version="$1"
    local tmp

    tmp="$(mktemp)"
    awk -v version="$new_version" '
        /^\[workspace\.package\]$/ { in_workspace_package = 1; print; next }
        /^\[/ { in_workspace_package = 0 }
        in_workspace_package && /^version = "/ {
            print "version = \"" version "\""
            updated = 1
            next
        }
        { print }
        END {
            if (!updated) {
                exit 42
            }
        }
    ' Cargo.toml > "$tmp" || {
        status=$?
        rm -f -- "$tmp"
        [[ "$status" -eq 42 ]] && die "Cargo.toml has no [workspace.package] version authority"
        exit "$status"
    }
    mv "$tmp" Cargo.toml
    printf '  Updated root [workspace.package] version\n'
}

update_packaging_versions() {
    local new_version="$1"
    local deb_date tmp

    deb_date="$(date -R)"
    sed -i "s/^Version:.*$/Version:        ${new_version}/" packaging/rpm/conary.spec
    sed -i "s/^pkgver=.*$/pkgver=${new_version}/" packaging/arch/PKGBUILD
    sed -i "s/^version = \".*\"/version = \"${new_version}\"/" packaging/ccs/ccs.toml

    if [[ "$(sed -n '1s/^[^(]*(\([^)]*\)-[0-9][^)]*) .*/\1/p' packaging/deb/debian/changelog)" != "$new_version" ]]; then
        tmp="$(mktemp)"
        {
            printf 'conary (%s-1) unstable; urgency=medium\n\n' "$new_version"
            printf '  * Release %s\n\n' "$new_version"
            printf ' -- Conary Contributors <contributors@conary.io>  %s\n\n' "$deb_date"
            cat packaging/deb/debian/changelog
        } > "$tmp"
        mv "$tmp" packaging/deb/debian/changelog
    fi

    printf '  Updated Conary RPM, Arch, Debian, and CCS packaging versions\n'
}

regenerate_conary_man_page() {
    local new_version="$1"
    local build_script="apps/conary/build.rs"
    local man_page="apps/conary/man/conary.1"

    [[ -f "$build_script" ]] || die "missing Conary build script: ${build_script}"
    touch "$build_script"
    cargo build -p conary --bin conary --quiet

    [[ -s "$man_page" ]] || die "Conary man-page generation did not produce ${man_page}"
    sed -i 's/[[:blank:]]\+$//' "$man_page"
    grep -Fq -- "conary ${new_version}" "$man_page" ||
        die "generated ${man_page} does not contain Conary version ${new_version}"
    printf '  Regenerated %s for %s\n' "$man_page" "$new_version"
}

insert_changelog() {
    local entry="$1"
    local new_version="$2"
    local tmp

    [[ -f CHANGELOG.md ]] || return
    if grep -Fq "## [v${new_version}]" CHANGELOG.md; then
        if [[ "$(sed -n 's/^## \[v\([^]]*\)\].*/\1/p' CHANGELOG.md | head -n1)" != "$new_version" ]]; then
            die "CHANGELOG.md contains v${new_version} outside the current release position"
        fi
        printf '  Retained existing CHANGELOG.md entry for v%s\n' "$new_version"
        return
    fi

    tmp="$(mktemp)"
    head -5 CHANGELOG.md > "$tmp"
    printf '%s\n' "$entry" >> "$tmp"
    tail -n +6 CHANGELOG.md >> "$tmp"
    mv "$tmp" CHANGELOG.md
}

stage_release_files() {
    local -a owned_paths=()
    mapfile -t owned_paths < <(bash "$MATRIX" owned-paths suite)
    git add -- "${owned_paths[@]}" Cargo.lock CHANGELOG.md
    git add -f -- apps/conary/man/conary.1
}

main() {
    local mode=""
    local target_version=""
    local arg

    [[ $# -ge 1 && "$1" == "suite" ]] || usage
    shift

    while [[ $# -gt 0 ]]; do
        arg="$1"
        case "$arg" in
            --dry-run|--prepare-only)
                [[ -z "$mode" ]] || die "select exactly one of --dry-run or --prepare-only"
                mode="$arg"
                ;;
            --target)
                shift
                [[ $# -gt 0 ]] || die "--target requires VERSION"
                target_version="$1"
                ;;
            --target=*) target_version="${arg#--target=}" ;;
            *) usage ;;
        esac
        shift
    done

    [[ -n "$mode" ]] || die "select exactly one of --dry-run or --prepare-only"
    if [[ -n "$target_version" ]]; then
        is_release_version "$target_version" ||
            die "release target must be an exact MAJOR.MINOR.PATCH version"
    fi

    local history_version history_tag manifest_version current_version level new_version
    local -a previous_tags=()
    local changelog_entry

    mapfile -t previous_tags < <(matching_suite_tags)
    history_version="$(history_baseline_version)"
    history_tag="$(history_baseline_tag "$history_version" 2>/dev/null || true)"
    manifest_version="$(bash "$MATRIX" max-owned-version suite)"
    current_version="$(version_max "$history_version" "$manifest_version")"

    printf '=== Conary synchronized suite release ===\n'
    printf '  Previous suite tags considered: %s\n' "${previous_tags[*]:-none}"
    printf '  Published baseline: v%s\n' "$history_version"
    printf '  Current version authority: %s\n' "$manifest_version"

    if [[ -n "$target_version" ]]; then
        version_lt "$history_version" "$target_version" ||
            die "explicit release target ${target_version} must be greater than published baseline ${history_version}"
        [[ -n "$(commits_for_suite "$history_tag")" ]] ||
            die "explicit release target has no suite changes since ${history_tag}"
        new_version="$target_version"
        level="explicit"
        printf '  Target authority: explicit\n'
    else
        level="$(determine_bump "$history_tag")"
        [[ "$level" != "none" ]] || die "no version-bumping suite commits found since ${history_tag:-repository start}"
        new_version="$(bump_version "$current_version" "$level")"
        printf '  Target authority: conventional commits (%s)\n' "$level"
    fi

    if version_lt "$new_version" "$manifest_version"; then
        die "release target ${new_version} would be lower than current version authority ${manifest_version}"
    fi

    printf '  Next version: %s\n' "$new_version"
    printf '  Tag after reviewed merge: v%s\n' "$new_version"
    printf '  Artifact products: conary remi conaryd conary-test\n'

    changelog_entry="$(generate_changelog "$history_tag" "$new_version")"
    if [[ "$mode" == "--dry-run" ]]; then
        printf '%s\n' "$changelog_entry"
        printf '=== Dry run complete ===\n'
        return
    fi

    update_workspace_version "$new_version"
    update_packaging_versions "$new_version"

    case "${CONARY_RELEASE_LOCKFILE_MODE:-offline}" in
        offline) cargo update --workspace --offline --quiet ;;
        online) cargo update --workspace --quiet ;;
        *) die "unknown CONARY_RELEASE_LOCKFILE_MODE: ${CONARY_RELEASE_LOCKFILE_MODE}" ;;
    esac
    printf '  Updated Cargo.lock\n'

    regenerate_conary_man_page "$new_version"
    insert_changelog "$changelog_entry" "$new_version"
    bash "$MATRIX" assert-owned-version suite "$new_version"
    stage_release_files

    printf '  [PREPARED] v%s files staged without commit or tag\n' "$new_version"
    printf '=== Release preparation complete ===\n'
}

main "$@"
