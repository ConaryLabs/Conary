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
  scripts/release-matrix.sh validate-version <version> [stable|nightly]
  scripts/release-matrix.sh version-channel <version>
  scripts/release-matrix.sh stable-version <version>
  scripts/release-matrix.sh render-version <version> <cargo|rpm|deb|arch|ccs|tag>
  scripts/release-matrix.sh resolve-tag <tag> [--format shell|json]
  scripts/release-matrix.sh canonical-tag <release> <version>
  scripts/release-matrix.sh latest-version-from-list <release> <tag...>
  scripts/release-matrix.sh latest-version-from-git <release>
  scripts/release-matrix.sh workspace-version
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
    release_channel_for_version "$1" >/dev/null 2>&1
}

is_stable_version() {
    [[ "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]
}

is_real_utc_date() {
    local value="$1"
    local rendered

    [[ "$value" =~ ^[0-9]{8}$ ]] || return 1
    rendered="$(date -u -d "${value:0:4}-${value:4:2}-${value:6:2}" +%Y%m%d 2>/dev/null)" || return 1
    [[ "$rendered" == "$value" ]]
}

release_channel_for_version() {
    local version="$1"
    local nightly_date

    if is_stable_version "$version"; then
        printf '%s\n' stable
        return
    fi
    if [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+-nightly\.([0-9]{8})$ ]]; then
        nightly_date="${BASH_REMATCH[1]}"
        is_real_utc_date "$nightly_date" || return 1
        printf '%s\n' nightly
        return
    fi
    return 1
}

stable_version_for() {
    local version="$1"
    local channel

    channel="$(release_channel_for_version "$version")" || return 1
    case "$channel" in
        stable) printf '%s\n' "$version" ;;
        nightly) printf '%s\n' "${version%%-nightly.*}" ;;
    esac
}

render_version() {
    local version="$1" target="$2" channel base nightly_date
    channel="$(release_channel_for_version "$version")" || die "invalid release version: $version"
    case "$target" in
        cargo|rpm|deb|arch|ccs|tag) ;;
        *) die "unknown version target: $target" ;;
    esac
    if [[ "$channel" == stable ]]; then
        printf '%s\n' "$version"
        return
    fi
    base="$(stable_version_for "$version")"
    nightly_date="${version##*-nightly.}"
    case "$target" in
        cargo|ccs|tag) printf '%s\n' "$version" ;;
        rpm|deb) printf '%s~nightly.%s\n' "$base" "$nightly_date" ;;
        arch) printf '%snightly%s\n' "$base" "$nightly_date" ;;
    esac
}

authority_target() {
    case "$1" in
        Cargo.toml) printf '%s\n' cargo ;;
        packaging/rpm/conary.spec) printf '%s\n' rpm ;;
        packaging/arch/PKGBUILD) printf '%s\n' arch ;;
        packaging/deb/debian/changelog) printf '%s\n' deb ;;
        packaging/ccs/ccs.toml) printf '%s\n' ccs ;;
        *) die "unknown version authority: $1" ;;
    esac
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
                'conary-<version>-1-x86_64.pkg.tar.zst' \
                'conary-bootstrap-v1.manifest' \
                'conary-bootstrap-v1.manifest.sig'
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
    local version="${tag#v}"
    if [[ "$tag" == v* ]] && is_release_version "$version"; then
        printf '%s\n' "$version"
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
    # Native renderings are not SemVer and must never enter sort -V together.
    local workspace_version
    workspace_version="$(extract_version_from_authority_file Cargo.toml)"
    if [[ "$(release_channel_for_version "$workspace_version")" == nightly ]]; then
        assert_owned_version "$release" "$workspace_version"
        printf '%s\n' "$workspace_version"
        return
    fi
    while IFS= read -r file; do
        [[ -f "$file" ]] || die "version authority file missing: $file"
        versions+=("$(extract_version_from_authority_file "$file")")
    done < <(version_authority_files)

    printf '%s\n' "${versions[@]}" | sort -V | tail -n1
}

assert_owned_version() {
    local release="$1"
    local expected_version="$2"
    local file actual_version field workspace_publish authority_version

    [[ "$release" == "suite" ]] || die "unknown release: $release"
    is_release_version "$expected_version" || die "invalid release version: $expected_version"

    while IFS= read -r file; do
        [[ -f "$file" ]] || die "version authority file missing: $file"
        actual_version="$(extract_version_from_authority_file "$file")"
        authority_version="$(render_version "$expected_version" "$(authority_target "$file")")"
        [[ "$actual_version" == "$authority_version" ]] ||
            die "suite version mismatch: $file is $actual_version, expected $authority_version"
    done < <(version_authority_files)

    workspace_publish="$({
        awk '
            /^\[workspace\.package\]$/ { in_workspace_package = 1; next }
            in_workspace_package && /^\[/ { exit }
            in_workspace_package && /^publish[[:space:]]*=/ {
                value = $0
                sub(/^[^=]*=[[:space:]]*/, "", value)
                sub(/[[:space:]]*#.*/, "", value)
                sub(/[[:space:]]*$/, "", value)
                print value
                exit
            }
        ' Cargo.toml
    } || true)"
    [[ "$workspace_publish" == "false" ]] ||
        die "workspace registry publication must be disabled in Cargo.toml [workspace.package]"

    # The Remi server is the one typed license exception (issue #900): it
    # declares AGPL-3.0-or-later itself while every other member inherits the
    # workspace's MIT OR Apache-2.0. scripts/check-license-authority.sh pins the
    # same decision; the two must agree.
    local remi_manifest='apps/remi/Cargo.toml'
    local remi_license='license = "AGPL-3.0-or-later"'
    while IFS= read -r file; do
        [[ -f "$file" ]] || die "workspace package manifest missing: $file"
        for field in version edition rust-version authors license publish; do
            if [[ "$field" == license && "$file" == "$remi_manifest" ]]; then
                grep -Fxq "$remi_license" "$file" ||
                    die "workspace package license must be exactly '$remi_license' in $file"
                [[ "$(grep -Ec '^license[[:space:]]*=' "$file")" -eq 1 ]] ||
                    die "workspace package license must be declared exactly once in $file"
                ! grep -Fxq "license.workspace = true" "$file" ||
                    die "workspace package must not also inherit license in $file"
                continue
            fi
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
    local expected_tag product first=true channel stable_version

    [[ "$release" == "suite" ]] || die "unknown release: $release"
    is_release_version "$version" || die "invalid release version: $version"
    channel="$(release_channel_for_version "$version")"
    stable_version="$(stable_version_for "$version")"
    expected_tag="$(canonical_tag_prefix_for "$release")${version}"
    [[ "$tag" == "$expected_tag" ]] || die "tag $tag does not match suite version $version"
    [[ "$dry_run" == "true" || "$dry_run" == "false" ]] || die "dry_run must be true or false"

    printf '{"schema_version":1'
    printf ',"release":'; print_json_string "$release"
    printf ',"canonical_tag_prefix":'; print_json_string "$(canonical_tag_prefix_for "$release")"
    printf ',"tag_name":'; print_json_string "$tag"
    printf ',"version":'; print_json_string "$version"
    printf ',"channel":'; print_json_string "$channel"
    printf ',"stable_version":'; print_json_string "$stable_version"
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
    local version channel stable_version

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
    channel="$(release_channel_for_version "$version")"
    stable_version="$(stable_version_for "$version")"
    case "$format" in
        shell)
            printf 'release=suite\n'
            printf 'canonical_tag_prefix=v\n'
            printf 'tag_name=%s\n' "$tag"
            printf 'version=%s\n' "$version"
            printf 'channel=%s\n' "$channel"
            printf 'stable_version=%s\n' "$stable_version"
            printf 'bundle_name=suite-bundle\n'
            printf 'deploy_mode=suite\n'
            ;;
        json)
            printf '{"release":"suite","canonical_tag_prefix":"v","tag_name":'
            print_json_string "$tag"
            printf ',"version":'; print_json_string "$version"
            printf ',"channel":'; print_json_string "$channel"
            printf ',"stable_version":'; print_json_string "$stable_version"
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
        render-version)
            [[ $# -eq 2 ]] || usage
            render_version "$1" "$2"
            ;;
        validate-version)
            [[ $# -ge 1 && $# -le 2 ]] || usage
            channel="$(release_channel_for_version "$1")" || die "invalid release version: $1"
            [[ $# -eq 1 || "$channel" == "$2" ]] ||
                die "release version $1 is channel $channel, expected $2"
            printf '%s\n' "$channel"
            ;;
        version-channel)
            [[ $# -eq 1 ]] || usage
            release_channel_for_version "$1" || die "invalid release version: $1"
            ;;
        stable-version)
            [[ $# -eq 1 ]] || usage
            stable_version_for "$1" || die "invalid release version: $1"
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
        workspace-version)
            [[ $# -eq 0 ]] || usage
            extract_version_from_authority_file Cargo.toml
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
