#!/usr/bin/env python3
"""Digest durable transaction authority while ignoring derived root snapshots."""

import base64
import hashlib
import json
import sqlite3
import sys
from pathlib import Path


DERIVED_SNAPSHOT_TABLES = frozenset(
    {
        "generation_db_mutation_epoch",
        "selected_root_snapshot_entries",
        "selected_root_snapshots",
        "sqlite_sequence",
    }
)


def quote_identifier(identifier: str) -> str:
    return '"' + identifier.replace('"', '""') + '"'


def encode_cell(value: object) -> list[str]:
    if value is None:
        return ["null", ""]
    if isinstance(value, bytes):
        return ["blob", base64.b64encode(value).decode("ascii")]
    if isinstance(value, float):
        return ["real", value.hex()]
    if isinstance(value, int):
        return ["integer", str(value)]
    if isinstance(value, str):
        return ["text", value]
    raise TypeError(f"unsupported SQLite value type: {type(value).__name__}")


def authority_digest(database: Path) -> str:
    digest = hashlib.sha256()
    with sqlite3.connect(f"file:{database}?mode=ro", uri=True) as connection:
        connection.execute("PRAGMA query_only = ON")
        tables = connection.execute(
            """
            SELECT name, sql
            FROM sqlite_schema
            WHERE type = 'table'
            ORDER BY name
            """
        ).fetchall()
        for table, schema in tables:
            if table in DERIVED_SNAPSHOT_TABLES or table.startswith("sqlite_"):
                continue
            digest.update(
                json.dumps(
                    ["table", table, schema],
                    ensure_ascii=False,
                    separators=(",", ":"),
                ).encode("utf-8")
            )
            query = f"SELECT * FROM {quote_identifier(table)}"
            rows = [
                json.dumps(
                    [encode_cell(cell) for cell in row],
                    ensure_ascii=False,
                    separators=(",", ":"),
                )
                for row in connection.execute(query)
            ]
            for row in sorted(rows):
                digest.update(row.encode("utf-8"))
    return digest.hexdigest()


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit("usage: database-authority-digest.py <database>")
    print(authority_digest(Path(sys.argv[1])))


if __name__ == "__main__":
    main()
