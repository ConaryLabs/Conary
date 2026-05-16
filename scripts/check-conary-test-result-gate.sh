#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 ]]; then
  echo "Usage: $0 <result.json> [<result.json> ...]" >&2
  exit 64
fi

for result in "$@"; do
  if [[ ! -f "$result" ]]; then
    echo "missing result file: $result" >&2
    exit 1
  fi

  python3 - "$result" <<'PY'
import json
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
data = json.loads(path.read_text())
summary = data.get("summary", {})
results = data.get("results", [])

failed = int(summary.get("failed", 0))
skipped = int(summary.get("skipped", 0))
cancelled = int(summary.get("cancelled", 0))

if "cancelled" not in summary:
    cancelled = sum(1 for item in results if item.get("status") == "cancelled")

bad = []
if failed:
    bad.append(f"failed={failed}")
if skipped:
    bad.append(f"skipped={skipped}")
if cancelled:
    bad.append(f"cancelled={cancelled}")

if bad:
    print(f"{path}: release gate failed: {', '.join(bad)}", file=sys.stderr)
    for item in results:
        if item.get("status") in {"failed", "skipped", "cancelled"}:
            print(
                f"  {item.get('id', '<unknown>')} {item.get('name', '<unknown>')}: "
                f"{item.get('status')} {item.get('message', '')}",
                file=sys.stderr,
            )
    sys.exit(1)

print(f"{path}: ok")
PY
done
