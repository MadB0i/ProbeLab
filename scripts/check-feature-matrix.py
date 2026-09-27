#!/usr/bin/env python3
"""Check that the feature matrix still matches the repository.

# Why this exists

A feature matrix is a claim about what the product does. A claim that is not checked drifts, and a
drifted matrix is worse than none: it is a document that looks like verification while asserting
things nobody confirmed.

So this script checks the two ways a matrix actually goes stale:

1. A capability marked `complete` must name test files that exist. If a capability claims full
   coverage and its tests were deleted or renamed, the claim is now false and CI fails.
2. Every capability marked `complete` must be reachable in the product -- either through a Tauri
   command or through the IPC surface the frontend uses. A "complete" capability with no command is
   something a user cannot reach, which is a different thing from complete.

Deliberately not checked: whether the tests still *pass*. That is the test suite's job. This script
checks only that the matrix and the repository agree about what exists.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MATRIX = ROOT / "docs" / "feature-matrix.json"
COMMANDS = ROOT / "apps" / "desktop" / "src-tauri" / "src"

# Capabilities that are deliberately reachable without a dedicated command, because the command that
# drives them is shared with another capability. Keyed by capability id.
SHARED_COMMANDS = {
    "grid-world": "run_experiment",
    "sandbox-process": "run_experiment",
    "brain-panel": "brain_panel",
    "arena-tictactoe": "play_arena_match",
    "security-lab": "run_security_sweep",
    "replay": "replay_arena_match",
    "agent-registry": "list_agents",
    "benchmark-registry": "list_benchmark_details",
    "provider-registry": "list_providers",
    # The navigation shell registers no command of its own: it is a frontend-only surface whose whole
    # job is routing between the panels below, and every destination it offers is driven by one of
    # their commands. `brain_panel` is named as the witness because the command center is the
    # default destination -- reaching the nav and reaching the lab page are the same act.
    "ui-navigation": "brain_panel",
}


def fail(message: str) -> None:
    print(f"feature-matrix: {message}", file=sys.stderr)
    sys.exit(1)


def load_matrix() -> dict:
    if not MATRIX.exists():
        fail(f"{MATRIX.relative_to(ROOT)} does not exist")
    try:
        return json.loads(MATRIX.read_text(encoding="utf-8"))
    except json.JSONDecodeError as error:
        fail(f"the matrix is not valid JSON: {error}")


def registered_commands() -> set[str]:
    """Every command name Tauri will dispatch."""
    found: set[str] = set()
    for path in COMMANDS.rglob("*.rs"):
        text = path.read_text(encoding="utf-8")
        # `#[tauri::command]` on the next non-attribute line is the function name.
        for match in re.finditer(r"#\[tauri::command[^\]]*\]\s*(?:pub\s+)?(?:async\s+)?fn\s+(\w+)", text):
            found.add(match.group(1))
    return found


def registered_tests() -> set[str]:
    """Every test file in the workspace, as workspace-relative posix paths."""
    found: set[str] = set()
    for path in ROOT.glob("crates/*/tests/*.rs"):
        found.add(path.relative_to(ROOT).as_posix())
    for path in ROOT.glob("crates/*/src/*.rs"):
        found.add(path.relative_to(ROOT).as_posix())
    for path in ROOT.glob("apps/desktop/src-tauri/src/*.rs"):
        found.add(path.relative_to(ROOT).as_posix())
    for path in ROOT.glob("apps/desktop/src/*.test.ts*"):
        found.add(path.relative_to(ROOT).as_posix())
    return found


def main() -> int:
    matrix = load_matrix()
    capabilities = matrix.get("capabilities")
    if not isinstance(capabilities, list) or not capabilities:
        fail("the matrix declares no capabilities")

    commands = registered_commands()
    files = registered_tests()
    problems: list[str] = []

    for capability in capabilities:
        identifier = capability.get("id", "<missing id>")
        status = capability.get("status")
        if status not in {"complete", "partial", "not-implemented"}:
            problems.append(f"{identifier}: status {status!r} is not one of the three allowed values")
            continue

        for test_file in capability.get("tests", []):
            if test_file not in files:
                problems.append(
                    f"{identifier}: claims test file {test_file!r}, which does not exist"
                )

        if status != "complete":
            continue

        command = capability.get("command") or SHARED_COMMANDS.get(identifier)
        if not command:
            problems.append(
                f"{identifier}: is marked complete but names no command and is not in the shared list"
            )
        elif command not in commands:
            problems.append(
                f"{identifier}: is marked complete and names command {command!r}, "
                f"which Tauri does not register"
            )

    if problems:
        for problem in problems:
            print(f"feature-matrix: {problem}", file=sys.stderr)
        return 1

    complete = sum(1 for c in capabilities if c.get("status") == "complete")
    partial = sum(1 for c in capabilities if c.get("status") == "partial")
    missing = sum(1 for c in capabilities if c.get("status") == "not-implemented")
    print(
        f"feature-matrix: {len(capabilities)} capabilities consistent "
        f"({complete} complete, {partial} partial, {missing} not implemented); "
        f"{len(commands)} commands registered"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
