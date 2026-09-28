/*
 * Enforces the legibility floor in `src/styles.css`: no rendered font size below 11px.
 *
 * This exists because the floor was previously only asserted in a CSS comment, and that assertion was
 * false for a long time. The check that produced the original count matched `font-size: 7px` but not
 * `font-size: 0.6rem`; the stylesheet mixes both units, so 58 sub-floor rules went unseen while a
 * comment claimed there were none. A comment cannot fail, so nothing noticed.
 *
 * Both units are converted to a common base before comparing, and both `rem` and `px` are accepted on
 * input. `clamp()`/`calc()` values are reported rather than skipped: they are all above the floor today,
 * and a silent skip is how the last version of this check came to pass.
 */
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const FLOOR_PX = 11;
const ROOT_PX = 16; // CSS initial font-size, so 1rem is 16px unless the user scales the page.
const HERE = dirname(fileURLToPath(import.meta.url));
const STYLESHEET = join(HERE, "..", "src", "styles.css");

const source = readFileSync(STYLESHEET, "utf8");

/** Selector of the rule a declaration belongs to, so a failure names the rule to fix. */
function selectorFor(index) {
  const before = source.slice(0, index);
  const open = before.lastIndexOf("{");
  if (open < 0) return "<unknown>";
  const head = before.slice(before.lastIndexOf("}", open) + 1, open);
  return (
    head
      .split("\n")
      .map((line) => line.trim())
      .filter(Boolean)
      .pop() ?? "<unknown>"
  );
}

const violations = [];
const functional = [];

/*
 * Matches a plain numeric size in either unit. Functional values (`clamp(...)`, `calc(...)`) do not
 * match the trailing unit group, so they are not silently accepted as compliant -- they are collected
 * separately below and reported for a human to read.
 */
const plain = /font-size:\s*(\d+(?:\.\d+)?)(rem|px)\s*;/g;
for (const match of source.matchAll(plain)) {
  const px = match[2] === "rem" ? Number(match[1]) * ROOT_PX : Number(match[1]);
  if (px < FLOOR_PX) {
    violations.push({
      selector: selectorFor(match.index),
      declaration: `font-size: ${match[1]}${match[2]};`,
      px,
    });
  }
}

const functionalSize = /font-size:\s*((?:clamp|calc|min|max)\([^;]*\))\s*;/g;
for (const match of source.matchAll(functionalSize)) {
  const lowest = Math.min(
    ...[...match[1].matchAll(/(\d+(?:\.\d+)?)(rem|px)/g)].map((size) =>
      size[2] === "rem" ? Number(size[1]) * ROOT_PX : Number(size[1]),
    ),
  );
  if (Number.isFinite(lowest) && lowest < FLOOR_PX) {
    violations.push({
      selector: selectorFor(match.index),
      declaration: `font-size: ${match[1]};`,
      px: lowest,
    });
  } else {
    functional.push(`${selectorFor(match.index)}: ${match[1]}`);
  }
}

if (violations.length > 0) {
  console.error(
    `Legibility floor: ${violations.length} rule(s) below ${FLOOR_PX}px in styles.css.\n` +
      "These carry labels on measured values, so a label nobody can resolve is a number nobody can\n" +
      "attribute. Raise them, or write the reason next to the rule.\n",
  );
  for (const violation of violations) {
    console.error(`  ${violation.selector} { ${violation.declaration} }  (${violation.px}px)`);
  }
  process.exit(1);
}

console.log(
  `Legibility floor: OK, no font size below ${FLOOR_PX}px.`,
  functional.length > 0 ? `${functional.length} functional size(s) checked: ${functional.join(", ")}` : "",
);
