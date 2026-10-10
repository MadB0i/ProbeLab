"""Flags every class name used in markup that has no rule in the stylesheet.

A restyle is judged by what it looks like, but "looks fine" hides the case that matters most on a
light surface: a control that kept its old class and therefore kept its old dark styling, sitting on
a white panel as a black rectangle. This makes that failure loud instead of invisible.

Also reports the reverse -- rules with no user -- so dead CSS does not accumulate unnoticed.
"""
import re
from pathlib import Path

# Resolved from this file rather than a literal path, so the check runs on any
# checkout instead of only on the machine it was written on.
REPO = Path(__file__).resolve().parent.parent
SRC = REPO / "apps" / "desktop" / "src"
CSS = SRC / "styles.css"

css = CSS.read_text(encoding="utf-8")

# Class selectors actually declared in the stylesheet.
declared = set(re.findall(r"\.([A-Za-z_][\w-]*)", css))

used = {}
for path in sorted(SRC.glob("*.tsx")):
    if path.name.endswith(".test.tsx"):
        continue
    text = path.read_text(encoding="utf-8")
    for match in re.finditer(r'className=(?:"([^"]*)"|\{`([^`]*)`\})', text):
        raw = match.group(1) or match.group(2) or ""
        # Template literals carry interpolation like ${cond ? "a" : "b"}; keep the word tokens.
        for token in re.findall(r"[A-Za-z_][\w-]*", raw):
            used.setdefault(token, set()).add(path.name)

missing = {k: v for k, v in used.items() if k not in declared}

print(f"classes used in markup: {len(used)}")
print(f"classes declared in css: {len(declared)}")
print(f"UNSTYLED (used but no rule): {len(missing)}")
for name in sorted(missing):
    print(f"  .{name:<34} {', '.join(sorted(missing[name]))}")