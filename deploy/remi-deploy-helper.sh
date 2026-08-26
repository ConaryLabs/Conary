#!/usr/bin/env bash
# deploy/remi-deploy-helper.sh -- Root-owned Remi deployment helper.
set -euo pipefail

PATH=/usr/sbin:/usr/bin:/sbin:/bin

ROOT="${CONARY_REMI_DEPLOY_ROOT:-}"
SKIP_RESTART="${CONARY_REMI_DEPLOY_SKIP_RESTART:-0}"
HEALTH_URL="${CONARY_REMI_DEPLOY_HEALTH_URL:-http://localhost:8081/health}"

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
  conary-remi-deploy inspect-remi [--require-private-candidates|--require-repopulated]
  conary-remi-deploy export-native-oracle-inputs <export-id> <fedora-sha256> <ubuntu-sha256> <arch-sha256>
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

validate_sha256() {
    local value="$1"
    [[ "$value" =~ ^[0-9a-f]{64}$ ]] || die "invalid SHA-256 digest"
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

    local conary_root releases_root release_dir self_update_dir
    conary_root="$(root_path /conary)"
    releases_root="$(root_path /conary/releases)"
    release_dir="$(root_path "/conary/releases/${version}")"
    self_update_dir="$(root_path /conary/self-update)"

    install_owned_dir 0750 "$conary_root" "$releases_root" "$release_dir" "$self_update_dir"

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
    install_owned_dir 0750 "$runtime_root"
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
        [[ "$SKIP_RESTART" == "1" ]] || systemctl start remi || true
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

    local conary_root parent target tmp backup
    conary_root="$(root_path /conary)"
    target="$(root_path "/conary/${site_target}")"
    parent="$(dirname "$target")"
    tmp="$(root_path "/conary/.${site_target}.next.$$")"
    backup="$(root_path "/conary/.${site_target}.previous.$$")"

    install_owned_dir 0750 "$conary_root"
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

    local size actual_sha conary_root artifact_root target next
    size="$(stat -c '%s' "$source")"
    (( size > 0 )) || die "test artifact must not be empty"
    (( size <= 8 * 1024 * 1024 * 1024 )) ||
        die "test artifact exceeds the 8 GiB publication limit"
    actual_sha="$(sha256sum "$source" | cut -d ' ' -f 1)"
    [[ "$actual_sha" == "$expected_sha" ]] || die "test-artifact SHA-256 mismatch"

    conary_root="$(root_path /conary)"
    artifact_root="$(root_path /conary/test-artifacts)"
    target="${artifact_root}/${filename}"
    next="${artifact_root}/.${filename}.next.$$"
    install_owned_dir 0750 "$conary_root"
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

install_helper() {
    local expected_sha="$1"
    local source
    validate_sha256 "$expected_sha"
    source="$(real_tmp_path "$2")"
    [[ -f "$source" && ! -L "$source" ]] || die "helper source is not a plain file: $source"

    local actual_sha target next
    actual_sha="$(sha256sum "$source" | cut -d ' ' -f 1)"
    [[ "$actual_sha" == "$expected_sha" ]] || die "helper SHA-256 mismatch"
    bash -n "$source" || die "helper shell validation failed"

    target="$(root_path /usr/local/sbin/conary-remi-deploy)"
    next="${target}.next.$$"
    install -m 0755 "$source" "$next"
    mv "$next" "$target"
    rm -f "$source"
}

inspect_remi() {
    local requirement="${1:-}"
    [[ -z "$requirement" || "$requirement" == "--require-private-candidates" || \
        "$requirement" == "--require-repopulated" ]] ||
        die "invalid inspect-remi option: $requirement"
    local bin
    bin="$(root_path /usr/local/bin/remi)"
    [[ -f "$bin" && ! -L "$bin" ]] || die "Remi binary is not a plain file: $bin"
    local args=(deployment inspect --config "$(root_path /etc/conary/remi.toml)")
    if [[ -n "$requirement" ]]; then
        args+=("$requirement")
    fi
    "$bin" "${args[@]}"
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

    local bin conary_root evidence_root output transport transport_next
    bin="$(root_path /usr/local/bin/remi)"
    [[ -f "$bin" && ! -L "$bin" ]] || die "Remi binary is not a plain file: $bin"
    conary_root="$(root_path /conary)"
    evidence_root="$(root_path /conary/evidence/native-oracle-inputs)"
    output="${evidence_root}/${export_id}"
    transport="/tmp/remi-native-oracle-input-${export_id}.tar"
    transport_next=""
    install_owned_dir 0750 "$conary_root" "$(root_path /conary/evidence)" "$evidence_root"
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

verify_access() {
    [[ "$(id -u)" == "0" ]] || die "helper must run as root"
    [[ -f "$(root_path /etc/conary/remi.toml)" ]] || die "missing /etc/conary/remi.toml"
    [[ "$SKIP_RESTART" == "1" ]] || systemctl status --no-pager remi >/dev/null
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
        [[ $# -le 2 ]] || usage
        inspect_remi "${2:-}"
        ;;
    export-native-oracle-inputs)
        [[ $# -eq 5 ]] || usage
        export_native_oracle_inputs "$2" "$3" "$4" "$5"
        ;;
    verify-access)
        [[ $# -eq 1 ]] || usage
        verify_access
        ;;
    *)
        usage
        ;;
esac
