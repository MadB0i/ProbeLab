/*
 * Enforces the colour contrast contract on `src/styles.css`: every text token must clear 4.5:1 against
 * every background in the system it can land on.
 *
 * # Why this check exists
 *
 * `--ink-3` and `--ink-4` were tuned against the panel they were designed in and shipped without ever
 * being measured against the surfaces they also appear on. The result was text at 2.72:1 on a tinted
 * chip and 3.06:1 on a panel, spread across sixty rules on nine routes — none of which any check had
 * visited, because the accessibility audit in this project scanned the same two routes for the whole
 * of its life.
 *
 * Fixing those sixty sites by hand is the obvious move and the wrong one: the next person who adds a
 * chip in a tint has no way of knowing the floor. So the tokens themselves carry the guarantee, and this
 * is what holds them to it. Change `--ink-4` and this fails in CI, naming the surface that failed and
 * the ratio it measured, rather than a human noticing a rendering weeks later.
 *
 * # The trade this check forced
 *
 * A fourth gray dark enough to read as clearly "dimmer" than tertiary text cannot clear 4.5:1 on this
 * palette's hovered and pressed surfaces — 4.33:1 and 3.88:1, both measured while writing this file. So
 * the meta tier was lifted until every pairing passed, which compresses the distance between "meta" and
 * "placeholder" from sixty steps of red to a handful. That is a real cost to the visual hierarchy and it
 * is the correct cost: a token that reads as dimmer at the price of being unreadable is not a tier.
 * The hierarchy between those two roles is now carried by weight and size, which is what carries it in
 * every design system that has ever tried this.
 */
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const AA_NORMAL = 4.5; // WCAG 2.1 SC 1.4.3, the floor for text below 18.66px bold / 24px
const HERE = dirname(fileURLToPath(import.meta.url));
const STYLESHEET = join(HERE, "..", "src", "styles.css");

const source = readFileSync(STYLESHEET, "utf8");

/** Reads one custom property out of the `:root` block. */
function token(name) {
  const match = source.match(new RegExp(`--${name}:\\s*(#[0-9a-fA-F]{3,8})`));
  if (!match) {
    throw new Error(`--${name} is not declared as a hex colour in styles.css`);
  }
  return match[1];
}

/**
 * WCAG 2.1 relative luminance, straight from the specification.
 *
 * The 0.03928 threshold is the step implementations usually get wrong, and it changes the answer for
 * exactly the darkest colours under test — the wells.
 */
function luminance(hex) {
  const full =
    hex.length === 4
      ? `#${hex[1]}${hex[1]}${hex[2]}${hex[2]}${hex[3]}${hex[3]}`
      : hex;
  const channels = [1, 3, 5].map((offset) => {
    const value = parseInt(full.slice(offset, offset + 2), 16) / 255;
    return value <= 0.03928 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * channels[0] + 0.7152 * channels[1] + 0.0722 * channels[2];
}

function contrast(foreground, background) {
  const a = luminance(foreground);
  const b = luminance(background);
  const [light, dark] = a > b ? [a, b] : [b, a];
  return (light + 0.05) / (dark + 0.05);
}

/** Every background a text token can be placed on, grouped by the role it plays. */
const PANEL_BACKGROUNDS = [
  "surface-1",
  "surface-2",
  "surface-3",
  "surface-4",
  "surface-inset",
  "surface-sunken",
];

const TINT_BACKGROUNDS = ["blue-tint", "green-tint", "amber-tint", "red-tint"];

/** The two tokens used for text at 13px or below. */
const TEXT_TOKENS = ["ink-3", "ink-4"];

const failures = [];

/**
 * Records a failure against one pairing.
 *
 * `what` is carried through to the message because "ink-4 on surface-4" is not actionable on its own
 * without saying whether it is the hover state or a pressed row that is affected.
 */
function verify(foregroundToken, backgroundToken, minimum = AA_NORMAL, what = "") {
  const fg = token(foregroundToken);
  const bg = token(backgroundToken);
  const ratio = contrast(fg, bg);
  if (ratio < minimum) {
    failures.push(
      `--${foregroundToken} (${fg}) on --${backgroundToken} (${bg}) measures ${ratio.toFixed(2)}:1, ` +
        `below the ${minimum}:1 floor${what ? ` (${what})` : ""}`,
    );
  }
  return ratio;
}

console.log("Contrast contract: text tokens against every background they can land on");

for (const ink of TEXT_TOKENS) {
  const worst = PANEL_BACKGROUNDS.map((surface) => ({
    surface,
    ratio: verify(ink, surface),
  })).sort((a, b) => a.ratio - b.ratio)[0];
  console.log(
    `  --${ink} ${token(ink)}: worst panel pairing --${worst.surface} at ${worst.ratio.toFixed(2)}:1`,
  );
}

for (const ink of TEXT_TOKENS) {
  const worst = TINT_BACKGROUNDS.map((tint) => ({ tint, ratio: verify(ink, tint) })).sort(
    (a, b) => a.ratio - b.ratio,
  )[0];
  console.log(
    `  --${ink} ${token(ink)}: worst tint pairing --${worst.tint} at ${worst.ratio.toFixed(2)}:1`,
  );
}

/*
 * Status text is read on its own tint, which is where the meaning lives: green on green-tint is how the
 * app says this is verified, red on red-tint is how it says this is a violation.
 *
 * Amber and red have separate `-ink` variants because their saturated marks sit on tints and the mark
 * colour itself does not clear the floor there. Green needs no such variant and does not have one, so it
 * is listed as `green`. That asymmetry is deliberate, not an oversight, and this list is where it is
 * recorded.
 */
console.log("Contrast contract: status text on its own tint");
const STATUS_PAIRS = [
  ["green", "green-tint", "verified success"],
  ["amber-ink", "amber-tint", "attention"],
  ["red-ink", "red-tint", "violations"],
  ["blue-hi", "blue-tint", "the action colour"],
  ["ink", "blue-tint", "primary text on a tinted chip"],
  ["ink", "green-tint", "primary text on a tinted chip"],
  ["ink", "amber-tint", "primary text on a tinted chip"],
  ["ink", "red-tint", "primary text on a tinted chip"],
];

for (const [ink, tint, what] of STATUS_PAIRS) {
  const ratio = verify(ink, tint, AA_NORMAL, what);
  console.log(`  --${ink} on --${tint} (${what}): ${ratio.toFixed(2)}:1`);
}

/*
 * The ink scale has to stay ordered, or "dimmer" stops meaning anything. This is cheap to check and it is
 * the property that makes the compressed meta tier above a deliberate choice rather than drift.
 */
console.log("Contrast contract: the ink scale stays ordered");
const scale = ["ink", "ink-2", "ink-3", "ink-4"];
for (let i = 1; i < scale.length; i += 1) {
  const higher = token(scale[i - 1]);
  const lower = token(scale[i]);
  if (luminance(higher) <= luminance(lower)) {
    failures.push(`--${scale[i - 1]} (${higher}) is not lighter than --${scale[i]} (${lower})`);
  }
}
console.log(
  `  ${scale.map((name) => `--${name} ${token(name)}`).join("  ->  ")}`,
);

if (failures.length > 0) {
  console.error("");
  for (const failure of failures) {
    console.error(`FAIL  ${failure}`);
  }
  console.error(`\nContrast contract: ${failures.length} failure(s)`);
  process.exit(1);
}

console.log("\nContrast contract: OK, every text token clears 4.5:1 on every background it can land on.");