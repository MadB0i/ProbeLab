/**
 * Exercises the Command Center's stored-run history and the Tasks package save, against the real exe.
 *
 * Usage: node scripts/lab-and-tasks.mjs [port] [outDir]
 */
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const port = process.argv[2] ?? "9333";
const outDir = process.argv[3] ?? join(process.env.TEMP ?? ".", "probelab-lab");
mkdirSync(outDir, { recursive: true });

const target = await fetch(`http://127.0.0.1:${port}/json/list`)
  .then((r) => r.json())
  .then((ts) => ts.find((t) => t.type === "page" && t.webSocketDebuggerUrl));
const socket = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((res) => socket.addEventListener("open", res, { once: true }));
let nextId = 0;
const pending = new Map();
socket.addEventListener("message", (e) => {
  const m = JSON.parse(e.data);
  if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); }
});
function send(method, params = {}) {
  const i = ++nextId;
  socket.send(JSON.stringify({ id: i, method, params }));
  return new Promise((r) => pending.set(i, r));
}
const ev = async (expr) => (await send("Runtime.evaluate", { expression: expr, returnByValue: true }))?.result?.result?.value;
const wait = (ms) => new Promise((r) => setTimeout(r, ms));
const go = async (route) => { await ev(`location.hash = "#/${route}"`); await wait(1500); };
const shot = async (name) => {
  const { result } = await send("Page.captureScreenshot", { format: "png" });
  writeFileSync(join(outDir, `${name}.png`), Buffer.from(result.data, "base64"));
  console.log(`  saved ${name}.png`);
};
const failures = [];
const check = (name, ok, detail = "") => {
  console.log(`${ok ? "  PASS" : "  FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
  if (!ok) failures.push(name);
};

await send("Runtime.enable");
await send("Page.enable");

console.log("=== 1. Command Center: the stored-run history ===");
await go("lab");
await wait(2500);

const before = JSON.parse(await ev(`JSON.stringify({
  rows: document.querySelectorAll(".history-table tbody tr").length,
  count: document.querySelector(".history-panel .catalog-count")?.textContent.trim() ?? null,
  error: document.querySelector(".history-panel .panel-note")?.textContent.trim() ?? null,
})`));
console.log(`  stored runs: ${before.count} (${before.rows} rows)`);
await shot("01-lab-history");

console.log("=== 2. Run a mock benchmark, then confirm the history grows ===");
// The Lab's primary control runs a benchmark through `run_benchmark`.
await ev(`(() => {
  const root = document.querySelector(".lab-console") ?? document;
  const b = [...root.querySelectorAll("button")].find(x => x.textContent.trim().startsWith("RUN WITH MOCK") && !x.disabled);
  if (b) { b.click(); return true; }
  return false;
})()`);
await wait(14000);

const after = JSON.parse(await ev(`JSON.stringify({
  rows: document.querySelectorAll(".history-table tbody tr").length,
  count: document.querySelector(".history-panel .catalog-count")?.textContent.trim() ?? null,
  next: document.querySelector(".brain-next") ? [...document.querySelectorAll(".brain-next button")].map(b=>b.textContent.trim()) : [],
  facts: [...document.querySelectorAll(".brain-facts dt")].map(d => d.textContent.trim()),
  confidence: document.querySelector(".brain-facts")?.textContent.includes("Confidence") ?? false,
  elapsed: document.querySelector(".brain-facts")?.textContent.includes("Took") ?? false,
  pills: [...document.querySelectorAll(".history-table .lifecycle-pill")].map(p=>p.textContent.trim()),
})`));
check("the history grew after a run", after.rows > before.rows, `${before.rows} → ${after.rows}`);
check("a lifecycle is shown per row", after.pills.length > 0, after.pills.slice(0, 3).join(", "));
// The next-step bar, confidence interval and duration belong to the Lab Brain's own experiment
// (`run_experiment`), which is a different command from the benchmark run above. Asserting them here
// would be asserting a different workflow than the one this script drives.
check("the run button is re-enabled after a run finishes",
  await ev(`(()=>{const b=[...document.querySelectorAll(".lab-console button")].find(x=>x.textContent.trim().startsWith("RUN WITH MOCK")); return !!b && !b.disabled})()`));
await shot("02-lab-run-complete");

console.log("=== 2b. Lab Brain: run an experiment and check its next-step bar ===");
await ev(`(() => {
  const root = document.querySelector(".lab-dock") ?? document;
  const b = [...root.querySelectorAll("button")].find(x => x.textContent.trim() === "RUN EXPERIMENT" && !x.disabled);
  if (b) { b.click(); return true; }
  return "not available: " + [...root.querySelectorAll("button")].map(x=>x.textContent.trim()).join(" | ");
})()`);
await wait(16000);
const brain = JSON.parse(await ev(`JSON.stringify({
  next: document.querySelector(".brain-next") ? [...document.querySelectorAll(".brain-next button")].map(b=>b.textContent.trim()) : [],
  facts: [...document.querySelectorAll(".brain-facts dt")].map(d => d.textContent.trim()),
  runButtonEnabled: (()=>{const b=[...document.querySelectorAll(".lab-dock button")].find(x=>x.textContent.trim().startsWith("RUN EXPERIMENT")); return !!b && !b.disabled})(),
})`));
check("the Brain's next-step actions appear when its run finishes", brain.next.length >= 3, brain.next.join(" | "));
check("a confidence interval is reported", brain.facts.includes("Confidence"), brain.facts.join(", "));
check("a duration is reported when recorded", brain.facts.includes("Took"), brain.facts.join(", "));
check("the Brain's run button is re-enabled after success", brain.runButtonEnabled);
await shot("02b-brain-run-complete");

console.log("=== 3. Load a stored run back ===");
await ev(`document.querySelectorAll(".history-open")[document.querySelectorAll(".history-open").length - 1].click()`);
await wait(2500);
const loaded = JSON.parse(await ev(`JSON.stringify({
  open: document.querySelectorAll('.history-open[aria-current="true"]').length,
  hasFacts: !!document.querySelector(".brain-facts"),
})`));
check("a stored run loads back into the reporting panels", loaded.open === 1 && loaded.hasFacts);
await shot("03-stored-run-loaded");

console.log("=== 4. Tasks: generate and save a package ===");
// A fresh seed each run: the save deliberately refuses to overwrite an existing folder, so reusing a
// fixed seed would make this check fail on the second run for the right reason and the wrong cause.
const seed = String(2_000_000 + Math.floor(Math.random() * 900_000));
await go("tasks");
await ev(`(() => {
  const input = [...document.querySelectorAll("input")].find(i => i.inputMode === "numeric");
  if (input) {
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value").set;
    setter.call(input, ${JSON.stringify(seed)});
    input.dispatchEvent(new Event("input", { bubbles: true }));
  }
  const root = document.querySelector(".tasks-panel, .panel") ?? document;
  const b = [...root.querySelectorAll("button")].find(x => x.textContent.trim() === "GENERATE PACKAGE" && !x.disabled);
  if (b) { b.click(); return true; }
  return false;
})()`);
await wait(2000);
const generated = await ev(`JSON.stringify({
  hasSave: [...document.querySelectorAll("button")].some(b => b.textContent.includes("SAVE PACKAGE")),
  files: document.querySelectorAll('.guided-package li').length,
})`);
check("a save action appears once a package exists", JSON.parse(generated).hasSave, generated);
check("the package lists its files", JSON.parse(generated).files > 0, `${JSON.parse(generated).files} files`);
await shot("04-tasks-package-generated");

await ev(`(() => {
  const b = [...document.querySelectorAll("button")].find(x => x.textContent.includes("SAVE PACKAGE") && !x.disabled);
  if (b) { b.click(); return true; }
  return false;
})()`);
await wait(3000);
const saved = JSON.parse(await ev(`JSON.stringify({
  saved: document.querySelector(".guided-saved")?.textContent.trim() ?? null,
  error: document.querySelector(".brain-error")?.textContent.trim() ?? null,
})`));
check("the package is written to disk", !!saved.saved, saved.saved ?? saved.error ?? "nothing reported");
check("no error is reported", saved.error === null, saved.error ?? "");
await shot("05-tasks-package-saved");

console.log("");
if (failures.length > 0) {
  console.log(`FAILED: ${failures.length} check(s)`);
  for (const f of failures) console.log(`  - ${f}`);
  process.exit(1);
}
console.log("All lab and tasks checks passed.");
socket.close();