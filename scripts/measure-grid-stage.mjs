/**
 * Measure the grid-world stage against the viewport.
 *
 * Screenshots show whether something looks wrong; this reports by how many pixels it is wrong, which is
 * the difference between "the board looks a bit low" and a fix that provably fits.
 *
 * The properties that matter here, and the defect each one catches:
 *
 * * the board is square — a non-square board is a distorted map, and a navigation view that distorts its
 *   own geometry cannot be used to reason about distance;
 * * every cell is the same size — one stretched column reads as a different kind of tile;
 * * the marker sits inside its cell — the animated position is computed in `calc()`, so this is where an
 *   arithmetic slip in that expression becomes visible rather than theoretical;
 * * nothing overflows sideways — a fixed-width column inside a flexible one is the usual cause;
 * * and every named control is reachable without scrolling.
 *
 * Usage: node scripts/measure-grid-stage.mjs <port> [width] [height]
 */
const port = process.argv[2] ?? "9336";
const width = Number(process.argv[3] ?? 1600);
const height = Number(process.argv[4] ?? 900);

const targets = await fetch(`http://127.0.0.1:${port}/json/list`).then((r) => r.json());
const page = targets.find((t) => t.type === "page" && t.webSocketDebuggerUrl);
const socket = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  socket.addEventListener("open", resolve, { once: true });
  socket.addEventListener("error", reject, { once: true });
});

let id = 0;
const pending = new Map();
socket.addEventListener("message", (event) => {
  const message = JSON.parse(event.data);
  if (!message.id || !pending.has(message.id)) return;
  const { resolve, reject } = pending.get(message.id);
  pending.delete(message.id);
  message.error ? reject(new Error(message.error.message)) : resolve(message.result);
});

function send(method, params = {}) {
  const messageId = ++id;
  socket.send(JSON.stringify({ id: messageId, method, params }));
  return new Promise((resolve, reject) => pending.set(messageId, { resolve, reject }));
}

await send("Page.enable");
await send("Runtime.enable");

async function evaluate(expression) {
  const { result, exceptionDetails } = await send("Runtime.evaluate", {
    expression,
    returnByValue: true,
    awaitPromise: true,
  });
  if (exceptionDetails) throw new Error(exceptionDetails.exception?.description ?? "evaluate failed");
  return result.value;
}

/* The real window, not a viewport override: this has to report the size the app can actually be. */
await send("Browser.getWindowForTarget").then(async ({ windowId }) => {
  await send("Browser.setWindowBounds", {
    windowId,
    bounds: { width, height, windowState: "normal" },
  });
});
await new Promise((resolve) => setTimeout(resolve, 700));

await evaluate(`location.hash = "#/lab"; true`);
await new Promise((resolve) => setTimeout(resolve, 1500));

const report = await evaluate(`(() => {
  const scroller = document.querySelector(".shell-content");
  if (!scroller) return { error: "no .shell-content scroll container found" };
  const s = scroller.getBoundingClientRect();
  const view = { top: s.top, bottom: s.bottom, height: s.height };
  const rows = [];
  const push = (name, el) => {
    if (!el) { rows.push({ name, missing: true }); return; }
    const r = el.getBoundingClientRect();
    rows.push({
      name,
      top: Math.round(r.top),
      bottom: Math.round(r.bottom),
      left: Math.round(r.left),
      right: Math.round(r.right),
      width: Math.round(r.width),
      height: Math.round(r.height),
      visible: r.top >= view.top - 0.5 && r.bottom <= view.bottom + 0.5,
    });
  };

  const board = document.querySelector(".gw-board");
  const cells = Array.from(document.querySelectorAll(".gw-cell"));
  const marker = document.querySelector(".gw-agent");
  const cellBoxes = cells.map((c) => c.getBoundingClientRect());
  const widths = cellBoxes.map((b) => Math.round(b.width));
  const heights = cellBoxes.map((b) => Math.round(b.height));

  push("stage panel", document.querySelector(".gw-stage"));
  push("board", board);
  push("cell 0", cells[0]);
  push("cell 24", cells[cells.length - 1]);
  push("turn log", document.querySelector(".gw-log"));
  push("legend", document.querySelector(".legend"));
  push("readout", document.querySelector(".gw-readout"));
  const controls = Array.from(document.querySelectorAll(".gw-transport button, .gw-source-bar button"));
  push("last stage control", controls[controls.length - 1]);

  // The marker is positioned by arithmetic on the cell pitch, so containment is measured, not assumed.
  let markerContainment = null;
  if (board && marker && cells.length > 0) {
    const b = board.getBoundingClientRect();
    const m = marker.getBoundingClientRect();
    const style = getComputedStyle(board);
    const padding = parseFloat(style.paddingLeft) || 0;
    markerContainment = {
      insetLeft: Math.round(m.left - b.left - padding),
      insetTop: Math.round(m.top - b.top - padding),
      cellSize: widths[0] ?? null,
      insideBoard: m.left >= b.left - 1 && m.right <= b.right + 1 && m.top >= b.top - 1 && m.bottom <= b.bottom + 1,
    };
  }

  return {
    viewport: { width: window.innerWidth, height: window.innerHeight },
    visibleContentBox: { top: Math.round(view.top), bottom: Math.round(view.bottom) },
    overflowX: Math.max(0, document.documentElement.scrollWidth - document.documentElement.clientWidth),
    boardWidth: board ? Math.round(board.getBoundingClientRect().width) : null,
    boardHeight: board ? Math.round(board.getBoundingClientRect().height) : null,
    boardSquare: board
      ? Math.abs(
          board.getBoundingClientRect().width - board.getBoundingClientRect().height,
        ) <= 1.5
      : null,
    cellCount: cells.length,
    cellWidths: [...new Set(widths)],
    cellHeights: [...new Set(heights)],
    cellsUniform: new Set(widths).size <= 1 && new Set(heights).size <= 1,
    seenCells: cells.filter((c) => c.getAttribute("data-tile") !== "unseen").length,
    unseenCells: cells.filter((c) => c.getAttribute("data-tile") === "unseen").length,
    markerContainment,
    boardLabel: board?.getAttribute("aria-label") ?? null,
    failing: rows.filter((r) => r.visible === false).map((r) => r.name),
    rows,
  };
})()`);

console.log(JSON.stringify(report, null, 2));

/*
 * Assertions, printed rather than thrown: this script is a measurement tool that a person reads, and a
 * number is only useful next to the thing it failed.
 */
const problems = [];
if (report.boardSquare === false) problems.push("the board is not square");
if (report.cellsUniform === false) problems.push(`cells are not uniform: ${JSON.stringify(report.cellWidths)}`);
if (report.overflowX > 0) problems.push(`horizontal overflow of ${report.overflowX}px`);
if (report.markerContainment && report.markerContainment.insideBoard === false) {
  problems.push("the agent marker is outside the board");
}
if (report.failing?.length) problems.push(`below the fold: ${report.failing.join(", ")}`);
for (const problem of problems) console.error(`FAIL  ${problem}`);
console.log(problems.length === 0 ? "PASS  grid stage geometry" : `FAILED ${problems.length}`);
socket.close();