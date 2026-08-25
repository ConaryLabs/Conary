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

    repo="$(mktemp -d "${TEST_RUN_ROOT}/fixture.XXXXXX")"
    mkdir -p \
        "$repo/scripts" \
        "$repo/.github/actions/setup-exact-ownership-tests" \
        "$repo/.github/actions/setup-rust-workspace" \
        "$repo/.github/ISSUE_TEMPLATE" \
        "$repo/.github/workflows" \
        "$repo/docs/operations" \
        "$repo/site/src/lib" \
        "$repo/site/static" \
        "$repo/site/src/routes/install" \
        "$repo/packaging/rpm" \
        "$repo/packaging/deb" \
        "$repo/packaging/arch" \
        "$repo/packaging/ccs"
    cp "$REPO_ROOT/Cargo.toml" "$repo/Cargo.toml"
    cp "$REPO_ROOT/scripts/release-matrix.sh" "$repo/scripts/release-matrix.sh"
    cp "$REPO_ROOT/.github/workflows/release-build.yml" "$repo/.github/workflows/release-build.yml"
    cp "$REPO_ROOT/.github/workflows/deploy-and-verify.yml" "$repo/.github/workflows/deploy-and-verify.yml"
    cp "$REPO_ROOT/.github/workflows/release-artifact-proof.yml" "$repo/.github/workflows/release-artifact-proof.yml"
    cp "$REPO_ROOT/.github/workflows/merge-validation.yml" "$repo/.github/workflows/merge-validation.yml"
    cp "$REPO_ROOT/.github/workflows/pr-gate.yml" "$repo/.github/workflows/pr-gate.yml"
    cp "$REPO_ROOT/.github/actions/setup-exact-ownership-tests/action.yml" \
        "$repo/.github/actions/setup-exact-ownership-tests/action.yml"
    cp "$REPO_ROOT/.github/actions/setup-rust-workspace/action.yml" \
        "$repo/.github/actions/setup-rust-workspace/action.yml"
    cp "$REPO_ROOT/.github/ISSUE_TEMPLATE/pre_alpha_feedback.md" "$repo/.github/ISSUE_TEMPLATE/pre_alpha_feedback.md"
    cp "$REPO_ROOT/docs/operations/release-artifact-matrix.md" "$repo/docs/operations/release-artifact-matrix.md"
    cp "$REPO_ROOT/site/src/lib/preview-release.ts" "$repo/site/src/lib/preview-release.ts"
    cp "$REPO_ROOT/site/src/routes/install/+page.svelte" "$repo/site/src/routes/install/+page.svelte"
    cp "$REPO_ROOT/site/static/install-conary-preview.sh" "$repo/site/static/install-conary-preview.sh"
    cp "$REPO_ROOT/scripts/bootstrap-manifest.sh" "$repo/scripts/bootstrap-manifest.sh"
    cp "$REPO_ROOT/scripts/test-install-conary-preview.sh" "$repo/scripts/test-install-conary-preview.sh"
    cp "$REPO_ROOT/packaging/rpm/Containerfile.build" "$repo/packaging/rpm/Containerfile.build"
    cp "$REPO_ROOT/packaging/rpm/conary.spec" "$repo/packaging/rpm/conary.spec"
    cp "$REPO_ROOT/packaging/deb/Containerfile.build" "$repo/packaging/deb/Containerfile.build"
    cp "$REPO_ROOT/packaging/arch/Containerfile.build" "$repo/packaging/arch/Containerfile.build"
    cp "$REPO_ROOT/packaging/arch/PKGBUILD" "$repo/packaging/arch/PKGBUILD"
    cp "$REPO_ROOT/packaging/rpm/build.sh" "$repo/packaging/rpm/build.sh"
    cp "$REPO_ROOT/packaging/deb/build.sh" "$repo/packaging/deb/build.sh"
    cp "$REPO_ROOT/packaging/arch/build.sh" "$repo/packaging/arch/build.sh"
    cp "$REPO_ROOT/packaging/ccs/build.sh" "$repo/packaging/ccs/build.sh"
    chmod +x "$repo/scripts/release-matrix.sh"
    printf '%s\n' "$repo"
}

test_bootstrap_installer_contract() {
    bash "$REPO_ROOT/scripts/test-install-conary-preview.sh"
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

test_check_release_matrix_rejects_conaryd_deploy_jobs_when_paused() {
    local repo
    repo="$(create_release_policy_fixture)"
    cat >> "$repo/.github/workflows/deploy-and-verify.yml" <<'YAML'

  deploy-conaryd:
    name: deploy-conaryd
    runs-on: ubuntu-latest
    steps:
      - run: echo deploy
YAML

    assert_check_release_matrix_fails "$repo" "deployment job for a build-only suite artifact"
}

test_check_release_matrix_rejects_beta_maturity_drift() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/ISSUE_TEMPLATE/pre_alpha_feedback.md" \
        'name: Pre-Alpha Tester Feedback' \
        'name: Beta Feedback'

    assert_check_release_matrix_fails \
        "$repo" \
        "public maturity surfaces must identify this project as pre-alpha, not beta"
}

test_check_release_matrix_rejects_conary_test_deploy_jobs() {
    local repo
    repo="$(create_release_policy_fixture)"
    cat >> "$repo/.github/workflows/deploy-and-verify.yml" <<'YAML'

  verify-conary-test:
    name: verify-conary-test
    runs-on: ubuntu-latest
    steps:
      - run: echo verify
YAML

    assert_check_release_matrix_fails "$repo" "deployment job for a build-only suite artifact"
}

test_check_release_matrix_rejects_unpinned_rpm_builder_image() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/release-build.yml" \
        'image: registry.fedoraproject.org/fedora@sha256:765b2260aa4b4eff379b9a6f983f15fcf41a6f9dda9b272b790e23e92fcbaafb' \
        'image: registry.fedoraproject.org/fedora:44'

    assert_check_release_matrix_fails "$repo" "release-build RPM builder must use the pinned Fedora 44 image"
}

test_check_release_matrix_rejects_unpinned_deb_builder_image() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/release-build.yml" \
        'image: docker.io/library/ubuntu@sha256:3131b4cc82a783df6c9df078f86e01819a13594b865c2cad47bd1bca2b7063bb' \
        'image: docker.io/library/ubuntu:26.04'

    assert_check_release_matrix_fails "$repo" "release-build DEB builder must use the pinned Ubuntu 26.04 image"
}

test_check_release_matrix_rejects_unpinned_arch_builder_image() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/release-build.yml" \
        'image: docker.io/library/archlinux@sha256:fe6972d4dc1f660c0c10f4c41b2de8986bab89e7e2955378f8beadb8ebcd7433' \
        'image: docker.io/library/archlinux:latest'

    assert_check_release_matrix_fails "$repo" "release-build Arch builder must use the pinned Arch image"
}

test_check_release_matrix_rejects_arch_bootstrap_partial_upgrade() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/release-build.yml" \
        'pacman -Syu --noconfirm curl openssl sudo ca-certificates' \
        'pacman -Sy --noconfirm curl openssl sudo ca-certificates'

    assert_check_release_matrix_fails "$repo" "Arch bootstrap rehearsal must avoid an unsupported partial upgrade"
}

test_check_release_matrix_rejects_rpm_debug_subpackage_generation() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/packaging/rpm/conary.spec" \
        '%global debug_package %{nil}' \
        '%global debug_package 1'

    assert_check_release_matrix_fails "$repo" "RPM spec must explain and disable debug subpackage generation"
}

test_check_release_matrix_rejects_automatic_rpm_rust_flags() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/packaging/rpm/conary.spec" \
        '%undefine _auto_set_build_flags' \
        '%global _auto_set_build_flags 1'

    assert_check_release_matrix_fails \
        "$repo" \
        "RPM spec must preserve non-debug Fedora Rust flags without overriding the workspace release profile"
}

test_check_release_matrix_rejects_rpm_debug_rust_flags() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/packaging/rpm/conary.spec" \
        'echo "Conary effective RUSTFLAGS: $RUSTFLAGS"' \
        $'RUSTFLAGS="$RUSTFLAGS -Cdebuginfo=2"\necho "Conary effective RUSTFLAGS: $RUSTFLAGS"'

    assert_check_release_matrix_fails "$repo" "RPM spec debug-oriented Rust flag override"
}

test_check_release_matrix_rejects_rpm_macro_expansion_in_comment() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/packaging/rpm/conary.spec" \
        '# the manual distro macro below populates native dependency toolchain flags.' \
        '# %set_build_flags populates native dependency toolchain flags.'

    assert_check_release_matrix_fails \
        "$repo" \
        "RPM spec manual build-flag macro invocation expected 1 occurrences"
}

test_check_release_matrix_rejects_arch_debug_split_package_generation() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/packaging/arch/PKGBUILD" \
        'options=(!debug !lto)' \
        'options=(debug !lto)'

    assert_check_release_matrix_fails "$repo" "Arch package must explicitly disable debug split-package generation"
}

test_check_release_matrix_rejects_hidden_native_debug_outputs() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/release-build.yml" \
        '          path: packaging/rpm/output/*.rpm' \
        $'          path: |\n            packaging/rpm/output/*.rpm\n            !packaging/rpm/output/*debug*'

    assert_check_release_matrix_fails "$repo" "native debug artifact filtering"
}

test_check_release_matrix_rejects_unverified_rustup_init() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/release-build.yml" \
        '20a06e644b0d9bd2fbdbfd52d42540bdde820ea7df86e92e533c073da0cdd43c  /tmp/rustup-init' \
        '0000000000000000000000000000000000000000000000000000000000000000  /tmp/rustup-init'

    assert_check_release_matrix_fails "$repo" "release-build RPM builder checksum-pinned rustup-init flow"
}

test_check_release_matrix_rejects_workspace_rust_version_drift() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/Cargo.toml" \
        'rust-version = "1.98.0"' \
        'rust-version = "1.99.0"'

    assert_check_release_matrix_fails \
        "$repo" \
        "shared workspace setup exact workspace Rust default"
}

test_check_release_matrix_rejects_unpinned_ccs_toolchain() {
    local repo
    repo="$(create_release_policy_fixture)"
    sed -i \
        '/^  build-ccs:/,/^  build-rpm:/{s/toolchain: 1\.98\.0/toolchain: stable/;}' \
        "$repo/.github/workflows/release-build.yml"

    assert_check_release_matrix_fails "$repo" "release-build CCS builder pinned Rust toolchain"
}

test_check_release_matrix_rejects_unpinned_arch_toolchain() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/release-build.yml" \
        'rustup default 1.98.0' \
        'rustup default stable'

    assert_check_release_matrix_fails "$repo" "release-build Arch builder pinned Rust toolchain"
}

test_check_release_matrix_rejects_leaf_manifest_native_versions() {
    local format label repo

    for format in rpm deb arch; do
        case "$format" in
            rpm) label="RPM" ;;
            deb) label="DEB" ;;
            arch) label="Arch" ;;
        esac
        repo="$(create_release_policy_fixture)"
        replace_fixture_text_once \
            "$repo/packaging/$format/build.sh" \
            'VERSION="$(bash "$REPO_ROOT/scripts/release-matrix.sh" workspace-version)"' \
            'VERSION=$(grep '\''^version'\'' "$REPO_ROOT/apps/conary/Cargo.toml" | head -1)'

        assert_check_release_matrix_fails \
            "$repo" \
            "$label build must use and validate the root workspace version authority"
    done
}

test_check_release_matrix_rejects_product_scoped_ccs_version() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/packaging/ccs/build.sh" \
        'assert-owned-version suite "$VERSION"' \
        'assert-owned-version conary "$VERSION"'

    assert_check_release_matrix_fails \
        "$repo" \
        "CCS build must validate the root workspace version authority"
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

test_check_release_matrix_rejects_untyped_workspace_toolchain_setup() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/actions/setup-rust-workspace/action.yml" \
        '        toolchain: ${{ inputs.toolchain }}' \
        '        toolchain: stable'

    assert_check_release_matrix_fails \
        "$repo" \
        "shared workspace setup exact workspace Rust default and typed toolchain input"
}

test_check_release_matrix_rejects_missing_live_version_assertion() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/release-build.yml" \
        'bash scripts/release-matrix.sh assert-owned-version "$release" "$version"' \
        'echo "owned version assertion removed"'

    assert_check_release_matrix_fails "$repo" "live suite tag must match the workspace-owned version"
}

test_check_release_matrix_rejects_lightweight_live_tag_guard() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/release-build.yml" \
        '[[ "$(git cat-file -t "refs/tags/${tag_name}")" == "tag" ]] || {' \
        'true || {'

    assert_check_release_matrix_fails "$repo" "live suite build must require an annotated tag at the exact checkout"
}

test_check_release_matrix_rejects_unmerged_live_tag() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/release-build.yml" \
        '              refs/heads/main:refs/remotes/origin/main' \
        '              main'

    assert_check_release_matrix_fails \
        "$repo" \
        "live suite tag must already be reachable from a freshly fetched main"
}

test_check_release_matrix_rejects_unbound_proof_metadata_version() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/release-artifact-proof.yml" \
        '          [[ "$version" == "$resolved_version" ]] || {' \
        '          true || {'

    assert_check_release_matrix_fails \
        "$repo" \
        "published artifact proof must bind metadata to the annotated tag version and suite authority"
}

test_check_release_matrix_rejects_mutable_artifact_proof() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/release-artifact-proof.yml" \
        '             "$(jq -r '\''.immutable'\'' <<< "$release_state")" == "true" ]] || {' \
        '             "$(jq -r '\''.immutable'\'' <<< "$release_state")" == "false" ]] || {'

    assert_check_release_matrix_fails \
        "$repo" \
        "published artifact proof must reject a draft, mutable, or mismatched GitHub release"
}

test_check_release_matrix_rejects_moving_artifact_proof_toolchain() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/release-artifact-proof.yml" \
        '          toolchain: 1.98.0' \
        '          toolchain: stable'

    assert_check_release_matrix_fails \
        "$repo" \
        "published artifact proof exact Rust toolchain"
}

test_check_release_matrix_rejects_rehearsal_artifact_promotion() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/deploy-and-verify.yml" \
        'elif [[ "$MANUAL_DRY_RUN" == "false" && "$dry_run" == "true" ]]; then' \
        'elif false; then'

    assert_check_release_matrix_fails "$repo" "manual deployment must not promote rehearsal artifacts"
}

test_check_release_matrix_rejects_unverified_remi_suite_bundle() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/deploy-and-verify.yml" \
        $'            sha256sum -c SHA256SUMS\n          )\n          bundle="${bundle_dir}/remi-${VERSION}-linux-x64.tar.gz"\n          [[ -s "$bundle" && ! -L "$bundle" ]] || { echo "remi bundle missing, empty, or symlinked" >&2; exit 1; }\n          [[ -n "${REMI_SSH_KEY:-}" ]]' \
        $'            echo "suite checksum verification removed"\n          )\n          bundle="${bundle_dir}/remi-${VERSION}-linux-x64.tar.gz"\n          [[ -s "$bundle" && ! -L "$bundle" ]] || { echo "remi bundle missing, empty, or symlinked" >&2; exit 1; }\n          [[ -n "${REMI_SSH_KEY:-}" ]]'

    assert_check_release_matrix_fails "$repo" "Remi deployment must verify the complete suite checksums before staging its bundle"
}

test_check_release_matrix_rejects_merge_validation_production_probes() {
    local repo

    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/merge-validation.yml" \
        '      - name: Explain paused remote validation' \
        $'      - name: Probe mutable production Remi through the current health script\n        run: ./scripts/remi-health.sh --smoke\n      - name: Explain paused remote validation'
    assert_check_release_matrix_fails \
        "$repo" \
        "mutable production Remi probe in source merge validation"

    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/merge-validation.yml" \
        '      - name: Explain paused remote validation' \
        $'      - name: Probe mutable production Remi directly\n        run: curl -fsS https://remi.conary.io/health\n      - name: Explain paused remote validation'
    assert_check_release_matrix_fails \
        "$repo" \
        "mutable production Remi probe in source merge validation"
}

test_check_release_matrix_rejects_missing_post_deploy_remi_readiness() {
    local repo

    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/deploy-and-verify.yml" \
        '          body=$(curl -fsS --max-time 30 https://remi.conary.io/health/ready)' \
        '          echo "structured readiness proof removed"'

    assert_check_release_matrix_fails \
        "$repo" \
        "exact post-deploy Remi liveness and structured readiness proof"
}

test_check_release_matrix_requires_shared_namespace_setup_in_every_workspace_lane() {
    local repo
    local workflow

    for workflow in pr-gate.yml merge-validation.yml release-build.yml; do
        repo="$(create_release_policy_fixture)"
        replace_fixture_text_once \
            "$repo/.github/workflows/$workflow" \
            '        uses: ./.github/actions/setup-exact-ownership-tests' \
            '        run: echo "exact ownership setup removed"'

        assert_check_release_matrix_fails "$repo" "shared exact ownership setup"
    done
}

test_check_release_matrix_rejects_unproven_namespace_action() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/actions/setup-exact-ownership-tests/action.yml" \
        '        unshare --user --map-root-user --mount --propagation private /bin/true' \
        '        /bin/true'

    assert_check_release_matrix_fails "$repo" "exact ownership namespace proof"
}

test_check_release_matrix_requires_hosted_alpm_parity_producer() {
    local repo
    local workflow

    for workflow in pr-gate.yml merge-validation.yml; do
        repo="$(create_release_policy_fixture)"
        replace_fixture_text_once \
            "$repo/.github/workflows/$workflow" \
            '        run: cargo test -p conary-core --features native-alpm-oracle repository::catalog::parity::alpm --verbose' \
            '        run: echo "ALPM producer proof removed"'

        assert_check_release_matrix_fails "$repo" "hosted"
    done
}

test_check_release_matrix_requires_resilient_alpm_archive_downloads() {
    local repo
    local workflow

    for workflow in pr-gate.yml merge-validation.yml; do
        repo="$(create_release_policy_fixture)"
        replace_fixture_text_once \
            "$repo/.github/workflows/$workflow" \
            "          sed -i '/^\[options\]/a DisableDownloadTimeout' /etc/pacman.conf" \
            '          true'

        assert_check_release_matrix_fails "$repo" "hosted"
    done
}

test_check_release_matrix_requires_hosted_rpm_parity_producer() {
    local repo
    local workflow

    for workflow in pr-gate.yml merge-validation.yml; do
        repo="$(create_release_policy_fixture)"
        replace_fixture_text_once \
            "$repo/.github/workflows/$workflow" \
            '        run: cargo test -p conary-core --features native-rpm-oracle repository::catalog::parity::rpm --verbose' \
            '        run: echo "RPM producer proof removed"'

        assert_check_release_matrix_fails "$repo" "hosted"
    done
}

test_check_release_matrix_requires_hosted_debian_parity_producer() {
    local repo
    local workflow

    for workflow in pr-gate.yml merge-validation.yml; do
        repo="$(create_release_policy_fixture)"
        replace_fixture_text_once \
            "$repo/.github/workflows/$workflow" \
            '        run: cargo test -p conary-core --features native-debian-oracle repository::catalog::parity::debian --verbose' \
            '        run: echo "Debian producer proof removed"'

        assert_check_release_matrix_fails "$repo" "hosted"
    done
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

test_check_release_matrix_rejects_non_failing_artifact_upload() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/release-build.yml" \
        'if-no-files-found: error' \
        'if-no-files-found: warn'

    assert_check_release_matrix_fails "$repo" "fail-closed release artifact uploads"
}

test_check_release_matrix_rejects_missing_exact_ccs_asset_assertion() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/release-build.yml" \
        '"release-packages/conary-${VERSION}.ccs"' \
        '"release-packages/conary-${VERSION}.ccs.unchecked"'

    assert_check_release_matrix_fails "$repo" "exact version-matching CCS release asset assertion"
}

test_check_release_matrix_rejects_missing_tester_authority_boundary() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/release-build.yml" \
        'Publication and released-package proof do not make' \
        'Publication proves all tester readiness and makes'

    assert_check_release_matrix_fails "$repo" "release notes must keep publication separate from tester authority"
}

test_check_release_matrix_rejects_missing_signer_trust_match() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/release-build.yml" \
        'release signing key does not match an embedded trusted update key' \
        'release signing key check removed'

    assert_check_release_matrix_fails "$repo" "live signing key must match an embedded trusted update key"
}

test_check_release_matrix_rejects_unsigned_embedded_ccs_authority() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/release-build.yml" \
        'target/release/examples/sign_hash --write-ccs-authority "$authority_dir"' \
        'echo "embedded CCS authority removed"'

    assert_check_release_matrix_fails "$repo" "CCS build must derive embedded authority from the configured release seed"
}

test_check_release_matrix_rejects_unverified_embedded_ccs_authority() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/release-build.yml" \
        '          target/release/conary ccs verify \' \
        '          echo "embedded CCS verification removed" \'

    assert_check_release_matrix_fails "$repo" "CCS build must verify its embedded release authority"
}

test_check_release_matrix_rejects_unstable_ccs_release_name() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/packaging/ccs/build.sh" \
        'mv -- "$BUILT_CCS" "$EXPECTED_CCS"' \
        'echo "stable CCS release name removed"'

    assert_check_release_matrix_fails "$repo" "CCS wrapper must normalize one exact package-release name to the stable self-update asset"
}

test_check_release_matrix_rejects_ambiguous_ccs_target_directory() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/packaging/ccs/build.sh" \
        '    --target-dir "$TARGET_DIR"' \
        '    --target-dir target'

    assert_check_release_matrix_fails "$repo" "CCS wrapper must use one explicit Cargo target directory"
}

test_check_release_matrix_rejects_stale_native_output_policy() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/packaging/rpm/build.sh" \
        'find "$OUTPUT" -maxdepth 1 -name '\''*.rpm'\'' -delete' \
        'echo "stale RPM output retained"'

    assert_check_release_matrix_fails "$repo" "RPM build must clean stale package output"
}

test_check_release_matrix_rejects_rpm_extra_output_blind_spot() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/packaging/rpm/build.sh" \
        'rpm_outputs=("$OUTPUT"/*.rpm)' \
        'rpm_outputs=("$OUTPUT/$NAME-$VERSION-"*.x86_64.rpm)'

    assert_check_release_matrix_fails "$repo" "RPM build must reject every extra package output"
}

test_check_release_matrix_rejects_arch_extra_output_blind_spot() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/packaging/arch/build.sh" \
        'package_outputs=("$OUTPUT"/*.pkg.tar.zst)' \
        'package_outputs=("$EXPECTED_PACKAGE")'

    assert_check_release_matrix_fails "$repo" "Arch build must reject every extra package output"
}

test_check_release_matrix_rejects_direct_release_publication() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/release-build.yml" \
        'gh release create "$TAG_NAME" \' \
        'gh release create "$TAG_NAME" suite-packages/* \'

    assert_check_release_matrix_fails "$repo" "direct published release creation with attached assets"
}

test_check_release_matrix_rejects_moved_tag_publication() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/release-build.yml" \
        '          verify_release_tag "before draft mutation"' \
        '          echo "tag revalidation removed"'

    assert_check_release_matrix_fails \
        "$repo" \
        "suite tag validation before draft mutation"
}

test_check_release_matrix_rejects_mutable_publication_result() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/release-build.yml" \
        '.tag_name == $tag and .draft == false and .immutable == true' \
        '.tag_name == $tag and .draft == false'

    assert_check_release_matrix_fails \
        "$repo" \
        "suite publisher must prove exact immutable state after publication"
}

test_check_release_matrix_rejects_late_suite_release_notes() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/release-build.yml" \
        'gh release edit "$TAG_NAME" --notes-file "$release_notes"' \
        'gh release view "$TAG_NAME" --json body'

    assert_check_release_matrix_fails "$repo" "immutable-compatible single suite publication sequence"
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

test_check_release_matrix_rejects_non_tag_static_site_checkout() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/deploy-and-verify.yml" \
        'ref: ${{ needs.resolve.outputs.tag_name }}' \
        'ref: main'

    assert_check_release_matrix_fails "$repo" "static-site checkout must use the serialized release tag"
}

test_check_release_matrix_rejects_single_static_site_deploy() {
    local repo
    repo="$(create_release_policy_fixture)"
    replace_fixture_text_once \
        "$repo/.github/workflows/deploy-and-verify.yml" \
        'bash deploy/deploy-sites.sh both' \
        'bash deploy/deploy-sites.sh site'

    assert_check_release_matrix_fails "$repo" "both-site deployment from the release tag"
}

main() {
    local -a tests=(
        test_bootstrap_installer_contract
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
        test_check_release_matrix_rejects_beta_maturity_drift
        test_check_release_matrix_rejects_conaryd_deploy_jobs_when_paused
        test_check_release_matrix_rejects_conary_test_deploy_jobs
        test_check_release_matrix_rejects_unpinned_rpm_builder_image
        test_check_release_matrix_rejects_unpinned_deb_builder_image
        test_check_release_matrix_rejects_unpinned_arch_builder_image
        test_check_release_matrix_rejects_arch_bootstrap_partial_upgrade
        test_check_release_matrix_rejects_rpm_debug_subpackage_generation
        test_check_release_matrix_rejects_automatic_rpm_rust_flags
        test_check_release_matrix_rejects_rpm_debug_rust_flags
        test_check_release_matrix_rejects_rpm_macro_expansion_in_comment
        test_check_release_matrix_rejects_arch_debug_split_package_generation
        test_check_release_matrix_rejects_hidden_native_debug_outputs
        test_check_release_matrix_rejects_unverified_rustup_init
        test_check_release_matrix_rejects_workspace_rust_version_drift
        test_check_release_matrix_rejects_unpinned_ccs_toolchain
        test_check_release_matrix_rejects_unpinned_arch_toolchain
        test_check_release_matrix_rejects_leaf_manifest_native_versions
        test_check_release_matrix_rejects_product_scoped_ccs_version
        test_check_release_matrix_rejects_moving_release_preparation_toolchain
        test_check_release_matrix_rejects_untyped_workspace_toolchain_setup
        test_check_release_matrix_rejects_missing_live_version_assertion
        test_check_release_matrix_rejects_lightweight_live_tag_guard
        test_check_release_matrix_rejects_unmerged_live_tag
        test_check_release_matrix_rejects_unbound_proof_metadata_version
        test_check_release_matrix_rejects_mutable_artifact_proof
        test_check_release_matrix_rejects_moving_artifact_proof_toolchain
        test_check_release_matrix_rejects_rehearsal_artifact_promotion
        test_check_release_matrix_rejects_unverified_remi_suite_bundle
        test_check_release_matrix_rejects_merge_validation_production_probes
        test_check_release_matrix_rejects_missing_post_deploy_remi_readiness
        test_check_release_matrix_requires_shared_namespace_setup_in_every_workspace_lane
        test_check_release_matrix_rejects_unproven_namespace_action
        test_check_release_matrix_requires_hosted_alpm_parity_producer
        test_check_release_matrix_requires_resilient_alpm_archive_downloads
        test_check_release_matrix_requires_hosted_rpm_parity_producer
        test_check_release_matrix_requires_hosted_debian_parity_producer
        test_check_release_matrix_rejects_namespace_setup_after_workspace_tests
        test_check_release_matrix_rejects_non_failing_artifact_upload
        test_check_release_matrix_rejects_missing_exact_ccs_asset_assertion
        test_check_release_matrix_rejects_missing_tester_authority_boundary
        test_check_release_matrix_rejects_missing_signer_trust_match
        test_check_release_matrix_rejects_unsigned_embedded_ccs_authority
        test_check_release_matrix_rejects_unverified_embedded_ccs_authority
        test_check_release_matrix_rejects_unstable_ccs_release_name
        test_check_release_matrix_rejects_ambiguous_ccs_target_directory
        test_check_release_matrix_rejects_stale_native_output_policy
        test_check_release_matrix_rejects_rpm_extra_output_blind_spot
        test_check_release_matrix_rejects_arch_extra_output_blind_spot
        test_check_release_matrix_rejects_direct_release_publication
        test_check_release_matrix_rejects_moved_tag_publication
        test_check_release_matrix_rejects_mutable_publication_result
        test_check_release_matrix_rejects_late_suite_release_notes
        test_check_release_matrix_rejects_missing_artifact_row
        test_check_release_matrix_rejects_unknown_deploy_route_pair
        test_check_release_matrix_rejects_non_tag_static_site_checkout
        test_check_release_matrix_rejects_single_static_site_deploy
    )

    local test_name
    for test_name in "${tests[@]}"; do
        "$test_name"
        printf 'ok - %s\n' "$test_name"
    done
}

main "$@"
