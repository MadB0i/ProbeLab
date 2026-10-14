/**
 * One real model, one grid episode, through the desktop application.
 *
 * Phase 5's gap was that the engine path was proven by an example binary but never driven from the
 * product: no run had gone through the IPC boundary, the event stream and the durable store with a real
 * model behind it. This closes that, and reports what the run actually produced rather than what it
 * should have.
 *
 * Everything printed here is read out of the live DOM after the run finishes: the turns it committed,
 * the reward, the distance reading, the evaluator's verdict and the token usage. Nothing is asserted in
 * advance, because the point is the measurement.
 *
 * Usage: node scripts/grid-connected-run.mjs <port> [model] [outDir]
 */
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const port = process.argv[2] ?? "9336";
const model = process.argv[3] ?? "qwen2.5:0.5b";
const outDir = process.argv[4] ?? join(process.env.TEMP ?? ".", "probelab-connected-run");
mkdirSync(outDir, { recursive: true });

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
  const response = await send("Runtime.evaluate", {
    expression,
    returnByValue: true,
    awaitPromise: true,
  });
  if (response.result?.exceptionDetails) {
    throw new Error(response.result.exceptionDetails.exception?.description ?? "evaluate failed");
  }
  return response.result?.result?.value;
};
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const failures = [];
const check = (name, ok, detail = "") => {
  console.log(`${ok ? "  PASS" : "  FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
  if (!ok) failures.push(name);
};

await send("Runtime.enable");
await send("Page.enable");
await evaluate(`location.hash = "#/lab"; true`);
await wait(2500);

console.log(`=== configuring the connected provider for ${model} ===`);
const configured = await evaluate(`(() => {
  const field = document.querySelector("#model-name");
  if (!field) return "no model field";
  const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value").set;
  setter.call(field, ${JSON.stringify(model)});
  field.dispatchEvent(new Event("input", { bubbles: true }));
  return field.value;
})()`);
check("the model field accepted the model name", configured === model, String(configured));
await wait(600);

const enabled = await evaluate(`(() => {
  const b = [...document.querySelectorAll(".lab-console button")]
    .find((x) => x.textContent.trim().startsWith("RUN WITH MODEL"));
  return JSON.stringify({ found: !!b, disabled: b ? b.disabled : null });
})()`);
check("the connected run control is enabled", JSON.parse(enabled).found && JSON.parse(enabled).disabled === false, enabled);
if (!JSON.parse(enabled).found || JSON.parse(enabled).disabled !== true) {
  // proceed
}
if (JSON.parse(enabled).disabled === true) {
  console.error("the connected run control is disabled; the provider is not reachable from this host");
  process.exitCode = 1;
  socket.close();
  process.exit(1);
}

console.log("=== running one episode against the real model ===");
const started = Date.now();
await evaluate(`(() => {
  const b = [...document.querySelectorAll(".lab-console button")]
    .find((x) => x.textContent.trim().startsWith("RUN WITH MODEL") && !x.disabled);
  b.click();
  return true;
})()`);

// Wait for the run to reach a terminal state, or give it a long episode's worth of time.
let finished = false;
for (let waited = 0; waited < 180_000 && !finished; waited += 500) {
  await wait(500);
  finished = await evaluate(`(() => {
    const label = document.querySelector(".run-state")?.textContent?.trim() ?? "";
    return label === "complete" || label === "error" || label === "idle";
  })()`);
}
const elapsed = ((Date.now() - started) / 1000).toFixed(1);

const outcome = JSON.parse(await evaluate(`(() => {
  const readout = Object.fromEntries(
    Array.from(document.querySelectorAll(".gw-readout dt")).map((dt) => [
      dt.textContent.trim(), dt.nextElementSibling.textContent.trim(),
    ]),
  );
  const metrics = Object.fromEntries(
    Array.from(document.querySelectorAll(".metric-grid > div")).map((d) => [
      d.querySelector("span")?.textContent?.trim(), d.querySelector("strong")?.textContent?.trim(),
    ]),
  );
  const details = Array.from(document.querySelectorAll(".result-details span")).map((s) => s.textContent.trim());
  return JSON.stringify({
    runState: document.querySelector(".run-state")?.textContent?.trim() ?? null,
    elapsed: ${elapsed},
    readout,
    metrics,
    details,
    verdict: document.querySelector(".gw-verdict-label")?.textContent?.trim() ?? null,
    verdictFacts: document.querySelector(".gw-verdict-facts")?.textContent?.trim() ?? null,
    boardLabel: document.querySelector(".gw-board")?.getAttribute("aria-label") ?? null,
    unseen: document.querySelectorAll('.gw-cell[data-tile="unseen"]').length,
    cells: document.querySelectorAll(".gw-cell").length,
    failures: Array.from(document.querySelectorAll(".telemetry-row .event-outcome span"))
      .map((s) => s.textContent.trim()),
    lastResponse: document.querySelector(".model-response")?.textContent?.trim().slice(0, 200) ?? null,
    error: document.querySelector(".error-message")?.textContent?.trim() ?? null,
  });
})()`));

const { result } = await send("Page.captureScreenshot", { format: "png" });
writeFileSync(join(outDir, "connected-run.png"), Buffer.from(result.data, "base64"));

console.log(JSON.stringify(outcome, null, 2));

check("the run reached a terminal state", ["complete", "idle"].includes(outcome.runState), String(outcome.runState));
check("no run error was reported", outcome.error === null, outcome.error ?? "");
check("the stage drew the episode", outcome.cells === 25, `${outcome.cells} cells, ${outcome.unseen} never sensed`);
check("the evaluator produced a verdict", Boolean(outcome.verdict), outcome.verdict ?? "none");
check("the run reported its provider and model",
  outcome.details.some((d) => d.startsWith("PROVIDER /")) && outcome.details.some((d) => d.startsWith("MODEL /")),
  outcome.details.filter((d) => d.startsWith("PROVIDER /") || d.startsWith("MODEL /")).join("  "));

console.log("");
if (failures.length) {
  console.error(`FAILED ${failures.length}: ${failures.join(", ")}`);
  process.exitCode = 1;
} else {
  console.log("PASS  a real model, one grid episode, through the desktop application");
}
socket.close();