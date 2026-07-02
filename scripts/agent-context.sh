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

load_map

case "$mode" in
    list)
        mode_list
        ;;
    feature)
        heading="$(heading_for_slug "$feature_slug")" \
            || fail "unknown feature slug: $feature_slug (list slugs with --list)"
        if [[ "$brief" -eq 1 ]]; then
            printf '%s\n' "$(brief_line "$heading")"
        else
            print_packet "$heading"
        fi
        ;;
    path|changed|validate)
        fail "mode not implemented yet: $mode"
        ;;
esac
