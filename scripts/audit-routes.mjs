/**
 * Sweep every route for overflow defects at a given window size.
 *
 * The Arena board defect was found by measuring one element against the content box. This applies the
 * same reasoning to every route: it reports anything that spills past the content horizontally, and
 * anything inside a scroll container whose own scroll area is taller than it can show.
 *
 * Horizontal overflow is the one that damages a desktop application outright -- a window that has to
 * be dragged sideways to read a table is not a finished application -- so it is checked against the
 * content box rather than the window, for the same reason the Arena measurement had to be.
 *
 * Usage: node scripts/audit-routes.mjs <port> [width] [height]
 */
const port = process.argv[2] ?? "9336";
const width = Number(process.argv[3] ?? 1600);
const height = Number(process.argv[4] ?? 900);

const ROUTES = [
  "readiness",
  "lab",
  "tasks",
  "agents",
  "benchmarks",
  "providers",
  "comparison",
  "arena",
  "security",
  "replay",
  "settings",
];

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

await send("Browser.getWindowForTarget").then(async ({ windowId }) => {
  await send("Browser.setWindowBounds", { windowId, bounds: { width, height, windowState: "normal" } });
});
await new Promise((resolve) => setTimeout(resolve, 700));

const findings = [];
for (const route of ROUTES) {
  await evaluate(`location.hash = "#/${route}"; true`);
  await new Promise((resolve) => setTimeout(resolve, 650));
  const report = await evaluate(`(() => {
    const scroller = document.querySelector(".shell-content");
    const s = scroller.getBoundingClientRect();
    const spilling = [];
    for (const el of document.querySelectorAll(".shell-content *")) {
      const r = el.getBoundingClientRect();
      if (r.width === 0 || r.height === 0) continue;
      if (r.right > s.right + 1 || r.left < s.left - 1) {
        spilling.push({
          tag: el.tagName.toLowerCase(),
          cls: (el.className && typeof el.className === "string" ? el.className : "")
            .split(" ").filter(Boolean).slice(0, 2).join("."),
          overRight: Math.round(r.right - s.right),
          overLeft: Math.round(s.left - r.left),
        });
      }
    }
    /*
     * Only the deepest offenders: a table that overflows makes every cell inside it overflow too, so
     * reporting all of them would bury the one element that has to be fixed. The widest spill wins.
     */
    spilling.sort((a, b) => Math.max(b.overRight, b.overLeft) - Math.max(a.overRight, a.overLeft));
    return {
      route: ${JSON.stringify(route)},
      title: (document.querySelector(".shell-title")?.textContent ?? "").trim(),
      horizontalOverflow: Math.max(0, scroller.scrollWidth - scroller.clientWidth),
      verticalOverflow: Math.max(0, scroller.scrollHeight - scroller.clientHeight),
      emptyButtons: Array.from(document.querySelectorAll("button")).filter(
        (b) => !b.textContent.trim() && !b.getAttribute("aria-label") && !b.title,
      ).length,
      spilling: spilling.slice(0, 3),
    };
  })()`);
  findings.push(report);
}

console.log(JSON.stringify({ window: `${width}x${height}`, findings }, null, 2));
socket.close();