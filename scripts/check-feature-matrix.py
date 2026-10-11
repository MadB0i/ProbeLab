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
#
# Empty. `get_arena_match` used to sit here, because the Arena only asked for the most recent match and
# the per-match accessor had no caller. Reopening a stored match is now the point of keeping an archive,
# so the command is bound and the exception is gone.
DECLARED_UNUSED_COMMANDS: dict[str, str] = {}


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
    return {name for (name, _keys) in invoked_calls()}


def invoked_calls() -> list[tuple[str, set[str] | None]]:
    """Every `invoke` call: the command name and its top-level argument keys.

    The keys are `None` when the payload is not an object literal (a variable
    or an absent second argument), which is unverifiable rather than wrong.
    String contents are skipped so a value like `{ format: "csv" }` does not
    read `"csv"` as a key.
    """
    calls: list[tuple[str, set[str] | None]] = []
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
                calls.append((literal.group(1), top_level_keys(arguments[len(literal.group(0)) :])))
    if not calls:
        fail(
            "no `invoke` calls found in the frontend. The extraction is broken, not the frontend, and "
            "a check that cannot see the calls would pass while every panel was dead."
        )
    return calls


def top_level_keys(after_name: str) -> set[str] | None:
    """The keys of the payload object literal following a command name.

    Returns `None` when there is no object literal to read (absent payload or
    a variable), which the caller treats as unverifiable rather than wrong.
    """
    stripped = after_name.lstrip()
    if not stripped.startswith(","):
        return set()
    rest = stripped[1:].lstrip()
    if not rest.startswith("{"):
        return None
    keys: set[str] = set()
    depth = 0
    index = 0
    in_string: str | None = None
    while index < len(rest):
        char = rest[index]
        if in_string is not None:
            if char == "\\":
                index += 2
                continue
            if char == in_string:
                in_string = None
            index += 1
            continue
        if char in "\"'`":
            in_string = char
            index += 1
            continue
        if char in "{[(":
            depth += 1
            index += 1
            continue
        if char in "}])":
            depth -= 1
            if depth <= 0:
                break
            index += 1
            continue
        if depth == 1:
            key = re.match(r"[A-Za-z0-9_]+", rest[index:])
            if key:
                word = key.group(0)
                after = rest[index + len(word) :].lstrip()
                if after.startswith(":"):
                    # `key: value` — record the key, then skip the whole value
                    # expression so a value identifier (`{ experiment_id:
                    # experimentId }`) is never mistaken for a second key.
                    keys.add(word)
                    index = skip_value(rest, index + len(word) + 1)
                    continue
                if after[:1] in ",}":
                    # Shorthand `{ request }`.
                    keys.add(word)
                index += len(word)
                continue
        index += 1
    return keys


def skip_value(text: str, index: int) -> int:
    """Skips one value expression, returning the index after it.

    Stops at a top-level comma or closing brace — the two things that can
    follow a value inside an object literal.
    """
    depth = 0
    in_string: str | None = None
    while index < len(text):
        char = text[index]
        if in_string is not None:
            if char == "\\":
                index += 2
                continue
            if char == in_string:
                in_string = None
            index += 1
            continue
        if char in "\"'`":
            in_string = char
            index += 1
            continue
        if char in "{[(":
            depth += 1
        elif char in "}])":
            if depth == 0:
                return index
            depth -= 1
        elif char == "," and depth == 0:
            return index + 1
        index += 1
    return index


def to_camel(name: str) -> str:
    """`experiment_id` to `experimentId`, for commands without a case policy."""
    parts = name.split("_")
    return parts[0] + "".join(part[:1].upper() + part[1:] for part in parts[1:])


# Parameter types Tauri injects rather than reading from the payload. A key
# with one of these base types is resolved from the runtime, never from JSON,
# so the frontend sends nothing for it and the checker must expect nothing.
INJECTED_TYPES = {"State", "AppHandle", "App", "Window", "Webview", "WebviewWindow"}


def rust_command_params() -> dict[str, set[str]]:
    """Every Tauri command's expected top-level argument keys.

    Read from the real `pub fn` signatures, not from documentation. Injectable
    parameters (`State`, `AppHandle`, windows) are excluded, and the command's
    own case policy (`rename_all = "snake_case"`, default camelCase) decides
    the spelling — the exact contract the webview must speak.
    """
    params: dict[str, set[str]] = {}
    for path in TAURI_SRC.glob("*.rs"):
        text = path.read_text(encoding="utf-8")
        for match in re.finditer(
            r"#\[tauri::command(?P<attr>[^\]]*)\]\s*pub\s+(?:async\s+)?fn\s+(?P<name>[A-Za-z0-9_]+)\s*\(",
            text,
        ):
            snake = 'rename_all = "snake_case"' in (match.group("attr") or "")
            depth = 1
            index = match.end()
            while index < len(text) and depth:
                if text[index] == "(":
                    depth += 1
                elif text[index] == ")":
                    depth -= 1
                index += 1
            signature = text[match.end() : index - 1]
            keys: set[str] = set()
            for part in split_top_level(signature, ","):
                part = part.strip()
                param = re.match(r"(?:mut\s+)?([A-Za-z0-9_]+)\s*:\s*(.+)", part, re.DOTALL)
                if not param:
                    continue
                ident, typename = param.group(1), param.group(2).strip()
                # Strip a module path before reading the type name: a command written as
                # `app: tauri::AppHandle` is the same injectable as one written `app: AppHandle`, and
                # treating the path as the type made the gate demand the frontend send something Tauri
                # injects itself.
                bare = typename.rsplit("::", 1)[-1]
                base = re.split(r"[<>,:\s&'\"]", bare, maxsplit=1)[0]
                if base in INJECTED_TYPES:
                    continue
                keys.add(ident if snake else to_camel(ident))
            params[match.group("name")] = keys
    if not params:
        fail("no Tauri commands found in src-tauri/src; the signature reader is broken")
    return params


def split_top_level(text: str, separator: str) -> list[str]:
    """Splits on a separator that is not nested in brackets or strings.

    A single quote starts a string only when it cannot be a Rust lifetime or
    char literal: `'_` and `'a` (followed by anything but a closing quote) are
    lifetimes, `'x'` is a char literal. Without this, `State<'_, AppState>`
    swallows the rest of a command signature and every parameter after it is
    lost — which is precisely the kind of silent parser miss this checker
    exists to avoid becoming.
    """
    parts: list[str] = []
    depth = 0
    in_string: str | None = None
    current: list[str] = []
    index = 0
    while index < len(text):
        char = text[index]
        if in_string is not None:
            current.append(char)
            if char == "\\":
                if index + 1 < len(text):
                    current.append(text[index + 1])
                    index += 1
            elif char == in_string:
                in_string = None
            index += 1
            continue
        if char == "'" and _quote_is_rust_string(text, index):
            in_string = char
            current.append(char)
            index += 1
            continue
        if char == "'" :
            # A Rust lifetime (`'_`, `'a`) or char literal (`'x'`): neither
            # opens a string. Lifetimes pass through as ordinary characters;
            # char literals are skipped whole so a comma inside one (unlikely
            # but legal) cannot split a signature.
            if _quote_is_char_literal(text, index):
                current.append(text[index : index + 3])
                index += 3
            else:
                current.append(char)
                index += 1
            continue
        if char == '"':
            in_string = char
            current.append(char)
            index += 1
            continue
        if char in "(<[{":
            depth += 1
        elif char in ")>]}":
            depth -= 1
        if char == separator and depth == 0:
            parts.append("".join(current))
            current = []
        else:
            current.append(char)
        index += 1
    parts.append("".join(current))
    return parts


def _quote_is_rust_string(text: str, index: int) -> bool:
    """Whether a single quote opens a string rather than a lifetime/literal.

    A quote followed by an identifier character starts a lifetime (`'_`,
    `'a`) or a char literal (`'x'`), never a string. Anything else opens one.
    """
    nxt = text[index + 1 : index + 2]
    return not (nxt.isascii() and (nxt.isalnum() or nxt == "_"))


def _quote_is_char_literal(text: str, index: int) -> bool:
    """Whether the quote starts a `'x'` char literal (skipped whole)."""
    nxt = text[index + 1 : index + 2]
    return (
        nxt.isascii()
        and (nxt.isalnum() or nxt == "_")
        and text[index + 2 : index + 3] == "'"
    )


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
    calls = invoked_calls()
    invoked = frontend_commands = {name for (name, _keys) in calls}
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

    # Top-level argument keys, in both directions. Tauri matches each key
    # exactly under the command's own case policy (`rename_all =
    # "snake_case"` here almost everywhere), so a camelCase key where a
    # snake_case one is expected is a missing argument at runtime — six call
    # sites shipped exactly that while every suite was green. Calls whose
    # payload is a variable rather than a literal are unverifiable, not wrong.
    expected_params = rust_command_params()
    for name, keys in calls:
        if keys is None or name not in expected_params:
            continue
        expected = expected_params[name]
        for key in sorted(keys - expected):
            problems.append(
                f"{name}: the frontend sends argument {key!r}, which the command does not declare. "
                f"Tauri matches argument keys exactly under the command's case policy, so this is a "
                f"missing argument at runtime, not a cosmetic mismatch."
            )
        for key in sorted(expected - keys):
            problems.append(
                f"{name}: the frontend never sends required argument {key!r}. "
                f"A call without it fails argument resolution before the command body runs."
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
