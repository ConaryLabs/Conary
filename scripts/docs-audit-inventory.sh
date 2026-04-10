#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

fail() {
    echo "ERROR: $*" >&2
    exit 1
}

classify_family() {
    local path="$1"

    case "$path" in
        README.md|ROADMAP.md|CONTRIBUTING.md|SECURITY.md|CHANGELOG.md|AGENTS.md|CLAUDE.md)
            echo "root"
            ;;
        .github/*|*.example.md)
            echo "template"
            ;;
        deploy/*)
            echo "deploy"
            ;;
        site/README.md|web/README.md)
            echo "frontend"
            ;;
        docs/llms/archive/*|docs/superpowers/archive/*|docs/superpowers/plans/archive/*|docs/superpowers/specs/archive/*|recipes/archive/*)
            echo "historical"
            ;;
        docs/superpowers/plans/*|docs/superpowers/specs/*|docs/superpowers/*.md)
            echo "planning"
            ;;
        apps/*|bootstrap/stage0/README.md)
            echo "app-local"
            ;;
        docs/*)
            echo "canonical"
            ;;
        *)
            fail "unclassified documentation path: $path"
            ;;
    esac
}

classify_audience() {
    local path="$1"
    local family="$2"

    case "$family" in
        root)
            case "$path" in
                README.md|ROADMAP.md)
                    echo "user"
                    ;;
                *)
                    echo "contributor"
                    ;;
            esac
            ;;
        template)
            echo "contributor"
            ;;
        deploy)
            echo "operator"
            ;;
        app-local|canonical|frontend)
            echo "contributor"
            ;;
        planning)
            echo "maintainer"
            ;;
        historical)
            echo "historical"
            ;;
        *)
            fail "unclassified documentation audience for family=$family path=$path"
            ;;
    esac
}

printf 'path\tfamily\taudience\n'

while IFS= read -r path; do
    family="$(classify_family "$path")"
    audience="$(classify_audience "$path" "$family")"
    printf '%s\t%s\t%s\n' "$path" "$family" "$audience"
done < <(
    git ls-files \
        | rg '(^|/)(README\.md|AGENTS\.md|CONTRIBUTING\.md|ROADMAP\.md|CHANGELOG\.md|SECURITY\.md|CLAUDE\.md|.*\.md|.*\.mdx|.*\.rst|.*\.adoc)$' \
        | sort
)
