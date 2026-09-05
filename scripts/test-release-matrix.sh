#!/usr/bin/env bash
# scripts/test-release-matrix.sh
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MATRIX="${REPO_ROOT}/scripts/release-matrix.sh"
TEST_RUN_ROOT="$(mktemp -d "${REPO_ROOT}/.tmp-release-matrix-run.XXXXXX")"

fail() {
    printf 'FAIL: %s\n' "$1" >&2
    exit 1
}

assert_eq() {
    local expected="$1"
    local actual="$2"
    local context="${3:-expected [$expected], got [$actual]}"

    if [[ "$expected" != "$actual" ]]; then
        fail "$context"
    fi
}

assert_contains() {
    local haystack="$1"
    local needle="$2"
    local context="${3:-expected output to contain [$needle]}"

    if [[ "$haystack" != *"$needle"* ]]; then
        fail "$context: $haystack"
    fi
}

cleanup() {
    rm -rf -- "$TEST_RUN_ROOT"
}

trap cleanup EXIT

run_matrix() {
    bash "$MATRIX" "$@"
}

write_cargo_manifest() {
    local file="$1"
    local name="$2"

    cat > "$file" <<EOF
[package]
name = "$name"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
authors.workspace = true
license.workspace = true
publish.workspace = true
EOF
}

create_release_fixture() {
    local repo

    repo="$(mktemp -d "${TEST_RUN_ROOT}/fixture.XXXXXX")"

    mkdir -p \
        "$repo/scripts" \
        "$repo/apps/conary/man" \
        "$repo/apps/remi" \
        "$repo/apps/conaryd" \
        "$repo/apps/conary-test" \
        "$repo/crates/conary-core" \
        "$repo/crates/conary-bootstrap" \
        "$repo/crates/conary-mcp" \
        "$repo/crates/conary-agent-contract" \
        "$repo/packaging/rpm" \
        "$repo/packaging/arch" \
        "$repo/packaging/deb/debian" \
        "$repo/packaging/ccs" \
        "$repo/test-bin"

    cp "$REPO_ROOT/scripts/release.sh" "$repo/scripts/release.sh"
    cp "$REPO_ROOT/scripts/release-matrix.sh" "$repo/scripts/release-matrix.sh"
    chmod +x "$repo/scripts/release.sh" "$repo/scripts/release-matrix.sh"

    write_cargo_manifest "$repo/apps/conary/Cargo.toml" "conary"
    write_cargo_manifest "$repo/crates/conary-core/Cargo.toml" "conary-core"
    write_cargo_manifest "$repo/crates/conary-bootstrap/Cargo.toml" "conary-bootstrap"
    write_cargo_manifest "$repo/apps/remi/Cargo.toml" "remi"
    write_cargo_manifest "$repo/apps/conaryd/Cargo.toml" "conaryd"
    write_cargo_manifest "$repo/apps/conary-test/Cargo.toml" "conary-test"
    write_cargo_manifest "$repo/crates/conary-mcp/Cargo.toml" "conary-mcp"
    write_cargo_manifest "$repo/crates/conary-agent-contract/Cargo.toml" "conary-agent-contract"

    cat > "$repo/Cargo.toml" <<'EOF'
[workspace]
members = [
    "apps/conary",
    "apps/remi",
    "apps/conaryd",
    "apps/conary-test",
    "crates/conary-bootstrap",
    "crates/conary-agent-contract",
    "crates/conary-mcp",
    "crates/conary-core",
]
resolver = "3"

[workspace.package]
version = "0.7.0"
edition = "2024"
rust-version = "1.98.0"
authors = ["Conary Contributors"]
license = "MIT"
publish = false
EOF

    printf 'fn main() {}\n' > "$repo/apps/conary/build.rs"
    printf '.TH conary 1 "" "conary 0.7.0"\n' > "$repo/apps/conary/man/conary.1"
    printf '# release fixture lockfile\n' > "$repo/Cargo.lock"
    printf '/apps/conary/man/\n' > "$repo/.gitignore"
    printf '%s' $'# Changelog\n\nFixture release history.\n\nEntries are newest first.\n\n## [fixture] - 2026-01-01\n' > "$repo/CHANGELOG.md"

    cat > "$repo/test-bin/cargo" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

case "${1:-}" in
    update)
        ;;
    build)
        version="$(sed -nE 's/^version[[:space:]]*=[[:space:]]*"([^"]+)"/\1/p' Cargo.toml | head -n1)"
        if [[ "${RELEASE_FIXTURE_STALE_MAN:-0}" == "1" ]]; then
            version="0.7.0"
        fi
        printf '.TH conary 1 "" "conary %s" \n' "$version" > apps/conary/man/conary.1
        ;;
    *)
        printf 'unexpected cargo fixture command: %s\n' "$*" >&2
        exit 1
        ;;
esac
EOF
    chmod +x "$repo/test-bin/cargo"

    cat > "$repo/packaging/rpm/conary.spec" <<'EOF'
Name:           conary
Version:        0.7.0
Release:        1
EOF

    cat > "$repo/packaging/arch/PKGBUILD" <<'EOF'
pkgname=conary
pkgver=0.7.0
pkgrel=1
EOF

    cat > "$repo/packaging/deb/debian/changelog" <<'EOF'
conary (0.7.0-1) unstable; urgency=medium

  * Release 0.7.0

 -- Conary Contributors <contributors@conary.io>  Thu, 09 Apr 2026 00:00:00 +0000
EOF

    cat > "$repo/packaging/ccs/ccs.toml" <<'EOF'
version = "0.7.0"
EOF

    printf 'initial conary fixture\n' > "$repo/apps/conary/changes.txt"
    printf 'initial remi fixture\n' > "$repo/apps/remi/changes.txt"
    printf 'initial conaryd fixture\n' > "$repo/apps/conaryd/changes.txt"
    printf 'initial conary-test fixture\n' > "$repo/apps/conary-test/changes.txt"

    (
        cd "$repo"
        git init -q
        git config user.name "Release Matrix Test"
        git config user.email "release-matrix@test"
        git add .
        git commit -q -m "chore: initial fixture"
    )

    printf '%s\n' "$repo"
}

tag_head() {
    local repo="$1"
    local tag="$2"

    (
        cd "$repo"
        git tag "$tag"
    )
}

commit_change() {
    local repo="$1"
    local path="$2"
    local message="$3"

    printf '%s\n' "$message" >> "$repo/$path"
    (
        cd "$repo"
        git add "$path"
        git commit -q -m "$message"
    )
}

commit_empty() {
    local repo="$1"
    local message="$2"

    (
        cd "$repo"
        git commit --allow-empty -q -m "$message"
    )
}

run_release_dry_run() {
    local repo="$1"
    local product="$2"
    shift 2

    (
        cd "$repo"
        ./scripts/release.sh "$product" --dry-run "$@"
    )
}

run_release() {
    local repo="$1"
    local product="$2"
    local stale_man="${RELEASE_FIXTURE_STALE_MAN:-0}"
    shift 2

    (
        cd "$repo"
        export PATH="$repo/test-bin:$PATH"
        export RELEASE_FIXTURE_STALE_MAN="$stale_man"
        ./scripts/release.sh "$product" "$@"
    )
}

run_repo_matrix() {
    local repo="$1"
    shift

    (
        cd "$repo"
        ./scripts/release-matrix.sh "$@"
    )
}

create_release_policy_fixture() {
    local repo
    local input

    repo="$(mktemp -d "${TEST_RUN_ROOT}/fixture.XXXXXX")"

    while IFS= read -r input; do
        mkdir -p "$repo/$(dirname "$input")"
        cp "$REPO_ROOT/$input" "$repo/$input"
    done < <(bash "$REPO_ROOT/scripts/check-release-matrix.sh" --list-inputs)
    chmod +x "$repo/scripts/release-matrix.sh"
    printf '%s\n' "$repo"
}

test_bootstrap_installer_contract() {
    bash "$REPO_ROOT/scripts/test-install-conary-preview.sh"
}

test_native_oracle_transport_contract() {
    python3 "$REPO_ROOT/scripts/test-native-oracle-input-transport.py"
}

test_native_oracle_lane_contract() {
    python3 "$REPO_ROOT/scripts/test-produce-native-oracle-lane.py"
}

test_resolution_survey_transport_contract() {
    python3 "$REPO_ROOT/scripts/test-remi-resolution-survey-transport.py"
}

test_native_oracle_assembly_contract() {
    python3 "$REPO_ROOT/scripts/test-assemble-native-oracle-lanes.py"
}

test_native_oracle_lane_selection_contract() {
    python3 "$REPO_ROOT/scripts/test-native-oracle-lane-selection.py"
}

test_native_oracle_producer_verification_contract() {
    python3 "$REPO_ROOT/scripts/test-verify-native-oracle-producer.py"
}

replace_fixture_text_once() {
    local file="$1"
    local old="$2"
    local new="$3"

    python3 - "$file" "$old" "$new" <<'PY'
import sys
from pathlib import Path

path = Path(sys.argv[1])
old = sys.argv[2]
new = sys.argv[3]
text = path.read_text()
if old not in text:
    raise SystemExit(f"fixture could not find text to replace in {path}: {old}")
path.write_text(text.replace(old, new, 1))
PY
}

assert_check_release_matrix_fails() {
    local repo="$1"
    local expected="$2"
    local output status

    set +e
    output="$(bash "$REPO_ROOT/scripts/check-release-matrix.sh" "$repo" 2>&1)"
    status=$?
    set -e

    if [[ "$status" -eq 0 ]]; then
        fail "check-release-matrix should fail for fixture containing $expected"
    fi

    assert_contains "$output" "$expected" "check-release-matrix failure should name $expected"
}

test_resolve_tag_suite_canonical() {
    local output
    output="$(run_matrix resolve-tag v0.15.0 --format shell)"
    assert_contains "$output" "release=suite" "canonical v tag should resolve to the suite"
    assert_contains "$output" "version=0.15.0" "canonical v tag should preserve the exact suite version"
}

test_latest_version_from_list_uses_canonical_tags() {
    local output
    output="$(run_matrix latest-version-from-list suite v0.4.0 v0.10.0 v0.6.0)"
    assert_eq "0.10.0" "$output" "suite tag comparison should choose the highest numeric version"
}

test_field_conary_test_deploy_mode() {
    local output
    output="$(run_matrix artifact-field conary-test deploy_mode)"
    assert_eq "none" "$output" "conary-test should not deploy automatically"
}

test_field_conaryd_build_only_mode() {
    local output
    output="$(run_matrix artifact-field conaryd deploy_mode)"
    assert_eq "none" "$output" "conaryd should remain a build-only suite artifact"
}

test_field_conary_bundle_name() {
    local output
    output="$(run_matrix artifact-field conary bundle_name)"
    assert_eq "release-bundle" "$output" "conary should use the release bundle name"
}

test_metadata_json_is_versioned_and_typed() {
    local output

    output="$(run_matrix metadata-json suite 0.15.0 v0.15.0 true)"
    jq -e '
        .schema_version == 1 and
        (.dry_run | type) == "boolean" and
        .dry_run == true and
        .release == "suite" and
        .version == "0.15.0" and
        (.artifacts | length) == 4
    ' <<< "$output" >/dev/null || fail "suite metadata should be a versioned, typed JSON resource"
}

test_unknown_tag_prefix_fails() {
    local output status

    set +e
    output="$(run_matrix resolve-tag foo-v1.0.0 2>&1)"
    status=$?
    set -e

    if [[ "$status" -eq 0 ]]; then
        fail "unknown tag prefix should fail"
    fi

    assert_contains "$output" "unknown current release tag: foo-v1.0.0" "unknown tag should fail clearly"
}

test_historical_tag_prefixes_are_rejected() {
    local tag output status

    for tag in remi-v0.12.1 conaryd-v0.7.0 conary-test-v0.9.0 server-v0.5.0 test-v0.3.0; do
        set +e
        output="$(run_matrix resolve-tag "$tag" 2>&1)"
        status=$?
        set -e

        if [[ "$status" -eq 0 ]]; then
            fail "historical tag prefix unexpectedly resolved: $tag"
        fi
        assert_contains "$output" "unknown current release tag: $tag" "historical product tag should fail clearly"
    done
}

test_latest_version_from_git_in_fixture() {
    local repo
    local output

    repo="$(create_release_fixture)"
    tag_head "$repo" "v1.0.0"
    commit_empty "$repo" "chore: canonical release point"
    tag_head "$repo" "v2.0.0"

    output="$(run_repo_matrix "$repo" latest-version-from-git suite)"
    assert_eq "2.0.0" "$output" "fixture repo should prefer the highest numeric suite version"
}

test_max_owned_version_in_fixture() {
    local repo
    local output

    repo="$(create_release_fixture)"
    output="$(run_repo_matrix "$repo" max-owned-version suite)"
    assert_eq "0.7.0" "$output" "fixture repo should report the workspace-owned suite version"
}

test_workspace_version_in_fixture() {
    local repo
    local output

    repo="$(create_release_fixture)"
    output="$(run_repo_matrix "$repo" workspace-version)"
    assert_eq "0.7.0" "$output" "fixture repo should expose the root workspace version"
}

test_assert_owned_version_accepts_matching_manifests() {
    local repo

    repo="$(create_release_fixture)"
    run_repo_matrix "$repo" assert-owned-version suite 0.7.0
}

test_assert_owned_version_rejects_mismatched_manifest() {
    local repo output status

    repo="$(create_release_fixture)"
    replace_fixture_text_once \
        "$repo/crates/conary-core/Cargo.toml" \
        'version.workspace = true' \
        'version = "0.7.1"'

    set +e
    output="$(run_repo_matrix "$repo" assert-owned-version suite 0.7.0 2>&1)"
    status=$?
    set -e

    if [[ "$status" -eq 0 ]]; then
        fail "assert-owned-version should reject an independently versioned workspace package"
    fi
    assert_contains \
        "$output" \
        "workspace package version is not inherited from [workspace.package]: crates/conary-core/Cargo.toml" \
        "version inheritance failure should identify the package manifest"
}

test_assert_owned_version_rejects_independent_publish_policy() {
    local repo output status

    repo="$(create_release_fixture)"
    replace_fixture_text_once \
        "$repo/crates/conary-agent-contract/Cargo.toml" \
        'publish.workspace = true' \
        'publish = true'

    set +e
    output="$(run_repo_matrix "$repo" assert-owned-version suite 0.7.0 2>&1)"
    status=$?
    set -e

    if [[ "$status" -eq 0 ]]; then
        fail "assert-owned-version should reject an independently publishable workspace package"
    fi
    assert_contains \
        "$output" \
        "workspace package publish is not inherited from [workspace.package]: crates/conary-agent-contract/Cargo.toml" \
        "publication inheritance failure should identify the package manifest"
}

test_assert_owned_version_rejects_publishable_workspace_root() {
    local repo output status

    repo="$(create_release_fixture)"
    replace_fixture_text_once \
        "$repo/Cargo.toml" \
        'publish = false' \
        'publish = true'

    set +e
    output="$(run_repo_matrix "$repo" assert-owned-version suite 0.7.0 2>&1)"
    status=$?
    set -e

    if [[ "$status" -eq 0 ]]; then
        fail "assert-owned-version should reject a publishable workspace root"
    fi
    assert_contains \
        "$output" \
        "workspace registry publication must be disabled in Cargo.toml [workspace.package]" \
        "root publication failure should identify the workspace authority"
}

test_release_dry_run_uses_one_suite_history() {
    local repo
    local output

    repo="$(create_release_fixture)"
    tag_head "$repo" "v0.7.0"
    commit_change "$repo" "apps/remi/changes.txt" "fix(remi): tighten deploy flow"

    output="$(run_release_dry_run "$repo" suite)"
    assert_contains "$output" "Tag after reviewed merge: v0.7.1" "a Remi fix should advance the one suite line"
}

test_release_dry_run_prefers_highest_numeric_suite_history() {
    local repo
    local output

    repo="$(create_release_fixture)"
    tag_head "$repo" "v1.0.0"
    commit_empty "$repo" "chore: canonical release point"
    tag_head "$repo" "v2.0.0"
    commit_change "$repo" "apps/remi/changes.txt" "fix(remi): tighten deploy flow"

    output="$(run_release_dry_run "$repo" suite)"
    assert_contains "$output" "Immutable tag baseline: v2.0.0" "the suite should choose the highest canonical numeric baseline"
    assert_contains "$output" "Tag after reviewed merge: v2.0.1" "the suite should bump from that baseline"
}

test_release_dry_run_cross_app_feature_selects_one_minor() {
    local repo
    local output

    repo="$(create_release_fixture)"
    tag_head "$repo" "v0.7.0"
    commit_change "$repo" "apps/conaryd/changes.txt" "feat(conaryd): add typed daemon health proof"

    output="$(run_release_dry_run "$repo" suite)"
    assert_contains "$output" "Tag after reviewed merge: v0.8.0" "a feature in any artifact should advance the suite minor"
}

test_release_dry_run_ignores_product_prefixed_history() {
    local repo
    local output

    repo="$(create_release_fixture)"
    tag_head "$repo" "v0.7.0"
    tag_head "$repo" "conary-test-v9.0.0"
    commit_empty "$repo" "release(remi): prepare 9.0.0"
    commit_change "$repo" "apps/conary-test/changes.txt" "fix(test): update bundle layout"

    output="$(run_release_dry_run "$repo" suite)"
    assert_contains "$output" "Immutable tag baseline: v0.7.0" "product-prefixed history must not become suite authority"
    assert_contains "$output" "Tag after reviewed merge: v0.7.1" "the suite should ignore product-prefixed versions"
    if [[ "$output" == *"prepare 9.0.0"* ]]; then
        fail "superseded product release preparation must not enter suite notes"
    fi
}

test_release_dry_run_accepts_explicit_target() {
    local repo
    local output

    repo="$(create_release_fixture)"
    tag_head "$repo" "v0.7.0"
    commit_change "$repo" "apps/remi/changes.txt" "fix(remi): tighten deploy flow"
    commit_change "$repo" "apps/remi/changes.txt" "refactor(remi): hard-cut service schema"

    output="$(run_release_dry_run "$repo" suite --target 0.8.0)"
    assert_contains "$output" "Target authority: explicit" "explicit target should own the release decision"
    assert_contains "$output" "Tag after reviewed merge: v0.8.0" "explicit target should select the exact suite tag"
    assert_contains "$output" "### Fixed" "scoped fixes should be categorized in release notes"
    assert_contains "$output" "- tighten deploy flow" "scoped fix prefixes should be removed"
    assert_contains "$output" "### Changed" "refactors should be categorized in release notes"
    assert_contains "$output" "- hard-cut service schema" "scoped refactor prefixes should be removed"
}

test_release_prepare_only_updates_one_workspace_authority() {
    local repo
    local committed_head
    local output
    local staged_files

    repo="$(create_release_fixture)"
    tag_head "$repo" "v0.7.0"
    commit_change "$repo" "apps/conary-test/changes.txt" "feat(test): add exact lifecycle proof"
    committed_head="$(git -C "$repo" rev-parse HEAD)"

    run_release "$repo" suite --prepare-only --target 0.9.0

    assert_eq "0.9.0" \
        "$(run_repo_matrix "$repo" max-owned-version suite)" \
        "prepare-only should update the one suite version authority"
    run_repo_matrix "$repo" assert-owned-version suite 0.9.0
    assert_eq "$committed_head" "$(git -C "$repo" rev-parse HEAD)" \
        "prepare-only should not create a commit"
    assert_eq "" "$(git -C "$repo" tag --list v0.9.0)" \
        "prepare-only should not create a tag"
    staged_files="$(git -C "$repo" diff --cached --name-only)"
    assert_contains "$staged_files" \
        "Cargo.toml" \
        "prepare-only should stage the root workspace version"
    assert_eq \
        "version.workspace = true" \
        "$(sed -n 's/^\(version\.workspace = true\)$/\1/p' "$repo/crates/conary-agent-contract/Cargo.toml")" \
        "the agent contract should inherit rather than duplicate the suite version"
    assert_contains "$(<"$repo/CHANGELOG.md")" \
        $'- add exact lifecycle proof\n\n## [fixture]' \
        "release notes should leave a blank line before prior history"

    run_release "$repo" suite --prepare-only --target 0.9.0
    assert_eq \
        "1" \
        "$(rg -c '^## \[v0\.9\.0\]' "$repo/CHANGELOG.md")" \
        "repeated preparation should retain one suite changelog entry"
    assert_eq \
        "1" \
        "$(rg -c '^conary \(0\.9\.0-1\)' "$repo/packaging/deb/debian/changelog")" \
        "repeated preparation should retain one Debian release entry"

    output="$(run_release_dry_run "$repo" suite --target 0.9.0)"
    assert_contains "$output" \
        "Tag after reviewed merge: v0.9.0" \
        "prepared explicit target should remain reproducible before publication"
}

test_release_rejects_product_scoped_target() {
    local repo
    local output
    local status

    repo="$(create_release_fixture)"
    tag_head "$repo" "v0.7.0"
    commit_change "$repo" "apps/remi/changes.txt" "fix(remi): tighten deploy flow"

    set +e
    output="$(
        cd "$repo"
        ./scripts/release.sh suite --dry-run --target remi=0.8.0 2>&1
    )"
    status=$?
    set -e

    if [[ "$status" -eq 0 ]]; then
        fail "a product-scoped target should fail after the suite hard cut"
    fi
    assert_contains "$output" \
        "release target must be an exact MAJOR.MINOR.PATCH version" \
        "product-scoped target should fail clearly"
}

test_release_conary_regenerates_and_stages_man_page() {
    local repo
    local output
    local staged_files

    repo="$(create_release_fixture)"
    assert_eq \
        "apps/conary/man/conary.1" \
        "$(git -C "$repo" check-ignore apps/conary/man/conary.1)" \
        "release fixture must reproduce the repository's ignored generated man page"
    tag_head "$repo" "v0.7.0"
    commit_change "$repo" "apps/conary/changes.txt" "fix(conary): refresh command surface"

    output="$(run_release "$repo" suite --prepare-only --target 0.7.1)"
    assert_contains "$output" \
        "Regenerated apps/conary/man/conary.1 for 0.7.1" \
        "suite preparation should regenerate the versioned Conary man page"

    assert_contains \
        "$(<"$repo/apps/conary/man/conary.1")" \
        "conary 0.7.1" \
        "generated man page should contain the release version"
    if grep -Eq '[[:blank:]]$' "$repo/apps/conary/man/conary.1"; then
        fail "generated man page should not contain trailing whitespace"
    fi

    staged_files="$(git -C "$repo" diff --cached --name-only)"
    assert_contains "$staged_files" \
        "apps/conary/man/conary.1" \
        "suite preparation should stage the generated man page"
    assert_eq "" "$(git -C "$repo" tag --list v0.7.1)" \
        "reviewed suite preparation must not create a tag"
}

test_release_conary_rejects_stale_generated_man_page() {
    local repo
    local output
    local status

    repo="$(create_release_fixture)"
    tag_head "$repo" "v0.7.0"
    commit_change "$repo" "apps/conary/changes.txt" "fix(conary): refresh command surface"

    set +e
    output="$(RELEASE_FIXTURE_STALE_MAN=1 run_release "$repo" suite --prepare-only --target 0.7.1 2>&1)"
    status=$?
    set -e

    if [[ "$status" -eq 0 ]]; then
        fail "Conary release should reject a generated man page with the old version"
    fi

    assert_contains "$output" \
        "does not contain Conary version 0.7.1" \
        "stale generated man page should fail before commit and tag"
    assert_eq "" "$(git -C "$repo" tag --list v0.7.1)" \
        "stale generated man page should not create the release tag"
}

release_matrix_mutation_cases() {
    python3 - <<'PY'
import sys

cases = (
    ('test_check_release_matrix_rejects_aliased_conversion_benchmark_authority', 'replace', '.github/workflows/remi-conversion-benchmark.yml', 'concurrency:', 'concurrency: &shared_concurrency', 'forbidden YAML anchors or aliases'),
    ('test_check_release_matrix_rejects_all_profile_retry', 'replace', '.github/workflows/deploy-remi-candidate.yml', 'refresh?force=true&profile=${profile}', 'refresh?force=true', 'retries only exact failed public profiles'),
    ('test_check_release_matrix_rejects_ambiguous_candidate_completion_mode', 'replace', '.github/workflows/deploy-remi-candidate.yml', '          - private-candidates', '          - candidate-ish', 'candidate deploy explicit typed completion mode'),
    ('test_check_release_matrix_rejects_ambiguous_ccs_target_directory', 'replace', 'packaging/ccs/build.sh', '    --target-dir "$TARGET_DIR"', '    --target-dir target', 'CCS wrapper must use one explicit Cargo target directory'),
    ('test_check_release_matrix_rejects_arbitrary_resolution_survey_file_limit', 'replace', 'scripts/remi-resolution-survey-transport.py', '    if not member.isreg() or member.size <= 0 or member.size != expected_size:', '    if not member.isreg() or member.size <= 0 or member.size != expected_size or member.size > 96 * 1024 * 1024:', 'resolution survey arbitrary aggregate output limit'),
    ('test_check_release_matrix_rejects_arbitrary_resolution_survey_input_limit', 'replace', 'deploy/remi-deploy-helper.sh', '    local listing="${manifest}.listing"', '    local transport_size\n    transport_size="$(stat -c \'%s\' "$transport")"\n    (( transport_size <= 32 * 1024 * 1024 * 1024 )) || die "oracle transport too large"\n\n    local listing="${manifest}.listing"', 'resolution survey arbitrary aggregate oracle input limit'),
    ('test_check_release_matrix_rejects_arbitrary_resolution_survey_transport_limit', 'replace', 'scripts/remi-resolution-survey-transport.py', '    metadata = plain_file(args.transport, "survey transport")', '    metadata = plain_file(args.transport, "survey transport", 640 * 1024 * 1024)', 'resolution survey arbitrary aggregate output limit'),
    ('test_check_release_matrix_rejects_arch_bootstrap_partial_upgrade', 'replace', '.github/workflows/release-build.yml', 'pacman -Syu --noconfirm curl openssl sudo ca-certificates', 'pacman -Sy --noconfirm curl openssl sudo ca-certificates', 'Arch bootstrap rehearsal must avoid an unsupported partial upgrade'),
    ('test_check_release_matrix_rejects_arch_debug_split_package_generation', 'replace', 'packaging/arch/PKGBUILD', 'options=(!debug !lto)', 'options=(debug !lto)', 'Arch package must explicitly disable debug split-package generation'),
    ('test_check_release_matrix_rejects_arch_extra_output_blind_spot', 'replace', 'packaging/arch/build.sh', 'package_outputs=("$OUTPUT"/*.pkg.tar.zst)', 'package_outputs=("$EXPECTED_PACKAGE")', 'Arch build must reject every extra package output'),
    ('test_check_release_matrix_rejects_automatic_rpm_rust_flags', 'replace', 'packaging/rpm/conary.spec', '%undefine _auto_set_build_flags', '%global _auto_set_build_flags 1', 'RPM spec must preserve non-debug Fedora Rust flags without overriding the workspace release profile'),
    ('test_check_release_matrix_rejects_beta_maturity_drift', 'replace', '.github/ISSUE_TEMPLATE/pre_alpha_feedback.md', 'name: Pre-Alpha Tester Feedback', 'name: Beta Feedback', 'public maturity surfaces must identify this project as pre-alpha, not beta'),
    ('test_check_release_matrix_rejects_buffered_resolution_survey_documents', 'replace', 'scripts/remi-resolution-survey-transport.py', '        file_entries: dict[str, tuple[int, str]] = {}', '        file_bytes: dict[str, bytes] = {}', 'resolution survey whole-document output buffering'),
    ('test_check_release_matrix_rejects_caller_authorized_helper_update', 'replace', 'deploy/remi-deploy-helper.sh', '    install -m 0755 "${staging}/helper" "$next"', '    install -m 0755 "$source" "$next"', 'Remi helper updates require exact current protected-main bytes from root-trusted HTTPS authority'),
    ('test_check_release_matrix_rejects_candidate_checkout_fencing_policy', 'replace', '.github/workflows/deploy-remi-candidate.yml', '              -f "$workflow_fencing_policy" \\', '              -f deploy/remi-postdeployment-fencing.jq \\', 'private candidate deploy evaluates post-transition fencing from the exact workflow authority, independent of the candidate checkout'),
    ('test_check_release_matrix_rejects_candidate_completion_catalog_rescan', 'replace', '.github/workflows/deploy-remi-candidate.yml', '.candidate_verification.catalog_bytes_hashed == 0', '.candidate_verification.catalog_bytes_hashed >= 0', 'candidate deploy binds one causal bounded private-candidate inspection to the exact transition while retaining full active inspection'),
    ('test_check_release_matrix_rejects_candidate_tier_as_public_refresh_authority', 'replace', '.github/workflows/deploy-remi-candidate.yml', '                    | select(. != "solus")]', '                    | select(true)]', 'retries only exact failed public profiles'),
    ('test_check_release_matrix_rejects_cold_candidate_rebuild', 'replace', '.github/workflows/deploy-remi-candidate.yml', '          verification="$(scripts/remi-candidate-artifact.sh verify \\', '          cargo build -p remi --release --locked\n          verification="$(scripts/remi-candidate-artifact.sh verify \\', 'candidate deploy cold Rust compilation'),
    ('test_check_release_matrix_rejects_collapsed_hot_conversion_phases', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '              and ($hot.timing.phases | map(.phase)) == [', '              and ($hot.timing.phases | map(.phase) | unique) == [', 'conversion benchmark reviewed helper, pinned-host, transport, and public-proof run authority'),
    ('test_check_release_matrix_rejects_collapsed_hot_skipped_phases', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '              and $hot.timing.skipped_phases == [', '              and ($hot.timing.skipped_phases | unique) == [', 'conversion benchmark reviewed helper, pinned-host, transport, and public-proof run authority'),
    ('test_check_release_matrix_rejects_commented_conversion_benchmark_checkout_ref', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '          ref: ${{ github.workflow_sha }}', '          ref: ${{ github.sha }}\n          # ref: ${{ github.workflow_sha }}', 'conversion benchmark exact workflow-revision checkout ref'),
    ('test_check_release_matrix_rejects_commented_conversion_benchmark_host_pin', 'replace', '.github/actions/setup-pinned-production-ssh/action.yml', '        if ! ssh-keygen -F "$host" -f "$known_hosts_path" >/dev/null; then', '        if ! true; then\n        # if ! ssh-keygen -F "$host" -f "$known_hosts_path" >/dev/null; then', 'shared production SSH action must validate and enforce the exclusive protected host identity pin'),
    ('test_check_release_matrix_rejects_commented_conversion_benchmark_input', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '      profile_revision_sha256:\n        description: Exact registered profile revision to hold constant across benchmark runs.\n        required: true', '      profile_revision_sha256:\n        description: Exact registered profile revision to hold constant across benchmark runs.\n        required: false\n        # required: true', 'conversion benchmark typed dispatch inputs'),
    ('test_check_release_matrix_rejects_commented_conversion_benchmark_permissions', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '  actions: read', '  actions: write\n  # actions: read', 'conversion benchmark read-only permissions'),
    ('test_check_release_matrix_rejects_conary_test_deploy_jobs', 'append', '.github/workflows/deploy-and-verify.yml', '', '\n  verify-conary-test:\n    name: verify-conary-test\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo verify\n', 'deployment job for a build-only suite artifact'),
    ('test_check_release_matrix_rejects_conaryd_deploy_jobs_when_paused', 'append', '.github/workflows/deploy-and-verify.yml', '', '\n  deploy-conaryd:\n    name: deploy-conaryd\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo deploy\n', 'deployment job for a build-only suite artifact'),
    ('test_check_release_matrix_rejects_container_native_oracle_shell', 'replace', '.github/workflows/produce-remi-native-oracles.yml', '        shell: bash', '        shell: sh', 'native-oracle production uses Bash inside pinned job containers'),
    ('test_check_release_matrix_rejects_direct_release_publication', 'replace', '.github/workflows/release-build.yml', 'gh release create "$TAG_NAME" \\', 'gh release create "$TAG_NAME" suite-packages/* \\', 'direct published release creation with attached assets'),
    ('test_check_release_matrix_rejects_dirty_native_oracle_producer', 'replace', '.github/workflows/produce-remi-native-oracles.yml', '          [[ -z "$(git status --porcelain)" ]]', '          true', 'native-oracle production selected exact clean producer source and typed lane adapter'),
    ('test_check_release_matrix_rejects_discarded_candidate_failure_inspection', 'replace', '.github/workflows/deploy-remi-candidate.yml', "            > remi-deployment-inspection.json <<'REMOTE_EOF'", "            > /dev/null <<'REMOTE_EOF'", 'candidate deploy retains one validated final typed inspection'),
    ('test_check_release_matrix_rejects_duplicate_conversion_benchmark_authority', 'append', '.github/workflows/remi-conversion-benchmark.yml', '', '\nconcurrency:\n  group: deploy-and-verify\n  cancel-in-progress: false\n', "duplicate key 'concurrency'"),
    ('test_check_release_matrix_rejects_duplicate_fused_conversion_phase', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '                | select(.phase == "independent_transport_reopen") ] | length) == 1', '                | select(.phase == "independent_transport_reopen") ] | length) >= 1', 'conversion benchmark reviewed helper, pinned-host, transport, and public-proof run authority'),
    ('test_check_release_matrix_rejects_duplicate_native_oracle_lane_selection', 'replace', 'scripts/native-oracle-lane-selection.py', '    if len(set(selected)) != len(selected):', '    if False:', 'native-oracle lane selection closed non-empty duplicate-free parser'),
    ('test_check_release_matrix_rejects_duplicate_survey_transport_copy', 'replace', 'deploy/remi-deploy-helper.sh', '        -C "$output" "${transport_members[@]}"', '        -C "$SURVEY_STAGING" "${transport_members[@]}"', 'resolution survey archives the frozen root-owned snapshot without another full copy'),
    ('test_check_release_matrix_rejects_empty_known_hosts_acceptance', 'replace', '.github/actions/setup-pinned-production-ssh/action.yml', '        [[ -n "$SSH_KNOWN_HOSTS" ]] || { echo "production SSH known-hosts pin is required" >&2; exit 1; }', '        [[ -z "$SSH_KNOWN_HOSTS" ]] || { echo "production SSH known-hosts pin is required" >&2; exit 1; }', 'fail closed clearly when the known-hosts input is empty'),
    ('test_check_release_matrix_rejects_executable_resolution_survey_summary', 'replace', '.github/workflows/survey-remi-resolution.yml', '            echo "- oracle run: \\`$ORACLE_RUN_ID\\`"', '            echo "- oracle run: `$ORACLE_RUN_ID`"', 'resolution survey escaped oracle run summary binding'),
    ('test_check_release_matrix_rejects_extra_conversion_benchmark_upload', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '            remi-candidate-manifest.json\n          if-no-files-found: error', '            remi-candidate-manifest.json\n            unexpected-benchmark-debug.json\n          if-no-files-found: error', 'conversion benchmark public-only retained evidence'),
    ('test_check_release_matrix_rejects_flat_nested_outcome_decode', 'replace', 'scripts/remi-resolution-survey-transport.py', 'CANDIDATE_ROOT_STREAM_SPEC = {"outcome": NATIVE_OUTCOME_STREAM_SPEC}', 'CANDIDATE_ROOT_STREAM_SPEC = {}', 'resolution survey verifier streams canonical root records and nested outcomes without whole-document buffering'),
    ('test_check_release_matrix_rejects_fractional_conversion_benchmark_timing', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '                and . <= 9007199254740991\n                and floor == .;', '                and . <= 9007199254740991\n                and true;', 'conversion benchmark reviewed helper, pinned-host, transport, and public-proof run authority'),
    ('test_check_release_matrix_rejects_global_known_hosts_fallback', 'replace', '.github/actions/setup-pinned-production-ssh/action.yml', "          printf '  GlobalKnownHostsFile /dev/null\\n'", "          printf '  GlobalKnownHostsFile /etc/ssh/ssh_known_hosts\\n'", 'shared production SSH action must validate and enforce the exclusive protected host identity pin'),
    ('test_check_release_matrix_rejects_helper_survey_document_slurp', 'replace', 'deploy/remi-deploy-helper.sh', '            --slurpfile outcome "$outcome" \'', '            --slurpfile outcome "$outcome" \\\n            --slurpfile candidate "$candidate_file" \'', 'resolution survey helper whole-document jq buffering'),
    ('test_check_release_matrix_rejects_hidden_native_debug_outputs', 'replace', '.github/workflows/release-build.yml', '          path: packaging/rpm/output/*.rpm', '          path: |\n            packaging/rpm/output/*.rpm\n            !packaging/rpm/output/*debug*', 'native debug artifact filtering'),
    ('test_check_release_matrix_rejects_historical_local_action_authority__01', 'replace', '.github/workflows/build-remi-candidate.yml', '          ref: ${{ github.workflow_sha }}', '          ref: ${{ github.sha }}', 'historical checkout local-action authority'),
    ('test_check_release_matrix_rejects_historical_local_action_authority__02', 'replace', '.github/workflows/deploy-remi-candidate.yml', '          ref: ${{ github.workflow_sha }}', '          ref: ${{ github.sha }}', 'historical checkout local-action authority'),
    ('test_check_release_matrix_rejects_historical_local_action_authority__03', 'replace', '.github/workflows/release-artifact-proof.yml', '          ref: ${{ github.workflow_sha }}', '          ref: ${{ github.sha }}', 'historical checkout local-action authority'),
    ('test_check_release_matrix_rejects_historical_local_action_authority__04', 'replace', '.github/workflows/remi-r2-durability.yml', '          ref: ${{ github.workflow_sha }}', '          ref: ${{ github.sha }}', 'historical checkout local-action authority'),
    ('test_check_release_matrix_rejects_hook_binary_for_hook_free_lifecycle_step', 'replace', 'apps/conary/tests/fixtures/native/run-cross-source-lifecycle-matrix.sh', 'preview="$(run_hook_free_conary install "${v1_package}" \\', 'preview="$(run_conary_requiring_hook CONARY_TEST_SKIP_GENERATION_MOUNT install "${v1_package}" \\', 'published binary hook-free lifecycle coverage'),
    ('test_check_release_matrix_rejects_late_suite_release_notes', 'replace', '.github/workflows/release-build.yml', 'gh release edit "$TAG_NAME" --notes-file "$release_notes"', 'gh release view "$TAG_NAME" --json body', 'immutable-compatible single suite publication sequence'),
    ('test_check_release_matrix_rejects_leaf_manifest_native_versions__01', 'replace', 'packaging/rpm/build.sh', 'VERSION="$(bash "$REPO_ROOT/scripts/release-matrix.sh" workspace-version)"', 'VERSION=$(grep \'^version\' "$REPO_ROOT/apps/conary/Cargo.toml" | head -1)', 'RPM build must use and validate the root workspace version authority'),
    ('test_check_release_matrix_rejects_leaf_manifest_native_versions__02', 'replace', 'packaging/deb/build.sh', 'VERSION="$(bash "$REPO_ROOT/scripts/release-matrix.sh" workspace-version)"', 'VERSION=$(grep \'^version\' "$REPO_ROOT/apps/conary/Cargo.toml" | head -1)', 'DEB build must use and validate the root workspace version authority'),
    ('test_check_release_matrix_rejects_leaf_manifest_native_versions__03', 'replace', 'packaging/arch/build.sh', 'VERSION="$(bash "$REPO_ROOT/scripts/release-matrix.sh" workspace-version)"', 'VERSION=$(grep \'^version\' "$REPO_ROOT/apps/conary/Cargo.toml" | head -1)', 'Arch build must use and validate the root workspace version authority'),
    ('test_check_release_matrix_rejects_lightweight_live_tag_guard', 'replace', '.github/workflows/release-build.yml', '[[ "$(git cat-file -t "refs/tags/${tag_name}")" == "tag" ]] || {', 'true || {', 'live suite build must require an annotated tag at the exact checkout'),
    ('test_check_release_matrix_rejects_literal_ssh_target_fallback', 'replace', '.github/workflows/deploy-remi-candidate.yml', '          target="$REMI_SSH_TARGET"', '          target="${REMI_SSH_TARGET:-operator@ssh.example.test}"', 'literal user@host fallback'),
    ('test_check_release_matrix_rejects_loose_artifact_latency_budget', 'replace', '.github/workflows/deploy-remi-candidate.yml', '          (( availability_ms <= 60000 )) || {', '          (( availability_ms <= 600000 )) || {', 'candidate deploy must download, verify, and budget the exact protected artifact'),
    ('test_check_release_matrix_rejects_loose_candidate_build_policy', 'replace', 'scripts/remi-candidate-artifact.sh', '          and .build.rustflags == ""', '          and (.build.rustflags | type == "string")', 'candidate artifact verifier must recompute version and enforce the exact build and bulk-cache policy'),
    ('test_check_release_matrix_rejects_loose_native_oracle_transport', 'replace', 'scripts/verify-native-oracle-input-transport.py', '        archive = tarfile.open(path, mode="r:")', '        archive = tarfile.open(path, mode="r:*")', 'native-oracle transport strict tar, canonical manifest, inventory, and byte verification'),
    ('test_check_release_matrix_rejects_loose_resolution_survey_manifest_schema', 'replace', 'scripts/remi-resolution-survey-transport.py', '        require_envelope_schema(manifest, OUTPUT_MANIFEST_SCHEMA, "survey output manifest")', '        # Incorrectly accept an obsolete outer manifest.', 'resolution survey strict sanitized output transport verification'),
    ('test_check_release_matrix_rejects_loose_resolution_survey_transport', 'replace', 'scripts/remi-resolution-survey-transport.py', '        archive = tarfile.open(args.transport, mode="r:")', '        archive = tarfile.open(args.transport, mode="r:*")', 'resolution survey strict sanitized output transport verification'),
    ('test_check_release_matrix_rejects_malformed_native_oracle_producer_commit', 'replace', '.github/workflows/produce-remi-native-oracles.yml', '[[ "$PRODUCER_COMMIT" =~ ^[0-9a-f]{40}$ ]]', '[[ -n "$PRODUCER_COMMIT" ]]', 'native-oracle production exact current protected-main and full producer SHA authorization'),
    ('test_check_release_matrix_rejects_merge_validation_production_probes__01', 'replace', '.github/workflows/merge-validation.yml', '      - name: Explain paused remote validation', '      - name: Probe mutable production Remi through the current health script\n        run: ./scripts/remi-health.sh --smoke\n      - name: Explain paused remote validation', 'mutable production Remi probe in source merge validation'),
    ('test_check_release_matrix_rejects_merge_validation_production_probes__02', 'replace', '.github/workflows/merge-validation.yml', '      - name: Explain paused remote validation', '      - name: Probe mutable production Remi directly\n        run: curl -fsS https://remi.conary.io/health\n      - name: Explain paused remote validation', 'mutable production Remi probe in source merge validation'),
    ('test_check_release_matrix_rejects_mismatched_native_oracle_producer_evidence', 'replace', '.github/workflows/produce-remi-native-oracles.yml', '            .producer_commit == $producer_commit and', '            .producer_commit == .deployed_commit and', 'strict and survey producer commit bindings'),
    ('test_check_release_matrix_rejects_missing_candidate_failure_artifact', 'replace', '.github/workflows/deploy-remi-candidate.yml', '        uses: actions/upload-artifact@bbbca2ddaa5d8feaa63e36b76fdaad77386f024f # v7.0.0', '        run: echo "deployment inspection upload removed"', 'candidate deploy retains before-and-after sanitized inspection artifacts'),
    ('test_check_release_matrix_rejects_missing_candidate_phase_evidence', 'replace', '.github/workflows/deploy-remi-candidate.yml', '            start_phase database-transition-and-restart', '            echo "database transition timing removed" >&2', 'candidate deploy retains typed refresh generations, phase timing, and early-failure evidence'),
    ('test_check_release_matrix_rejects_missing_candidate_storage_evidence', 'replace', '.github/workflows/deploy-remi-candidate.yml', '            > remi-predeployment-storage.json', '            > /dev/null', 'candidate deploy retains before-and-after numeric storage evidence'),
    ('test_check_release_matrix_rejects_missing_exact_ccs_asset_assertion', 'replace', '.github/workflows/release-build.yml', '"release-packages/conary-${VERSION}.ccs"', '"release-packages/conary-${VERSION}.ccs.unchecked"', 'exact version-matching CCS release asset assertion'),
    ('test_check_release_matrix_rejects_missing_fused_ccs_output_hash_work', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '                  "ccs_output_bytes",\n                  "ccs_output_bytes_hashed",', '                  "ccs_output_bytes",', 'conversion benchmark reviewed helper, pinned-host, transport, and public-proof run authority'),
    ('test_check_release_matrix_rejects_missing_live_version_assertion', 'replace', '.github/workflows/release-build.yml', 'bash scripts/release-matrix.sh assert-owned-version "$release" "$version"', 'echo "owned version assertion removed"', 'live suite tag must match the workspace-owned version'),
    ('test_check_release_matrix_rejects_missing_local_action_checkout', 'replace', '.github/workflows/deploy-and-verify.yml', '          ref: ${{ github.workflow_sha }}', '          ref: ${{ github.sha }}', 'check out the exact workflow repository before using the local SSH action'),
    ('test_check_release_matrix_rejects_missing_native_oracle_binary_digest', 'replace', '.github/workflows/produce-remi-native-oracles.yml', '            .producer_binaries.resolution == {name:$resolution_name,sha256:$resolution_sha256} and', '            true and', 'strict and survey resolution producer digest bindings'),
    ('test_check_release_matrix_rejects_missing_post_deploy_remi_readiness', 'replace', '.github/workflows/deploy-and-verify.yml', '          body=$(curl -fsS --max-time 30 https://remi.conary.io/health/ready)', '          echo "structured readiness proof removed"', 'exact post-deploy Remi liveness and structured readiness proof'),
    ('test_check_release_matrix_rejects_missing_refresh_causal_floor', 'replace', '.github/workflows/deploy-remi-candidate.yml', 'refresh?force=true&accept_completed_after=${transition_completed_at}', 'refresh?force=true', 'private candidate deploy coalesces one bounded post-transition refresh'),
    ('test_check_release_matrix_rejects_missing_rpm_spool_reopen_counter', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '                  "native_payload_spool_bytes_reread",\n                  "native_payload_spool_file_reopens",', '                  "native_payload_spool_bytes_reread",', 'conversion benchmark reviewed helper, pinned-host, transport, and public-proof run authority'),
    ('test_check_release_matrix_rejects_missing_signer_trust_match', 'replace', '.github/workflows/release-build.yml', 'release signing key does not match an embedded trusted update key', 'release signing key check removed', 'live signing key must match an embedded trusted update key'),
    ('test_check_release_matrix_rejects_missing_tester_authority_boundary', 'replace', '.github/workflows/release-build.yml', 'Publication and released-package proof do not make', 'Publication proves all tester readiness and makes', 'release notes must derive tester authority from versioned launch status'),
    ('test_check_release_matrix_rejects_mixed_candidate_inspection_channels', 'replace', '.github/workflows/deploy-remi-candidate.yml', '                "${helper_args[@]}" 2>/dev/null)"; then', '                "${helper_args[@]}" 2>&1)"; then', 'channel-separated diagnostics'),
    ('test_check_release_matrix_rejects_moved_tag_publication', 'replace', '.github/workflows/release-build.yml', '          verify_release_tag "before draft mutation"', '          echo "tag revalidation removed"', 'suite tag validation before draft mutation'),
    ('test_check_release_matrix_rejects_moving_artifact_proof_toolchain', 'replace', '.github/workflows/release-artifact-proof.yml', '          toolchain: 1.98.0', '          toolchain: stable', 'published artifact proof exact Rust toolchain'),
    ('test_check_release_matrix_rejects_mutable_artifact_proof', 'replace', '.github/workflows/release-artifact-proof.yml', '             "$(jq -r \'.immutable\' <<< "$release_state")" == "true" ]] || {', '             "$(jq -r \'.immutable\' <<< "$release_state")" == "false" ]] || {', 'published artifact proof must reject a draft, mutable, or mismatched GitHub release'),
    ('test_check_release_matrix_rejects_mutable_publication_result', 'replace', '.github/workflows/release-build.yml', '.tag_name == $tag and .draft == false and .immutable == true', '.tag_name == $tag and .draft == false', 'suite publisher must prove exact immutable state after publication'),
    ('test_check_release_matrix_rejects_mutating_native_oracle_authority', 'replace', '.github/workflows/produce-remi-native-oracles.yml', '          python3 operator/scripts/produce-native-oracle-lane.py \\', '          remi conversion-crawl && python3 operator/scripts/produce-native-oracle-lane.py \\', 'native-oracle production generic or mutating authority'),
    ('test_check_release_matrix_rejects_mutating_resolution_survey', 'replace', '.github/workflows/survey-remi-resolution.yml', '              "sudo -n /usr/local/sbin/conary-remi-deploy survey-resolution \'$SURVEY_ID\' \'$EXPORT_ID\' \'$remote_input\'"', '              "sudo -n /usr/local/sbin/conary-remi-deploy promotion-activate \'$SURVEY_ID\' \'$EXPORT_ID\' \'$remote_input\'"', 'resolution survey fixed helper, fail-closed SSH, and independent output verification'),
    ('test_check_release_matrix_rejects_native_oracle_archive_digest_bypass', 'replace', '.github/workflows/produce-remi-native-oracles.yml', '            [[ "$observed" == "$artifact_digest" ]] || {', '            true || {', 'native-oracle assembly exact current or latest successful same-export artifact with archive digest proof'),
    ('test_check_release_matrix_rejects_native_oracle_implementation_pin_drift', 'replace', '.github/workflows/produce-remi-native-oracles.yml', 'projection_schema:5,version:"0.7.36"', 'projection_schema:4,version:"0.7.36"', 'strict and survey RPM implementation pins'),
    ('test_check_release_matrix_rejects_native_oracle_lane_schema_drift', 'replace', '.github/workflows/produce-remi-native-oracles.yml', '            .schema_version == 5 and', '            .schema_version == 4 and', 'native-oracle production exact lane, package, resolution, and implementation schemas'),
    ('test_check_release_matrix_rejects_non_descendant_native_oracle_producer', 'replace', 'scripts/verify-native-oracle-producer.py', '        ["merge-base", "--is-ancestor", deployed_commit, producer_commit],', '        ["merge-base", "--is-ancestor", deployed_commit, deployed_commit],', 'native-oracle shared full-SHA fetch and deployed-producer-main predicate'),
    ('test_check_release_matrix_rejects_non_failing_artifact_upload', 'replace', '.github/workflows/release-build.yml', 'if-no-files-found: error', 'if-no-files-found: warn', 'fail-closed release artifact uploads'),
    ('test_check_release_matrix_rejects_non_tag_static_site_checkout', 'replace', '.github/workflows/deploy-and-verify.yml', 'ref: ${{ needs.resolve.outputs.tag_name }}', 'ref: main', 'static-site checkout must use the serialized release tag'),
    ('test_check_release_matrix_rejects_non_xfs_conversion_benchmark_evidence', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '                and .filesystem_type == "0x58465342"', '                and .filesystem_type == "ext4"', 'conversion benchmark reviewed helper, pinned-host, transport, and public-proof run authority'),
    ('test_check_release_matrix_rejects_nonadvancing_candidate_fence', 'replace', 'deploy/remi-postdeployment-fencing.jq', '                > fencing_epoch($before; $profile)', '                >= fencing_epoch($before; $profile)', 'candidate deploy requires a zero-scan publication-attested post-transition refresh, candidate completion, and advances fences only within one schema authority'),
    ('test_check_release_matrix_rejects_noncanonical_conversion_failure_envelope', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '                && [[ "$canonical_envelope" == "$envelope_json" ]]; then', '                && [[ -n "$canonical_envelope" ]]; then', 'conversion benchmark reviewed helper, pinned-host, transport, and public-proof run authority'),
    ('test_check_release_matrix_rejects_nonexclusive_conversion_failure_upload', 'replace', '.github/workflows/remi-conversion-benchmark.yml', "        if: ${{ steps.benchmark.outputs.result == 'failure' }}", '        if: ${{ failure() }}', 'conversion benchmark mutually exclusive result publication'),
    ('test_check_release_matrix_rejects_nonexclusive_conversion_success_upload', 'replace', '.github/workflows/remi-conversion-benchmark.yml', "        if: ${{ steps.benchmark.outputs.result == 'success' }}", '        if: ${{ success() }}', 'conversion benchmark mutually exclusive result publication'),
    ('test_check_release_matrix_rejects_nonportable_helper_summary_jq', 'replace', 'deploy/remi-deploy-helper.sh', '                comparison: (if $result.comparison == null then null else {', '                comparison: if $result.comparison == null then null else {', 'resolution survey helper builds portable transport summaries from bounded Remi outcome authority'),
    ('test_check_release_matrix_rejects_nonproduction_conversion_benchmark', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '    environment: production', '    environment: staging\n    # environment: production', 'production SSH job must use the production environment for its protected known-hosts secret'),
    ('test_check_release_matrix_rejects_nonproduction_native_oracle_export', 'replace', '.github/workflows/export-remi-native-oracle-inputs.yml', '    environment: production', '    environment: staging', 'production SSH job must use the production environment for its protected known-hosts secret'),
    ('test_check_release_matrix_rejects_nonproduction_native_oracle_production', 'replace', '.github/workflows/produce-remi-native-oracles.yml', '    environment: production', '    environment: staging', 'native-oracle production exact current protected-main and full producer SHA authorization'),
    ('test_check_release_matrix_rejects_nonterminal_conversion_failure_guard', 'replace', '.github/workflows/remi-conversion-benchmark.yml', "        if: ${{ always() && steps.benchmark.outputs.result != 'success' }}", "        if: ${{ steps.benchmark.outputs.result == 'failure' }}", 'conversion benchmark typed terminal failure result'),
    ('test_check_release_matrix_rejects_nonzero_hot_conversion_work', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '              and ($hot.timing.work | all(.. | numbers; . == 0))', '              and ($hot.timing.work | all(.. | numbers; . >= 0))', 'conversion benchmark reviewed helper, pinned-host, transport, and public-proof run authority'),
    ('test_check_release_matrix_rejects_omitted_survey_manifest_budget', 'replace', 'scripts/produce-native-oracle-lane.py', '        "evidence_byte_limit": survey["evidence_byte_limit"],', '', 'native-oracle sanitized survey retains its validated evidence byte limit'),
    ('test_check_release_matrix_rejects_out_of_order_cold_conversion_finalizer_phases', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '                "complete_archive_copy",\n                "independent_transport_reopen",\n                "complete_archive_hash",\n                "database_persistence"', '                "independent_transport_reopen",\n                "complete_archive_copy",\n                "complete_archive_hash",\n                "database_persistence"', 'conversion benchmark reviewed helper, pinned-host, transport, and public-proof run authority'),
    ('test_check_release_matrix_rejects_out_of_order_hot_conversion_finalizer_phases', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '                "complete_archive_copy",\n                "independent_transport_reopen",\n                "complete_archive_hash",\n                "durable_cas_ingestion",\n                "r2_write_through",\n                "database_persistence"', '                "independent_transport_reopen",\n                "complete_archive_copy",\n                "complete_archive_hash",\n                "durable_cas_ingestion",\n                "r2_write_through",\n                "database_persistence"', 'conversion benchmark reviewed helper, pinned-host, transport, and public-proof run authority'),
    ('test_check_release_matrix_rejects_partial_public_conversion_output_shape', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '                and (.output | output_shape)', '                and (.output | type == "object")', 'conversion benchmark reviewed helper, pinned-host, transport, and public-proof run authority'),
    ('test_check_release_matrix_rejects_partial_public_conversion_timing_shape', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '                and (.timing | timing_shape)', '                and (.timing | type == "object")', 'conversion benchmark reviewed helper, pinned-host, transport, and public-proof run authority'),
    ('test_check_release_matrix_rejects_partial_public_conversion_work_shape', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '                timing_shape_without_work and (.work | work_shape);', '                timing_shape_without_work and (.work | type == "object");', 'conversion benchmark reviewed helper, pinned-host, transport, and public-proof run authority'),
    ('test_check_release_matrix_rejects_partial_xfs_conversion_benchmark_proof', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '              and all(.environment.roots[];', '              and any(.environment.roots[];', 'conversion benchmark reviewed helper, pinned-host, transport, and public-proof run authority'),
    ('test_check_release_matrix_rejects_per_object_candidate_cache', 'replace', '.github/actions/setup-remi-candidate-compiler-cache/action.yml', 'SCCACHE_CACHE_BACKEND=local-disk-bulk-v1', 'SCCACHE_CACHE_BACKEND=remote-object-v1', 'candidate compiler cache must use one exact-policy bounded local bulk seed'),
    ('test_check_release_matrix_rejects_post_findings_package_coverage', 'replace', 'scripts/remi-resolution-survey-transport.py', '            candidate_failures += candidate_value["total_failures"]\n            validate_candidate_package_coverage(', '            candidate_failures += candidate_value["total_failures"]\n            validate_candidate_package_coverage_after_findings(', 'resolution survey validates package coverage before the findings branch'),
    ('test_check_release_matrix_rejects_pretransition_candidate_completion', 'replace', 'deploy/remi-postdeployment-fencing.jq', '                > $final.deployment.transition_completed_at))', '                >= 0))', 'candidate deploy requires a zero-scan publication-attested post-transition refresh, candidate completion, and advances fences only within one schema authority'),
    ('test_check_release_matrix_rejects_preupload_conversion_failure_exit', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '            echo "fixed production conversion benchmark operation failed" >&2\n            exit 0', '            echo "fixed production conversion benchmark operation failed" >&2\n            exit "$helper_status"', 'conversion benchmark reviewed helper, pinned-host, transport, and public-proof run authority'),
    ('test_check_release_matrix_rejects_private_conversion_failure_upload', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '          path: remi-conversion-benchmark-failure-v1.json', '          path: ${{ runner.temp }}/remi-conversion-benchmark-helper.stderr', 'conversion benchmark failure-only retained evidence'),
    ('test_check_release_matrix_rejects_private_mode_public_readiness_claim', 'replace', '.github/workflows/deploy-remi-candidate.yml', '          if [[ "$COMPLETION_MODE" == "active-repopulation" ]]; then', '          if true; then', 'candidate deploy mode-specific public readiness contract'),
    ('test_check_release_matrix_rejects_product_scoped_ccs_version', 'replace', 'packaging/ccs/build.sh', 'assert-owned-version suite "$VERSION"', 'assert-owned-version conary "$VERSION"', 'CCS build must validate the root workspace version authority'),
    ('test_check_release_matrix_rejects_published_binary_as_hook_runner', 'replace', '.github/workflows/release-artifact-proof.yml', '          CONARY_HOOKS_BIN: /usr/libexec/conary-test/conary-test-hooks', '          CONARY_BIN: /usr/libexec/conary-test/conary-test-hooks', 'published native package fence and separate test-hook lifecycle proof'),
    ('test_check_release_matrix_rejects_raw_conversion_benchmark_upload', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '            remi-conversion-benchmark-public-v6.json', '            remi-conversion-benchmark-public-v6.json\n            conversion-benchmark-v8.json', 'conversion benchmark public-only retained evidence'),
    ('test_check_release_matrix_rejects_rehearsal_artifact_promotion', 'replace', '.github/workflows/deploy-and-verify.yml', 'elif [[ "$MANUAL_DRY_RUN" == "false" && "$dry_run" == "true" ]]; then', 'elif false; then', 'manual deployment must not promote rehearsal artifacts'),
    ('test_check_release_matrix_rejects_release_tag_local_ssh_action', 'replace', '.github/workflows/deploy-and-verify.yml', '      - name: Check out deploy-remi workflow repository for local actions\n        uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6.0.2\n        with:\n          ref: ${{ github.workflow_sha }}', '      - name: Check out deploy-remi workflow repository for local actions\n        uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6.0.2\n        with:\n          ref: ${{ needs.resolve.outputs.tag_name }}', 'load the local SSH action from the workflow revision after checking out the release tag'),
    ('test_check_release_matrix_rejects_reserved_ssh_conversion_helper_status', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '          if (( helper_status >= 1 && helper_status <= 254 )) \\', '          if (( helper_status >= 1 && helper_status <= 255 )) \\', 'conversion benchmark reviewed helper, pinned-host, transport, and public-proof run authority'),
    ('test_check_release_matrix_rejects_resolution_survey_helper_downgrade', 'replace', '.github/workflows/survey-remi-resolution.yml', '          [[ "$helper_sha256" == "$preinstall_helper_sha256" ]] || {', '          [[ "$helper_sha256" == "$helper_sha256" ]] || {', 'resolution survey revalidates protected main immediately before helper installation'),
    ('test_check_release_matrix_rejects_resolution_survey_lane_digest_bypass', 'replace', '.github/workflows/survey-remi-resolution.yml', '            [[ "$(sha256sum "$lane_archive" | cut -d \' \' -f 1)" == "$artifact_sha256" ]] || {', '            true || {', 'resolution survey independently authenticates every assembled strict lane artifact'),
    ('test_check_release_matrix_rejects_resolution_survey_set_digest_bypass', 'replace', '.github/workflows/survey-remi-resolution.yml', '          [[ "sha256:$(sha256sum "$set_archive" | cut -d \' \' -f 1)" == "$set_digest" ]] || {', '          true || {', 'resolution survey exact assembled oracle to export to deployment run chain'),
    ('test_check_release_matrix_rejects_retained_resolution_survey_lane_payloads', 'replace', '.github/workflows/survey-remi-resolution.yml', '            --consume-lane-files \\', '            # retain every extracted lane member', 'resolution survey releases authenticated lane archives and members while building its transport'),
    ('test_check_release_matrix_rejects_retained_survey_profile_copies', 'replace', 'scripts/remi-resolution-survey-transport.py', '            comparison_path.unlink()', '            pass', 'resolution survey verification stages and deletes one profile at a time'),
    ('test_check_release_matrix_rejects_retired_resolution_survey_envelopes__01', 'replace', 'scripts/remi-resolution-survey-transport.py', 'INPUT_EVIDENCE_SCHEMA = 2', 'INPUT_EVIDENCE_SCHEMA = 1', 'resolution survey hard-cut envelope schemas'),
    ('test_check_release_matrix_rejects_retired_resolution_survey_envelopes__02', 'replace', '.github/workflows/survey-remi-resolution.yml', '              else .schema_version == 3 end)', '              else .schema_version == 2 end)', 'resolution survey verification evidence envelope fences'),
    ('test_check_release_matrix_rejects_rpm_debug_rust_flags', 'replace', 'packaging/rpm/conary.spec', 'echo "Conary effective RUSTFLAGS: $RUSTFLAGS"', 'RUSTFLAGS="$RUSTFLAGS -Cdebuginfo=2"\necho "Conary effective RUSTFLAGS: $RUSTFLAGS"', 'RPM spec debug-oriented Rust flag override'),
    ('test_check_release_matrix_rejects_rpm_debug_subpackage_generation', 'replace', 'packaging/rpm/conary.spec', '%global debug_package %{nil}', '%global debug_package 1', 'RPM spec must explain and disable debug subpackage generation'),
    ('test_check_release_matrix_rejects_rpm_extra_output_blind_spot', 'replace', 'packaging/rpm/build.sh', 'rpm_outputs=("$OUTPUT"/*.rpm)', 'rpm_outputs=("$OUTPUT/$NAME-$VERSION-"*.x86_64.rpm)', 'RPM build must reject every extra package output'),
    ('test_check_release_matrix_rejects_rpm_macro_expansion_in_comment', 'replace', 'packaging/rpm/conary.spec', '# the manual distro macro below populates native dependency toolchain flags.', '# %set_build_flags populates native dependency toolchain flags.', 'RPM spec manual build-flag macro invocation expected 1 occurrences'),
    ('test_check_release_matrix_rejects_single_static_site_deploy', 'replace', '.github/workflows/deploy-and-verify.yml', 'bash deploy/deploy-sites.sh both', 'bash deploy/deploy-sites.sh site', 'both-site deployment from the release tag'),
    ('test_check_release_matrix_rejects_stale_native_oracle_export_before_ssh', 'python-rfind', '.github/workflows/export-remi-native-oracle-inputs.yml', '(git rev-parse origin/main)', 'WORKFLOW_SHA', 'native-oracle export initial and pre-SSH current-main revalidation'),
    ('test_check_release_matrix_rejects_stale_native_oracle_export_operator', 'replace', '.github/workflows/export-remi-native-oracle-inputs.yml', '          [[ "$(git rev-parse origin/main)" == "$WORKFLOW_SHA" ]] || {', '          [[ "$WORKFLOW_SHA" == "$WORKFLOW_SHA" ]] || {', 'native-oracle export initial and pre-SSH current-main revalidation'),
    ('test_check_release_matrix_rejects_stale_native_oracle_export_source', 'replace', '.github/workflows/produce-remi-native-oracles.yml', '            .head_sha == $workflow_sha and', '            (.head_sha | test("^[0-9a-f]{40}$")) and', 'native-oracle production exact current-main successful protected export source'),
    ('test_check_release_matrix_rejects_stale_native_oracle_production_operator', 'replace', '.github/workflows/produce-remi-native-oracles.yml', '          ref: ${{ github.workflow_sha }}', '          ref: main', 'native-oracle production exact current protected-main and full producer SHA authorization'),
    ('test_check_release_matrix_rejects_stale_native_output_policy', 'replace', 'packaging/rpm/build.sh', 'find "$OUTPUT" -maxdepth 1 -name \'*.rpm\' -delete', 'echo "stale RPM output retained"', 'RPM build must clean stale package output'),
    ('test_check_release_matrix_rejects_stale_resolution_survey_oracle_operator', 'replace', 'scripts/remi-resolution-survey-transport.py', '    if oracle_run["head_sha"] != workflow_commit:', '    if oracle_run["head_sha"] != oracle_run["head_sha"]:', 'resolution survey rejects stale oracle workflow authority'),
    ('test_check_release_matrix_rejects_stale_resolution_survey_verifier', 'replace', '.github/workflows/survey-remi-resolution.yml', '          [[ "$(git rev-parse origin/main)" == "$WORKFLOW_SHA" ]] || {', '          [[ "$WORKFLOW_SHA" == "$WORKFLOW_SHA" ]] || {', 'resolution survey exact current protected-main operator boundary'),
    ('test_check_release_matrix_rejects_test_hooks_in_release_workflow', 'replace', '.github/workflows/release-build.yml', '        run: cargo fmt --check', '        run: cargo fmt --check --features test-hooks', 'release-build test-hooks feature'),
    ('test_check_release_matrix_rejects_timing_as_hot_output_identity', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '                independent_transport_reopen_bytes,', '                independent_transport_reopen_bytes,\n                independent_transport_reopen_ms,', 'conversion benchmark reviewed helper, pinned-host, transport, and public-proof run authority'),
    ('test_check_release_matrix_rejects_tmp_survey_oracle_duplication', 'replace', 'deploy/remi-deploy-helper.sh', '    local survey_staging_root="${evidence_root}/.remi-operator-staging"', '    local survey_staging_root="/tmp/remi-resolution-survey-staging"', 'resolution survey materializes unbounded oracles on the evidence capacity domain'),
    ('test_check_release_matrix_rejects_tofu_in_every_production_ssh_workflow__01', 'replace', '.github/workflows/deploy-and-verify.yml', '      - name: Configure pinned production SSH', '      - name: Discover the production SSH host key\n        run: ssh-keyscan ssh.conary.io\n      - name: Configure pinned production SSH', 'live SSH host-key discovery'),
    ('test_check_release_matrix_rejects_tofu_in_every_production_ssh_workflow__02', 'replace', '.github/workflows/deploy-and-verify.yml', '      - name: Configure pinned production SSH', '      - name: Trust the first production SSH host key\n        run: ssh -o StrictHostKeyChecking=accept-new ssh.conary.io true\n      - name: Configure pinned production SSH', 'SSH trust on first use'),
    ('test_check_release_matrix_rejects_tofu_in_every_production_ssh_workflow__03', 'replace', '.github/workflows/deploy-remi-candidate.yml', '      - name: Configure pinned production SSH', '      - name: Discover the production SSH host key\n        run: ssh-keyscan ssh.conary.io\n      - name: Configure pinned production SSH', 'live SSH host-key discovery'),
    ('test_check_release_matrix_rejects_tofu_in_every_production_ssh_workflow__04', 'replace', '.github/workflows/deploy-remi-candidate.yml', '      - name: Configure pinned production SSH', '      - name: Trust the first production SSH host key\n        run: ssh -o StrictHostKeyChecking=accept-new ssh.conary.io true\n      - name: Configure pinned production SSH', 'SSH trust on first use'),
    ('test_check_release_matrix_rejects_tofu_in_every_production_ssh_workflow__05', 'replace', '.github/workflows/deploy-site.yml', '      - name: Configure pinned production SSH', '      - name: Discover the production SSH host key\n        run: ssh-keyscan ssh.conary.io\n      - name: Configure pinned production SSH', 'live SSH host-key discovery'),
    ('test_check_release_matrix_rejects_tofu_in_every_production_ssh_workflow__06', 'replace', '.github/workflows/deploy-site.yml', '      - name: Configure pinned production SSH', '      - name: Trust the first production SSH host key\n        run: ssh -o StrictHostKeyChecking=accept-new ssh.conary.io true\n      - name: Configure pinned production SSH', 'SSH trust on first use'),
    ('test_check_release_matrix_rejects_tofu_in_every_production_ssh_workflow__07', 'replace', '.github/workflows/export-remi-native-oracle-inputs.yml', '      - name: Configure pinned production SSH', '      - name: Discover the production SSH host key\n        run: ssh-keyscan ssh.conary.io\n      - name: Configure pinned production SSH', 'live SSH host-key discovery'),
    ('test_check_release_matrix_rejects_tofu_in_every_production_ssh_workflow__08', 'replace', '.github/workflows/export-remi-native-oracle-inputs.yml', '      - name: Configure pinned production SSH', '      - name: Trust the first production SSH host key\n        run: ssh -o StrictHostKeyChecking=accept-new ssh.conary.io true\n      - name: Configure pinned production SSH', 'SSH trust on first use'),
    ('test_check_release_matrix_rejects_tofu_in_every_production_ssh_workflow__09', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '      - name: Configure pinned production SSH', '      - name: Discover the production SSH host key\n        run: ssh-keyscan ssh.conary.io\n      - name: Configure pinned production SSH', 'live SSH host-key discovery'),
    ('test_check_release_matrix_rejects_tofu_in_every_production_ssh_workflow__10', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '      - name: Configure pinned production SSH', '      - name: Trust the first production SSH host key\n        run: ssh -o StrictHostKeyChecking=accept-new ssh.conary.io true\n      - name: Configure pinned production SSH', 'SSH trust on first use'),
    ('test_check_release_matrix_rejects_tofu_in_every_production_ssh_workflow__11', 'replace', '.github/workflows/remi-r2-durability.yml', '      - name: Configure pinned production SSH', '      - name: Discover the production SSH host key\n        run: ssh-keyscan ssh.conary.io\n      - name: Configure pinned production SSH', 'live SSH host-key discovery'),
    ('test_check_release_matrix_rejects_tofu_in_every_production_ssh_workflow__12', 'replace', '.github/workflows/remi-r2-durability.yml', '      - name: Configure pinned production SSH', '      - name: Trust the first production SSH host key\n        run: ssh -o StrictHostKeyChecking=accept-new ssh.conary.io true\n      - name: Configure pinned production SSH', 'SSH trust on first use'),
    ('test_check_release_matrix_rejects_tofu_in_every_production_ssh_workflow__13', 'replace', '.github/workflows/survey-remi-resolution.yml', '      - name: Configure pinned production SSH', '      - name: Discover the production SSH host key\n        run: ssh-keyscan ssh.conary.io\n      - name: Configure pinned production SSH', 'live SSH host-key discovery'),
    ('test_check_release_matrix_rejects_tofu_in_every_production_ssh_workflow__14', 'replace', '.github/workflows/survey-remi-resolution.yml', '      - name: Configure pinned production SSH', '      - name: Trust the first production SSH host key\n        run: ssh -o StrictHostKeyChecking=accept-new ssh.conary.io true\n      - name: Configure pinned production SSH', 'SSH trust on first use'),
    ('test_check_release_matrix_rejects_unattested_candidate_fencing', 'replace', 'deploy/remi-postdeployment-fencing.jq', '      and ($final.candidate_verification.catalog_bytes_integrity_checked == 0)', '      and ($final.candidate_verification.catalog_bytes_integrity_checked >= 0)', 'candidate deploy requires a zero-scan publication-attested post-transition refresh, candidate completion, and advances fences only within one schema authority'),
    ('test_check_release_matrix_rejects_unattested_native_oracle_export', 'replace', 'scripts/remi-resolution-survey-transport.py', '        or attestation["ssh_host_key_contract"] != "protected-pinned-known-hosts-v1"', '        or attestation["ssh_host_key_contract"] != "live-discovery"', 'resolution survey requires exact pinned export operator evidence'),
    ('test_check_release_matrix_rejects_unattested_native_oracle_production_input', 'replace', '.github/workflows/produce-remi-native-oracles.yml', '            .ssh_host_key_contract == "protected-pinned-known-hosts-v1"', '            .ssh_host_key_contract == "live-discovery"', 'native-oracle production requires the export pinned-SSH operator attestation'),
    ('test_check_release_matrix_rejects_unbound_candidate_binary', 'replace', '.github/workflows/deploy-remi-candidate.yml', '            and .deployment.binary_sha256 == $expected_binary', '            and (.deployment.binary_sha256 | type == "string")', 'candidate deploy binds final evidence to exact commit and binary'),
    ('test_check_release_matrix_rejects_unbound_candidate_package_roots', 'replace', 'scripts/remi-resolution-survey-transport.py', '                if actual != expected:', '                if False:', 'resolution survey candidate roots exactly cover the authenticated package oracle'),
    ('test_check_release_matrix_rejects_unbound_comparison_candidate_manifest', 'replace', 'scripts/remi-resolution-survey-transport.py', '        or survey["candidate_manifest_sha256"] != candidate_manifest_sha256', '        or False', 'resolution survey comparison binds its streamed reconstructed candidate manifest'),
    ('test_check_release_matrix_rejects_unbound_conversion_benchmark_revision', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '          REQUESTED_REVISION: ${{ inputs.profile_revision_sha256 }}', '          REQUESTED_REVISION: ${{ inputs.package_key_sha256 }}', 'conversion benchmark explicit comparable registered revision'),
    ('test_check_release_matrix_rejects_unbound_conversion_helper_identity', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '                    helper_sha256: $helper_sha256,', '                    helper_sha256: $workflow_commit_sha,', 'conversion benchmark reviewed helper, pinned-host, transport, and public-proof run authority'),
    ('test_check_release_matrix_rejects_unbound_fused_ccs_output_hash_work', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '                and $work.ccs_output_bytes_hashed == $output.ccs_size_bytes', '                and $work.ccs_output_bytes_hashed >= 0', 'conversion benchmark reviewed helper, pinned-host, transport, and public-proof run authority'),
    ('test_check_release_matrix_rejects_unbound_native_oracle_producer_source', 'replace', '.github/workflows/produce-remi-native-oracles.yml', '          ref: ${{ needs.authorize.outputs.producer_commit }}', '          ref: ${{ github.sha }}', 'native-oracle production selected exact clean producer source and typed lane adapter'),
    ('test_check_release_matrix_rejects_unbound_proof_metadata_version', 'replace', '.github/workflows/release-artifact-proof.yml', '          [[ "$version" == "$resolved_version" ]] || {', '          true || {', 'published artifact proof must bind metadata to the annotated tag version and suite authority'),
    ('test_check_release_matrix_rejects_unbound_resolution_survey_assembly', 'replace', '.github/workflows/survey-remi-resolution.yml', '            --assembly-evidence "$RUNNER_TEMP/oracle-set/evidence.json" \\', '            # assembled oracle binding removed', 'resolution survey downloads and authenticates every exact current-operator assembled input'),
    ('test_check_release_matrix_rejects_unbound_resolution_survey_comparison_roots', 'replace', 'scripts/remi-resolution-survey-transport.py', '    if roots != candidate_roots_walked or len(candidate_survey["outcomes"]) != roots:', '    if False:', 'resolution survey comparison binds the exact candidate root population'),
    ('test_check_release_matrix_rejects_unbound_resolution_survey_output', 'replace', '.github/workflows/survey-remi-resolution.yml', '            --input-evidence resolution-survey-input-verification.json \\', '            # authenticated input binding removed', 'resolution survey fixed helper, fail-closed SSH, and independent output verification'),
    ('test_check_release_matrix_rejects_unbound_rpm_payload_hash_work', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '                    == $work.native_payload_bytes_spooled', '                    >= $work.native_payload_bytes_spooled', 'conversion benchmark reviewed helper, pinned-host, transport, and public-proof run authority'),
    ('test_check_release_matrix_rejects_unbound_rpm_spool_declared_geometry', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '                    == $work.native_payload_declared_bytes', '                    >= $work.native_payload_declared_bytes', 'conversion benchmark reviewed helper, pinned-host, transport, and public-proof run authority'),
    ('test_check_release_matrix_rejects_unbound_rpm_spool_reopen_counter', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '                  and $work.native_payload_spool_file_reopens == 0', '                  and $work.native_payload_spool_file_reopens >= 0', 'conversion benchmark reviewed helper, pinned-host, transport, and public-proof run authority'),
    ('test_check_release_matrix_rejects_unbound_rpm_spool_reread_counter', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '                  and $work.native_payload_spool_bytes_reread == 0', '                  and $work.native_payload_spool_bytes_reread >= 0', 'conversion benchmark reviewed helper, pinned-host, transport, and public-proof run authority'),
    ('test_check_release_matrix_rejects_unbounded_candidate_completion_inspection', 'replace', '.github/workflows/deploy-remi-candidate.yml', '--accept-candidates-completed-after "$completed_after"', '--config "$completed_after"', 'candidate deploy binds one causal bounded private-candidate inspection to the exact transition while retaining full active inspection'),
    ('test_check_release_matrix_rejects_unbounded_candidate_transport', 'replace', '.github/actions/setup-pinned-production-ssh/action.yml', "          printf '  ServerAliveInterval 30\\n'", "          printf '  ServerAliveInterval 0\\n'", 'shared production SSH action must validate and enforce the exclusive protected host identity pin'),
    ('test_check_release_matrix_rejects_unbounded_conversion_helper_stdout', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '            && (( stdout_bytes <= 4096 )) \\', '            && (( stdout_bytes <= 65536 )) \\', 'conversion benchmark reviewed helper, pinned-host, transport, and public-proof run authority'),
    ('test_check_release_matrix_rejects_unbounded_fused_conversion_timing', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '                | .duration_ms) <= $cold.timing.total_ms', '                | .duration_ms) >= 0', 'conversion benchmark reviewed helper, pinned-host, transport, and public-proof run authority'),
    ('test_check_release_matrix_rejects_unchecked_survey_manifest_budget', 'replace', '.github/workflows/produce-remi-native-oracles.yml', '            .survey.evidence_byte_limit == 33554432 and', '', 'native-oracle workflow validates the sanitized survey evidence byte limit'),
    ('test_check_release_matrix_rejects_unforced_post_deploy_candidates', 'replace', '.github/workflows/deploy-remi-candidate.yml', 'refresh?force=true&accept_completed_after=${transition_completed_at}', 'refresh?force=false&accept_completed_after=${transition_completed_at}', 'private candidate deploy coalesces one bounded post-transition refresh'),
    ('test_check_release_matrix_rejects_unmerged_deployed_candidate', 'replace', '.github/workflows/export-remi-native-oracle-inputs.yml', '          git merge-base --is-ancestor "$deployed_commit" origin/main || {', '          true || {', 'native-oracle export reopens the exact merged deployed candidate from refresh-bound evidence'),
    ('test_check_release_matrix_rejects_unmerged_live_tag', 'replace', '.github/workflows/release-build.yml', '              refs/heads/main:refs/remotes/origin/main', '              main', 'live suite tag must already be reachable from a freshly fetched main'),
    ('test_check_release_matrix_rejects_unmerged_native_oracle_producer', 'replace', 'scripts/verify-native-oracle-producer.py', '        ["merge-base", "--is-ancestor", producer_commit, "origin/main"],', '        ["merge-base", "--is-ancestor", producer_commit, producer_commit],', 'native-oracle shared full-SHA fetch and deployed-producer-main predicate'),
    ('test_check_release_matrix_rejects_unpinned_arch_builder_image', 'replace', '.github/workflows/release-build.yml', 'image: docker.io/library/archlinux@sha256:fe6972d4dc1f660c0c10f4c41b2de8986bab89e7e2955378f8beadb8ebcd7433', 'image: docker.io/library/archlinux:latest', 'release-build Arch builder must use the pinned Arch image'),
    ('test_check_release_matrix_rejects_unpinned_arch_toolchain', 'replace', '.github/workflows/release-build.yml', 'rustup default 1.98.0', 'rustup default stable', 'release-build Arch builder pinned Rust toolchain'),
    ('test_check_release_matrix_rejects_unpinned_deb_builder_image', 'replace', '.github/workflows/release-build.yml', 'image: docker.io/library/ubuntu@sha256:3131b4cc82a783df6c9df078f86e01819a13594b865c2cad47bd1bca2b7063bb', 'image: docker.io/library/ubuntu:26.04', 'release-build DEB builder must use the pinned Ubuntu 26.04 image'),
    ('test_check_release_matrix_rejects_unpinned_rpm_builder_image', 'replace', '.github/workflows/release-build.yml', 'image: registry.fedoraproject.org/fedora@sha256:765b2260aa4b4eff379b9a6f983f15fcf41a6f9dda9b272b790e23e92fcbaafb', 'image: registry.fedoraproject.org/fedora:44', 'release-build RPM builder must use the pinned Fedora 44 image'),
    ('test_check_release_matrix_rejects_unprotected_candidate_artifact', 'replace', '.github/workflows/deploy-remi-candidate.yml', '                | select(.event == "push")', '                | select(.event == "pull_request")', 'candidate deploy must select only the exact successful protected-main build'),
    ('test_check_release_matrix_rejects_unprotected_conversion_benchmark_operator_checkout', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '          [[ "$(git rev-parse HEAD)" == "$WORKFLOW_SHA" ]] || {', '          [[ "$(git rev-parse HEAD^)" == "$WORKFLOW_SHA" ]] || {', 'conversion benchmark protected merged-main operator boundary'),
    ('test_check_release_matrix_rejects_unprotected_conversion_benchmark_source', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '              and .conclusion == "success"', '              and .conclusion == "failure"', 'conversion benchmark exact successful protected deployment source'),
    ('test_check_release_matrix_rejects_unprotected_native_oracle_source', 'replace', '.github/workflows/export-remi-native-oracle-inputs.yml', '              and .conclusion == "success"', '              and .conclusion == "failure"', 'native-oracle export exact successful protected deployment source'),
    ('test_check_release_matrix_rejects_unprotected_resolution_survey_helper', 'replace', '.github/workflows/survey-remi-resolution.yml', '          ref: ${{ github.workflow_sha }}', '          ref: main', 'resolution survey exact current protected-main operator boundary'),
    ('test_check_release_matrix_rejects_unproven_namespace_action', 'replace', '.github/actions/setup-exact-ownership-tests/action.yml', '        unshare --user --map-root-user --mount --propagation private /bin/true', '        /bin/true', 'exact ownership namespace proof'),
    ('test_check_release_matrix_rejects_unrecomputed_native_comparison', 'replace', 'scripts/remi-resolution-survey-transport.py', '    if comparison_survey["counts"] != expected_counts:', '    if False:', 'resolution survey recomputes comparison authority from authenticated native roots'),
    ('test_check_release_matrix_rejects_unreopened_survey_oracle_transport', 'replace', '.github/workflows/survey-remi-resolution.yml', '            --oracle-transport "$ORACLE_TRANSPORT" \\', '            --oracle-transport "$SURVEY_TRANSPORT" \\', 'resolution survey fixed helper, fail-closed SSH, and independent output verification'),
    ('test_check_release_matrix_rejects_unsanitized_conversion_failure_stage', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '          failure_stage="helper-envelope-invalid"', '          failure_stage="internal"', 'conversion benchmark reviewed helper, pinned-host, transport, and public-proof run authority'),
    ('test_check_release_matrix_rejects_unserialized_conversion_benchmark', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '  group: deploy-and-verify', '  group: remi-conversion-benchmark-production\n  # group: deploy-and-verify', 'conversion benchmark serialized with deployment and verification'),
    ('test_check_release_matrix_rejects_unserialized_native_oracle_export', 'replace', '.github/workflows/export-remi-native-oracle-inputs.yml', '  group: deploy-and-verify', '  group: remi-native-oracle-input-export-production', 'native-oracle export serialized with candidate and release deployment'),
    ('test_check_release_matrix_rejects_unserialized_r2_durability', 'replace', '.github/workflows/remi-r2-durability.yml', '  group: deploy-and-verify', '  group: remi-r2-durability-production', 'R2 durability serialized with production host authority'),
    ('test_check_release_matrix_rejects_unserialized_resolution_survey', 'replace', '.github/workflows/survey-remi-resolution.yml', '  group: deploy-and-verify', '  group: remi-resolution-survey', 'resolution survey exact oracle input, read-only permissions, and shared serialization'),
    ('test_check_release_matrix_rejects_unserialized_site_deployment', 'replace', '.github/workflows/deploy-site.yml', '  group: deploy-and-verify', '  group: deploy-site-production', 'site deployment serialized with production host authority'),
    ('test_check_release_matrix_rejects_unsigned_embedded_ccs_authority', 'replace', '.github/workflows/release-build.yml', 'target/release/examples/sign_hash --write-ccs-authority "$authority_dir"', 'echo "embedded CCS authority removed"', 'CCS build must derive embedded authority from the configured release seed'),
    ('test_check_release_matrix_rejects_unstable_ccs_release_name', 'replace', 'packaging/ccs/build.sh', 'mv -- "$BUILT_CCS" "$EXPECTED_CCS"', 'echo "stable CCS release name removed"', 'CCS wrapper must normalize one exact package-release name to the stable self-update asset'),
    ('test_check_release_matrix_rejects_untyped_candidate_baseline_failure', 'replace', '.github/workflows/deploy-remi-candidate.yml', '                      failure_phase: "predeployment-candidate-baseline",', '                      failure_phase: null,', 'candidate deploy verifies the exact staged candidate, validates the schema-compatible live baseline, and retains typed preflight failure evidence before mutation'),
    ('test_check_release_matrix_rejects_untyped_incomplete_candidate_baseline', 'replace', 'deploy/remi-predeployment-inspection.jq', '  . == null or (', '  . != null or (', 'candidate deploy baseline must distinguish an absent candidate'),
    ('test_check_release_matrix_rejects_untyped_refresh_coalescing_result', 'replace', '.github/workflows/deploy-remi-candidate.yml', '                  and (.coalesced | type == "boolean")', '                  and true', 'private candidate deploy coalesces one bounded post-transition refresh'),
    ('test_check_release_matrix_rejects_untyped_survey_aggregate_counts', 'replace', 'scripts/remi-resolution-survey-transport.py', '        exact_nonnegative_int(value, f"survey counts.{key}")', '        pass', 'resolution survey aggregate manifest counts retain exact integer types'),
    ('test_check_release_matrix_rejects_untyped_workspace_toolchain_setup', 'replace', '.github/actions/setup-rust-workspace/action.yml', '        toolchain: ${{ inputs.toolchain }}', '        toolchain: stable', 'shared workspace setup exact workspace Rust default and typed toolchain input'),
    ('test_check_release_matrix_rejects_unvalidated_candidate_inspection', 'replace', '.github/workflows/deploy-remi-candidate.yml', '            deployment_inspection_is_typed() {', '            deployment_inspection_is_untyped() {', 'channel-separated diagnostics'),
    ('test_check_release_matrix_rejects_unvalidated_native_oracle_survey', 'replace', 'scripts/produce-native-oracle-lane.py', '        survey = write_resolution_survey(', '        survey = resolution_survey_evidence(', 'native-oracle lane writes and validates diagnostics survey before strict resolution'),
    ('test_check_release_matrix_rejects_unverified_embedded_ccs_authority', 'replace', '.github/workflows/release-build.yml', '          target/release/conary ccs verify \\', '          echo "embedded CCS verification removed" \\', 'CCS build must verify its embedded release authority'),
    ('test_check_release_matrix_rejects_unverified_remi_suite_bundle', 'replace', '.github/workflows/deploy-and-verify.yml', '            sha256sum -c SHA256SUMS\n          )\n          bundle="${bundle_dir}/remi-${VERSION}-linux-x64.tar.gz"\n          [[ -s "$bundle" && ! -L "$bundle" ]] || { echo "remi bundle missing, empty, or symlinked" >&2; exit 1; }\n          target="$REMI_SSH_TARGET"', '            echo "suite checksum verification removed"\n          )\n          bundle="${bundle_dir}/remi-${VERSION}-linux-x64.tar.gz"\n          [[ -s "$bundle" && ! -L "$bundle" ]] || { echo "remi bundle missing, empty, or symlinked" >&2; exit 1; }\n          target="$REMI_SSH_TARGET"', 'Remi deployment must verify the complete suite checksums before staging its bundle'),
    ('test_check_release_matrix_rejects_unverified_rustup_init', 'replace', '.github/workflows/release-build.yml', '20a06e644b0d9bd2fbdbfd52d42540bdde820ea7df86e92e533c073da0cdd43c  /tmp/rustup-init', '0000000000000000000000000000000000000000000000000000000000000000  /tmp/rustup-init', 'release-build RPM builder checksum-pinned rustup-init flow'),
    ('test_check_release_matrix_rejects_ustar_resolution_survey_input', 'replace', 'scripts/remi-resolution-survey-transport.py', 'ORACLE_TRANSPORT_TAR_FORMAT = tarfile.GNU_FORMAT', 'ORACLE_TRANSPORT_TAR_FORMAT = tarfile.USTAR_FORMAT', 'resolution survey input transport supports unbounded member sizes'),
    ('test_check_release_matrix_rejects_variable_conversion_benchmark_transport', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '          remote_transport="/tmp/remi-conversion-benchmark-${benchmark_id}.json"', '          remote_transport="/tmp/remi-conversion-benchmark-${benchmark_id}-raw.json"', 'conversion benchmark reviewed helper, pinned-host, transport, and public-proof run authority'),
    ('test_check_release_matrix_rejects_workflow_head_as_deployed_candidate', 'replace', '.github/workflows/export-remi-native-oracle-inputs.yml', '          deployed_commit="$(jq -r \'.deployment.commit_sha\' "$inspection")"', '          deployed_commit="$(git rev-parse HEAD)"', 'native-oracle export reopens the exact merged deployed candidate from refresh-bound evidence'),
    ('test_check_release_matrix_rejects_workspace_rust_version_drift', 'replace', 'Cargo.toml', 'rust-version = "1.98.0"', 'rust-version = "1.99.0"', 'shared workspace setup exact workspace Rust default'),
    ('test_check_release_matrix_rejects_wrong_candidate_inspection_predicate', 'replace', '.github/workflows/deploy-remi-candidate.yml', '                requirement=--require-private-candidates', '                requirement=--require-repopulated', 'candidate deploy mode-specific typed inspection predicate'),
    ('test_check_release_matrix_rejects_xfs_conversion_fallback_syncs', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '                and $work.cas_fallback_object_syncs == 0', '                and $work.cas_fallback_object_syncs >= 0', 'conversion benchmark reviewed helper, pinned-host, transport, and public-proof run authority'),
    ('test_check_release_matrix_requires_bounded_dependency_review_retry', 'replace', '.github/workflows/pr-gate.yml', '          for attempt in 1 2 3 4; do', '          for attempt in 1; do', 'dependency review API retry'),
    ('test_check_release_matrix_requires_hosted_alpm_parity_producer__01', 'replace', '.github/workflows/pr-gate.yml', '        run: cargo test -p conary-core --features native-alpm-oracle repository::catalog::parity::alpm --verbose', '        run: echo "ALPM producer proof removed"', 'hosted'),
    ('test_check_release_matrix_requires_hosted_alpm_parity_producer__02', 'replace', '.github/workflows/merge-validation.yml', '        run: cargo test -p conary-core --features native-alpm-oracle repository::catalog::parity::alpm --verbose', '        run: echo "ALPM producer proof removed"', 'hosted'),
    ('test_check_release_matrix_requires_hosted_debian_parity_producer__01', 'replace', '.github/workflows/pr-gate.yml', '        run: cargo test -p conary-core --features native-debian-oracle repository::catalog::parity::debian --verbose', '        run: echo "Debian producer proof removed"', 'hosted'),
    ('test_check_release_matrix_requires_hosted_debian_parity_producer__02', 'replace', '.github/workflows/merge-validation.yml', '        run: cargo test -p conary-core --features native-debian-oracle repository::catalog::parity::debian --verbose', '        run: echo "Debian producer proof removed"', 'hosted'),
    ('test_check_release_matrix_requires_hosted_debian_resolution_binary__01', 'replace', '.github/workflows/pr-gate.yml', ' --bin conary-debian-resolution-oracle', '', 'hosted'),
    ('test_check_release_matrix_requires_hosted_debian_resolution_binary__02', 'replace', '.github/workflows/merge-validation.yml', ' --bin conary-debian-resolution-oracle', '', 'hosted'),
    ('test_check_release_matrix_requires_hosted_rpm_parity_producer__01', 'replace', '.github/workflows/pr-gate.yml', '        run: cargo test -p conary-core --features native-rpm-oracle repository::catalog::parity::rpm --verbose', '        run: echo "RPM producer proof removed"', 'hosted'),
    ('test_check_release_matrix_requires_hosted_rpm_parity_producer__02', 'replace', '.github/workflows/merge-validation.yml', '        run: cargo test -p conary-core --features native-rpm-oracle repository::catalog::parity::rpm --verbose', '        run: echo "RPM producer proof removed"', 'hosted'),
    ('test_check_release_matrix_requires_named_hook_for_every_container_mutation__01', 'replace', 'apps/conary/tests/fixtures/native/run-cross-source-lifecycle-matrix.sh', 'run_conary_requiring_hook CONARY_TEST_SKIP_GENERATION_MOUNT install "${v1_package}"', 'run_conary_requiring_hook CONARY_TEST_UNDECLARED install "${v1_package}"', 'four explicit hook-dependent lifecycle mutations'),
    ('test_check_release_matrix_requires_named_hook_for_every_container_mutation__02', 'replace', 'apps/conary/tests/fixtures/native/run-cross-source-lifecycle-matrix.sh', 'run_conary_requiring_hook CONARY_TEST_SKIP_GENERATION_MOUNT install "${v2_package}"', 'run_conary_requiring_hook CONARY_TEST_UNDECLARED install "${v2_package}"', 'four explicit hook-dependent lifecycle mutations'),
    ('test_check_release_matrix_requires_named_hook_for_every_container_mutation__03', 'replace', 'apps/conary/tests/fixtures/native/run-cross-source-lifecycle-matrix.sh', 'run_conary_requiring_hook CONARY_TEST_SKIP_GENERATION_MOUNT \\', 'run_conary_requiring_hook CONARY_TEST_UNDECLARED \\', 'four explicit hook-dependent lifecycle mutations'),
    ('test_check_release_matrix_requires_named_hook_for_every_container_mutation__04', 'replace', 'apps/conary/tests/fixtures/native/run-cross-source-lifecycle-matrix.sh', 'run_conary_requiring_hook CONARY_TEST_SKIP_GENERATION_MOUNT remove "${package_name}"', 'run_conary_requiring_hook CONARY_TEST_UNDECLARED remove "${package_name}"', 'four explicit hook-dependent lifecycle mutations'),
    ('test_check_release_matrix_requires_ordinary_conary_hook_fence', 'replace', '.github/workflows/release-build.yml', '        run: cargo test -p conary --no-default-features --test test_hook_ownership --verbose', '        run: echo "ordinary Conary hook fence removed"', 'release ordinary Conary test-hook fence'),
    ('test_check_release_matrix_requires_pinned_conversion_benchmark_host', 'replace', '.github/workflows/remi-conversion-benchmark.yml', '          known-hosts: ${{ secrets.REMI_SSH_KNOWN_HOSTS }}', '          known-hosts: ${{ secrets.REMI_SSH_KEY }}', 'protected production SSH known-hosts secret'),
    ('test_check_release_matrix_requires_pinned_native_oracle_export_host', 'replace', '.github/workflows/export-remi-native-oracle-inputs.yml', '          known-hosts: ${{ secrets.REMI_SSH_KNOWN_HOSTS }}', '          known-hosts: ${{ secrets.REMI_SSH_KEY }}', 'protected production SSH known-hosts secret'),
    ('test_check_release_matrix_requires_pinned_resolution_survey_host', 'replace', '.github/workflows/survey-remi-resolution.yml', '          known-hosts: ${{ secrets.REMI_SSH_KNOWN_HOSTS }}', '          known-hosts: ${{ secrets.REMI_SSH_KEY }}', 'protected production SSH known-hosts secret'),
    ('test_check_release_matrix_requires_production_environment_for_ssh_jobs', 'replace', '.github/workflows/deploy-and-verify.yml', '    environment: production', '    environment: staging', 'production SSH job must use the production environment for its protected known-hosts secret'),
    ('test_check_release_matrix_requires_resilient_alpm_archive_downloads__01', 'replace', '.github/workflows/pr-gate.yml', "          sed -i '/^\\[options\\]/a DisableDownloadTimeout' /etc/pacman.conf", '          true', 'hosted'),
    ('test_check_release_matrix_requires_resilient_alpm_archive_downloads__02', 'replace', '.github/workflows/merge-validation.yml', "          sed -i '/^\\[options\\]/a DisableDownloadTimeout' /etc/pacman.conf", '          true', 'hosted'),
    ('test_check_release_matrix_requires_resolution_survey_helper_install', 'replace', '.github/workflows/survey-remi-resolution.yml', '            "sudo -n /usr/local/sbin/conary-remi-deploy install-helper \'$helper_sha256\' \'$remote_helper\'"', '            "sudo -n /usr/local/sbin/conary-remi-deploy verify-access"', 'resolution survey installs its exact protected helper before staging survey input'),
    ('test_check_release_matrix_requires_rpm_parity_completion_budget__01', 'replace', '.github/workflows/pr-gate.yml', '    timeout-minutes: 60', '    timeout-minutes: 30', 'hosted'),
    ('test_check_release_matrix_requires_rpm_parity_completion_budget__02', 'replace', '.github/workflows/merge-validation.yml', '    timeout-minutes: 60', '    timeout-minutes: 30', 'hosted'),
    ('test_check_release_matrix_requires_shared_namespace_setup_in_every_workspace_lane__01', 'replace', '.github/workflows/pr-gate.yml', '        uses: ./.github/actions/setup-exact-ownership-tests', '        run: echo "exact ownership setup removed"', 'shared exact ownership setup'),
    ('test_check_release_matrix_requires_shared_namespace_setup_in_every_workspace_lane__02', 'replace', '.github/workflows/merge-validation.yml', '        uses: ./.github/actions/setup-exact-ownership-tests', '        run: echo "exact ownership setup removed"', 'shared exact ownership setup'),
    ('test_check_release_matrix_requires_shared_namespace_setup_in_every_workspace_lane__03', 'replace', '.github/workflows/release-build.yml', '        uses: ./.github/actions/setup-exact-ownership-tests', '        run: echo "exact ownership setup removed"', 'shared exact ownership setup'),
)
for case in cases:
    for field in case:
        sys.stdout.buffer.write(field.encode() + b"\0")
PY
}

run_release_policy_mutation_cases() {
    local name kind file old new expected repo

    while IFS= read -r -d '' name \
        && IFS= read -r -d '' kind \
        && IFS= read -r -d '' file \
        && IFS= read -r -d '' old \
        && IFS= read -r -d '' new \
        && IFS= read -r -d '' expected; do
        repo="$(create_release_policy_fixture)"
        case "$kind" in
            replace)
                replace_fixture_text_once "$repo/$file" "$old" "$new"
                ;;
            append)
                printf '%s' "$new" >> "$repo/$file"
                ;;
            python-rfind)
                python3 - "$repo/$file" "$old" "$new" <<'PY'
import sys
from pathlib import Path

path = Path(sys.argv[1])
text = path.read_text()
position = text.rfind(sys.argv[2])
if position < 0:
    raise SystemExit("fixture could not find final mutation target")
path.write_text(text[:position] + sys.argv[3] + text[position + len(sys.argv[2]):])
PY
                ;;
            *)
                fail "unknown release-policy mutation kind $kind"
                ;;
        esac
        assert_check_release_matrix_fails "$repo" "$expected"
        printf 'ok - %s
' "$name"
    done < <(release_matrix_mutation_cases)
}
















test_check_release_matrix_rejects_unpinned_ccs_toolchain() {
    local repo
    repo="$(create_release_policy_fixture)"
    sed -i \
        '/^  build-ccs:/,/^  build-rpm:/{s/toolchain: 1\.98\.0/toolchain: stable/;}' \
        "$repo/.github/workflows/release-build.yml"

    assert_check_release_matrix_fails "$repo" "release-build CCS builder pinned Rust toolchain"
}




test_check_release_matrix_rejects_moving_release_preparation_toolchain() {
    local repo
    repo="$(create_release_policy_fixture)"
    sed -i \
        '/^  build-remi:/,/^  build-conaryd:/{s/toolchain: 1\.98\.0/toolchain: stable/;}' \
        "$repo/.github/workflows/release-build.yml"

    assert_check_release_matrix_fails \
        "$repo" \
        "build-remi exact workspace toolchain must precede Cargo-backed release preparation"
}









remi_predeployment_inspection_fixture() {
    jq -n '
      def refresh($run; $epoch; $state; $candidate_members): {
        run_id: $run,
        fencing_epoch: $epoch,
        state: $state,
        started_at: 10,
        heartbeat_at: 11,
        finished_at: 12,
        failure_stage: null,
        failure_category: null,
        failure_evidence_sha256: null,
        failure_diagnostic: null,
        run_members: 2,
        candidate_members: $candidate_members,
        redactions: []
      };
      {
        baseline_schema_version: 1,
        schema_epoch: "conary-current-v1",
        schema_revision: 55,
        configured_profiles: 3,
        candidate_profiles: 2,
        candidates: [
          {
            profile: "fedora-44",
            configured_sources: 2,
            identity: {
              profile_revision_sha256:
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
              run_id: "fedora-run",
              completed_at: 12
            },
            latest_refresh: refresh("fedora-run"; 6; "candidate"; 2)
          },
          {
            profile: "ubuntu-26.04",
            configured_sources: 16,
            identity: null,
            latest_refresh: refresh("ubuntu-failed"; 6; "abandoned"; 0)
          },
          {
            profile: "arch",
            configured_sources: 3,
            identity: {
              profile_revision_sha256:
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
              run_id: "arch-run",
              completed_at: 12
            },
            latest_refresh: refresh("arch-run"; 5; "candidate"; 3)
          }
        ],
        measurement: {
          wall_time_micros: 12000,
          user_cpu_micros: 4000,
          system_cpu_micros: 2000,
          max_rss_bytes: 10485760,
          sqlite_statements: 84,
          sqlite_page_cache_misses: 12,
          sqlite_logical_read_bytes: 49152,
          catalog_file_opens: 0,
          catalog_bytes_read: 0,
          output_bytes: 2048
        }
      }
    '
}

test_remi_predeployment_filter_accepts_typed_incomplete_baseline() {
    local fixture filter
    fixture="$(remi_predeployment_inspection_fixture)"
    filter="$REPO_ROOT/deploy/remi-predeployment-inspection.jq"

    jq -e -f "$filter" <<<"$fixture" >/dev/null ||
        fail "typed incomplete predeployment baseline was rejected"

    if jq '.candidates[1].identity = {run_id: "orphan-run"}' <<<"$fixture" \
        | jq -e -f "$filter" >/dev/null; then
        fail "half-present candidate identity passed predeployment validation"
    fi
    if jq '.candidates[1].profile = "arch"' <<<"$fixture" \
        | jq -e -f "$filter" >/dev/null; then
        fail "duplicate public profile passed predeployment validation"
    fi
    if jq '.measurement.catalog_file_opens = 1' <<<"$fixture" \
        | jq -e -f "$filter" >/dev/null; then
        fail "catalog-reading baseline passed predeployment validation"
    fi
    if jq '.measurement.wall_time_micros = 2000001' <<<"$fixture" \
        | jq -e -f "$filter" >/dev/null; then
        fail "over-budget baseline passed predeployment validation"
    fi
}

remi_postdeployment_fencing_fixture() {
    local schema_revision="$1"
    local fedora_epoch="$2"
    local ubuntu_epoch="$3"
    local arch_epoch="$4"

    jq -n \
        --argjson schema_revision "$schema_revision" \
        --argjson fedora_epoch "$fedora_epoch" \
        --argjson ubuntu_epoch "$ubuntu_epoch" \
        --argjson arch_epoch "$arch_epoch" '
      def candidate($profile; $run; $epoch; $sources): {
        profile: $profile,
        configured_sources: $sources,
        packages: 1,
        profile_revision_sha256:
          "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        run_id: $run,
        latest_refresh: {
          run_id: $run,
          fencing_epoch: $epoch,
          state: "candidate",
          started_at: 21,
          heartbeat_at: 22,
          finished_at: 23,
          failure_stage: null,
          failure_category: null,
          run_members: $sources,
          candidate_members: $sources,
          failure_evidence_sha256: null,
          failure_diagnostic: null,
          redactions: []
        }
      };
      {
        schema_epoch: "conary-current-v1",
        schema_revision: $schema_revision,
        candidate_verification: {
          mode: "publication_attested",
          completed_after: 20,
          elapsed_micros: 1000,
          catalog_files_reopened: 0,
          catalog_bytes_hashed: 0,
          catalog_bytes_integrity_checked: 0
        },
        deployment: {
          transition_completed_at: 20,
          repository_refreshes: [{
            generation: 1,
            scope: {kind: "all"},
            force: false,
            started_at: 21,
            finished_at: 24,
            coalesced: true,
            status: "complete",
            synced: 22,
            skipped: 0,
            failed: 0,
            successful_profiles: ["arch", "fedora-44", "solus", "ubuntu-26.04"],
            failed_profiles: []
          }]
        },
        candidates: [
          candidate("fedora-44"; "fedora-final"; $fedora_epoch; 2),
          candidate("ubuntu-26.04"; "ubuntu-final"; $ubuntu_epoch; 16),
          candidate("arch"; "arch-final"; $arch_epoch; 3)
        ]
      }
    '
}

test_remi_postdeployment_filter_scopes_fences_to_schema_authority() {
    local baseline final filter baseline_path
    baseline="$(remi_predeployment_inspection_fixture)"
    filter="$REPO_ROOT/deploy/remi-postdeployment-fencing.jq"
    baseline_path="$(mktemp "${TEST_RUN_ROOT}/remi-baseline.XXXXXX")"
    printf '%s\n' "$baseline" > "$baseline_path"

    final="$(remi_postdeployment_fencing_fixture 55 7 7 6)"
    jq -e --slurpfile baseline "$baseline_path" -f "$filter" \
        <<<"$final" >/dev/null ||
        fail "same-schema advancing fences were rejected"

    if jq '.candidates[0].latest_refresh.fencing_epoch = 6' <<<"$final" \
        | jq -e --slurpfile baseline "$baseline_path" -f "$filter" \
            >/dev/null; then
        fail "same-schema nonadvancing fence passed postdeployment validation"
    fi

    baseline="$(jq \
        '.schema_revision = 54
          | .candidates[0].latest_refresh.fencing_epoch = 12
          | .candidates[1].latest_refresh.fencing_epoch = 13
          | .candidates[2].latest_refresh.fencing_epoch = 11' \
        <<<"$baseline")"
    printf '%s\n' "$baseline" > "$baseline_path"
    final="$(remi_postdeployment_fencing_fixture 55 2 2 2)"
    jq -e --slurpfile baseline "$baseline_path" -f "$filter" \
        <<<"$final" >/dev/null ||
        fail "fresh post-transition schema fences were compared to retired authority"

    if jq '.candidates[0].latest_refresh.finished_at = 20' <<<"$final" \
        | jq -e --slurpfile baseline "$baseline_path" -f "$filter" \
            >/dev/null; then
        fail "pre-transition candidate completion passed postdeployment validation"
    fi
    if jq '.deployment.repository_refreshes = []' <<<"$final" \
        | jq -e --slurpfile baseline "$baseline_path" -f "$filter" \
            >/dev/null; then
        fail "candidate set without a typed refresh generation passed postdeployment validation"
    fi
    if jq '.deployment.repository_refreshes[0].finished_at = 20' <<<"$final" \
        | jq -e --slurpfile baseline "$baseline_path" -f "$filter" \
            >/dev/null; then
        fail "pre-transition refresh generation passed postdeployment validation"
    fi
    if jq '.deployment.repository_refreshes[0].successful_profiles -= ["fedora-44"]' \
        <<<"$final" \
        | jq -e --slurpfile baseline "$baseline_path" -f "$filter" \
            >/dev/null; then
        fail "candidate absent from the retained refresh result passed postdeployment validation"
    fi
    if jq '.candidates[0].latest_refresh.run_id = "different-run"' <<<"$final" \
        | jq -e --slurpfile baseline "$baseline_path" -f "$filter" \
            >/dev/null; then
        fail "candidate with mismatched refresh identity passed postdeployment validation"
    fi
    if jq '.candidates[0].latest_refresh.fencing_epoch = 0' <<<"$final" \
        | jq -e --slurpfile baseline "$baseline_path" -f "$filter" \
            >/dev/null; then
        fail "nonpositive fresh schema fence passed postdeployment validation"
    fi
}

test_candidate_deploy_materializes_policy_across_candidate_history() {
    local repo candidate_sha workflow_sha materialized
    repo="$(mktemp -d "${TEST_RUN_ROOT}/workflow-authority.XXXXXX")"
    git -C "$repo" init -q
    git -C "$repo" config user.name "Conary Release Fixture"
    git -C "$repo" config user.email "release-fixture@invalid.example"
    mkdir -p "$repo/deploy"
    printf '%s\n' candidate > "$repo/candidate-marker"
    git -C "$repo" add candidate-marker
    git -C "$repo" commit -q -m candidate
    candidate_sha="$(git -C "$repo" rev-parse HEAD)"

    cp "$REPO_ROOT/deploy/remi-postdeployment-fencing.jq" \
        "$repo/deploy/remi-postdeployment-fencing.jq"
    git -C "$repo" add deploy/remi-postdeployment-fencing.jq
    git -C "$repo" commit -q -m workflow-authority
    workflow_sha="$(git -C "$repo" rev-parse HEAD)"
    git -C "$repo" checkout -q --detach "$candidate_sha"
    [[ ! -e "$repo/deploy/remi-postdeployment-fencing.jq" ]] ||
        fail "older candidate fixture unexpectedly contains workflow fencing policy"

    materialized="$(mktemp "${TEST_RUN_ROOT}/workflow-fencing.XXXXXX")"
    git -C "$repo" show \
        "${workflow_sha}:deploy/remi-postdeployment-fencing.jq" \
        > "$materialized"
    cmp "$REPO_ROOT/deploy/remi-postdeployment-fencing.jq" "$materialized" ||
        fail "exact workflow SHA did not materialize its fencing policy"
}






















































































































































test_conversion_workflow_checker_fails_without_pyyaml() {
    local repo output status
    repo="$(create_release_policy_fixture)"

    set +e
    output="$(python3 -I -S "$REPO_ROOT/scripts/check-remi-conversion-workflow.py" "$repo" 2>&1)"
    status=$?
    set -e

    [[ "$status" -ne 0 ]] || fail "conversion workflow checker passed without PyYAML"
    assert_contains \
        "$output" \
        "python3-yaml is required for structural GitHub workflow policy" \
        "missing PyYAML failure must be explicit"
}













test_check_release_matrix_rejects_namespace_setup_after_workspace_tests() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/release-build.yml" \
        '        uses: ./.github/actions/setup-exact-ownership-tests' \
        '        run: echo "namespace setup delayed"'
    replace_fixture_text_once \
        "$repo/.github/workflows/release-build.yml" \
        '        run: cargo test --workspace --exclude conary-test --verbose' \
        $'        run: cargo test --workspace --exclude conary-test --verbose\n      - name: Delayed exact ownership setup\n        uses: ./.github/actions/setup-exact-ownership-tests'

    assert_check_release_matrix_fails "$repo" "release workspace validation exact ownership setup order"
}





















test_check_release_matrix_rejects_missing_artifact_row() {
    local repo
    repo="$(create_release_policy_fixture)"
    grep -v '^| `remi` |' "$repo/docs/operations/release-artifact-matrix.md" > "$repo/docs/operations/release-artifact-matrix.md.tmp"
    mv "$repo/docs/operations/release-artifact-matrix.md.tmp" "$repo/docs/operations/release-artifact-matrix.md"

    assert_check_release_matrix_fails "$repo" "release artifact matrix missing remi row"
}

test_check_release_matrix_rejects_unknown_deploy_route_pair() {
    local repo
    repo="$(create_release_policy_fixture)"
    python3 - "$repo/.github/workflows/deploy-and-verify.yml" <<'PY'
import sys
from pathlib import Path

path = Path(sys.argv[1])
text = path.read_text()
old = '{"product":"conary-test","bundle_name":"conary-test-bundle","deploy_mode":"none"}'
new = '{"product":"conary-test","bundle_name":"conary-test-bundle","deploy_mode":"remote_bundle"}'
if old not in text:
    raise SystemExit("fixture could not find serialized artifact route")
path.write_text(text.replace(old, new))
PY

    assert_check_release_matrix_fails "$repo" "exact serialized artifact deployment routes"
}





test_check_release_matrix_rejects_reversed_authority_checkout_order() {
    local target relative job mutation repo expected
    for target in \
        build-remi-candidate.yml:build-remi-candidate \
        deploy-remi-candidate.yml:deploy-remi-candidate \
        release-artifact-proof.yml:native-package-lifecycle \
        remi-r2-durability.yml:inventory \
        deploy-and-verify.yml:deploy-conary \
        deploy-and-verify.yml:deploy-remi; do
        relative="${target%%:*}"
        job="${target#*:}"
        for mutation in authority-first authority-after-action; do
            repo="$(create_release_policy_fixture)"
            python3 - "$repo/.github/workflows/$relative" "$job" "$mutation" <<'PY'
import sys
from pathlib import Path

path = Path(sys.argv[1])
text = path.read_text()
job_start = text.index(f"  {sys.argv[2]}:\n")
steps_start = text.index("    steps:\n", job_start) + len("    steps:\n")
root_end = text.index("      - ", steps_start + len("      - "))
authority_end = text.index("      - ", root_end + len("      - "))
root = text[steps_start:root_end]
authority = text[root_end:authority_end]
assert "path: workflow-authority" in authority
assert "path: workflow-authority" not in root
if sys.argv[3] == "authority-first":
    # Reproduce the broken fresh-runner sequence, including its ineffective
    # clean:false workaround. Preserve formatting for unrelated policy checks.
    text = text[:steps_start] + authority + root.rstrip() + "\n          clean: false\n" + text[authority_end:]
else:
    action_start = text.index("uses: ./workflow-authority/", authority_end)
    action_end = text.index("      - ", action_start)
    text = text[:root_end] + text[authority_end:action_end] + authority + text[action_end:]
path.write_text(text)
PY
            expected="historical checkout local-action authority"
            if [[ "$mutation" == authority-after-action ]]; then
                case "$job" in
                    deploy-conary)
                        expected="check out the exact workflow repository before using the local SSH action"
                        ;;
                    deploy-remi)
                        expected="load the local SSH action from the workflow revision after checking out the release tag"
                        ;;
                esac
            fi
            assert_check_release_matrix_fails \
                "$repo" \
                "$expected"
        done
    done
}


main() {
    local -a tests=(
        test_bootstrap_installer_contract
        test_native_oracle_transport_contract
        test_native_oracle_lane_contract
        test_resolution_survey_transport_contract
        test_native_oracle_assembly_contract
        test_native_oracle_lane_selection_contract
        test_native_oracle_producer_verification_contract
        test_resolve_tag_suite_canonical
        test_latest_version_from_list_uses_canonical_tags
        test_field_conary_test_deploy_mode
        test_field_conaryd_build_only_mode
        test_field_conary_bundle_name
        test_metadata_json_is_versioned_and_typed
        test_unknown_tag_prefix_fails
        test_historical_tag_prefixes_are_rejected
        test_latest_version_from_git_in_fixture
        test_workspace_version_in_fixture
        test_max_owned_version_in_fixture
        test_assert_owned_version_accepts_matching_manifests
        test_assert_owned_version_rejects_mismatched_manifest
        test_assert_owned_version_rejects_independent_publish_policy
        test_assert_owned_version_rejects_publishable_workspace_root
        test_release_dry_run_uses_one_suite_history
        test_release_dry_run_prefers_highest_numeric_suite_history
        test_release_dry_run_cross_app_feature_selects_one_minor
        test_release_dry_run_ignores_product_prefixed_history
        test_release_dry_run_accepts_explicit_target
        test_release_prepare_only_updates_one_workspace_authority
        test_release_rejects_product_scoped_target
        test_release_conary_regenerates_and_stages_man_page
        test_release_conary_rejects_stale_generated_man_page















        test_check_release_matrix_rejects_unpinned_ccs_toolchain



        test_check_release_matrix_rejects_moving_release_preparation_toolchain








        test_remi_predeployment_filter_accepts_typed_incomplete_baseline
        test_remi_postdeployment_filter_scopes_fences_to_schema_authority
        test_candidate_deploy_materializes_policy_across_candidate_history





















































































































































        test_conversion_workflow_checker_fails_without_pyyaml












        test_check_release_matrix_rejects_namespace_setup_after_workspace_tests




















        test_check_release_matrix_rejects_missing_artifact_row
        test_check_release_matrix_rejects_unknown_deploy_route_pair




        test_check_release_matrix_rejects_reversed_authority_checkout_order

    )

    local test_name
    for test_name in "${tests[@]}"; do
        "$test_name"
        printf 'ok - %s\n' "$test_name"
    done

    run_release_policy_mutation_cases
}

main "$@"
