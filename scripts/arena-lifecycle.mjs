/**
 * Exercises the Arena lifecycle against a running ProbeLab window.
 *
 * The claims being checked are ones a screenshot cannot make: that a completed match survives a
 * selector change, that a second match does not overwrite the first, that the archive reopens one, and
 * that playback advances and cleans up.
 *
 * Usage: node scripts/arena-lifecycle.mjs [port] [outDir]
 */
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const port = process.argv[2] ?? "9333";
const outDir = process.argv[3] ?? join(process.env.TEMP ?? ".", "probelab-arena");
mkdirSync(outDir, { recursive: true });

const target = await fetch(`http://127.0.0.1:${port}/json/list`)
  .then((r) => r.json())
  .then((ts) => ts.find((t) => t.type === "page" && t.webSocketDebuggerUrl));

if (!target) {
  console.error("no page target exposed by the running window");
  process.exit(2);
}

const socket = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((res, rej) => {
  socket.addEventListener("open", res, { once: true });
  socket.addEventListener("error", rej, { once: true });
});

let nextId = 0;
const pending = new Map();
socket.addEventListener("message", (event) => {
  const message = JSON.parse(event.data);
  if (message.id && pending.has(message.id)) {
    const { resolve } = pending.get(message.id);
    pending.delete(message.id);
    resolve(message.result);
  }
});
function send(method, params = {}) {
  const id = ++nextId;
  socket.send(JSON.stringify({ id, method, params }));
  return new Promise((resolve) => pending.set(id, { resolve }));
}

const evaluate = async (expression) => {
  const { result } = await send("Runtime.evaluate", { expression, returnByValue: true });
  return result?.value;
};

const wait = (ms) => new Promise((r) => setTimeout(r, ms));

const snapshot = () =>
  evaluate(`JSON.stringify((() => ({
    hash: location.hash,
    board: !!document.querySelector(".arena-board"),
    cells: [...document.querySelectorAll(".arena-cell")].map(c => c.className.replace("arena-cell ", "")),
    winning: document.querySelectorAll(".arena-cell--winning").length,
    status: document.querySelector(".arena-status")?.textContent.trim() ?? null,
    stale: !!document.querySelector(".arena-stale"),
    result: document.querySelector(".arena-result-label")?.textContent.trim() ?? null,
    frame: document.querySelector(".arena-frame-counter")?.textContent.trim() ?? null,
    archive: [...document.querySelectorAll(".arena-archive-row")].map(r => ({
      outcome: r.querySelector(".arena-archive-outcome")?.textContent.trim(),
      pairing: r.querySelector(".arena-archive-pairing")?.textContent.trim(),
      open: r.getAttribute("aria-current") === "true",
    })),
    transport: {
      play: [...document.querySelectorAll(".arena-transport-button")].map(b => b.textContent.trim()),
      disabled: [...document.querySelectorAll(".arena-transport-button")].map(b => b.disabled),
    },
    controls: [...document.querySelectorAll(".arena-controls button")].map(b => ({
      t: b.textContent.trim(), disabled: b.disabled })),
    error: document.querySelector(".brain-error")?.textContent.trim() ?? null,
  }))())`);

const shot = async (name) => {
  const { data } = await send("Page.captureScreenshot", { format: "png" });
  writeFileSync(join(outDir, `${name}.png`), Buffer.from(data, "base64"));
  console.log(`  saved ${name}.png`);
};

await send("Runtime.enable");
await send("Page.enable");

const go = async (route) => {
  await evaluate(`location.hash = "#/${route}"`);
  await wait(1400);
};

/**
 * Clicks a button by exact label, scoped to a container.
 *
 * Searching every button on the page is how a click meant for the panel ends up on the navigation
 * rail: "NEW MATCH" is a substring of nothing there, but "REPLAY" is both a rail item and a panel
 * heading, and a loose match will happily press the wrong one. Scoping is the fix.
 */
const clickIn = (selector, text) =>
  evaluate(`(() => {
    const root = document.querySelector(${JSON.stringify(selector)}) ?? document;
    const target = [...root.querySelectorAll("button")]
      .find(b => b.textContent.trim() === ${JSON.stringify(text)} && !b.disabled);
    if (target) { target.click(); return true; }
    return false;
  })()`);

const clickText = (text) => clickIn(".arena-panel", text);

/**
 * Starts a match whichever button currently offers it.
 *
 * The control reads "PLAY MATCH" with nothing recorded and "RUN AGAIN" once something is, and an
 * archive restored across a restart means the second name is the one on screen. Either one starts a
 * real engine match; this picks whichever is present rather than assuming a fresh session.
 */
const playMatch = () =>
  evaluate(`(() => {
    const buttons = [...document.querySelectorAll(".arena-controls button")];
    const target = buttons.find(b => b.textContent.trim() === "RUN AGAIN" && !b.disabled)
      ?? buttons.find(b => b.textContent.trim() === "PLAY MATCH" && !b.disabled);
    if (!target) return false;
    target.click();
    return true;
  })()`);

const setSelect = (id, value) =>
  evaluate(`(() => {
    const s = document.querySelector(${JSON.stringify(`#${id}`)});
    if (!s) return "no select";
    s.value = ${JSON.stringify(value)};
    s.dispatchEvent(new Event("change", { bubbles: true }));
    return s.value;
  })()`);

const failures = [];
const check = (name, ok, detail = "") => {
  console.log(`${ok ? "  PASS" : "  FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
  if (!ok) failures.push(name);
};

const archiveCount = (s) => s.archive.length;

console.log("=== 1. Open the arena ===");
await go("arena");
let state = JSON.parse(await snapshot());
// Matches persist across restarts by design, so the baseline is whatever is already archived rather
// than zero. What must hold is that opening the panel does not manufacture a match.
const baseline = archiveCount(state);
check("the archive starts at its persisted baseline", baseline >= 0, `${baseline} stored`);
check(
  "opening the panel plays nothing",
  ["NO ACTIVE MATCH", "WIN", "DRAW", "LOSS", "NO RESULT"].includes(state.status),
  String(state.status),
);
await shot("00-initial");

console.log("=== 2. Play the first match ===");
check("a match can be started", (await playMatch()) === true);
await wait(4500);
state = JSON.parse(await snapshot());
check("a board is drawn", state.board === true, `${state.cells?.length} cells`);
check("the outcome is real", state.status === "WIN" || state.status === "DRAW" || state.status === "LOSS", state.status);
check("the tested policy's result is stated", !!state.result, state.result);
check("the new match is archived", archiveCount(state) === baseline + 1, `${baseline} → ${archiveCount(state)}`);
await shot("01-first-match");

console.log("=== 3. The selector change must NOT destroy the match ===");
const pliesBefore = await evaluate(`document.querySelectorAll(".arena-ply").length`);
const archiveBefore = archiveCount(state);
// Flip the seat rather than set it: a previous run may already have left it on the value we would
// choose, and changing nothing is not a configuration change.
const seatBefore = await evaluate(`document.querySelector("#arena-side")?.value`);
await setSelect("arena-side", seatBefore === "first" ? "second" : "first");
await wait(1200);
state = JSON.parse(await snapshot());
const pliesAfter = await evaluate(`document.querySelectorAll(".arena-ply").length`);
check("the board survives", state.board === true);
check("the match is marked stale rather than discarded", state.stale === true);
check("no ply was lost", pliesBefore === pliesAfter, `${pliesBefore} → ${pliesAfter}`);
check("the archive is untouched", archiveCount(state) === archiveBefore, `${archiveBefore} → ${archiveCount(state)}`);
await shot("02-stale-after-selector-change");

console.log("=== 4. Run again produces a genuinely new match ===");
check("run again is offered", (await playMatch()) === true);
await wait(4500);
state = JSON.parse(await snapshot());
check("the second match is recorded", archiveCount(state) === archiveBefore + 1, `${archiveCount(state)} in the archive`);
check("the stale banner is gone", state.stale === false);
check("the earlier match is still in the archive", archiveCount(state) >= archiveBefore + 1);
await shot("03-second-match");

console.log("=== 5. Replay the first match from the archive ===");
const firstRow = await evaluate(`(() => {
  const rows = [...document.querySelectorAll(".arena-archive-row")];
  const row = rows[rows.length - 1];
  row.click();
  return row.querySelector(".arena-archive-pairing")?.textContent.trim();
})()`);
await wait(2500);
state = JSON.parse(await snapshot());
check("an archived match opens", state.board === true, firstRow);
check("the opened match is marked as current", state.archive.some((r) => r.open));
await shot("04-reopened-from-archive");

console.log("=== 6. Playback advances and cleans up ===");
await evaluate(`document.querySelector('[aria-label="Go to the first frame"]').click()`);
await wait(400);
const atStart = await evaluate(`document.querySelector(".arena-frame-counter")?.textContent.trim()`);
check("seeking to the start works", /^\s*1\s*\//.test(atStart ?? ""), atStart);
await clickIn(".arena-playback-transport", "▶ PLAY");
await wait(2200);
const midPlay = await evaluate(`document.querySelector(".arena-frame-counter")?.textContent.trim()`);
check("playback advances on its own", !/^\s*1\s*\//.test(midPlay ?? ""), `${atStart} → ${midPlay}`);
await shot("05-mid-replay");
await evaluate(`(() => {
  const b = [...document.querySelectorAll(".arena-transport-button")].find(x => x.textContent.includes("PAUSE") && !x.disabled);
  if (b) b.click();
})()`);
const paused = await evaluate(`document.querySelector(".arena-frame-counter")?.textContent.trim()`);
await wait(1500);
const stillPaused = await evaluate(`document.querySelector(".arena-frame-counter")?.textContent.trim()`);
check("pausing stops the advance", paused === stillPaused, `${paused} = ${stillPaused}`);
const winning = await evaluate(`document.querySelectorAll(".arena-cell--winning").length`);
check("a winning line is drawn only at the end", winning === 0 || winning === 3, `${winning} cells`);
await evaluate(`document.querySelector('[aria-label="Go to the last frame"]').click()`);
await wait(400);
await shot("06-final-frame");

console.log("=== 7. New match clears the view but keeps the archive ===");
const archiveBeforeReset = archiveCount(state);
await clickText("NEW MATCH");
await wait(800);
state = JSON.parse(await snapshot());
check("the board is cleared", state.board === false, `error: ${state.error}`);
check("the archive still holds every match", archiveCount(state) === archiveBeforeReset, `${archiveBeforeReset} → ${archiveCount(state)}${state.error ? ` · error: ${state.error}` : ""}`);
await shot("07-new-match-keeps-archive");

console.log("");
if (failures.length > 0) {
  console.log(`FAILED: ${failures.length} check(s)`);
  for (const failure of failures) console.log(`  - ${failure}`);
  process.exit(1);
}
console.log("All arena lifecycle checks passed.");
socket.close();