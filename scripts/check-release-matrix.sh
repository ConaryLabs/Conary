#!/usr/bin/env bash
set -euo pipefail

repo_root="${1:-$(git rev-parse --show-toplevel)}"
cd "$repo_root"

release_build=".github/workflows/release-build.yml"
deploy_workflow=".github/workflows/deploy-and-verify.yml"
artifact_proof_workflow=".github/workflows/release-artifact-proof.yml"
merge_workflow=".github/workflows/merge-validation.yml"
pr_workflow=".github/workflows/pr-gate.yml"
exact_ownership_action=".github/actions/setup-exact-ownership-tests/action.yml"
workspace_setup_action=".github/actions/setup-rust-workspace/action.yml"
artifact_matrix="docs/operations/release-artifact-matrix.md"
feedback_template=".github/ISSUE_TEMPLATE/pre_alpha_feedback.md"
site_preview_release="site/src/lib/preview-release.ts"
site_install_page="site/src/routes/install/+page.svelte"
rpm_containerfile="packaging/rpm/Containerfile.build"
rpm_spec="packaging/rpm/conary.spec"
deb_containerfile="packaging/deb/Containerfile.build"
arch_containerfile="packaging/arch/Containerfile.build"
rpm_build_script="packaging/rpm/build.sh"
deb_build_script="packaging/deb/build.sh"
arch_build_script="packaging/arch/build.sh"
ccs_build_script="packaging/ccs/build.sh"

fedora_release_image='registry.fedoraproject.org/fedora@sha256:765b2260aa4b4eff379b9a6f983f15fcf41a6f9dda9b272b790e23e92fcbaafb'
ubuntu_release_image='docker.io/library/ubuntu@sha256:3131b4cc82a783df6c9df078f86e01819a13594b865c2cad47bd1bca2b7063bb'
arch_release_image='docker.io/library/archlinux@sha256:fe6972d4dc1f660c0c10f4c41b2de8986bab89e7e2955378f8beadb8ebcd7433'
rustup_init_url='https://static.rust-lang.org/rustup/archive/1.28.2/x86_64-unknown-linux-gnu/rustup-init'
rustup_init_sha256='20a06e644b0d9bd2fbdbfd52d42540bdde820ea7df86e92e533c073da0cdd43c'

fail() {
    echo "ERROR: $*" >&2
    exit 1
}

require_match() {
    local file="$1"
    local pattern="$2"
    local description="$3"

    rg -q --multiline "$pattern" "$file" || fail "$description missing in $file"
}

forbid_match() {
    local file="$1"
    local pattern="$2"
    local description="$3"

    if rg -q --multiline "$pattern" "$file"; then
        fail "$description unexpectedly present in $file"
    fi
}

extract_job_block() {
    local file="$1"
    local job="$2"

    awk -v header="  ${job}:" '
        $0 == header {
            in_job = 1
        }
        in_job && $0 != header && /^  [A-Za-z0-9_-]+:/ {
            exit
        }
        in_job {
            print
        }
    ' "$file"
}

require_job_match() {
    local file="$1"
    local job="$2"
    local pattern="$3"
    local description="$4"
    local block

    block="$(extract_job_block "$file" "$job")"
    [[ -n "$block" ]] || fail "$job job missing in $file"
    printf '%s\n' "$block" | rg -q --multiline "$pattern" ||
        fail "$description missing in $file job $job"
}

require_literal_count() {
    local file="$1"
    local literal="$2"
    local expected="$3"
    local description="$4"
    local actual

    actual="$(rg -F -c -- "$literal" "$file" || true)"
    actual="${actual:-0}"
    [[ "$actual" == "$expected" ]] ||
        fail "$description expected $expected occurrences in $file, found $actual"
}

require_artifact_matrix_row() {
    local product="$1"
    local expected_route="$2"
    local row

    row="$(rg -n -- "^\| \`$product\` \|" "$artifact_matrix" || true)"
    [[ -n "$row" ]] || fail "release artifact matrix missing $product row"

    [[ "$row" == *'scripts/release.sh suite'* ]] ||
        fail "release artifact matrix row for $product must use suite construction authority"
    [[ "$row" == *"$expected_route"* ]] ||
        fail "release artifact matrix row for $product missing suite deployment route $expected_route"
    [[ "$row" == *"cargo build -p $product"* ]] ||
        fail "release artifact matrix row for $product missing focused local build"
}

validate_release_topology() {
    [[ "$(bash scripts/release-matrix.sh release-units)" == "suite" ]] ||
        fail "release matrix must expose exactly one suite release unit"
    [[ "$(bash scripts/release-matrix.sh artifacts | tr '\n' ' ')" == "conary remi conaryd conary-test " ]] ||
        fail "release matrix artifact products do not match the four-product suite"
    [[ "$(bash scripts/release-matrix.sh field suite deploy_mode)" == "suite" ]] ||
        fail "suite release must use the suite deployment mode"

    declare -A expected=(
        [conary]=release_bundle
        [remi]=remote_bundle
        [conaryd]=none
        [conary-test]=none
    )
    local product
    for product in "${!expected[@]}"; do
        [[ "$(bash scripts/release-matrix.sh artifact-field "$product" deploy_mode)" == "${expected[$product]}" ]] ||
            fail "unexpected artifact deployment mode for $product"
    done
}

for required_file in \
    "$release_build" \
    "$deploy_workflow" \
    "$artifact_proof_workflow" \
    "$merge_workflow" \
    "$pr_workflow" \
    "$exact_ownership_action" \
    "$workspace_setup_action" \
    "$artifact_matrix" \
    "$feedback_template" \
    "$site_preview_release" \
    "$site_install_page" \
    "$rpm_containerfile" \
    "$rpm_spec" \
    "$deb_containerfile" \
    "$arch_containerfile" \
    "$rpm_build_script" \
    "$deb_build_script" \
    "$arch_build_script" \
    "$ccs_build_script"; do
    [[ -f "$required_file" ]] || fail "missing $required_file"
done

if rg -n -i -- '\bbeta\b|beta[-_]feedback|beta_feedback' .github docs site/src; then
    fail "public maturity surfaces must identify this project as pre-alpha, not beta"
fi
require_match "$feedback_template" '^name: Pre-Alpha Tester Feedback$' 'pre-alpha feedback template name'
require_match "$feedback_template" '^labels: pre-alpha-feedback$' 'pre-alpha feedback label'
require_match "$release_build" 'issues/new\?template=pre_alpha_feedback\.md' 'pre-alpha release-note feedback URL'
require_match "$artifact_matrix" '\.github/ISSUE_TEMPLATE/pre_alpha_feedback\.md' 'pre-alpha artifact-matrix feedback path'
require_match "$site_preview_release" 'issues/new\?template=pre_alpha_feedback\.md' 'pre-alpha site feedback URL'
require_match "$site_install_page" 'Open pre-alpha feedback' 'pre-alpha site feedback label'

require_match "$release_build" "tags: \['v\*'\]" 'single suite release trigger'
forbid_match "$release_build" 'remi-v\*|conaryd-v\*|conary-test-v\*' 'product-prefixed current release trigger'
require_match "$release_build" 'scripts/release-matrix\.sh resolve-tag' 'helper-based tag resolution'
require_match "$release_build" 'scripts/release-matrix\.sh metadata-json' 'helper-based metadata serialization'
require_job_match "$release_build" bundle-suite '\.schema_version == 1[\s\S]*\(\.dry_run \| type\) == "boolean"' 'suite publication metadata schema and boolean dry-run validation'
require_match "$release_build" 'workflow_dispatch is dry-run only; push the canonical tag for live releases' 'manual live-release guardrail'
require_match "$release_build" 'Prepare dry-run release tree' 'dry-run release tree preparation step'
require_match "$release_build" '\./scripts/release\.sh "\$release" --prepare-only --target "\$version"' 'dry-run release tree should be prepared by the canonical suite release script'
require_match "$release_build" 'CONARY_RELEASE_LOCKFILE_MODE: online' 'dry-run release tree should allow online lockfile refreshes in CI'
require_match "$release_build" 'git config --global --add safe\.directory "\$\(pwd\)"' 'dry-run release tree should mark the checked-out repo as a safe git directory'
require_match "$release_build" '\[\[ "\$tag_name" == "v\$\{version\}" \]\]' 'dry-run preparation should bind the target version to the suite tag'
require_job_match "$release_build" prepare 'if \[\[ "\$dry_run" != "true" \]\]; then[\s\S]*scripts/release-matrix\.sh assert-owned-version "\$release" "\$version"' 'live suite tag must match the workspace-owned version'
require_job_match "$release_build" prepare 'git cat-file -t "refs/tags/\$\{tag_name\}"[\s\S]*== "tag"[\s\S]*git rev-parse HEAD[\s\S]*refs/tags/\$\{tag_name\}\^\{\}' 'live suite build must require an annotated tag at the exact checkout'
require_job_match "$release_build" prepare 'git fetch --no-tags origin[\s\S]*refs/heads/main:refs/remotes/origin/main[\s\S]*git merge-base --is-ancestor "refs/tags/\$\{tag_name\}\^\{\}" origin/main' 'live suite tag must already be reachable from a freshly fetched main'
require_literal_count "$release_build" 'bash scripts/release-matrix.sh assert-owned-version "$release" "$version"' 8 'live and dry-run suite-version assertions'
require_job_match "$release_build" build-rpm "image: ${fedora_release_image}" 'release-build RPM builder must use the pinned Fedora 44 image'
require_job_match "$release_build" build-deb "image: ${ubuntu_release_image}" 'release-build DEB builder must use the pinned Ubuntu 26.04 image'
require_job_match "$release_build" build-arch "image: ${arch_release_image}" 'release-build Arch builder must use the pinned Arch image'
require_job_match "$release_build" build-ccs 'name: Install build dependencies[\s\S]*name: Prepare dry-run release tree' 'release-build CCS dry-run prerequisites must be installed before release preparation'
require_match "$workspace_setup_action" 'toolchain:[\s\S]*default: stable[\s\S]*toolchain: \$\{\{ inputs\.toolchain \}\}' 'shared workspace setup typed toolchain input'
for product_job in build-remi build-conaryd build-conary-test; do
    require_job_match "$release_build" "$product_job" 'uses: \./\.github/actions/setup-rust-workspace[\s\S]*toolchain: 1\.97\.1[\s\S]*name: Prepare dry-run release tree' "$product_job exact workspace toolchain must precede Cargo-backed release preparation"
done
require_job_match "$release_build" workspace-validation 'uses: \./\.github/actions/setup-rust-workspace[\s\S]*components: clippy,rustfmt[\s\S]*toolchain: 1\.97\.1' 'release workspace validation exact Rust toolchain'
require_match "$rpm_containerfile" "^FROM ${fedora_release_image}$" 'RPM Containerfile must use the release-build Fedora image digest'
require_match "$deb_containerfile" "^FROM ${ubuntu_release_image}$" 'DEB Containerfile must use the release-build Ubuntu image digest'
require_match "$arch_containerfile" "^FROM ${arch_release_image}$" 'Arch Containerfile must use the release-build Arch image digest'
require_match "$rpm_spec" '^BuildRequires:[[:space:]]+systemd-rpm-macros$' 'RPM spec systemd macro build dependency'
require_job_match "$release_build" build-rpm 'dnf install -y[\s\S]*systemd-rpm-macros' 'release-build RPM systemd macro dependency'
require_match "$rpm_containerfile" 'systemd-rpm-macros[\s\S]*rpm --eval '\''%\{_unitdir\}'\''[\s\S]*/usr/lib/systemd/system' 'RPM Containerfile systemd macro dependency and expansion proof'

rustup_flow_pattern="${rustup_init_url}[\\s\\S]*${rustup_init_sha256}  /tmp/rustup-init[\\s\\S]*sha256sum -c -[\\s\\S]*/tmp/rustup-init -y --default-toolchain 1\\.97\\.1 --profile minimal[\\s\\S]*rm -f /tmp/rustup-init"
require_job_match "$release_build" build-rpm "$rustup_flow_pattern" 'release-build RPM builder checksum-pinned rustup-init flow'
require_job_match "$release_build" build-deb "$rustup_flow_pattern" 'release-build DEB builder checksum-pinned rustup-init flow'
require_match "$rpm_containerfile" "$rustup_flow_pattern" 'RPM Containerfile checksum-pinned rustup-init flow'
require_match "$deb_containerfile" "$rustup_flow_pattern" 'DEB Containerfile checksum-pinned rustup-init flow'
require_job_match "$release_build" build-ccs 'toolchain: 1\.97\.1' 'release-build CCS builder pinned Rust toolchain'
require_job_match "$release_build" build-ccs 'RELEASE_SIGNING_KEY: \$\{\{ secrets\.RELEASE_SIGNING_KEY \}\}[\s\S]*cargo build[\s\S]*--target-dir target[\s\S]*sign_hash --write-ccs-authority "\$authority_dir"[\s\S]*packaging/ccs/build\.sh[\s\S]*--version "\$VERSION"[\s\S]*--key "\$authority_dir/release\.private"' 'CCS build must derive embedded authority from the configured release seed'
require_job_match "$release_build" build-ccs 'conary ccs verify[\s\S]*packaging/ccs/output/conary-\$\{VERSION\}\.ccs[\s\S]*--policy "\$authority_dir/trust-policy\.toml"' 'CCS build must verify its embedded release authority'
require_job_match "$release_build" build-ccs 'RELEASE_SIGNING_KEY must be configured for embedded CCS release authority' 'live CCS build must fail without its release authority'
require_job_match "$release_build" build-arch 'rustup default 1\.97\.1[\s\S]*runuser -u builder -- rustup default 1\.97\.1' 'release-build Arch builder pinned Rust toolchain'
require_match "$arch_containerfile" '^RUN rustup default 1\.97\.1$' 'Arch Containerfile pinned Rust toolchain'
require_literal_count "$release_build" 'uses: actions/upload-artifact@' 11 'release artifact upload actions'
require_literal_count "$release_build" 'if-no-files-found: error' 11 'fail-closed release artifact uploads'
require_job_match "$release_build" bundle-conary 'require_exact_asset CCS[\s\S]*release-packages/conary-\$\{VERSION\}\.ccs"[\s\S]*release-packages/\*\.ccs' 'exact version-matching CCS release asset assertion'
require_job_match "$release_build" bundle-conary 'require_exact_asset RPM[\s\S]*release-packages/conary-\$\{VERSION\}-1\.fc44\.x86_64\.rpm"[\s\S]*release-packages/\*\.rpm' 'exact version-matching RPM release asset assertion'
require_job_match "$release_build" bundle-conary 'require_exact_asset DEB[\s\S]*release-packages/conary_\$\{VERSION\}-1_amd64\.deb"[\s\S]*release-packages/\*\.deb' 'exact version-matching DEB release asset assertion'
require_job_match "$release_build" bundle-conary 'require_exact_asset Arch[\s\S]*release-packages/conary-\$\{VERSION\}-1-x86_64\.pkg\.tar\.zst"[\s\S]*release-packages/\*\.pkg\.tar\.zst' 'exact version-matching Arch release asset assertion'
require_job_match "$release_build" bundle-conary 'CCS_FILE="release-packages/conary-\$\{VERSION\}\.ccs"' 'direct version-matching CCS signing path'
forbid_match "$release_build" 'CCS_FILE=\$\(ls ' 'ambiguous first-match CCS signing path'
require_job_match "$release_build" bundle-conary 'sign_hash --show-public-key[\s\S]*TRUSTED_UPDATE_KEYS[\s\S]*release signing key does not match an embedded trusted update key' 'live signing key must match an embedded trusted update key'
require_job_match "$release_build" bundle-suite 'Publication and released-package proof do not make[\s\S]*pinned external-tester release[\s\S]*issue #110[\s\S]*tester loop stays[\s\S]*paused' 'release notes must keep publication separate from tester authority'
forbid_match "$release_build" '### Supported tester lane|blob/\$\{TAG_NAME\}/docs/guides/agent-assisted-tester-loop\.md' 'premature tester-lane release note'
require_match "$release_build" 'deterministic dry-run signing key' 'dry-run signing fallback'
require_match "$release_build" 'REHEARSAL_SIGNING_PUBLIC_KEY\.txt' 'dry-run signing public key artifact'
require_match "$release_build" 'bundle_name: \$\{\{ steps\.meta\.outputs\.bundle_name \}\}' 'prepare bundle_name output'
require_match "$release_build" 'deploy_mode: \$\{\{ steps\.meta\.outputs\.deploy_mode \}\}' 'prepare deploy_mode output'
require_match "$release_build" 'artifact_patterns: \$\{\{ steps\.meta\.outputs\.artifact_patterns \}\}' 'prepare artifact_patterns output'
require_match "$release_build" 'build-conary-test:' 'conary-test build lane'
require_match "$release_build" 'bundle-suite:' 'single suite publication lane'
forbid_match "$release_build" '^  publish-(remi|conaryd|conary-test):' 'independent product publication lane'
require_match "$release_build" 'workspace-validation:' 'release workspace validation lane'
require_match "$release_build" 'workspace-validation:[\s\S]*needs: prepare' 'release workspace validation should depend on prepare'
require_match "$release_build" 'cargo fmt --check' 'release formatting validation'
require_match "$release_build" 'cargo clippy --workspace --all-targets -- -D warnings' 'release clippy validation'
require_match "$release_build" 'cargo test --workspace --exclude conary-test --verbose' 'release workspace test validation'
require_match "$release_build" 'cargo test -p conary-test --verbose' 'release conary-test validation'
require_match "$release_build" 'cargo test --doc --workspace --verbose' 'release doctest validation'
require_match "$exact_ownership_action" '^        set -euo pipefail$' 'fail-closed exact ownership namespace setup'
require_match "$exact_ownership_action" 'sudo sysctl -w kernel\.apparmor_restrict_unprivileged_userns=0' 'AppArmor user-namespace enablement'
require_match "$exact_ownership_action" 'unshare --user --map-root-user --mount --propagation private /bin/true' 'exact ownership namespace proof'
require_literal_count "$exact_ownership_action" 'sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0' 1 'centralized AppArmor namespace setup'
require_literal_count "$exact_ownership_action" 'unshare --user --map-root-user --mount --propagation private /bin/true' 1 'centralized namespace proof'
for workflow in "$pr_workflow" "$merge_workflow" "$release_build"; do
    require_literal_count "$workflow" 'uses: ./.github/actions/setup-exact-ownership-tests' 1 'shared exact ownership setup'
    forbid_match "$workflow" 'apparmor_restrict_unprivileged_userns|unshare --user' 'inline exact ownership namespace setup'
done
namespace_before_tests_pattern='uses: \./\.github/actions/setup-exact-ownership-tests[\s\S]*cargo test --workspace --exclude conary-test --verbose'
require_job_match "$pr_workflow" workspace-tests "$namespace_before_tests_pattern" 'PR workspace tests exact ownership setup order'
require_job_match "$merge_workflow" workspace-tests "$namespace_before_tests_pattern" 'merge workspace tests exact ownership setup order'
require_job_match "$release_build" workspace-validation "$namespace_before_tests_pattern" 'release workspace validation exact ownership setup order'
require_match "$release_build" 'build-ccs:[\s\S]*needs: \[prepare, workspace-validation\]' 'ccs build should need workspace validation'
require_match "$release_build" 'build-remi:[\s\S]*needs: \[prepare, workspace-validation\]' 'remi build should need workspace validation'
require_job_match "$release_build" bundle-suite 'needs:[\s\S]*bundle-conary[\s\S]*build-remi[\s\S]*build-conaryd[\s\S]*build-conary-test' 'suite publication must wait for every product bundle'
require_job_match "$release_build" bundle-suite 'copy_exact[\s\S]*conary-\$\{VERSION\}\.ccs[\s\S]*remi-\$\{VERSION\}-linux-x64[\s\S]*conaryd-\$\{VERSION\}-linux-x64[\s\S]*conary-test-\$\{VERSION\}-linux-x64' 'suite bundle must require every product artifact'
require_job_match "$release_build" bundle-suite '\.artifacts \| map\(\{product, bundle_name, deploy_mode\}\)[\s\S]*"conary"[\s\S]*"remi"[\s\S]*"conaryd"[\s\S]*"conary-test"' 'suite bundle must validate exact artifact identities and routes'
require_job_match "$release_build" bundle-suite 'cmp[\s\S]*--version[\s\S]*sha256sum -- "\$\{assets\[@\]\}" > SHA256SUMS[\s\S]*sha256sum -c SHA256SUMS' 'suite bundle must prove tar identity, versions, and complete checksums'
require_job_match "$release_build" bundle-suite 'verify_release_tag\(\)[\s\S]*git fetch --force origin[\s\S]*refs/tags/\$\{TAG_NAME\}:refs/tags/\$\{TAG_NAME\}[\s\S]*git cat-file -t "refs/tags/\$\{TAG_NAME\}"[\s\S]*== "tag"[\s\S]*git rev-parse "refs/tags/\$\{TAG_NAME\}\^\{\}"[\s\S]*"\$actual_commit" == "\$EXPECTED_COMMIT"' 'suite publisher must revalidate the exact annotated tag commit'
require_literal_count "$release_build" 'verify_release_tag "before draft mutation"' 1 'suite tag validation before draft mutation'
require_literal_count "$release_build" 'verify_release_tag "before publication"' 1 'suite tag validation before publication'
require_job_match "$release_build" bundle-suite 'gh release edit "\$TAG_NAME" --draft=false[\s\S]*X-GitHub-Api-Version: 2026-03-10[\s\S]*releases/tags/\$\{TAG_NAME\}[\s\S]*\.tag_name == \$tag and \.draft == false and \.immutable == true' 'suite publisher must prove exact immutable state after publication'
immutable_publish_pattern='verify_release_tag "before draft mutation"[\s\S]*if gh release view "\$TAG_NAME" >/dev/null 2>&1; then[\s\S]*--json isDraft --jq[\s\S]*release \$TAG_NAME is already published; refusing to replace immutable assets[\s\S]*else[\s\S]*gh release create "\$TAG_NAME"[\s\S]*--draft[\s\S]*--generate-notes[\s\S]*--verify-tag[\s\S]*fi[\s\S]*gh release edit "\$TAG_NAME" --notes-file "\$release_notes"[\s\S]*gh release upload "\$TAG_NAME" suite-packages/\* --clobber[\s\S]*diff -u "\$local_names" "\$remote_names"[\s\S]*draft release digest[\s\S]*verify_release_tag "before publication"[\s\S]*gh release edit "\$TAG_NAME" --draft=false[\s\S]*\.immutable == true'
require_job_match "$release_build" bundle-suite "$immutable_publish_pattern" 'immutable-compatible single suite publication sequence'
require_literal_count "$release_build" 'gh release create "$TAG_NAME"' 1 'single draft release creation command'
require_literal_count "$release_build" 'gh release upload "$TAG_NAME" suite-packages/* --clobber' 1 'single suite asset upload command'
require_literal_count "$release_build" 'gh release edit "$TAG_NAME" --draft=false' 1 'single release publication command'
forbid_match "$release_build" 'gh release create "\$TAG_NAME" (release|suite)-packages/\*' 'direct published release creation with attached assets'

require_match "$rpm_build_script" 'find "\$OUTPUT".*\*\.rpm.*-delete' 'RPM build must clean stale package output'
require_match "$rpm_build_script" 'Expected exactly one \$NAME \$VERSION x86_64 RPM' 'RPM build must fail without its expected package'
require_match "$rpm_build_script" 'rpm --eval '\''%\{_unitdir\}'\''[\s\S]*systemd-rpm-macros build dependency' 'RPM build must fail fast without systemd macro authority'
require_match "$deb_build_script" 'find "\$OUTPUT".*\*\.deb.*-delete' 'DEB build must clean stale package output'
require_match "$deb_build_script" 'EXPECTED_DEB="\$OUTPUT/\$\{NAME\}_\$\{VERSION\}-1_amd64\.deb"[\s\S]*\[\[ ! -s "\$EXPECTED_DEB"' 'DEB build must require its expected package'
require_match "$arch_build_script" 'find "\$OUTPUT".*\*\.pkg\.tar\.zst.*-delete' 'Arch build must clean stale package output'
require_match "$arch_build_script" 'EXPECTED_PACKAGE="\$OUTPUT/\$\{NAME\}-\$\{VERSION\}-1-x86_64\.pkg\.tar\.zst"[\s\S]*\[\[ ! -s "\$EXPECTED_PACKAGE"' 'Arch build must require its expected package'
require_match "$ccs_build_script" 'find "\$OUTPUT".*\*\.ccs.*-delete' 'CCS build must clean stale package output'
require_match "$ccs_build_script" 'SIGNING_KEY=""[\s\S]*--key\)[\s\S]*SIGNING_KEY="\$2"[\s\S]*CCS release signing key must be a regular, non-symlink file' 'CCS wrapper must require an explicit regular signing key'
require_match "$ccs_build_script" 'CARGO_TARGET_DIR[\s\S]*TARGET_DIR="\$REPO_ROOT/target"[\s\S]*RELEASE_BIN="\$TARGET_DIR/release/\$NAME"[\s\S]*--target-dir "\$TARGET_DIR"' 'CCS wrapper must use one explicit Cargo target directory'
require_match "$ccs_build_script" 'BUILT_CCS="\$OUTPUT/\$\{NAME\}-\$\{VERSION\}-1\.ccs"[\s\S]*EXPECTED_CCS="\$OUTPUT/\$\{NAME\}-\$\{VERSION\}\.ccs"[\s\S]*Expected exactly one CCS package at \$BUILT_CCS[\s\S]*mv -- "\$BUILT_CCS" "\$EXPECTED_CCS"' 'CCS wrapper must normalize one exact package-release name to the stable self-update asset'

require_match "$merge_workflow" 'workflow-runtime-policy:' 'merge validation workflow runtime policy job'
require_match "$merge_workflow" 'bash scripts/test-github-action-runtimes\.sh' 'merge validation action checker test'
require_match "$merge_workflow" 'release-matrix-policy:' 'merge validation release matrix policy job'
require_match "$merge_workflow" 'bash scripts/test-release-matrix\.sh' 'merge validation release matrix test'
require_match "$merge_workflow" 'bash scripts/test-remi-deploy-helper\.sh' 'merge validation deploy helper test'
require_match "$merge_workflow" 'bash scripts/test-remi-health\.sh' 'merge validation Remi health test'
require_match "$merge_workflow" 'bash scripts/test-deploy-sites\.sh' 'merge validation static-site deploy wrapper test'
require_match "$merge_workflow" 'fmt:' 'merge validation formatting job'
require_match "$merge_workflow" 'dependency-consistency:' 'merge validation dependency consistency job'
require_match "$merge_workflow" 'clippy:' 'merge validation clippy job'
require_match "$merge_workflow" 'workspace-tests:' 'merge validation workspace test job'
require_match "$merge_workflow" 'conary-test-crate:' 'merge validation conary-test job'
require_match "$merge_workflow" 'doctests:' 'merge validation doctest job'
forbid_match "$merge_workflow" 'scripts/remi-health\.sh|https://remi\.conary\.io' 'mutable production Remi probe in source merge validation'

require_match "$deploy_workflow" 'bundle_name: \$\{\{ steps\.meta\.outputs\.bundle_name \}\}' 'deploy resolve bundle_name output'
require_match "$deploy_workflow" 'deploy_mode: \$\{\{ steps\.meta\.outputs\.deploy_mode \}\}' 'deploy resolve deploy_mode output'
require_match "$deploy_workflow" 'artifact_patterns: \$\{\{ steps\.meta\.outputs\.artifact_patterns \}\}' 'deploy resolve artifact_patterns output'
require_match "$deploy_workflow" 'artifacts: \$\{\{ steps\.meta\.outputs\.artifacts \}\}' 'deploy resolve typed artifact outputs'
require_match "$deploy_workflow" 'validate-routing:' 'deploy routing validation job'
require_match "$deploy_workflow" 'No deploy lane defined for release=' 'explicit unmatched deploy failure'
require_match "$deploy_workflow" 'verify-build-only-routes:' 'explicit build-only artifact route proof'
forbid_match "$deploy_workflow" '^  (deploy|verify)-(conaryd|conary-test):' 'deployment job for a build-only suite artifact'
require_job_match "$deploy_workflow" resolve 'source_run_json=[\s\S]*\.github/workflows/release-build\.yml[\s\S]*did not conclude successfully' 'deploy source must be a successful release-build run'
require_job_match "$deploy_workflow" resolve 'MANUAL_DRY_RUN" == "false" && "\$dry_run" == "true"[\s\S]*manual deployment cannot promote rehearsal artifacts into a live deployment' 'manual deployment must not promote rehearsal artifacts'
require_job_match "$deploy_workflow" resolve 'metadata_file="source-artifacts/suite-bundle/metadata\.json"[\s\S]*metadata tag_name does not match version[\s\S]*expected suite-bundle' 'deploy metadata must come from the exact typed suite bundle'
require_job_match "$deploy_workflow" resolve '\.schema_version == 1 and \(\.dry_run \| type\) == "boolean"' 'deploy metadata schema and boolean dry-run validation'
require_job_match "$deploy_workflow" validate-routing 'map\(\{product, bundle_name, deploy_mode\}\) == \[[\s\S]*"product":"conary","bundle_name":"release-bundle","deploy_mode":"release_bundle"[\s\S]*"product":"remi","bundle_name":"remi-bundle","deploy_mode":"remote_bundle"[\s\S]*"product":"conaryd","bundle_name":"conaryd-bundle","deploy_mode":"none"[\s\S]*"product":"conary-test","bundle_name":"conary-test-bundle","deploy_mode":"none"' 'exact serialized artifact deployment routes'
validate_release_topology
require_match "$deploy_workflow" 'BUNDLE_NAME: \$\{\{ needs\.resolve\.outputs\.bundle_name \}\}' 'bundle_name-driven artifact lookup'
require_match "$deploy_workflow" 'gh api "repos/\$\{?GH_REPO\}?/actions/runs/\$\{?SOURCE_RUN\}?" --jq '\''\.head_branch'\''' 'source-run head-branch lookup for release fallback'
require_match "$deploy_workflow" 'gh release download "\$source_tag"' 'release-asset fallback for expired source-run artifacts'
require_job_match "$deploy_workflow" deploy-conary 'name: Verify self-update endpoint[\s\S]*name: Check out exact release tag for static sites[\s\S]*ref: \$\{\{ needs\.resolve\.outputs\.tag_name \}\}' 'live Conary static-site checkout must use the serialized release tag after endpoint verification'
require_job_match "$deploy_workflow" deploy-conary 'name: Check out exact release tag for static sites[\s\S]*persist-credentials: false[\s\S]*git tag --points-at HEAD \| grep -Fx "\$TAG_NAME"' 'live Conary static-site checkout verification'
require_job_match "$deploy_workflow" deploy-conary 'name: Set up pinned Node\.js for static sites[\s\S]*actions/setup-node@820762786026740c76f36085b0efc47a31fe5020[\s\S]*node-version: '\''24'\''' 'live Conary static-site pinned Node setup'
require_job_match "$deploy_workflow" deploy-conary 'name: Install locked static-site dependencies[\s\S]*npm ci --prefix site[\s\S]*npm ci --prefix web' 'live Conary locked static-site dependency installation'
require_job_match "$deploy_workflow" deploy-conary 'name: Configure static-site deployment access[\s\S]*REMI_SSH_KEY: \$\{\{ secrets\.REMI_SSH_KEY \}\}[\s\S]*REMI_SSH_TARGET: \$\{\{ secrets\.REMI_SSH_TARGET \}\}' 'live Conary static-site SSH configuration'
require_job_match "$deploy_workflow" deploy-conary 'name: Deploy both static sites from the release tag[\s\S]*bash deploy/deploy-sites\.sh both' 'live Conary both-site deployment from the release tag'
require_job_match "$deploy_workflow" deploy-conary 'needs: \[resolve, validate-routing, deploy-remi\][\s\S]*needs\.deploy-remi\.result == '\''success'\''' 'Conary deployment must follow successful Remi deployment for one suite'
require_job_match "$deploy_workflow" deploy-conary 'sha256sum -c SHA256SUMS[\s\S]*conary_deploy_dir[\s\S]*conary-\$\{VERSION\}\.ccs[\s\S]*sha256sum -- \* > SHA256SUMS' 'Conary deployment must verify the suite and stage only its product assets'
require_job_match "$deploy_workflow" prove-conary-release-artifacts 'needs: \[resolve, deploy-conary\][\s\S]*needs\.deploy-conary\.result == '\''success'\''[\s\S]*uses: \./\.github/workflows/release-artifact-proof\.yml[\s\S]*tag_name: \$\{\{ needs\.resolve\.outputs\.tag_name \}\}' 'live Conary deployment must hand the serialized tag to published-artifact proof'
require_job_match "$deploy_workflow" deploy-remi 'name: Deploy remi bundle[\s\S]*name: Verify remi health[\s\S]*curl -fsS https://remi\.conary\.io/health >/dev/null[\s\S]*name: Verify remi readiness[\s\S]*body=\$\(curl -fsS --max-time 30 https://remi\.conary\.io/health/ready\)[\s\S]*jq -e '\''\.ready == true'\''' 'exact post-deploy Remi liveness and structured readiness proof'
require_job_match "$deploy_workflow" deploy-remi 'bundle_dir="source-artifacts/\$\{BUNDLE_NAME\}"[\s\S]*sha256sum -c SHA256SUMS[\s\S]*bundle="\$\{bundle_dir\}/remi-\$\{VERSION\}-linux-x64\.tar\.gz"' 'Remi deployment must verify the complete suite checksums before staging its bundle'
forbid_match "$deploy_workflow" 'CONARYD_VERIFY_URL' 'obsolete public verify URL'
forbid_match "$deploy_workflow" '24273700060' 'retired one-time conaryd bootstrap exception'
forbid_match "$deploy_workflow" 'deploy_asset_ref' 'retired bootstrap-only deploy asset ref'
forbid_match "$deploy_workflow" 'bootstrap_exception' 'retired bootstrap exception output'

require_match "$artifact_proof_workflow" 'workflow_call:[\s\S]*tag_name:[\s\S]*required: true[\s\S]*type: string' 'reusable published-artifact proof input'
require_match "$artifact_proof_workflow" 'workflow_dispatch:[\s\S]*tag_name:[\s\S]*required: true[\s\S]*type: string' 'manual published-artifact proof input'
require_job_match "$artifact_proof_workflow" native-package-lifecycle 'actions/checkout@[0-9a-f]+[\s\S]*ref: \$\{\{ inputs\.tag_name \}\}[\s\S]*fetch-depth: 0[\s\S]*persist-credentials: false' 'published artifact proof must run the exact tag harness'
require_job_match "$artifact_proof_workflow" native-package-lifecycle 'uses: \./\.github/actions/setup-rust-workspace[\s\S]*toolchain: 1\.97\.1[\s\S]*name: Require the hosted container runtime' 'published artifact proof exact Rust toolchain'
require_job_match "$artifact_proof_workflow" native-package-lifecycle 'distro: fedora44[\s\S]*native_format: rpm[\s\S]*distro: ubuntu-26\.04[\s\S]*native_format: deb[\s\S]*distro: arch[\s\S]*native_format: arch' 'published-artifact three-distro typed matrix'
require_job_match "$artifact_proof_workflow" native-package-lifecycle 'release-matrix\.sh resolve-tag "\$RELEASE_TAG"[\s\S]*resolved_version=.*\^version=[\s\S]*git cat-file -t "\$RELEASE_TAG"[\s\S]*== "tag"[\s\S]*git worktree add --detach "\$tag_tree"[\s\S]*"\$version" == "\$resolved_version"[\s\S]*"\$tag_tree/scripts/release-matrix\.sh" assert-owned-version suite "\$version"' 'published artifact proof must bind metadata to the annotated tag version and suite authority'
require_job_match "$artifact_proof_workflow" native-package-lifecycle 'gh api[\s\S]*X-GitHub-Api-Version: 2026-03-10[\s\S]*releases/tags/\$\{RELEASE_TAG\}[\s\S]*\$\(jq -r '\''\.draft'\'' <<< "\$release_state"\)" == "false"[\s\S]*\$\(jq -r '\''\.immutable'\'' <<< "\$release_state"\)" == "true"' 'published artifact proof must reject a draft, mutable, or mismatched GitHub release'
require_job_match "$artifact_proof_workflow" native-package-lifecycle '\.schema_version == 1 and \(\.dry_run \| type\) == "boolean"' 'published artifact metadata schema and boolean dry-run validation'
require_job_match "$artifact_proof_workflow" native-package-lifecycle 'gh release download "\$RELEASE_TAG"[\s\S]*--pattern metadata\.json[\s\S]*sha256sum -c SHA256SUMS --ignore-missing[\s\S]*published_digest[\s\S]*actual_digest' 'published artifact metadata, checksum, and GitHub digest proof'
require_job_match "$artifact_proof_workflow" native-package-lifecycle 'images build[\s\S]*--native-package "\$\{\{ steps\.release\.outputs\.native_package \}\}"[\s\S]*CONARY_TEST_REUSE_IMAGE: "1"[\s\S]*--suite native-cross-source-lifecycle' 'published native package installation and Cartesian lifecycle proof'
require_job_match "$artifact_proof_workflow" release-artifact-proof 'needs: native-package-lifecycle[\s\S]*MATRIX_RESULT[\s\S]*"\$MATRIX_RESULT" != "success"' 'stable all-distro published-artifact proof gate'

require_artifact_matrix_row conary "protected release assets"
require_artifact_matrix_row remi "protected Remi deployment"
require_artifact_matrix_row conaryd '`none`'
require_artifact_matrix_row conary-test '`none`'

require_match "$artifact_matrix" 'one annotated `vMAJOR\.MINOR\.PATCH` tag publishes one GitHub release' 'one canonical suite publication contract'
require_match "$artifact_matrix" 'checksums|SHA-256' 'suite checksum evidence contract'
require_match "$artifact_matrix" 'signature' 'per-artifact signature evidence contract'
require_match "$artifact_matrix" 'SBOM' 'per-artifact SBOM evidence contract'
require_match "$artifact_matrix" 'provenance' 'per-artifact provenance evidence contract'

echo "Release matrix workflow checks passed."
