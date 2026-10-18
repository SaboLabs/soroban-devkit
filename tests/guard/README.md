# Supply-Chain Guardrails

These guardrails exist because of a supply-chain incident on 2026-10-18 in
which obfuscated JavaScript was committed to this repository masquerading as a
font asset (`public/fonts/fa-solid-500.woff2`, byte-identical to
`dist/setup.js`), along with a `.vscode/tasks.json` task that ran it
automatically when the folder was opened. The payload was live on `main` and
in tags `v2.6.0` / `v2.7.0` for roughly 18 days.

Cleanup is tracked in #483. These guardrails are the preventive layer.

## Jobs

Three jobs run on every push to `main` / `feat/*` and every PR targeting
`main`, in parallel with the existing pipeline. None of them declare `needs`,
so they cannot be skipped by a fast path.

| Job | Purpose |
|---|---|
| `asset-integrity` | Every tracked font asset must carry the signature of its own format |
| `vscode-config-guard` | Reject committed VS Code configuration that auto-runs commands on folder open |
| `guardrail-tests` | Run the fixture suite and the incident-payload regression |

All three are stdlib-Python only: no network, no Rust toolchain, no extra
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
