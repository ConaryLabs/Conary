#!/bin/bash
# .claude/hooks/post-edit-clippy.sh
# PostToolUse hook: run clippy after .rs file edits

INPUT=$(cat)
FILE_PATH=$(echo "$INPUT" | jq -r '.tool_input.file_path // empty')

# Only run on .rs files
[[ "$FILE_PATH" != *.rs ]] && exit 0

# Run clippy, capture output
CLIPPY_OUTPUT=$(cargo clippy --workspace --all-targets -- -D warnings 2>&1)
CLIPPY_EXIT=$?

if [ $CLIPPY_EXIT -ne 0 ]; then
    # Filter to just errors/warnings (skip compiling lines)
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
