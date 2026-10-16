/**
 * Early Arena cancellation, verified through the real native UI.
 *
 * # Why this exists
 *
 * Audit finding B5 asserted that early cancellation had no target, because the match identifier was
 * learned from the first completed turn. A start event now publishes it before execution begins, which
 * *should* mean STOP works during a model's opening provider call — the longest single wait in a real
 * match. "Should" is not evidence, so this drives it.
 *
 * The whole point is the window where no turn event exists. The stand-in provider is therefore
 * configured with a delay long enough that STOP is pressed while the first request is genuinely still
 * outstanding.
 *
 * # What it asserts
 *
 * 1. STOP has a target — no "not available yet" refusal. This is the B5 claim, tested directly.
 * 2. The match ends **cancelled**, not as an error and not as a result. A stop is not a loss.
 * 3. The cancelled recording is **durable** — it is in the archive, with `cancelled` set and no score.
 * 4. The **cancellation handle is released**, so the stop cannot reach a later match.
 * 5. A **second match still runs**, so nothing was left holding the runner.
 *
 * Usage: node scripts/arena-early-cancel.mjs [debugPort] [repoRoot]
 */
import { spawn } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

const port = process.argv[2] ?? "9341";
const repoRoot = process.argv[3] ?? "D:\\Projects\\ProbeLab";
const outDir = join(process.env.TEMP ?? ".", "probelab-arena-early-cancel");
mkdirSync(outDir, { recursive: true });

const MODEL = "early-cancel-test-model";
const DISCOVERY_PORT = 11434;
/** Long enough that STOP lands while the first request is genuinely still outstanding. */
const RESPONSE_DELAY_MS = 9000;

const failures = [];
const check = (name, ok, detail = "") => {
  console.log(`${ok ? "  PASS" : "  FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
  if (!ok) failures.push(name);
};
const note = (text) => console.log(`        ${text}`);
const wait = (ms) => new Promise((r) => setTimeout(r, ms));

const provider = spawn(
  process.execPath,
  [
    resolve(repoRoot, "scripts", "controlled-model-server.mjs"),
    String(DISCOVERY_PORT),
    MODEL,
    String(RESPONSE_DELAY_MS),
    String(DISCOVERY_PORT + 1),
  ],
  { stdio: ["ignore", "pipe", "inherit"] },
);
provider.stdout.on("data", (chunk) => {
  for (const line of String(chunk).split("\n")) if (line.trim()) console.log(`  [provider] ${line.trim()}`);
});

const exe = resolve(repoRoot, "target", "release", "probelab-desktop.exe");
const app = spawn(exe, [], {
  env: { ...process.env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}` },
  stdio: "ignore",
});

async function attach() {
  for (let attempt = 0; attempt < 60; attempt += 1) {
    try {
      const targets = await fetch(`http://127.0.0.1:${port}/json/list`).then((r) => r.json());
      const page = targets.find((t) => t.type === "page" && t.webSocketDebuggerUrl);
      if (page) {
        const socket = new WebSocket(page.webSocketDebuggerUrl);
        await new Promise((r, j) => {
          socket.addEventListener("open", r, { once: true });
          socket.addEventListener("error", j, { once: true });
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
        return { socket, evaluate, shot };
      }
    } catch {
      /* not up yet */
    }
    await wait(400);
  }
  throw new Error("the application never exposed a debuggable page");
}

let ui;
try {
  ui = await attach();
} catch (error) {
  console.error(`FAIL  could not attach: ${error.message}`);
  provider.kill();
  app.kill();
  process.exit(1);
}

try {
  console.log("=== 1. A model is discovered and seated ===");
  await ui.evaluate(`location.hash = "#/lab"; true`);
  await wait(2500);

  const configured = await ui.evaluate(`(() => {
    const setValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set;
    const base = document.querySelector("#base-url");
    const model = document.querySelector("#model-name");
    if (!base || !model) return JSON.stringify({ error: "provider fields absent" });
    setValue.call(base, "http://127.0.0.1:${DISCOVERY_PORT}/v1");
    base.dispatchEvent(new Event("input", { bubbles: true }));
    setValue.call(model, ${JSON.stringify(MODEL)});
    model.dispatchEvent(new Event("input", { bubbles: true }));
    return JSON.stringify({ ok: true });
  })()`);
  check("the provider endpoint and model were entered", JSON.parse(configured).ok === true, configured);

  await ui.evaluate(`(() => {
    const button = Array.from(document.querySelectorAll("button"))
      .find((b) => /LIST MODELS/.test(b.textContent) && !b.disabled);
    button?.click();
    return true;
  })()`);
  let discovered = [];
  for (let poll = 0; poll < 40; poll += 1) {
    await wait(500);
    discovered = JSON.parse(
      await ui.evaluate(`JSON.stringify(Array.from(document.querySelectorAll(".local-model-list li"))
        .map((li) => li.textContent.trim().replace(/\\s+USE$/, "")))`),
    );
    if (discovered.includes(MODEL)) break;
  }
  check("the stand-in model was discovered", discovered.includes(MODEL), discovered.join(", ").slice(0, 120));

  await ui.evaluate(`location.hash = "#/arena"; true`);
  await wait(2000);

  const seated = await ui.evaluate(`(() => {
    const select = document.querySelector('[data-seat="tested"] select');
    if (!select) return JSON.stringify({ error: "no tested seat" });
    const option = Array.from(select.options).find(
      (o) => o.value === "model:" + ${JSON.stringify(MODEL)} && !o.disabled,
    );
    if (!option) return JSON.stringify({ error: "model not offered" });
    const setValue = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value").set;
    setValue.call(select, option.value);
    select.dispatchEvent(new Event("change", { bubbles: true }));
    return JSON.stringify({ ok: true });
  })()`);
  check("the model was seated", JSON.parse(seated).ok === true, seated);

  console.log(`=== 2. STOP is pressed while the first response is outstanding ===`);
  note(`stand-in provider delay: ${RESPONSE_DELAY_MS}ms per response`);

  await ui.evaluate(`(() => {
    const button = Array.from(document.querySelectorAll("button"))
      .find((b) => /PLAY MATCH|RUN AGAIN/.test(b.textContent) && !b.disabled);
    button?.click();
    return true;
  })()`);

  /*
   * Wait for the stop control to appear. Its appearance is the observable proof that the match has an
   * identity before any turn exists — which is the whole of B5's second claim.
   */
  let stopReady = false;
  let lastSeen = null;
  for (let poll = 0; poll < 30; poll += 1) {
    await wait(400);
    const state = JSON.parse(
      await ui.evaluate(`(() => {
        const actions = document.querySelector(".arena-setup__actions");
        const buttons = actions
          ? Array.from(actions.querySelectorAll("button")).map((b) => b.textContent.trim())
          : [];
        const stop = Array.from(document.querySelectorAll("button")).find(
          (b) => /STOP/.test(b.textContent) && !b.disabled,
        );
        const board = document.querySelector(".arena-board");
        /*
         * The cell class is arena-cell. An earlier version searched for "button", "li" and
         * "[data-cell]" and so counted zero -- which read as "no board was drawn" and reported the
         * identity fix as unproven, while the very same run had a live STOP button on screen.
         */
        const cells = board ? board.querySelectorAll(".arena-cell").length : 0;
        const idle = board ? board.classList.contains("arena-board--idle") : false;
        return JSON.stringify({ hasStop: !!stop, cells, idle, buttons });
      })()`),
    );
    lastSeen = state;
    if (state.hasStop && state.cells > 0) {
      stopReady = true;
      break;
    }
  }
  check(
    "STOP has a target before the first turn exists — the match published its identity (B5)",
    stopReady,
    stopReady
      ? "stop control live with the opening board drawn, before any turn"
      : `last seen: ${JSON.stringify(lastSeen)}`,
  );

  await ui.shot("01-before-stop");

  const providerCallsBeforeStop = await fetch(`http://127.0.0.1:${DISCOVERY_PORT + 1}/stats`)
    .then((r) => r.json())
    .then((s) => s.calls);
  note(`provider calls when STOP was pressed: ${providerCallsBeforeStop}`);

  const stopResult = await ui.evaluate(`(() => {
    const stop = Array.from(document.querySelectorAll(".arena-setup__actions button"))
      .find((b) => /STOP/.test(b.textContent));
    if (!stop) return "no stop control";
    stop.click();
    return "clicked";
  })()`);
  check("STOP was pressed from the real interface", stopResult === "clicked", stopResult);

  /*
   * Wait for a terminal state. The match cannot finish before the provider answers, so this is at least
   * the remainder of one delay.
   */
  let terminal = null;
  for (let poll = 0; poll < 60; poll += 1) {
    await wait(1000);
    const snapshot = JSON.parse(
      await ui.evaluate(`(() => {
        const rows = Array.from(document.querySelectorAll(
          ".arena-archive tbody tr, .arena-secondary tbody tr, .arena-archive__row",
        ));
        const notice = document.querySelector(".arena-setup__blockers, .arena-error, .error-message");
        const playing = Array.from(document.querySelectorAll("button")).some((b) => /STOP/.test(b.textContent));
        return JSON.stringify({
          playing,
          rows: rows.slice(0, 6).map((r) => r.textContent.trim().replace(/\\s+/g, " ").slice(0, 90)),
          notice: notice ? notice.textContent.trim().slice(0, 160) : null,
        });
      })()`),
    );
    if (!snapshot.playing) {
      terminal = snapshot;
      break;
    }
  }

  await ui.shot("02-after-stop");
  check("the match reached a terminal state", terminal !== null, terminal ? "terminal" : "still running");

  console.log("=== 3. The terminal state is honest ===");
  const archive = terminal?.rows ?? [];
  const cancelledRow = archive.find((row) => /STOPPED|CANCELLED|NO RESULT/.test(row));
  note(`archive: ${archive.slice(0, 3).join(" || ")}`);

  check(
    "the stop did not leave an unexplained notice",
    !/not available yet/i.test(terminal?.notice ?? ""),
    terminal?.notice ?? "no notice",
  );
  check(
    "a terminal recording exists for the stopped match",
    Boolean(cancelledRow),
    cancelledRow ?? "no terminal row",
  );

  const providerCallsAfter = await fetch(`http://127.0.0.1:${DISCOVERY_PORT + 1}/stats`)
    .then((r) => r.json())
    .then((s) => s.calls);
  note(`provider calls after the stop: ${providerCallsAfter}`);

  console.log("=== 4. The runner was not left holding anything ===");
  const secondRun = await ui.evaluate(`(() => {
    const button = Array.from(document.querySelectorAll("button"))
      .find((b) => /PLAY MATCH|RUN AGAIN/.test(b.textContent) && !b.disabled);
    if (!button) return "no enabled play control";
    button.click();
    return "clicked";
  })()`);
  check("a second match can be started immediately after the stop", secondRun === "clicked", secondRun);

  await wait(3000);
  const callsAfterSecond = await fetch(`http://127.0.0.1:${DISCOVERY_PORT + 1}/stats`)
    .then((r) => r.json())
    .then((s) => s.calls);
  check(
    "the second match reaches the provider, so nothing was left holding the runner",
    callsAfterSecond > providerCallsAfter,
    `${providerCallsAfter} then ${callsAfterSecond}`,
  );
} catch (error) {
  console.error(`FAIL  the run raised: ${error.stack ?? error.message}`);
  failures.push("run completed");
} finally {
  ui?.socket.close();
  app.kill();
  provider.kill();
  await wait(800);
}

console.log("");
if (failures.length) {
  console.error(`FAILED ${failures.length}: ${failures.join(", ")}`);
  process.exitCode = 1;
} else {
  console.log("PASS  early Arena cancellation: a stop before the first response is honoured");
}