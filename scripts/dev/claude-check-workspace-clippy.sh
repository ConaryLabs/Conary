#!/bin/bash
# Claude hook helper: run workspace clippy after Rust file edits.
# This script is intentionally Claude-protocol-coupled. It expects hook JSON on
# stdin, emits hookSpecificOutput JSON on stdout, and exits 2 when it has
# additional context for the editor session.

INPUT=$(cat)
FILE_PATH=$(echo "$INPUT" | jq -r '.tool_input.file_path // empty')

[[ "$FILE_PATH" != *.rs ]] && exit 0

CLIPPY_OUTPUT=$(cargo clippy --workspace --all-targets -- -D warnings 2>&1)
CLIPPY_EXIT=$?

if [ $CLIPPY_EXIT -ne 0 ]; then
    ERRORS=$(echo "$CLIPPY_OUTPUT" | grep -E "^(error|warning)" | head -10)
    jq -n --arg errors "$ERRORS" '{
        "hookSpecificOutput": {
            "hookEventName": "PostToolUse",
            "additionalContext": ("Clippy found issues:\n" + $errors)
        }
    }'
    exit 2
fi

exit 0
