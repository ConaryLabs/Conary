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
Usage: scripts/release.sh [conary|remi|conaryd|conary-test|all] [OPTIONS]

Analyze conventional commits since the latest product release tag and bump versions.
  conary       - conary CLI + owned crates + packaging
  remi         - Remi service app
  conaryd      - daemon service app
  conary-test  - integration harness + conary-mcp
  all          - all release tracks

Options:
  --dry-run                  Show what would happen without making changes.
  --prepare-only             Update and stage release files without committing or tagging.
  --target PRODUCT=VERSION   Use an explicit exact target for one selected product.
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

is_release_version() {
    [[ "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]
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

    canonical_prefix="$(matrix_field "$product" canonical_tag_prefix)"
    git tag --list "${canonical_prefix}*"
}

tag_version_for_product() {
    local product="$1"
    local tag="$2"
    local canonical_prefix

    canonical_prefix="$(matrix_field "$product" canonical_tag_prefix)"
    if [[ "$tag" == "${canonical_prefix}"* ]]; then
        printf '%s\n' "${tag#"$canonical_prefix"}"
        return 0
    fi

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
    local description
    local -a features=()
    local -a changed=()
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
            elif [[ "$subject" =~ ^(test|chore|docs)(\(.+\))?!?: ]]; then
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
        if [[ ${#changed[@]} -gt 0 ]]; then
            printf '### Changed\n'
            printf '%s\n' "${changed[@]}"
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

regenerate_conary_man_page() {
    local new_version="$1"
    local build_script="apps/conary/build.rs"
    local man_page="apps/conary/man/conary.1"

    [[ -f "$build_script" ]] || die "missing Conary build script: ${build_script}"

    # The package-version edit already invalidates Cargo's package fingerprint.
    # Touching the build script additionally guarantees that Cargo reruns it even
    # when a release is prepared in a reused target directory.
    touch "$build_script"
    cargo build -p conary --bin conary --quiet

    [[ -s "$man_page" ]] || die "Conary man-page generation did not produce ${man_page}"
    # Keep the tracked release artifact diff-clean when the roff generator pads
    # a field with spaces or tabs.
    sed -i 's/[[:blank:]]\+$//' "$man_page"
    grep -Fq -- "conary ${new_version}" "$man_page" ||
        die "generated ${man_page} does not contain Conary version ${new_version}"

    printf '  Regenerated %s for %s\n' "$man_page" "$new_version"
}

main() {
    local DRY_RUN=false
    local PREPARE_ONLY=false
    local -a RELEASE_GROUPS=()
    local -A TARGET_VERSIONS=()
    local arg
    local target_product

    append_release_group() {
        local candidate="$1"
        local existing

        for existing in "${RELEASE_GROUPS[@]}"; do
            if [[ "$existing" == "$candidate" ]]; then
                return
            fi
        done
        RELEASE_GROUPS+=("$candidate")
    }

    register_target() {
        local spec="$1"
        local product="${spec%%=*}"
        local version="${spec#*=}"

        if [[ "$product" == "$spec" || -z "$product" || -z "$version" ]]; then
            die "release target must use PRODUCT=VERSION"
        fi
        is_product "$product" || die "unknown release target product: $product"
        is_release_version "$version" ||
            die "release target for ${product} must be an exact MAJOR.MINOR.PATCH version"
        if [[ -n "${TARGET_VERSIONS[$product]:-}" && "${TARGET_VERSIONS[$product]}" != "$version" ]]; then
            die "conflicting release targets for ${product}"
        fi
        TARGET_VERSIONS["$product"]="$version"
    }

    while [[ $# -gt 0 ]]; do
        arg="$1"
        case "$arg" in
            --dry-run)
                DRY_RUN=true
                ;;
            --prepare-only)
                PREPARE_ONLY=true
                ;;
            --target)
                shift
                [[ $# -gt 0 ]] || die "--target requires PRODUCT=VERSION"
                register_target "$1"
                ;;
            --target=*)
                register_target "${arg#--target=}"
                ;;
            all)
                for target_product in "${PRODUCTS[@]}"; do
                    append_release_group "$target_product"
                done
                ;;
            *)
                if is_product "$arg"; then
                    append_release_group "$arg"
                else
                    usage
                fi
                ;;
        esac
        shift
    done

    [[ ${#RELEASE_GROUPS[@]} -gt 0 ]] || usage
    if [[ "$DRY_RUN" == "true" && "$PREPARE_ONLY" == "true" ]]; then
        die "--dry-run and --prepare-only cannot be combined"
    fi

    for target_product in "${!TARGET_VERSIONS[@]}"; do
        local selected=false
        local release_group
        for release_group in "${RELEASE_GROUPS[@]}"; do
            if [[ "$release_group" == "$target_product" ]]; then
                selected=true
                break
            fi
        done
        [[ "$selected" == "true" ]] ||
            die "release target provided for unselected product: ${target_product}"
    done

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

        if [[ -n "${TARGET_VERSIONS[$product]:-}" ]]; then
            new_version="${TARGET_VERSIONS[$product]}"
            version_lt "$history_version" "$new_version" ||
                die "explicit release target ${new_version} must be greater than published history baseline ${history_version} for ${product}"
            if [[ -n "$local_history_tag" && -z "$(commits_for_product "$product" "$local_history_tag")" ]]; then
                die "explicit release target for ${product} has no scoped changes since ${local_history_tag}"
            fi
            level="explicit"
            printf '  Target authority: explicit\n'
        else
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
            printf '  Target authority: conventional commits (%s)\n' "$level"
        fi

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

        if [[ "$product" == "conary" ]]; then
            regenerate_conary_man_page "$new_version"
        fi

        changelog_entry="$(generate_changelog "$product" "$local_history_tag" "$new_version")"
        if [[ -f CHANGELOG.md ]]; then
            tmp="$(mktemp)"
            head -5 CHANGELOG.md > "$tmp"
            printf '%s' "$changelog_entry" >> "$tmp"
            tail -n +6 CHANGELOG.md >> "$tmp"
            mv "$tmp" CHANGELOG.md
        fi

        stage_release_files "${owned_paths[@]}"
        if [[ "$product" == "conary" ]]; then
            # Generated man pages are intentionally ignored between releases.
            # Force-add this one exact artifact so the release tag carries the
            # CLI surface generated for the version being published.
            git add -f -- apps/conary/man/conary.1
        fi
        if [[ "$PREPARE_ONLY" == "true" ]]; then
            printf '  [PREPARED] Updated %s without commit or tag\n\n' "$new_tag"
            continue
        fi
        git commit -m "chore: release ${new_tag}"
        git tag -a "$new_tag" -m "Release ${new_tag}"

        printf '  [DONE] Released %s\n\n' "$new_tag"
    done

    printf '=== Release complete ===\n'
}

main "$@"
