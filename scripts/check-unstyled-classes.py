"""Flags every class name used in markup that has no rule in the stylesheet.

A restyle is judged by what it looks like, but "looks fine" hides the case that matters most on a
light surface: a control that kept its old class and therefore kept its old dark styling, sitting on
a white panel as a black rectangle. This makes that failure loud instead of invisible.

Also reports the reverse -- rules with no user -- so dead CSS does not accumulate unnoticed.

The tokenizer reads only what can actually be a class name. Interpolations are treated as data, not
as markup: `${outcomeTone(x)}` is a call, not a class, and counting it produced 51 phantom misses on
a sheet where none existed. String literals inside an interpolation are kept, because
`className={`cell ${ready ? "ready" : "not-ready"}`}` really does put those two names in the DOM.
"""
import re
import sys
from pathlib import Path

# Resolved from this file rather than a literal path, so the check runs on any
# checkout instead of only the machine it was written on.
REPO = Path(__file__).resolve().parent.parent
SRC = REPO / "apps" / "desktop" / "src"
CSS = SRC / "styles.css"

WORD = r"[A-Za-z_][\w-]*"
QUOTE = r'"([^"]*)"|\'([^\']*)\''

css = CSS.read_text(encoding="utf-8")

# Class selectors actually declared in the stylesheet.
declared = set(re.findall(r"\.([A-Za-z_][\w-]*)", css))


def quoted_tokens(raw):
    """Names inside string literals -- the arms of a ternary, the entries of a class array.

    A literal that is being compared rather than assigned (`capability === "full"`) names a value,
    not a class, and it is dropped before the scan so it cannot register as a miss.
    """
    raw = re.sub(r"[!=]==?\s*(?:\"[^\"]*\"|'[^']*')", " ", raw)
    raw = re.sub(r"(?:\"[^\"]*\"|'[^']*')\s*[!=]==?", " ", raw)
    out = []
    for double, single in re.findall(QUOTE, raw):
        out += re.findall(WORD, double or single)
    return {tok.rstrip("-") for tok in out if tok.rstrip("-")}


def static_tokens(raw):
    """Literal text in a template or a plain attribute -- everything that is not an expression."""
    return {tok.rstrip("-") for tok in re.findall(WORD, re.sub(r"\$\{[^}]*\}", " ", raw)) if tok}


def interpolation_prefixes(raw):
    """Class names built by interpolation, reduced to the literal part before the hole.

    `arena-cell--${cell}` is a class in the DOM; the audit only sees the prefix unless the
    interpolation is kept. Recording the prefix stops the suffix variants from looking unused
    while still refusing to guess the full name.
    """
    prefixes = set()
    for match in re.finditer(r"([A-Za-z_][\w-]*)\$\{", raw):
        prefixes.add(match.group(1).rstrip("-"))
    return prefixes


def brace_expressions(text):
    """The content of every `className={...}` argument, braces balanced."""
    for match in re.finditer(r"className=\{", text):
        depth, index = 1, match.end()
        while index < len(text) and depth:
            if text[index] == "{":
                depth += 1
            elif text[index] == "}":
                depth -= 1
            index += 1
        yield text[match.end() : index - 1]


used = {}
dynamic = set()
for path in sorted(SRC.glob("*.tsx")):
    if path.name.endswith(".test.tsx"):
        continue
    text = path.read_text(encoding="utf-8")
    for match in re.finditer(r'className="([^"]*)"', text):
        for token in static_tokens(match.group(1)):
            used.setdefault(token, set()).add(path.name)
    for expression in brace_expressions(text):
        stripped = expression.strip()
        if stripped.startswith("`"):
            # Template literal: both the fixed segments and any quoted arms of a ternary.
            found = static_tokens(expression) | quoted_tokens(expression)
        elif stripped.startswith("["):
            # An array of literals is class names by construction. Bare identifiers in it are
            # call names (`.filter(Boolean).join(" ")`), not classes, so only quotes count.
            found = quoted_tokens(expression)
        elif '"' in stripped or "'" in stripped:
            found = quoted_tokens(expression)
        else:
            # A class list produced by a call this audit cannot resolve.
            dynamic.add(stripped.split("\n")[0].strip()[:60])
            continue
        for token in found:
            used.setdefault(token, set()).add(path.name)

missing = {k: v for k, v in used.items() if k not in declared}

# A rule nobody uses is a rule nobody can check. Hover/active states are written in CSS alone, so
# this half is reported rather than enforced.
present = set()
prefixes = set()
for path in SRC.glob("*.tsx"):
    raw = path.read_text(encoding="utf-8")
    present |= {tok.rstrip("-") for tok in re.findall(WORD, raw)}
    prefixes |= interpolation_prefixes(raw)
unused = sorted(d for d in declared if d not in present and not any(d.startswith(p) for p in prefixes))

print(f"classes used in markup: {len(used)}")
print(f"classes declared in css: {len(declared)}")
print(f"UNSTYLED (used but no rule): {len(missing)}")
for name in sorted(missing):
    print(f"  .{name:<34} {', '.join(sorted(missing[name]))}")
print(f"UNUSED (declared, never assigned): {len(unused)}")
for name in unused:
    print(f"  .{name}")
print(f"NOT AUDITED (className built by a call): {len(dynamic)}")
for expression in sorted(dynamic):
    print(f"  {expression}")

sys.exit(1 if missing else 0)
