/// <reference types="vite/client" />

/**
 * Ambient types for the Vite build.
 *
 * This file is part of Vite's own scaffold and its absence here left `Brand.test.tsx` failing to
 * typecheck: it imports the application icon as text — `import("../src-tauri/app-icon.svg?raw")` — to
 * assert the shipped icon matches its source, and without the `?raw` module declaration TypeScript
 * reports the import as an unresolvable module. The test ran and passed in Vitest, which is why the
 * error went unnoticed until a full `tsc -b` was run with the icon assertions in place: the runtime
 * behaviour was never wrong, only the build gate was red.
 */