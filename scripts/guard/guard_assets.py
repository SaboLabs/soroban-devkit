#!/usr/bin/env python3
"""Guard A - tracked font/asset integrity.

Design:
  * enumerate tracked files safely (git ls-files -z) so filenames with spaces work
  * per-format signatures, none assumed at offset zero except where the spec says so
    (EOT carries its magic at offset 34, not 0)
  * SVG validated by XML parse + root element name, never by a 5-byte prefix

LIMITATIONS (documented on purpose):
  a valid signature is NOT proof that a file is safe, complete, or even a usable
  font. This guard only detects gross format mismatch, e.g. JavaScript stored as
  ".woff2". It performs no execution, no decompression and no content decoding.

Usage:
  guard_assets.py [--root DIR] [--paths PATH ...] [--ext .woff2 ...]
Exit codes: 0 = all checked assets look structurally consistent, 1 = at least one failure.
"""
from __future__ import annotations

import argparse
import os
import struct
import subprocess
import sys
import xml.etree.ElementTree as ET

SUPPORTED = {".woff2", ".woff", ".ttf", ".otf", ".eot", ".svg"}
WOFF2_MAGIC = b"wOF2"
WOFF_MAGIC = b"wOFF"
TTF_MAGIC = b"\x00\x01\x00\x00"
TTF_ACCEPTED = (TTF_MAGIC, b"true", b"ttcf")
OTF_MAGIC = b"OTTO"
EOT_MAGIC_OFFSET = 34
EOT_SIZE_FIELD = 0
EOT_MAGIC = (b"\x4c\x50", b"\x4d\x46")  # 0x504C ("LP"); 0x4D46 accepted as legacy
SVG_NS = "http://www.w3.org/2000/svg"


def tracked_paths(root: str, exts: set[str]) -> list[str]:
    """Tracked files under *root* whose extension is in *exts*.

    -z keeps NUL-delimited output so paths containing spaces/newlines survive.

    Test fixtures are excluded: tests/guard/fixtures deliberately contains
    invalid assets (broken SVG, truncated EOT, JavaScript-shaped headers) that
    must be rejected when passed explicitly via --paths, but which must not
    fail the production asset guard when they are merely tracked.
    """
    out = subprocess.run(
        ["git", "-C", root, "ls-files", "-z"],
        capture_output=True, check=True,
    ).stdout
    result = []
    for raw in out.split(b"\x00"):
        if not raw:
            continue
        path = os.fsdecode(raw)
        if path.startswith("tests/guard/fixtures/"):
            continue
        if os.path.splitext(path)[1].lower() in exts:
            result.append(path)
    return result


def check_svg(path: str) -> str | None:
    try:
        root = ET.parse(path).getroot()
    except ET.ParseError as exc:
        return f"SVG is not well-formed XML: {exc}"
    tag = root.tag
    if tag.startswith("{"):
        ns, _, local = tag[1:].partition("}")
        if ns != SVG_NS:
            return f"SVG root namespace {ns!r} != {SVG_NS!r}"
        tag = local
    if tag.lower() != "svg":
        return f"SVG root element is <{tag}>, expected <svg>"
    return None


def check_file(path: str) -> str | None:
    ext = os.path.splitext(path)[1].lower()
    try:
        size = os.path.getsize(path)
    except OSError as exc:
        return f"cannot stat file: {exc}"

    if ext == ".svg":
        return check_svg(path)

    if size < 4:
        return f"file too small ({size}B) to hold a signature"

    with open(path, "rb") as fh:
        head = fh.read(36)

    if ext == ".woff2":
        if head[:4] != WOFF2_MAGIC:
            return f"bad woff2 signature {head[:4]!r}, expected {WOFF2_MAGIC!r}"
        return None

    if ext == ".woff":
        if head[:4] != WOFF_MAGIC:
            return f"bad woff signature {head[:4]!r}, expected {WOFF_MAGIC!r}"
        return None

    if ext == ".ttf":
        if head[:4] not in TTF_ACCEPTED:
            return f"bad ttf signature {head[:4]!r}, expected 00010000/'true'/'ttcf'"
        return None

    if ext == ".otf":
        if head[:4] in (OTF_MAGIC, TTF_MAGIC):
            return None
        return f"bad otf signature {head[:4]!r}, expected {OTF_MAGIC!r}"

    if ext == ".eot":
        if size < 36:
            return f"EOT too small ({size}B < 36B header)"
        declared = struct.unpack("<I", head[EOT_SIZE_FIELD:EOT_SIZE_FIELD + 4])[0]
        if declared != size:
            return f"EOT size field {declared} != actual file size {size}"
        magic = head[EOT_MAGIC_OFFSET:EOT_MAGIC_OFFSET + 2]
        if magic not in EOT_MAGIC:
            return (
                f"bad EOT magic {magic.hex()} at offset {EOT_MAGIC_OFFSET}, "
                f"expected 4c50"
            )
        return None

    return f"unsupported extension {ext!r}"


def main() -> int:
    parser = argparse.ArgumentParser(description="tracked font/asset integrity guard")
    parser.add_argument("--root", default=".", help="repository root for git ls-files")
    parser.add_argument("--paths", nargs="*", default=None,
                        help="explicit paths to check (fixture mode); skips git")
    parser.add_argument("--ext", nargs="*", default=None,
                        help="extensions to scan when enumerating tracked files")
    args = parser.parse_args()

    exts = {e.lower() if e.startswith(".") else "." + e.lower()
            for e in (args.ext or SUPPORTED)}

    if args.paths is not None:
        paths = list(args.paths)
    else:
        try:
            paths = tracked_paths(args.root, exts)
        except subprocess.CalledProcessError as exc:
            print(f"::error::could not enumerate tracked files: {exc}", file=sys.stderr)
            return 1

    failures = 0
    for path in sorted(paths):
        full = path if os.path.isabs(path) else os.path.join(args.root, path)
        if not os.path.isfile(full):
            print(f"::error file={path}::tracked asset is missing from the working tree")
            failures += 1
            continue
        problem = check_file(full)
        if problem:
            print(f"::error file={path}::{problem}")
            failures += 1
        else:
            print(f"ok    {path}")

    print(f"checked {len(paths)} asset(s), {failures} failure(s)")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
