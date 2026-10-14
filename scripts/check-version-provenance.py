#!/usr/bin/env python3
"""
Checks that the project's version is declared once and agrees everywhere.

# Why this exists

The version lives in three files that nothing keeps in step: the Cargo workspace manifest, the Tauri
configuration, and the frontend's package.json. A build takes the Rust crate's version into the binary
and the Tauri configuration's version into the installer, so if those two disagree the application and
its installer report different versions and nobody notices until a bug report quotes one of them.

Three declarations and no check between them is three chances to be wrong. This is the check that
makes the disagreement loud, and it runs on every commit rather than at release time.

# What it does not do

It does not bump the version, and it does not know what the next version should be. Cutting a release is
a decision about what has changed, and it is made by a person, not by a script that notices three files
disagree.

Usage: python3 scripts/check-version-provenance.py
"""
from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DESKTOP = ROOT / "apps" / "desktop"

# Cargo's workspace version, which every crate in the workspace inherits.
CARGO = ROOT / "Cargo.toml"
TAURI = DESKTOP / "src-tauri" / "tauri.conf.json"
PACKAGE = DESKTOP / "package.json"
STATUS = ROOT / "docs" / "STATUS.md"
ARCHITECTURE = ROOT / "ARCHITECTURE.md"

# Semver with an optional pre-release and build suffix, which is what Cargo accepts here too.
VERSION_PATTERN = re.compile(r"^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$")


def cargo_version() -> str | None:
    """Reads `[workspace.package] version` out of the root manifest."""
    text = CARGO.read_text(encoding="utf-8")
    in_workspace_package = False
    for raw in text.splitlines():
        line = raw.split("#", 1)[0].strip()
        if line.startswith("["):
            in_workspace_package = line == "[workspace.package]"
            continue
        if in_workspace_package and line.startswith("version"):
            _, _, value = line.partition("=")
            return value.strip().strip('"')
    return None


def tauri_version() -> str | None:
    return json.loads(TAURI.read_text(encoding="utf-8")).get("version")


def package_version() -> str | None:
    return json.loads(PACKAGE.read_text(encoding="utf-8")).get("version")


VERSION_CLAIM = re.compile(r"\bv?\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?\b")
VERSION_WORD = re.compile(r"version|release", re.IGNORECASE)


def version_claims(path: Path) -> list[tuple[str, str]]:
    """
    Every version number in a document that sits on a line talking about a version.

    Returns the number and the line it came from, so a mismatch can quote what it is disagreeing with
    instead of only reporting a number.
    """
    if not path.exists():
        return []
    claims: list[tuple[str, str]] = []
    for line in path.read_text(encoding="utf-8").splitlines():
        if not VERSION_WORD.search(line):
            continue
        for number in VERSION_CLAIM.findall(line):
            claims.append((number, line))
    return claims


def main() -> int:
    problems: list[str] = []

    cargo = cargo_version()
    tauri = tauri_version()
    package = package_version()

    declared = {
        "Cargo.toml [workspace.package].version": cargo,
        "apps/desktop/src-tauri/tauri.conf.json version": tauri,
        "apps/desktop/package.json version": package,
    }

    print("Version provenance")
    for source, value in declared.items():
        print(f"  {value or 'MISSING':<12} {source}")
        if value is None:
            problems.append(f"{source} declares no version")
        elif not VERSION_PATTERN.match(value):
            problems.append(f"{source} declares {value!r}, which is not a semantic version")

    distinct = {value for value in declared.values() if value is not None}
    if len(distinct) > 1:
        problems.append(
            "the version disagrees between files: "
            + ", ".join(f"{source} says {value}" for source, value in declared.items())
        )
    elif distinct:
        version = next(iter(distinct))
        bare = version.lstrip("v")

        # `docs/STATUS.md` is the document this repository nominates as its single source of truth for
        # state, so every version it states on a line that talks about a version has to be the current
        # one. That is how the stale `0.4.1` was found: it was not floating in a paragraph, it was the
        # document's own answer to "what version is this".
        #
        # Reading every number in the file instead would report the Rust toolchain (1.97.0) and the
        # previous release (0.4.1) as disagreements, and a check that cries wolf is a check that gets
        # switched off.
        for number, line in version_claims(STATUS):
            if number.lstrip("v") != bare:
                problems.append(
                    f"docs/STATUS.md states version {number}, but the build declares {version}: "
                    f"{line.strip()}"
                )

        # `ARCHITECTURE.md` is held to a weaker rule on purpose: it quotes historical versions on
        # purpose ("the security sections describe v0.4.1 and are unchanged by v0.5"), so a
        # line-by-line rule would fire on accurate history. What it must do is name the version being
        # described at all, which is the failure mode when a release is cut and the header is missed.
        arch_claims = version_claims(ARCHITECTURE)
        if ARCHITECTURE.exists() and arch_claims and not any(
            number.lstrip("v") == bare for number, _ in arch_claims
        ):
            problems.append(
                f"ARCHITECTURE.md describes {arch_claims[0][0]}, but the build declares {version}"
            )

    if problems:
        print("")
        for problem in problems:
            print(f"FAIL  {problem}")
        print(f"\nVersion provenance: {len(problems)} problem(s)")
        return 1

    print(f"\nVersion provenance: every declaration agrees on {next(iter(distinct))}.")
    return 0


if __name__ == "__main__":
    sys.exit(main())