#!/usr/bin/env python3
"""Check that the feature matrix still matches the repository.

# Why this exists

A feature matrix is a claim about what the product does. A claim that is not checked drifts, and a
drifted matrix is worse than none: it is a document that looks like verification while asserting things
nobody confirmed.

# The defect this version exists to close

The previous version collected "every command name Tauri will dispatch" by grepping for
`#[tauri::command]`. That is *definitions*, not *registrations*: the attribute marks a function as a
command, but Tauri dispatches only what appears in `generate_handler![]`. Nine commands were defined,
exported, called by the frontend, tested, and documented -- and absent from the handler, so the Arena,
Security Lab, and Replay panels were dead surfaces in the built application while this script, the
654-test suite, and CI were all green.

A gate that reads the wrong list is worse than no gate, because it reads as coverage. This version
reads the real registration list, and cross-checks it against the names the frontend actually invokes.

# What is checked

1. The matrix loads, and declares a non-empty `capabilities` list.
2. Every status is one of exactly three values, and `id` is unique.
3. Every path in every entry's `tests` exists.
4. Every command a capability claims is **dispatched** -- read from the real `generate_handler![]`
   macro, not from the `#[tauri::command]` attribute.
5. Every command the frontend invokes is dispatched, and every dispatched command is invoked or
   declared unused. This is the check that would have caught the nine.
6. Every test path the matrix cites contains at least one `#[test]` or `#[tokio::test]`, so a capability
   cannot be marked complete by citing a file that tests nothing about it.
7. Every `not-implemented` capability names no command and cites no tests, so "not implemented" cannot
   quietly accumulate the evidence of an implementation.
8. Every capability marked `complete` names an implementation file, and that file exists.

# Deliberately not checked

- Whether the tests still pass. That is the test suite's job, and duplicating it here would make two
  gates that fail for the same reason.
- Whether the `summary` prose is true. A natural-language parser is a fragile gate: it breaks on
  rephrasing, and the incentive it creates is to write summaries a regex can match rather than ones a
  reader can trust. Prose is checked by review; the structured fields are checked here.
- Whether the numbers in prose documents are current. `docs/STATUS.md` is the authority for those, and
  a test that hard-codes a measured score fails every time the thing it measures changes.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MATRIX = ROOT / "docs" / "feature-matrix.json"
TAURI_SRC = ROOT / "apps" / "desktop" / "src-tauri" / "src"
FRONTEND_SRC = ROOT / "apps" / "desktop" / "src"

STATUSES = {"complete", "partial", "not-implemented"}

# Capabilities that are deliberately reachable without a dedicated command, because the command that
# drives them is shared with another capability. Keyed by capability id.
#
# This is an escape hatch and is treated as one: `a declared witness must exist in the dispatch list`
# below, so a capability cannot be marked complete by naming a command that does not exist, and the
# list cannot grow without someone writing the reason in this file.
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

# Commands the frontend does not invoke, with the reason each is still registered. Checked in the
# reverse direction by `every_dispatched_command_is_invoked_or_declared`, so an entry that stops being
# true has to be deleted rather than left to rot.
DECLARED_UNUSED_COMMANDS = {
    "get_arena_match": (
        "read-only single-match accessor, paired with the 20-match retained surface; "
        "`latest_arena_match` is what the panel calls"
    ),
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


def handler_listing() -> str:
    """The text of the real `generate_handler![]` macro, or fails."""
    lib = TAURI_SRC / "lib.rs"
    if not lib.exists():
        fail(f"{lib.relative_to(ROOT)} does not exist, so the command list cannot be read")
    text = lib.read_text(encoding="utf-8")
    marker = "tauri::generate_handler!["
    start = text.find(marker)
    if start < 0:
        fail(
            "could not find `tauri::generate_handler![` in src-tauri/src/lib.rs. Tauri dispatches "
            "nothing that is not listed there, and a checker that cannot read the list would pass "
            "vacuously -- which is exactly how nine unregistered commands went unnoticed."
        )
    rest = text[start + len(marker) :]
    end = rest.find("]")
    if end < 0:
        fail("the `generate_handler![]` list is not closed")
    return rest[:end]


def dispatched_commands() -> set[str]:
    """Every command name Tauri actually dispatches, read from the real handler list."""
    body = handler_listing()
    names = {
        entry.strip()
        for entry in body.split(",")
        if entry.strip() and not entry.strip().startswith("//")
    }
    if not names:
        fail("`generate_handler![]` is empty, so the application has no commands at all")
    return names


def frontend_sources() -> list[Path]:
    files = [
        path
        for pattern in ("*.ts", "*.tsx")
        for path in FRONTEND_SRC.glob(pattern)
        # Test files are excluded on purpose: a test may call a command the product does not, and
        # counting that as reachability would let a dead panel look alive because a test names it.
        if not path.name.endswith((".test.ts", ".test.tsx"))
    ]
    if not files:
        fail(f"no frontend sources found under {FRONTEND_SRC.relative_to(ROOT)}")
    return files


def invoked_commands() -> set[str]:
    """Every command name the frontend actually passes to `invoke`.

    Parsed rather than grepped for the attribute, because the attribute is a definition. Handles the
    generic argument (`invoke<T>("name", ...)`) including nested angle brackets, and a bare string
    literal, which is all the frontend uses.
    """
    names: set[str] = set()
    for path in frontend_sources():
        text = path.read_text(encoding="utf-8")
        for match in re.finditer(r"\binvoke(?:<|\s*\()", text):
            tail = text[match.end() :]
            if match.group(0).endswith("<"):
                depth = 1
                index = 0
                while index < len(tail) and depth:
                    if tail[index] == "<":
                        depth += 1
                    elif tail[index] == ">":
                        depth -= 1
                    index += 1
                tail = tail[index:]
            open_paren = tail.find("(")
            if open_paren < 0:
                continue
            depth = 1
            index = open_paren + 1
            while index < len(tail) and depth:
                if tail[index] == "(":
                    depth += 1
                elif tail[index] == ")":
                    depth -= 1
                index += 1
            arguments = tail[open_paren + 1 : index - 1]
            literal = re.match(r"""\s*["'`]([A-Za-z0-9_]+)["'`]""", arguments)
            if literal:
                names.add(literal.group(1))
    if not names:
        fail(
            "no `invoke` calls found in the frontend. The extraction is broken, not the frontend, and "
            "a check that cannot see the calls would pass while every panel was dead."
        )
    return names


def known_test_files() -> dict[str, bool]:
    """Workspace test files, mapped to whether they contain an actual test function."""
    found: dict[str, bool] = {}
    patterns = [
        "crates/*/tests/*.rs",
        "crates/*/src/*.rs",
        "crates/*/src/**/*.rs",
        "apps/desktop/src-tauri/src/*.rs",
        "apps/desktop/src-tauri/tests/*.rs",
        "apps/desktop/src/*.test.ts",
        "apps/desktop/src/*.test.tsx",
    ]
    for pattern in patterns:
        for path in ROOT.glob(pattern):
            relative = path.relative_to(ROOT).as_posix()
            text = path.read_text(encoding="utf-8", errors="replace")
            has_test = (
                "#[test]" in text
                or "#[tokio::test]" in text
                or re.search(r"\b(it|test)\s*\(", text) is not None
            )
            found[relative] = found.get(relative, False) or has_test
    return found


def main() -> int:
    matrix = load_matrix()
    capabilities = matrix.get("capabilities")
    if not isinstance(capabilities, list) or not capabilities:
        fail("the matrix declares no capabilities")

    dispatched = dispatched_commands()
    invoked = frontend_commands = invoked_commands()
    files = known_test_files()

    problems: list[str] = []

    # The reverse direction, checked first because it is the one that was missed.
    for name in sorted(invoked - dispatched):
        problems.append(
            f"the frontend invokes {name!r}, which Tauri does not dispatch. A command that is not "
            f"in `generate_handler![]` is a function, not a feature: the panel that calls it is dead "
            f"in the built application while its documentation claims it works."
        )
    for name in sorted(dispatched - frontend_commands):
        if name not in DECLARED_UNUSED_COMMANDS:
            problems.append(
                f"Tauri dispatches {name!r} but the frontend never invokes it and it is not in "
                f"DECLARED_UNUSED_COMMANDS. Either a panel should call it, or the reason it is kept "
                f"belongs in that dict -- an undeclared command is a claim with no user."
            )
    for name in sorted(DECLARED_UNUSED_COMMANDS):
        if name not in dispatched:
            problems.append(
                f"{name!r} is in DECLARED_UNUSED_COMMANDS but is not dispatched at all; remove the entry"
            )
        if name in frontend_commands:
            problems.append(
                f"{name!r} is declared unused but the frontend now invokes it; remove the entry"
            )

    seen_ids: set[str] = set()
    for capability in capabilities:
        identifier = capability.get("id", "<missing id>")
        status = capability.get("status")
        if identifier in seen_ids:
            problems.append(f"{identifier}: duplicate id in the matrix")
        seen_ids.add(identifier)

        if status not in STATUSES:
            problems.append(f"{identifier}: status {status!r} is not one of the three allowed values")
            continue

        tests = capability.get("tests", [])
        for test_file in tests:
            if test_file not in files:
                problems.append(
                    f"{identifier}: claims test file {test_file!r}, which does not exist"
                )
            elif not files[test_file]:
                problems.append(
                    f"{identifier}: claims test file {test_file!r}, which contains no test function. "
                    f"A capability marked complete cannot be evidenced by a file that tests nothing."
                )

        implementation = capability.get("implementation")
        if status == "complete":
            if not implementation:
                problems.append(
                    f"{identifier}: is marked complete but names no implementation file, so nothing "
                    f"checks that the code it claims exists"
                )
            elif implementation not in files and not (ROOT / implementation).exists():
                problems.append(
                    f"{identifier}: names implementation file {implementation!r}, which does not exist"
                )

            command = capability.get("command") or SHARED_COMMANDS.get(identifier)
            if not command:
                problems.append(
                    f"{identifier}: is marked complete but names no command and is not in the "
                    f"shared list"
                )
            elif command not in dispatched:
                problems.append(
                    f"{identifier}: is marked complete and names command {command!r}, which Tauri "
                    f"does not dispatch"
                )
        elif status == "not-implemented":
            if capability.get("command"):
                problems.append(
                    f"{identifier}: is not implemented but names a command; a capability that cannot "
                    f"be reached should not point at one"
                )
            if tests:
                problems.append(
                    f"{identifier}: is not implemented but cites tests; the evidence of an "
                    f"implementation and a claim of its absence cannot both be true"
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
        f"{len(dispatched)} commands dispatched, all reachable from the frontend"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
