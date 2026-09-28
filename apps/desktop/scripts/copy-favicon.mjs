/*
 * Copies the generated 32px icon into the web build as a favicon.
 *
 * `tauri icon` writes every platform size into `src-tauri/icons`, but the frontend is served by Vite
 * in development and in the browser, and neither had any favicon at all -- `index.html` linked no
 * icon, so the tab showed a default glyph and a saved bookmark carried no identity.
 *
 * The desktop shell does not use this file; Tauri reads `src-tauri/icons/icon.ico` directly. This
 * exists for the web surface, and it is a copy rather than a second render so the favicon and the
 * taskbar icon cannot drift into looking like different products.
 */
import { copyFileSync, mkdirSync, existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));
const SOURCE = join(HERE, "..", "src-tauri", "icons", "32x32.png");
const TARGET_DIR = join(HERE, "..", "public");
const TARGET = join(TARGET_DIR, "favicon.png");

if (!existsSync(SOURCE)) {
  console.error(
    `${SOURCE} is missing. Run \`pnpm icons\` first -- this script copies a generated file, it does ` +
      "not render one.",
  );
  process.exit(1);
}

mkdirSync(TARGET_DIR, { recursive: true });
copyFileSync(SOURCE, TARGET);
console.log(`favicon: ${TARGET}`);
