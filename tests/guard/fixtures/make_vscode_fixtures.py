#!/usr/bin/env python3
"""Generate VS Code config fixtures: negative (safe) and positive (dangerous).

The "dangerous" fixtures reproduce the *shape* of the incident configuration
using inert placeholder commands (echo), never the real payload command.
"""
from __future__ import annotations

import json
import os

HERE = os.path.dirname(os.path.abspath(__file__))


def write_pair(case_dir: str, tasks_obj, settings_obj, comments=None):
    base = os.path.join(HERE, "vscode", case_dir)
    os.makedirs(base, exist_ok=True)
    if tasks_obj is not None:
        with open(os.path.join(base, "tasks.json"), "w", encoding="utf-8") as fh:
            json.dump(tasks_obj, fh, indent=2)
            fh.write("\n")
    if settings_obj is not None:
        with open(os.path.join(base, "settings.json"), "w", encoding="utf-8") as fh:
            json.dump(settings_obj, fh, indent=2)
            fh.write("\n")
    if comments:
        with open(os.path.join(base, "notes.txt"), "w", encoding="utf-8") as fh:
            fh.write(comments + "\n")


# ============================ NEGATIVE (must pass) ============================

# 1. clean repo-style settings.json: benign keys only, no tasks block
write_pair(
    "negative/clean-settings",
    None,
    {
        "search.exclude": {"**/.sst": True},
        "typescript.tsdk": "node_modules/typescript/lib",
        "editor.defaultFormatter": "esbenp.prettier-vscode",
        "problems.autoReveal": True,
    },
    "benign settings only - the post-cleanup state of the real repo",
)

# 2. tasks.json with a normal manual task, no folderOpen, no allowAutomaticTasks
write_pair(
    "negative/manual-task",
    {
        "version": "2.0.0",
        "tasks": [
            {
                "label": "build",
                "type": "shell",
                "command": "cargo build",
                "group": "build",
                "problemMatcher": ["$rustc"],
            }
        ],
    },
    {"editor.formatOnSave": True},
    "ordinary manual task - nothing automatic",
)

# 3. FALSE-POSITIVE GUARD: property names containing "tasks", and the word
#    "folderOpen" appearing inside an unrelated string value. Must pass.
write_pair(
    "negative/name-collisions",
    {
        "version": "2.0.0",
        "tasks": [
            {
                "label": "lint",
                "type": "shell",
                "command": "cargo clippy",
                # a *manual* task whose label mentions folderOpen as prose
                "detail": "runs on demand, never on folderOpen",
            }
        ],
    },
    {
        "myTasks.enabled": True,
        "tasksViewer.show": True,
        "folderOpenBehavior": "ask",
        "notes": "the word folderOpen here is a value, not a directive",
    },
    "names/values containing 'tasks'/'folderOpen' must NOT trip the guard",
)

# 4. JSONC: comments + trailing comma must parse cleanly
_jsonc_dir = os.path.join(HERE, "vscode", "negative", "jsonc")
os.makedirs(_jsonc_dir, exist_ok=True)
with open(os.path.join(_jsonc_dir, "tasks.json"), "w", encoding="utf-8") as _fh:
    _fh.write(
        "{\n"
        '  "version": "2.0.0",\n'
        "  // a normal manual task, JSONC comment form\n"
        '  "tasks": [\n'
        '    { "label": "build", "type": "shell", "command": "cargo build" },\n'
        "  ],\n"
        "}\n"
    )
with open(os.path.join(_jsonc_dir, "settings.json"), "w", encoding="utf-8") as _fh:
    _fh.write(
        "{\n"
        "  /* block comment */\n"
        '  "editor.tabSize": 2, // trailing comment\n'
        '  "files.trimTrailingWhitespace": true,\n'
        "}\n"
    )
with open(os.path.join(_jsonc_dir, "notes.txt"), "w", encoding="utf-8") as _fh:
    _fh.write("JSONC with comments and trailing commas must parse cleanly\n")

# ============================ POSITIVE (must fail) ============================

# 1. exact shape of the incident: runOptions.runOn folderOpen + hidden background
write_pair(
    "positive/runon-folderopen",
    {
        "version": "2.0.0",
        "tasks": [
            {
                "label": "eslint-check",
                "type": "shell",
                "command": "echo placeholder",
                "isBackground": True,
                "hide": True,
                "presentation": {"reveal": "never"},
                "runOptions": {"runOn": "folderOpen"},
            }
        ],
    },
    {"editor.tabSize": 2},
    "incident shape: runOptions.runOn folderOpen",
)

# 2. legacy flat runOn on the task
write_pair(
    "positive/legacy-flat-runon",
    {
        "version": "2.0.0",
        "tasks": [
            {"label": "legacy", "type": "shell", "command": "echo x", "runOn": "folderOpen"}
        ],
    },
    None,
    "legacy flat runOn form",
)

# 3. settings.json carrying task.allowAutomaticTasks
write_pair(
    "positive/allow-automatic-tasks",
    None,
    {
        "task.allowAutomaticTasks": True,
        "editor.tabSize": 4,
    },
    "settings-level allowAutomaticTasks",
)

# 4. settings.json carrying a stray tasks block with task fields
write_pair(
    "positive/stray-tasks-block",
    None,
    {
        "compile-hero.disable-compile-files-on-did-save-code": True,
        "tasks": {
            "label": "lint on open",
            "type": "shell",
            "command": "npm run lint",
            "runOn": "folderOpen",
        },
    },
    "the inert stub still present on upstream main",
)

# 5. both files dangerous at once
write_pair(
    "positive/combined",
    {
        "version": "2.0.0",
        "tasks": [
            {
                "label": "a",
                "type": "shell",
                "command": "echo a",
                "runOptions": {"runOn": "folderOpen"},
            }
        ],
    },
    {"task.allowAutomaticTasks": True},
    "tasks.json + settings.json both dangerous",
)

print("vscode fixtures written under", HERE)
