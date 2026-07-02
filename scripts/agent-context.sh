#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

usage() {
    cat >&2 <<'EOF'
Usage: scripts/agent-context.sh <mode> [options]

Print feature-card task context from docs/modules/feature-ownership.md.

Modes (exactly one):
  --feature <slug>        Print the task packet for one card.
  --path <path>           Route one repo path to its owning card; print packet.
  --changed               Route all changed paths; print brief hints per path.
  --list                  Print slug + capability summary for all cards.
  --validate              Validate the map schema; non-zero exit on violation.

Options:
  --base <ref>            With --changed: diff base. Defaults to HEAD.
  --all                   With --changed: route all tracked files instead of
                          changed, cached, and untracked paths.
  --brief                 With --feature/--path: one-line summary instead of
                          full packet (drift-report format).
  --run <focused|gate>    With --feature: execute the extracted proof
                          commands sequentially, fail-fast, echoing each.
  --map <path>            Map file override (for tests). Defaults to
                          docs/modules/feature-ownership.md.
  -h, --help              Show this help.
EOF
}

fail() {
    echo "ERROR: $*" >&2
    exit 1
}

mode=""
feature_slug=""
route_path_arg=""
base_ref="HEAD"
base_ref_set=0
scan_all=0
brief=0
run_kind=""
map_file="docs/modules/feature-ownership.md"

set_mode() {
    if [[ -n "$mode" ]]; then
        usage
        exit 2
    fi
    mode="$1"
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --feature)
            [[ $# -ge 2 ]] || { usage; exit 2; }
            set_mode feature
            feature_slug="$2"
            shift 2
            ;;
        --path)
            [[ $# -ge 2 ]] || { usage; exit 2; }
            set_mode path
            route_path_arg="$2"
            shift 2
            ;;
        --changed)
            set_mode changed
            shift
            ;;
        --list)
            set_mode list
            shift
            ;;
        --validate)
            set_mode validate
            shift
            ;;
        --base)
            [[ $# -ge 2 ]] || { usage; exit 2; }
            base_ref="$2"
            base_ref_set=1
            shift 2
            ;;
        --all)
            scan_all=1
            shift
            ;;
        --brief)
            brief=1
            shift
            ;;
        --run)
            [[ $# -ge 2 ]] || { usage; exit 2; }
            run_kind="$2"
            shift 2
            ;;
        --map)
            [[ $# -ge 2 ]] || { usage; exit 2; }
            map_file="$2"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            usage
            exit 2
            ;;
    esac
done

if [[ -z "$mode" ]]; then
    usage
    exit 2
fi

if [[ "$scan_all" -eq 1 || "$base_ref_set" -eq 1 ]]; then
    if [[ "$mode" != "changed" ]]; then
        usage
        exit 2
    fi
fi

if [[ "$brief" -eq 1 ]]; then
    case "$mode" in
        feature|path) ;;
        *)
            usage
            exit 2
            ;;
    esac
fi

if [[ -n "$run_kind" ]]; then
    if [[ "$mode" != "feature" || "$brief" -eq 1 ]]; then
        usage
        exit 2
    fi
    case "$run_kind" in
        focused|gate) ;;
        *)
            fail "invalid --run kind: $run_kind (expected focused or gate)"
            ;;
    esac
fi

[[ -f "$map_file" ]] || fail "map file not found: $map_file"

declare -a card_headings=()
declare -A card_fields=()

load_map() {
    local kind a b c
    while IFS=$'\037' read -r kind a b c; do
        case "$kind" in
            C)
                card_headings+=("$a")
                ;;
            F)
                card_fields["$a|$b"]="$c"
                ;;
        esac
    done < <(awk '
        function flush_field() {
            if (card != "" && field != "") {
                gsub(/[ \t]+/, " ", value)
                sub(/^ /, "", value)
                sub(/ $/, "", value)
                printf "F\037%s\037%s\037%s\n", card, field, value
            }
            field = ""
            value = ""
        }
        /^## / {
            flush_field()
            heading = substr($0, 4)
            if (heading == "How To Use This Map" || heading == "Card Schema") {
                card = ""
                next
            }
            card = heading
            printf "C\037%s\n", card
            next
        }
        card == "" { next }
        /^[ \t]*$/ { flush_field(); next }
        /^\*\*[A-Za-z ]+:\*\*/ {
            flush_field()
            match($0, /^\*\*[A-Za-z ]+:\*\*/)
            field = substr($0, 3, RLENGTH - 5)
            value = substr($0, RLENGTH + 1)
            next
        }
        {
            if (field != "") {
                value = value " " $0
            }
        }
        END { flush_field() }
    ' "$map_file")
}

extract_spans() {
    grep -o '`[^`]*`' <<< "$1" | sed 's/^`//; s/`$//' || true
}

strip_backticks() {
    tr -d '`' <<< "$1"
}

print_entries() {
    local value="$1" entry
    local -a entries=()
    local IFS=';'
    read -ra entries <<< "$value"
    for entry in "${entries[@]}"; do
        entry="${entry#"${entry%%[![:space:]]*}"}"
        entry="${entry%"${entry##*[![:space:]]}"}"
        if [[ -n "$entry" ]]; then
            printf '%s\n' "$entry"
        fi
    done
}

print_commands_backticked() {
    local cmd
    while IFS= read -r cmd; do
        if [[ -n "$cmd" ]]; then
            printf '`%s`\n' "$cmd"
        fi
    done < <(extract_spans "$1")
}

gate_when_prose() {
    local prose
    prose="$(sed 's/`[^`]*`//g; s/;/ /g' <<< "$1" | tr -s ' ' | sed 's/^ //; s/ $//')"
    prose="${prose#when }"
    printf '%s\n' "$prose"
}

heading_for_slug() {
    local slug="$1" heading
    for heading in "${card_headings[@]}"; do
        if [[ "${card_fields["$heading|Slug"]:-}" == "$slug" ]]; then
            printf '%s\n' "$heading"
            return 0
        fi
    done
    return 1
}

print_packet() {
    local heading="$1" gate when
    gate="${card_fields["$heading|Interaction gate"]:-}"
    printf '# Task Packet: %s\n' "$heading"
    printf 'slug: %s\n' "${card_fields["$heading|Slug"]:-}"
    printf 'capability: %s\n' "${card_fields["$heading|Capability"]:-}"
    printf '\n## Read first\n'
    print_entries "${card_fields["$heading|Start here"]:-}"
    printf '\n## Paths owned\n'
    print_entries "${card_fields["$heading|Paths"]:-}"
    printf '\n## Neighbor systems\n'
    printf '%s\n' "${card_fields["$heading|Neighbor systems"]:-}"
    printf '\n## Focused proof\n'
    print_commands_backticked "${card_fields["$heading|Focused proof"]:-}"
    printf '\n## Interaction gate\n'
    print_commands_backticked "$gate"
    when="$(gate_when_prose "$gate")"
    if [[ -n "$when" ]]; then
        printf 'when: %s\n' "$when"
    fi
    printf '\n## Docs to update\n'
    print_entries "${card_fields["$heading|Docs to update"]:-}"
    printf '\n## Safety invariants\n'
    printf '%s\n' "${card_fields["$heading|Safety notes"]:-}"
}

brief_line() {
    local heading="$1"
    printf '%s | focused: %s | gate: %s' \
        "$heading" \
        "$(strip_backticks "${card_fields["$heading|Focused proof"]:-}")" \
        "$(strip_backticks "${card_fields["$heading|Interaction gate"]:-}")"
}

mode_list() {
    local heading
    for heading in "${card_headings[@]}"; do
        printf '%s\t%s\n' \
            "${card_fields["$heading|Slug"]:-}" \
            "${card_fields["$heading|Capability"]:-}"
    done
}

declare -a glob_patterns=()
declare -a glob_headings=()
declare -a glob_specificities=()

load_globs() {
    local heading glob prefix
    for heading in "${card_headings[@]}"; do
        while IFS= read -r glob; do
            if [[ -z "$glob" ]]; then
                continue
            fi
            glob_patterns+=("$glob")
            glob_headings+=("$heading")
            prefix="${glob%%[\*\?\[]*}"
            glob_specificities+=("${#prefix}")
        done < <(extract_spans "${card_fields["$heading|Paths"]:-}")
    done
}

route_heading=""
declare -a route_tied=()

route_path() {
    local path="$1"
    local i best=-1
    route_heading=""
    route_tied=()
    for i in "${!glob_patterns[@]}"; do
        # The glob must stay unquoted so [[ == ]] treats it as a pattern.
        if [[ "$path" == ${glob_patterns[$i]} ]]; then
            if (( glob_specificities[i] > best )); then
                best="${glob_specificities[$i]}"
                route_heading="${glob_headings[$i]}"
                route_tied=("${glob_headings[$i]}")
            elif (( glob_specificities[i] == best )); then
                route_tied+=("${glob_headings[$i]}")
            fi
        fi
    done
    [[ -n "$route_heading" ]]
}

distinct_tied_count() {
    printf '%s\n' "${route_tied[@]}" | sort -u | wc -l
}

require_unambiguous_route() {
    local path="$1"
    if (( $(distinct_tied_count) > 1 )); then
        fail "ambiguous Paths routing for $path: $(printf '%s\n' "${route_tied[@]}" | sort -u | paste -sd ';' -) (run --validate and fix the map)"
    fi
}

fallback_hint_for_path() {
    local path="$1"
    case "$path" in
        AGENTS.md|CONTRIBUTING.md|.github/PULL_REQUEST_TEMPLATE.md|docs/llms/*|docs/modules/feature-ownership.md|docs/superpowers/documentation-accuracy-audit-*|scripts/maintainability-drift-report.sh|scripts/agent-context.sh)
            printf 'Assistant/contributor guidance | focused: bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete | gate: docs-audit inventory diff and stale-term added-line sweep'
            ;;
        docs/modules/*|docs/operations/*|docs/INTEGRATION-TESTING.md|docs/ARCHITECTURE.md)
            printf 'Canonical docs | focused: docs-audit ledger and inventory checks | gate: affected feature card proof if behavior claims changed'
            ;;
        docs/superpowers/plans/*|docs/superpowers/specs/*)
            printf 'Planning docs | focused: docs-audit ledger and inventory checks | gate: agentic review before lock-in'
            ;;
        *)
            return 1
            ;;
    esac
}

no_hint_message='No feature-card hint matched. Use the owning package tests and update docs/modules/feature-ownership.md if this should be routed.'

mode_path() {
    local hint
    if route_path "$route_path_arg"; then
        require_unambiguous_route "$route_path_arg"
        if [[ "$brief" -eq 1 ]]; then
            printf '%s\n' "$(brief_line "$route_heading")"
        else
            print_packet "$route_heading"
        fi
    elif hint="$(fallback_hint_for_path "$route_path_arg")"; then
        printf '%s\n' "$hint"
    else
        printf '%s\n' "$no_hint_message"
    fi
}

collect_paths() {
    if [[ "$scan_all" -eq 1 ]]; then
        git ls-files
        return
    fi

    {
        git diff --name-only "$base_ref" --
        git diff --cached --name-only --
        git ls-files --others --exclude-standard
    } | awk 'NF' | sort -u
}

mode_changed() {
    local p hint
    local -a changed=()
    mapfile -t changed < <(collect_paths)

    if [[ "${#changed[@]}" -eq 0 ]]; then
        printf '[ok] no changed paths detected\n'
        return
    fi

    printf 'changed_paths: %s\n' "${#changed[@]}"
    for p in "${changed[@]}"; do
        if route_path "$p"; then
            require_unambiguous_route "$p"
            printf -- '- %s\n  %s\n' "$p" "$(brief_line "$route_heading")"
        elif hint="$(fallback_hint_for_path "$p")"; then
            printf -- '- %s\n  %s\n' "$p" "$hint"
        else
            printf -- '- %s\n  %s\n' "$p" "$no_hint_message"
        fi
    done
}

required_fields=("Slug" "Capability" "Start here" "Neighbor systems" "Paths" "Focused proof" "Interaction gate" "Docs to update" "Safety notes")

validation_errors=0

validate_err() {
    printf 'INVALID: %s\n' "$*" >&2
    validation_errors=$((validation_errors + 1))
}

is_repo_path_span() {
    local span="$1"
    [[ "$span" =~ ^[A-Za-z0-9][A-Za-z0-9._/-]*$ ]] || return 1
    if [[ "$span" == */* ]]; then
        return 0
    fi
    [[ "$span" == *.md ]]
}

span_exists_tracked() {
    local span="${1%/}"
    if git ls-files --error-unmatch -- "$span" >/dev/null 2>&1; then
        return 0
    fi
    [[ -n "$(git ls-files -- "$span/" | head -n 1)" ]]
}

mode_validate() {
    local heading field slug span i t matched
    local -a tracked_files=()
    declare -A seen_slugs=()

    if [[ "${#card_headings[@]}" -eq 0 ]]; then
        fail "no ownership cards parsed from $map_file"
    fi

    for heading in "${card_headings[@]}"; do
        for field in "${required_fields[@]}"; do
            if [[ -z "${card_fields["$heading|$field"]:-}" ]]; then
                validate_err "card '$heading' is missing field: $field"
            fi
        done

        slug="${card_fields["$heading|Slug"]:-}"
        if [[ -n "$slug" ]]; then
            if [[ ! "$slug" =~ ^[a-z0-9]+(-[a-z0-9]+)*$ ]]; then
                validate_err "card '$heading' slug is not kebab-case: $slug"
            fi
            if [[ -n "${seen_slugs["$slug"]:-}" ]]; then
                validate_err "card '$heading' duplicates slug: $slug"
            fi
            seen_slugs["$slug"]=1
        fi

        if [[ -n "${card_fields["$heading|Focused proof"]:-}" ]] \
            && [[ -z "$(extract_spans "${card_fields["$heading|Focused proof"]}")" ]]; then
            validate_err "card '$heading' Focused proof has no backticked command"
        fi

        if [[ -n "${card_fields["$heading|Paths"]:-}" ]] \
            && [[ -z "$(extract_spans "${card_fields["$heading|Paths"]}")" ]]; then
            validate_err "card '$heading' Paths has no backticked glob"
        fi

        for field in "Start here" "Docs to update"; do
            while IFS= read -r span; do
                if [[ -n "$span" ]] && is_repo_path_span "$span" && ! span_exists_tracked "$span"; then
                    validate_err "card '$heading' $field references untracked path: $span"
                fi
            done < <(extract_spans "${card_fields["$heading|$field"]:-}")
        done
    done

    mapfile -t tracked_files < <(git ls-files)

    for i in "${!glob_patterns[@]}"; do
        matched=0
        for t in "${tracked_files[@]}"; do
            # The glob must stay unquoted so [[ == ]] treats it as a pattern.
            if [[ "$t" == ${glob_patterns[$i]} ]]; then
                matched=1
                break
            fi
        done
        if [[ "$matched" -eq 0 ]]; then
            validate_err "card '${glob_headings[$i]}' has dead Paths glob: ${glob_patterns[$i]}"
        fi
    done

    for t in "${tracked_files[@]}"; do
        if route_path "$t"; then
            if (( $(distinct_tied_count) > 1 )); then
                validate_err "equal-specificity Paths overlap for $t: $(printf '%s\n' "${route_tied[@]}" | sort -u | paste -sd ';' -)"
            fi
        fi
    done

    if (( validation_errors > 0 )); then
        fail "feature ownership map validation failed with $validation_errors problem(s): $map_file"
    fi
    printf 'Feature ownership map validation passed (%s cards).\n' "${#card_headings[@]}"
}

mode_run() {
    local heading="$1" field cmd
    local -a cmds=()

    case "$run_kind" in
        focused) field="Focused proof" ;;
        gate) field="Interaction gate" ;;
    esac

    mapfile -t cmds < <(extract_spans "${card_fields["$heading|$field"]:-}")
    if [[ "${#cmds[@]}" -eq 0 ]]; then
        fail "card '$heading' has no $field commands to run"
    fi

    for cmd in "${cmds[@]}"; do
        printf '+ %s\n' "$cmd"
        bash -lc "$cmd" || fail "command failed: $cmd"
    done
    printf 'All %s %s command(s) passed for %s.\n' "${#cmds[@]}" "$run_kind" "$heading"
}

load_map
load_globs

case "$mode" in
    list)
        mode_list
        ;;
    feature)
        heading="$(heading_for_slug "$feature_slug")" \
            || fail "unknown feature slug: $feature_slug (list slugs with --list)"
        if [[ -n "$run_kind" ]]; then
            mode_run "$heading"
        elif [[ "$brief" -eq 1 ]]; then
            printf '%s\n' "$(brief_line "$heading")"
        else
            print_packet "$heading"
        fi
        ;;
    path)
        mode_path
        ;;
    changed)
        if [[ "$scan_all" -eq 0 ]] \
            && ! git rev-parse --verify --quiet "$base_ref^{commit}" >/dev/null; then
            echo "ERROR: base ref not found: $base_ref" >&2
            exit 2
        fi
        mode_changed
        ;;
    validate)
        mode_validate
        ;;
esac
