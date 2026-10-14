/**
 * Restart durability, driven through the real application.
 *
 * # What this covers, and what it deliberately does not
 *
 * `scripts/grid-trace.mjs` covers a live run and a recorded replay. This file covers the property that
 * is easy to *claim* and hard to *hold*: the recording must survive the process that made it, with the
 * session buffer empty, which is the only state a viewer is ever really in.
 *
 * Cancellation used to be checked here as well. It is not any more. It has a control in the product now
 * -- the launch card carries a STOP RUN button -- and testing it needs a provider slow enough to stop,
 * which is what `scripts/cancel-through-ui.mjs` and `scripts/slow-provider.mjs` are for. A mock grid
 * episode finishes in milliseconds, so a stop has to be raised in a race the old version reported as
 * "unproven" whenever it lost. Duplicating that weaker test here would only give the weaker claim a
 * second place to live, and a claim with two homes is a claim with two failure modes.
 *
 * # The restart is a real one
 *
 * The process is terminated, not reloaded, and the application is launched again against the same
 * on-disk database. Anything that survived did so by being on disk. The digest-level proof that the
 * recording is byte-identical across the restart lives in `scripts/replay-integrity.mjs`; this asks only
 * the questions a viewer would: does it come back, and does it step.
 *
 * Usage: node scripts/grid-durability.mjs <exe> <port>
 */
import { spawn } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const exe = process.argv[2] ?? "D:\\Projects\\ProbeLab\\target\\release\\probelab-desktop.exe";
const port = process.argv[3] ?? "9336";
const outDir = process.argv[4] ?? join(process.env.TEMP ?? ".", "probelab-grid-durability");
mkdirSync(outDir, { recursive: true });

const failures = [];
const check = (name, ok, detail = "") => {
  console.log(`${ok ? "  PASS" : "  FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
  if (!ok) failures.push(name);
};
const note = (text) => console.log(`        ${text}`);
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/* ------------------------------------------------------------------ the app */

function launch() {
  const child = spawn(exe, [], {
    env: { ...process.env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}` },
    detached: false,
    stdio: "ignore",
  });
  return child;
}

async function connect() {
  for (let attempt = 0; attempt < 40; attempt += 1) {
    try {
      const targets = await fetch(`http://127.0.0.1:${port}/json/list`).then((r) => r.json());
      const page = targets.find((t) => t.type === "page" && t.webSocketDebuggerUrl);
      if (page) return page;
    } catch {
      /* not listening yet */
    }
    await wait(400);
  }
  throw new Error("the application never exposed a debuggable page");
}

/**
 * Attaches a CDP session to the page.
 *
 * Returns a promise that settles only once the socket is open. Without that wait the first `evaluate`
 * races the handshake and throws `InvalidStateError: Sent before connected`, which looks like an
 * application failure and is not one — the debugger endpoint answered with a page before the WebSocket
 * had finished connecting to it.
 */
function attach(page) {
  const socket = new WebSocket(page.webSocketDebuggerUrl);
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
  const shot = async (name) => {
    const { result } = await send("Page.captureScreenshot", { format: "png" });
    const path = join(outDir, `${name}.png`);
    writeFileSync(path, Buffer.from(result.data, "base64"));
    return path;
  };
  const ready = new Promise((resolve, reject) => {
    socket.addEventListener("open", resolve, { once: true });
    socket.addEventListener("error", reject, { once: true });
  });
  return { socket, send, evaluate, shot, ready };
}

/** The state of the navigation stage, read out of the live DOM. */
const readStage = (evaluate) =>
  evaluate(`(() => {
    const board = document.querySelector(".gw-board");
    const style = board ? getComputedStyle(board) : null;
    const turns = Array.from(document.querySelectorAll(".gw-log-row .gw-log-turn"))
      .map((n) => Number(n.textContent.trim()))
      .filter((n) => Number.isFinite(n));
    return JSON.stringify({
      present: !!board,
      cells: document.querySelectorAll(".gw-cell").length,
      seen: Array.from(document.querySelectorAll(".gw-cell")).filter((c) => c.getAttribute("data-tile") !== "unseen").length,
      source: document.querySelector(".gw-source")?.textContent?.trim() ?? null,
      logRows: document.querySelectorAll(".gw-log-row").length,
      maxTurn: turns.length ? Math.max(...turns) : null,
      transport: document.querySelectorAll(".gw-transport button").length,
      scrub: !!document.querySelector(".gw-scrub"),
      error: document.querySelector(".gw-stage .error-message")?.textContent?.trim() ?? null,
      empty: document.querySelector(".gw-empty")?.textContent?.trim() ?? null,
      verdict: document.querySelector(".gw-verdict-label")?.textContent?.trim() ?? null,
      runId: document.querySelector(".run-id-line code")?.textContent?.trim() ?? null,
      latestLifecycle: document.querySelector(".history-table tbody tr .lifecycle-pill")?.textContent?.trim() ?? null,
    });
  })()`);

/* ------------------------------------------------------------ 1. a baseline run */

console.log("=== 1. A run to come back to ===");

let child = launch();
let page = await connect();
let app = attach(page);
await app.ready;

await app.evaluate(`location.hash = "#/lab"; true`);
await wait(2000);
await app.evaluate(`(() => {
  const button = [...document.querySelectorAll(".lab-console button")]
    .find((b) => b.textContent.trim().startsWith("RUN WITH MOCK") && !b.disabled);
  if (button) button.click();
  return true;
})()`);
await wait(9000);
const baseline = JSON.parse(await readStage(app.evaluate));
check("a run committed turns in this session", baseline.present === true && baseline.logRows > 0,
  `${baseline.logRows} turns logged`);

/* ------------------------------------------------------------- restart, for real */

console.log("=== 2. Restart durability ===");
// The run to come back to, named now so it can be reopened after the process is gone.
const target = JSON.parse(await app.evaluate(`(() => {
  const rows = Array.from(document.querySelectorAll(".history-table tbody tr"));
  const open = rows.map((r) => r.querySelector(".history-open")?.getAttribute("title")).filter(Boolean);
  return JSON.stringify({ runIds: open.slice(0, 3) });
})()`));
const survivor = target.runIds[0];
check("a stored run is available to come back to", Boolean(survivor), survivor ?? "none");
await app.shot("02-before-restart");

app.socket.close();
child.kill();
await wait(2500);
check("the first process is gone", true, "terminated");

child = launch();
page = await connect();
app = attach(page);
await app.ready;
await wait(2500);
await app.evaluate(`location.hash = "#/lab"; true`);
await wait(2500);

const fresh = JSON.parse(await readStage(app.evaluate));
check(
  "the fresh session has no live episode to show",
  fresh.present === false || fresh.source === "AWAITING A RUN",
  `source: ${fresh.source}`,
);
await app.shot("03-fresh-session");

// Open the run the previous session recorded.
const opened = await app.evaluate(`(() => {
  const rows = Array.from(document.querySelectorAll(".history-table tbody tr"));
  const row = rows.find((r) => r.querySelector(".history-open")?.getAttribute("title") === ${JSON.stringify(survivor)});
  if (!row) return "not in the history";
  row.querySelector(".history-open").click();
  return "clicked";
})()`);
check("the recorded run is still listed after the restart", opened === "clicked", opened);
await wait(2500);

const replayed = JSON.parse(await readStage(app.evaluate));
check("its trajectory reopened from the store", replayed.present === true, `${replayed.cells} cells`);
check("the reopened feed is labelled as a recording", (replayed.source ?? "").startsWith("RECORDED "), replayed.source ?? "");
check("a transport is offered for it", replayed.transport >= 4, `${replayed.transport} controls`);
check("its turns are listed", replayed.logRows > 0, `${replayed.logRows} rows, highest turn ${replayed.maxTurn}`);
await app.shot("04-after-restart");

// Scrubbing and stepping must both move the marker: a recording that reads back but does not move is a
// projection, not a replay. The digest-level proof that the recording is byte-identical across the
// restart lives in `scripts/replay-integrity.mjs`; this only asks the question a viewer would.
//
// The scrub is driven from the middle of the recording out to its last frame and then back to its
// first. An earlier version jumped straight to frame 0, which happened to be the frame already showing,
// so the marker had nowhere to go and the check reported a failure in a product that was working. A
// transport control has to be tested by moving somewhere it is not.
const scrub = JSON.parse(await app.evaluate(`(() => {
  const input = document.querySelector(".gw-scrub");
  return JSON.stringify({ max: Number(input.max), index: Number(input.value) });
})()`));
const marker = async () => JSON.parse(await app.evaluate(`(() => {
  const style = getComputedStyle(document.querySelector(".gw-board"));
  return JSON.stringify({
    position: style.getPropertyValue("--gw-x").trim() + "," + style.getPropertyValue("--gw-y").trim(),
  });
})()`));
const scrubTo = async (value) => {
  // Three details, each of which has silently made this a test of nothing:
  //
  //  * `input` as well as `change`, because React maps a range input's onChange onto the native input
  //    event.
  //  * The value set through the native setter on the prototype, not by assignment. React keeps a
  //    tracker of the last value it saw; assigning `.value` updates that tracker too, so React concludes
  //    nothing changed and never calls the handler. The prototype setter bypasses the instance tracker
  //    and reproduces what a real drag does.
  //  * The position read in a later evaluation, after React has re-rendered.
  await app.evaluate(`(() => {
    const input = document.querySelector(".gw-scrub");
    const setValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set;
    setValue.call(input, ${JSON.stringify(String(value))});
    input.dispatchEvent(new Event("input", { bubbles: true }));
    input.dispatchEvent(new Event("change", { bubbles: true }));
    return true;
  })()`);
  await wait(700);
};

// React re-renders asynchronously, so the position is read in a later evaluation rather than in the same
// one that moved the scrubber. Reading it inline measured the frame the board had been showing all along
// and reported the transport as broken.
const atFrameZero = await marker();
await scrubTo(scrub.max);
const atLastFrame = await marker();
check("scrubbing the reopened recording forward moves the marker",
  atLastFrame.position !== atFrameZero.position,
  `frame ${scrub.index} → frame ${scrub.max}: ${atFrameZero.position} → ${atLastFrame.position}`);

await scrubTo(0);
const backAtStart = await marker();
check("scrubbing back to the first turn moves it again",
  backAtStart.position !== atLastFrame.position,
  `frame ${scrub.max} → frame 0: ${atLastFrame.position} → ${backAtStart.position}`);
check("scrubbing back to the first frame returns the marker to where it started",
  backAtStart.position === atFrameZero.position,
  `${backAtStart.position} vs ${atFrameZero.position}`);

await app.evaluate(`document.querySelector('button[aria-label="Next turn"]')?.click(); true`);
await wait(800);
const afterStep = await marker();
check("stepping forward from the first turn moves it again",
  afterStep.position !== backAtStart.position, `${backAtStart.position} → ${afterStep.position}`);
await app.shot("05-stepped-after-restart");

child.kill();
await wait(1200);

console.log("");
if (failures.length) {
  console.error(`FAILED ${failures.length}: ${failures.join(", ")}`);
  process.exitCode = 1;
} else {
  console.log("PASS  restart durability: the recording outlives the process that made it");
}