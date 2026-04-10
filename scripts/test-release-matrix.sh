#!/usr/bin/env bash
# scripts/test-release-matrix.sh
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MATRIX="${REPO_ROOT}/scripts/release-matrix.sh"

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
    find "$REPO_ROOT" -maxdepth 1 -type d -name '.tmp-release-matrix-test.*' -exec rm -rf {} +
}

trap cleanup EXIT

run_matrix() {
    bash "$MATRIX" "$@"
}

write_cargo_manifest() {
    local file="$1"
    local name="$2"
    local version="$3"

    cat > "$file" <<EOF
[package]
name = "$name"
version = "$version"
edition = "2024"
authors = ["Conary Contributors"]
license = "MIT OR Apache-2.0"
EOF
}

create_release_fixture() {
    local repo

    repo="$(mktemp -d "${REPO_ROOT}/.tmp-release-matrix-test.XXXXXX")"

    mkdir -p \
        "$repo/scripts" \
        "$repo/apps/conary" \
        "$repo/apps/remi" \
        "$repo/apps/conaryd" \
        "$repo/apps/conary-test" \
        "$repo/crates/conary-core" \
        "$repo/crates/conary-bootstrap" \
        "$repo/crates/conary-mcp" \
        "$repo/packaging/rpm" \
        "$repo/packaging/arch" \
        "$repo/packaging/deb/debian" \
        "$repo/packaging/ccs"

    cp "$REPO_ROOT/scripts/release.sh" "$repo/scripts/release.sh"
    cp "$REPO_ROOT/scripts/release-matrix.sh" "$repo/scripts/release-matrix.sh"
    chmod +x "$repo/scripts/release.sh" "$repo/scripts/release-matrix.sh"

    write_cargo_manifest "$repo/apps/conary/Cargo.toml" "conary" "0.7.0"
    write_cargo_manifest "$repo/crates/conary-core/Cargo.toml" "conary-core" "0.7.0"
    write_cargo_manifest "$repo/crates/conary-bootstrap/Cargo.toml" "conary-bootstrap" "0.7.0"
    write_cargo_manifest "$repo/apps/remi/Cargo.toml" "remi" "0.5.0"
    write_cargo_manifest "$repo/apps/conaryd/Cargo.toml" "conaryd" "0.5.0"
    write_cargo_manifest "$repo/apps/conary-test/Cargo.toml" "conary-test" "0.7.0"
    write_cargo_manifest "$repo/crates/conary-mcp/Cargo.toml" "conary-mcp" "0.7.0"

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

    (
        cd "$repo"
        ./scripts/release.sh "$product" --dry-run
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

test_resolve_tag_remi_canonical() {
    local output
    output="$(run_matrix resolve-tag remi-v0.5.0 --format shell)"
    assert_contains "$output" "product=remi" "canonical remi tag should resolve to remi"
}

test_resolve_tag_remi_legacy() {
    local output
    output="$(run_matrix resolve-tag server-v0.5.0 --format shell)"
    assert_contains "$output" "product=remi" "legacy server tag should resolve to remi"
}

test_resolve_tag_conary_test_legacy() {
    local output
    output="$(run_matrix resolve-tag test-v0.3.0 --format shell)"
    assert_contains "$output" "product=conary-test" "legacy test tag should resolve to conary-test"
}

test_latest_version_from_list_mixed_prefixes() {
    local output
    output="$(run_matrix latest-version-from-list remi server-v0.5.0 remi-v0.4.0 remi-v0.6.0)"
    assert_eq "0.6.0" "$output" "mixed-prefix comparison should choose the highest numeric version"
}

test_field_conary_test_deploy_mode() {
    local output
    output="$(run_matrix field conary-test deploy_mode)"
    assert_eq "none" "$output" "conary-test should not deploy automatically"
}

test_field_conary_bundle_name() {
    local output
    output="$(run_matrix field conary bundle_name)"
    assert_eq "release-bundle" "$output" "conary should use the release bundle name"
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

    assert_contains "$output" "unknown tag prefix: foo-v1.0.0" "unknown tag prefix should fail clearly"
}

test_latest_version_from_git_in_fixture() {
    local repo
    local output

    repo="$(create_release_fixture)"
    tag_head "$repo" "server-v1.0.0"
    commit_empty "$repo" "chore: canonical release point"
    tag_head "$repo" "remi-v2.0.0"

    output="$(run_repo_matrix "$repo" latest-version-from-git remi)"
    assert_eq "2.0.0" "$output" "fixture repo should prefer the highest numeric remi version"
}

test_max_owned_version_in_fixture() {
    local repo
    local output

    repo="$(create_release_fixture)"
    output="$(run_repo_matrix "$repo" max-owned-version conary-test)"
    assert_eq "0.7.0" "$output" "fixture repo should report the highest owned conary-test version"
}

test_release_dry_run_remi_legacy_history() {
    local repo
    local output

    repo="$(create_release_fixture)"
    tag_head "$repo" "server-v0.5.0"
    commit_change "$repo" "apps/remi/changes.txt" "fix(remi): tighten deploy flow"

    output="$(run_release_dry_run "$repo" remi)"
    assert_contains "$output" "Tag: remi-v0.5.1" "remi should emit canonical tags after legacy history"
}

test_release_dry_run_remi_prefers_highest_numeric_history() {
    local repo
    local output

    repo="$(create_release_fixture)"
    tag_head "$repo" "server-v1.0.0"
    commit_empty "$repo" "chore: canonical release point"
    tag_head "$repo" "remi-v2.0.0"
    commit_change "$repo" "apps/remi/changes.txt" "fix(remi): tighten deploy flow"

    output="$(run_release_dry_run "$repo" remi)"
    assert_contains "$output" "Current: remi-v2.0.0" "mixed remi history should choose the highest numeric baseline"
}

test_release_dry_run_conaryd_canonical_history() {
    local repo
    local output

    repo="$(create_release_fixture)"
    tag_head "$repo" "conaryd-v0.5.0"
    commit_change "$repo" "apps/conaryd/changes.txt" "fix(conaryd): tighten daemon health checks"

    output="$(run_release_dry_run "$repo" conaryd)"
    assert_contains "$output" "Tag: conaryd-v0.5.1" "conaryd should continue on its canonical release line"
}

test_release_dry_run_conary_test_uses_owned_manifest_baseline() {
    local repo
    local output

    repo="$(create_release_fixture)"
    tag_head "$repo" "test-v0.3.0"
    commit_change "$repo" "apps/conary-test/changes.txt" "fix(test): update bundle layout"

    output="$(run_release_dry_run "$repo" conary-test)"
    assert_contains "$output" "Current: conary-test-v0.7.0" "conary-test should respect owned manifest versions"
    assert_contains "$output" "Tag: conary-test-v0.7.1" "conary-test should bump from the owned-manifest baseline"
}

main() {
    local -a tests=(
        test_resolve_tag_remi_canonical
        test_resolve_tag_remi_legacy
        test_resolve_tag_conary_test_legacy
        test_latest_version_from_list_mixed_prefixes
        test_field_conary_test_deploy_mode
        test_field_conary_bundle_name
        test_unknown_tag_prefix_fails
        test_latest_version_from_git_in_fixture
        test_max_owned_version_in_fixture
        test_release_dry_run_remi_legacy_history
        test_release_dry_run_remi_prefers_highest_numeric_history
        test_release_dry_run_conaryd_canonical_history
        test_release_dry_run_conary_test_uses_owned_manifest_baseline
    )

    local test_name
    for test_name in "${tests[@]}"; do
        "$test_name"
        printf 'ok - %s\n' "$test_name"
    done
}

main "$@"
