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
from collections import Counter

path = pathlib.Path(sys.argv[1])
data = json.loads(path.read_text())
if not isinstance(data, dict):
    print(f"{path}: release gate failed: result JSON must be an object", file=sys.stderr)
    sys.exit(1)

summary = data.get("summary")
results = data.get("results")

if not isinstance(summary, dict):
    print(f"{path}: release gate failed: missing summary object", file=sys.stderr)
    sys.exit(1)
if not isinstance(results, list) or not results:
    print(f"{path}: release gate failed: results must be a non-empty array", file=sys.stderr)
    sys.exit(1)

required = ["total", "passed", "failed", "skipped", "cancelled"]
missing = [key for key in required if key not in summary]
if missing:
    print(
        f"{path}: release gate failed: summary missing {', '.join(missing)}",
        file=sys.stderr,
    )
    sys.exit(1)

counts = {}
for key in required:
    try:
        value = int(summary[key])
    except (TypeError, ValueError):
        print(f"{path}: release gate failed: summary.{key} is not an integer", file=sys.stderr)
        sys.exit(1)
    if value < 0:
        print(f"{path}: release gate failed: summary.{key} is negative", file=sys.stderr)
        sys.exit(1)
    counts[key] = value

if counts["total"] != len(results):
    print(
        f"{path}: release gate failed: summary.total={counts['total']} "
        f"but results has {len(results)} item(s)",
        file=sys.stderr,
    )
    sys.exit(1)

status_counts = Counter()
allowed_statuses = {"passed", "failed", "skipped", "cancelled"}
for item in results:
    if not isinstance(item, dict):
        print(f"{path}: release gate failed: result item must be an object", file=sys.stderr)
        sys.exit(1)
    status = item.get("status")
    if status not in allowed_statuses:
        print(f"{path}: release gate failed: invalid result status {status!r}", file=sys.stderr)
        sys.exit(1)
    status_counts[status] += 1

for key in ["passed", "failed", "skipped", "cancelled"]:
    if counts[key] != status_counts[key]:
        print(
            f"{path}: release gate failed: summary.{key}={counts[key]} "
            f"but results contain {status_counts[key]}",
            file=sys.stderr,
        )
        sys.exit(1)

failed = counts["failed"]
skipped = counts["skipped"]
cancelled = counts["cancelled"]

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
