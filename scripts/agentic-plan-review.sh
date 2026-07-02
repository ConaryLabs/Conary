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
  --review-kind <auto|design|plan|implementation>
                                  Select the review rubric. Defaults to auto.
  --context <path>                Add an extra local context path to the prompt. Repeatable.
  --feature <slug>               Feature ownership card whose Start here,
                                 Docs to update, Paths, and Safety notes feed
                                 the prompt. Repeatable; at least one required.
                                 Example: --feature packaging --feature ccs --feature remi
  --deepseek-model <name>        Reasonix model alias. Defaults to deepseek-pro.
  --gemini-model <name>          Antigravity model name. Defaults to Gemini 3.5 Flash (High).
  --print-timeout <duration>     agy print timeout. Defaults to 90m.
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
review_kind="auto"
extra_context=()
features=()
deepseek_model="deepseek-pro"
gemini_model="Gemini 3.5 Flash (High)"
print_timeout="90m"
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
        --review-kind)
            [[ $# -ge 2 ]] || {
                usage
                exit 2
            }
            review_kind="$2"
            shift 2
            ;;
        --context)
            [[ $# -ge 2 ]] || {
                usage
                exit 2
            }
            extra_context+=("$2")
            shift 2
            ;;
        --feature)
            [[ $# -ge 2 ]] || {
                usage
                exit 2
            }
            features+=("$2")
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

case "$review_kind" in
    auto|design|plan|implementation)
        ;;
    *)
        fail "invalid review kind: $review_kind"
        ;;
esac

[[ -n "$review_target" ]] || {
    usage
    exit 2
}

[[ -f "$review_target" ]] || fail "review target not found: $review_target"

for context_path in "${extra_context[@]}"; do
    [[ -e "$context_path" ]] || fail "extra context path not found: $context_path"
done

[[ "${#features[@]}" -ge 1 ]] \
    || fail "at least one --feature <slug> is required (list slugs with: bash scripts/agent-context.sh --list)"

feature_context_file="$(mktemp)"
feature_pressure_file="$(mktemp)"
trap 'rm -f "$feature_context_file" "$feature_pressure_file"' EXIT

packet_section() {
    local section_heading="$1"
    awk -v want="## $section_heading" '
        $0 == want { on = 1; next }
        /^## /     { on = 0 }
        on && NF   { print }
    '
}

resolve_features() {
    local slug packet heading
    for slug in "${features[@]}"; do
        packet="$(bash scripts/agent-context.sh --feature "$slug")" \
            || fail "unknown feature slug: $slug (list slugs with: bash scripts/agent-context.sh --list)"
        heading="$(sed -n 's/^# Task Packet: //p' <<<"$packet")"
        {
            packet_section "Read first" <<<"$packet"
            packet_section "Docs to update" <<<"$packet"
            packet_section "Paths owned" <<<"$packet"
        } | tr -d '`' >> "$feature_context_file"
        printf -- '- %s: %s\n' "$heading" "$(packet_section "Safety invariants" <<<"$packet")" \
            >> "$feature_pressure_file"
    done
}

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

infer_review_kind() {
    local path="$1"
    local base
    base="$(basename "$path")"

    if [[ "$review_kind" != "auto" ]]; then
        printf '%s\n' "$review_kind"
        return
    fi

    case "$path" in
        */plans/*)
            printf 'plan\n'
            return
            ;;
        */specs/*)
            printf 'design\n'
            return
            ;;
    esac

    case "$base" in
        *implementation-plan*.md|*plan*.md)
            printf 'plan\n'
            ;;
        *design*.md|*spec*.md)
            printf 'design\n'
            ;;
        *implementation*.md)
            printf 'implementation\n'
            ;;
        *)
            printf 'design\n'
            ;;
    esac
}

build_prompt() {
    cat <<EOF
You are conducting a senior implementation/security review for the Conary repository. This is review only: do not modify files, do not create files, do not run writes, and do not make commits.

Do not rush. Treat this as a deep pre-lock review. Before producing findings, perform separate passes for repository-context fit, scope and milestone boundaries, failure/security/trust behavior, migration and compatibility hazards, implementation-plan readiness, and missing tests.

Review target:
- $review_target

Review kind: $resolved_review_kind

Required local context to inspect before judging:
- AGENTS.md
- docs/llms/README.md
- docs/ARCHITECTURE.md
- docs/INTEGRATION-TESTING.md
- docs/modules/feature-ownership.md
EOF

    LC_ALL=C sort -u "$feature_context_file" | sed 's/^/- /'

    if [[ "${#extra_context[@]}" -gt 0 ]]; then
        printf '\nAdditional local context requested by caller:\n'
        for context_path in "${extra_context[@]}"; do
            printf -- '- %s\n' "$context_path"
        done
    fi

    cat <<'EOF'

Shared review goals:
1. Check whether the target is implementable or lockable from the current codebase without hidden prerequisites.
2. Find security, provenance, trust, migration, and failure-behavior gaps.
3. Find task ordering problems, underspecified ownership boundaries, or missing regression tests.
4. Check whether it respects parent design boundaries and does not pull later slices forward without a deliberate gate.
5. Check whether supported target language stays limited to Fedora 44, Ubuntu 26.04, and Arch unless the target explicitly scopes otherwise.

Review pressure points from feature ownership:
EOF

    cat "$feature_pressure_file"

    cat <<'EOF'

Evidence rules:
- Cite the target section and repository file/line when available.
- If a required path is irrelevant to the target, say so briefly instead of forcing a finding.
- Do not invent code or behavior that is not in the repository.
- Treat broad compatibility claims, silent defaulting, live host I/O in core validation, and conversion-only evidence as review pressure points.
EOF

    case "$resolved_review_kind" in
        design)
            cat <<'EOF'

Design-specific review rubric:
1. Is the problem statement grounded in current repo facts?
2. Are scope, non-goals, invariants, and slice boundaries sharp enough to prevent accidental implementation drift?
3. Are migration and compatibility stances explicit, especially where old behavior should be deleted, narrowed, or fail-closed?
4. Are failure semantics visible, retryable where relevant, and non-destructive where the design promises that?
5. Are child designs/plans given the right decision questions without pretending this design already solved them?
6. Are docs-audit, feature-coherency, and verification expectations named at the right level?
EOF
            ;;
        plan)
            cat <<'EOF'

Plan-specific review rubric:
1. Are tasks ordered so tests, ownership boundaries, schema changes, and call-site migrations happen safely?
2. Does every behavior change have a focused regression test and an expected failing state before implementation?
3. Are code ownership boundaries clear enough to avoid expanding existing hotspots unnecessarily?
4. Are rollback, retry, failure recovery, and diagnostic outputs covered where the design requires them?
5. Are verification commands specific, sufficient, and sequenced after the relevant tasks?
6. Are docs/coherency updates included when public claims, command help, routes, or assistant-facing surfaces change?
EOF
            ;;
        implementation)
            cat <<'EOF'

Implementation-specific review rubric:
1. Does the described implementation match the target design or plan without unreviewed scope creep?
2. Are changed files owned by the expected subsystem, with large-file or hotspot risks acknowledged?
3. Are security, trust, migration, and compatibility behaviors represented in code and tests?
4. Are failure paths covered with regression tests instead of only happy-path proof?
5. Are verification results enough to support merge/commit/push decisions?
EOF
            ;;
    esac

    cat <<'EOF'

Output format:
- Start with a one-paragraph verdict.
- Then list findings ordered by severity. Each finding must include: severity, blocking/non-blocking status, affected target section or repository file/line if available, repository evidence, why it matters, and the exact adjustment you recommend.
- Include open questions only if they block implementation.
- End with a short list of recommended patch bullets for the target.

Be tough but practical. Prefer concrete, actionable findings over general advice. If there are no blocking findings, say that explicitly and still list any non-blocking tightening suggestions.
EOF
}

write_header() {
    local title="$1"
    local tool="$2"
    local model="$3"

    printf '# %s\n\n' "$title"
    printf 'review_tool: %s\n' "$tool"
    printf 'model: %s\n' "$model"
    printf 'review_kind: %s\n' "$resolved_review_kind"
    printf 'repo: %s\n' "$repo_root"
    printf 'commit: %s\n' "$(git rev-parse HEAD)"
    printf 'target: %s\n' "$review_target"
    printf 'generated_at: %s\n\n' "$(date -Is)"
}

target_slug="$(slug_from_path "$review_target")"
timestamp_prefix="$(date +%Y-%m-%d-%H%M%S)"
resolved_review_kind="$(infer_review_kind "$review_target")"
resolve_features
deepseek_out="$out_dir/$timestamp_prefix-$target_slug-$(model_slug deepseek "$deepseek_model").md"
gemini_out="$out_dir/$timestamp_prefix-$target_slug-$(model_slug gemini "$gemini_model").md"

if [[ "$dry_run" -eq 1 ]]; then
    printf 'DRY RUN\n'
    printf 'target: %s\n' "$review_target"
    printf 'review kind: %s\n' "$resolved_review_kind"
    if [[ "${#extra_context[@]}" -gt 0 ]]; then
        printf 'extra context:\n'
        for context_path in "${extra_context[@]}"; do
            printf -- '- %s\n' "$context_path"
        done
    fi
    if [[ "$only" == "all" || "$only" == "deepseek" ]]; then
        printf 'DeepSeek output: %s\n' "$deepseek_out"
        printf 'DeepSeek command: reasonix run --model %s <prompt>\n' "$deepseek_model"
    fi
    if [[ "$only" == "all" || "$only" == "gemini" ]]; then
        printf 'Gemini output: %s\n' "$gemini_out"
        printf "Gemini command: agy --model '%s' --print-timeout %s --print <prompt>\n" "$gemini_model" "$print_timeout"
    fi
    printf 'features:'
    printf ' %s' "${features[@]}"
    printf '\n'
    printf -- '--- prompt ---\n'
    build_prompt
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
