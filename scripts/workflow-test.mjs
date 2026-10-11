/**
 * Drives primary workflows on a running ProbeLab window via CDP and captures screenshots.
 *
 * Usage: node scripts/workflow-test.mjs <port> <outDir>
 */
const port = process.argv[2] ?? "9333";
const outDir = process.argv[3] ?? process.env.TEMP + "\\opencode\\workflow";

import { writeFileSync, mkdirSync } from "node:fs";

mkdirSync(outDir, { recursive: true });

const target = await fetch(`http://127.0.0.1:${port}/json/list`)
  .then((r) => r.json())
  .then((ts) => ts.find((t) => t.type === "page" && t.webSocketDebuggerUrl));

if (!target) {
  console.error("no page target");
  process.exit(2);
}

const socket = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((res, rej) => {
  socket.addEventListener("open", res, { once: true });
  socket.addEventListener("error", rej, { once: true });
});

let id = 0;
const pending = new Map();
const errors = [];

socket.addEventListener("message", (e) => {
  const m = JSON.parse(e.data);
  if (m.id && pending.has(m.id)) {
    const { res, rej } = pending.get(m.id);
    pending.delete(m.id);
    m.error ? rej(new Error(m.error.message)) : res(m.result);
    return;
  }
  if (m.method === "Runtime.exceptionThrown") {
    errors.push(m.params.exceptionDetails.text);
  }
  if (m.method === "Runtime.consoleAPICalled" && m.params.type === "error") {
    errors.push(m.params.args.map((a) => a.value ?? a.description ?? "").join(" "));
  }
});

function send(method, params = {}) {
  const i = ++id;
  socket.send(JSON.stringify({ id: i, method, params }));
  return new Promise((res, rej) => pending.set(i, { res, rej }));
}

async function evalJS(expression) {
  const { result } = await send("Runtime.evaluate", { expression, returnByValue: true });
  return result.value;
}

async function nav(route, settle = 1200) {
  await evalJS(`location.hash = "#/${route}"`);
  await new Promise((r) => setTimeout(r, settle));
}

async function clickButton(text, settle = 800) {
  const found = await evalJS(`(() => {
    const btns = [...document.querySelectorAll("button")];
    const b = btns.find(b => b.textContent.trim().toLowerCase().includes(${JSON.stringify(text.toLowerCase())}) && !b.disabled);
    if (!b) return false;
    b.click();
    return true;
  })()`);
  if (!found) console.log(`  button not found or disabled: "${text}"`);
  await new Promise((r) => setTimeout(r, settle));
  return found;
}

async function screenshot(name) {
  const { data } = await send("Page.captureScreenshot", { format: "png" });
  const path = `${outDir}\\${name}.png`;
  writeFileSync(path, Buffer.from(data, "base64"));
  console.log(`  saved ${path}`);
}

async function textSnapshot(selector) {
  return evalJS(`document.querySelector(${JSON.stringify(selector)})?.textContent?.trim().slice(0, 200) ?? ""`);
}

await send("Page.enable");
await send("Runtime.enable");
await send("Log.enable");

console.log("=== 1. Readiness: preflight ===");
await nav("readiness", 1500);
await clickButton("preflight", 2500);
const preflight = await textSnapshot(".preflight-checks");
console.log(`  preflight checks: ${preflight ? preflight.slice(0, 120) : "(none)"}`);
await screenshot("01-readiness-preflight");

console.log("=== 2. Command Center: run with mock ===");
await nav("lab", 1500);
await clickButton("run with mock", 4000);
const progress = await textSnapshot(".progress-value") || await textSnapshot(".metric-value");
console.log(`  progress after mock: "${progress}"`);
const runId = await textSnapshot(".mono");
await screenshot("02-lab-mock-run");

console.log("=== 3. Arena: play match ===");
await nav("arena", 1500);
await clickButton("play match", 3000);
const boardCells = await evalJS(`document.querySelectorAll(".arena-cell").length`);
const outcome = await textSnapshot(".arena-outcome-gloss") || await textSnapshot(".telemetry-value");
console.log(`  board cells: ${boardCells}, outcome/telemetry: "${outcome.slice(0, 100)}"`);
await screenshot("03-arena-match");

console.log("=== 4. Security: run sweep ===");
await nav("security", 1500);
await clickButton("run security sweep", 3500);
const sweepResult = await textSnapshot(".security-sweep-results") || await textSnapshot(".panel-note");
console.log(`  sweep result: "${sweepResult.slice(0, 140)}"`);
await screenshot("04-security-sweep");

console.log("=== 5. Tasks: generate package ===");
await nav("tasks", 1500);
await clickButton("generate package", 2500);
const pkg = await textSnapshot(".guided-package") || await textSnapshot(".panel-note");
console.log(`  package: "${pkg.slice(0, 140)}"`);
await screenshot("05-tasks-package");

console.log("=== 6. Replay after arena match ===");
await nav("replay", 2000);
await clickButton("load recording", 2000);
const replay = await textSnapshot(".replay-timeline") || await textSnapshot(".panel-note");
console.log(`  replay: "${replay.slice(0, 140)}"`);
await screenshot("06-replay-loaded");

console.log("=== 7. Comparison after run ===");
await nav("comparison", 2000);
const comp = await textSnapshot(".compare-") || await textSnapshot(".panel-note");
console.log(`  comparison: "${comp.slice(0, 140)}"`);
await screenshot("07-comparison-after");

if (errors.length) {
  console.log("\nCONSOLE ERRORS:");
  errors.forEach((e) => console.log("  " + e));
}
socket.close();
process.exit(errors.length ? 1 : 0);
