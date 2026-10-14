/**
 * Replay integrity: does the drawn board match the recorded observations, and does it survive a restart?
 *
 * # Why a digest and not a row count
 *
 * "38 rows before, 38 after" is evidence that a count did not change. It is not evidence that the
 * *content* did not change, and a count that happens to be right for the wrong reason is the failure mode
 * worth designing against. So this compares two things that cannot agree by accident:
 *
 * * a **content digest** over every field a frame is drawn from — run, sequence, turn, action, outcome,
 *   reward, the position before and after, and every tile each observation carried — computed the same
 *   way in both sessions; and
 * * the **rendered tile map**, read straight out of the DOM, compared against the set of tiles the
 *   backend says were observed.
 *
 # What it asserts
 *
 * 1. The board shows terrain only on tiles an observation carried. Every other tile is drawn as
 *    `unseen`. A tile that appears without a record behind it is a fabricated map.
 * 2. The terrain the board shows is the terrain the recording holds, tile for tile — not a superset, not
 *    a subset.
 * 3. The marker sits where the recorded position says.
 * 4. The recording's content digest is identical across a process restart, and so is the engine's own
 *    `replay_digest` from the stored result.
 * 5. Scrubbing backwards never reveals more than the recording holds: at every frame, the rendered tiles
 *    are a subset of what had been observed *by that frame*.
 *
 * Usage: node scripts/replay-integrity.mjs <exe> <port> [outDir]
 */
import { spawn } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const exe = process.argv[2] ?? "D:\\Projects\\ProbeLab\\target\\release\\probelab-desktop.exe";
const port = process.argv[3] ?? "9336";
const outDir = process.argv[4] ?? join(process.env.TEMP ?? ".", "probelab-replay-integrity");
mkdirSync(outDir, { recursive: true });

const failures = [];
const check = (name, ok, detail = "") => {
  console.log(`${ok ? "  PASS" : "  FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
  if (!ok) failures.push(name);
};
const note = (text) => console.log(`        ${text}`);
const wait = (ms) => new Promise((r) => setTimeout(r, ms));

function launch() {
  return spawn(exe, [], {
    env: { ...process.env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}` },
    stdio: "ignore",
  });
}

async function connect() {
  for (let attempt = 0; attempt < 50; attempt += 1) {
    try {
      const targets = await fetch(`http://127.0.0.1:${port}/json/list`).then((r) => r.json());
      const page = targets.find((t) => t.type === "page" && t.webSocketDebuggerUrl);
      if (page) return page;
    } catch {
      /* not up yet */
    }
    await wait(400);
  }
  throw new Error("the application never exposed a debuggable page");
}

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
    writeFileSync(join(outDir, `${name}.png`), Buffer.from(result.data, "base64"));
  };
  return { socket, send, evaluate, shot };
}

/**
 * Reads, in one round trip: the run's events, a content digest over them, the tiles they observed, and
 * what the board is actually rendering.
 *
 * It runs inside the page so it can read both the backend command and the DOM without a round trip in
 * between that a re-render could invalidate. The digest is computed here rather than in Node so the two
 * sessions are compared by exactly the same code.
 */
const READ_INTEGRITY = `(async (runId) => {
  const api = window.__TAURI_INTERNALS__;
  const events = await api.invoke("get_run_telemetry", { run_id: runId });

  /* FNV-1a over a canonical rendering of every field a frame is drawn from. */
  const digestOf = (parts) => {
    let hash = 0xcbf29ce484222325n;
    const prime = 0x100000001b3n;
    for (const part of parts) {
      for (const byte of new TextEncoder().encode(part)) {
        hash ^= BigInt(byte);
        hash = (hash * prime) & 0xffffffffffffffffn;
      }
      hash ^= 0x1fn;
      hash = (hash * prime) & 0xffffffffffffffffn;
    }
    return hash.toString(16).padStart(16, "0");
  };
  const sense = (observation) => {
    const tiles = [[observation.position.x + "," + observation.position.y, observation.current_tile]];
    for (const n of observation.visible_neighbors) {
      tiles.push([n.coordinate.x + "," + n.coordinate.y, n.tile]);
    }
    return tiles.sort((a, b) => (a[0] < b[0] ? -1 : 1));
  };

  const parts = [];
  const observed = {};
  /* One cumulative map per frame: what had been observed by that frame, and nothing later. The
     renderer is checked against this rather than against the whole recording, which is what makes
     "scrubbing back never reveals more" a real assertion. */
  const prefixes = [];
  for (const event of events) {
    for (const [key, tile] of sense(event.observation)) {
      observed[key] = tile;
    }
    prefixes.push({ ...observed });
    parts.push([
      event.run_id, event.sequence, event.turn, event.selected_action ?? "-",
      event.action_outcome ? event.action_outcome.kind : "-",
      event.action_outcome ? event.action_outcome.position.x + "," + event.action_outcome.position.y : "-",
      event.reward, event.invalid_action, event.timeout,
      event.observation.position.x + "," + event.observation.position.y,
      event.observation.current_tile,
      event.observation.distance_to_goal,
      sense(event.observation).map((p) => p.join(":")).join("|"),
    ].join(""));
  }
  /* The frame the final move leaves behind carries the last observation, or none at all when the
     episode ended on that move. Either way there is a frame, so there is a prefix. */
  const after = events.length ? events[events.length - 1].action_outcome : null;
  if (after && after.observation) {
    for (const [key, tile] of sense(after.observation)) {
      observed[key] = tile;
    }
  }
  prefixes.push({ ...observed });

  let storedDigest = null;
  try {
    const result = await api.invoke("get_result", { run_id: runId });
    storedDigest = result.replay_digest;
  } catch (error) {
    storedDigest = "absent: " + String(error);
  }

  const board = document.querySelector(".gw-board");
  /* Park on the last frame before reading the DOM, so two sessions are compared at the same position.
     A live run leaves the stage on its newest frame and a freshly loaded recording opens on its first,
     so comparing whatever happened to be on screen would compare a frame against a frame. */
  const scrub = document.querySelector(".gw-scrub");
  if (scrub) {
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value").set;
    setter.call(scrub, scrub.max);
    scrub.dispatchEvent(new Event("input", { bubbles: true }));
    scrub.dispatchEvent(new Event("change", { bubbles: true }));
    await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
  }
  const cells = Array.from(document.querySelectorAll(".gw-cell"));
  const rendered = {};
  cells.forEach((cell, index) => {
    rendered[index] = cell.getAttribute("data-tile");
  });
  const style = board ? getComputedStyle(board) : null;

  return JSON.stringify({
    events: events.length,
    sequences: events.map((e) => e.sequence),
    digest: digestOf(parts),
    storedDigest,
    observed,
    prefixes,
    rendered,
    marker: style
      ? style.getPropertyValue("--gw-x").trim() + "," + style.getPropertyValue("--gw-y").trim()
      : null,
    lastPosition: events.length
      ? (() => {
          const last = events[events.length - 1];
          const p = (last.action_outcome && last.action_outcome.position) || last.observation.position;
          return p.x + "," + p.y;
        })()
      : null,
    boardLabel: board ? board.getAttribute("aria-label") : null,
  });
})`;

/** Steps the recording to every frame and reports the tiles rendered at each. */
const READ_EVERY_FRAME = `(async () => {
  try {
    const scrub = document.querySelector(".gw-scrub");
    if (!scrub) return JSON.stringify({ error: "no scrub control" });
    /* React tracks the input's value on the element, so assigning to it directly does not always
       register. Going through the native setter is what makes React see a change. */
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value").set;
    const frames = [];
    for (let at = 0; at <= Number(scrub.max); at += 1) {
      setter.call(scrub, String(at));
      scrub.dispatchEvent(new Event("input", { bubbles: true }));
      scrub.dispatchEvent(new Event("change", { bubbles: true }));
      /* Wait for the frame to actually be on screen rather than for a fixed number of frames to pass.
         The transport prints "n / total", so the rendered position can be compared with the one asked
         for — without this the walk reads the previous frame and every comparison is off by one. */
      const wanted = String(at + 1);
      for (let waited = 0; waited < 1000; waited += 20) {
        await new Promise((r) => setTimeout(r, 20));
        const shown = document.querySelector(".gw-position")?.textContent?.trim() ?? "";
        if (shown.startsWith(wanted)) break;
      }
      const seen = {};
      Array.from(document.querySelectorAll(".gw-cell")).forEach((cell, index) => {
        const tile = cell.getAttribute("data-tile");
        if (tile !== "unseen") seen[index] = tile;
      });
      frames.push({ at, seen, shown: document.querySelector(".gw-position")?.textContent?.trim() ?? null });
    }
    return JSON.stringify({ frames });
  } catch (error) {
    return JSON.stringify({ error: String(error) });
  }
})()`;

let child = launch();
let page = await connect();
let app = attach(page);
await wait(2500);
await app.evaluate(`location.hash = "#/lab"; true`);
await wait(2500);

/* ------------------------------------------------------------- a run to check */

console.log("=== 1. Record an episode and read it back ===");
/*
 * Polls for the control rather than assuming it is there on the first frame. The panel mounts after the
 * command catalog resolves, and a script that reads the DOM once at a fixed moment measures its own
 * timing rather than the product's.
 */
const start = JSON.parse(await app.evaluate(`(async () => {
  const labels = () => [...document.querySelectorAll(".lab-console button")].map((b) => b.textContent.trim() + (b.disabled ? " [disabled]" : ""));
  for (let waited = 0; waited < 12000; waited += 100) {
    const button = [...document.querySelectorAll(".lab-console button")]
      .find((b) => b.textContent.trim().startsWith("RUN WITH MOCK") && !b.disabled);
    if (button) {
      button.click();
      for (let more = 0; more < 8000; more += 50) {
        // Every wait yields. A polling loop that does not await blocks the page's own thread, so the
        // very thing it is waiting for can never be painted — a test that deadlocks itself and reports
        // it as a product failure.
        await new Promise((r) => setTimeout(r, 50));
        const id = document.querySelector(".run-id-line code")?.textContent?.trim();
        if (id && id !== "AWAITING REQUEST") return JSON.stringify({ id });
      }
      return JSON.stringify({ id: null, saw: labels() });
    }
    await new Promise((r) => setTimeout(r, 100));
  }
  return JSON.stringify({ id: null, saw: labels(), route: location.hash });
})()`));
const runId = start.id;
check("a run reported its identifier", Boolean(runId), runId ?? `not found; buttons: ${JSON.stringify(start)}`);
await wait(10_000);

const first = JSON.parse(await app.evaluate(`${READ_INTEGRITY}(${JSON.stringify(runId)})`));
note(`events ${first.events}, digest ${first.digest}, engine digest ${first.storedDigest}`);
check("the recording is non-empty", first.events > 0, `${first.events} events`);
check("the sequence is gap-free from one",
  JSON.stringify(first.sequences) === JSON.stringify(Array.from({ length: first.events }, (_, i) => i + 1)),
  JSON.stringify(first.sequences));
check("the board is rendered", Object.keys(first.rendered).length === 25, `${Object.keys(first.rendered).length} cells`);

/* ---------------------------------------------- the drawn board vs the record */

const WIDTH = 5;
const CELLS = 25;
/** DOM index to board coordinate. The cells are written row-major, which is the board's own order. */
const coordinateOf = (index) => `${index % WIDTH},${Math.floor(index / WIDTH)}`;

const observedKeys = Object.keys(first.observed);
const renderedCoordinates = Object.keys(first.rendered)
  .filter((index) => first.rendered[index] !== "unseen")
  .map((index) => coordinateOf(Number(index)));

const drawnButUnobserved = renderedCoordinates.filter((c) => !(c in first.observed));
check("no tile is drawn that no observation carried",
  drawnButUnobserved.length === 0, drawnButUnobserved.join(", "));

const wrongTile = renderedCoordinates.filter((c) => first.rendered[coordinateIndex(c)] !== first.observed[c]);
check("every drawn tile is the terrain the recording holds",
  wrongTile.length === 0, wrongTile.join(", "));

const observedButUndrawn = observedKeys.filter((c) => !renderedCoordinates.includes(c));
check("every observed tile is drawn", observedButUndrawn.length === 0, observedButUndrawn.join(", "));

check("the marker sits where the recorded position says",
  first.marker === first.lastPosition, `marker ${first.marker}, recorded ${first.lastPosition}`);

const unseenCount = Object.values(first.rendered).filter((t) => t === "unseen").length;
check("the unseen count in the label matches the unseen count on the board",
  (first.boardLabel ?? "").includes(`${CELLS - unseenCount} of ${CELLS} tiles observed`),
  `${unseenCount} unseen; label: ${first.boardLabel}`);

await app.shot("01-first-session");

/* ------------------------------------------------- scrubbing reveals no more */

/*
 * Load the recording from the durable store first.
 *
 * The scrub control exists only for a complete recording — a live run is its own transport — so
 * stepping frames means loading the run back, which is the path being checked anyway.
 */
const loaded = JSON.parse(await app.evaluate(`(async () => {
  const button = [...document.querySelectorAll(".gw-heading-actions button")]
    .find((b) => b.textContent.trim().startsWith("REPLAY RECORDING") && !b.disabled);
  if (!button) return JSON.stringify({ loaded: false, saw: [...document.querySelectorAll(".gw-heading-actions button")].map((b) => b.textContent.trim()) });
  button.click();
  for (let waited = 0; waited < 6000; waited += 50) {
    await new Promise((r) => setTimeout(r, 50));
    if (document.querySelector(".gw-scrub")) return JSON.stringify({ loaded: true });
  }
  return JSON.stringify({ loaded: false, reason: "no transport after the load" });
})()`));
check("the recording loads back and offers a transport", loaded.loaded === true, JSON.stringify(loaded));

// One entry per scrub position, so every frame is checked rather than the last one only.
const rawEveryFrame = await app.evaluate(READ_EVERY_FRAME);
if (typeof rawEveryFrame !== "string") {
  console.error("        the frame walk returned a non-string:", JSON.stringify(rawEveryFrame));
  check("the frame walk returned a report", false, typeof rawEveryFrame);
} else {
  check("the frame walk returned a report", true, `${rawEveryFrame.length} bytes`);
}
const everyFrame = typeof rawEveryFrame === "string" ? JSON.parse(rawEveryFrame) : { frames: [], error: "not a string" };
check("the recording can be stepped through",
  Array.isArray(everyFrame.frames) && everyFrame.frames.length === first.prefixes.length,
  everyFrame.error ?? `${everyFrame.frames?.length} frames for ${first.prefixes.length} observations`);

const frames = everyFrame.frames ?? [];
let framesWithExtra = [];
let framesMissing = [];
let framesWrong = [];
for (const frame of frames) {
  const allowed = first.prefixes[frame.at] ?? {};
  // `frame.seen` is keyed by DOM index and `allowed` by coordinate; comparing them directly would
  // report every tile as missing.
  const shown = new Map(
    Object.entries(frame.seen).map(([index, tile]) => [coordinateOf(Number(index)), tile]),
  );
  for (const [coordinate, tile] of shown) {
    if (!(coordinate in allowed)) {
      framesWithExtra.push(`${frame.at}:${coordinate}`);
    } else if (allowed[coordinate] !== tile) {
      framesWrong.push(`${frame.at}:${coordinate}`);
    }
  }
  for (const coordinate of Object.keys(allowed)) {
    if (!shown.has(coordinate)) {
      framesMissing.push(`${frame.at}:${coordinate}`);
    }
  }
}
check("no frame paints a tile the recording had not observed by then",
  framesWithExtra.length === 0, framesWithExtra.slice(0, 6).join(", "));
check("no frame paints the wrong terrain",
  framesWrong.length === 0, framesWrong.slice(0, 6).join(", "));
check("no frame omits a tile the recording had observed by then",
  framesMissing.length === 0, framesMissing.slice(0, 6).join(", "));
check("every frame walked was the one that was asked for",
  frames.every((f) => (f.shown ?? "").startsWith(String(f.at + 1))),
  frames.filter((f) => !(f.shown ?? "").startsWith(String(f.at + 1))).map((f) => `${f.at}→${f.shown}`).join(", "));
// Knowledge only ever grows, so the tile count per frame is monotone. A frame that hid a tile would be
// indistinguishable, on screen, from a recording that had lost one.
let regressions = [];
for (const frame of frames.slice(1)) {
  const before = Object.keys(frames[frame.at - 1].seen).length;
  if (Object.keys(frame.seen).length < before) {
    regressions.push(`${frame.at}: ${Object.keys(frames[frame.at - 1].seen).length} → ${Object.keys(frame.seen).length}`);
  }
}
check("the tile count never falls as the recording advances",
  regressions.length === 0, regressions.slice(0, 4).join(", "));

/* ------------------------------------------------------------- the restart */

console.log("=== 2. The same run, after a restart ===");
app.socket.close();
child.kill();
await wait(2500);

child = launch();
page = await connect();
app = attach(page);
await wait(2500);
await app.evaluate(`location.hash = "#/lab"; true`);
await wait(2500);

const reopened = await app.evaluate(`(() => {
  const rows = Array.from(document.querySelectorAll(".history-table tbody tr"));
  const row = rows.find((r) => r.querySelector(".history-open")?.getAttribute("title") === ${JSON.stringify(runId)});
  if (!row) return "not in the history";
  row.querySelector(".history-open").click();
  return "clicked";
})()`);
check("the run is still listed after the restart", reopened === "clicked", reopened);
await wait(2500);

const second = JSON.parse(await app.evaluate(`${READ_INTEGRITY}(${JSON.stringify(runId)})`));
note(`events ${second.events}, digest ${second.digest}, engine digest ${second.storedDigest}`);

check("the recording holds the same number of events", second.events === first.events,
  `${first.events} before, ${second.events} after`);
check("the recording's content digest is identical across the restart",
  second.digest === first.digest, `${first.digest} vs ${second.digest}`);
check("the engine's own replay digest is identical across the restart",
  second.storedDigest === first.storedDigest, `${first.storedDigest} vs ${second.storedDigest}`);
check("the rendered tile map is identical across the restart",
  JSON.stringify(second.rendered) === JSON.stringify(first.rendered));
check("the marker lands in the same place after the restart",
  second.marker === first.marker, `${first.marker} vs ${second.marker}`);

await app.shot("02-after-restart");

child.kill();
await wait(1_000);

console.log("");
if (failures.length) {
  console.error(`FAILED ${failures.length}: ${failures.join(", ")}`);
  process.exitCode = 1;
} else {
  console.log("PASS  replay integrity: the board matches the record, and the record survives a restart");
}

/** "x,y" back to its row-major DOM index. */
function coordinateIndex(coordinate) {
  const [x, y] = coordinate.split(",").map(Number);
  return y * WIDTH + x;
}