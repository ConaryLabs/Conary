#!/usr/bin/env bash
# scripts/release-matrix.sh
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

PRODUCTS=(
    conary
    remi
    conaryd
    conary-test
)

usage() {
    cat <<'EOF'
Usage:
  scripts/release-matrix.sh products
  scripts/release-matrix.sh field <product> <field>
  scripts/release-matrix.sh resolve-tag <tag> [--format shell|json]
  scripts/release-matrix.sh canonical-tag <product> <version>
  scripts/release-matrix.sh latest-version-from-list <product> <tag...>
  scripts/release-matrix.sh latest-version-from-git <product>
  scripts/release-matrix.sh max-owned-version <product>
  scripts/release-matrix.sh owned-paths <product>
  scripts/release-matrix.sh metadata-json <product> <version> <tag> <dry_run>
EOF
    exit 1
}

die() {
    printf '%s\n' "$1" >&2
    exit 1
}

is_product() {
    case "$1" in
        conary|remi|conaryd|conary-test) return 0 ;;
        *) return 1 ;;
    esac
}

canonical_tag_prefix_for() {
    case "$1" in
        conary) printf '%s\n' 'v' ;;
        remi) printf '%s\n' 'remi-v' ;;
        conaryd) printf '%s\n' 'conaryd-v' ;;
        conary-test) printf '%s\n' 'conary-test-v' ;;
        *) return 1 ;;
    esac
}

bundle_name_for() {
    case "$1" in
        conary) printf '%s\n' 'release-bundle' ;;
        remi) printf '%s\n' 'remi-bundle' ;;
        conaryd) printf '%s\n' 'conaryd-bundle' ;;
        conary-test) printf '%s\n' 'conary-test-bundle' ;;
        *) return 1 ;;
    esac
}

deploy_mode_for() {
    case "$1" in
        conary) printf '%s\n' 'release_bundle' ;;
        remi) printf '%s\n' 'remote_bundle' ;;
        conaryd) printf '%s\n' 'remote_bundle' ;;
        conary-test) printf '%s\n' 'none' ;;
        *) return 1 ;;
    esac
}

accepted_legacy_prefixes_for() {
    case "$1" in
        conary) return 0 ;;
        remi) printf '%s\n' 'server-v' ;;
        conaryd) return 0 ;;
        conary-test) printf '%s\n' 'test-v' ;;
        *) return 1 ;;
    esac
}

version_owned_manifests_for() {
    case "$1" in
        conary)
            printf '%s\n' \
                'apps/conary/Cargo.toml' \
                'crates/conary-core/Cargo.toml' \
                'crates/conary-bootstrap/Cargo.toml' \
                'packaging/rpm/conary.spec' \
                'packaging/arch/PKGBUILD' \
                'packaging/deb/debian/changelog' \
                'packaging/ccs/ccs.toml'
            ;;
        remi)
            printf '%s\n' 'apps/remi/Cargo.toml'
            ;;
        conaryd)
            printf '%s\n' 'apps/conaryd/Cargo.toml'
            ;;
        conary-test)
            printf '%s\n' \
                'apps/conary-test/Cargo.toml' \
                'crates/conary-mcp/Cargo.toml'
            ;;
        *) return 1 ;;
    esac
}

bump_scope_paths_for() {
    case "$1" in
        conary)
            printf '%s\n' \
                'apps/conary/' \
                'crates/conary-core/' \
                'crates/conary-bootstrap/' \
                'packaging/' \
                '.github/workflows/release-build.yml' \
                '.github/workflows/deploy-and-verify.yml' \
                'scripts/' \
                'deploy/'
            ;;
        remi)
            printf '%s\n' \
                'apps/remi/' \
                'crates/conary-core/' \
                'crates/conary-bootstrap/' \
                'crates/conary-mcp/' \
                'deploy/' \
                'scripts/rebuild-remi.sh' \
                'scripts/bootstrap-remi.sh' \
                '.github/workflows/release-build.yml' \
                '.github/workflows/deploy-and-verify.yml'
            ;;
        conaryd)
            printf '%s\n' \
                'apps/conaryd/' \
                'crates/conary-core/' \
                'deploy/' \
                '.github/workflows/release-build.yml' \
                '.github/workflows/deploy-and-verify.yml'
            ;;
        conary-test)
            printf '%s\n' \
                'apps/conary-test/' \
                'crates/conary-core/' \
                'crates/conary-mcp/' \
                'scripts/test-release-matrix.sh' \
                '.github/workflows/release-build.yml' \
                '.github/workflows/deploy-and-verify.yml'
            ;;
        *) return 1 ;;
    esac
}

primary_artifact_patterns_for() {
    case "$1" in
        conary)
            printf '%s\n' \
                '*.ccs' \
                '*.rpm' \
                '*.deb' \
                '*.pkg.tar.zst'
            ;;
        remi)
            printf '%s\n' 'remi-<version>-linux-x64.tar.gz'
            ;;
        conaryd)
            printf '%s\n' 'conaryd-<version>-linux-x64.tar.gz'
            ;;
        conary-test)
            printf '%s\n' 'conary-test-<version>-linux-x64.tar.gz'
            ;;
        *) return 1 ;;
    esac
}

field_value() {
    local product="$1"
    local field="$2"

    case "$field" in
        canonical_tag_prefix) canonical_tag_prefix_for "$product" ;;
        accepted_legacy_prefixes) accepted_legacy_prefixes_for "$product" ;;
        bundle_name) bundle_name_for "$product" ;;
        deploy_mode) deploy_mode_for "$product" ;;
        version_owned_manifests) version_owned_manifests_for "$product" ;;
        bump_scope_paths) bump_scope_paths_for "$product" ;;
        primary_artifact_patterns) primary_artifact_patterns_for "$product" ;;
        *)
            die "unknown field: $field"
            ;;
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

resolve_tag_to_product() {
    local tag="$1"
    local product prefix version

    case "$tag" in
        v*)
            product=conary
            prefix='v'
            ;;
        remi-v*)
            product=remi
            prefix='remi-v'
            ;;
        conaryd-v*)
            product=conaryd
            prefix='conaryd-v'
            ;;
        conary-test-v*)
            product=conary-test
            prefix='conary-test-v'
            ;;
        server-v*)
            product=remi
            prefix='server-v'
            ;;
        test-v*)
            product=conary-test
            prefix='test-v'
            ;;
        *)
            die "unknown tag prefix: $tag"
            ;;
    esac

    version="${tag#"$prefix"}"
    [[ -n "$version" ]] || die "unknown tag prefix: $tag"

    printf '%s\t%s\t%s\n' "$product" "$prefix" "$version"
}

tag_version_for_product() {
    local product="$1"
    local tag="$2"
    local prefix version

    case "$product" in
        conary)
            case "$tag" in
                v*) prefix='v' ;;
                *) return 1 ;;
            esac
            ;;
        remi)
            case "$tag" in
                remi-v*) prefix='remi-v' ;;
                server-v*) prefix='server-v' ;;
                *) return 1 ;;
            esac
            ;;
        conaryd)
            case "$tag" in
                conaryd-v*) prefix='conaryd-v' ;;
                *) return 1 ;;
            esac
            ;;
        conary-test)
            case "$tag" in
                conary-test-v*) prefix='conary-test-v' ;;
                test-v*) prefix='test-v' ;;
                *) return 1 ;;
            esac
            ;;
        *)
            return 1
            ;;
    esac

    version="${tag#"$prefix"}"
    [[ -n "$version" ]] || return 1
    printf '%s\n' "$version"
}

latest_version_from_list() {
    local product="$1"
    shift

    local -a versions=()
    local tag version

    for tag in "$@"; do
        if version="$(tag_version_for_product "$product" "$tag")"; then
            versions+=("$version")
        fi
    done

    [[ ${#versions[@]} -gt 0 ]] || die "no matching tags found for product: $product"

    printf '%s\n' "${versions[@]}" | sort -V | tail -n1
}

latest_version_from_git() {
    local product="$1"
    local -a tags=()
    local tag

    while IFS= read -r tag; do
        [[ -n "$tag" ]] || continue
        tags+=("$tag")
    done < <(git -C "$REPO_ROOT" tag --list)

    latest_version_from_list "$product" "${tags[@]}"
}

extract_version_from_file() {
    local file="$1"
    local version=""

    case "$file" in
        *.toml|*/Cargo.toml)
            version="$(sed -n 's/^version = "\([^"]*\)".*/\1/p' "$file" | head -n1)"
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
    local product="$1"
    local -a versions=()
    local file version

    while IFS= read -r file; do
        [[ -n "$file" ]] || continue
        [[ -f "$file" ]] || die "owned manifest missing: $file"
        version="$(extract_version_from_file "$file")"
        versions+=("$version")
    done < <(version_owned_manifests_for "$product")

    [[ ${#versions[@]} -gt 0 ]] || die "no owned manifests defined for product: $product"

    printf '%s\n' "${versions[@]}" | sort -V | tail -n1
}

owned_paths() {
    version_owned_manifests_for "$1"
}

metadata_json() {
    local product="$1"
    local version="$2"
    local tag="$3"
    local dry_run="$4"
    local bundle_name deploy_mode canonical_prefix

    canonical_prefix="$(canonical_tag_prefix_for "$product")"
    bundle_name="$(bundle_name_for "$product")"
    deploy_mode="$(deploy_mode_for "$product")"

    {
        printf '{'
        printf '"product":'; print_json_string "$product"
        printf ',"canonical_tag_prefix":'; print_json_string "$canonical_prefix"
        printf ',"tag_name":'; print_json_string "$tag"
        printf ',"version":'; print_json_string "$version"
        printf ',"bundle_name":'; print_json_string "$bundle_name"
        printf ',"deploy_mode":'; print_json_string "$deploy_mode"
        printf ',"artifact_patterns":'
        primary_artifact_patterns_for "$product" | json_array_from_lines
        printf ',"dry_run":'; print_json_string "$dry_run"
        printf '}'
        printf '\n'
    }
}

resolve_tag_cmd() {
    local tag="$1"
    local format="shell"
    local product prefix version bundle_name deploy_mode

    while [[ $# -gt 1 ]]; do
        shift
        case "$1" in
            --format)
                shift
                [[ $# -gt 0 ]] || die "resolve-tag requires a format after --format"
                format="$1"
                ;;
            *)
                die "unknown resolve-tag option: $1"
                ;;
        esac
    done

    IFS=$'\t' read -r product prefix version < <(resolve_tag_to_product "$tag")
    bundle_name="$(bundle_name_for "$product")"
    deploy_mode="$(deploy_mode_for "$product")"

    case "$format" in
        shell)
            printf 'product=%s\n' "$product"
            printf 'canonical_tag_prefix=%s\n' "$(canonical_tag_prefix_for "$product")"
            printf 'tag_name=%s\n' "$tag"
            printf 'version=%s\n' "$version"
            printf 'bundle_name=%s\n' "$bundle_name"
            printf 'deploy_mode=%s\n' "$deploy_mode"
            ;;
        json)
            {
                printf '{'
                printf '"product":'; print_json_string "$product"
                printf ',"canonical_tag_prefix":'; print_json_string "$(canonical_tag_prefix_for "$product")"
                printf ',"tag_name":'; print_json_string "$tag"
                printf ',"version":'; print_json_string "$version"
                printf ',"bundle_name":'; print_json_string "$bundle_name"
                printf ',"deploy_mode":'; print_json_string "$deploy_mode"
                printf '}'
                printf '\n'
            }
            ;;
        *)
            die "unknown format: $format"
            ;;
    esac
}

field_cmd() {
    local product="$1"
    local field="$2"
    field_value "$product" "$field"
}

main() {
    [[ $# -ge 1 ]] || usage

    local command="$1"
    shift

    case "$command" in
        products)
            printf '%s\n' "${PRODUCTS[@]}"
            ;;
        field)
            [[ $# -eq 2 ]] || usage
            is_product "$1" || die "unknown product: $1"
            field_cmd "$1" "$2"
            ;;
        resolve-tag)
            [[ $# -ge 1 ]] || usage
            resolve_tag_cmd "$@"
            ;;
        canonical-tag)
            [[ $# -eq 2 ]] || usage
            is_product "$1" || die "unknown product: $1"
            printf '%s%s\n' "$(canonical_tag_prefix_for "$1")" "$2"
            ;;
        latest-version-from-list)
            [[ $# -ge 2 ]] || usage
            is_product "$1" || die "unknown product: $1"
            latest_version_from_list "$@"
            ;;
        latest-version-from-git)
            [[ $# -eq 1 ]] || usage
            is_product "$1" || die "unknown product: $1"
            latest_version_from_git "$1"
            ;;
        max-owned-version)
            [[ $# -eq 1 ]] || usage
            is_product "$1" || die "unknown product: $1"
            max_owned_version "$1"
            ;;
        owned-paths)
            [[ $# -eq 1 ]] || usage
            is_product "$1" || die "unknown product: $1"
            owned_paths "$1"
            ;;
        metadata-json)
            [[ $# -eq 4 ]] || usage
            is_product "$1" || die "unknown product: $1"
            metadata_json "$1" "$2" "$3" "$4"
            ;;
        *)
            usage
            ;;
    esac
}

main "$@"
