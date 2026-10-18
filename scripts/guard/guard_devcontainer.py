"""Guard B-2 — VS Code Dev Container auto-run configuration.

Detects dangerous *auto-execution* configuration in ``.devcontainer`` files, which
are JSONC and whose ``postCreateCommand`` / ``postStartCommand`` /
``postAttachCommand`` hooks run code automatically when a container is (re)started.

Scope (explicit, intentionally narrow):
  * files: ``.devcontainer/devcontainer.json``, ``.devcontainer.json``,
    ``.devcontainer/<name>/devcontainer.json`` (first match wins; no recursion into
    unrelated directories)
  * signals checked:
    - postCreateCommand / postStartCommand / postAttachCommand /
      onCreateCommand / initializeCommand / updateContentCommand
    - command bodies that pipe a network fetch to a shell
      (``curl ... | sh``, ``wget -O- ... | bash``, ``Invoke-WebRequest … | iex``)
    - command bodies that invoke a *binary/asset* path rather than a manifest
      (``node ./public/fonts/x.woff2``, ``dist/setup.js``, arbitrary ``./path``)
    - ``remoteUser`` / ``containerUser`` set to ``root`` *combined with* any exec hook
      (root alone is not flagged)
  * ALLOWED: plain package-manager / build commands invoked via manifest only
    (``npm ci``, ``yarn install``, ``cargo build``, ``rustup component add``,
    ``sudo apt-get update``). These are common legitimate devcontainer bootstrap lines
    and must not create noise.

False-positive policy (documented, not suppressed silently):
  * Any devcontainer.json is inspected; if *none* of the above hooks exist the file
    passes. A devcontainer with only ``"build": {...}, "extensions": [...]` passes.
  * If ``.devcontainer/`` does not exist at all, the guard prints
    ``inspected 0 file(s)`` and exits 0 — this repo currently has no devcontainer,
    so the guard is preventive only and its production effectiveness is verified
    by fixtures, not by a live target.

NOT claimed:
  - Does NOT scan Dockerfile `RUN`/`ENTRYPOINT` inside the devcontainer image build
    (out of scope; Dockerfile linting is a separate concern).
  - Does NOT cover user-level VS Code *devcontainer* overrides, extensions that
    inject tasks, or remote-SSH behavior.
  - Does NOT execute or interpret the commands it inspects.
"""
from __future__ import annotations

import argparse
import json
import os
import sys
import re
from pathlib import Path


# Commands that are ordinary, manifest-driven bootstrap — not suspicious on their own.
# These are allow-listed so the guard does not flag common legitimate devcontainer
# setup lines like "npm ci" or "cargo build".
SAFE_BOOTSTRAP_PATTERNS: list[str] = [
    r"^\s*npm\s+ci\b",
    r"^\s*yarn\s+install\b",
    r"^\s*pnpm\s+install\b",
    r"^\s*cargo\s+build\b",
    r"^\s*cargo\s+check\b",
    r"^\s*cargo\s+fmt\b",
    r"^\s*cargo\s+clippy\b",
    r"^\s*cargo\s+test\b",
    r"^\s*rustup\s+component\s+add\b",
    r"^\s*sudo\s+apt-get\s+update\b",
    r"^\s*sudo\s+apt-get\s+install\b",
    r"^\s*sudo\s+apt\s+update\b",
    r"^\s*sudo\s+apt\s+install\b",
]
_SAFE_BOOTSTRAP = re.compile("|".join(SAFE_BOOTSTRAP_PATTERNS), re.IGNORECASE)

# Suspicious: fetch-and-exec from the network
FETCH_AND_RUN = re.compile(
    r"curl\b.*\|\s*(sh|bash)|wget\b.*-O\?-.*\|\s*(sh|bash)|"
    r"Invoke-WebRequest\b.*\|\s*iex|irm\b.*\|\s*iex",
    re.IGNORECASE | re.DOTALL,
)

# Suspicious: run a binary/asset path that is clearly not a manifest tool
RUNS_ASSET = re.compile(
    r"\bnode\b\s+\S*\.(woff2?|ttf|otf|otf2|eot|js)\b|"
    r"\bsh\s+\S*\.(woff2?|ttf|otf|eot|js)\b|"
    r"\bbash\s+\S*\.(woff2?|ttf|otf|eot|js)\b|"
    # any ./relative path that points at a non-manifest file
    r"(?:\bsh\b|\bnode\b|\bbash\b)\s+\./(?!package\.json\b|Cargo\.toml\b)\S*\.\w{2,5}\b",
    re.IGNORECASE,
)

HOOK_KEYS = (
    "postCreateCommand", "postStartCommand", "postAttachCommand",
    "onCreateCommand", "initializeCommand", "updateContentCommand",
)


def _candidate_files():
    """Return likely devcontainer.json paths relative to repo root, in priority order.

    Does NOT recurse: devcontainer.json lives at one of these known locations.
    """
    return [
        ".devcontainer/devcontainer.json",
        ".devcontainer.json",
    ]


def _devcontainer_dirs(root: str) -> list[str]:
    """Named devcontainer folders under .devcontainer/ (e.g. .devcontainer/name/)."""
    base = os.path.join(root, ".devcontainer")
    out: list[str] = []
    if os.path.isdir(base):
        for name in os.listdir(base):
            p = os.path.join(base, name, "devcontainer.json")
            if os.path.isdir(os.path.join(base, name)) and os.path.isfile(p):
                out.append(p)
    return out


def strip_jsonc(text: str) -> str:
    """Minimal JSONC stripper: removes // and /* */ comments + trailing commas."""
    out: list[str] = []
    i, n = 0, len(text)
    in_str, esc = False, False
    while i < n:
        ch = text[i]
        if in_str:
            out.append(ch)
            if esc:
                esc = False
            elif ch == "\\":
                esc = True
            elif ch == '"':
                in_str = False
            i += 1
            continue
        if ch == '"':
            in_str = True
            out.append(ch)
            i += 1
            continue
        if ch == "/" and i + 1 < n and text[i + 1] == "/":
            while i < n and text[i] not in "\r\n":
                i += 1
            continue
        if ch == "/" and i + 1 < n and text[i + 1] == "*":
            i += 2
            while i + 1 < n and not (text[i] == "*" and text[i + 1] == "/"):
                i += 1
            i += 2
            continue
        out.append(ch)
        i += 1
    stripped = "".join(out)
    cleaned: list[str] = []
    j, m = 0, len(stripped)
    in_str, esc = False, False
    while j < m:
        ch = stripped[j]
        if in_str:
            cleaned.append(ch)
            if esc:
                esc = False
            elif ch == "\\":
                esc = True
            elif ch == '"':
                in_str = False
            j += 1
            continue
        if ch == '"':
            in_str = True
            cleaned.append(ch)
            j += 1
            continue
        if ch == ",":
            k = j + 1
            while k < m and stripped[k] in " \t\r\n":
                k += 1
            if k < m and stripped[k] in "}]":
                j += 1
                continue
        cleaned.append(ch)
        j += 1
    return "".join(cleaned)


def _is_safe_bootstrap(cmd: str) -> bool:
    if _SAFE_BOOTSTRAP.match(cmd):
        return True
    return False


def check_devcontainer(path: str, errors: list[str], rel: str) -> None:
    """Inspect a single devcontainer.json. Appends human-readable error strings."""
    try:
        raw = Path(path).read_text(encoding="utf-8")
    except OSError as exc:
        errors.append(f"{rel}: cannot read file: {exc}")
        return
    try:
        data = json.loads(strip_jsonc(raw))
    except json.JSONDecodeError as exc:
        errors.append(f"{rel}: invalid JSON/JSONC: {exc}")
        return
    if not isinstance(data, dict):
        errors.append(f"{rel}: top-level JSON must be an object")
        return

    hooks_found: list[str] = []
    for key in HOOK_KEYS:
        val = data.get(key)
        if val is None:
            continue
        if not isinstance(val, str):
            errors.append(f"{rel}: {key} must be a string, got {type(val).__name__}")
            continue
        hooks_found.append(key)
        cmd = val.strip()
        if not cmd:
            continue
        if _is_safe_bootstrap(cmd):
            continue
        if FETCH_AND_RUN.search(cmd):
            errors.append(
                f"{rel}: {key} pipes a network fetch to a shell ({cmd.strip()[:80]!r})"
            )
            continue
        if RUNS_ASSET.search(cmd):
            errors.append(
                f"{rel}: {key} executes a binary/asset path rather than a manifest ({cmd.strip()[:80]!r})"
            )
            continue
        # otherwise: a non-listed, non-safe command — report for human review
        errors.append(
            f"{rel}: {key} uses a command not in the allow-list ({cmd.strip()[:80]!r})"
        )

    # root user only matters if combined with an exec hook
    user = data.get("remoteUser") or data.get("containerUser")
    if user in ("root", "0") and hooks_found:
        errors.append(
            f"{rel}: runs as root (remoteUser/containerUser={user}) combined with "
            f"auto-exec hooks: {', '.join(hooks_found)}"
        )


def main() -> int:
    parser = argparse.ArgumentParser(description="devcontainer auto-run guard")
    parser.add_argument("--root", default=".", help="repository root")
    parser.add_argument("--paths", nargs="*", default=None,
                        help="explicit devcontainer.json paths (fail loud if any missing)")
    args = parser.parse_args()

    errors: list[str] = []
    inspected: list[str] = []

    if args.paths:
        missing = [p for p in args.paths if not os.path.isfile(p)]
        if missing:
            for p in missing:
                print(f"::error file={p}::explicitly requested devcontainer.json path does not exist")
            return 1
        targets = [(p, os.path.relpath(p, args.root)) for p in args.paths]
    else:
        targets = []
        for rel in _candidate_files():
            abs_p = os.path.join(args.root, rel)
            if os.path.isfile(abs_p):
                targets.append((abs_p, rel))
        for abs_p in _devcontainer_dirs(args.root):
            # rel made pretty
            try:
                rel = os.path.relpath(abs_p, args.root)
            except ValueError:
                rel = abs_p
            targets.append((abs_p, rel))

    for path, rel in targets:
        if os.path.isfile(path):
            inspected.append(rel)
            check_devcontainer(path, errors, rel)

    for err in errors:
        print(f"::error::{err}")
    print(f"inspected {len(inspected)} devcontainer file(s), {len(errors)} problem(s)")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
