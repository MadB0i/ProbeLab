/**
 * Capture the specific Arena states the README needs, from the real window.
 *
 * The generic route capture lands on whatever frame playback happens to be in, which produced a
 * "completed match" screenshot that was actually paused at frame 4 of 8 -- a picture labelled as a
 * finished result that showed an unfinished board. This drives the transport to a known position
 * first, so the completed-match shot really is the last frame and the playback shot really is
 * mid-match, and then it proves it by reading the frame counter back out of the DOM.
 *
 * Usage: node scripts/capture-arena-states.mjs <port> <outDir>
 */
const port = process.argv[2] ?? "9333";
const outDir = process.argv[3] ?? process.env.TEMP + "\\opencode\\arena-states";

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

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function shot(name) {
  const { data } = await send("Page.captureScreenshot", { format: "png", captureBeyondViewport: false });
  writeFileSync(join(outDir, `${name}.png`), Buffer.from(data, "base64"));
}

/** Read the transport's own state back out of the DOM, so a caption can be checked against it. */
const readFrame = () =>
  evalJS(`(() => {
    const label = document.querySelector(".arena-playback-section");
    const text = label ? label.textContent : "";
    const m = text.match(/(\\d+)\\s*\\/\\s*(\\d+)/);
    const filled = Array.from(document.querySelectorAll(".arena-cell"))
      .filter((c) => c.className.includes("arena-cell--first") || c.className.includes("arena-cell--second"))
      .length;
    return {
      frame: m ? Number(m[1]) : null,
      ofFrames: m ? Number(m[2]) : null,
      filledCells: filled,
      outcome: (document.querySelector(".arena-outcome-gloss")?.textContent ?? "").trim().slice(0, 80),
    };
  })()`);

/*
 * The transport's controls are icon-only and carry their name in `aria-label`, not in text content.
 * Matching on text found the Play/Next buttons by luck and found nothing for Previous, which is how
 * the mid-match capture silently stayed on the final frame. Selecting by accessible name is also the
 * interaction a screen-reader user performs, so it is the one worth scripting.
 */
const clickNamed = (name) =>
  evalJS(`(() => {
    const el = document.querySelector(${JSON.stringify(`button[aria-label="${name}"]`)});
    if (!el) return false;
    if (el.disabled) return false;
    el.click();
    return true;
  })()`);

await send("Page.enable");
await send("Runtime.enable");
await send("Browser.getWindowForTarget").then(async ({ windowId }) => {
  await send("Browser.setWindowBounds", { windowId, bounds: { width: 1600, height: 900, windowState: "normal" } });
});
await sleep(900);

await evalJS(`location.hash = "#/arena"; true`);
await sleep(2500);

// The completed match: walk the transport to the last frame and let it settle.
//
// Each of these buttons steps one frame, so "go to the end" is a loop, not one click. Asserting the
// resulting frame afterwards is what makes the screenshot trustworthy -- an earlier version pressed
// END once, assumed it meant "final position", and labelled a frame-4 board as a completed match.
console.log("walking the transport to the final frame");
for (let i = 0; i < 24; i += 1) {
  await clickNamed("Next frame");
  await sleep(120);
  const at = await readFrame();
  if (at.frame && at.ofFrames && at.frame >= at.ofFrames) break;
}
await sleep(1000);
const completed = await readFrame();
console.log("completed-match state:", JSON.stringify(completed));
await shot("arena-completed");

// The playback view: step back into the middle of the match so the board is genuinely unfinished.
console.log("stepping back to a mid-match frame");
const targetFrame = completed.ofFrames ? Math.max(1, Math.floor(completed.ofFrames / 2)) : 3;
for (let i = 0; i < 24; i += 1) {
  const at = await readFrame();
  if (at.frame && at.frame <= targetFrame) break;
  await clickNamed("Previous frame");
  await sleep(120);
}
await sleep(1000);
const mid = await readFrame();
console.log("playback state:", JSON.stringify(mid));
await shot("arena-playback");

const problems = [];
if (completed.ofFrames && completed.frame !== completed.ofFrames) {
  problems.push(`completed shot is at frame ${completed.frame}/${completed.ofFrames}, not the end`);
}
if (mid.frame && completed.ofFrames && mid.frame >= completed.ofFrames) {
  problems.push(`playback shot is at frame ${mid.frame}, not mid-match`);
}
console.log(problems.length ? `PROBLEMS: ${problems.join("; ")}` : "both states verified");
socket.close();
