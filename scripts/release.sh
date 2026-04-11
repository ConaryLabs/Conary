#!/usr/bin/env bash
# scripts/release.sh -- Automated release based on conventional commits
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MATRIX="${REPO_ROOT}/scripts/release-matrix.sh"
cd "$REPO_ROOT"

die() {
    printf '%s\n' "$1" >&2
    exit 1
}

mapfile -t PRODUCTS < <(bash "$MATRIX" products)

usage() {
    cat <<'EOF'
Usage: scripts/release.sh [conary|remi|conaryd|conary-test|all] [--dry-run]

Analyze conventional commits since the latest product release tag and bump versions.
  conary       - conary CLI + owned crates + packaging
  remi         - Remi service app
  conaryd      - daemon service app
  conary-test  - integration harness + conary-mcp
  all          - all release tracks
  --dry-run    Show what would happen without making changes
EOF
    exit 1
}

is_product() {
    local candidate="$1"
    local product

    for product in "${PRODUCTS[@]}"; do
        if [[ "$product" == "$candidate" ]]; then
            return 0
        fi
    done

    return 1
}

matrix_field() {
    local product="$1"
    local field="$2"
    bash "$MATRIX" field "$product" "$field"
}

join_by() {
    local delimiter="$1"
    shift
    local joined=""
    local value

    for value in "$@"; do
        if [[ -n "$joined" ]]; then
            joined+="${delimiter}"
        fi
        joined+="${value}"
    done

    printf '%s\n' "$joined"
}

version_max() {
    local first="${1:-}"
    local second="${2:-}"

    if [[ -z "$first" ]]; then
        printf '%s\n' "$second"
        return
    fi

    if [[ -z "$second" ]]; then
        printf '%s\n' "$first"
        return
    fi

    printf '%s\n' "$first" "$second" | sort -V | tail -n1
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

matching_tags_for_product() {
    local product="$1"
    local canonical_prefix
    local legacy_prefix

    canonical_prefix="$(matrix_field "$product" canonical_tag_prefix)"
    git tag --list "${canonical_prefix}*"

    while IFS= read -r legacy_prefix; do
        [[ -n "$legacy_prefix" ]] || continue
        git tag --list "${legacy_prefix}*"
    done < <(matrix_field "$product" accepted_legacy_prefixes)
}

tag_version_for_product() {
    local product="$1"
    local tag="$2"
    local canonical_prefix
    local legacy_prefix

    canonical_prefix="$(matrix_field "$product" canonical_tag_prefix)"
    if [[ "$tag" == "${canonical_prefix}"* ]]; then
        printf '%s\n' "${tag#"$canonical_prefix"}"
        return 0
    fi

    while IFS= read -r legacy_prefix; do
        [[ -n "$legacy_prefix" ]] || continue
        if [[ "$tag" == "${legacy_prefix}"* ]]; then
            printf '%s\n' "${tag#"$legacy_prefix"}"
            return 0
        fi
    done < <(matrix_field "$product" accepted_legacy_prefixes)

    return 1
}

history_baseline_version() {
    local product="$1"
    local -a tags=()

    mapfile -t tags < <(matching_tags_for_product "$product")
    if [[ ${#tags[@]} -eq 0 ]]; then
        printf '%s\n' '0.0.0'
        return
    fi

    bash "$MATRIX" latest-version-from-list "$product" "${tags[@]}"
}

history_baseline_tag() {
    local product="$1"
    local history_version="$2"
    local canonical_prefix
    local -a tags=()
    local tag
    local version

    mapfile -t tags < <(matching_tags_for_product "$product")
    if [[ ${#tags[@]} -eq 0 ]]; then
        return 1
    fi

    canonical_prefix="$(matrix_field "$product" canonical_tag_prefix)"

    for tag in "${tags[@]}"; do
        version="$(tag_version_for_product "$product" "$tag")" || continue
        if [[ "$version" == "$history_version" && "$tag" == "${canonical_prefix}"* ]]; then
            printf '%s\n' "$tag"
            return 0
        fi
    done

    for tag in "${tags[@]}"; do
        version="$(tag_version_for_product "$product" "$tag")" || continue
        if [[ "$version" == "$history_version" ]]; then
            printf '%s\n' "$tag"
            return 0
        fi
    done

    return 1
}

commits_for_product() {
    local product="$1"
    local since_ref="$2"
    local -a scope_paths=()

    mapfile -t scope_paths < <(matrix_field "$product" bump_scope_paths)

    if [[ -n "$since_ref" ]]; then
        git log "${since_ref}..HEAD" --oneline -- "${scope_paths[@]}" 2>/dev/null || true
    else
        git log --oneline -- "${scope_paths[@]}" 2>/dev/null || true
    fi
}

determine_bump() {
    local product="$1"
    local since_ref="$2"
    local level="none"
    local commits
    local line
    local subject

    commits="$(commits_for_product "$product" "$since_ref")"
    if [[ -z "$commits" ]]; then
        printf '%s\n' 'none'
        return
    fi

    while IFS= read -r line; do
        [[ -n "$line" ]] || continue
        subject="${line#* }"

        if [[ "$subject" =~ ^(feat|fix|refactor|perf)(\(.+\))?!: ]]; then
            printf '%s\n' 'major'
            return
        fi

        if [[ "$subject" =~ ^feat(\(.+\))?: ]] && [[ "$level" != "major" ]]; then
            level="minor"
        fi

        if [[ "$subject" =~ ^(fix|security|perf)(\(.+\))?: ]] && [[ "$level" == "none" ]]; then
            level="patch"
        fi
    done <<< "$commits"

    printf '%s\n' "$level"
}

generate_changelog() {
    local product="$1"
    local since_ref="$2"
    local new_version="$3"
    local tag_name
    local date
    local line
    local subject
    local -a features=()
    local -a fixes=()
    local -a security=()
    local -a perf=()
    local -a other=()

    date="$(date +%Y-%m-%d)"
    tag_name="$(bash "$MATRIX" canonical-tag "$product" "$new_version")"

    {
        printf '\n'
        printf '## [%s] - %s\n\n' "$tag_name" "$date"

        while IFS= read -r line; do
            [[ -n "$line" ]] || continue
            subject="${line#* }"

            if [[ "$subject" =~ ^feat!?: ]]; then
                features+=("- ${subject#*: }")
            elif [[ "$subject" =~ ^fix: ]]; then
                fixes+=("- ${subject#*: }")
            elif [[ "$subject" =~ ^security: ]]; then
                security+=("- ${subject#*: }")
            elif [[ "$subject" =~ ^perf: ]]; then
                perf+=("- ${subject#*: }")
            elif [[ "$subject" =~ ^(refactor|test|chore|docs): ]]; then
                :
            else
                other+=("- ${subject}")
            fi
        done < <(commits_for_product "$product" "$since_ref")

        if [[ ${#features[@]} -gt 0 ]]; then
            printf '### Added\n'
            printf '%s\n' "${features[@]}"
            printf '\n'
        fi
        if [[ ${#fixes[@]} -gt 0 ]]; then
            printf '### Fixed\n'
            printf '%s\n' "${fixes[@]}"
            printf '\n'
        fi
        if [[ ${#security[@]} -gt 0 ]]; then
            printf '### Security\n'
            printf '%s\n' "${security[@]}"
            printf '\n'
        fi
        if [[ ${#perf[@]} -gt 0 ]]; then
            printf '### Performance\n'
            printf '%s\n' "${perf[@]}"
            printf '\n'
        fi
        if [[ ${#other[@]} -gt 0 ]]; then
            printf '### Other\n'
            printf '%s\n' "${other[@]}"
            printf '\n'
        fi
    }
}

update_cargo_version() {
    local file="$1"
    local new_version="$2"
    sed -i "0,/^version = \".*\"/s/^version = \".*\"/version = \"${new_version}\"/" "$file"
}

has_owned_path() {
    local needle="$1"
    shift
    local path

    for path in "$@"; do
        if [[ "$path" == "$needle" ]]; then
            return 0
        fi
    done

    return 1
}

update_packaging_versions() {
    local new_version="$1"
    shift
    local -a owned_paths=("$@")
    local deb_date
    local tmp

    deb_date="$(date -R)"

    if has_owned_path "packaging/rpm/conary.spec" "${owned_paths[@]}" && [[ -f packaging/rpm/conary.spec ]]; then
        sed -i "s/^Version:.*$/Version:        ${new_version}/" packaging/rpm/conary.spec
        printf '  Updated packaging/rpm/conary.spec\n'
    fi

    if has_owned_path "packaging/arch/PKGBUILD" "${owned_paths[@]}" && [[ -f packaging/arch/PKGBUILD ]]; then
        sed -i "s/^pkgver=.*$/pkgver=${new_version}/" packaging/arch/PKGBUILD
        printf '  Updated packaging/arch/PKGBUILD\n'
    fi

    if has_owned_path "packaging/deb/debian/changelog" "${owned_paths[@]}" && [[ -f packaging/deb/debian/changelog ]]; then
        tmp="$(mktemp)"
        cat > "$tmp" <<DEBEOF
conary (${new_version}-1) unstable; urgency=medium

  * Release ${new_version}

 -- Conary Contributors <contributors@conary.io>  ${deb_date}

DEBEOF
        cat packaging/deb/debian/changelog >> "$tmp"
        mv "$tmp" packaging/deb/debian/changelog
        printf '  Updated packaging/deb/debian/changelog\n'
    fi

    if has_owned_path "packaging/ccs/ccs.toml" "${owned_paths[@]}" && [[ -f packaging/ccs/ccs.toml ]]; then
        sed -i "s/^version = \".*\"/version = \"${new_version}\"/" packaging/ccs/ccs.toml
        printf '  Updated packaging/ccs/ccs.toml\n'
    fi
}

print_owned_paths() {
    local -a owned_paths=("$@")
    local path

    printf '  Owned manifests:\n'
    for path in "${owned_paths[@]}"; do
        printf '    - %s\n' "$path"
    done
}

stage_release_files() {
    local -a files=("$@")

    files+=("Cargo.lock")
    if [[ -f CHANGELOG.md ]]; then
        files+=("CHANGELOG.md")
    fi

    git add -- "${files[@]}"
}

main() {
    local DRY_RUN=false
    local -a RELEASE_GROUPS=()
    local arg

    for arg in "$@"; do
        case "$arg" in
            --dry-run)
                DRY_RUN=true
                ;;
            all)
                RELEASE_GROUPS=("${PRODUCTS[@]}")
                ;;
            *)
                if is_product "$arg"; then
                    RELEASE_GROUPS+=("$arg")
                else
                    usage
                fi
                ;;
        esac
    done

    [[ ${#RELEASE_GROUPS[@]} -gt 0 ]] || usage

    local product
    for product in "${RELEASE_GROUPS[@]}"; do
        local local_history_tag=""
        local history_version=""
        local manifest_version=""
        local current_version=""
        local current_tag=""
        local level=""
        local new_version=""
        local new_tag=""
        local bundle_name=""
        local deploy_mode=""
        local previous_tags_display=""
        local canonical_prefix=""
        local changelog_entry=""
        local tmp=""
        local -a owned_paths=()
        local -a previous_tags=()
        local owned_path

        printf '=== Releasing: %s ===\n' "$product"

        canonical_prefix="$(matrix_field "$product" canonical_tag_prefix)"
        bundle_name="$(matrix_field "$product" bundle_name)"
        deploy_mode="$(matrix_field "$product" deploy_mode)"
        mapfile -t owned_paths < <(bash "$MATRIX" owned-paths "$product")
        mapfile -t previous_tags < <(matching_tags_for_product "$product")

        history_version="$(history_baseline_version "$product")"
        if local_history_tag="$(history_baseline_tag "$product" "$history_version" 2>/dev/null)"; then
            :
        else
            local_history_tag=""
        fi
        manifest_version="$(bash "$MATRIX" max-owned-version "$product")"
        current_version="$(version_max "$history_version" "$manifest_version")"
        current_tag="${canonical_prefix}${current_version}"

        if [[ ${#previous_tags[@]} -gt 0 ]]; then
            previous_tags_display="$(join_by ', ' "${previous_tags[@]}")"
        else
            previous_tags_display="none"
        fi

        printf '  Previous tags considered: %s\n' "$previous_tags_display"
        printf '  History baseline: %s\n' "$history_version"
        printf '  Owned manifest baseline: %s\n' "$manifest_version"
        printf '  Current: %s\n' "$current_tag"

        level="$(determine_bump "$product" "$local_history_tag")"
        if [[ "$level" == "none" ]]; then
            if [[ -n "$local_history_tag" ]]; then
                printf '  No version-bumping commits since %s. Skipping.\n' "$local_history_tag"
            else
                printf '  No version-bumping commits found in product scope. Skipping.\n'
            fi
            print_owned_paths "${owned_paths[@]}"
            printf '  Bundle: %s\n' "$bundle_name"
            printf '  Deploy mode: %s\n' "$deploy_mode"
            printf '\n'
            continue
        fi

        new_version="$(bump_version "$current_version" "$level")"
        if version_lt "$new_version" "$manifest_version"; then
            die "computed release target ${new_version} would be lower than owned manifest version ${manifest_version} for ${product}"
        fi

        new_tag="$(bash "$MATRIX" canonical-tag "$product" "$new_version")"

        printf '  Next version: %s\n' "$new_version"
        printf '  Tag: %s\n' "$new_tag"
        print_owned_paths "${owned_paths[@]}"
        printf '  Bundle: %s\n' "$bundle_name"
        printf '  Deploy mode: %s\n' "$deploy_mode"

        if [[ "$DRY_RUN" == "true" ]]; then
            printf '\n'
            generate_changelog "$product" "$local_history_tag" "$new_version"
            continue
        fi

        for owned_path in "${owned_paths[@]}"; do
            case "$owned_path" in
                */Cargo.toml)
                    update_cargo_version "$owned_path" "$new_version"
                    printf '  Updated %s\n' "$owned_path"
                    ;;
            esac
        done

        if [[ "$product" == "conary" ]]; then
            update_packaging_versions "$new_version" "${owned_paths[@]}"
        fi

        case "${CONARY_RELEASE_LOCKFILE_MODE:-offline}" in
            offline)
                cargo update --workspace --offline --quiet
                ;;
            online)
                cargo update --workspace --quiet
                ;;
            *)
                die "unknown CONARY_RELEASE_LOCKFILE_MODE: ${CONARY_RELEASE_LOCKFILE_MODE}"
                ;;
        esac
        printf '  Updated Cargo.lock\n'

        changelog_entry="$(generate_changelog "$product" "$local_history_tag" "$new_version")"
        if [[ -f CHANGELOG.md ]]; then
            tmp="$(mktemp)"
            head -5 CHANGELOG.md > "$tmp"
            printf '%s' "$changelog_entry" >> "$tmp"
            tail -n +6 CHANGELOG.md >> "$tmp"
            mv "$tmp" CHANGELOG.md
        fi

        stage_release_files "${owned_paths[@]}"
        git commit -m "chore: release ${new_tag}"
        git tag -a "$new_tag" -m "Release ${new_tag}"

        printf '  [DONE] Released %s\n\n' "$new_tag"
    done

    printf '=== Release complete ===\n'
}

main "$@"
