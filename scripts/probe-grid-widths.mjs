/**
 * One-off: report the widths that decide how much room the grid board gets.
 *
 * The board is measured *after* opening the most recent recording, because a stage with no run loaded
 * renders no board at all: the probe used to report `board: null` on a fresh session and the numbers it
 * was written to produce — the board column, its max-width, the side rail beside it — were silently
 * missing from every reading taken on a machine that had not just finished a run.
 *
 * Usage: node scripts/probe-grid-widths.mjs <port>
 */
const port = process.argv[2] ?? "9336";
const targets = await fetch(`http://127.0.0.1:${port}/json/list`).then((r) => r.json());
const page = targets.find((t) => t.type === "page" && t.webSocketDebuggerUrl);
const socket = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((resolve) => socket.addEventListener("open", resolve, { once: true }));
let id = 0;
const pending = new Map();
socket.addEventListener("message", (event) => {
  const message = JSON.parse(event.data);
  if (pending.has(message.id)) {
    pending.get(message.id)(message);
    pending.delete(message.id);
  }
});
const send = (method, params = {}) => {
  const messageId = ++id;
  socket.send(JSON.stringify({ id: messageId, method, params }));
  return new Promise((resolve) => pending.set(messageId, resolve));
};
const evaluate = async (expression) => {
  const response = await send("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true });
  if (response.result?.exceptionDetails) {
    throw new Error(response.result.exceptionDetails.exception?.description ?? "evaluate failed");
  }
  return response.result?.result?.value;
};
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/* Land on the lab, then open the newest recording the history holds. */
await evaluate(`location.hash = "#/lab"; true`);
await wait(2000);
const opened = await evaluate(`(() => {
  const row = document.querySelector(".history-table tbody tr");
  const open = row?.querySelector(".history-open");
  if (!open) return "no stored run to open";
  open.click();
  return "opened";
})()`);
if (opened !== "opened") {
  console.warn(`warning: ${opened} — the board widths below will be null`);
}
await wait(2000);

console.log(await evaluate(`(() => {
    const q = (s) => document.querySelector(s);
    const w = (s) => { const e = q(s); return e ? Math.round(e.getBoundingClientRect().width) : null; };
    const body = q(".gw-body");
    const board = q(".gw-board");
    return JSON.stringify({
      dpr: window.devicePixelRatio,
      innerWidth: window.innerWidth,
      labConsole: w(".lab-console"),
      controls: w(".lab-controls"),
      labStage: w(".lab-stage"),
      inspector: w(".lab-inspector"),
      stage: w(".gw-stage"),
      body: w(".gw-body"),
      bodyCols: body ? getComputedStyle(body).gridTemplateColumns : null,
      boardColumn: w(".gw-board-column"),
      board: w(".gw-board"),
      boardMax: board ? getComputedStyle(board).maxWidth : null,
      side: w(".gw-side"),
    }, null, 1);
  })()`));
socket.close();