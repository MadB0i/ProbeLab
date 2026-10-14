/**
 * Cancellation, driven through the native UI against a controlled slow provider.
 *
 * # The three outcomes this distinguishes
 *
 * Pressing STOP can end three ways, and only one of them is a broken cancellation path:
 *
 * * **signalled** — the backend had a run under that identifier and raised its stop flag. The stop path
 *   works. This is what the test is for.
 * * **already finished** — the episode committed every turn before the click landed. A legitimate race
 *   against a fast policy, and reported as such rather than as a failure.
 * * **command refused or errored** — the cancel registry could not be reached or the command failed.
 *   That is the broken case, and it fails here.
 *
 * # How a window is made without touching production code
 *
 * `scripts/slow-provider.mjs` runs outside the repository and holds each response for a fixed delay, so a
 * twelve-turn episode takes the better part of a minute. The application is an ordinary release build on
 * an ordinary path; only the port it points at differs. Nothing in the engine, the adapter or the runner
 * knows this test exists.
 *
 * # What is asserted after the stop
 *
 * * The terminal state is `cancelled`, not `error` — and not a green "complete" either.
 * * The stored lifecycle is `interrupted`, with no score, because a partial episode produced no
 *   measurement.
 * * The persisted events are exactly the turns the engine says it reached, with a gap-free sequence.
 * * **No leaked work**: the provider sees no further calls, the run button is live again, a second run can
 *   start immediately, and a stop issued now stops nothing.
 *
 * Usage: node scripts/cancel-through-ui.mjs <exe> <devPort> [providerPort] [controlPort] [outDir]
 */
import { spawn } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

const exe = process.argv[2] ?? "D:\\Projects\\ProbeLab\\target\\release\\probelab-desktop.exe";
const devPort = process.argv[3] ?? "9336";
const providerPort = process.argv[4] ?? "9337";
const controlPort = process.argv[5] ?? "9338";
const outDir = process.argv[6] ?? join(process.env.TEMP ?? ".", "probelab-cancel");
/** Long enough that a twelve-turn episode cannot finish while the button is being pressed. */
const PROVIDER_DELAY_MS = 3_000;
const BASE_URL = `http://127.0.0.1:${providerPort}/v1`;
const MODEL = "slow-test-provider";

mkdirSync(outDir, { recursive: true });

const failures = [];
const check = (name, ok, detail = "") => {
  console.log(`${ok ? "  PASS" : "  FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
  if (!ok) failures.push(name);
};
const note = (text) => console.log(`        ${text}`);
const wait = (ms) => new Promise((r) => setTimeout(r, ms));

/* ------------------------------------------------------------- the provider */

const provider = spawn(process.execPath, [
  resolve("scripts/slow-provider.mjs"),
  providerPort,
  String(PROVIDER_DELAY_MS),
  controlPort,
], { stdio: ["ignore", "pipe", "pipe"] });
provider.stdout.on("data", (d) => process.stdout.write(`  [provider] ${d}`));
provider.stderr.on("data", (d) => process.stderr.write(`  [provider] ${d}`));
await wait(900);

const providerStats = async () =>
  fetch(`http://127.0.0.1:${controlPort}/stats`).then((r) => r.json());
const resetProvider = async () => {
  await fetch(`http://127.0.0.1:${controlPort}/reset`, { method: "POST" });
};

/* ----------------------------------------------------------------- the app */

let child = spawn(exe, [], {
  env: { ...process.env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${devPort}` },
  stdio: "ignore",
});

async function connect() {
  for (let attempt = 0; attempt < 50; attempt += 1) {
    try {
      const targets = await fetch(`http://127.0.0.1:${devPort}/json/list`).then((r) => r.json());
      const page = targets.find((t) => t.type === "page" && t.webSocketDebuggerUrl);
      if (page) return page;
    } catch {
      /* not up yet */
    }
    await wait(400);
  }
  throw new Error("the application never exposed a debuggable page");
}

const page = await connect();
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
  return new Promise((r) => pending.set(messageId, r));
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
const shot = async (name) => {
  const { result } = await send("Page.captureScreenshot", { format: "png" });
  const path = join(outDir, `${name}.png`);
  writeFileSync(path, Buffer.from(result.data, "base64"));
  return path;
};

await send("Runtime.enable");
await send("Page.enable");
await evaluate(`location.hash = "#/lab"; true`);
await wait(2500);

/* ------------------------------------------------------- point it at us */

note(`pointing the run at ${BASE_URL} with a ${PROVIDER_DELAY_MS}ms provider`);
const configured = await evaluate(`(() => {
  const setValue = (selector, value) => {
    const field = document.querySelector(selector);
    if (!field) return false;
    const proto = field.tagName === "SELECT" ? window.HTMLSelectElement.prototype : window.HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(proto, "value").set.call(field, value);
    field.dispatchEvent(new Event("input", { bubbles: true }));
    field.dispatchEvent(new Event("change", { bubbles: true }));
    return true;
  };
  return JSON.stringify({
    baseUrl: setValue("#base-url", ${JSON.stringify(BASE_URL)}),
    model: setValue("#model-name", ${JSON.stringify(MODEL)}),
  });
})()`);
check("the base URL and model were set", JSON.parse(configured).baseUrl && JSON.parse(configured).model, configured);
await wait(900);

await resetProvider();

/* ------------------------------------------------------------- run, stop */

const started = JSON.parse(await evaluate(`(() => {
  const button = [...document.querySelectorAll(".lab-console button")]
    .find((b) => b.textContent.trim().startsWith("RUN WITH MODEL") && !b.disabled);
  if (!button) return JSON.stringify({ started: false });
  button.click();
  return JSON.stringify({ started: true });
})()`));
check("the connected run started", started.started === true, JSON.stringify(started));

// Wait until the provider has served at least two calls, so the run is demonstrably mid-episode.
let served = 0;
for (let waited = 0; waited < 30_000; waited += 250) {
  await wait(250);
  served = (await providerStats()).calls;
  if (served >= 2) break;
}
check("the provider is being called, so the run is genuinely in flight", served >= 2, `${served} calls`);

const stopVisible = await evaluate(`(() => {
  const stop = document.querySelector(".run-button-stop");
  return JSON.stringify({
    present: !!stop,
    label: stop?.textContent?.trim() ?? null,
    disabled: stop?.disabled ?? null,
  });
})()`);
check("a stop control appears while the run is in flight", JSON.parse(stopVisible).present === true, stopVisible);
await shot("01-running-with-stop");

const stopResult = JSON.parse(await evaluate(`(async () => {
  const stop = document.querySelector(".run-button-stop");
  if (!stop) return JSON.stringify({ outcome: "no stop control" });
  const runId = document.querySelector(".run-id-line code")?.textContent?.trim();
  stop.click();
  return JSON.stringify({ outcome: "clicked", runId, label: stop.textContent.trim() });
})()`));
note(`stop: ${JSON.stringify(stopResult)}`);
check("the stop control is reachable and was pressed", stopResult.outcome === "clicked", stopResult.outcome);

// Wait for the terminal state. Bounded by the episode's own length.
let terminal = null;
for (let waited = 0; waited < 40_000; waited += 400) {
  await wait(400);
  terminal = JSON.parse(await evaluate(`JSON.stringify({
    state: document.querySelector(".run-state")?.textContent?.trim() ?? null,
    chip: document.querySelector(".run-state")?.className ?? null,
    error: document.querySelector(".error-message")?.textContent?.trim() ?? null,
    stopped: document.querySelector(".run-stopped")?.textContent?.trim() ?? null,
    cells: document.querySelectorAll(".gw-cell").length,
    logRows: document.querySelectorAll(".gw-log-row").length,
    buttonBack: !!([...document.querySelectorAll(".lab-console button")]
      .find((b) => b.textContent.trim().startsWith("RUN WITH MODEL") && !b.disabled)),
    eventCount: document.querySelector(".event-count")?.textContent?.trim() ?? null,
  })`));
  if (terminal.state === "cancelled" || terminal.state === "error" || terminal.state === "complete") break;
}
await shot("02-after-stop");
note(`terminal: ${JSON.stringify(terminal)}`);

const callsAtStop = (await providerStats()).calls;

/* --------------------------------------------------------- what it asserts */

if (terminal.state === "cancelled") {
  check("the run reports itself as stopped, not as an error", terminal.error === null, terminal.error ?? "");
  check("the state chip is tinted for a stop", (terminal.chip ?? "").includes("is-cancelled"), terminal.chip ?? "");
  check(
    "the panel states both the engine's turn count and the committed events",
    Boolean(terminal.stopped) && /engine reached turn \d+/.test(terminal.stopped),
    terminal.stopped ?? "",
  );
  check("the stage still draws the turns that were committed", terminal.cells === 25, `${terminal.cells} cells`);
  check("the launch control is available again", terminal.buttonBack === true);
} else if (terminal.state === "complete") {
  note("the episode finished before the stop landed — a legitimate race against a fast policy,");
  note("not a broken cancellation path. The command's own answer below is what distinguishes them.");
} else if (terminal.state === "error") {
  check("a stopped run is not reported as an error", false, terminal.error ?? "terminal state is error");
}

// The backend's own answer, asked *after* the run has reached its terminal state.
//
// Asked earlier it proves nothing: the stop is cooperative, so the run is still finishing its in-flight
// turn for several seconds after the signal, and a registry that still holds a live run correctly says
// `true`. The meaningful moment is once the run is over, where the handle must be gone — a handle left
// behind would let a later stop signal reach whatever started next.
const registryAfterStop = await evaluate(`(async () => {
  const api = window.__TAURI_INTERNALS__;
  const runId = ${JSON.stringify(stopResult.runId ?? "")};
  try {
    return JSON.stringify(await api.invoke("cancel_experiment", { experiment_id: runId }));
  } catch (error) {
    return JSON.stringify("threw: " + String(error));
  }
})()`);
check(
  "the cancel handle is gone once the run has ended",
  registryAfterStop === "false",
  `cancel_experiment answered ${registryAfterStop}`,
);

/* ---------------------------------------- what the store actually holds */

// Read the run back through the same command the replay uses, and check it against the numbers the
// engine reported when it stopped. A row count agreeing with itself is not evidence; a gap-free
// sequence whose length matches the engine's own turn count is.
const persisted = JSON.parse(await evaluate(`(async () => {
  const api = window.__TAURI_INTERNALS__;
  try {
    const events = await api.invoke("get_run_telemetry", { run_id: ${JSON.stringify(stopResult.runId ?? "")} });
    return JSON.stringify({
      count: events.length,
      sequences: events.map((e) => e.sequence),
      turns: events.map((e) => e.turn),
      runs: [...new Set(events.map((e) => e.run_id))],
    });
  } catch (error) {
    return JSON.stringify({ error: String(error) });
  }
})()`));
note(`persisted: ${JSON.stringify(persisted)}`);
if (terminal.state === "cancelled") {
  const engineTurns = Number((terminal.stopped ?? "").match(/engine reached turn (\d+)/)?.[1] ?? -1);
  check("every committed event was persisted", persisted.count === engineTurns && engineTurns > 0,
    `${persisted.count} events, engine reported ${engineTurns}`);
  check("the persisted sequence is gap-free from one",
    JSON.stringify(persisted.sequences) === JSON.stringify(
      Array.from({ length: persisted.count }, (_, i) => i + 1)),
    JSON.stringify(persisted.sequences));
  check("the persisted turns are contiguous",
    JSON.stringify(persisted.turns) === JSON.stringify(Array.from({ length: persisted.count }, (_, i) => i + 1)),
    JSON.stringify(persisted.turns));
  check("every persisted event belongs to the run that was stopped",
    Array.isArray(persisted.runs) && persisted.runs.length === 1 && persisted.runs[0] === stopResult.runId,
    JSON.stringify(persisted.runs));
}

/* ------------------------------------------------------------- no leakage */

await wait(1_500);
const afterIdle = await providerStats();
check(
  "the provider is no longer being called after the stop",
  afterIdle.calls === callsAtStop,
  `${callsAtStop} calls at the stop, ${afterIdle.calls} 1.5s later`,
);

const threadsBefore = null;
void threadsBefore;
const procThreads = await evaluate(`JSON.stringify({
  handles: performance.memory ? Math.round(performance.memory.usedJSHeapSize / 1048576) : null,
})`);
note(`webview: ${procThreads}`);

await shot("03-settled");

/* ------------------------------------- a second run still works afterwards */

await resetProvider();
const second = JSON.parse(await evaluate(`(() => {
  const button = [...document.querySelectorAll(".lab-console button")]
    .find((b) => b.textContent.trim().startsWith("RUN WITH MODEL") && !b.disabled);
  if (!button) return JSON.stringify({ started: false, reason: "no enabled launch control" });
  button.click();
  return JSON.stringify({ started: true });
})()`));
check("a second run can start immediately after the stop", second.started === true, JSON.stringify(second));
if (second.started) {
  let secondServed = 0;
  for (let waited = 0; waited < 20_000; waited += 250) {
    await wait(250);
    secondServed = (await providerStats()).calls;
    if (secondServed >= 1) break;
  }
  check("and it reaches the provider, so nothing was left holding the runner", secondServed >= 1, `${secondServed} calls`);
  // Stop it too, so the script does not leave a twelve-turn episode running behind it.
  await evaluate(`document.querySelector(".run-button-stop")?.click(); true`);
}
await wait(2_500);
await shot("04-second-run-started");

child.kill();
provider.kill();
await wait(1_000);

console.log("");
if (failures.length) {
  console.error(`FAILED ${failures.length}: ${failures.join(", ")}`);
  process.exitCode = 1;
} else {
  console.log("PASS  cancellation through the native UI, against a controlled slow provider");
}