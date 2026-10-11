/**
 * Records the README demo from the real ProbeLab executable.
 *
 * Everything here is a real interaction against the built application over the DevTools protocol:
 * a benchmark is really executed, the match is really played by the engine, and playback really reads
 * the recording the engine produced. Nothing is staged, stubbed, or typed into a mock.
 *
 * Frames are captured on a fixed cadence by *waiting* rather than by capturing as fast as the
 * protocol allows. An uncapped loop produces a demo whose speed drifts with machine load, and the
 * scrubber looks like it stutters for no reason.
 *
 * Usage: node scripts/record-demo.mjs <port> <outDir> [fps]
 */
const port = process.argv[2] ?? "9333";
const outDir = process.argv[3] ?? process.env.TEMP + "\\opencode\\demo";
const fps = Number(process.argv[4] ?? 10);

import { writeFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";

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
socket.addEventListener("message", (e) => {
  const m = JSON.parse(e.data);
  if (m.id && pending.has(m.id)) {
    const { res, rej } = pending.get(m.id);
    pending.delete(m.id);
    m.error ? rej(new Error(m.error.message)) : res(m.result);
  }
});

function send(method, params = {}) {
  const i = ++id;
  socket.send(JSON.stringify({ id: i, method, params }));
  return new Promise((res, rej) => pending.set(i, { res, rej }));
}

async function evalJS(expression) {
  const { result, exceptionDetails } = await send("Runtime.evaluate", {
    expression,
    returnByValue: true,
    awaitPromise: true,
  });
  if (exceptionDetails) throw new Error(exceptionDetails.exception?.description ?? "eval failed");
  return result.value;
}

let frameIndex = 0;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** Capture the laid-out viewport, on the fixed cadence rather than as fast as possible. */
async function grab(label) {
  const { data } = await send("Page.captureScreenshot", {
    format: "png",
    captureBeyondViewport: false,
  });
  writeFileSync(join(outDir, `frame-${String(frameIndex).padStart(4, "0")}-${label}.png`), Buffer.from(data, "base64"));
  frameIndex += 1;
}

/** Hold for a number of frames so a view that is being read is on screen long enough to be read. */
async function hold(frames, label) {
  for (let i = 0; i < frames; i += 1) {
    await grab(label);
    await sleep(1000 / fps);
  }
}

/** Click by visible text, the way a person finds a control. Returns true when it was found. */
async function clickText(text, scope = "button") {
  return evalJS(`(() => {
    const el = Array.from(document.querySelectorAll(${JSON.stringify(scope)}))
      .find((b) => (b.textContent || "").trim().toUpperCase().includes(${JSON.stringify(text.toUpperCase())}));
    if (!el) return false;
    el.scrollIntoView({ block: "center" });
    el.click();
    return true;
  })()`);
}

async function goto(route) {
  await evalJS(`location.hash = "#/${route}"; true`);
  await sleep(900);
}

async function settle(ms = 1200) {
  await sleep(ms);
}

/*
 * A stable window for the recording.
 *
 * A GIF is cropped to whatever size the first frame was, so a window that changes size mid-recording
 * produces a video with a jump in it. Pinning the bounds up front is what keeps the frame constant.
 */
await send("Browser.getWindowForTarget").then(async ({ windowId }) => {
  await send("Browser.setWindowBounds", {
    windowId,
    bounds: { width: 1440, height: 860, windowState: "normal" },
  });
});
await settle(1200);

const log = (step) => console.log(`  [${String(frameIndex).padStart(3, "0")}] ${step}`);

// 1. Readiness -- the gate that explains what the machine can run.
log("readiness");
await goto("readiness");
await hold(14, "readiness");

// 2. Command Center -- run a real benchmark through the real engine.
//
// "RUN WITH MOCK" is the label the application itself uses for the deterministic baseline policy, so
// using it keeps the recording honest: the benchmark definition, the engine and the telemetry are all
// real, and the policy under test is the shipped baseline rather than a model. The panel states which
// was used on the run it records.
log("command center");
await goto("lab");
await hold(12, "lab");
const ran = await clickText("RUN WITH MOCK");
log(`run benchmark clicked: ${ran}`);
await hold(34, "lab-running");
await settle(4000);
await hold(14, "lab-result");

// 3. Arena -- a match the engine really played.
log("arena");
await goto("arena");
await hold(14, "arena");

// 4. Arena playback -- the recorded positions, played back.
log("playback");
const played = await clickText("PLAY");
log(`play clicked: ${played}`);
await hold(26, "arena-playback");

// 5. Comparison -- why two runs may not be comparable.
log("comparison");
await goto("comparison");
await hold(14, "comparison");

// 6. Security lab.
log("security");
await goto("security");
await hold(14, "security");

console.log(`captured ${frameIndex} frames at ${fps} fps -> ${outDir}`);
socket.close();