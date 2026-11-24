#!/usr/bin/env python3
"""Guard B - reject VS Code configuration that auto-runs commands on folder open.

Rules (structural, not textual):
  tasks.json
    * any task with runOptions.runOn == "folderOpen"        -> error
    * any task with a legacy flat runOn == "folderOpen"     -> error
    * task.allowAutomaticTasks == true                      -> error
  settings.json
    * task.allowAutomaticTasks == true                      -> error
    * a stray top-level "tasks" *task definition* (an object carrying
      "command"/"label"/"type") -> error; tasks belong in tasks.json
    * runOn == "folderOpen"                                 -> error

Deliberately NOT rules (avoid false positives):
  * a property whose name merely contains "tasks" (e.g. "myTasks.enabled")
  * the string "folderOpen" appearing in an unrelated value (e.g. a word list)
  * text inside // or /* */ comments, or trailing commas (VS Code uses JSONC)

LIMITATIONS:
  * only .vscode/tasks.json and .vscode/settings.json are inspected; user-level
    (~/Library/Application Support/Code/User) and workspace-*.code-workspace
    configs are outside the repository and cannot be reviewed here
  * tasks defined by extensions, by a "dependsOn" chain, or with
    "runOptions.runOn": "folderOpen" expressed via a variable cannot be resolved
    statically
  * parsing is JSONC-by-stripping: exotic JSONC extensions are rejected, not
    silently accepted

Usage: guard_vscode_config.py [--root DIR] [--tasks PATH] [--settings PATH]
Exit codes: 0 = clean, 1 = at least one dangerous configuration found.
"""
from __future__ import annotations

import argparse
import json
import os
import sys

AUTO_KEY = "task.allowAutomaticTasks"
FOLDER_OPEN = "folderOpen"
TASK_SHAPE_KEYS = ("command", "label", "type", "dependsOn")


def strip_jsonc(text: str) -> str:
    """Remove // and /* */ comments plus trailing commas, preserving strings."""
    out: list[str] = []
    i, n = 0, len(text)
    in_str = False
    escaped = False
    while i < n:
        ch = text[i]
        if in_str:
            out.append(ch)
            if escaped:
                escaped = False
            elif ch == "\\":
                escaped = True
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
    # drop trailing commas before } or ]
    cleaned: list[str] = []
    j, m = 0, len(stripped)
    in_str = False
    escaped = False
    while j < m:
        ch = stripped[j]
        if in_str:
            cleaned.append(ch)
            if escaped:
                escaped = False
            elif ch == "\\":
                escaped = True
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


def load_jsonc(path: str):
    with open(path, "r", encoding="utf-8") as fh:
        raw = fh.read()
    return json.loads(strip_jsonc(raw))


def is_folder_open(value) -> bool:
    return isinstance(value, str) and value.strip().lower() == FOLDER_OPEN.lower()


def check_tasks_json(path: str, errors: list[str], rel: str) -> None:
    data = load_jsonc(path)
    if not isinstance(data, dict):
        errors.append(f"{rel}: expected a JSON object at the top level")
        return

    if data.get(AUTO_KEY) is True:
        errors.append(f"{rel}: {AUTO_KEY} is true (automatic task execution enabled)")

    tasks = data.get("tasks")
    if tasks is None:
        return
    if not isinstance(tasks, list):
        errors.append(f"{rel}: 'tasks' must be an array")
        return

    for idx, task in enumerate(tasks):
        if not isinstance(task, dict):
            continue
        label = task.get("label", f"#{idx + 1}")
        if is_folder_open(task.get("runOn")):
            errors.append(f"{rel}: task {label!r} has runOn: folderOpen (legacy form)")
        run_options = task.get("runOptions")
        if isinstance(run_options, dict) and is_folder_open(run_options.get("runOn")):
            errors.append(f"{rel}: task {label!r} has runOptions.runOn: folderOpen")
        if task.get(AUTO_KEY) is True:
            errors.append(f"{rel}: task {label!r} sets {AUTO_KEY}: true")


def check_settings_json(path: str, errors: list[str], rel: str) -> None:
    data = load_jsonc(path)
    if not isinstance(data, dict):
        errors.append(f"{rel}: expected a JSON object at the top level")
        return

    if data.get(AUTO_KEY) is True:
        errors.append(f"{rel}: {AUTO_KEY} is true (automatic task execution enabled)")

    if is_folder_open(data.get("runOn")):
        errors.append(f"{rel}: top-level runOn: folderOpen")

    stray = data.get("tasks")
    if isinstance(stray, dict):
        if any(key in stray for key in TASK_SHAPE_KEYS):
            errors.append(
                f"{rel}: stray 'tasks' block with task fields "
                f"({', '.join(k for k in TASK_SHAPE_KEYS if k in stray)}) - "
                f"task definitions belong in .vscode/tasks.json"
            )
    elif stray is not None:
        errors.append(f"{rel}: unexpected 'tasks' value of type {type(stray).__name__}")


def check_config(path: str, kind: str) -> list[str]:
    errors: list[str] = []
    rel = path
    if not os.path.isfile(path):
        return errors
    try:
        if kind == "tasks":
            check_tasks_json(path, errors, rel)
        else:
            check_settings_json(path, errors, rel)
    except json.JSONDecodeError as exc:
        errors.append(f"{rel}: invalid JSON/JSONC ({exc})")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser(description="VS Code auto-task guard")
    parser.add_argument("--root", default=".")
    parser.add_argument("--tasks", default=None, help="explicit tasks.json path")
    parser.add_argument("--settings", default=None, help="explicit settings.json path")
    args = parser.parse_args()

    if args.tasks or args.settings:
        targets = ((args.tasks, "tasks"), (args.settings, "settings"))
        missing = [p for p, _ in targets if p and not os.path.isfile(p)]
        if missing:
            for p in missing:
                print(f"::error file={p}::explicitly requested config path does not exist")
            return 1
    else:
        targets = (
            (os.path.join(args.root, ".vscode", "tasks.json"), "tasks"),
            (os.path.join(args.root, ".vscode", "settings.json"), "settings"),
        )

    errors: list[str] = []
    inspected = []
    for path, kind in targets:
        if path and os.path.isfile(path):
            inspected.append(path)
            errors.extend(check_config(path, kind))

    for err in errors:
        print(f"::error::{err}")
    print(f"inspected {len(inspected)} config file(s), {len(errors)} problem(s)")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
