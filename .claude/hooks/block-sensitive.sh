#!/bin/bash
# .claude/hooks/block-sensitive.sh
# PreToolUse hook: block edits to sensitive files

INPUT=$(cat)
FILE_PATH=$(echo "$INPUT" | jq -r '.tool_input.file_path // empty')

# Nothing to check if no file path
[ -z "$FILE_PATH" ] && exit 0

# Sensitive patterns that should never be auto-edited
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
