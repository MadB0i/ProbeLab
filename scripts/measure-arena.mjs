/**
 * Measure the Arena route against the viewport.
 *
 * Screenshots show whether something looks wrong; this reports by how many pixels it is wrong,
 * which is the difference between "the board looks a bit low" and a fix that provably fits.
 *
 * For each of the named rows and controls it reports the box, and whether every pixel of that box
 * lands inside the scrollable content area without needing a scroll. A row that fails here is a row
 * a user has to scroll to reach, which is the defect this exists to catch.
 *
 * Usage: node scripts/measure-arena.mjs <port> [routeHash] [width] [height]
 */
const port = process.argv[2] ?? "9336";
const route = process.argv[3] ?? "#/arena";

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

/*
 * The real window, not a viewport override.
 *
 * A CSS emulated viewport proves the layout responds to a size; it does not prove the window can
 * actually be that size, nor that the chrome and title bar leave the content the room it appears to.
 * So this resizes the native window and lets the renderer report what it genuinely got.
 */
const width = Number(process.argv[4] ?? 1600);
const height = Number(process.argv[5] ?? 900);
await send("Browser.getWindowForTarget").then(async ({ windowId }) => {
  await send("Browser.setWindowBounds", {
    windowId,
    bounds: { width, height, windowState: "normal" },
  });
});
await new Promise((resolve) => setTimeout(resolve, 700));

await evaluate(`location.hash = ${JSON.stringify(route)}; true`);
await new Promise((resolve) => setTimeout(resolve, 1200));

const report = await evaluate(`(() => {
  /*
   * Measured against the scroll container, not the window.
   *
   * The window is not the viewport. The shell pins a topbar above the content and a status bar
   * below it, so a row can sit inside the window and still be hidden underneath the status bar --
   * which is exactly the bug this script exists to catch, and exactly what the first version of it
   * missed when it reported a comfortable margin against a height the user can never see.
   */
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
      height: Math.round(r.height),
      visible: r.top >= view.top - 0.5 && r.bottom <= view.bottom + 0.5,
    });
  };
  const cells = Array.from(document.querySelectorAll(".arena-cell"));
  push("arena panel", document.querySelector(".arena-panel"));
  push("board frame", document.querySelector(".arena-board-frame"));
  push("board grid", document.querySelector(".arena-board"));
  push("row 1 (first cell)", cells[0] ?? null);
  push("row 3 (last cell)", cells[8] ?? null);
  push("playback section", document.querySelector(".arena-playback-section"));
  const transportButtons = Array.from(
    document.querySelectorAll(".arena-playback-section button"),
  );
  push("last transport button", transportButtons[transportButtons.length - 1]);
  return {
    viewport: { width: window.innerWidth, height: window.innerHeight },
    visibleContentBox: { top: Math.round(view.top), bottom: Math.round(view.bottom) },
    overflowY: Math.max(0, scroller.scrollHeight - scroller.clientHeight),
    boardCellSize: cells[0] ? Math.round(cells[0].getBoundingClientRect().width) : null,
    failing: rows.filter((r) => r.visible === false).map((r) => r.name),
    rows,
  };
})()`);

console.log(JSON.stringify(report, null, 2));
socket.close();