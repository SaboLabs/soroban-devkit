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
echo "==== Real-repo guards (post-cleanup tree) ===="

out="$(python3 "$GUARD/guard_assets.py" --root "$ROOT" 2>&1)"
assert_rc "A real repo (payload removed)" 0 "$?"

out="$(python3 "$GUARD/guard_vscode_config.py" --root "$ROOT" 2>&1)"
assert_rc "B real repo (settings clean)" 0 "$?"

echo
echo "==== summary: PASS=$PASS FAIL=$FAIL ===="
if [ "$FAIL" -ne 0 ]; then exit 1; fi
exit 0
