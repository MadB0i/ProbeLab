/**
 * Proves that a completed run is still listed and still reopens after the application is restarted.
 *
 * # Why this exists
 *
 * The Command Center's history was permanently empty while every run appeared to succeed, because the
 * listing command read the session surface and the history the user saw was rendered from the durable
 * store. Nothing crashed and every number on screen was correct, which is the worst kind of failure:
 * invisible. This drives the real executable across a genuine restart so the property is checked
 * against the shipped binary rather than against a unit test's in-memory store.
 *
 * Usage: node scripts/restart-durability.mjs <exe> [port]
 */
import { spawn } from "node:child_process";

const exe = process.argv[2] ?? "D:\\Projects\\ProbeLab\\target\\release\\probelab-desktop.exe";
const port = process.argv[3] ?? "9333";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const failures = [];
const check = (name, ok, detail = "") => {
  console.log(`${ok ? "  PASS" : "  FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
  if (!ok) failures.push(name);
};

async function launch() {
  const child = spawn(exe, [], {
    env: { ...process.env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}` },
    stdio: "ignore",
  });
  // The window takes a few seconds to open and the DevTools endpoint to appear.
  for (let attempt = 0; attempt < 40; attempt += 1) {
    await sleep(700);
    try {
      const targets = await fetch(`http://127.0.0.1:${port}/json/list`).then((r) => r.json());
      if (targets.some((t) => t.type === "page" && t.webSocketDebuggerUrl)) return child;
    } catch {
      // not up yet
    }
  }
  throw new Error("the application did not expose a DevTools endpoint");
}

async function connect() {
  const targets = await fetch(`http://127.0.0.1:${port}/json/list`).then((r) => r.json());
  const target = targets.find((t) => t.type === "page" && t.webSocketDebuggerUrl);
  const socket = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((res, rej) => {
    socket.addEventListener("open", res, { once: true });
    socket.addEventListener("error", rej, { once: true });
  });
  let id = 0;
  const pending = new Map();
  socket.addEventListener("message", (event) => {
    const m = JSON.parse(event.data);
    if (m.id && pending.has(m.id)) {
      pending.get(m.id)(m);
      pending.delete(m.id);
    }
  });
  const send = (method, params = {}) => {
    const i = ++id;
    socket.send(JSON.stringify({ id: i, method, params }));
    return new Promise((r) => pending.set(i, r));
  };
  const ev = async (expr) =>
    (await send("Runtime.evaluate", { expression: expr, returnByValue: true }))?.result?.result?.value;
  await send("Runtime.enable");
  return { ev, close: () => socket.close() };
}

console.log("=== Session 1: run a benchmark and record what was stored ===");
let app = await launch();
let ui = await connect();
await ui.ev(`location.hash = "#/lab"`);
await sleep(3000);

const before = JSON.parse(
  await ui.ev(`JSON.stringify({
    runs: document.querySelectorAll(".history-table tbody tr").length,
    chip: document.querySelector(".history-panel .catalog-count")?.textContent.trim() ?? null,
  })`),
);
console.log(`  stored runs before: ${before.chip}`);

await ui.ev(`(() => {
  const root = document.querySelector(".lab-console") ?? document;
  const b = [...root.querySelectorAll("button")].find(x => x.textContent.trim().startsWith("RUN WITH MOCK") && !x.disabled);
  if (b) { b.click(); return true; }
  return false;
})()`);
await sleep(15000);

const afterRun = JSON.parse(
  await ui.ev(`JSON.stringify({
    runs: document.querySelectorAll(".history-table tbody tr").length,
    ids: [...document.querySelectorAll(".history-open")].map(b => b.getAttribute("title")),
  })`),
);
check("the run appears in the history immediately", afterRun.runs === before.runs + 1, `${before.runs} → ${afterRun.runs}`);
const newRunId = afterRun.ids[0];
console.log(`  newest run: ${newRunId}`);

console.log("=== Closing the application ===");
ui.close();
app.kill();
await sleep(4000);

console.log("=== Session 2: the same machine, a fresh process ===");
app = await launch();
ui = await connect();
await ui.ev(`location.hash = "#/lab"`);
await sleep(3500);

const afterRestart = JSON.parse(
  await ui.ev(`JSON.stringify({
    runs: document.querySelectorAll(".history-table tbody tr").length,
    ids: [...document.querySelectorAll(".history-open")].map(b => b.getAttribute("title")),
    chip: document.querySelector(".history-panel .catalog-count")?.textContent.trim() ?? null,
  })`),
);
check(
  "the run survives a restart",
  afterRestart.ids.includes(newRunId),
  `${afterRestart.chip} stored; newest ${afterRestart.ids[0]}`,
);

console.log("=== Reopening it after the restart ===");
await ui.ev(`(() => {
  const rows = [...document.querySelectorAll(".history-open")];
  const row = rows.find(r => r.getAttribute("title") === ${JSON.stringify(newRunId)});
  if (row) { row.click(); return true; }
  return false;
})()`);
await sleep(3000);

const reopened = JSON.parse(
  await ui.ev(`JSON.stringify({
    error: document.querySelector(".brain-panel .brain-error")?.textContent.trim() ?? null,
    open: document.querySelectorAll('.history-open[aria-current="true"]').length,
    facts: [...document.querySelectorAll(".brain-facts dt")].map(d => d.textContent.trim()),
    score: [...document.querySelectorAll(".brain-facts dd")].map(d => d.textContent.trim()).slice(0, 6),
  })`),
);
check("it reopens with no error", reopened.error === null, reopened.error ?? "");
check("it is marked as the loaded run", reopened.open === 1);
check("it reports its real measurements", reopened.facts.includes("Score"), reopened.facts.join(", "));
console.log(`  reopened facts: ${reopened.facts.slice(0, 8).join(", ")}`);

ui.close();
app.kill();

console.log("");
if (failures.length > 0) {
  console.log(`FAILED: ${failures.length} check(s)`);
  for (const failure of failures) console.log(`  - ${failure}`);
  process.exit(1);
}
console.log("Restart durability verified.");