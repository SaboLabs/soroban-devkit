"""Guard B-3 — npm package.json lifecycle hooks.

The historical incident included a launcher that ran a JavaScript payload disguised
as a font asset. One delivery vector for that class of attack is a *lifecycle hook*
(``preinstall`` / ``install`` / ``postinstall`` / ``prepare`` / ``prepublish`` /
``prepublishOnly``) that executes a binary/asset path or fetches code from the
network at install time.

Scope (explicit, intentionally narrow):
  * enumerates tracked ``package.json`` files via ``git ls-files -z`` (so paths
    with spaces/newlines survive) and excludes ``tests/guard/fixtures/`` exactly
    like ``guard_assets.py``
  * only the lifecycle keys listed above are inspected; ordinary
    ``scripts.build`` / ``scripts.test`` etc. are NOT inspected
  * signals checked inside a lifecycle hook:
    - reference to a binary/asset path (``dist/``, ``public/fonts/``, ``*.woff2``,
      ``*.ttf``, ``*.otf``, ``*.eot``, any ``node ./some.js`` outside ``scripts/``)
    - network fetch (``curl``, ``wget``, ``Invoke-WebRequest``, ``irm``)
    - obfuscation markers (``eval``, ``atob``, base64 blobs longer than 120 chars)
  * ALLOWED: manifest-driven bootstrap commands (``husky install``,
    ``patch-package``, ``npm run build``, ``tsc``, ``node-gyp``, ``lerna``,
    ``pinst``, ``corepack``). These are common legitimate lifecycle hooks and must
    not create noise.

False-positive policy (documented):
  * A lifecycle hook that only calls ``npm run <script>`` or a known tool passes.
  * A hook that is a plain ``echo`` / ``exit 0`` passes.
  * If NO tracked ``package.json`` exists (the current state of this repo), the
    guard prints ``inspected 0 file(s)`` and exits 0. This repo is pure Rust and
    has no npm manifest, so this guard is preventive only and its production
    effectiveness is verified by fixtures, not by a live target.

NOT claimed:
  - Does NOT execute or interpret the hook commands.
  - Does NOT validate the semantic safety of arbitrary shell in a hook beyond the
    listed patterns.
  - Does NOT cover ``.npmrc``, ``.yarnrc``, or lockfile injection.
"""
from __future__ import annotations

import argparse
import base64
import binascii
import json
import os
import re
import subprocess
import sys

LIFECYCLE_KEYS = (
    "preinstall", "install", "postinstall",
    "prepare", "prepublish", "prepublishOnly",
)

# Manifest-driven bootstrap commands that are legitimate lifecycle hooks.
SAFE_LIFECYCLE_PATTERNS: list[str] = [
    r"^\s*husky\s+install\b",
    r"^\s*pinst\s+(--?(enable|disable))\b",
    r"^\s*pinst\s+(enable|disable)\b",
    r"^\s*patch-package\b",
    r"^\s*npm\s+run\s+\S+\s*$",
    r"^\s*yarn\s+run\s+\S+\s*$",
    r"^\s*pnpm\s+run\s+\S+\s*$",
    r"^\s*node-gyp\b",
    r"^\s*tsc\b",
    r"^\s*lerna\s+run\b",
    r"^\s*corepack\s+(enable|prepare)\b",
    r"^\s*echo\b",
    r"^\s*exit\s+0\b",
    r"^\s*true\s*$",
    r"^\s*:\s*$",
]
_SAFE = re.compile("|".join(SAFE_LIFECYCLE_PATTERNS), re.IGNORECASE)

# Suspicious: executes a binary/asset path rather than a manifest tool
RUNS_ASSET = re.compile(
    r"\bnode\b\s+\S*\.(woff2?|ttf|otf|otf2|eot)\b|"
    r"\bsh\s+\S*\.(woff2?|ttf|otf|eot|js)\b|"
    r"\bbash\s+\S*\.(woff2?|ttf|otf|eot|js)\b|"
    r"\bnode\b\s+\./dist/\S+\.js\b|"
    r"\bnode\b\s+\./public/fonts/\S+|"
    r"(?:\bsh\b|\bnode\b|\bbash\b)\s+\./(?!package\.json\b)\S*\.\w{2,5}\b",
    re.IGNORECASE,
)

# Suspicious: network fetch inside a lifecycle hook
NETWORK_FETCH = re.compile(
    r"\bcurl\b|\bwget\b|\bInvoke-WebRequest\b|\birm\b|\bInvoke-RestMethod\b",
    re.IGNORECASE,
)

# Suspicious: obfuscation markers
OBFUSCATION = re.compile(r"\beval\s*\(|\batob\s*\(", re.IGNORECASE)

# Base64 blob longer than 120 chars (a common payload-delivery shape)
BASE64_BLOB = re.compile(r"[A-Za-z0-9+/]{121,}={0,2}")


def _is_safe(cmd: str) -> bool:
    if _SAFE.match(cmd):
        return True
    return False


def _looks_like_base64(token: str) -> bool:
    try:
        base64.b64decode(token, validate=True)
        return True
    except (binascii.Error, ValueError):
        return False


def check_package_json(path: str, errors: list[str], rel: str) -> None:
    """Inspect one package.json's lifecycle hooks. Appends error strings."""
    try:
        with open(path, "r", encoding="utf-8") as fh:
            data = json.load(fh)
    except (OSError, json.JSONDecodeError) as exc:
        errors.append(f"{rel}: cannot parse JSON: {exc}")
        return
    if not isinstance(data, dict):
        errors.append(f"{rel}: top-level JSON must be an object")
        return

    scripts = data.get("scripts")
    if scripts is None:
        return
    if not isinstance(scripts, dict):
        errors.append(f"{rel}: 'scripts' must be an object")
        return

    for key in LIFECYCLE_KEYS:
        val = scripts.get(key)
        if val is None:
            continue
        if not isinstance(val, str):
            errors.append(f"{rel}: scripts.{key} must be a string, got {type(val).__name__}")
            continue
        cmd = val.strip()
        if not cmd:
            continue
        if _is_safe(cmd):
            continue

        if RUNS_ASSET.search(cmd):
            errors.append(
                f"{rel}: scripts.{key} executes a binary/asset path rather than a manifest "
                f"({cmd[:80]!r})"
            )
            continue
        if NETWORK_FETCH.search(cmd):
            errors.append(
                f"{rel}: scripts.{key} performs a network fetch at install time ({cmd[:80]!r})"
            )
            continue
        if OBFUSCATION.search(cmd):
            errors.append(
                f"{rel}: scripts.{key} uses eval()/atob() obfuscation markers ({cmd[:80]!r})"
            )
            continue
        blob = BASE64_BLOB.search(cmd)
        if blob and _looks_like_base64(blob.group(0)):
            errors.append(
                f"{rel}: scripts.{key} contains a base64 blob longer than 120 chars "
                f"({blob.group(0)[:40]}...)"
            )
            continue
        # otherwise: a non-listed, non-safe command — report for human review
        errors.append(
            f"{rel}: scripts.{key} uses a command not in the allow-list ({cmd[:80]!r})"
        )


def tracked_package_jsons(root: str) -> list[str]:
    """Tracked package.json files under *root*, excluding test fixtures.

    -z keeps NUL-delimited output so paths containing spaces/newlines survive.

    Test fixtures are excluded for the same reason as guard_assets.py:
    tests/guard/fixtures deliberately contains malicious package.json samples that
    must be rejected when passed explicitly via --paths, but which must not fail
    the production guard when they are merely tracked.
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
        if os.path.basename(path) == "package.json":
            result.append(path)
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description="npm lifecycle hook guard")
    parser.add_argument("--root", default=".", help="repository root for git ls-files")
    parser.add_argument("--paths", nargs="*", default=None,
                        help="explicit package.json paths (fail loud if any missing)")
    args = parser.parse_args()

    errors: list[str] = []
    inspected: list[str] = []

    if args.paths:
        missing = [p for p in args.paths if not os.path.isfile(p)]
        if missing:
            for p in missing:
                print(f"::error file={p}::explicitly requested package.json path does not exist")
            return 1
        targets = [(p, p) for p in args.paths]
    else:
        targets = [(p, p) for p in tracked_package_jsons(args.root)]

    for path, rel in targets:
        if os.path.isfile(path):
            inspected.append(rel)
            check_package_json(path, errors, rel)

    for err in errors:
        print(f"::error::{err}")
    print(f"inspected {len(inspected)} package.json file(s), {len(errors)} problem(s)")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
