#!/bin/bash
# Claude hook helper: guard likely secret-bearing files before edits.
# This script is intentionally Claude-protocol-coupled. It expects hook JSON on
# stdin, emits hookSpecificOutput JSON on stdout, and exits 2 to ask for
# confirmation.

INPUT=$(cat)
FILE_PATH=$(echo "$INPUT" | jq -r '.tool_input.file_path // empty')

# Nothing to check if no file path was supplied by the hook payload.
[ -z "$FILE_PATH" ] && exit 0

BLOCKED_PATTERNS=(
    ".credentials.toml"
    ".env"
    "settings.json"
    "id_ed25519"
    "id_rsa"
    "remi_conary_io"
)

for pattern in "${BLOCKED_PATTERNS[@]}"; do
    if [[ "$FILE_PATH" == *"$pattern"* ]]; then
        echo "Protected file matches pattern: $pattern" >&2
        jq -n '{
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "ask",
                "permissionDecisionReason": "This file may contain secrets or sensitive configuration. Please confirm."
            }
        }'
        exit 2
    fi
done

exit 0
