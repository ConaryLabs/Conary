#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

usage() {
    cat >&2 <<'EOF'
Usage: scripts/agentic-plan-review.sh <plan-or-spec.md> [options]

Run the local multi-model plan/spec review loop.

Options:
  --only <all|deepseek|gemini>   Select reviewers. Defaults to all.
  --out-dir <path>               Review output directory. Defaults to docs/superpowers/reviews.
  --deepseek-model <name>        Reasonix model alias. Defaults to deepseek-pro.
  --gemini-model <name>          Antigravity model name. Defaults to Gemini 3.5 Flash (High).
  --print-timeout <duration>     agy print timeout. Defaults to 60m.
  --dry-run                      Print planned commands and output paths without running reviewers.
  -h, --help                     Show this help.

Review files are local-only by default because docs/superpowers/reviews/ is ignored.
Secrets are read by each CLI from its own local configuration or environment.
EOF
}

fail() {
    echo "ERROR: $*" >&2
    exit 1
}

review_target=""
only="all"
out_dir="docs/superpowers/reviews"
deepseek_model="deepseek-pro"
gemini_model="Gemini 3.5 Flash (High)"
print_timeout="60m"
dry_run=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        --only)
            [[ $# -ge 2 ]] || {
                usage
                exit 2
            }
            only="$2"
            shift 2
            ;;
        --out-dir)
            [[ $# -ge 2 ]] || {
                usage
                exit 2
            }
            out_dir="$2"
            shift 2
            ;;
        --deepseek-model)
            [[ $# -ge 2 ]] || {
                usage
                exit 2
            }
            deepseek_model="$2"
            shift 2
            ;;
        --gemini-model)
            [[ $# -ge 2 ]] || {
                usage
                exit 2
            }
            gemini_model="$2"
            shift 2
            ;;
        --print-timeout)
            [[ $# -ge 2 ]] || {
                usage
                exit 2
            }
            print_timeout="$2"
            shift 2
            ;;
        --dry-run)
            dry_run=1
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        --*)
            usage
            exit 2
            ;;
        *)
            if [[ -n "$review_target" ]]; then
                usage
                exit 2
            fi
            review_target="$1"
            shift
            ;;
    esac
done

case "$only" in
    all|deepseek|gemini)
        ;;
    *)
        usage
        exit 2
        ;;
esac

[[ -n "$review_target" ]] || {
    usage
    exit 2
}

[[ -f "$review_target" ]] || fail "review target not found: $review_target"

slug_from_path() {
    local path="$1"
    local base
    base="$(basename "$path")"
    base="${base%.md}"
    printf '%s' "$base" | tr '[:upper:]' '[:lower:]' \
        | sed -E 's/[^a-z0-9]+/-/g; s/^-+//; s/-+$//'
}

model_slug() {
    local provider="$1"
    local model="$2"

    if [[ "$provider" == "gemini" && "$model" == "Gemini 3.5 Flash (High)" ]]; then
        printf 'gemini-35-flash-high'
        return
    fi

    printf '%s' "$model" | tr '[:upper:]' '[:lower:]' \
        | sed -E 's/[^a-z0-9]+/-/g; s/^-+//; s/-+$//'
}

strip_ansi() {
    sed -E $'s/\x1B\\[[0-9;?]*[ -/]*[@-~]//g'
}

build_prompt() {
    cat <<EOF
You are conducting a senior security/packaging design review for the Conary repository. This is review only: do not modify files, do not create files, do not run writes, and do not make commits.

Review target:
- $review_target

Required local context to inspect before judging:
- AGENTS.md
- docs/llms/README.md
- docs/ARCHITECTURE.md
- docs/modules/recipe.md
- docs/modules/ccs.md
- docs/modules/feature-ownership.md
- docs/superpowers/specs/archive/2026-06-13-m2-publish-hardening-remi-design.md
- apps/conary/src/commands/publish.rs
- apps/conary/src/commands/cook.rs
- apps/conary/src/command_risk.rs
- crates/conary-core/src/recipe/kitchen/config.rs
- crates/conary-core/src/recipe/kitchen/cook.rs
- crates/conary-core/src/recipe/kitchen/provenance_capture.rs
- crates/conary-core/src/recipe/pkgbuild.rs
- crates/conary-core/src/ccs/manifest.rs
- crates/conary-core/src/ccs/convert/command_evidence.rs
- crates/conary-core/src/container/analysis.rs

Review goals:
1. Check whether the target plan/spec is implementable from the current codebase without hidden prerequisites.
2. Find security/provenance gaps, especially around hermeticity, local source identity, offline dependency/cache policy, build-body/package-manager fetch risk, reproducibility controls, and publish gates.
3. Find task ordering problems or missing regression tests.
4. Check whether it respects its parent design boundaries and does not pull later slices forward without a deliberate gate.
5. Identify any file-size/refactor hazards, especially around ccs::manifest and kitchen cook ownership.

Output format:
- Start with a one-paragraph verdict.
- Then list findings ordered by severity. Each finding must include: severity, affected plan section or repository file/line if available, why it matters, and the exact plan adjustment you recommend.
- Include open questions only if they block implementation.
- End with a short list of recommended patch bullets for the plan.

Be tough but practical. Prefer concrete, actionable findings over general advice.
EOF
}

write_header() {
    local title="$1"
    local tool="$2"
    local model="$3"

    printf '# %s\n\n' "$title"
    printf 'review_tool: %s\n' "$tool"
    printf 'model: %s\n' "$model"
    printf 'repo: %s\n' "$repo_root"
    printf 'commit: %s\n' "$(git rev-parse HEAD)"
    printf 'target: %s\n' "$review_target"
    printf 'generated_at: %s\n\n' "$(date -Is)"
}

target_slug="$(slug_from_path "$review_target")"
timestamp_prefix="$(date +%Y-%m-%d-%H%M%S)"
deepseek_out="$out_dir/$timestamp_prefix-$target_slug-$(model_slug deepseek "$deepseek_model").md"
gemini_out="$out_dir/$timestamp_prefix-$target_slug-$(model_slug gemini "$gemini_model").md"

if [[ "$dry_run" -eq 1 ]]; then
    printf 'DRY RUN\n'
    printf 'target: %s\n' "$review_target"
    if [[ "$only" == "all" || "$only" == "deepseek" ]]; then
        printf 'DeepSeek output: %s\n' "$deepseek_out"
        printf 'DeepSeek command: reasonix run --model %s <prompt>\n' "$deepseek_model"
    fi
    if [[ "$only" == "all" || "$only" == "gemini" ]]; then
        printf 'Gemini output: %s\n' "$gemini_out"
        printf "Gemini command: agy --model '%s' --print-timeout %s --print <prompt>\n" "$gemini_model" "$print_timeout"
    fi
    exit 0
fi

mkdir -p "$out_dir"

if [[ "$only" == "all" || "$only" == "deepseek" ]]; then
    command -v reasonix >/dev/null 2>&1 || fail "reasonix not found in PATH"
    {
        write_header "DeepSeek Pro Review: $(basename "$review_target")" "reasonix" "$deepseek_model"
        build_prompt | reasonix run --model "$deepseek_model" | strip_ansi
    } > "$deepseek_out" 2>&1
    printf 'DeepSeek review: %s\n' "$deepseek_out"
fi

if [[ "$only" == "all" || "$only" == "gemini" ]]; then
    command -v agy >/dev/null 2>&1 || fail "agy not found in PATH"
    prompt="$(build_prompt)"
    {
        write_header "Gemini Review: $(basename "$review_target")" "agy" "$gemini_model"
        agy --model "$gemini_model" --print-timeout "$print_timeout" --print "$prompt" | strip_ansi
    } > "$gemini_out" 2>&1
    printf 'Gemini review: %s\n' "$gemini_out"
fi
