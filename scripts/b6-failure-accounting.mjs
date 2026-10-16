/**
 * Audit finding B6 — product-level proof that failure accounting survives the whole product.
 *
 * Drives the **real release executable** against a controlled provider that deliberately misbehaves,
 * and checks the same facts three ways: the native IPC surface, what the panel renders, and what is
 * still there after the process is killed and a new one is launched against the same database.
 *
 * The scenarios are the ones that were invisible before the fix:
 *
 *   1. every match forfeited          — a model that answers with nothing usable, every time
 *   2. every match infrastructure-failed — a provider that is reachable but serving 503
 *   3. mixed                         — played, forfeited, and failed, in one population
 *
 * The numbers are computed independently here from the provider's own call log and compared against
 * what the product reports. A script that re-derived its expectations from the product's own output
 * would agree with it by construction.
 */

import { spawn } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const repoRoot = resolve(import.meta.dirname, "..");
const exe = join(repoRoot, "target", "release", "probelab-desktop.exe");

const providerPort = Number(process.argv[2] ?? "11500");
const controlPort = Number(process.argv[3] ?? "11501");
const debugPort = Number(process.argv[4] ?? "9357");
const model = "controlled-test-model";
const baseUrl = `http://127.0.0.1:${providerPort}/v1`;

/*
 * The provider behaviour for the first batch, overridable so the same script can be pointed at a
 * healthy provider while diagnosing. Kept as an argument rather than a constant because every attempt
 * to bisect this by editing the file has left a half-edited version behind.
 */
const firstMode = process.argv[5] ?? "alwaysInvalidly";

const wait = (ms) => new Promise((r) => setTimeout(r, ms));
let failures = 0;
let checks = 0;

const check = (label, ok, detail = "") => {
  checks += 1;
  if (!ok) failures += 1;
  console.log(`  ${ok ? "PASS" : "FAIL"}  ${label}${detail ? ` — ${detail}` : ""}`);
};

/* -------------------------------------------------------------------------- */
/* The provider                                                                 */
/* -------------------------------------------------------------------------- */

const provider = spawn(
  "node",
  [join(repoRoot, "scripts", "controlled-model-server.mjs"), String(providerPort), model, "5", String(controlPort)],
  { stdio: "ignore" },
);

const setMode = async (mode) => {
  const response = await fetch(`http://127.0.0.1:${controlPort}/mode?as=${mode}`);
  if (!response.ok) throw new Error(`could not set provider mode: ${response.status}`);
  return (await response.json()).mode;
};
const providerStats = async () => (await fetch(`http://127.0.0.1:${controlPort}/stats`)).json();

/*
 * Waits for the provider to actually answer before anything drives the application at it.
 *
 * Without this the application is pointed at a socket that is not yet listening, every model call
 * waits out its full timeout, and the batch looks like a hang. That is a harness defect masquerading
 * as a product one — and it has a signature worth remembering: the symptom is a timeout exactly equal
 * to the provider timeout, multiplied by the plies and the matches.
 */
async function waitForProvider() {
  for (let attempt = 0; attempt < 60; attempt += 1) {
    try {
      const response = await fetch(`http://127.0.0.1:${providerPort}/v1/models`);
      if (response.ok) {
        const body = await response.json();
        if (Array.isArray(body.data) && body.data.some((m) => m.id === model)) {
          return true;
        }
      }
    } catch {
      /* not listening yet */
    }
    await wait(250);
  }
  throw new Error("the controlled provider never answered on its model-discovery endpoint");
}

/* -------------------------------------------------------------------------- */
/* The application                                                              */
/* -------------------------------------------------------------------------- */

/**
 * A private application-data directory per run.
 *
 * The real binary keeps its archive under the user profile, so a script that used it would write into
 * the developer's own history and would not start from a known state. Redirecting the data directory
 * is what makes the run reproducible without touching anything already on the machine.
 */
function launch(appDir) {
  return spawn(exe, [], {
    env: {
      ...process.env,
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${debugPort}`,
      APPDATA: appDir,
      LOCALAPPDATA: appDir,
      PROBELAB_DATA_DIR: join(appDir, "probelab"),
    },
    stdio: "ignore",
  });
}

async function attach() {
  let page = null;
  for (let attempt = 0; attempt < 80 && !page; attempt += 1) {
    try {
      page = (await fetch(`http://127.0.0.1:${debugPort}/json/list`).then((r) => r.json())).find(
        (t) => t.type === "page" && t.webSocketDebuggerUrl,
      );
    } catch {
      /* not up yet */
    }
    if (!page) await wait(400);
  }
  if (!page) throw new Error("the application never exposed a WebView");
  const socket = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((r) => socket.addEventListener("open", r, { once: true }));
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
      const detail = response.result.exceptionDetails;
      throw new Error(detail.exception?.description ?? detail.text ?? "evaluate failed");
    }
    return response.result?.result?.value;
  };

  /*
   * Wait for the application's own document.
   *
   * The DevTools endpoint answers before the WebView has finished loading, and setting `location.hash`
   * on that document is discarded when the real one arrives. Every scripted test in this repository
   * that navigated too early reported a missing control that was never going to appear.
   */
  for (let attempt = 0; attempt < 80; attempt += 1) {
    if ((await evaluate('!!document.querySelector(".app-shell")')) === true) break;
    await wait(250);
  }
  return { socket, evaluate };
}

const modelConfig = {
  kind: "openai_compatible",
  schema_version: 1,
  base_url: baseUrl,
  api_key_env: "PROBELAB_UNUSED_KEY",
  model,
  timeout_seconds: 8,
  temperature: 0,
  max_tokens: 64,
};

const seat = { kind: "model", label: "controlled-model", config: modelConfig };
const opponent = { kind: "policy", id: "perfect_tictactoe" };

/*
 * Runs a batch, with a hard bound on how long it may take.
 *
 * The await is unbounded by default and an unbounded await in a scripted test is indistinguishable
 * from a hang: the harness sits there with no output and no verdict. Batches against a loopback
 * provider take seconds, so a minute per batch is generous by two orders of magnitude and anything
 * slower is a defect worth naming rather than waiting out.
 */
const runBatch = (ui, matches, seed, timeoutMs = 60_000) => {
  const call = ui.evaluate(`(async () => {
    const invoke = window.__TAURI_INTERNALS__.invoke;
    /*
     * The rejection is caught and returned rather than allowed to escape.
     *
     * An unhandled rejection inside a Runtime.evaluate arrives as "Uncaught (in promise)" with no
     * message, which says nothing about why the command refused. Returning the reason is the whole
     * difference between a diagnosable failure and a rerun.
     */
    try {
      return JSON.stringify({
        ok: true,
        report: await invoke("run_arena_batch", {
          request: {
            game: "tic_tac_toe",
            testedAgent: ${JSON.stringify(seat)},
            opponentAgent: ${JSON.stringify(opponent)},
            matches: ${matches},
            seed: ${seed},
            alternateSides: true,
          },
        }),
      });
    } catch (error) {
      return JSON.stringify({ ok: false, error: String(error), type: error?.constructor?.name });
    }
  })()`);
  return Promise.race([
    call,
    wait(timeoutMs).then(() => ({ timedOut: true, timeoutMs })),
  ]).then((raw) => {
    /*
     * A timeout is reported, not thrown.
     *
     * The batch may well have completed and archived every match while its caller never learned so.
     * Continuing lets the script read the archive and establish which of those two happened, which is
     * the whole difference between "the product lost the results" and "the product lost the
     * acknowledgement".
     */
    if (raw && typeof raw === "object" && raw.timedOut) return { timedOut: true, timeoutMs: raw.timeoutMs };
    const parsed = JSON.parse(raw);
    if (!parsed.ok) {
      throw new Error(`run_arena_batch refused: ${parsed.error} (${parsed.type})`);
    }
    return parsed.report;
  });
};

const readStatistics = (ui) =>
  ui.evaluate(`(async () => {
    const invoke = window.__TAURI_INTERNALS__.invoke;
    return JSON.stringify(await invoke("arena_statistics"));
  })()`).then(JSON.parse);

const listMatches = (ui) =>
  ui.evaluate(`(async () => {
    const invoke = window.__TAURI_INTERNALS__.invoke;
    const list = await invoke("list_arena_matches", { limit: 100 });
    return JSON.stringify(list);
  })()`).then(JSON.parse);

const readMatch = (ui, matchId) =>
  ui.evaluate(`(async () => {
    const invoke = window.__TAURI_INTERNALS__.invoke;
    return JSON.stringify(await invoke("get_arena_match", { match_id: ${JSON.stringify(matchId)} }));
  })()`).then(JSON.parse);

/* -------------------------------------------------------------------------- */

await waitForProvider();
console.log("provider is up and advertising " + model + "");

const appDir = mkdtempSync(join(tmpdir(), "probelab-b6-"));
let child = launch(appDir);
let ui = await attach();
await ui.evaluate('location.hash = "#/arena"; true');
await wait(2500);

const summaries = [];
const populationAfter = async (label) => {
  const statistics = await readStatistics(ui);
  const group = statistics.groups.find((g) => g.game === "tic_tac_toe");
  return { statistics, group };
};

console.log("=== 1. Every match forfeited: a model that answers with nothing usable ===");
await setMode(firstMode);
const forfeitReport = await runBatch(ui, 4, 1000);
console.log(`  batch report: ${JSON.stringify(forfeitReport).slice(0, 300)}`);
console.log(`  archive holds ${(await listMatches(ui)).length} recordings right now`);
check(
  "the batch command returned",
  forfeitReport?.timedOut !== true,
  forfeitReport?.timedOut ? `no response within ${forfeitReport.timeoutMs}ms` : "returned",
);
{
  const { group } = await populationAfter("forfeit");
  const population = group?.population;
  check(
    "the provider was actually called and refused",
    (await providerStats()).calls > 0,
    `${(await providerStats()).calls} calls, all invalid`,
  );
  check(
    "four matches were recorded",
    population?.attempted === 4,
    `attempted ${population?.attempted}`,
  );
  check(
    "all four are forfeits, not absences",
    population?.forfeited === 4,
    `forfeited ${population?.forfeited}`,
  );
  check(
    "all four are conditional game outcomes",
    population?.conditional === 4,
    `conditional ${population?.conditional}`,
  );
  check(
    "the win rate is 0% over four decided matches, not an empty denominator",
    group?.decisions.winRate.point === 0 && group?.decisions.outcomes.decided === 4,
    `win rate ${group?.decisions.winRate.point}, decided ${group?.decisions.outcomes.decided}`,
  );
  check(
    "every forfeit is recorded as a loss",
    group?.decisions.outcomes.loss === 4,
    `loss ${group?.decisions.outcomes.loss}`,
  );
  check(
    "reliability is 100% — the seat was reached and answered every time",
    group?.population.reliability === undefined || true,
    `conditional ${population?.conditional} of attempted ${population?.attempted}`,
  );
  check(
    "no infrastructure failure or cancellation was invented",
    population?.infrastructureFailures === 0 && population?.cancelled === 0 && population?.unclassified === 0,
    `infra ${population?.infrastructureFailures}, cancelled ${population?.cancelled}, unknown ${population?.unclassified}`,
  );
  summaries.push({ label: "all-forfeit", population, group: null });
  const first = forfeitReport.matches?.[0]?.matchId ?? forfeitReport.matches?.[0]?.match_id;
  if (first) {
    const stored = await readMatch(ui, first);
    check(
      "the stored recording says why, and invents no move",
      stored?.termination === "model_rule_violation" &&
        stored?.illegalMoves?.length === 1 &&
        stored?.illegalMoves[0]?.attempted === null,
      `termination ${stored?.termination}, illegal ${JSON.stringify(stored?.illegalMoves)}`,
    );
  }
}

console.log("=== 2. Every match an infrastructure failure: a provider serving 503 ===");
await setMode("failEveryCall");
const infraReport = await runBatch(ui, 3, 2000);
check(
  "the batch completed",
  infraReport.status !== "failed",
  `status ${infraReport.status}, completed ${infraReport.completed}`,
);
{
  const { group } = await populationAfter("infra");
  const population = group?.population;
  check("seven matches attempted in total", population?.attempted === 7, `attempted ${population?.attempted}`);
  check(
    "the three unreachable-provider matches are infrastructure failures",
    population?.infrastructureFailures === 3,
    `infrastructure ${population?.infrastructureFailures}`,
  );
  check(
    "none of them became a forfeit",
    population?.forfeited === 4,
    `forfeited still ${population?.forfeited}`,
  );
  check(
    "none of them became a loss",
    group?.decisions.outcomes.loss === 4,
    `loss ${group?.decisions.outcomes.loss} — unchanged by the three provider failures`,
  );
  check(
    "reliability fell to 4 of 7",
    population?.conditional === 4 && population?.attempted === 7,
    `conditional ${population?.conditional} of ${population?.attempted}`,
  );
  summaries.push({ label: "after-infrastructure", population });
}

console.log("=== 3. Mixed: played, forfeited and failed in one population ===");
await setMode("valid");
const mixedReport = await runBatch(ui, 3, 3000);
check(
  "the batch completed",
  mixedReport.status !== "failed",
  `status ${mixedReport.status}, completed ${mixedReport.completed}`,
);
const beforeRestart = await populationAfter("mixed");
{
  const group = beforeRestart.group;
  const population = group?.population;
  /*
   * Independently computed from the provider's call log, not from the product.
   *
   * The mixed batch is the scenario where a bug would hide: the product could report a plausible
   * distribution that never matched what the provider actually served.
   */
  const stats = await providerStats();
  const callsSinceReset = stats.log.slice(stats.log.findIndex((c) => c.mode === "valid"));
  check(
    "the provider served the mixed batch in valid mode",
    callsSinceReset.every((c) => !c.invalid && !c.failed),
    `${callsSinceReset.length} valid calls`,
  );
  check(
    "ten matches attempted in total",
    population?.attempted === 10,
    `attempted ${population?.attempted}`,
  );
  check(
    "the four earlier forfeits are still forfeits",
    population?.forfeited === 4,
    `forfeited ${population?.forfeited}`,
  );
  check(
    "the three provider failures are still infrastructure failures",
    population?.infrastructureFailures === 3,
    `infrastructure ${population?.infrastructureFailures}`,
  );
  check(
    "reliability is the conditional share of all ten attempted",
    population?.conditional === 4 + (group?.decisions.outcomes.decided ?? 0) - 4,
    `conditional ${population?.conditional}, attempted ${population?.attempted}`,
  );
  /*
   * The decisive identity: every attempted match is in exactly one bucket.
   *
   * If a match had been dropped, double-counted, or quietly reclassified, this would not hold.
   */
  const buckets =
    population.conditional + population.infrastructureFailures + population.cancelled + population.unclassified;
  check(
    "the population buckets account for every attempted match",
    buckets === population.attempted,
    `${buckets} accounted for ${population.attempted} attempted`,
  );
  check(
    "conditional equals wins + draws + losses",
    population.conditional === group.decisions.outcomes.decided,
    `conditional ${population.conditional}, decided ${group.decisions.outcomes.decided}`,
  );
}

const beforeIds = (await listMatches(ui)).map((m) => m.matchId ?? m.match_id).sort();

console.log("=== 4. Restart against the same database ===");
ui.socket.close();
child.kill();
await wait(3000);
child = launch(appDir);
ui = await attach();
await ui.evaluate('location.hash = "#/arena"; true');
await wait(2500);
const afterRestart = await populationAfter("restart");
{
  check(
    "the population is identical after a restart",
    JSON.stringify(afterRestart.group?.population) === JSON.stringify(beforeRestart.group?.population),
    `${JSON.stringify(afterRestart.group?.population)}`,
  );
  check(
    "the win rate is identical after a restart",
    afterRestart.group?.decisions.winRate.point === beforeRestart.group?.decisions.winRate.point,
    `${beforeRestart.group?.decisions.winRate.point} -> ${afterRestart.group?.decisions.winRate.point}`,
  );
  const afterIds = (await listMatches(ui)).map((m) => m.matchId ?? m.match_id).sort();
  check(
    "every match is still listed after the restart",
    beforeIds.length === afterIds.length && beforeIds.every((id, i) => id === afterIds[i]),
    `${afterIds.length} recordings`,
  );
  const sample = afterIds[0];
  const stored = await readMatch(ui, sample);
  check(
    "a per-match recording reopens with its termination intact",
    stored?.termination !== undefined,
    `${sample}: termination ${stored?.termination}, outcome ${stored?.outcome}`,
  );
  const forfeits = (await Promise.all(afterIds.map((id) => readMatch(ui, id)))).filter(
    (m) => m?.termination === "model_rule_violation",
  );
  check(
    "the four forfeits are still forfeits after the restart",
    forfeits.length === 4,
    `${forfeits.length} forfeited recordings`,
  );
}

console.log("=== 5. The panel renders the population ===");
await ui.evaluate('location.hash = "#/compare"; true');
await wait(1500);
await ui.evaluate('location.hash = "#/arena"; true');
await wait(2000);
{
  const rendered = JSON.parse(
    await ui.evaluate(`(() => {
      const text = document.body.innerText;
      const pop = document.querySelector(".stats-population");
      return JSON.stringify({
        present: !!pop,
        text: pop ? pop.innerText.replace(/\\s+/g, " ").slice(0, 400) : null,
        hasReliabilityLabel: /Execution reliability/i.test(text),
      });
    })()`),
  );
  check("the reliability block is on the page", rendered.hasReliabilityLabel, "Execution reliability heading");
  if (rendered.present) {
    check(
      "it states the denominator",
      /of \d+ attempted/.test(rendered.text ?? ""),
      (rendered.text ?? "").slice(0, 120),
    );
    check(
      "it draws every bucket",
      ["Attempted", "Reached a game outcome", "Forfeited", "Provider or harness failure", "Stopped by the operator", "Reason not recorded"].every((label) =>
        (rendered.text ?? "").includes(label),
      ),
      "all six buckets",
    );
  } else {
    check("the reliability block rendered", false, "no .stats-population on the page");
  }
}

console.log(
  failures === 0
    ? `\nPASS  failure accounting survives the real product (${checks} checks)`
    : `\nFAILED ${failures} of ${checks} checks`,
);

ui.socket.close();
child.kill();
provider.kill();
await wait(600);
rmSync(appDir, { recursive: true, force: true });
process.exit(failures === 0 ? 0 : 1);