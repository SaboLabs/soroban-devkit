#!/usr/bin/env bash
# tests/guard/run_fixture_tests.sh
# Runs the guardrail scripts against synthetic fixtures and asserts expected
# exit codes. Self-contained: generates fixtures if missing, cleans up
# generated artifacts afterwards (keeps the fixture generators committed).
#
# Usage: bash tests/guard/run_fixture_tests.sh
# Exit 0 = all assertions passed, 1 = at least one failed.
set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
FIX="$ROOT/tests/guard/fixtures"
GUARD="$ROOT/scripts/guard"
PASS=0
FAIL=0

# Regenerate fixtures if missing (first run / after clean).
if [ ! -f "$FIX/fonts/valid/fa-valid.woff2" ]; then
  python3 "$FIX/make_fixtures.py" >/dev/null
fi
if [ ! -f "$FIX/vscode/positive/runon-folderopen/tasks.json" ]; then
  python3 "$FIX/make_vscode_fixtures.py" >/dev/null
fi

assert_rc() {
  local label="$1" expected="$2" got="$3"
  if [ "$got" = "$expected" ]; then
    printf 'PASS  %-40s rc=%s\n' "$label" "$got"
    PASS=$((PASS + 1))
  else
    printf 'FAIL  %-40s expected rc=%s got rc=%s\n' "$label" "$expected" "$got"
    FAIL=$((FAIL + 1))
  fi
}

assert_output() {
  local label="$1" needle="$2" haystack="$3"
  if printf '%s' "$haystack" | grep -qF "$needle"; then
    printf 'PASS  %-40s\n' "$label"
    PASS=$((PASS + 1))
  else
    printf 'FAIL  %-40s expected substring: %s\n' "$label" "$needle"
    FAIL=$((FAIL + 1))
  fi
}

echo "==== Guard A — asset integrity ===="

# --- valid assets (rc=0) ---
for f in \
  fonts/valid/fa-valid.woff2 \
  fonts/valid/fa-valid.woff \
  fonts/valid/fa-valid.ttf \
  fonts/valid/fa-valid.otf \
  fonts/valid/fa-valid.eot \
  fonts/valid/fa-valid.svg \
  "fonts/valid/fa solid 500 copy.woff2"
do
  out="$(python3 "$GUARD/guard_assets.py" --root "$FIX" --paths "$f" 2>&1)"
  assert_rc "A valid:${f##*/}" 0 "$?"
done

# --- invalid assets (rc=1) ---
for f in \
  fonts/invalid/not-a-font.woff2 \
  fonts/invalid/not-a-font.woff \
  fonts/invalid/not-a-font.otf \
  fonts/invalid/gzip-as.ttf \
  fonts/invalid/truncated.eot \
  fonts/invalid/size-lies.eot \
  fonts/invalid/not-svg.svg \
  fonts/invalid/broken.svg \
  fonts/invalid/empty.woff2
do
  out="$(python3 "$GUARD/guard_assets.py" --root "$FIX" --paths "$f" 2>&1)"
  assert_rc "A invalid:${f##*/}" 1 "$?"
done

# --- multi-path enumeration with a filename containing spaces ---
out="$(python3 "$GUARD/guard_assets.py" --root "$FIX" \
  --paths fonts/valid/fa-valid.woff2 "fonts/valid/fa solid 500 copy.woff2" 2>&1)"
assert_rc "A multi-path spaces" 0 "$?"

# --- missing tracked asset is reported (rc=1) ---
out="$(python3 "$GUARD/guard_assets.py" --root "$ROOT" \
  --paths fonts/does-not-exist.woff2 2>&1)"
assert_rc "A missing-asset" 1 "$?"
assert_output "A missing-asset message" "missing from the working tree" "$out"

# --- SVG checks ---
out="$(python3 "$GUARD/guard_assets.py" --root "$FIX" --paths fonts/invalid/not-svg.svg 2>&1)"
assert_output "A not-svg reason" "expected <svg>" "$out"
out="$(python3 "$GUARD/guard_assets.py" --root "$FIX" --paths fonts/invalid/size-lies.eot 2>&1)"
assert_output "A eot size reason" "EOT size field" "$out"

echo
echo "==== Guard B — VS Code auto-task ===="

run_vscode() {
  # $1 = case dir name (relative to fixtures/vscode/)
  # Passes only the files that exist: several negative fixtures legitimately
  # contain just one of the two config files.
  local d="$FIX/vscode/$1" args=()
  [ -f "$d/tasks.json" ] && args+=(--tasks "$d/tasks.json")
  [ -f "$d/settings.json" ] && args+=(--settings "$d/settings.json")
  python3 "$GUARD/guard_vscode_config.py" "${args[@]}"
}
# --- safe configs (rc=0) ---
for c in clean-settings manual-task name-collisions jsonc; do
  out="$(run_vscode "negative/$c")"
  assert_rc "B safe:$c" 0 "$?"
done

# --- dangerous configs (rc=1) ---
for c in runon-folderopen legacy-flat-runon allow-automatic-tasks stray-tasks-block combined; do
  out="$(run_vscode "positive/$c")"
  assert_rc "B dangerous:$c" 1 "$?"
done

# --- explicit error messages ---
out="$(run_vscode positive/runon-folderopen)"
assert_output "B folderOpen named" "folderOpen" "$out"
out="$(run_vscode positive/allow-automatic-tasks)"
assert_output "B allowAutomaticTasks named" "allowAutomaticTasks" "$out"
out="$(run_vscode positive/stray-tasks-block)"
assert_output "B stray tasks named" "tasks" "$out"

# --- an explicitly requested config path that does not exist must fail loud,
#     not silently pass ---
out="$(python3 "$GUARD/guard_vscode_config.py" \
  --tasks "$FIX/vscode/does-not-exist/tasks.json" 2>&1)"
assert_rc "B missing explicit path" 1 "$?"

echo
echo "==== Guard C — devcontainer auto-run ===="

run_devc() {
  # $1 = case dir relative to fixtures/devcontainer/
  python3 "$GUARD/guard_devcontainer.py" --root "$FIX" --paths "$FIX/devcontainer/$1/devcontainer.json"
}

# --- safe configs (rc=0) ---
for c in clean-minimal clean-npm-ci clean-cargo-build clean-apt-update root-without-hooks jsonc-comments-trailing-comma clean-bun-install clean-pip-install array-form-clean object-form-clean named-key-setup-clean named-key-multi-clean array-form-argv-clean option-script-clean fullpath-script-clean; do
  out="$(run_devc "negative/$c" 2>&1)"
  assert_rc "C safe:$c" 0 "$?"
done

# --- dangerous configs (rc=1) ---
for c in fetch-and-run runs-asset-path poststart-network root-with-hooks initialize-exec ps-fetch npm-ci-chain-curl npm-ci-semicolon-curl cargo-chain-font array-form-danger object-form-danger named-key-setup-danger command-key-with-dangerous-sibling array-argv-node-woff2 array-argv-bash-dist-js array-argv-sh-c-fetch asset-bare-dist-js asset-bare-public-fonts-js asset-bare-assets-mjs object-form-empty object-form-bad-type array-element-number array-element-null array-element-object array-element-mixed-object array-element-subarray option-asset fullpath-asset nested-seed traversal-seed; do
  out="$(run_devc "positive/$c" 2>&1)"
  assert_rc "C dangerous:$c" 1 "$?"
done

# --- explicit error messages ---
out="$(run_devc positive/fetch-and-run)"
assert_output "C fetch-and-run named" "network fetch" "$out"
out="$(run_devc positive/runs-asset-path)"
assert_output "C asset path named" "binary/asset path" "$out"
out="$(run_devc positive/root-with-hooks)"
assert_output "C root-with-hooks named" "runs as root" "$out"
out="$(run_devc negative/clean-npm-ci)"
assert_rc "C npm ci allowed" 0 "$?"

# --- allow-list must NOT short-circuit danger checks on chained commands ---
out="$(run_devc positive/npm-ci-chain-curl)"
assert_output "C chained curl named" "network fetch" "$out"
out="$(run_devc positive/cargo-chain-font)"
assert_output "C chained asset named" "binary/asset path" "$out"
out="$(run_devc positive/array-form-danger)"
assert_output "C array danger named" "network fetch" "$out"
out="$(run_devc positive/object-form-danger)"
assert_output "C object danger named" "asset/seed directory" "$out"

# --- named-command objects under arbitrary keys must be scanned ---
out="$(run_devc positive/named-key-setup-danger)"
assert_rc "C named-key danger" 1 "$?"
assert_output "C named-key danger named" "network fetch" "$out"
out="$(run_devc positive/object-form-empty)"
assert_rc "C empty object fails loud" 1 "$?"
out="$(run_devc positive/object-form-bad-type)"
assert_rc "C bad-type object fails loud" 1 "$?"

# --- malformed array elements must fail closed with a diagnostic ---
out="$(run_devc positive/array-element-number)"
assert_rc "C array number fails" 1 "$?"
assert_output "C array number named" "unsupported element" "$out"
out="$(run_devc positive/array-element-null)"
assert_rc "C array null fails" 1 "$?"
out="$(run_devc positive/array-element-object)"
assert_rc "C array object fails" 1 "$?"
out="$(run_devc positive/array-element-mixed-object)"
assert_rc "C array mixed fails" 1 "$?"
out="$(run_devc positive/array-element-subarray)"
assert_rc "C array subarray fails" 1 "$?"

# --- a "command" key must not hide a dangerous sibling key ---
out="$(run_devc positive/command-key-with-dangerous-sibling)"
assert_rc "C dangerous sibling" 1 "$?"
assert_output "C dangerous sibling named" "network fetch" "$out"

# --- array form executes directly: interpreter+path across argv elements ---
out="$(run_devc positive/array-argv-node-woff2)"
assert_rc "C array argv woff2" 1 "$?"
assert_output "C array argv woff2 named" "binary/asset path" "$out"
out="$(run_devc positive/array-argv-bash-dist-js)"
assert_rc "C array argv dist js" 1 "$?"
out="$(run_devc positive/array-argv-sh-c-fetch)"
assert_rc "C array argv sh -c fetch" 1 "$?"
assert_output "C array argv sh -c named" "network fetch" "$out"

# --- interpreter options / full paths must not bypass asset detection ---
out="$(run_devc positive/option-asset)"
assert_rc "C option-asset blocked" 1 "$?"
assert_output "C option-asset named" "binary/asset path" "$out"
out="$(run_devc positive/fullpath-asset)"
assert_rc "C fullpath-asset blocked" 1 "$?"
assert_output "C fullpath-asset named" "binary/asset path" "$out"
out="$(run_devc positive/nested-seed)"
assert_rc "C nested-seed blocked" 1 "$?"
assert_output "C nested-seed named" "asset/seed directory" "$out"
out="$(run_devc positive/traversal-seed)"
assert_rc "C traversal-seed blocked" 1 "$?"
assert_output "C traversal-seed named" "asset/seed directory" "$out"
out="$(run_devc negative/option-script-clean)"
assert_rc "C option-script-clean passes" 0 "$?"
out="$(run_devc negative/fullpath-script-clean)"
assert_rc "C fullpath-script-clean passes" 0 "$?"

# --- quoted arguments must not bypass asset detection ---
out="$(run_devc positive/quoted-asset)"
assert_rc "C quoted-asset blocked" 1 "$?"
assert_output "C quoted-asset named" "asset/seed directory" "$out"
out="$(run_devc positive/quoted-asset-fullpath)"
assert_rc "C quoted-asset-fullpath blocked" 1 "$?"
assert_output "C quoted-asset-fullpath named" "asset/seed directory" "$out"
out="$(run_devc positive/quoted-bare-asset)"
assert_rc "C quoted-bare-asset blocked" 1 "$?"
assert_output "C quoted-bare-asset named" "binary/asset path" "$out"
out="$(run_devc positive/quoted-traversal-asset)"
assert_rc "C quoted-traversal-asset blocked" 1 "$?"
assert_output "C quoted-traversal-asset named" "asset/seed directory" "$out"
out="$(run_devc negative/quoted-script-clean)"
assert_rc "C quoted-script-clean passes" 0 "$?"

# --- bare (no "./") asset/seed paths must be flagged ---
for c in asset-bare-dist-js asset-bare-public-fonts-js asset-bare-assets-mjs; do
  out="$(run_devc "positive/$c")"
  assert_rc "C bare path:$c" 1 "$?"
done
out="$(run_devc positive/asset-bare-dist-js)"
assert_output "C bare dist named" "asset/seed directory" "$out"

# --- an explicitly requested path that does not exist must fail loud ---
out="$(python3 "$GUARD/guard_devcontainer.py" \
  --paths "$FIX/devcontainer/does-not-exist/devcontainer.json" 2>&1)"
assert_rc "C missing explicit path" 1 "$?"

echo
echo "==== Guard D — npm lifecycle hooks ===="

run_npm() {
  python3 "$GUARD/guard_npm_lifecycle.py" --root "$FIX" --paths "$FIX/npm/$1/package.json"
}

# --- safe configs (rc=0) ---
for c in husky-prepare pinst-postinstall npm-run-build tsc-prepare node-gyp-preinstall no-lifecycle-hooks no-scripts-key echo-postinstall patch-package npm-run-build-chain npm-run-if-present node-scripts-postinstall option-local-script-clean fullpath-local-script-clean option-flag-value-clean; do
  out="$(run_npm "negative/$c" 2>&1)"
  assert_rc "D safe:$c" 0 "$?"
done

# --- dangerous configs (rc=1) ---
for c in preinstall-runs-asset preinstall-font postinstall-network postinstall-wget prepare-eval install-base64-blob install-runs-js-outside-scripts husky-chain-curl tsc-semicolon-asset husky-newline-curl husky-backslash-chain-curl echo-chain-asset local-script-danger local-script-js-fetch local-script-escape asset-bare-mjs asset-bare-cjs asset-bare-public-fonts-js asset-bare-dist-js option-asset fullpath-asset nested-seed traversal-seed option-local-script-danger; do
  out="$(run_npm "positive/$c" 2>&1)"
  assert_rc "D dangerous:$c" 1 "$?"
done

# --- bare (no "./") asset/seed paths must be flagged for the intended reason ---
for c in asset-bare-mjs asset-bare-cjs asset-bare-public-fonts-js asset-bare-dist-js; do
  out="$(run_npm "positive/$c")"
  assert_output "D bare path named:$c" "asset/build directory" "$out"
done

# --- a local script referenced by an allow-listed spelling must have its
#     CONTENTS inspected: the hook text alone is safe, the script is not ---
out="$(run_npm positive/local-script-danger)"
assert_rc "D local-script-danger" 1 "$?"
assert_output "D local-script danger named" "local script" "$out"
out="$(run_npm negative/local-script-benign)"
assert_rc "D local-script-benign" 0 "$?"

# --- a local script that performs a JavaScript network fetch must fail ---
out="$(run_npm positive/local-script-js-fetch)"
assert_rc "D local-script-js-fetch" 1 "$?"
assert_output "D local-script-js-fetch named" "JavaScript network fetch" "$out"

# --- an absolute local-script reference must fail closed ---
out="$(run_npm positive/local-script-escape)"
assert_rc "D local-script-escape" 1 "$?"
assert_output "D local-script-escape named" "outside the package directory" "$out"

# --- interpreter options / full paths must not bypass asset detection ---
out="$(run_npm positive/option-asset)"
assert_rc "D option-asset blocked" 1 "$?"
assert_output "D option-asset named" "binary/asset path" "$out"
out="$(run_npm positive/fullpath-asset)"
assert_rc "D fullpath-asset blocked" 1 "$?"
assert_output "D fullpath-asset named" "binary/asset path" "$out"
out="$(run_npm positive/nested-seed)"
assert_rc "D nested-seed blocked" 1 "$?"
assert_output "D nested-seed named" "asset/build directory" "$out"
out="$(run_npm positive/traversal-seed)"
assert_rc "D traversal-seed blocked" 1 "$?"
assert_output "D traversal-seed named" "asset/build directory" "$out"

# --- a local script invoked WITH interpreter flags must still be inspected ---
out="$(run_npm positive/option-local-script-danger)"
assert_rc "D option-local-script-danger" 1 "$?"
assert_output "D option-local-script-danger named" "JavaScript network fetch" "$out"
out="$(run_npm negative/option-local-script-clean)"
assert_rc "D option-local-script-clean" 0 "$?"
out="$(run_npm negative/fullpath-local-script-clean)"
assert_rc "D fullpath-local-script-clean" 0 "$?"
out="$(run_npm negative/option-flag-value-clean)"
assert_rc "D option-flag-value-clean" 0 "$?"

# --- quoted arguments must not bypass asset detection or script inspection ---
out="$(run_npm positive/quoted-asset)"
assert_rc "D quoted-asset blocked" 1 "$?"
assert_output "D quoted-asset named" "asset/build directory" "$out"
out="$(run_npm positive/quoted-asset-fullpath)"
assert_rc "D quoted-asset-fullpath blocked" 1 "$?"
assert_output "D quoted-asset-fullpath named" "asset/build directory" "$out"
out="$(run_npm positive/quoted-bare-asset)"
assert_rc "D quoted-bare-asset blocked" 1 "$?"
assert_output "D quoted-bare-asset named" "binary/asset path" "$out"
out="$(run_npm positive/quoted-traversal-asset)"
assert_rc "D quoted-traversal-asset blocked" 1 "$?"
assert_output "D quoted-traversal-asset named" "asset/build directory" "$out"
out="$(run_npm negative/quoted-script-clean)"
assert_rc "D quoted-script-clean" 0 "$?"
out="$(run_npm negative/quoted-local-script-clean)"
assert_rc "D quoted-local-script-clean" 0 "$?"
out="$(run_npm positive/require-preload-danger)"
assert_rc "D require-preload-danger" 1 "$?"
assert_output "D require-preload-danger named" "JavaScript network fetch" "$out"
out="$(run_npm negative/require-preload-clean)"
assert_rc "D require-preload-clean" 0 "$?"

# --- the decoded backslash-newline fixture must contain a real shell line
#     continuation, not a backslash escaping a space ---
python3 - "$FIX/npm/positive/husky-backslash-chain-curl/package.json" <<'PY'
import json, sys
p = sys.argv[1]
v = json.load(open(p))["scripts"]["postinstall"]
if "\\\n" not in v:
    print("FAIL backslash-newline not exercised: %r" % v); sys.exit(1)
if "\\ " in v:
    print("FAIL fixture uses backslash-space: %r" % v); sys.exit(1)
print("PASS  D backslash-newline decoded")
PY
assert_rc "D backslash-newline fixture" 0 "$?"

# --- explicit error messages ---
out="$(run_npm positive/preinstall-runs-asset)"
assert_output "D asset path named" "asset/build directory" "$out"
out="$(run_npm positive/postinstall-network)"
assert_output "D network named" "network fetch" "$out"
out="$(run_npm positive/prepare-eval)"
assert_output "D eval named" "eval()" "$out"

# --- allow-list must NOT short-circuit danger checks on chained commands ---
out="$(run_npm positive/husky-chain-curl)"
assert_output "D chained curl named" "network fetch" "$out"
out="$(run_npm positive/tsc-semicolon-asset)"
assert_output "D chained asset named" "asset/build directory" "$out"
out="$(run_npm positive/husky-newline-curl)"
assert_output "D newline chain named" "network fetch" "$out"
out="$(run_npm positive/husky-backslash-chain-curl)"
assert_output "D backslash chain named" "network fetch" "$out"
out="$(run_npm positive/echo-chain-asset)"
assert_output "D echo chain named" "asset/build directory" "$out"

# --- an explicitly requested path that does not exist must fail loud ---
out="$(python3 "$GUARD/guard_npm_lifecycle.py" \
  --paths "$FIX/npm/does-not-exist/package.json" 2>&1)"
assert_rc "D missing explicit path" 1 "$?"

# --- --root outside the current working directory must still inspect every
#     tracked manifest (regression: paths used to be CWD-relative) ---
EXTROOT="$(mktemp -d)"
python3 "$FIX/make_external_root.py" "$EXTROOT" >/dev/null
# Run from a directory that is NOT the repo root.
out="$(cd /tmp && python3 "$GUARD/guard_npm_lifecycle.py" --root "$EXTROOT" 2>&1)"
assert_rc "D external-root hostile" 1 "$?"
assert_output "D external-root hostile named" "network fetch" "$out"
assert_output "D external-root inspected both" "inspected 2 package.json file(s)" "$out"
# Safe manifest under the same external root must pass.
out="$(cd /tmp && python3 "$GUARD/guard_npm_lifecycle.py" --root "$EXTROOT" \
  --paths "$EXTROOT/safe/package.json" 2>&1)"
assert_rc "D external-root safe" 0 "$?"
# A tracked manifest that cannot be resolved against --root must fail loud,
# not silently report "inspected 0".
out="$(cd /tmp && python3 "$GUARD/guard_npm_lifecycle.py" --root "$EXTROOT" \
  --paths "$EXTROOT/missing.json" 2>&1)"
assert_rc "D external-root missing explicit" 1 "$?"
rm -rf "$EXTROOT"

echo
echo "==== Real-repo guards (post-cleanup tree) ===="

out="$(python3 "$GUARD/guard_assets.py" --root "$ROOT" 2>&1)"
assert_rc "A real repo (payload removed)" 0 "$?"

out="$(python3 "$GUARD/guard_vscode_config.py" --root "$ROOT" 2>&1)"
assert_rc "B real repo (settings clean)" 0 "$?"

# Preventive guards: this repo currently has no .devcontainer/ and no package.json,
# so these exercise the enumeration path and must exit 0 with 0 inspected files.
out="$(python3 "$GUARD/guard_devcontainer.py" --root "$ROOT" 2>&1)"
assert_rc "C real repo (no devcontainer)" 0 "$?"
assert_output "C real repo inspected 0" "inspected 0 devcontainer file(s)" "$out"

out="$(python3 "$GUARD/guard_npm_lifecycle.py" --root "$ROOT" 2>&1)"
assert_rc "D real repo (no package.json)" 0 "$?"
assert_output "D real repo inspected 0" "inspected 0 package.json file(s)" "$out"

echo
echo "==== summary: PASS=$PASS FAIL=$FAIL ===="
if [ "$FAIL" -ne 0 ]; then exit 1; fi
exit 0
