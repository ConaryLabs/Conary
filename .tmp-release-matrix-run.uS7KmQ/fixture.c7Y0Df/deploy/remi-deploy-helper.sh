#!/usr/bin/env bash
# deploy/remi-deploy-helper.sh -- Root-owned Remi deployment helper.
set -euo pipefail

PATH=/usr/sbin:/usr/bin:/sbin:/bin

ROOT="${CONARY_REMI_DEPLOY_ROOT:-}"
SKIP_RESTART="${CONARY_REMI_DEPLOY_SKIP_RESTART:-0}"
HEALTH_URL="${CONARY_REMI_DEPLOY_HEALTH_URL:-http://localhost:8081/health}"
SURVEY_READINESS_URL="${CONARY_REMI_DEPLOY_SURVEY_READINESS_URL:-http://localhost:8081/health/ready}"
SITE_HOME_URL="${CONARY_REMI_DEPLOY_SITE_HOME_URL:-https://conary.io/}"
SITE_INSTALLER_URL="${CONARY_REMI_DEPLOY_SITE_INSTALLER_URL:-https://conary.io/install-conary-preview.sh}"
SITE_ORIGIN_RESOLVE="${CONARY_REMI_DEPLOY_SITE_ORIGIN_RESOLVE:-conary.io:443:127.0.0.1}"

die() {
    echo "remi deploy helper: $*" >&2
    exit 1
}

usage() {
    cat >&2 <<'USAGE'
usage:
  conary-remi-deploy deploy-conary <version> <staging-dir>
  conary-remi-deploy deploy-remi <version> <bundle.tar.gz> <repositories.toml> <max-concurrent>
  conary-remi-deploy deploy-site <site|web> <staging-dir>
  conary-remi-deploy publish-test-artifact <filename> <sha256> <staged-file>
  conary-remi-deploy install-helper <sha256> <helper>
  conary-remi-deploy inspect-remi [--require-private-candidates [--accept-candidates-completed-after <unix-seconds>]|--require-repopulated]
  conary-remi-deploy inspect-remi-candidate-baseline <version> <sha256> <bundle.tar.gz>
  conary-remi-deploy inspect-remi-storage
  conary-remi-deploy export-native-oracle-inputs <export-id> <fedora-sha256> <ubuntu-sha256> <arch-sha256>
  conary-remi-deploy survey-resolution <survey-id> <export-id> <oracle-transport-path>
  conary-remi-deploy benchmark-remi-conversion <run-id> <installed-binary-sha256> <profile> <revision-sha256> <package-key-sha256> <source-sha256> <source-size>
  conary-remi-deploy verify-ingress
  conary-remi-deploy verify-access
USAGE
    exit 2
}

root_path() {
    local path="$1"
    if [[ -n "$ROOT" ]]; then
        printf '%s%s' "$ROOT" "$path"
    else
        printf '%s' "$path"
    fi
}

owner_args() {
    if [[ -z "$ROOT" ]]; then
        printf '%s\n' -o conary -g conary
    fi
}

validate_version() {
    local version="$1"
    [[ "$version" =~ ^[0-9A-Za-z._+-]+$ ]] || die "invalid version: $version"
}

validate_positive_int() {
    local value="$1"
    [[ "$value" =~ ^[0-9]+$ ]] || die "expected positive integer, got: $value"
    (( value >= 1 && value <= 128 )) || die "value out of allowed range 1..128: $value"
}

validate_positive_timestamp() {
    local value="$1"
    [[ "$value" =~ ^[1-9][0-9]{0,17}$ ]] ||
        die "expected positive Unix timestamp, got: $value"
}

validate_sha256() {
    local value="$1"
    [[ "$value" =~ ^[0-9a-f]{64}$ ]] || die "invalid SHA-256 digest"
}

validate_commit() {
    local value="$1"
    [[ "$value" =~ ^[0-9a-f]{40}$ ]] || die "invalid commit SHA"
}

validate_site_target() {
    local target="$1"
    case "$target" in
        site|web) ;;
        *) die "invalid site target: $target" ;;
    esac
}

validate_artifact_filename() {
    local filename="$1"
    [[ "$filename" =~ ^[0-9A-Za-z][0-9A-Za-z._+-]*$ ]] ||
        die "invalid test-artifact filename: $filename"
}

validate_export_id() {
    local export_id="$1"
    [[ "$export_id" =~ ^[a-z0-9][a-z0-9._-]{0,127}$ ]] ||
        die "invalid native-oracle export identity: $export_id"
}

validate_survey_id() {
    local survey_id="$1"
    [[ "$survey_id" =~ ^[a-z0-9][a-z0-9._-]{0,127}$ ]] ||
        die "invalid resolution-survey identity: $survey_id"
}

validate_profile_id() {
    local profile="$1"
    case "$profile" in
        fedora-44|ubuntu-26.04|arch) ;;
        *) die "invalid benchmark profile: $profile" ;;
    esac
}

validate_positive_size() {
    local value="$1"
    [[ "$value" =~ ^[1-9][0-9]{0,17}$ ]] ||
        die "expected positive byte size, got: $value"
    (( 10#$value <= 8 * 1024 * 1024 * 1024 )) ||
        die "benchmark source exceeds the 8 GiB staging limit"
}

real_tmp_path() {
    local path="$1"
    local resolved
    resolved="$(realpath -e "$path")" || die "missing path: $path"
    [[ "$resolved" == /tmp/* ]] || die "staging path must be under /tmp: $resolved"
    printf '%s' "$resolved"
}

install_owned_dir() {
    local mode="$1"
    shift
    local owners=()
    mapfile -t owners < <(owner_args)
    install -d -m "$mode" "${owners[@]}" "$@"
}

install_owned_file() {
    local mode="$1"
    local src="$2"
    local dest="$3"
    local owners=()
    mapfile -t owners < <(owner_args)
    install -m "$mode" "${owners[@]}" "$src" "$dest"
}

require_shared_conary_root() {
    local path
    path="$(root_path /conary)"
    [[ -d "$path" && ! -L "$path" ]] ||
        die "shared Conary root must be a plain pre-provisioned directory: $path"

    local observed_mode
    observed_mode="$(stat -c '%a' "$path")"
    [[ "$observed_mode" == "750" ]] ||
        die "shared Conary root must have mode 0750, found ${observed_mode}: $path"

    local expected_uid expected_gid expected_identity observed_identity
    if [[ -z "$ROOT" ]]; then
        expected_uid="$(id -u conary)" || die "missing conary service account"
        expected_gid="$(getent group conary-web | cut -d: -f3)"
        [[ -n "$expected_gid" ]] || die "missing conary-web traversal group"
        expected_identity="conary:conary-web"
    else
        expected_uid="$(id -u)"
        expected_gid="$(id -g)"
        expected_identity="$(id -un):$(id -gn)"
    fi
    observed_identity="$(stat -c '%u:%g' "$path")"
    [[ "$observed_identity" == "${expected_uid}:${expected_gid}" ]] ||
        die "shared Conary root must be owned by ${expected_identity}, found ${observed_identity}: $path"
}

probe_exact_ingress_bytes() {
    local description="$1"
    local url="$2"
    local expected="$3"
    local resolve="${4:-}"
    local args=(
        --fail
        --silent
        --show-error
        --location
        --max-time 30
        --retry 3
        --retry-delay 2
    )
    if [[ -n "$resolve" ]]; then
        args+=(--resolve "$resolve")
    fi
    if ! curl "${args[@]}" "$url" | cmp -s - "$expected"; then
        die "${description} did not serve the exact deployed bytes: $url"
    fi
}

verify_ingress() {
    [[ -n "$ROOT" || "$(id -u)" == "0" ]] || die "helper must run as root"
    require_shared_conary_root

    local site_root home installer
    site_root="$(root_path /conary/site)"
    home="${site_root}/index.html"
    installer="${site_root}/install-conary-preview.sh"
    [[ -f "$home" && ! -L "$home" ]] ||
        die "deployed site homepage is not a plain file: $home"
    [[ -f "$installer" && ! -L "$installer" ]] ||
        die "deployed preview installer is not a plain file: $installer"

    if [[ -n "$SITE_ORIGIN_RESOLVE" ]]; then
        probe_exact_ingress_bytes "origin homepage" "$SITE_HOME_URL" "$home" \
            "$SITE_ORIGIN_RESOLVE"
        probe_exact_ingress_bytes "origin preview installer" "$SITE_INSTALLER_URL" \
            "$installer" "$SITE_ORIGIN_RESOLVE"
    fi
    probe_exact_ingress_bytes "public homepage" "$SITE_HOME_URL" "$home"
    probe_exact_ingress_bytes "public preview installer" "$SITE_INSTALLER_URL" "$installer"
}

ensure_repository_keys_root() {
    local path="$1"
    if [[ -e "$path" || -L "$path" ]]; then
        [[ -d "$path" && ! -L "$path" ]] ||
            die "repository signing authority root is not a plain directory: $path"
        [[ "$(stat -c '%a' "$path")" == "700" ]] ||
            die "repository signing authority root must have mode 0700: $path"
        if [[ -z "$ROOT" ]]; then
            local expected_owner observed_owner
            expected_owner="$(id -u conary):$(id -g conary)"
            observed_owner="$(stat -c '%u:%g' "$path")"
            [[ "$observed_owner" == "$expected_owner" ]] ||
                die "repository signing authority root must be owned by conary:conary: $path"
        fi
        return
    fi
    install_owned_dir 0700 "$path"
}

ensure_runtime_lock_file() {
    local path="$1"
    if [[ -e "$path" || -L "$path" ]]; then
        [[ -f "$path" && ! -L "$path" ]] ||
            die "Remi runtime lock is not a plain file: $path"
        [[ "$(stat -c '%a' "$path")" == "600" ]] ||
            die "Remi runtime lock must have mode 0600: $path"
        if [[ -z "$ROOT" ]]; then
            local expected_owner observed_owner
            expected_owner="$(id -u conary):$(id -g conary)"
            observed_owner="$(stat -c '%u:%g' "$path")"
            [[ "$observed_owner" == "$expected_owner" ]] ||
                die "Remi runtime lock must be owned by conary:conary: $path"
        fi
        return
    fi
    install_owned_file 0600 /dev/null "$path"
}

restart_remi() {
    [[ "$SKIP_RESTART" == "1" ]] && return 0
    systemctl restart remi
    sleep 2
    curl -fsS "$HEALTH_URL" >/dev/null
}

deploy_conary() {
    local version="$1"
    local staging
    validate_version "$version"
    staging="$(real_tmp_path "$2")"
    [[ -d "$staging" && ! -L "$staging" ]] || die "staging path is not a plain directory: $staging"

    local releases_root release_dir self_update_dir
    releases_root="$(root_path /conary/releases)"
    release_dir="$(root_path "/conary/releases/${version}")"
    self_update_dir="$(root_path /conary/self-update)"

    require_shared_conary_root
    install_owned_dir 0750 "$releases_root" "$release_dir" "$self_update_dir"

    shopt -s nullglob
    local files=("$staging"/*)
    shopt -u nullglob
    (( ${#files[@]} > 0 )) || die "staging directory is empty: $staging"

    local checksum_file="${staging}/SHA256SUMS"
    [[ -f "$checksum_file" && ! -L "$checksum_file" ]] ||
        die "missing plain release checksum file: ${checksum_file}"
    (
        cd "$staging"
        sha256sum -c SHA256SUMS >/dev/null
    ) || die "release checksum verification failed for: $staging"

    local ccs_source=""
    shopt -s nullglob
    local ccs_files=("$staging"/*.ccs)
    shopt -u nullglob
    for file in "${ccs_files[@]}"; do
        [[ -f "$file" && ! -L "$file" ]] || die "refusing non-regular CCS artifact: $file"
        [[ -f "${file}.sig" && ! -L "${file}.sig" ]] ||
            die "missing plain CCS signature for: $file"
        if [[ -z "$ccs_source" ]]; then
            ccs_source="$file"
        fi
    done

    local file base
    for file in "${files[@]}"; do
        [[ -f "$file" && ! -L "$file" ]] || die "refusing non-regular release artifact: $file"
        base="$(basename "$file")"
        install_owned_file 0644 "$file" "${release_dir}/${base}"
    done

    if [[ -n "$ccs_source" ]]; then
        install_owned_file 0644 "$ccs_source" "${self_update_dir}/conary-${version}.ccs"
        install_owned_file 0644 "${ccs_source}.sig" "${self_update_dir}/conary-${version}.ccs.sig"
    fi

    ln -sfn "$version" "${releases_root}/latest"
    if [[ -z "$ROOT" ]]; then
        chown -h conary:conary "${releases_root}/latest"
    fi

    rm -rf "$staging"
}

deploy_remi() {
    local version="$1"
    local bundle
    local repositories
    local max_concurrent="$4"
    validate_version "$version"
    validate_positive_int "$max_concurrent"
    bundle="$(real_tmp_path "$2")"
    repositories="$(real_tmp_path "$3")"
    [[ -f "$bundle" && ! -L "$bundle" ]] || die "bundle path is not a plain file: $bundle"
    [[ -f "$repositories" && ! -L "$repositories" ]] ||
        die "repository manifest is not a plain file: $repositories"

    local tmpdir bin candidate backup had_previous transition_manifest repository_keys_dir
    local runtime_root runtime_lock
    tmpdir="$(mktemp -d /tmp/remi-install.XXXXXX)"
    backup="${tmpdir}/remi.previous"
    bin="$(root_path /usr/local/bin/remi)"
    had_previous=false
    trap 'rm -rf "$tmpdir"' RETURN

    tar xzf "$bundle" -C "$tmpdir"
    candidate="${tmpdir}/remi-${version}-linux-x64"
    [[ -f "$candidate" && ! -L "$candidate" ]] || die "bundle did not contain remi-${version}-linux-x64"
    [[ "$("$candidate" --version)" == "remi ${version}" ]] ||
        die "candidate binary version does not match ${version}"

    runtime_root="$(root_path /conary)"
    runtime_lock="${runtime_root}/.remi-runtime.lock"
    require_shared_conary_root
    install_owned_dir 0750 "${runtime_root}/metadata"
    ensure_runtime_lock_file "$runtime_lock"
    repository_keys_dir="${runtime_root}/repository-keys"
    ensure_repository_keys_root "$repository_keys_dir"

    if [[ -f "$bin" ]]; then
        cp "$bin" "$backup"
        had_previous=true
    fi

    if [[ "$SKIP_RESTART" != "1" ]]; then
        systemctl stop remi
    fi

    if ! transition_manifest="$(
        "$candidate" deployment prepare \
            --config "$(root_path /etc/conary/remi.toml)" \
            --repository-manifest "$repositories" \
            --repository-manifest-target "$(root_path /etc/conary/remi-repositories.toml)" \
            --repository-keys-dir "$repository_keys_dir" \
            --deployment-id "remi-${version}" \
            --max-concurrent "$max_concurrent"
    )"; then
        if [[ "$had_previous" == true && "$SKIP_RESTART" != "1" ]]; then
            systemctl start remi || true
        fi
        die "failed to prepare Remi deployment transition"
    fi
    [[ "$transition_manifest" == /* && -f "$transition_manifest" && ! -L "$transition_manifest" ]] || {
        die "candidate returned an invalid transition manifest; Remi remains stopped because rollback authority is unavailable"
    }

    if ! install -m 0755 "$candidate" "$bin"; then
        local rollback_status=0
        "$candidate" deployment rollback --manifest "$transition_manifest" || rollback_status=$?
        if [[ "$had_previous" == true && "$SKIP_RESTART" != "1" ]]; then
            systemctl start remi || true
        fi
        (( rollback_status == 0 )) ||
            die "failed to install Remi binary and rollback failed with status ${rollback_status}"
        die "failed to install Remi binary"
    fi

    if ! restart_remi; then
        local rollback_status=0
        [[ "$SKIP_RESTART" == "1" ]] || systemctl stop remi || true
        "$candidate" deployment rollback --manifest "$transition_manifest" || rollback_status=$?
        if [[ "$had_previous" == true ]]; then
            install -m 0755 "$backup" "$bin" || true
        else
            rm -f "$bin"
        fi
        if [[ "$had_previous" == true ]]; then
            restart_remi || true
        fi
        (( rollback_status == 0 )) ||
            die "Remi health check failed and rollback failed with status ${rollback_status}"
        die "Remi health check failed after deployment"
    fi

    rm -f "$bundle" "$repositories"
    echo "Remi deployment transition: ${transition_manifest}"
}

deploy_site() {
    local site_target="$1"
    local staging
    validate_site_target "$site_target"
    staging="$(real_tmp_path "$2")"
    [[ -d "$staging" && ! -L "$staging" ]] ||
        die "staging path is not a plain directory: $staging"
    [[ -f "${staging}/index.html" && ! -L "${staging}/index.html" ]] ||
        die "staging directory is missing plain index.html: $staging"

    local parent target tmp backup
    target="$(root_path "/conary/${site_target}")"
    parent="$(dirname "$target")"
    tmp="$(root_path "/conary/.${site_target}.next.$$")"
    backup="$(root_path "/conary/.${site_target}.previous.$$")"

    require_shared_conary_root
    rm -rf "$tmp" "$backup"
    mkdir -p "$tmp"

    if ! cp -a "${staging}/." "$tmp/"; then
        rm -rf "$tmp"
        die "failed to copy staged ${site_target} site"
    fi

    find "$tmp" -type d -exec chmod 0755 {} +
    find "$tmp" -type f -exec chmod 0644 {} +
    if [[ -z "$ROOT" ]]; then
        chown -R conary:conary "$tmp"
    fi

    if [[ -e "$target" || -L "$target" ]]; then
        [[ -d "$target" && ! -L "$target" ]] ||
            die "target is not a plain directory: $target"
        mv "$target" "$backup"
    fi

    if ! mv "$tmp" "$target"; then
        if [[ -e "$backup" ]]; then
            mv "$backup" "$target" || true
        fi
        rm -rf "$tmp"
        die "failed to publish ${site_target} site"
    fi

    rm -rf "$backup" "$staging"
    rmdir "$parent" 2>/dev/null || true
}

publish_test_artifact() {
    local filename="$1"
    local expected_sha="$2"
    local source_arg="$3"
    local source
    validate_artifact_filename "$filename"
    validate_sha256 "$expected_sha"
    [[ ! -L "$source_arg" ]] ||
        die "test-artifact source must not be a symlink: $source_arg"
    source="$(real_tmp_path "$source_arg")"
    [[ -f "$source" && ! -L "$source" ]] ||
        die "test-artifact source is not a plain file: $source"

    local size actual_sha artifact_root target next
    size="$(stat -c '%s' "$source")"
    (( size > 0 )) || die "test artifact must not be empty"
    (( size <= 8 * 1024 * 1024 * 1024 )) ||
        die "test artifact exceeds the 8 GiB publication limit"
    actual_sha="$(sha256sum "$source" | cut -d ' ' -f 1)"
    [[ "$actual_sha" == "$expected_sha" ]] || die "test-artifact SHA-256 mismatch"

    artifact_root="$(root_path /conary/test-artifacts)"
    target="${artifact_root}/${filename}"
    next="${artifact_root}/.${filename}.next.$$"
    require_shared_conary_root
    if [[ -e "$artifact_root" || -L "$artifact_root" ]]; then
        [[ -d "$artifact_root" && ! -L "$artifact_root" ]] ||
            die "test-artifact root is not a plain directory: $artifact_root"
    else
        install_owned_dir 0755 "$artifact_root"
    fi

    if [[ -e "$target" || -L "$target" ]]; then
        [[ -f "$target" && ! -L "$target" ]] ||
            die "published test-artifact target is not a plain file: $target"
        actual_sha="$(sha256sum "$target" | cut -d ' ' -f 1)"
        [[ "$actual_sha" == "$expected_sha" ]] ||
            die "immutable test-artifact target already exists with a different SHA-256: $target"
        rm -f "$source"
        printf 'Test artifact already published: %s sha256=%s size=%s\n' \
            "$filename" "$expected_sha" "$size"
        return
    fi

    trap 'rm -f "$next"' EXIT
    install_owned_file 0644 "$source" "$next"
    actual_sha="$(sha256sum "$next" | cut -d ' ' -f 1)"
    [[ "$actual_sha" == "$expected_sha" ]] ||
        die "staged test-artifact changed during publication"

    if ! ln "$next" "$target"; then
        [[ -f "$target" && ! -L "$target" ]] ||
            die "test-artifact target appeared during publication: $target"
        actual_sha="$(sha256sum "$target" | cut -d ' ' -f 1)"
        [[ "$actual_sha" == "$expected_sha" ]] ||
            die "immutable test-artifact target raced with a different SHA-256: $target"
    fi
    rm -f "$next" "$source"
    trap - EXIT

    printf 'Published test artifact: %s sha256=%s size=%s\n' \
        "$filename" "$expected_sha" "$size"
}

protected_main_commit() {
    local test_authority="${CONARY_REMI_DEPLOY_TEST_PROTECTED_MAIN_ROOT:-}"
    local commit
    if [[ -n "$test_authority" ]]; then
        [[ -n "$ROOT" ]] || die "protected-main test authority requires a fake root"
        [[ -d "$test_authority" && ! -L "$test_authority" \
            && -f "${test_authority}/main-commit" \
            && ! -L "${test_authority}/main-commit" ]] ||
            die "protected-main test authority is malformed"
        commit="$(<"${test_authority}/main-commit")"
    else
        commit="$(curl -fsS --proto '=https' --tlsv1.2 --max-time 60 \
            --retry 2 --retry-connrefused --max-filesize 1048576 \
            -H 'Accept: application/vnd.github+json' \
            -H 'X-GitHub-Api-Version: 2026-03-10' \
            https://api.github.com/repos/FieldmouseWorks/Conary/commits/main \
            | jq -er '.sha')" || die "could not resolve protected-main helper authority"
    fi
    validate_commit "$commit"
    printf '%s' "$commit"
}

fetch_protected_main_helper() {
    local commit="$1"
    local destination="$2"
    local test_authority="${CONARY_REMI_DEPLOY_TEST_PROTECTED_MAIN_ROOT:-}"
    validate_commit "$commit"
    if [[ -n "$test_authority" ]]; then
        [[ -n "$ROOT" ]] || die "protected-main test authority requires a fake root"
        local canonical="${test_authority}/deploy/remi-deploy-helper.sh"
        [[ -f "$canonical" && ! -L "$canonical" ]] ||
            die "protected-main test helper is not plain data"
        install -m 0600 "$canonical" "$destination"
    else
        curl -fsS --proto '=https' --tlsv1.2 --max-time 60 \
            --retry 2 --retry-connrefused --max-filesize 1048576 \
            --output "$destination" \
            "https://raw.githubusercontent.com/FieldmouseWorks/Conary/${commit}/deploy/remi-deploy-helper.sh" ||
            die "could not fetch the exact protected-main helper"
        chmod 0600 "$destination"
    fi
}

install_helper() {
    local expected_sha="$1"
    local source
    validate_sha256 "$expected_sha"
    source="$(real_tmp_path "$2")"
    [[ -f "$source" && ! -L "$source" ]] || die "helper source is not a plain file: $source"

    local actual_sha target next staging main_commit canonical_sha256
    actual_sha="$(sha256sum "$source" | cut -d ' ' -f 1)"
    [[ "$actual_sha" == "$expected_sha" ]] || die "helper SHA-256 mismatch"

    staging="$(mktemp -d /tmp/conary-remi-helper.XXXXXX)"
    chmod 0700 "$staging"
    trap 'rm -rf -- "$staging"' EXIT
    main_commit="$(protected_main_commit)"
    fetch_protected_main_helper "$main_commit" "${staging}/helper"
    canonical_sha256="$(sha256sum "${staging}/helper" | cut -d ' ' -f 1)"
    [[ "$canonical_sha256" == "$expected_sha" ]] ||
        die "helper digest is not authorized by current protected main"
    [[ "$(protected_main_commit)" == "$main_commit" ]] ||
        die "protected main advanced during helper authorization"
    bash -n "${staging}/helper" || die "protected-main helper shell validation failed"

    target="$(root_path /usr/local/sbin/conary-remi-deploy)"
    next="${target}.next.$$"
    install -m 0755 "${staging}/helper" "$next"
    mv "$next" "$target"
    rm -f "$source"
    rm -rf -- "$staging"
    trap - EXIT
}

inspect_remi() {
    local requirement=""
    local completed_after=""
    while (( $# > 0 )); do
        case "$1" in
            --require-private-candidates|--require-repopulated)
                [[ -z "$requirement" ]] || die "duplicate inspect-remi requirement"
                requirement="$1"
                shift
                ;;
            --accept-candidates-completed-after)
                [[ -z "$completed_after" ]] ||
                    die "duplicate private-candidate completion floor"
                (( $# >= 2 )) || die "private-candidate completion floor is missing"
                validate_positive_timestamp "$2"
                completed_after="$2"
                shift 2
                ;;
            *) die "invalid inspect-remi option: $1" ;;
        esac
    done
    if [[ -n "$completed_after" && "$requirement" != "--require-private-candidates" ]]; then
        die "private-candidate completion floor requires --require-private-candidates"
    fi
    local bin
    bin="$(root_path /usr/local/bin/remi)"
    [[ -f "$bin" && ! -L "$bin" ]] || die "Remi binary is not a plain file: $bin"
    local args=(deployment inspect --config "$(root_path /etc/conary/remi.toml)")
    if [[ -n "$requirement" ]]; then
        args+=("$requirement")
    fi
    if [[ -n "$completed_after" ]]; then
        args+=(--accept-candidates-completed-after "$completed_after")
    fi
    "$bin" "${args[@]}"
}

inspect_remi_candidate_baseline() {
    local version="$1"
    local expected_sha="$2"
    local bundle
    validate_version "$version"
    validate_sha256 "$expected_sha"
    bundle="$(real_tmp_path "$3")"
    [[ -f "$bundle" && ! -L "$bundle" ]] || die "bundle path is not a plain file: $bundle"

    local tmpdir member candidate occurrences actual_sha
    tmpdir="$(mktemp -d /tmp/remi-baseline.XXXXXX)"
    trap 'rm -rf "$tmpdir"' RETURN
    member="remi-${version}-linux-x64"
    candidate="${tmpdir}/${member}"
    occurrences="$(tar tzf "$bundle" | awk -v expected="$member" '
        $0 == expected { count += 1 }
        END { print count + 0 }
    ')" || die "could not inspect candidate bundle"
    [[ "$occurrences" == "1" ]] ||
        die "bundle must contain exactly one plain ${member}"
    tar xOzf "$bundle" -- "$member" >"$candidate" ||
        die "could not extract ${member} from candidate bundle"
    chmod 0755 "$candidate"
    actual_sha="$(sha256sum "$candidate" | cut -d ' ' -f 1)"
    [[ "$actual_sha" == "$expected_sha" ]] || die "candidate Remi SHA-256 mismatch"
    [[ "$("$candidate" --version)" == "remi ${version}" ]] ||
        die "candidate binary version does not match ${version}"
    local installed baseline_owner
    installed="$(root_path /usr/local/bin/remi)"
    if [[ -e "$installed" || -L "$installed" ]]; then
        [[ -f "$installed" && ! -L "$installed" && -x "$installed" ]] ||
            die "installed Remi baseline owner is not a plain executable: $installed"
        baseline_owner="$installed"
    else
        baseline_owner="$candidate"
    fi
    "$baseline_owner" deployment baseline --config "$(root_path /etc/conary/remi.toml)"
}

inspect_remi_storage() {
    [[ -n "$ROOT" || "$(id -u)" == "0" ]] || die "helper must run as root"
    require_shared_conary_root

    local runtime_root database_root backup_root
    runtime_root="$(root_path /conary)"
    database_root="${runtime_root}/metadata/conary.db"
    backup_root="${runtime_root}/deployment-backups"

    local database_files=0 database_logical_bytes=0 database_allocated_bytes=0
    local path size blocks
    for path in "$database_root" "${database_root}-wal" "${database_root}-shm"; do
        if [[ ! -e "$path" && ! -L "$path" ]]; then
            continue
        fi
        [[ -f "$path" && ! -L "$path" ]] ||
            die "Remi database storage contains a non-plain SQLite file"
        size="$(stat -c '%s' "$path")" || die "could not measure Remi SQLite size"
        blocks="$(stat -c '%b' "$path")" || die "could not measure Remi SQLite blocks"
        [[ "$size" =~ ^[0-9]+$ && "$blocks" =~ ^[0-9]+$ ]] ||
            die "could not measure Remi SQLite storage"
        database_files=$((database_files + 1))
        database_logical_bytes=$((database_logical_bytes + size))
        database_allocated_bytes=$((database_allocated_bytes + blocks * 512))
    done

    local backup_directories=0 backup_logical_bytes=0 backup_allocated_bytes=0
    if [[ -e "$backup_root" || -L "$backup_root" ]]; then
        [[ -d "$backup_root" && ! -L "$backup_root" ]] ||
            die "deployment backup root is not a plain directory"
        local unexpected
        unexpected="$(find "$backup_root" -xdev -type l -print -quit)" ||
            die "could not inspect deployment backup symlinks"
        [[ -z "$unexpected" ]] ||
            die "deployment backup storage contains a symlink"
        unexpected="$(find "$backup_root" -mindepth 1 -maxdepth 1 ! -type d -print -quit)" ||
            die "could not inspect deployment backup entries"
        [[ -z "$unexpected" ]] ||
            die "deployment backup root contains an unexpected entry"
        backup_directories="$(
            find "$backup_root" -mindepth 1 -maxdepth 1 -type d -printf x |
                awk '{ total += length($0) } END { print total + 0 }'
        )" || die "could not count deployment backups"
        backup_logical_bytes="$(du --bytes --summarize -- "$backup_root" | cut -f1)" ||
            die "could not measure logical deployment backup bytes"
        backup_allocated_bytes="$(du --block-size=1 --summarize -- "$backup_root" | cut -f1)" ||
            die "could not measure allocated deployment backup bytes"
        [[ "$backup_directories" =~ ^[0-9]+$ \
            && "$backup_logical_bytes" =~ ^[0-9]+$ \
            && "$backup_allocated_bytes" =~ ^[0-9]+$ ]] ||
            die "deployment backup storage returned nonnumeric evidence"
    fi

    local available_blocks block_size available_bytes
    if ! read -r available_blocks block_size \
        < <(stat -f -c '%a %S' "$runtime_root"); then
        die "could not measure Remi filesystem availability"
    fi
    [[ "$available_blocks" =~ ^[0-9]+$ && "$block_size" =~ ^[0-9]+$ ]] ||
        die "could not measure Remi filesystem availability"
    available_bytes=$((available_blocks * block_size))

    jq -n \
        --argjson available_bytes "$available_bytes" \
        --argjson database_files "$database_files" \
        --argjson database_logical_bytes "$database_logical_bytes" \
        --argjson database_allocated_bytes "$database_allocated_bytes" \
        --argjson backup_directories "$backup_directories" \
        --argjson backup_logical_bytes "$backup_logical_bytes" \
        --argjson backup_allocated_bytes "$backup_allocated_bytes" '
        {
          schema_version: 1,
          filesystem: {available_bytes: $available_bytes},
          database: {
            files: $database_files,
            logical_bytes: $database_logical_bytes,
            allocated_bytes: $database_allocated_bytes
          },
          transition_backups: {
            directories: $backup_directories,
            logical_bytes: $backup_logical_bytes,
            allocated_bytes: $backup_allocated_bytes
          }
        }
    '
}

export_native_oracle_inputs() {
    local export_id="$1"
    local fedora_sha256="$2"
    local ubuntu_sha256="$3"
    local arch_sha256="$4"
    validate_export_id "$export_id"
    validate_sha256 "$fedora_sha256"
    validate_sha256 "$ubuntu_sha256"
    validate_sha256 "$arch_sha256"

    local bin evidence_root output transport transport_next
    bin="$(root_path /usr/local/bin/remi)"
    [[ -f "$bin" && ! -L "$bin" ]] || die "Remi binary is not a plain file: $bin"
    evidence_root="$(root_path /conary/evidence/native-oracle-inputs)"
    output="${evidence_root}/${export_id}"
    transport="/tmp/remi-native-oracle-input-${export_id}.tar"
    transport_next=""
    require_shared_conary_root
    install_owned_dir 0750 "$(root_path /conary/evidence)" "$evidence_root"
    [[ ! -e "$output" && ! -L "$output" ]] ||
        die "native-oracle export already exists: $output"
    [[ ! -e "$transport" && ! -L "$transport" ]] ||
        die "native-oracle transport already exists: $transport"

    local command=(
        "$bin" native-oracle-input
        --db "$(root_path /conary/metadata/conary.db)"
        --catalog-dir "$(root_path /conary/catalogs)"
        --candidate "fedora-44=${fedora_sha256}"
        --candidate "ubuntu-26.04=${ubuntu_sha256}"
        --candidate "arch=${arch_sha256}"
        --output-dir "$output"
    )
    if [[ -z "$ROOT" ]]; then
        runuser -u conary -- "${command[@]}"
    else
        "${command[@]}"
    fi
    [[ -d "$output" && ! -L "$output" ]] ||
        die "native-oracle exporter did not publish its exact output"

    transport_next="$(mktemp "/tmp/remi-native-oracle-input-${export_id}.XXXXXX")"
    trap 'rm -f "$transport_next"' EXIT
    tar -cf "$transport_next" -C "$evidence_root" "$export_id"
    chmod 0600 "$transport_next"
    if [[ -z "$ROOT" ]]; then
        chown "${SUDO_UID:-0}:${SUDO_GID:-0}" "$transport_next"
    fi
    if ! ln "$transport_next" "$transport"; then
        die "native-oracle transport target appeared during publication: $transport"
    fi
    rm -f "$transport_next"
    trap - EXIT
    printf 'Native oracle inputs: export=%s transport=%s sha256=%s\n' \
        "$export_id" "$transport" "$(sha256sum "$transport" | cut -d ' ' -f 1)"
}

SURVEY_REMI_STOPPED=0
SURVEY_SYSTEMCTL=systemctl
SURVEY_STAGING=""
SURVEY_TRANSPORT_NEXT=""

survey_systemctl() {
    "$SURVEY_SYSTEMCTL" "$@" >/dev/null
}

survey_start_and_probe() {
    if ! survey_systemctl start remi; then
        echo "remi deploy helper: failed to restart Remi after resolution survey" >&2
        return 1
    fi
    local readiness_attempts_remaining=30
    while (( readiness_attempts_remaining > 0 )); do
        if curl -fsS --max-time 2 "$SURVEY_READINESS_URL" >/dev/null 2>&1; then
            SURVEY_REMI_STOPPED=0
            return 0
        fi
        readiness_attempts_remaining=$((readiness_attempts_remaining - 1))
        if (( readiness_attempts_remaining > 0 )) && [[ -z "$ROOT" ]]; then
            sleep 1
        fi
    done
    echo "remi deploy helper: Remi readiness check failed after resolution survey" >&2
    return 1
}

survey_restore_and_exit() {
    local status="$1"
    trap - EXIT INT TERM
    set +e
    if [[ -n "$SURVEY_TRANSPORT_NEXT" ]]; then
        rm -f -- "$SURVEY_TRANSPORT_NEXT"
        SURVEY_TRANSPORT_NEXT=""
    fi
    if [[ "$SURVEY_REMI_STOPPED" == "1" ]]; then
        if ! survey_start_and_probe; then
            if (( status == 0 )); then
                status=1
            fi
        fi
    fi
    if [[ -n "$SURVEY_STAGING" ]]; then
        rm -rf -- "$SURVEY_STAGING"
        SURVEY_STAGING=""
    fi
    exit "$status"
}

survey_validate_oracle_transport() {
    local survey_id="$1"
    local export_id="$2"
    local transport="$3"
    local manifest="$4"

    local listing="${manifest}.listing"
    if ! tar -tf "$transport" >"$listing"; then
        die "resolution-survey oracle transport is not an uncompressed tar archive"
    fi
    [[ "$(head -n 1 "$listing")" == "manifest.json" ]] ||
        die "resolution-survey oracle transport must begin with manifest.json"
    [[ -z "$(sort "$listing" | uniq -d)" ]] ||
        die "resolution-survey oracle transport repeats a member"
    if grep -Ev '^(manifest\.json|(fedora-44|ubuntu-26\.04|arch)/(package-oracle|native-resolution)/(manifest\.json|packages\.jsonl|roots\.jsonl))$' \
        "$listing" | grep -q .; then
        die "resolution-survey oracle transport contains an unsafe member"
    fi
    local verbose_listing="${manifest}.verbose"
    if ! tar -tvf "$transport" >"$verbose_listing"; then
        die "could not inspect resolution-survey oracle member types"
    fi
    if awk 'substr($1, 1, 1) != "-" { exit 1 }' "$verbose_listing"; then
        :
    else
        die "resolution-survey oracle transport contains a non-plain member"
    fi
    [[ "$(grep -c '^manifest\.json$' "$listing")" == "1" ]] ||
        die "resolution-survey oracle transport has no unique manifest"
    tar -xOf "$transport" -- manifest.json >"$manifest" ||
        die "could not read resolution-survey oracle manifest"
    [[ -s "$manifest" && "$(stat -c '%s' "$manifest")" -le 1048576 ]] ||
        die "resolution-survey oracle manifest size is outside its bounded contract"
    jq -e -cS . "$manifest" >/dev/null ||
        die "resolution-survey oracle manifest is not valid JSON"
    [[ "$(jq -cS . "$manifest")" == "$(cat "$manifest")" ]] ||
        die "resolution-survey oracle manifest is not canonical JSON"

    if jq -e '.schema_version == 1' "$manifest" >/dev/null; then
        die '{"status":"obsolete","reason":"schema_rebuild_required","envelope":"survey input manifest","found_schema":1,"current_schema":2,"message":"rebuild retained survey input as schema 2"}'
    fi
    jq -e \
        --arg survey_id "$survey_id" \
        --arg export_id "$export_id" '
        def sha256: type == "string" and test("^[0-9a-f]{64}$");
        def commit: type == "string" and test("^[0-9a-f]{40}$");
        def uint: type == "number" and floor == . and . >= 0;
        def exact_keys($keys): (keys | sort) == ($keys | sort);
        exact_keys(["deployment", "export_id", "files", "profiles", "schema_version", "survey_id", "workflow_runs"])
        and .schema_version == 2
        and .survey_id == $survey_id
        and .export_id == $export_id
        and (.workflow_runs | exact_keys(["deployment", "export", "oracle"]))
        and all(.workflow_runs[]; uint and . > 0)
        and (.deployment | exact_keys(["binary_sha256", "commit_sha"]))
        and (.deployment.commit_sha | commit)
        and (.deployment.binary_sha256 | sha256)
        and ([.profiles[].profile] == ["fedora-44", "ubuntu-26.04", "arch"])
        and ([.profiles[].target_architecture] == ["x86_64", "amd64", "x86_64"])
        and all(.profiles[];
          exact_keys(["input_manifest_sha256", "native_resolution", "package_oracle", "profile", "profile_revision_sha256", "target_architecture"])
          and (.profile_revision_sha256 | sha256)
          and (.input_manifest_sha256 | sha256)
          and (.package_oracle | exact_keys(["artifact", "manifest_sha256"]))
          and (.package_oracle.manifest_sha256 | sha256)
          and (.package_oracle.artifact | exact_keys(["name", "sha256", "size"]))
          and .package_oracle.artifact.name == "packages.jsonl"
          and (.package_oracle.artifact.sha256 | sha256)
          and (.package_oracle.artifact.size | uint)
          and (.native_resolution | exact_keys(["artifact", "manifest_sha256", "package_oracle_manifest_sha256"]))
          and (.native_resolution.manifest_sha256 | sha256)
          and .native_resolution.package_oracle_manifest_sha256 == .package_oracle.manifest_sha256
          and (.native_resolution.artifact | exact_keys(["name", "sha256", "size"]))
          and .native_resolution.artifact.name == "roots.jsonl"
          and (.native_resolution.artifact.sha256 | sha256)
          and (.native_resolution.artifact.size | uint))
        and (.files | type == "array" and length == 12)
        and ([.files[].path] == [
          "fedora-44/package-oracle/manifest.json",
          "fedora-44/package-oracle/packages.jsonl",
          "fedora-44/native-resolution/manifest.json",
          "fedora-44/native-resolution/roots.jsonl",
          "ubuntu-26.04/package-oracle/manifest.json",
          "ubuntu-26.04/package-oracle/packages.jsonl",
          "ubuntu-26.04/native-resolution/manifest.json",
          "ubuntu-26.04/native-resolution/roots.jsonl",
          "arch/package-oracle/manifest.json",
          "arch/package-oracle/packages.jsonl",
          "arch/native-resolution/manifest.json",
          "arch/native-resolution/roots.jsonl"
        ])
        and all(.files[];
          exact_keys(["path", "sha256", "size"])
          and (.sha256 | sha256)
          and (.size | uint))
        ' "$manifest" >/dev/null ||
        die "resolution-survey oracle manifest violates its exact schema"

    local expected_members="${manifest}.expected"
    {
        printf '%s\n' manifest.json
        jq -r '.files[].path' "$manifest"
    } >"$expected_members"
    cmp -s "$listing" "$expected_members" ||
        die "resolution-survey oracle transport members disagree with its manifest"

    local path expected_sha256 expected_size observed_sha256 observed_size
    while IFS=$'\t' read -r path expected_sha256 expected_size; do
        observed_size="$(tar -xOf "$transport" -- "$path" | wc -c)" ||
            die "could not size resolution-survey oracle member: $path"
        [[ "$observed_size" == "$expected_size" ]] ||
            die "resolution-survey oracle member size mismatch: $path"
        observed_sha256="$(tar -xOf "$transport" -- "$path" | sha256sum | cut -d ' ' -f 1)" ||
            die "could not hash resolution-survey oracle member: $path"
        [[ "$observed_sha256" == "$expected_sha256" ]] ||
            die "resolution-survey oracle member SHA-256 mismatch: $path"
    done < <(jq -r '.files[] | [.path, .sha256, (.size | tostring)] | @tsv' "$manifest")
}

survey_validate_unpacked_oracles() {
    local manifest="$1"
    local oracle_root="$2"
    local profile architecture revision package_manifest resolution_manifest
    local package_manifest_sha256 resolution_manifest_sha256 package_artifact resolution_artifact
    while IFS=$'\t' read -r profile architecture revision package_manifest_sha256 resolution_manifest_sha256; do
        package_manifest="${oracle_root}/${profile}/package-oracle/manifest.json"
        resolution_manifest="${oracle_root}/${profile}/native-resolution/manifest.json"
        package_artifact="${oracle_root}/${profile}/package-oracle/packages.jsonl"
        resolution_artifact="${oracle_root}/${profile}/native-resolution/roots.jsonl"
        jq -e -cS . "$package_manifest" >/dev/null ||
            die "${profile} package-oracle manifest is invalid"
        jq -e -cS . "$resolution_manifest" >/dev/null ||
            die "${profile} resolution-oracle manifest is invalid"
        [[ "$(jq -cS . "$package_manifest")" == "$(cat "$package_manifest")" ]] ||
            die "${profile} package-oracle manifest is not canonical"
        [[ "$(jq -cS . "$resolution_manifest")" == "$(cat "$resolution_manifest")" ]] ||
            die "${profile} resolution-oracle manifest is not canonical"
        jq -e \
            --arg profile "$profile" \
            --arg revision "$revision" \
            --arg artifact_sha256 "$(sha256sum "$package_artifact" | cut -d ' ' -f 1)" \
            --argjson artifact_size "$(stat -c '%s' "$package_artifact")" '
            .schema_version == 1
            and .profile == $profile
            and .profile_revision_sha256 == $revision
            and .artifact.sha256 == $artifact_sha256
            and .artifact.size == $artifact_size
        ' "$package_manifest" >/dev/null ||
            die "${profile} package oracle differs from the authenticated binding"
        jq -e \
            --arg profile "$profile" \
            --arg revision "$revision" \
            --arg architecture "$architecture" \
            --arg package_manifest_sha256 "$package_manifest_sha256" \
            --arg artifact_sha256 "$(sha256sum "$resolution_artifact" | cut -d ' ' -f 1)" \
            --argjson artifact_size "$(stat -c '%s' "$resolution_artifact")" '
            (if .schema_version == 1 or .schema_version == 2 then
              error("schema_rebuild_required: obsolete native resolution bundle; rebuild required as schema 3")
              else .schema_version == 3 end)
            and .profile == $profile
            and .profile_revision_sha256 == $revision
            and .package_oracle_manifest_sha256 == $package_manifest_sha256
            and .policy.architecture == $architecture
            and .artifact.sha256 == $artifact_sha256
            and .artifact.size == $artifact_size
        ' "$resolution_manifest" >/dev/null ||
            die "${profile} resolution oracle differs from the authenticated binding"
        [[ "$(sha256sum "$package_manifest" | cut -d ' ' -f 1)" == "$package_manifest_sha256" ]] ||
            die "${profile} package-oracle manifest digest changed after unpacking"
        [[ "$(sha256sum "$resolution_manifest" | cut -d ' ' -f 1)" == "$resolution_manifest_sha256" ]] ||
            die "${profile} resolution-oracle manifest digest changed after unpacking"
    done < <(jq -r '.profiles[] | [
      .profile,
      .target_architecture,
      .profile_revision_sha256,
      .package_oracle.manifest_sha256,
      .native_resolution.manifest_sha256
    ] | @tsv' "$manifest")
}

survey_resolution() {
    [[ $# -eq 3 ]] || usage
    local survey_id="$1"
    local export_id="$2"
    local transport_arg="$3"
    validate_survey_id "$survey_id"
    validate_export_id "$export_id"

    [[ -n "$ROOT" || "$(id -u)" == "0" ]] || die "helper must run as root"
    [[ "$SKIP_RESTART" == "0" ]] ||
        die "resolution survey may not skip Remi service restoration"
    require_shared_conary_root

    local test_systemctl="${CONARY_REMI_DEPLOY_TEST_SYSTEMCTL:-}"
    if [[ -n "$test_systemctl" ]]; then
        [[ -n "$ROOT" ]] || die "resolution-survey systemctl override requires a fake root"
        [[ -f "$test_systemctl" && ! -L "$test_systemctl" && -x "$test_systemctl" ]] ||
            die "resolution-survey systemctl test override is not a plain executable"
        SURVEY_SYSTEMCTL="$(realpath -e "$test_systemctl")"
    fi

    [[ "$transport_arg" == "/tmp/remi-resolution-survey-oracles-${survey_id}.tar" ]] ||
        die "resolution-survey oracle transport path does not match its survey identity"
    [[ ! -L "$transport_arg" ]] ||
        die "resolution-survey oracle transport must not be a symlink"
    local oracle_transport
    oracle_transport="$(real_tmp_path "$transport_arg")"
    [[ -f "$oracle_transport" && ! -L "$oracle_transport" ]] ||
        die "resolution-survey oracle transport is not a plain file"
    [[ "$(stat -c '%h' "$oracle_transport")" == "1" ]] ||
        die "resolution-survey oracle transport must have exactly one link"
    local source_uid
    if [[ -n "$ROOT" ]]; then
        source_uid="$(id -u)"
    else
        source_uid="${SUDO_UID:-0}"
        [[ "$source_uid" =~ ^[0-9]+$ ]] || die "invalid sudo caller identity"
    fi
    [[ "$(stat -c '%u' "$oracle_transport")" == "$source_uid" ]] ||
        die "resolution-survey oracle transport has the wrong owner"
    local transport_mode transport_mode_value
    transport_mode="$(stat -c '%a' "$oracle_transport")"
    transport_mode_value=$((8#$transport_mode))
    (( (transport_mode_value & 0077) == 0 )) ||
        die "resolution-survey oracle transport must be private"

    local bin config evidence_root survey_root output public_transport
    bin="$(root_path /usr/local/bin/remi)"
    config="$(root_path /etc/conary/remi.toml)"
    evidence_root="$(root_path /conary/evidence)"
    survey_root="${evidence_root}/resolution-surveys"
    output="${survey_root}/${survey_id}"
    public_transport="/tmp/remi-resolution-survey-${survey_id}.tar"
    [[ -f "$bin" && ! -L "$bin" && -x "$bin" ]] ||
        die "installed Remi binary is not a plain executable"
    [[ -f "$config" && ! -L "$config" ]] ||
        die "Remi configuration is not a plain file"
    local control_uid
    if [[ -n "$ROOT" ]]; then
        control_uid="$(id -u)"
    else
        control_uid=0
    fi
    require_secure_benchmark_file "$bin" "installed Remi binary" "$control_uid" 1
    require_secure_benchmark_file "$config" "Remi configuration" "$control_uid" 0
    [[ ! -e "$output" && ! -L "$output" ]] ||
        die "resolution survey already exists: $output"
    [[ ! -e "$public_transport" && ! -L "$public_transport" ]] ||
        die "resolution-survey transport already exists: $public_transport"

    install_owned_dir 0750 "$evidence_root"
    local survey_staging_root="/tmp/remi-resolution-survey-staging"
    if [[ ! -e "$survey_staging_root" && ! -L "$survey_staging_root" ]]; then
        mkdir -m 0750 "$survey_staging_root"
        if [[ -z "$ROOT" ]]; then
            chown root:conary "$survey_staging_root"
        fi
    fi
    [[ -d "$survey_staging_root" && ! -L "$survey_staging_root" \
        && "$(stat -c '%a' "$survey_staging_root")" == "750" \
        && "$(stat -c '%u' "$survey_staging_root")" == "$control_uid" ]] ||
        die "resolution-survey operator staging root is not a private root-owned directory"

    SURVEY_REMI_STOPPED=0
    SURVEY_TRANSPORT_NEXT=""
    SURVEY_STAGING="$(mktemp -d "${survey_staging_root}/resolution-survey-${survey_id}.XXXXXX")"
    trap 'survey_restore_and_exit "$?"' EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM
    chmod 0750 "$SURVEY_STAGING"
    if [[ -z "$ROOT" ]]; then
        chown root:conary "$SURVEY_STAGING"
    fi
    local diagnostic="${SURVEY_STAGING}/diagnostic.log"
    : >"$diagnostic"
    chmod 0600 "$diagnostic"
    [[ "$(stat -c '%u' "$diagnostic")" == "$control_uid" ]] ||
        die "resolution survey diagnostic staging has the wrong owner"
    local input_manifest="${SURVEY_STAGING}/oracle-manifest.json"
    survey_validate_oracle_transport "$survey_id" "$export_id" "$oracle_transport" "$input_manifest"

    local oracle_root="${SURVEY_STAGING}/oracles"
    mkdir -m 0750 "$oracle_root"
    local oracle_members=()
    mapfile -t oracle_members < <(jq -r '.files[].path' "$input_manifest")
    tar -xf "$oracle_transport" -C "$oracle_root" -- "${oracle_members[@]}" ||
        die "could not unpack authenticated resolution-survey oracles"
    find "$oracle_root" -type d -exec chmod 0750 {} +
    find "$oracle_root" -type f -exec chmod 0440 {} +
    if [[ -z "$ROOT" ]]; then
        chown -R root:conary "$oracle_root"
    fi
    survey_validate_unpacked_oracles "$input_manifest" "$oracle_root"

    local observed_binary_sha256
    observed_binary_sha256="$(sha256sum "$bin" | cut -d ' ' -f 1)"
    [[ "$observed_binary_sha256" == "$(jq -r '.deployment.binary_sha256' "$input_manifest")" ]] ||
        die "installed Remi binary differs from the oracle deployment binding"
    if [[ ! -e "$survey_root" && ! -L "$survey_root" ]]; then
        install_owned_dir 0700 "$survey_root"
    fi
    [[ -d "$survey_root" && ! -L "$survey_root" && "$(stat -c '%a' "$survey_root")" == "700" ]] ||
        die "resolution-survey evidence root is not a private plain directory"
    local runtime_uid
    if [[ -n "$ROOT" ]]; then
        runtime_uid="$(id -u)"
    else
        runtime_uid="$(id -u conary)" || die "missing conary service account"
    fi
    [[ "$(stat -c '%u' "$survey_root")" == "$runtime_uid" ]] ||
        die "resolution-survey evidence root has the wrong owner"

    survey_systemctl is-active --quiet remi ||
        die "Remi must be active before a production resolution survey"
    SURVEY_REMI_STOPPED=1
    survey_systemctl stop remi || die "failed to stop Remi for resolution survey"

    local inspection="${SURVEY_STAGING}/candidate-inspection.json"
    "$bin" deployment inspect --config "$config" --require-private-candidates \
        >"$inspection" 2>>"$diagnostic" ||
        die "could not inspect exact stopped-runtime candidate pointers"
    jq -e '
        .configured_profiles == 3
        and .candidate_profiles == 3
        and ([.candidates[].profile] == ["fedora-44", "ubuntu-26.04", "arch"])
        and all(.candidates[];
          (.profile_revision_sha256 | type == "string" and test("^[0-9a-f]{64}$"))
          and (.packages | type == "number" and . > 0))
    ' "$inspection" >/dev/null ||
        die "stopped-runtime inspection does not prove three exact private candidates"
    jq -e --slurpfile inspection "$inspection" '
        [.profiles[].profile_revision_sha256]
        == [$inspection[0].candidates[].profile_revision_sha256]
    ' "$input_manifest" >/dev/null ||
        die "current candidate pointers differ from the authenticated oracle revisions"

    local command=("$bin" resolution-survey --config "$config")
    local profile revision architecture
    while IFS=$'\t' read -r profile revision; do
        command+=(--candidate "${profile}=${revision}")
    done < <(jq -r '.candidates[] | [.profile, .profile_revision_sha256] | @tsv' "$inspection")
    while IFS=$'\t' read -r profile revision architecture; do
        command+=(--package-oracle "${profile}=${oracle_root}/${profile}/package-oracle")
    done < <(jq -r '.profiles[] | [.profile, .profile_revision_sha256, .target_architecture] | @tsv' "$input_manifest")
    while IFS=$'\t' read -r profile revision architecture; do
        command+=(--native-resolution "${profile}=${oracle_root}/${profile}/native-resolution")
    done < <(jq -r '.profiles[] | [.profile, .profile_revision_sha256, .target_architecture] | @tsv' "$input_manifest")
    while IFS=$'\t' read -r profile revision architecture; do
        command+=(--architecture "${profile}=${architecture}")
    done < <(jq -r '.profiles[] | [.profile, .profile_revision_sha256, .target_architecture] | @tsv' "$input_manifest")
    command+=(--output-dir "$output")

    local outcome="${SURVEY_STAGING}/outcome.json"
    local survey_status=0
    if [[ -z "$ROOT" ]]; then
        if runuser -u conary -- "${command[@]}" >"$outcome" 2>>"$diagnostic"; then
            survey_status=0
        else
            survey_status=$?
        fi
    elif "${command[@]}" >"$outcome" 2>>"$diagnostic"; then
        survey_status=0
    else
        survey_status=$?
    fi

    [[ -d "$output" && ! -L "$output" && "$(stat -c '%a' "$output")" == "700" ]] ||
        die "resolution survey did not create its private output directory"
    [[ "$(stat -c '%u' "$output")" == "$runtime_uid" ]] ||
        die "resolution survey output directory has the wrong owner"
    local source_file source_name source_mode frozen_output
    frozen_output="${SURVEY_STAGING}/survey-output"
    mkdir -m 0700 "$frozen_output"
    [[ "$(stat -c '%u' "$frozen_output")" == "$control_uid" ]] ||
        die "resolution-survey frozen output has the wrong owner"
    while IFS= read -r source_file; do
        source_name="$(basename "$source_file")"
        case "$source_name" in
            fedora-44.candidate-resolution-survey.json | \
                fedora-44.candidate-resolution-implementation.json | \
                fedora-44.native-resolution-comparison-survey.json | \
                fedora-44.comparison-resolution-implementation.json | \
                ubuntu-26.04.candidate-resolution-survey.json | \
                ubuntu-26.04.candidate-resolution-implementation.json | \
                ubuntu-26.04.native-resolution-comparison-survey.json | \
                ubuntu-26.04.comparison-resolution-implementation.json | \
                arch.candidate-resolution-survey.json | \
                arch.candidate-resolution-implementation.json | \
                arch.comparison-resolution-implementation.json | \
                arch.native-resolution-comparison-survey.json) ;;
            *) die "resolution survey output contains an unexpected file" ;;
        esac
        [[ -f "$source_file" && ! -L "$source_file" \
            && "$(stat -c '%u' "$source_file")" == "$runtime_uid" ]] ||
            die "resolution survey output is not owned plain data"
        source_mode="$(stat -c '%a' "$source_file")"
        [[ "$source_mode" == "600" ]] ||
            die "resolution survey output file is not private"
        install -m 0600 -- "$source_file" "${frozen_output}/${source_name}"
        cmp -s -- "$source_file" "${frozen_output}/${source_name}" ||
            die "resolution survey output changed while it was frozen"
    done < <(find "$output" -mindepth 1 -maxdepth 1 -type f -printf '%p\n' | sort)
    local unexpected_before_restart
    unexpected_before_restart="$(find "$output" -mindepth 1 -maxdepth 1 ! -type f -print -quit)"
    [[ -z "$unexpected_before_restart" ]] ||
        die "resolution survey output contains a non-plain entry"

    if ! survey_start_and_probe; then
        die "failed to restore Remi after resolution survey"
    fi

    jq -e \
        --arg output "$output" '
        .output_dir == $output
        and .profiles == 3
        and (.roots_walked | type == "number" and . >= 0)
        and (.candidate_failures | type == "number" and . >= 0)
        and (.comparison_mismatches | type == "number" and . >= 0)
        and (.comparison_profiles | type == "number" and . >= 0 and . <= 3)
        and (.profile_results | type == "array" and length == 3)
        and ([.profile_results[].profile] == ["fedora-44", "ubuntu-26.04", "arch"])
        and all(.profile_results[];
          (.candidate | keys | sort) == ["counts", "total_failures"]
          and (.candidate.counts | type == "object")
          and (.candidate.total_failures | type == "number" and . >= 0)
          and if .candidate.total_failures == 0 then
            (.comparison | type == "object")
            and ((.comparison | keys | sort)
              == ["candidate_manifest_sha256", "counts", "total_mismatches"])
            and (.comparison.candidate_manifest_sha256
              | type == "string" and test("^[0-9a-f]{64}$"))
            and (.comparison.counts | type == "object")
            and (.comparison.total_mismatches | type == "number" and . >= 0)
          else .comparison == null end)
        and .roots_walked == ([.profile_results[].candidate.counts.roots_walked] | add)
        and .candidate_failures == ([.profile_results[].candidate.total_failures] | add)
        and .comparison_profiles == ([.profile_results[].comparison | select(. != null)] | length)
        and .comparison_mismatches == (
          [.profile_results[].comparison.total_mismatches // 0] | add)
    ' "$outcome" >/dev/null || {
        die "resolution survey did not return its exact typed outcome"
    }
    local candidate_failures comparison_mismatches
    candidate_failures="$(jq -r '.candidate_failures' "$outcome")"
    comparison_mismatches="$(jq -r '.comparison_mismatches' "$outcome")"
    if (( survey_status == 0 )); then
        (( candidate_failures == 0 && comparison_mismatches == 0 )) ||
            die "resolution survey reported findings with a successful exit status"
    # Remi's top-level bootstrap maps every returned error, including the
    # typed "findings recorded" result, to exit status 101.
    elif (( survey_status == 101 )); then
        (( candidate_failures > 0 || comparison_mismatches > 0 )) || {
            die "resolution survey failed without recording findings"
        }
    else
        die "resolution survey failed with status ${survey_status}"
    fi

    # Everything below reads the root-owned snapshot made while Remi was stopped.
    output="$frozen_output"
    [[ -d "$output" && ! -L "$output" && "$(stat -c '%a' "$output")" == "700" ]] ||
        die "resolution survey did not create its private output directory"
    local candidate_file candidate_implementation_file comparison_file
    local comparison_implementation_file profile_failures
    for profile in fedora-44 ubuntu-26.04 arch; do
        candidate_file="${output}/${profile}.candidate-resolution-survey.json"
        candidate_implementation_file="${output}/${profile}.candidate-resolution-implementation.json"
        [[ -f "$candidate_file" && ! -L "$candidate_file" && "$(stat -c '%a' "$candidate_file")" == "600" ]] ||
            die "${profile} candidate survey is not a private plain file"
        [[ -f "$candidate_implementation_file" && ! -L "$candidate_implementation_file" \
            && "$(stat -c '%a' "$candidate_implementation_file")" == "600" ]] ||
            die "${profile} candidate implementation evidence is not a private plain file"
        profile_failures="$(jq -r --arg profile "$profile" \
            '.profile_results[] | select(.profile == $profile) | .candidate.total_failures' \
            "$outcome")"
        comparison_file="${output}/${profile}.native-resolution-comparison-survey.json"
        comparison_implementation_file="${output}/${profile}.comparison-resolution-implementation.json"
        if [[ "$profile_failures" == "0" ]]; then
            [[ -f "$comparison_file" && ! -L "$comparison_file" && "$(stat -c '%a' "$comparison_file")" == "600" ]] ||
                die "${profile} comparison survey is not a private plain file"
            [[ -f "$comparison_implementation_file" && ! -L "$comparison_implementation_file" \
                && "$(stat -c '%a' "$comparison_implementation_file")" == "600" ]] ||
                die "${profile} comparison implementation evidence is not a private plain file"
        else
            [[ ! -e "$comparison_file" && ! -L "$comparison_file" ]] ||
                die "${profile} comparison survey exists despite candidate failures"
            [[ ! -e "$comparison_implementation_file" && ! -L "$comparison_implementation_file" ]] ||
                die "${profile} comparison implementation evidence exists despite candidate failures"
        fi
    done
    local unexpected
    unexpected="$(find "$output" -mindepth 1 -maxdepth 1 -type f \
        ! -name '*.candidate-resolution-survey.json' \
        ! -name '*.candidate-resolution-implementation.json' \
        ! -name '*.comparison-resolution-implementation.json' \
        ! -name '*.native-resolution-comparison-survey.json' -print -quit)"
    [[ -z "$unexpected" ]] || die "resolution survey output contains an unexpected file"
    unexpected="$(find "$output" -mindepth 1 -maxdepth 1 ! -type f -print -quit)"
    [[ -z "$unexpected" ]] || die "resolution survey output contains a non-plain entry"
    if grep -F -e /conary/ -e /etc/conary/ -e /tmp/ -e /data/ "$output"/*.json >/dev/null; then
        die "resolution survey output contains a private host path"
    fi

    local inventory="${SURVEY_STAGING}/survey-files.json"
    local inventory_rows="${SURVEY_STAGING}/survey-files.jsonl"
    : >"$inventory_rows"
    while IFS= read -r candidate_file; do
        jq -n -c \
            --arg path "$(basename "$candidate_file")" \
            --arg sha256 "$(sha256sum "$candidate_file" | cut -d ' ' -f 1)" \
            --argjson size "$(stat -c '%s' "$candidate_file")" \
            '{path: $path, sha256: $sha256, size: $size}' >>"$inventory_rows"
    done < <(find "$output" -mindepth 1 -maxdepth 1 -type f -printf '%p\n' | sort)
    jq -s -cS . "$inventory_rows" >"$inventory"

    local survey_manifest="${SURVEY_STAGING}/manifest.json"
    local profiles_json="${SURVEY_STAGING}/profiles.json"
    : >"${profiles_json}.jsonl"
    for profile in fedora-44 ubuntu-26.04 arch; do
        jq -n -cS \
            --arg profile "$profile" \
            --arg candidate_file "${profile}.candidate-resolution-survey.json" \
            --arg candidate_implementation_file "${profile}.candidate-resolution-implementation.json" \
            --arg comparison_file "${profile}.native-resolution-comparison-survey.json" \
            --arg comparison_implementation_file "${profile}.comparison-resolution-implementation.json" \
            --slurpfile input "$input_manifest" \
            --slurpfile outcome "$outcome" '
            ($input[0].profiles[] | select(.profile == $profile)) as $binding
            | ($outcome[0].profile_results[] | select(.profile == $profile)) as $result
            | {
                profile: $profile,
                profile_revision_sha256: $binding.profile_revision_sha256,
                target_architecture: $binding.target_architecture,
                package_oracle_manifest_sha256: $binding.package_oracle.manifest_sha256,
                native_resolution_manifest_sha256: $binding.native_resolution.manifest_sha256,
                candidate: {
                  file: $candidate_file,
                  implementation_file: $candidate_implementation_file,
                  counts: $result.candidate.counts,
                  total_failures: $result.candidate.total_failures,
                  error_histogram: $result.candidate.counts.error_kinds
                },
                comparison: (if $result.comparison == null then null else {
                  file: $comparison_file,
                  implementation_file: $comparison_implementation_file,
                  candidate_manifest_sha256: $result.comparison.candidate_manifest_sha256,
                  counts: $result.comparison.counts,
                  total_mismatches: $result.comparison.total_mismatches,
                  mismatch_histogram: $result.comparison.counts.mismatch_kinds,
                  outcome_histogram: $result.comparison.counts.outcome_kind_pairs
                } end)
              }
        ' >>"${profiles_json}.jsonl"
    done
    jq -s -cS . "${profiles_json}.jsonl" >"$profiles_json"
    local manifest_canonical
    manifest_canonical="$(jq -n -cS \
        --arg survey_id "$survey_id" \
        --arg export_id "$export_id" \
        --slurpfile input "$input_manifest" \
        --slurpfile profiles "$profiles_json" \
        --slurpfile files "$inventory" \
        --slurpfile outcome "$outcome" '
        {
          schema_version: 3,
          survey_id: $survey_id,
          export_id: $export_id,
          deployment: $input[0].deployment,
          profiles: $profiles[0],
          counts: {
            profiles: $outcome[0].profiles,
            roots_walked: $outcome[0].roots_walked,
            candidate_failures: $outcome[0].candidate_failures,
            comparison_profiles: $outcome[0].comparison_profiles,
            comparison_mismatches: $outcome[0].comparison_mismatches
          },
          files: $files[0]
        }
    ')" || die "could not construct sanitized resolution-survey manifest"
    printf '%s' "$manifest_canonical" >"$survey_manifest"
    if grep -F -e "$output" -e "$oracle_root" -e "$config" -e /tmp/ "$survey_manifest" >/dev/null; then
        die "sanitized resolution-survey manifest contains a private path"
    fi

    SURVEY_TRANSPORT_NEXT="$(mktemp "/tmp/remi-resolution-survey-${survey_id}.XXXXXX")"
    mapfile -t transport_members < <(jq -r '.files[].path' "$survey_manifest")
    tar -cf "$SURVEY_TRANSPORT_NEXT" \
        -C "$SURVEY_STAGING" manifest.json \
        -C "$output" "${transport_members[@]}"
    chmod 0600 "$SURVEY_TRANSPORT_NEXT"
    if [[ -z "$ROOT" ]]; then
        chown "${SUDO_UID:-0}:${SUDO_GID:-0}" "$SURVEY_TRANSPORT_NEXT"
    fi
    if ! ln "$SURVEY_TRANSPORT_NEXT" "$public_transport"; then
        die "resolution-survey transport target appeared during publication"
    fi
    rm -f "$SURVEY_TRANSPORT_NEXT"
    SURVEY_TRANSPORT_NEXT=""
    local public_sha256 public_bytes
    public_sha256="$(sha256sum "$public_transport" | cut -d ' ' -f 1)"
    public_bytes="$(stat -c '%s' "$public_transport")"

    trap - EXIT INT TERM
    rm -rf -- "$SURVEY_STAGING"
    SURVEY_STAGING=""
    printf 'Resolution survey: survey=%s export=%s transport=%s sha256=%s bytes=%s candidate_failures=%s comparison_mismatches=%s\n' \
        "$survey_id" "$export_id" "$public_transport" "$public_sha256" "$public_bytes" \
        "$candidate_failures" "$comparison_mismatches"
}

BENCHMARK_REMI_STOPPED=0
BENCHMARK_SYSTEMCTL=systemctl
BENCHMARK_FAILURE_ARMED=0
BENCHMARK_FAILURE_EMITTED=0
BENCHMARK_FAILURE_STAGE=internal
BENCHMARK_SERVICE_OUTCOME=not-stopped
BENCHMARK_STOP_ATTEMPTED=0
BENCHMARK_TRANSPORT_NEXT=""

benchmark_systemctl() {
    "$BENCHMARK_SYSTEMCTL" "$@" >/dev/null
}

benchmark_start_and_probe() {
    if ! benchmark_systemctl start remi; then
        echo "remi deploy helper: failed to restart Remi after conversion benchmark" >&2
        return 1
    fi
    if [[ -z "$ROOT" ]]; then
        sleep 2
    fi
    if ! curl -fsS --max-time 30 "$HEALTH_URL" >/dev/null; then
        echo "remi deploy helper: Remi liveness check failed after conversion benchmark" >&2
        return 1
    fi
    BENCHMARK_REMI_STOPPED=0
    BENCHMARK_SERVICE_OUTCOME=restored
}

benchmark_emit_failure() {
    local status="$1"
    local stage="$BENCHMARK_FAILURE_STAGE"
    local service_outcome="$BENCHMARK_SERVICE_OUTCOME"
    [[ "$BENCHMARK_FAILURE_ARMED" == "1" \
        && "$BENCHMARK_FAILURE_EMITTED" == "0" \
        && "$status" != "0" ]] || return 0
    BENCHMARK_FAILURE_EMITTED=1

    case "$stage" in
        request-validation|runtime-authority|systemctl-authority|account-identity|\
        binary-config-authority|live-root-authority|work-root-type|\
        work-root-owner|work-root-mode|work-root-resolution|\
        work-root-separation|work-root-filesystem|work-root-device|\
        benchmark-root-authority|input-target-authority|source-authentication|\
        binary-authentication|private-config-copy|private-source-copy|\
        service-active|service-stop|benchmark-command|raw-report-validation|\
        public-sidecar-validation|service-restore|transport-publication|internal) ;;
        *) stage=internal ;;
    esac
    if [[ ! "$status" =~ ^[1-9][0-9]{0,2}$ ]] || (( status > 255 )); then
        status=1
        stage=internal
    fi
    if [[ "$BENCHMARK_STOP_ATTEMPTED" == "0" ]]; then
        service_outcome=not-stopped
    elif [[ "$service_outcome" != "restored" ]]; then
        service_outcome=restore-failed
    fi
    case "$stage" in
        request-validation|runtime-authority|systemctl-authority|account-identity|\
        binary-config-authority|live-root-authority|work-root-type|\
        work-root-owner|work-root-mode|work-root-resolution|\
        work-root-separation|work-root-filesystem|work-root-device|\
        benchmark-root-authority|input-target-authority|source-authentication|\
        binary-authentication|private-config-copy|private-source-copy|service-active)
            [[ "$service_outcome" == "not-stopped" ]] || stage=internal
            ;;
        service-stop|benchmark-command|raw-report-validation|\
        public-sidecar-validation|service-restore)
            [[ "$service_outcome" == "restored" \
                || "$service_outcome" == "restore-failed" ]] || stage=internal
            ;;
        transport-publication)
            [[ "$service_outcome" == "restored" ]] || stage=internal
            ;;
        internal) ;;
    esac
    printf 'Conversion benchmark failure: {"schema_version":1,"stage":"%s","status":%s,"service_outcome":"%s"}\n' \
        "$stage" "$status" "$service_outcome"
}

benchmark_restore_and_exit() {
    local status="$1"
    trap - EXIT INT TERM
    set +e
    if [[ "$status" == "255" ]]; then
        status=254
        BENCHMARK_FAILURE_STAGE=internal
    fi
    if [[ -n "$BENCHMARK_TRANSPORT_NEXT" ]]; then
        rm -f -- "$BENCHMARK_TRANSPORT_NEXT"
        BENCHMARK_TRANSPORT_NEXT=""
    fi
    if [[ "$BENCHMARK_REMI_STOPPED" == "1" ]]; then
        if ! benchmark_start_and_probe; then
            BENCHMARK_SERVICE_OUTCOME=restore-failed
            if (( status == 0 )); then
                status=1
            fi
        fi
    fi
    benchmark_emit_failure "$status"
    exit "$status"
}

benchmark_filesystem_type() {
    local path="$1"
    local test_type="${CONARY_REMI_DEPLOY_TEST_FILESYSTEM_TYPE:-}"
    local test_root_type="${CONARY_REMI_DEPLOY_TEST_ROOT_FILESYSTEM_TYPE:-$test_type}"
    local test_work_type="${CONARY_REMI_DEPLOY_TEST_WORK_FILESYSTEM_TYPE:-$test_type}"
    if [[ -n "$test_type" ]]; then
        [[ -n "$ROOT" ]] || die "benchmark filesystem override requires a fake root"
        [[ "$test_type" =~ ^[0-9A-Za-z._+-]+$ ]] ||
            die "invalid benchmark filesystem test override"
        [[ "$test_root_type" =~ ^[0-9A-Za-z._+-]+$ ]] ||
            die "invalid benchmark root-filesystem test override"
        [[ "$test_work_type" =~ ^[0-9A-Za-z._+-]+$ ]] ||
            die "invalid benchmark work-filesystem test override"
        case "$path" in
            "$ROOT/conary"|"$ROOT/conary/"*) printf '%s' "$test_type" ;;
            "$ROOT/work"|"$ROOT/work/"*) printf '%s' "$test_work_type" ;;
            *) printf '%s' "$test_root_type" ;;
        esac
        return
    fi
    stat -f -c '%T' "$path" || die "could not inspect benchmark filesystem: $path"
}

benchmark_filesystem_device() {
    local path="$1"
    local test_device="${CONARY_REMI_DEPLOY_TEST_FILESYSTEM_DEVICE:-}"
    local test_work_device="${CONARY_REMI_DEPLOY_TEST_WORK_FILESYSTEM_DEVICE:-$test_device}"
    if [[ -n "$test_device" || -n "$test_work_device" ]]; then
        [[ -n "$ROOT" ]] || die "benchmark filesystem-device override requires a fake root"
        [[ "$test_device" =~ ^[0-9]+$ ]] ||
            die "invalid benchmark filesystem-device test override"
        [[ "$test_work_device" =~ ^[0-9]+$ ]] ||
            die "invalid benchmark work-filesystem-device test override"
        case "$path" in
            "$ROOT/work"|"$ROOT/work/"*) printf '%s' "$test_work_device" ;;
            *) printf '%s' "$test_device" ;;
        esac
        return
    fi
    stat -c '%d' "$path" || die "could not inspect benchmark filesystem device: $path"
}

require_secure_benchmark_file() {
    local path="$1"
    local label="$2"
    local expected_uid="$3"
    local require_executable="$4"
    [[ -f "$path" && ! -L "$path" ]] || die "$label is not a plain file: $path"
    [[ "$(stat -c '%u' "$path")" == "$expected_uid" ]] ||
        die "$label has the wrong owner: $path"
    local mode mode_value
    mode="$(stat -c '%a' "$path")"
    mode_value=$((8#$mode))
    (( (mode_value & 0022) == 0 )) || die "$label must not be group/world writable: $path"
    if [[ "$require_executable" == "1" ]]; then
        [[ -x "$path" ]] || die "$label is not executable: $path"
    fi
}

benchmark_remi_conversion() {
    BENCHMARK_REMI_STOPPED=0
    BENCHMARK_FAILURE_ARMED=1
    BENCHMARK_FAILURE_EMITTED=0
    BENCHMARK_FAILURE_STAGE=request-validation
    BENCHMARK_SERVICE_OUTCOME=not-stopped
    BENCHMARK_STOP_ATTEMPTED=0
    BENCHMARK_TRANSPORT_NEXT=""
    trap 'benchmark_restore_and_exit "$?"' EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM
    [[ $# -eq 7 ]] || usage

    local run_id="$1"
    local expected_binary_sha256="$2"
    local profile="$3"
    local revision_sha256="$4"
    local package_key_sha256="$5"
    local expected_source_sha256="$6"
    local expected_source_size="$7"
    validate_export_id "$run_id"
    validate_sha256 "$expected_binary_sha256"
    validate_profile_id "$profile"
    validate_sha256 "$revision_sha256"
    validate_sha256 "$package_key_sha256"
    validate_sha256 "$expected_source_sha256"
    validate_positive_size "$expected_source_size"

    BENCHMARK_FAILURE_STAGE=runtime-authority
    [[ -n "$ROOT" || "$(id -u)" == "0" ]] || die "helper must run as root"
    [[ "$SKIP_RESTART" == "0" ]] ||
        die "conversion benchmark may not skip Remi service restoration"
    require_shared_conary_root

    BENCHMARK_FAILURE_STAGE=systemctl-authority
    local test_systemctl="${CONARY_REMI_DEPLOY_TEST_SYSTEMCTL:-}"
    if [[ -n "$test_systemctl" ]]; then
        [[ -n "$ROOT" ]] || die "benchmark systemctl override requires a fake root"
        [[ -f "$test_systemctl" && ! -L "$test_systemctl" && -x "$test_systemctl" ]] ||
            die "benchmark systemctl test override is not a plain executable"
        BENCHMARK_SYSTEMCTL="$(realpath -e "$test_systemctl")"
    fi

    BENCHMARK_FAILURE_STAGE=account-identity
    local control_uid control_gid runtime_uid runtime_gid source_uid
    if [[ -n "$ROOT" ]]; then
        control_uid="$(id -u)"
        control_gid="$(id -g)"
        runtime_uid="$(id -u)"
        runtime_gid="$(id -g)"
        source_uid="$(id -u)"
    else
        control_uid=0
        control_gid=0
        runtime_uid="$(id -u conary)" || die "missing conary service account"
        runtime_gid="$(id -g conary)" || die "missing conary service group"
        source_uid="${SUDO_UID:-0}"
        [[ "$source_uid" =~ ^[0-9]+$ ]] || die "invalid sudo caller identity"
    fi

    local bin config live_root work_container benchmark_parent run_root work_root
    local staged_source trusted_config trusted_source raw_report public_sidecar transport
    bin="$(root_path /usr/local/bin/remi)"
    config="$(root_path /etc/conary/remi.toml)"
    live_root="$(root_path /conary)"
    work_container="$(root_path /work)"
    benchmark_parent="${work_container}/remi-conversion-benchmarks"
    run_root="${benchmark_parent}/${run_id}"
    work_root="${run_root}/work"
    staged_source="/tmp/remi-conversion-source-${run_id}.native"
    trusted_config="${run_root}/remi.toml"
    trusted_source="${run_root}/source.native"
    raw_report="${work_root}/conversion-benchmark-v8.json"
    public_sidecar="${work_root}/conversion-benchmark-public-v6.json"
    transport="/tmp/remi-conversion-benchmark-${run_id}.json"

    BENCHMARK_FAILURE_STAGE=binary-config-authority
    require_secure_benchmark_file "$bin" "installed Remi binary" "$control_uid" 1
    require_secure_benchmark_file "$config" "Remi configuration" "$control_uid" 0
    [[ "$(stat -c '%a' "$bin")" == "755" ]] ||
        die "installed Remi binary must have exact mode 0755"
    local config_gid config_mode config_mode_value
    config_gid="$(stat -c '%g' "$config")"
    config_mode="$(stat -c '%a' "$config")"
    config_mode_value=$((8#$config_mode))
    (( (config_mode_value & 0004) != 0 \
        || ((config_mode_value & 0040) != 0 && config_gid == runtime_gid) )) ||
        die "Remi configuration is not readable by the service account"
    if [[ -z "$ROOT" ]]; then
        runuser -u conary -- test -x "$bin" ||
            die "installed Remi binary is not executable by the service account"
        runuser -u conary -- test -r "$config" ||
            die "Remi configuration is not readable by the service account"
    fi

    BENCHMARK_FAILURE_STAGE=live-root-authority
    local live_real live_device work_real work_device benchmark_real benchmark_device
    live_real="$(realpath -e "$live_root")" || die "could not resolve live Remi root"
    [[ "$(benchmark_filesystem_type "$live_real")" == "xfs" ]] ||
        die "live Remi root is not on XFS: $live_real"
    live_device="$(benchmark_filesystem_device "$live_real")"

    BENCHMARK_FAILURE_STAGE=work-root-type
    [[ -d "$work_container" && ! -L "$work_container" ]] ||
        die "benchmark XFS container is not a plain directory: $work_container"

    BENCHMARK_FAILURE_STAGE=work-root-owner
    [[ "$(stat -c '%u:%g' "$work_container")" == "${control_uid}:${control_gid}" ]] ||
        die "benchmark XFS container has the wrong owner: $work_container"

    local work_mode work_mode_value
    BENCHMARK_FAILURE_STAGE=work-root-mode
    work_mode="$(stat -c '%a' "$work_container")"
    work_mode_value=$((8#$work_mode))
    (( (work_mode_value & 0022) == 0 )) ||
        die "benchmark XFS container must not be group/world writable: $work_container"

    BENCHMARK_FAILURE_STAGE=work-root-resolution
    work_real="$(realpath -e "$work_container")" ||
        die "could not resolve benchmark XFS container"

    BENCHMARK_FAILURE_STAGE=work-root-separation
    [[ "$work_real" != "$live_real" \
        && "$work_real" != "$live_real"/* \
        && "$live_real" != "$work_real"/* ]] ||
        die "benchmark XFS container overlaps the live Remi root"
    [[ "$(stat -c '%d:%i' "$work_real")" != "$(stat -c '%d:%i' "$live_real")" ]] ||
        die "benchmark XFS container aliases the live Remi root"

    BENCHMARK_FAILURE_STAGE=work-root-filesystem
    [[ "$(benchmark_filesystem_type "$work_real")" == "xfs" ]] ||
        die "benchmark XFS container is not on XFS: $work_real"

    BENCHMARK_FAILURE_STAGE=work-root-device
    work_device="$(benchmark_filesystem_device "$work_real")"
    [[ "$work_device" == "$live_device" ]] ||
        die "benchmark XFS container is not on the live Remi filesystem device: $work_real"

    BENCHMARK_FAILURE_STAGE=benchmark-root-authority
    if [[ ! -e "$benchmark_parent" && ! -L "$benchmark_parent" ]]; then
        install_owned_dir 0700 "$benchmark_parent"
    fi
    [[ -d "$benchmark_parent" && ! -L "$benchmark_parent" ]] ||
        die "benchmark root is not a plain directory: $benchmark_parent"
    [[ "$(stat -c '%u' "$benchmark_parent")" == "$runtime_uid" ]] ||
        die "benchmark root has the wrong owner: $benchmark_parent"
    [[ "$(stat -c '%a' "$benchmark_parent")" == "700" ]] ||
        die "benchmark root must have mode 0700: $benchmark_parent"

    benchmark_real="$(realpath -e "$benchmark_parent")" || die "could not resolve benchmark root"
    [[ "$benchmark_real" != "$live_real" \
        && "$benchmark_real" != "$live_real"/* \
        && "$live_real" != "$benchmark_real"/* ]] ||
        die "benchmark root overlaps the live Remi root"
    [[ "$(stat -c '%d:%i' "$benchmark_real")" != "$(stat -c '%d:%i' "$live_real")" ]] ||
        die "benchmark root aliases the live Remi root"
    [[ "$(benchmark_filesystem_type "$benchmark_real")" == "xfs" ]] ||
        die "benchmark root is not on XFS: $benchmark_real"
    benchmark_device="$(benchmark_filesystem_device "$benchmark_real")"
    [[ "$benchmark_device" == "$live_device" ]] ||
        die "benchmark root is not on the live Remi filesystem device: $benchmark_real"

    BENCHMARK_FAILURE_STAGE=input-target-authority
    [[ ! -e "$run_root" && ! -L "$run_root" ]] ||
        die "conversion benchmark run already exists: $run_root"
    [[ ! -e "$transport" && ! -L "$transport" ]] ||
        die "conversion benchmark transport already exists: $transport"
    require_secure_benchmark_file "$staged_source" "staged benchmark source" "$source_uid" 0
    [[ "$(stat -c '%h' "$staged_source")" == "1" ]] ||
        die "staged benchmark source must have exactly one link: $staged_source"

    BENCHMARK_FAILURE_STAGE="source-authentication"
    local config_sha256 observed_source_size observed_source_sha256 observed_binary_sha256
    config_sha256="$(sha256sum "$config" | cut -d ' ' -f 1)"
    observed_source_size="$(stat -c '%s' "$staged_source")"
    [[ "$observed_source_size" == "$expected_source_size" ]] ||
        die "staged benchmark source size mismatch"
    observed_source_sha256="$(sha256sum "$staged_source" | cut -d ' ' -f 1)"
    [[ "$observed_source_sha256" == "$expected_source_sha256" ]] ||
        die "staged benchmark source SHA-256 mismatch"
    [[ "$(stat -c '%s' "$staged_source")" == "$expected_source_size" ]] ||
        die "staged benchmark source changed while being authenticated"

    BENCHMARK_FAILURE_STAGE=binary-authentication
    observed_binary_sha256="$(sha256sum "$bin" | cut -d ' ' -f 1)"
    [[ "$observed_binary_sha256" == "$expected_binary_sha256" ]] ||
        die "installed Remi binary SHA-256 mismatch"

    BENCHMARK_FAILURE_STAGE=private-config-copy
    mkdir -m 0700 "$run_root" || die "could not create benchmark run root: $run_root"
    if [[ -z "$ROOT" ]]; then
        chown conary:conary "$run_root"
    fi
    install_owned_file 0400 "$config" "$trusted_config"
    require_secure_benchmark_file "$trusted_config" "trusted benchmark configuration" "$runtime_uid" 0
    [[ "$(stat -c '%a' "$trusted_config")" == "400" ]] ||
        die "trusted benchmark configuration must have mode 0400"
    [[ "$(sha256sum "$trusted_config" | cut -d ' ' -f 1)" == "$config_sha256" \
        && "$(sha256sum "$config" | cut -d ' ' -f 1)" == "$config_sha256" ]] ||
        die "trusted benchmark configuration changed during private copy"

    BENCHMARK_FAILURE_STAGE=private-source-copy
    install_owned_file 0400 "$staged_source" "$trusted_source"
    [[ -f "$trusted_source" && ! -L "$trusted_source" ]] ||
        die "trusted benchmark source copy is not a plain file"
    [[ "$(stat -c '%u' "$trusted_source")" == "$runtime_uid" ]] ||
        die "trusted benchmark source copy has the wrong owner"
    [[ "$(stat -c '%s' "$trusted_source")" == "$expected_source_size" ]] ||
        die "trusted benchmark source copy size mismatch"
    [[ "$(sha256sum "$trusted_source" | cut -d ' ' -f 1)" == "$expected_source_sha256" ]] ||
        die "trusted benchmark source copy SHA-256 mismatch"

    BENCHMARK_FAILURE_STAGE=service-active
    benchmark_systemctl is-active --quiet remi ||
        die "Remi must be active before a production conversion benchmark"

    BENCHMARK_STOP_ATTEMPTED=1
    BENCHMARK_SERVICE_OUTCOME=restore-failed
    BENCHMARK_REMI_STOPPED=1
    BENCHMARK_FAILURE_STAGE=service-stop
    benchmark_systemctl stop remi || die "failed to stop Remi for conversion benchmark"

    BENCHMARK_FAILURE_STAGE=benchmark-command
    local command=(
        "$bin" conversion-benchmark
        --config "$trusted_config"
        --work-root "$work_root"
        --profile "$profile"
        --revision "$revision_sha256"
        --package-key "$package_key_sha256"
        --source-artifact "$trusted_source"
        --hardware-label remi-production-i7-8700-xfs
        --iterations 2
    )
    local benchmark_status=0
    if [[ -z "$ROOT" ]]; then
        if runuser -u conary -- "${command[@]}" >&2; then
            benchmark_status=0
        else
            benchmark_status=$?
        fi
    elif "${command[@]}" >&2; then
        benchmark_status=0
    else
        benchmark_status=$?
    fi
    if (( benchmark_status != 0 )); then
        echo "remi deploy helper: conversion benchmark failed with status ${benchmark_status}" >&2
        exit "$benchmark_status"
    fi

    BENCHMARK_FAILURE_STAGE=raw-report-validation
    [[ -f "$raw_report" && ! -L "$raw_report" ]] ||
        die "conversion benchmark omitted its plain raw report"
    [[ "$(stat -c '%u' "$raw_report")" == "$runtime_uid" ]] ||
        die "conversion benchmark raw report has the wrong owner"
    [[ "$(stat -c '%a' "$raw_report")" == "600" ]] ||
        die "conversion benchmark raw report must have mode 0600"

    local raw_sha256 raw_bytes public_sha256 public_bytes
    raw_sha256="$(sha256sum "$raw_report" | cut -d ' ' -f 1)"
    raw_bytes="$(stat -c '%s' "$raw_report")"
    (( raw_bytes > 0 )) || die "conversion benchmark raw report is empty"
    jq -e '.schema_version == 8 and type == "object"' "$raw_report" >/dev/null ||
        die "conversion benchmark raw report has an invalid schema"

    BENCHMARK_FAILURE_STAGE=public-sidecar-validation
    [[ -f "$public_sidecar" && ! -L "$public_sidecar" ]] ||
        die "conversion benchmark omitted its plain public sidecar"
    [[ "$(stat -c '%u' "$public_sidecar")" == "$runtime_uid" ]] ||
        die "conversion benchmark public sidecar has the wrong owner"
    [[ "$(stat -c '%a' "$public_sidecar")" == "600" ]] ||
        die "conversion benchmark public sidecar must have mode 0600"
    jq -e \
        --arg raw_sha256 "$raw_sha256" \
        --argjson raw_bytes "$raw_bytes" '
        type == "object"
        and .schema_version == 6
        and .raw_report.schema_version == 8
        and .raw_report.sha256 == $raw_sha256
        and .raw_report.size_bytes == $raw_bytes
    ' "$public_sidecar" >/dev/null ||
        die "conversion benchmark public sidecar does not bind the raw report"
    public_sha256="$(sha256sum "$public_sidecar" | cut -d ' ' -f 1)"
    public_bytes="$(stat -c '%s' "$public_sidecar")"
    (( public_bytes > 0 )) || die "conversion benchmark public sidecar is empty"

    BENCHMARK_FAILURE_STAGE=service-restore
    if ! benchmark_start_and_probe; then
        die "failed to restore Remi after conversion benchmark"
    fi

    BENCHMARK_FAILURE_STAGE="transport-publication"
    local transport_next transport_sha256 transport_bytes
    transport_next="$(mktemp "/tmp/remi-conversion-benchmark-${run_id}.XXXXXX")"
    BENCHMARK_TRANSPORT_NEXT="$transport_next"
    install -m 0600 "$public_sidecar" "$transport_next"
    [[ "$(sha256sum "$transport_next" | cut -d ' ' -f 1)" == "$public_sha256" \
        && "$(stat -c '%s' "$transport_next")" == "$public_bytes" ]] ||
        die "conversion benchmark public sidecar changed during transport copy"
    if [[ -z "$ROOT" ]]; then
        chown "${SUDO_UID:-0}:${SUDO_GID:-0}" "$transport_next"
    fi
    if ! ln "$transport_next" "$transport"; then
        die "conversion benchmark transport target appeared during publication: $transport"
    fi
    rm -f "$transport_next"
    BENCHMARK_TRANSPORT_NEXT=""
    transport_sha256="$(sha256sum "$transport" | cut -d ' ' -f 1)"
    transport_bytes="$(stat -c '%s' "$transport")"
    [[ "$transport_sha256" == "$public_sha256" && "$transport_bytes" == "$public_bytes" ]] ||
        die "conversion benchmark transport changed during publication"
    BENCHMARK_FAILURE_ARMED=0
    trap - EXIT INT TERM
    printf 'Conversion benchmark: run=%s transport=%s sha256=%s bytes=%s\n' \
        "$run_id" "$transport" "$transport_sha256" "$transport_bytes"
}

verify_access() {
    [[ -n "$ROOT" || "$(id -u)" == "0" ]] || die "helper must run as root"
    require_shared_conary_root
    [[ -f "$(root_path /etc/conary/remi.toml)" ]] || die "missing /etc/conary/remi.toml"
}

case "${1:-}" in
    deploy-conary)
        [[ $# -eq 3 ]] || usage
        deploy_conary "$2" "$3"
        ;;
    deploy-remi)
        [[ $# -eq 5 ]] || usage
        deploy_remi "$2" "$3" "$4" "$5"
        ;;
    deploy-site)
        [[ $# -eq 3 ]] || usage
        deploy_site "$2" "$3"
        ;;
    publish-test-artifact)
        [[ $# -eq 4 ]] || usage
        publish_test_artifact "$2" "$3" "$4"
        ;;
    install-helper)
        [[ $# -eq 3 ]] || usage
        install_helper "$2" "$3"
        ;;
    inspect-remi)
        shift
        inspect_remi "$@"
        ;;
    inspect-remi-candidate-baseline)
        [[ $# -eq 4 ]] || usage
        inspect_remi_candidate_baseline "$2" "$3" "$4"
        ;;
    inspect-remi-storage)
        [[ $# -eq 1 ]] || usage
        inspect_remi_storage
        ;;
    export-native-oracle-inputs)
        [[ $# -eq 5 ]] || usage
        export_native_oracle_inputs "$2" "$3" "$4" "$5"
        ;;
    survey-resolution)
        shift
        survey_resolution "$@"
        ;;
    benchmark-remi-conversion)
        shift
        benchmark_remi_conversion "$@"
        ;;
    verify-ingress)
        [[ $# -eq 1 ]] || usage
        verify_ingress
        ;;
    verify-access)
        [[ $# -eq 1 ]] || usage
        verify_access
        ;;
    *)
        usage
        ;;
esac
