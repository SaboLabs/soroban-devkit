#!/usr/bin/env python3
"""Generate synthetic fixtures for the guardrail prototypes.

Every fixture is inert: fonts are header stubs padded with 0x00 (no executable
content, no JavaScript, no payload bytes copied from the incident).
"""
from __future__ import annotations

import os
import struct

HERE = os.path.dirname(os.path.abspath(__file__))
PAD = b"\x00" * 48


def write(rel: str, data: bytes) -> None:
    path = os.path.join(HERE, rel)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as fh:
        fh.write(data)


def eot_stub(extra: int = 16) -> bytes:
    total = 36 + extra
    body = struct.pack("<I", total) + b"\x00" * 30 + b"\x4c\x50"
    return body + b"\x00" * extra


# ---- valid, one per supported format -----------------------------------------
write("fonts/valid/fa-valid.woff2", b"wOF2" + PAD)
write("fonts/valid/fa-valid.woff", b"wOFF" + PAD)
write("fonts/valid/fa-valid.ttf", b"\x00\x01\x00\x00" + PAD)
write("fonts/valid/fa-valid.otf", b"OTTO" + PAD)
write("fonts/valid/fa-valid.eot", eot_stub())
write(
    "fonts/valid/fa-valid.svg",
    b'<?xml version="1.0" encoding="UTF-8"?>\n'
    b'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10">'
    b'<rect width="10" height="10"/></svg>\n',
)
# filename containing spaces -> proves enumeration is space-safe
write("fonts/valid/fa solid 500 copy.woff2", b"wOF2" + PAD)

# ---- invalid: non-font content wearing a font extension ----------------------
NOT_FONT = b"THIS IS NOT A FONT FILE - synthetic inert text marker\n"
write("fonts/invalid/not-a-font.woff2", NOT_FONT)
write("fonts/invalid/not-a-font.woff", NOT_FONT)
write("fonts/invalid/not-a-font.otf", NOT_FONT)
write("fonts/invalid/gzip-as.ttf", b"\x1f\x8b\x08\x00" + PAD)          # gzip magic
write("fonts/invalid/truncated.eot", b"\x4c\x50" + b"\x00" * 4)          # < 36B header
write("fonts/invalid/size-lies.eot", struct.pack("<I", 999) + b"\x00" * 30 + b"\x4c\x50" + PAD)
write("fonts/invalid/not-svg.svg", b"<html><body>not svg</body></html>\n")
write("fonts/invalid/broken.svg", b'<svg xmlns="http://www.w3.org/2000/svg"><rect>\n')
write("fonts/invalid/empty.woff2", b"")

print("fixtures written under", HERE)
