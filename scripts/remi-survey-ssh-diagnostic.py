#!/usr/bin/env python3
# scripts/remi-survey-ssh-diagnostic.py
"""Produce a typed, connection-redacted survey SSH diagnostic."""

import argparse
import json
import os
from pathlib import Path
import re
import shlex


TOKENS = ("<ssh-target>", "<ssh-user>", "<ssh-host>", "<ssh-alias>", "<ip>", "<redacted-path>")
IP_LITERAL = re.compile(
    r"(?<![A-Za-z0-9_])(?:[0-9A-Fa-f]*:){2,}[0-9A-Fa-f:.]*(?:%[A-Za-z0-9_.-]+)?"
    r"|(?<![0-9.])(?:[0-9]{1,3}\.){3}[0-9]{1,3}(?![0-9.])"
)


def withheld(reason: str) -> dict:
    return {"schema_version": 1, "outcome": "withheld", "reason": reason}


def read_private(path: Path) -> str:
    with path.open("rb") as stream:
        data = stream.read(1024 * 1024 + 1)
    if len(data) > 1024 * 1024:
        raise ValueError("private input exceeds diagnostic bound")
    return data.decode("utf-8", errors="strict")


def connection_identifiers(target: str, config: str, known_hosts: str) -> dict[str, str]:
    identifiers = {}

    def add(value: str, token: str) -> None:
        if not value or re.search(r"[\s*?!<>]", value):
            raise ValueError("connection identity is not literal")
        identifiers.setdefault(value.casefold(), token)

    user, separator, host = target.partition("@")
    if not separator or "@" in host:
        raise ValueError("connection target is not user@host")
    add(target, "<ssh-target>")
    add(user, "<ssh-user>")
    add(host, "<ssh-host>")
    # Brackets/ports in known_hosts are transport syntax, not part of the host.
    def add_host(value: str, token: str) -> None:
        add(value, token)
        if value.startswith("["):
            match = re.fullmatch(r"\[([^\]]+)\](?::[0-9]+)?", value)
            if not match:
                raise ValueError("malformed bracketed host")
            add(match[1], token)

    add_host(host, "<ssh-host>")
    found_alias = False
    for line in config.splitlines():
        fields = shlex.split(line, comments=True)
        if not fields:
            continue
        directive = fields[0].casefold()
        if directive in {"host", "hostname", "hostkeyalias", "user"}:
            if len(fields) < 2:
                raise ValueError("missing connection identity")
            for value in fields[1:]:
                add_host(value, "<ssh-user>" if directive == "user" else "<ssh-alias>")
            found_alias |= directive == "host"
    if not found_alias:
        raise ValueError("missing SSH Host binding")
    found_pin = False
    for line in known_hosts.splitlines():
        fields = line.split()
        if not fields or fields[0].startswith("#"):
            continue
        if fields[0].startswith("@"):
            fields = fields[1:]
        if len(fields) < 3:
            raise ValueError("malformed known_hosts entry")
        found_pin = True
        for value in fields[0].split(","):
            # Hashed host names cannot be emitted by SSH as resolved host names;
            # their literal hashes are still private identifiers to redact.
            add_host(value, "<ssh-host>")
    if not found_pin:
        raise ValueError("missing known_hosts binding")
    return identifiers


def remaining_identifier(message: str, identifiers: dict[str, str]) -> bool:
    # Typed tokens contain no source bytes and are not connection identifiers.
    for token in TOKENS:
        message = message.replace(token, "")
    return any(value in message.casefold() for value in identifiers) or bool(IP_LITERAL.search(message))


def public_diagnostic(message: str, identifiers: dict[str, str]) -> dict:
    if remaining_identifier(message, identifiers):
        return withheld("connection_identifier_remaining")
    message = next((line for line in message.splitlines() if line.strip()), "helper returned missing or malformed survey evidence")
    return {"schema_version": 1, "outcome": "sanitized", "message": message}


def sanitize(stderr: str, identifiers: dict[str, str]) -> dict:
    # Reject terminal escapes and other control encodings rather than publishing
    # an obfuscated identifier or terminal control sequence.
    if any(ord(char) < 32 and char not in "\n\r\t" for char in stderr):
        return withheld("unsupported_diagnostic_encoding")
    pattern = re.compile("|".join(re.escape(value) for value in sorted(identifiers, key=len, reverse=True)), re.IGNORECASE)
    message = pattern.sub(lambda match: identifiers[match[0].casefold()], stderr)
    message = IP_LITERAL.sub("<ip>", message)
    message = re.sub(r"/[^\s]+", "<redacted-path>", message)
    return public_diagnostic(message, identifiers)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--stderr", type=Path, required=True)
    parser.add_argument("--ssh-config", type=Path, required=True)
    parser.add_argument("--known-hosts", type=Path, required=True)
    args = parser.parse_args()
    try:
        identifiers = connection_identifiers(
            os.environ["REMI_SSH_TARGET"], read_private(args.ssh_config), read_private(args.known_hosts)
        )
        result = sanitize(read_private(args.stderr), identifiers)
    except (OSError, ValueError, KeyError):
        # Exception text may itself contain private connection metadata.
        result = withheld("connection_metadata_or_diagnostic_invalid")
    print(json.dumps(result, sort_keys=True))


if __name__ == "__main__":
    main()
