/**
 * One-off diagnostic: screenshot the running window's viewport through DevTools.
 *
 * PrintWindow captures the native window; this captures what the page laid out. When the two
 * disagree -- a column that looks cut off in the native capture -- this is the second opinion.
 *
 * Usage: node scripts/probe-viewport.mjs <port> <outPath>
 */
const port = process.argv[2] ?? "9336";
const out = process.argv[3] ?? "viewport.png";

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
const { data } = await send("Page.captureScreenshot", { format: "png" });
const { writeFileSync } = await import("node:fs");
writeFileSync(out, Buffer.from(data, "base64"));
console.log(`saved ${out}`);
socket.close();
