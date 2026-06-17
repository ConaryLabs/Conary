#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

fail() {
    echo "ERROR: $*" >&2
    exit 1
}

script="scripts/agentic-plan-review.sh"
[[ -x "$script" ]] || fail "$script is not executable"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

bin_dir="$tmp/bin"
out_dir="$tmp/reviews"
mkdir -p "$bin_dir" "$out_dir"
fixture_dir="$tmp/review-fixtures"
mkdir -p "$fixture_dir"
target="$fixture_dir/m4-wrapper-plan.md"
design_target="$fixture_dir/m4-wrapper-design.md"
printf '# M4 wrapper plan fixture\n' >"$target"
printf '# M4 wrapper design fixture\n' >"$design_target"

cat > "$bin_dir/reasonix" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
prompt="$(cat)"
printf 'reasonix args:'
printf ' [%s]' "$@"
printf '\n'
printf 'reasonix prompt contains target: '
grep -q 'm4-wrapper-plan.md' <<<"$prompt" && printf 'yes\n'
printf 'reasonix prompt contains review kind: '
grep -q 'Review kind: plan' <<<"$prompt" && printf 'yes\n'
printf 'reasonix prompt contains deep passes: '
grep -q 'perform separate passes' <<<"$prompt" && printf 'yes\n'
printf 'reasonix prompt contains extra context: '
grep -q 'docs/modules/remi.md' <<<"$prompt" && printf 'yes\n'
STUB
chmod +x "$bin_dir/reasonix"

cat > "$bin_dir/agy" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
printf 'agy args:'
printf ' [%s]' "$@"
printf '\n'
prompt="$*"
printf 'agy prompt contains target: '
grep -q 'm4-wrapper-plan.md' <<<"$prompt" && printf 'yes\n'
printf 'agy prompt contains review kind: '
grep -q 'Review kind: plan' <<<"$prompt" && printf 'yes\n'
printf 'agy prompt contains deep passes: '
grep -q 'perform separate passes' <<<"$prompt" && printf 'yes\n'
printf 'agy prompt contains extra context: '
grep -q 'docs/modules/remi.md' <<<"$prompt" && printf 'yes\n'
STUB
chmod +x "$bin_dir/agy"

help_output="$("$script" --help 2>&1)"
grep -q "Usage: scripts/agentic-plan-review.sh" <<<"$help_output" \
    || fail "help output did not include usage"
grep -q "deepseek-pro" <<<"$help_output" \
    || fail "help output did not name the default DeepSeek model"
grep -q "Defaults to 90m" <<<"$help_output" \
    || fail "help output did not name the longer default print timeout"
grep -q -- "--review-kind <auto|design|plan|implementation>" <<<"$help_output" \
    || fail "help output did not name review-kind"
grep -q -- "--context <path>" <<<"$help_output" \
    || fail "help output did not name extra context"

if "$script" "does-not-exist.md" --out-dir "$out_dir" >"$tmp/missing.out" 2>&1; then
    fail "missing review target unexpectedly succeeded"
fi
grep -q "review target not found" "$tmp/missing.out" \
    || fail "missing target did not produce a clear error"

if "$script" "$target" --review-kind nonsense --out-dir "$out_dir" >"$tmp/bad-kind.out" 2>&1; then
    fail "invalid review kind unexpectedly succeeded"
fi
grep -q "invalid review kind" "$tmp/bad-kind.out" \
    || fail "invalid review kind did not produce a clear error"

if "$script" "$target" --context docs/does-not-exist.md --out-dir "$out_dir" >"$tmp/bad-context.out" 2>&1; then
    fail "missing extra context unexpectedly succeeded"
fi
grep -q "extra context path not found" "$tmp/bad-context.out" \
    || fail "missing extra context did not produce a clear error"

PATH="$bin_dir:$PATH" "$script" "$target" --context docs/modules/remi.md --out-dir "$out_dir" >"$tmp/run.out"

grep -q "DeepSeek review:" "$tmp/run.out" \
    || fail "script did not report the DeepSeek output path"
grep -q "Gemini review:" "$tmp/run.out" \
    || fail "script did not report the Gemini output path"

deepseek_review="$(find "$out_dir" -type f -name '*deepseek-pro.md' -print -quit)"
gemini_review="$(find "$out_dir" -type f -name '*gemini-35-flash-high.md' -print -quit)"

[[ -n "$deepseek_review" ]] || fail "DeepSeek review file was not created"
[[ -n "$gemini_review" ]] || fail "Gemini review file was not created"

grep -q "model: deepseek-pro" "$deepseek_review" \
    || fail "DeepSeek review metadata missing model"
grep -q "review_kind: plan" "$deepseek_review" \
    || fail "DeepSeek review metadata missing resolved review kind"
grep -q "review_tool: reasonix" "$deepseek_review" \
    || fail "DeepSeek review metadata missing tool"
grep -q "reasonix args: \\[run\\] \\[--model\\] \\[deepseek-pro\\]" "$deepseek_review" \
    || fail "DeepSeek command did not use reasonix run with deepseek-pro"
grep -q "reasonix prompt contains target: yes" "$deepseek_review" \
    || fail "DeepSeek prompt did not include target"
grep -q "reasonix prompt contains review kind: yes" "$deepseek_review" \
    || fail "DeepSeek prompt did not include resolved review kind"
grep -q "reasonix prompt contains deep passes: yes" "$deepseek_review" \
    || fail "DeepSeek prompt did not include deep review passes"
grep -q "reasonix prompt contains extra context: yes" "$deepseek_review" \
    || fail "DeepSeek prompt did not include extra context"

grep -q "model: Gemini 3.5 Flash (High)" "$gemini_review" \
    || fail "Gemini review metadata missing model"
grep -q "review_kind: plan" "$gemini_review" \
    || fail "Gemini review metadata missing resolved review kind"
grep -q "review_tool: agy" "$gemini_review" \
    || fail "Gemini review metadata missing tool"
grep -q "agy args: \\[--model\\] \\[Gemini 3.5 Flash (High)\\] \\[--print-timeout\\] \\[90m\\] \\[--print\\]" "$gemini_review" \
    || fail "Gemini command did not use agy print with Gemini high model"
grep -q "agy prompt contains target: yes" "$gemini_review" \
    || fail "Gemini prompt did not include target"
grep -q "agy prompt contains review kind: yes" "$gemini_review" \
    || fail "Gemini prompt did not include resolved review kind"
grep -q "agy prompt contains deep passes: yes" "$gemini_review" \
    || fail "Gemini prompt did not include deep review passes"
grep -q "agy prompt contains extra context: yes" "$gemini_review" \
    || fail "Gemini prompt did not include extra context"

PATH="$bin_dir:$PATH" "$script" "$target" --out-dir "$out_dir" --only deepseek >"$tmp/deepseek-only.out"
grep -q "DeepSeek review:" "$tmp/deepseek-only.out" \
    || fail "deepseek-only mode did not run DeepSeek"
if grep -q "Gemini review:" "$tmp/deepseek-only.out"; then
    fail "deepseek-only mode unexpectedly ran Gemini"
fi

dry_out_dir="$tmp/dry-reviews"
PATH="$bin_dir:$PATH" "$script" "$target" --out-dir "$dry_out_dir" --dry-run >"$tmp/dry-run.out"
grep -q "DRY RUN" "$tmp/dry-run.out" \
    || fail "dry-run output did not identify itself"
grep -q "review kind: plan" "$tmp/dry-run.out" \
    || fail "dry-run output did not show resolved review kind"
grep -q "reasonix run --model deepseek-pro" "$tmp/dry-run.out" \
    || fail "dry-run output did not show DeepSeek command"
grep -q "agy --model 'Gemini 3.5 Flash (High)' --print-timeout 90m --print" "$tmp/dry-run.out" \
    || fail "dry-run output did not show Gemini command"
if [[ -d "$dry_out_dir" ]] && find "$dry_out_dir" -type f | grep -q .; then
    fail "dry-run unexpectedly wrote review files"
fi

PATH="$bin_dir:$PATH" "$script" "$design_target" --review-kind design --only deepseek --out-dir "$out_dir" >"$tmp/design.out"
design_review="$(find "$out_dir" -type f -name '*m4-wrapper-design-deepseek-pro.md' -print -quit)"
[[ -n "$design_review" ]] || fail "design review file was not created"
grep -q "review_kind: design" "$design_review" \
    || fail "explicit design review kind was not recorded"

echo "Agentic plan review wrapper fixtures passed."
