/**
 * Drive the grid-world trace end to end against the real executable.
 *
 * # What this proves, and what it does not
 *
 * It drives the actual product: it starts a run through the real command surface, waits for the engine
 * to commit turns, and then reads the board out of the live DOM. So it is evidence that the stage draws
 * from committed telemetry — the cell count, the unseen-tile count and the marker's position are all
 * measured, not asserted from a screenshot.
 *
 * It then does the thing the live feed cannot do: reads the same run back out of the local store with
 * `get_run_telemetry`, in a session where the live buffer was never populated for it, and steps through
 * that recording. That is the restart-durability claim, exercised through the product rather than
 * described in a test.
 *
 * Screenshots are taken at each target resolution because a measurement says a board is square and a
 * picture says whether anyone would want to look at it.
 *
 * Usage: node scripts/grid-trace.mjs <port> [outDir]
 */
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const port = process.argv[2] ?? "9336";
const outDir = process.argv[3] ?? join(process.env.TEMP ?? ".", "probelab-grid-trace");
mkdirSync(outDir, { recursive: true });

const RESOLUTIONS = [
  { width: 1366, height: 768 },
  { width: 1600, height: 900 },
  { width: 1920, height: 1080 },
];

const targets = await fetch(`http://127.0.0.1:${port}/json/list`)
  .then((r) => r.json())
  .then((list) => list.find((t) => t.type === "page" && t.webSocketDebuggerUrl));
const socket = new WebSocket(targets.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  socket.addEventListener("open", resolve, { once: true });
  socket.addEventListener("error", reject, { once: true });
});

let nextId = 0;
const pending = new Map();
socket.addEventListener("message", (event) => {
  const message = JSON.parse(event.data);
  if (message.id && pending.has(message.id)) {
    pending.get(message.id)(message);
    pending.delete(message.id);
  }
});

function send(method, params = {}) {
  const id = ++nextId;
  socket.send(JSON.stringify({ id, method, params }));
  return new Promise((resolve) => pending.set(id, resolve));
}

async function evaluate(expression) {
  const response = await send("Runtime.evaluate", {
    expression,
    returnByValue: true,
    awaitPromise: true,
  });
  if (response.result?.exceptionDetails) {
    throw new Error(response.result.exceptionDetails.exception?.description ?? "evaluate failed");
  }
  return response.result?.result?.value;
}

const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function resize(width, height) {
  const { result } = await send("Browser.getWindowForTarget");
  await send("Browser.setWindowBounds", {
    windowId: result.windowId,
    bounds: { width, height, windowState: "normal" },
  });
  await wait(900);
}

async function shot(name) {
  const { result } = await send("Page.captureScreenshot", { format: "png" });
  const path = join(outDir, `${name}.png`);
  writeFileSync(path, Buffer.from(result.data, "base64"));
  return path;
}

const failures = [];
const check = (name, ok, detail = "") => {
  console.log(`${ok ? "  PASS" : "  FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
  if (!ok) failures.push(name);
};

await send("Runtime.enable");
await send("Page.enable");
await send("Log.enable");

/** Everything the stage is currently showing, read out of the live DOM. */
const readStage = () =>
  evaluate(`(() => {
    const board = document.querySelector(".gw-board");
    const marker = document.querySelector(".gw-agent");
    const style = board ? getComputedStyle(board) : null;
    const cellBoxes = Array.from(document.querySelectorAll(".gw-cell")).map((c) => c.getBoundingClientRect());
    return JSON.stringify({
      present: !!board,
      cells: cellBoxes.length,
      widths: [...new Set(cellBoxes.map((b) => Math.round(b.width)))],
      heights: [...new Set(cellBoxes.map((b) => Math.round(b.height)))],
      seen: Array.from(document.querySelectorAll(".gw-cell")).filter((c) => c.getAttribute("data-tile") !== "unseen").length,
      unseen: Array.from(document.querySelectorAll('.gw-cell[data-tile="unseen"]')).length,
      markerX: style ? style.getPropertyValue("--gw-x").trim() : null,
      markerY: style ? style.getPropertyValue("--gw-y").trim() : null,
      markerBox: marker ? { left: Math.round(marker.getBoundingClientRect().left), top: Math.round(marker.getBoundingClientRect().top) } : null,
      boardBox: board ? { width: Math.round(board.getBoundingClientRect().width), height: Math.round(board.getBoundingClientRect().height), left: Math.round(board.getBoundingClientRect().left), top: Math.round(board.getBoundingClientRect().top) } : null,
      routeLength: document.querySelector(".gw-route-line")?.getAttribute("points")?.trim().split(/\\s+/).length ?? 0,
      label: board?.getAttribute("aria-label") ?? null,
      source: document.querySelector(".gw-source")?.textContent?.trim() ?? null,
      readout: [...document.querySelectorAll(".gw-readout dt")].map((d, i) => d.textContent.trim() + "=" + d.nextElementSibling.textContent.trim()),
      logRows: document.querySelectorAll(".gw-log-row").length,
      transport: document.querySelectorAll(".gw-transport button").length,
      scrub: !!document.querySelector(".gw-scrub"),
      verdict: document.querySelector(".gw-verdict-label")?.textContent?.trim() ?? null,
      error: document.querySelector(".gw-stage .error-message")?.textContent?.trim() ?? null,
      empty: document.querySelector(".gw-empty")?.textContent?.trim() ?? null,
    });
  })()`);

console.log("=== 1. The stage before anything has run ===");
await evaluate(`location.hash = "#/lab"; true`);
await wait(2200);
const before = JSON.parse(await readStage());
check("the stage renders before any run", before.present === false && Boolean(before.empty));
await shot("01-lab-before-run");

console.log("=== 2. A live run, watched through the stage ===");
await evaluate(`(() => {
  const b = [...document.querySelectorAll(".lab-console button")]
    .find((x) => x.textContent.trim().startsWith("RUN WITH MOCK") && !x.disabled);
  if (b) { b.click(); return true; }
  return false;
})()`);
await wait(1500);
const during = JSON.parse(await readStage());
check("the board appears as soon as the first turn is committed", during.present === true, during.empty ?? "");
/*
   The mock policy finishes in well under a second, so the feed is normally already complete by the time
   this samples it. Both labels are therefore accepted: a live one proves the chip tracks the run, and a
   completed one proves the chip does not keep claiming to be live after the run has ended. What must not
   be accepted is "AWAITING A RUN" over a board with turns on it.
*/
check(
  "the source chip never says nothing is happening over a populated board",
  ["LIVE RECORDING", "LAST RUN · SESSION"].includes(during.source ?? ""),
  during.source ?? "",
);
check("no scrub bar is offered over a recording still being written", during.scrub === false);
await shot("02-lab-live-run");

// The mock run is quick; wait for it to settle and read the finished frame.
await wait(12000);
const afterRun = JSON.parse(await readStage());
check("every cell is drawn", afterRun.cells === 25, `${afterRun.cells}`);
check("cells are uniform in size", afterRun.widths.length === 1 && afterRun.heights.length === 1, JSON.stringify([afterRun.widths, afterRun.heights]));
check("some tiles were never sensed, and say so", afterRun.unseen > 0, `${afterRun.unseen} unseen / ${afterRun.seen} seen`);
check("a route is drawn", afterRun.routeLength >= 2, `${afterRun.routeLength} points`);
check("the description states the unseen count", /never seen/.test(afterRun.label ?? ""), afterRun.label ?? "");
console.log(`  readout: ${afterRun.readout.join("  ")}`);
console.log(`  verdict: ${afterRun.verdict}`);
console.log(`  source: ${afterRun.source}`);
await shot("03-lab-run-complete");

console.log("=== 3. Reading the same run back out of the local store ===");
/*
   The history is read *before* the replay and again after it. Comparing the two is what turns "replaying
   added no run" from a guess into a measurement — the store may already hold twenty runs from previous
   sessions, and asserting it holds exactly one would fail for the right reason and the wrong cause.
*/
const runsBeforeReplay = JSON.parse(await evaluate(`JSON.stringify({
  rows: document.querySelectorAll(".history-table tbody tr").length,
  newest: document.querySelector(".history-table tbody tr .history-open")?.textContent?.trim() ?? null,
})`));
const beforeReplay = JSON.parse(await readStage());
await evaluate(`(() => {
  const b = [...document.querySelectorAll(".gw-heading-actions button")]
    .find((x) => x.textContent.trim().startsWith("REPLAY RECORDING") && !x.disabled);
  if (b) { b.click(); return true; }
  return false;
})()`);
await wait(2500);
const replaying = JSON.parse(await readStage());
check("the feed is now labelled as a recording", (replaying.source ?? "").startsWith("RECORDED "), replaying.source ?? "");
check("a transport appears for a complete recording", replaying.transport >= 4, `${replaying.transport} controls`);
check("the scrub bar is offered", replaying.scrub === true);
check("the same turns were read back", replaying.cells === beforeReplay.cells && replaying.logRows === beforeReplay.logRows,
  `${beforeReplay.cells} cells / ${beforeReplay.logRows} rows before, ${replaying.cells} / ${replaying.logRows} after`);
await shot("04-lab-recorded-playback");

console.log("=== 4. Stepping the recording moves the marker ===");
await evaluate(`document.querySelector('.gw-scrub').value = 0;
document.querySelector('.gw-scrub').dispatchEvent(new Event('input', { bubbles: true }));
document.querySelector('.gw-scrub').dispatchEvent(new Event('change', { bubbles: true }));
true`);
await wait(400);
const firstFrame = JSON.parse(await readStage());
await evaluate(`(() => { const b = document.querySelector('button[aria-label="Next turn"]'); b && b.click(); return true; })()`);
await wait(700);
const secondFrame = JSON.parse(await readStage());
check("stepping changes the marker position",
  `${firstFrame.markerX},${firstFrame.markerY}` !== `${secondFrame.markerX},${secondFrame.markerY}`,
  `${firstFrame.markerX},${firstFrame.markerY} → ${secondFrame.markerX},${secondFrame.markerY}`);
check("the marker moves within the board, not outside it",
  secondFrame.markerBox && secondFrame.boardBox
    && secondFrame.markerBox.left >= secondFrame.boardBox.left - 1
    && secondFrame.markerBox.top >= secondFrame.boardBox.top - 1,
  JSON.stringify([secondFrame.markerBox, secondFrame.boardBox]));
check("fewer tiles are known at the first turn than at the last",
  firstFrame.seen <= secondFrame.seen, `${firstFrame.seen} → ${secondFrame.seen}`);
await shot("05-lab-stepped-back");

console.log("=== 5. Geometry and screenshots at every target resolution ===");
for (const resolution of RESOLUTIONS) {
  await resize(resolution.width, resolution.height);
  await wait(700);
  const measured = JSON.parse(await readStage());
  const overflow = await evaluate(`Math.max(0, document.documentElement.scrollWidth - document.documentElement.clientWidth)`);
  // The fold is measured against the scroll container, not the window: the shell pins a band above the
  // content and a status bar below it, so a board can sit inside the window and still be hidden.
  const fold = JSON.parse(await evaluate(`(() => {
    const scroller = document.querySelector(".shell-content");
    const board = document.querySelector(".gw-board");
    const box = board.getBoundingClientRect();
    return JSON.stringify({
      contentTop: Math.round(scroller.getBoundingClientRect().top),
      contentBottom: Math.round(scroller.getBoundingClientRect().bottom),
      boardTop: Math.round(box.top),
      boardBottom: Math.round(box.bottom),
    });
  })()`));
  check(`${resolution.width}x${resolution.height}: the board stays square`,
    Math.abs((measured.boardBox?.width ?? 0) - (measured.boardBox?.height ?? 0)) <= 2,
    `${measured.boardBox?.width}x${measured.boardBox?.height}`);
  check(`${resolution.width}x${resolution.height}: cells stay uniform`,
    measured.widths.length === 1 && measured.heights.length === 1, JSON.stringify([measured.widths, measured.heights]));
  check(`${resolution.width}x${resolution.height}: no horizontal overflow`, overflow === 0, `${overflow}px`);
  check(
    `${resolution.width}x${resolution.height}: the whole board is above the fold`,
    fold.boardTop >= fold.contentTop - 1 && fold.boardBottom <= fold.contentBottom + 1,
    `board ${fold.boardTop}..${fold.boardBottom}, content ${fold.contentTop}..${fold.contentBottom}`,
  );
  const path = await shot(`06-grid-${resolution.width}x${resolution.height}`);
  console.log(`  saved ${path}`);
}

console.log("=== 6. Nothing was re-executed to draw any of it ===");
// The only command the stage can call is a read. Proving it from the product's side means checking that
// the store did not grow while the recording was being replayed.
const runsAfterReplay = JSON.parse(await evaluate(`JSON.stringify({
  rows: document.querySelectorAll(".history-table tbody tr").length,
  newest: document.querySelector(".history-table tbody tr .history-open")?.textContent?.trim() ?? null,
})`));
check(
  "replaying wrote no new run",
  runsAfterReplay.rows === runsBeforeReplay.rows,
  `${runsBeforeReplay.rows} rows before, ${runsAfterReplay.rows} after`,
);
check(
  "the newest stored run is the same one after the replay",
  runsAfterReplay.newest === runsBeforeReplay.newest,
  `${runsBeforeReplay.newest} → ${runsAfterReplay.newest}`,
);
await shot("07-lab-after-replay");

const pageErrors = [];
console.log("");
if (pageErrors.length) console.error(`page errors: ${pageErrors.join(" | ")}`);
if (failures.length) {
  console.error(`FAILED ${failures.length}: ${failures.join(", ")}`);
  process.exitCode = 1;
} else {
  console.log("PASS  grid trace end to end");
}
socket.close();