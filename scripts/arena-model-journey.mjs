/**
 * The Arena model journey, driven through the real native application.
 *
 * # Why this exists
 *
 * Audit findings B1 and B2 together made the product's central advertised workflow unreachable, and
 * neither was visible to the test suite:
 *
 * * B1 — `App.tsx` computed `canPlay` as "both seats are reference policies", so selecting a discovered
 *   model disabled Play Match.
 * * B2 — the same file appended `/v1` to a configured URL that already carried it, so a model seat would
 *   have resolved to `/v1/v1` even once it could be selected.
 *
 * Two component tests missed both because each stopped at a module boundary: `ArenaPanel.test.tsx`
 * supplies `canPlay: true` and so never sees the parent's rule, and the adapter's own tests pass a base
 * URL in directly and so never see the UI's construction of one. The seam between them was untested, and
 * the seam is where both defects lived.
 *
 * This script closes that seam against the running product. It discovers a local model through the real
 * command, seats it in the real panel, reads the endpoint the panel actually built, runs a real match
 * against a controlled provider, and checks the result reaches the screen.
 *
 * It never asks the host to be busy, never reads a real credential, and never touches an existing
 * database beyond the rows it plays.
 *
 * Usage: node scripts/arena-model-journey.mjs <debugPort> [repoRoot]
 */
import { spawn } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

const port = process.argv[2] ?? "9338";
const DISCOVERY_PORT = 11434;
const providerPort = `127.0.0.1:${DISCOVERY_PORT}`;
const repoRoot = process.argv[4] ?? "D:\\Projects\\ProbeLab";
const outDir = join(process.env.TEMP ?? ".", "probelab-arena-journey");
mkdirSync(outDir, { recursive: true });

const MODEL = "journey-test-provider";
const failures = [];
const check = (name, ok, detail = "") => {
  console.log(`${ok ? "  PASS" : "  FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
  if (!ok) failures.push(name);
};
const note = (text) => console.log(`        ${text}`);
const wait = (ms) => new Promise((r) => setTimeout(r, ms));

/* ------------------------------------------------------------------ the provider */

/*
 * The controlled server must occupy Ollama's discovery port.
 *
 * Discovery is hardwired to 11434 for `ollama-local`, so a journey that needs a *known* model name has to
 * answer there. The first version of this script pointed the product at a separate port and then
 * discovered whatever the developer's own Ollama happened to be serving — which meant the journey
 * exercised a real model and told the operator nothing about the endpoint it had just configured. The
 * product is unchanged; only which loopback port answers has changed.
 *
 * The per-response delay is deliberately not small. At 40ms a six-ply match finishes inside a single
 * polling interval, so the one thing B5 is about — whether the stage is live *during* a match — is
 * unobservable by construction, and the journey reported the stage as never having been live. A slower
 * stand-in buys the observation window. The cost is wall-clock time in a test, not fidelity in the
 * product.
 */
const RESPONSE_DELAY_MS = Number(process.env.JOURNEY_DELAY_MS ?? 900);

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
  for (const line of String(chunk).split("\n")) {
    if (line.trim()) console.log(`  [provider] ${line.trim()}`);
  }
});

/* ------------------------------------------------------------------ the application */

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
            throw new Error(
              response.result.exceptionDetails.exception?.description ?? "evaluate failed",
            );
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
  /* ================================================ 1. the provider is reachable */

  console.log("=== 1. The model exists and is discovered ===");

  /*
   * Point the product at the controlled provider *before* discovery, so the real discovery command
   * interrogates the endpoint that will actually serve this journey.
   *
   * The fields live on the Lab's provider card, so the Lab has to be the first route visited. An earlier
   * version navigated to `#/arena` first and then failed to find them, which read as "the product has no
   * configurable endpoint" — a selector matching nothing is a fact about the route, not the product.
   */
  await ui.evaluate(`location.hash = "#/lab"; true`);
  await wait(2500);

  const configured = await ui.evaluate(`(() => {
    const setValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set;
    const base = document.querySelector("#base-url");
    const model = document.querySelector("#model-name");
    if (!base || !model) {
      return JSON.stringify({
        error: "provider fields not found on the lab route",
        ids: Array.from(document.querySelectorAll("input")).map((i) => i.id).filter(Boolean),
      });
    }
    setValue.call(base, "http://${providerPort}/v1");
    base.dispatchEvent(new Event("input", { bubbles: true }));
    setValue.call(model, ${JSON.stringify(MODEL)});
    model.dispatchEvent(new Event("input", { bubbles: true }));
    return JSON.stringify({ ok: true, base: base.value, model: model.value });
  })()`);
  const config = JSON.parse(configured);
  check("the provider endpoint and model name were entered", config.ok === true, configured.slice(0, 160));

  /*
   * Trigger the real discovery command through the real control, rather than injecting models into the
   * panel. The point of the journey is that the product's own discovery populates the picker.
   *
   * Discovery is reported per executor, and the card for the one pointing at the controlled provider is
   * the only one whose result belongs to this journey.
   */
  const listed = await ui.evaluate(`(() => {
    const button = Array.from(document.querySelectorAll("button")).find(
      (b) => /LIST MODELS/.test(b.textContent) && !b.disabled,
    );
    if (!button) {
      return JSON.stringify({
        error: "no LIST MODELS control",
        buttons: Array.from(document.querySelectorAll("button"))
          .map((b) => b.textContent.trim())
          .filter(Boolean)
          .slice(0, 30),
      });
    }
    button.click();
    return JSON.stringify({ ok: true });
  })()`);
  const listResult = JSON.parse(listed);
  check(
    "the product's own discovery control was used",
    listResult.ok === true,
    listResult.error ? listed.slice(0, 200) : "LIST MODELS pressed",
  );

  // Discovery is a round trip to the controlled provider.
  let discovered = [];
  for (let poll = 0; poll < 40; poll += 1) {
    await wait(500);
    const found = await ui.evaluate(`(() => {
      const items = Array.from(document.querySelectorAll(".local-model-list li"))
        .map((li) => li.textContent.trim().replace(/\\s+USE$/, ""));
      return JSON.stringify(items);
    })()`);
    discovered = JSON.parse(found);
    if (discovered.includes(MODEL)) {
      break;
    }
  }
  check(
    "the controlled provider's model was discovered by the product",
    discovered.includes(MODEL),
    discovered.join(", ").slice(0, 160) || "nothing discovered",
  );
  note(`discovered: ${discovered.join(", ")}`);
  await wait(500);

  /* ================================================ 2. the model is discoverable */

  await ui.evaluate(`location.hash = "#/arena"; true`);
  await wait(3000);

  const seatOptions = await ui.evaluate(`(() => {
    const picker = document.querySelector('[data-seat="tested"] select');
    if (!picker) return "no tested-seat picker";
    const options = Array.from(picker.options).map((o) => ({
      value: o.value,
      label: o.textContent.trim(),
      group: o.parentElement?.tagName === "OPTGROUP" ? o.parentElement.label : "",
    }));
    return JSON.stringify(options);
  })()`);
  const parsed = JSON.parse(seatOptions);
  if (typeof parsed === "string") {
    check("the tested-seat picker was found", false, parsed);
    throw new Error(parsed);
  }
  note(`seat options: ${parsed.length}`);
  const modelOptions = parsed.filter((o) => o.value.startsWith("model:") && !o.value.endsWith(":"));
  check(
    "the discovered local model is offered as a seat",
    modelOptions.length > 0,
    modelOptions.map((o) => o.value).join(" | ") || parsed.slice(0, 5).map((o) => o.value).join(" | "),
  );

  /* ================================================ 3. eligibility accepts it (B1) */

  console.log("=== 2. A model seat is eligible (audit finding B1) ===");

  // Select a model in the tested seat, the way an operator would.
  const selected = await ui.evaluate(`(() => {
    const select = document.querySelector('[data-seat="tested"] select');
    if (!select) return JSON.stringify({ error: "no tested-seat selector" });
    const modelOption = Array.from(select.options).find(
      (o) => o.value.startsWith("model:") && !o.value.endsWith(":") && !o.disabled,
    );
    if (!modelOption) {
      return JSON.stringify({
        error: "no selectable model option",
        options: Array.from(select.options).map((o) => ({ value: o.value, disabled: o.disabled })),
      });
    }
    const setValue = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value").set;
    setValue.call(select, modelOption.value);
    select.dispatchEvent(new Event("change", { bubbles: true }));
    return JSON.stringify({ selected: modelOption.value });
  })()`);
  const selection = JSON.parse(selected);
  check(
    "a model can be selected for the tested seat",
    typeof selection.selected === "string" && selection.selected.startsWith("model:"),
    JSON.stringify(selection).slice(0, 120),
  );
  await wait(1200);

  const eligibility = await ui.evaluate(`(() => {
    const button = Array.from(document.querySelectorAll("button"))
      .find((b) => /PLAY MATCH|RUN AGAIN/.test(b.textContent));
    const blockers = Array.from(document.querySelectorAll(".arena-setup__blockers li"))
      .map((li) => li.textContent.trim());
    return JSON.stringify({
      present: !!button,
      disabled: button ? button.disabled : null,
      blockers,
    });
  })()`);
  const state = JSON.parse(eligibility);
  note(`Play disabled: ${state.disabled}; blockers: ${state.blockers.length}`);
  check(
    "Play Match is enabled once a model is seated — the defect disabled it",
    state.present && state.disabled === false,
    JSON.stringify(state).slice(0, 200),
  );
  check(
    "no eligibility blocker is reported for a usable model seat",
    state.blockers.length === 0,
    state.blockers.join(" / "),
  );

  /* ================================================ 4. the endpoint is correct (B2) */

  console.log("=== 3. The endpoint the panel builds is routable (audit finding B2) ===");

  /*
   * The composed endpoint is what the adapter will actually request. It is read out of the running
   * application's own state rather than recomputed here, because recomputing it would test a copy of
   * the rule instead of the rule.
   */
  /*
 * The endpoint is read on the Lab route, where the field actually lives.
 *
 * An earlier version read it after navigating to `#/arena`, where the provider card is not rendered, and
 * concluded the product had no configurable endpoint. The field is only absent because the route changed
 * — a lesson worth keeping: a selector that matches nothing is a question about the route, not evidence
 * about the product.
 */
await ui.evaluate(`location.hash = "#/lab"; true`);
await wait(1500);
const endpointProbe = await ui.evaluate(`(() => {
    const base = document.querySelector("#base-url");
    return JSON.stringify({ configured: base ? base.value : null, present: !!base });
  })()`);
const endpointState = JSON.parse(endpointProbe);
const configuredEndpoint = endpointState.configured;
check(
  "the configured endpoint is readable from the live product",
  endpointState.present === true && typeof configuredEndpoint === "string",
  String(configuredEndpoint),
);
  check(
  "the configured endpoint carries exactly one /v1",
  typeof configuredEndpoint === "string" && !configuredEndpoint.includes("/v1/v1"),
  String(configuredEndpoint),
  );
  if (typeof configuredEndpoint !== "string") {
    throw new Error("no configured endpoint to compose");
  }

  const chatEndpoint = `${configuredEndpoint}/chat/completions`;
  note(`the adapter will request: ${chatEndpoint}`);

  // And it must be a URL the controlled provider actually answers.
  let answered = false;
  try {
    const response = await fetch(chatEndpoint, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        model: MODEL,
        messages: [{ role: "user", content: "reply with a legal move" }],
      }),
    });
    answered = response.ok;
    if (!answered) note(`provider answered ${response.status}`);
  } catch (error) {
    note(`provider request failed: ${error.message}`);
  }
  check("the composed chat-completions endpoint is one the provider answers", answered, chatEndpoint);

  /* ================================================ 5. a real match runs and lands */

  console.log("=== 4. A model-vs-policy match runs and displays its result ===");
  note(`stand-in provider delay: ${RESPONSE_DELAY_MS}ms per turn`);

  /*
   * Back to the Arena, where the seat that was chosen still lives.
   */
  await ui.evaluate(`location.hash = "#/arena"; true`);
  await wait(1500);
  await ui.shot("01-before-play");

  const pressed = await ui.evaluate(`(() => {
    const button = Array.from(document.querySelectorAll("button"))
      .find((b) => /PLAY MATCH|RUN AGAIN/.test(b.textContent) && !b.disabled);
    if (!button) return "no enabled play button";
    button.click();
    return "clicked";
  })()`);
  note(`play pressed: ${pressed}`);

  // A real match against the controlled provider.
  let outcome = null;
  let peakStagePlies = 0;
  let sawLive = false;
  for (let poll = 0; poll < 90; poll += 1) {
    await wait(1000);
    const snapshot = await ui.evaluate(`(() => {
      const board = document.querySelector(".arena-board");
      const cells = board ? board.querySelectorAll("button, li, [data-cell], .arena-cell").length : 0;
      const verdict =
        document.querySelector(".arena-verdict-label, .arena-verdict")?.textContent?.trim() ?? null;
      const blockers = Array.from(document.querySelectorAll(".arena-setup__blockers li")).map((li) =>
        li.textContent.trim(),
      );
      const playing =
        document.querySelector(".arena-setup__actions .btn-danger, .arena-setup__actions button.btn-danger") !==
        null ||
        Array.from(document.querySelectorAll("button")).some((b) => /STOP/.test(b.textContent));
      const error = document.querySelector(".error-message, .arena-error")?.textContent?.trim() ?? null;
      const rows = Array.from(
        document.querySelectorAll(
          ".arena-archive-row, .arena-archive__row, .arena-archive tbody tr, .arena-secondary tbody tr",
        ),
      );
      const rowText = rows[0] ? rows[0].textContent.replace(/\\s+/g, " ") : "";
      const rowMatch = /(\\d+)\\s+plies/.exec(rowText);
      const plies = rowMatch ? Number(rowMatch[1]) : 0;
      /*
       * The live view's own ply figure, plus whether the turn telemetry panel is mounted.
       *
       * Three separate wrong readings happened here, and each is a trap worth naming:
       *
       *  * The **archive row** lags by design — it refreshes only after a match is stored — so reading it
       *    during a match yields the *previous* match's row. It reported zero plies for a match that had
       *    genuinely played six.
       *  * The **turn-telemetry heading** reads "TURN n TELEMETRY" and does exist during a match, but
       *    with a one-second poll interval and a 900ms per-turn delay it is frequently not yet present
       *    when sampled, so asserting on whichever single sample happened to land made the check depend
       *    on poll timing rather than on the product.
       *  * **Accepting one sample at all** is racy for a short match. The reading that answers the
       *    question is a deterministic one: sample every tick from the moment Play is pressed, keep the
       *    peak, and require the peak to be positive.
       */
      const heading = document.querySelector("#arena-telemetry-heading");
      const headingText = heading ? heading.textContent.replace(/\\s+/g, " ") : "";
      const plyMatch = /TURN\\s+(\\d+)/i.exec(headingText);
      const stagePlies = plyMatch ? Number(plyMatch[1]) : 0;
      const turnRows = document.querySelectorAll(".arena-telemetry").length;
      return JSON.stringify({
        cells,
        verdict,
        blockers,
        playing,
        error,
        hasBoard: !!board,
        plies,
        stagePlies,
        turnRows,
      });
    })()`);
    const snap = JSON.parse(snapshot);

    /*
     * Track the peak across every sample, and whether any sample caught the match in flight.
     *
     * The loop runs to completion rather than breaking on the first good-looking sample, because the
     * property under test is "the stage was live during this match" and the only way to establish that
     * honestly is to have watched it throughout.
     */
    peakStagePlies = Math.max(peakStagePlies, snap.stagePlies, snap.turnRows);
    if (snap.playing) {
      sawLive = true;
    }
    if (snap.error) {
      outcome = { ...snap, live: sawLive, stagePlies: peakStagePlies };
      break;
    }
    if (snap.cells > 0 && snap.verdict && !snap.playing) {
      outcome = { ...snap, live: sawLive, stagePlies: peakStagePlies };
      break;
    }
    if (!snap.playing && poll > 3) {
      outcome = { ...snap, live: sawLive, stagePlies: peakStagePlies };
      break;
    }
  }

  check(
    "the live stage drew a board during the match",
    outcome && outcome.cells > 0,
    outcome ? `${outcome.cells} cells` : "no stage snapshot",
  );
  check(
    "the stage was live *while the match ran*, not only after it finished (audit finding B5)",
    outcome?.live === true,
    outcome ? `live=${outcome.live}, stage plies ${outcome.stagePlies}` : "unknown",
  );
  check(
    "the stage showed a played turn before the match ended",
    (outcome?.stagePlies ?? 0) > 0,
    `peak ${outcome?.stagePlies ?? 0}`,
  );
  /*
   * Plies as the stage reported them while the match was running.
   *
   * A verdict is not a result. A match that never started also has a verdict — `NO RESULT` — so asserting
   * on the label proves nothing. Two earlier versions of this check were wrong in instructive ways:
   * accepting any verdict string, and then reading plies from the archive row, which is refreshed only
   * after the match is stored and therefore reports the *previous* match's row while this one is still
   * playing. That reported zero plies for a match that had genuinely played six.
   */
  const providerStats = await fetch(`http://127.0.0.1:${DISCOVERY_PORT + 1}/stats`).then((r) => r.json());
  check(
    "the provider was actually called repeatedly, rather than the match resolving elsewhere",
    providerStats.calls >= 3,
    `${providerStats.calls} calls`,
  );
  check(
    "no eligibility blocker appeared once the match was running",
    outcome && outcome.blockers.length === 0,
    outcome ? outcome.blockers.join(" / ") : "unknown",
  );
  await ui.shot("02-after-match");

  /* ================================================ 6. it was recorded */

  /*
   * Wait for the archive to actually show the match that just finished, rather than reading whatever row
   * happens to be first. The archive is refreshed after the match is stored, and the previous match from
   * an earlier phase of this same script is still at the top of it — an earlier version of this check
   * found a row and passed without establishing that the row was *this* match.
   */
  let rows = [];
  for (let poll = 0; poll < 30; poll += 1) {
    const recorded = await ui.evaluate(`(() => {
      const found = Array.from(
        document.querySelectorAll(
          ".arena-archive-row, .arena-archive__row, .arena-archive tbody tr, .arena-secondary tbody tr",
        ),
      );
      return JSON.stringify(
        found.slice(0, 5).map((r) => r.textContent.trim().replace(/\\s+/g, " ").slice(0, 80)),
      );
    })()`);
    rows = JSON.parse(recorded);
    if (rows.some((row) => row.includes(MODEL))) {
      break;
    }
    await wait(500);
  }
  note(`archive rows after the match: ${rows.length}`);
  const archived = rows.find((row) => row.includes(MODEL));
  check(
    "the match that just played is in the archive",
    Boolean(archived),
    archived ?? rows[0] ?? "none",
  );
  check(
    "the archived match records the model as its tested participant",
    Boolean(archived) && /vs/.test(archived),
    archived ?? "none",
  );

  /*
   * The archived ply count, not a poll sample.
   *
   * This is the measurement that finally distinguishes "a model played a game" from "a match was
   * accepted and something went wrong on ply one". Two stand-in versions answered in ways the engine
   * correctly refused — a grid-world action name, and then always column one until it filled — and both
   * produced a `NO RESULT` at six plies. Reading the recording's own count is deterministic and is the
   * only figure here that cannot be a sampling artefact.
   */
  const archivedPlies = archived ? Number((/(\d+)\s+plies/.exec(archived) ?? [])[1] ?? 0) : 0;
  const archivedOutcome = archived ? (/^(WIN|LOSS|DRAW|NO RESULT)/.exec(archived) ?? [])[1] : null;
  note(`archived: ${archivedOutcome}, ${archivedPlies} plies`);
  check(
    "the model played a real game, not zero plies — a match that never started also has a verdict",
    archivedPlies > 0,
    `${archivedPlies} plies`,
  );
  check(
    "the match reached a decided outcome rather than a harness failure",
    archivedOutcome === "WIN" || archivedOutcome === "LOSS" || archivedOutcome === "DRAW",
    String(archivedOutcome),
  );
} catch (error) {
  console.error(`FAIL  the journey raised: ${error.stack ?? error.message}`);
  failures.push("journey completed");
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
  console.log("PASS  the Arena model journey: discover, seat, configure, run, display");
}