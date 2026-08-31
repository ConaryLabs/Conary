#!/usr/bin/env python3
# apps/conary/tests/fixtures/adversarial/repack-mgzip.py

"""Repack an uncompressed fixture tar with the current CCS MGZIP framing."""

import binascii
import os
from pathlib import Path
import struct
import sys
import zlib


BLOCK_BYTES = 1024 * 1024
HEADER_PREFIX = bytes(
    [0x1F, 0x8B, 8, 4, 0, 0, 0, 0, 0, 255, 8, 0, ord("I"), ord("G"), 4, 0]
)
HEADER_BYTES = 20
FOOTER_BYTES = 8


def encode_frame(block: bytes) -> bytes:
    compressor = zlib.compressobj(level=6, wbits=-zlib.MAX_WBITS)
    deflate = compressor.compress(block) + compressor.flush(zlib.Z_FINISH)
    frame_bytes = HEADER_BYTES + len(deflate) + FOOTER_BYTES
    header = HEADER_PREFIX + struct.pack("<I", frame_bytes)
    footer = struct.pack("<II", binascii.crc32(block) & 0xFFFFFFFF, len(block))
    return header + deflate + footer


def repack(source: Path, destination: Path) -> None:
    temporary = destination.with_name(f".{destination.name}.tmp-{os.getpid()}")
    blocks = 0
    try:
        with source.open("rb") as source_file, temporary.open("xb") as output_file:
            while block := source_file.read(BLOCK_BYTES):
                output_file.write(encode_frame(block))
                blocks += 1
            output_file.flush()
            os.fsync(output_file.fileno())
        if blocks == 0:
            raise ValueError("fixture tar is empty")
        os.replace(temporary, destination)
    finally:
        temporary.unlink(missing_ok=True)


def main() -> None:
    if len(sys.argv) != 3:
        raise SystemExit(f"usage: {sys.argv[0]} INPUT.tar OUTPUT.ccs")
    repack(Path(sys.argv[1]), Path(sys.argv[2]))


if __name__ == "__main__":
    main()
