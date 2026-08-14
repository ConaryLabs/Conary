#!/usr/bin/env bash
# scripts/release-matrix.sh
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

RELEASE_UNITS=(suite)
ARTIFACT_PRODUCTS=(conary remi conaryd conary-test)

usage() {
    cat <<'EOF'
Usage:
  scripts/release-matrix.sh release-units
  scripts/release-matrix.sh artifacts
  scripts/release-matrix.sh field <release> <field>
  scripts/release-matrix.sh artifact-field <product> <field>
  scripts/release-matrix.sh resolve-tag <tag> [--format shell|json]
  scripts/release-matrix.sh canonical-tag <release> <version>
  scripts/release-matrix.sh latest-version-from-list <release> <tag...>
  scripts/release-matrix.sh latest-version-from-git <release>
  scripts/release-matrix.sh max-owned-version <release>
  scripts/release-matrix.sh assert-owned-version <release> <version>
  scripts/release-matrix.sh owned-paths <release>
  scripts/release-matrix.sh metadata-json <release> <version> <tag> <dry_run>
EOF
    exit 1
}

die() {
    printf '%s\n' "$1" >&2
    exit 1
}

is_release_unit() {
    [[ "$1" == "suite" ]]
}

is_artifact_product() {
    case "$1" in
        conary|remi|conaryd|conary-test) return 0 ;;
        *) return 1 ;;
    esac
}

is_release_version() {
    [[ "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]
}

workspace_member_manifests() {
    printf '%s\n' \
        'apps/conary/Cargo.toml' \
        'apps/remi/Cargo.toml' \
        'apps/conaryd/Cargo.toml' \
        'apps/conary-test/Cargo.toml' \
        'crates/conary-bootstrap/Cargo.toml' \
        'crates/conary-agent-contract/Cargo.toml' \
        'crates/conary-mcp/Cargo.toml' \
        'crates/conary-core/Cargo.toml'
}

version_authority_files() {
    printf '%s\n' \
        'Cargo.toml' \
        'packaging/rpm/conary.spec' \
        'packaging/arch/PKGBUILD' \
        'packaging/deb/debian/changelog' \
        'packaging/ccs/ccs.toml'
}

release_owned_paths() {
    version_authority_files
    workspace_member_manifests
}

release_bump_scope_paths() {
    printf '%s\n' \
        'apps/' \
        'crates/' \
        'packaging/' \
        'deploy/' \
        'scripts/' \
        '.github/workflows/' \
        '.github/actions/'
}

canonical_tag_prefix_for() {
    [[ "$1" == "suite" ]] || return 1
    printf '%s\n' 'v'
}

release_bundle_name_for() {
    [[ "$1" == "suite" ]] || return 1
    printf '%s\n' 'suite-bundle'
}

release_deploy_mode_for() {
    [[ "$1" == "suite" ]] || return 1
    printf '%s\n' 'suite'
}

artifact_bundle_name_for() {
    case "$1" in
        conary) printf '%s\n' 'release-bundle' ;;
        remi) printf '%s\n' 'remi-bundle' ;;
        conaryd) printf '%s\n' 'conaryd-bundle' ;;
        conary-test) printf '%s\n' 'conary-test-bundle' ;;
        *) return 1 ;;
    esac
}

artifact_deploy_mode_for() {
    case "$1" in
        conary) printf '%s\n' 'release_bundle' ;;
        remi) printf '%s\n' 'remote_bundle' ;;
        conaryd|conary-test) printf '%s\n' 'none' ;;
        *) return 1 ;;
    esac
}

artifact_patterns_for() {
    case "$1" in
        conary)
            printf '%s\n' \
                'conary-<version>.ccs' \
                'conary-<version>.ccs.sig' \
                'conary-<version>-1.fc44.x86_64.rpm' \
                'conary_<version>-1_amd64.deb' \
                'conary-<version>-1-x86_64.pkg.tar.zst'
            ;;
        remi)
            printf '%s\n' \
                'remi-<version>-linux-x64' \
                'remi-<version>-linux-x64.tar.gz'
            ;;
        conaryd)
            printf '%s\n' \
                'conaryd-<version>-linux-x64' \
                'conaryd-<version>-linux-x64.tar.gz'
            ;;
        conary-test)
            printf '%s\n' \
                'conary-test-<version>-linux-x64' \
                'conary-test-<version>-linux-x64.tar.gz'
            ;;
        *) return 1 ;;
    esac
}

all_artifact_patterns() {
    local product
    for product in "${ARTIFACT_PRODUCTS[@]}"; do
        artifact_patterns_for "$product"
    done
}

release_field_value() {
    local release="$1"
    local field="$2"

    case "$field" in
        canonical_tag_prefix) canonical_tag_prefix_for "$release" ;;
        bundle_name) release_bundle_name_for "$release" ;;
        deploy_mode) release_deploy_mode_for "$release" ;;
        version_authority_files) version_authority_files ;;
        workspace_member_manifests) workspace_member_manifests ;;
        bump_scope_paths) release_bump_scope_paths ;;
        primary_artifact_patterns) all_artifact_patterns ;;
        *) die "unknown release field: $field" ;;
    esac
}

artifact_field_value() {
    local product="$1"
    local field="$2"

    case "$field" in
        bundle_name) artifact_bundle_name_for "$product" ;;
        deploy_mode) artifact_deploy_mode_for "$product" ;;
        primary_artifact_patterns) artifact_patterns_for "$product" ;;
        *) die "unknown artifact field: $field" ;;
    esac
}

print_json_string() {
    local value="${1//\\/\\\\}"
    value="${value//\"/\\\"}"
    printf '"%s"' "$value"
}

json_array_from_lines() {
    local first=true
    printf '['
    while IFS= read -r line; do
        [[ -n "$line" ]] || continue
        if [[ "$first" == true ]]; then
            first=false
        else
            printf ','
        fi
        print_json_string "$line"
    done
    printf ']'
}

resolve_tag_version() {
    local tag="$1"
    if [[ "$tag" =~ ^v([0-9]+\.[0-9]+\.[0-9]+)$ ]]; then
        printf '%s\n' "${BASH_REMATCH[1]}"
        return
    fi
    die "unknown current release tag: $tag"
}

tag_version_for_release() {
    local release="$1"
    local tag="$2"
    [[ "$release" == "suite" ]] || return 1
    [[ "$tag" =~ ^v([0-9]+\.[0-9]+\.[0-9]+)$ ]] || return 1
    printf '%s\n' "${BASH_REMATCH[1]}"
}

latest_version_from_list() {
    local release="$1"
    shift

    local -a versions=()
    local tag version
    for tag in "$@"; do
        if version="$(tag_version_for_release "$release" "$tag")"; then
            versions+=("$version")
        fi
    done

    [[ ${#versions[@]} -gt 0 ]] || die "no matching tags found for release: $release"
    printf '%s\n' "${versions[@]}" | sort -V | tail -n1
}

latest_version_from_git() {
    local release="$1"
    local -a tags=()
    mapfile -t tags < <(git -C "$REPO_ROOT" tag --list)
    latest_version_from_list "$release" "${tags[@]}"
}

extract_version_from_authority_file() {
    local file="$1"
    local version=""

    case "$file" in
        Cargo.toml)
            version="$({
                awk '
                    /^\[workspace\.package\]$/ { in_workspace_package = 1; next }
                    in_workspace_package && /^\[/ { exit }
                    in_workspace_package && /^version = "/ {
                        value = $0
                        sub(/^version = "/, "", value)
                        sub(/".*$/, "", value)
                        print value
                        exit
                    }
                ' "$file"
            } || true)"
            ;;
        packaging/rpm/*.spec|*.spec)
            version="$(sed -n 's/^Version:[[:space:]]*\(.*\)$/\1/p' "$file" | head -n1 | tr -d '[:space:]')"
            ;;
        packaging/arch/PKGBUILD|*/PKGBUILD)
            version="$(sed -n 's/^pkgver=\(.*\)$/\1/p' "$file" | head -n1 | tr -d '[:space:]')"
            ;;
        packaging/deb/debian/changelog|*/debian/changelog)
            version="$(sed -n '1s/^[^(]*(\([^)]*\)-[0-9][^)]*) .*/\1/p' "$file" | head -n1 | tr -d '[:space:]')"
            ;;
        *)
            version="$(sed -n 's/^version = "\([^"]*\)".*/\1/p' "$file" | head -n1)"
            ;;
    esac

    [[ -n "$version" ]] || die "could not extract version from $file"
    printf '%s\n' "$version"
}

max_owned_version() {
    local release="$1"
    local -a versions=()
    local file

    [[ "$release" == "suite" ]] || die "unknown release: $release"
    while IFS= read -r file; do
        [[ -f "$file" ]] || die "version authority file missing: $file"
        versions+=("$(extract_version_from_authority_file "$file")")
    done < <(version_authority_files)

    printf '%s\n' "${versions[@]}" | sort -V | tail -n1
}

assert_owned_version() {
    local release="$1"
    local expected_version="$2"
    local file actual_version field

    [[ "$release" == "suite" ]] || die "unknown release: $release"
    is_release_version "$expected_version" || die "invalid release version: $expected_version"

    while IFS= read -r file; do
        [[ -f "$file" ]] || die "version authority file missing: $file"
        actual_version="$(extract_version_from_authority_file "$file")"
        [[ "$actual_version" == "$expected_version" ]] ||
            die "suite version mismatch: $file is $actual_version, expected $expected_version"
    done < <(version_authority_files)

    while IFS= read -r file; do
        [[ -f "$file" ]] || die "workspace package manifest missing: $file"
        for field in version edition rust-version authors license; do
            grep -Fxq "${field}.workspace = true" "$file" ||
                die "workspace package $field is not inherited from [workspace.package]: $file"
            if grep -Eq "^${field}[[:space:]]*=" "$file"; then
                die "workspace package retains independent $field authority: $file"
            fi
        done
    done < <(workspace_member_manifests)
}

metadata_json() {
    local release="$1"
    local version="$2"
    local tag="$3"
    local dry_run="$4"
    local expected_tag product first=true

    [[ "$release" == "suite" ]] || die "unknown release: $release"
    is_release_version "$version" || die "invalid release version: $version"
    expected_tag="$(canonical_tag_prefix_for "$release")${version}"
    [[ "$tag" == "$expected_tag" ]] || die "tag $tag does not match suite version $version"
    [[ "$dry_run" == "true" || "$dry_run" == "false" ]] || die "dry_run must be true or false"

    printf '{"schema_version":1'
    printf ',"release":'; print_json_string "$release"
    printf ',"canonical_tag_prefix":'; print_json_string "$(canonical_tag_prefix_for "$release")"
    printf ',"tag_name":'; print_json_string "$tag"
    printf ',"version":'; print_json_string "$version"
    printf ',"bundle_name":'; print_json_string "$(release_bundle_name_for "$release")"
    printf ',"deploy_mode":'; print_json_string "$(release_deploy_mode_for "$release")"
    printf ',"artifact_patterns":'; all_artifact_patterns | json_array_from_lines
    printf ',"artifacts":['
    for product in "${ARTIFACT_PRODUCTS[@]}"; do
        if [[ "$first" == true ]]; then
            first=false
        else
            printf ','
        fi
        printf '{"product":'; print_json_string "$product"
        printf ',"bundle_name":'; print_json_string "$(artifact_bundle_name_for "$product")"
        printf ',"deploy_mode":'; print_json_string "$(artifact_deploy_mode_for "$product")"
        printf ',"artifact_patterns":'; artifact_patterns_for "$product" | json_array_from_lines
        printf '}'
    done
    printf ']'
    printf ',"dry_run":%s' "$dry_run"
    printf '}\n'
}

resolve_tag_cmd() {
    local tag="$1"
    local format="shell"
    local version

    while [[ $# -gt 1 ]]; do
        shift
        case "$1" in
            --format)
                shift
                [[ $# -gt 0 ]] || die "resolve-tag requires a format after --format"
                format="$1"
                ;;
            *) die "unknown resolve-tag option: $1" ;;
        esac
    done

    version="$(resolve_tag_version "$tag")"
    case "$format" in
        shell)
            printf 'release=suite\n'
            printf 'canonical_tag_prefix=v\n'
            printf 'tag_name=%s\n' "$tag"
            printf 'version=%s\n' "$version"
            printf 'bundle_name=suite-bundle\n'
            printf 'deploy_mode=suite\n'
            ;;
        json)
            printf '{"release":"suite","canonical_tag_prefix":"v","tag_name":'
            print_json_string "$tag"
            printf ',"version":'; print_json_string "$version"
            printf ',"bundle_name":"suite-bundle","deploy_mode":"suite"}\n'
            ;;
        *) die "unknown format: $format" ;;
    esac
}

main() {
    [[ $# -ge 1 ]] || usage
    local command="$1"
    shift

    case "$command" in
        release-units)
            [[ $# -eq 0 ]] || usage
            printf '%s\n' "${RELEASE_UNITS[@]}"
            ;;
        artifacts)
            [[ $# -eq 0 ]] || usage
            printf '%s\n' "${ARTIFACT_PRODUCTS[@]}"
            ;;
        field)
            [[ $# -eq 2 ]] || usage
            is_release_unit "$1" || die "unknown release: $1"
            release_field_value "$1" "$2"
            ;;
        artifact-field)
            [[ $# -eq 2 ]] || usage
            is_artifact_product "$1" || die "unknown artifact product: $1"
            artifact_field_value "$1" "$2"
            ;;
        resolve-tag)
            [[ $# -ge 1 ]] || usage
            resolve_tag_cmd "$@"
            ;;
        canonical-tag)
            [[ $# -eq 2 ]] || usage
            is_release_unit "$1" || die "unknown release: $1"
            is_release_version "$2" || die "invalid release version: $2"
            printf '%s%s\n' "$(canonical_tag_prefix_for "$1")" "$2"
            ;;
        latest-version-from-list)
            [[ $# -ge 2 ]] || usage
            is_release_unit "$1" || die "unknown release: $1"
            latest_version_from_list "$@"
            ;;
        latest-version-from-git)
            [[ $# -eq 1 ]] || usage
            is_release_unit "$1" || die "unknown release: $1"
            latest_version_from_git "$1"
            ;;
        max-owned-version)
            [[ $# -eq 1 ]] || usage
            max_owned_version "$1"
            ;;
        assert-owned-version)
            [[ $# -eq 2 ]] || usage
            assert_owned_version "$1" "$2"
            ;;
        owned-paths)
            [[ $# -eq 1 ]] || usage
            is_release_unit "$1" || die "unknown release: $1"
            release_owned_paths
            ;;
        metadata-json)
            [[ $# -eq 4 ]] || usage
            metadata_json "$@"
            ;;
        *) usage ;;
    esac
}

main "$@"
