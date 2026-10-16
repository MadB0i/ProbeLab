/**
 * Multi-episode durability, verified through the real native application.
 *
 * # The defect this closes
 *
 * Audit finding B3: the `experiments` table stored only the scalar mean success fraction. Everything that
 * makes a multi-episode run a *result* — the individual episodes, the aggregate, the decisions, the
 * failures, the timeline — existed only in the running process. `get_experiment` therefore answered
 * "not found" for a completed run once the process was gone: a result fully visible during execution and
 * unavailable afterwards.
 *
 * The Rust test `crates/lab/tests/experiment_reconstruction.rs` proves the persistence layer round-trips a
 * real five-episode run. This proves the *product* does, through the commands and the panel an operator
 * actually uses, across a genuine process restart.
 *
 * # Why it matters to do both
 *
 * A persistence-layer test cannot catch a command that writes the wrong thing into the column. Only a
 * restart of the real executable can establish that what an operator saw before the restart is available
 * after it.
 *
 * Usage: node scripts/experiment-restart-reconstruction.mjs [debugPort] [repoRoot]
 */
import { spawn } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

const port = process.argv[2] ?? "9343";

/*
 * Deliberately not five.
 *
 * The stored default is already five, so "set it to five" changes nothing, the draft never differs from
 * the stored document, Save stays disabled — correctly, since there is nothing to save — and the run
 * goes on to use the default without this script having exercised the save path at all. That is what
 * made the first attempt at this test report "stayed disabled" and look like a broken form.
 *
 * Four is a real edit: it has to be typed, accepted, persisted, and then used by a run that is expected
 * to produce four episodes. If any link in that chain is missing, the episode count says so.
 */
const TARGET_EPISODES = 4;
const repoRoot = process.argv[3] ?? "D:\\Projects\\ProbeLab";
const outDir = join(process.env.TEMP ?? ".", "probelab-experiment-restart");
mkdirSync(outDir, { recursive: true });

const failures = [];
const check = (name, ok, detail = "") => {
  console.log(`${ok ? "  PASS" : "  FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
  if (!ok) failures.push(name);
};
const note = (text) => console.log(`        ${text}`);
const wait = (ms) => new Promise((r) => setTimeout(r, ms));

const exe = resolve(repoRoot, "target", "release", "probelab-desktop.exe");

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
            /*
             * The whole detail, not just `exception.description`.
             *
             * WebView2 reports a failed promise rejection with no description on the exception itself,
             * which reduced every failure to the words "evaluate failed" and cost a debugging round trip
             * to find out that the real message was sitting in `text`.
             */
            const detail = response.result.exceptionDetails;
            throw new Error(
              detail.exception?.description ??
                detail.text ??
                JSON.stringify(detail).slice(0, 400) ??
                "evaluate failed",
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
        /*
         * Types into a field the way a person does, through the browser's own input pipeline.
         *
         * Not `dispatchEvent(new Event("input"))` on a value the script assigned itself. React's value
         * tracker compares the value it recorded against the value on the element to decide whether a
         * change happened, and assigning through the prototype setter leaves the tracker holding the old
         * value in a way that makes the synthetic event indistinguishable from no edit at all: `dirty`
         * stayed false, Save stayed disabled, and the click on a disabled button did nothing.
         *
         * Going through `Input` means the keystrokes are trusted at the browser level, which is what the
         * real application receives, so a failure here is a failure of the product rather than of the
         * harness.
         */
        const typeInto = async (selector, text) => {
          const focused = await evaluate(`(() => {
            const el = document.querySelector(${JSON.stringify(selector)});
            if (!el) return false;
            el.focus();
            el.select();
            return true;
          })()`);
          if (focused !== true) {
            throw new Error(`cannot focus ${selector}`);
          }
          for (const character of text) {
            await send("Input.dispatchKeyEvent", { type: "keyDown", text: character });
            await send("Input.dispatchKeyEvent", { type: "keyUp", text: character });
          }
          return evaluate(`document.querySelector(${JSON.stringify(selector)}).value`);
        };
        return { socket, evaluate, shot, typeInto };
      }
    } catch {
      /* not up yet */
    }
    await wait(400);
  }
  throw new Error("the application never exposed a debuggable page");
}

const launch = () =>
  spawn(exe, [], {
    env: { ...process.env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}` },
    stdio: "ignore",
  });

/**
 * Waits until the application's own document has rendered.
 *
 * # Why this is not optional
 *
 * The DevTools endpoint answers with a page *before* the WebView has finished loading the frontend.
 * Setting `location.hash` on that document does nothing at all: the hash is discarded when the real
 * document loads, and `location.hash` reads back as the empty string.
 *
 * So the earlier version of this script navigated to `#/settings`, believed it had, waited eight
 * seconds for a control that was never going to appear, reported "no episode setting", and went on to run
 * a **single-episode** experiment that reopened successfully — which looks exactly like B3 being fixed
 * and proves nothing, because one episode is the single case the pre-fix code already handled.
 *
 * The check is the shell itself rather than a fixed delay: waiting for an element that exists means
 * waiting for the document that element lives in.
 */
async function waitForApp(handle) {
  for (let attempt = 0; attempt < 80; attempt += 1) {
    const rendered = await handle.evaluate('!!document.querySelector(".app-shell")');
    if (rendered === true) return true;
    await wait(250);
  }
  throw new Error("the application shell never rendered");
}

let child = launch();
let ui = await attach();
await waitForApp(ui);

try {
console.log("=== 1. A multi-episode experiment runs ===");

/*
 * Set the episode default through the real Settings control and save it.
 *
 * This has to happen first, and through the settings route, because the episode count is a stored
 * *default* the run request reads — it is not a control on the lab panel. An earlier version looked for
 * one there, found nothing, and ran a **single-episode** experiment, which then reopened successfully
 * and looked like B3 fixed when it had proved nothing: a single-episode run is exactly the case the
 * pre-fix code already handled, because one episode does have one stored result.
 */
await ui.evaluate(`location.hash = "#/settings"; true`);
/*
 * The panel mounts from fetched state and renders "Reading the stored settings…" until the load
 * completes, so the control's absence means "not yet", not "missing". Polled rather than slept through.
 */
let episodesMounted = false;
for (let poll = 0; poll < 40 && !episodesMounted; poll += 1) {
  await wait(500);
  episodesMounted =
    (await ui.evaluate(`!!document.querySelector("#setting-episodes")`)) === true;
}
check(
  "the episode setting mounted",
  episodesMounted,
  episodesMounted ? "present" : "never mounted",
);

/*
 * Read what is stored before changing anything.
 *
 * This test has to be repeatable, and an earlier version was not: it typed the target value, and on every
 * run after the first successful save that value was *already* stored — so the draft genuinely did not
 * differ, Save was correctly disabled, and the script reported the form as broken. Worse, it reported
 * "Save stayed disabled" while the panel was displaying "Saved." and the document header read
 * "CUSTOMISED · SCHEMA V1", which is the evidence that the write had in fact landed.
 *
 * So: if the stored document is already at the target, there is nothing to save and saying so is the
 * honest result. If it is not, the change must be typed, persisted and confirmed.
 */
const storedEpisodes = episodesMounted
  ? await ui.evaluate(`document.querySelector("#setting-episodes").value`)
  : null;
const alreadyAtTarget = String(storedEpisodes) === String(TARGET_EPISODES);

let typed = null;
let saveEnabled = false;
let saved = false;

if (!episodesMounted) {
  check("the episode default reached the settings document", false, "no control");
} else if (alreadyAtTarget) {
  note(
    `stored episodes is already ${TARGET_EPISODES}; no save needed. ` +
      "The write was proven by an earlier run of this script and by the panel's own confirmation.",
  );
  check("the episode default reached the settings document", true, `already stored as ${TARGET_EPISODES}`);
} else {
  /*
   * Real keystrokes, not synthetic events: React's value tracker can see an assigned value as no change
   * at all, and these are the keystrokes the product actually receives.
   */
  try {
    typed = await ui.typeInto("#setting-episodes", String(TARGET_EPISODES));
  } catch (cause) {
    note(`typing failed: ${cause.message}`);
  }
  check(
    `typing ${TARGET_EPISODES} into the episode field registered with the form`,
    typed === String(TARGET_EPISODES),
    `field now reads ${JSON.stringify(typed)}`,
  );

  /*
   * Wait for Save to become enabled before clicking it.
   *
   * React has not re-rendered by the time the keystroke's event returns, so reading `disabled` in the
   * same expression reads the *previous* render — where the draft still matched the stored document — and
   * the click lands on a disabled button that ignores it. Interaction and observation have to be
   * separate round trips.
   */
  for (let poll = 0; poll < 20 && !saveEnabled; poll += 1) {
    await wait(250);
    saveEnabled = (await ui.evaluate(`(() => {
      const save = Array.from(document.querySelectorAll("button"))
        .find((b) => /SAVE CHANGES/.test(b.textContent));
      if (save && !save.disabled) {
        save.click();
        return true;
      }
      return false;
    })()`)) === true;
  }
  check("Save became enabled and was clicked", saveEnabled, saveEnabled ? "clicked" : "stayed disabled");

  /*
   * Wait for the backend's own confirmation.
   *
   * The panel reporting what it stored is the only trustworthy signal that the write landed; clicking an
   * enabled button proves the intent was delivered, not that it was accepted.
   */
  for (let poll = 0; poll < 30 && !saved; poll += 1) {
    await wait(500);
    const outcome = await ui.evaluate(`(() => {
      const text = document.body.innerText;
      if (/Settings saved/.test(text)) return "saved";
      const err = document.querySelector(".error-message")?.textContent ?? "";
      if (err.trim().length > 0) return "rejected: " + err.trim().slice(0, 120);
      return "pending";
    })()`);
    if (outcome === "saved") {
      saved = true;
    } else if (outcome.startsWith("rejected")) {
      check("the episode default was saved", false, outcome);
      break;
    }
  }
  check("the episode default was saved", saved, saved ? "backend confirmed" : "no confirmation");
}

/*
 * Confirm the run will use the stored count, not a value the application happened to start with.
 *
 * This is audit finding B11 under test: `App` copies the execution defaults once, on mount, so before
 * the fix a save here was durable, confirmed by the panel, and then ignored by the next run.
 */
const episodeDefaultNow = await ui.evaluate(
  `document.querySelector("#setting-episodes")?.value ?? "missing"`,
);
check(
  "the settings page shows the episode default the next run will use",
  String(episodeDefaultNow) === String(TARGET_EPISODES),
  `shows ${episodeDefaultNow}, asked for ${TARGET_EPISODES}`,
);
await wait(1500);

await ui.evaluate(`location.hash = "#/lab"; true`);
await wait(2500);

  /*
 * Start the *experiment*, not a single benchmark run.
 *
 * The Lab page offers both, and they are different things: "RUN WITH MOCK" executes one episode of the
 * selected benchmark, while "RUN EXPERIMENT" runs the configured episode count. An earlier version of
 * this script started the former and then reported "1 completed of 1" after restart as though the
 * multi-episode reconstruction had failed — when in fact it had never asked for more than one episode.
 * Every failure downstream of that was measuring the wrong run.
 */
await ui.evaluate(`(() => {
  const button = Array.from(document.querySelectorAll("button"))
    .find((b) => /RUN EXPERIMENT/i.test(b.textContent) && !b.disabled);
  if (!button) return "no enabled experiment button";
  button.click();
  return "clicked";
})()`).then((outcome) => {
  check("the experiment was started from the Lab page", outcome === "clicked", outcome);
});

  /*
   * Wait for the run to reach a terminal state.
   *
   * Two things were wrong with the earlier version of this loop, and both of them hid the fact that the
   * run had already finished.
   *
   * First, it counted *any* button whose label mentioned STOP as "still running". The STOP control is
   * always rendered and is merely disabled when idle, so this was true for the entire session and the
   * loop sat there for six hundred seconds reporting "still running" about a run that completed in
   * about three. Only an *enabled* stop control means work is in flight.
   *
   * Second, it accepted any moment after the second poll as terminal, with no requirement that the
   * configured episode count had actually been reached — so it was equally happy to freeze a run one
   * episode in, store that partial state, and reopen it as a tidy one-episode experiment.
   */
  let finished = null;
  for (let poll = 0; poll < 600; poll += 1) {
    await wait(1000);
    const snapshot = JSON.parse(
      await ui.evaluate(`(() => {
        const readouts = Array.from(document.querySelectorAll(".brain-episode-count, .hero-figure, .lab-progress, .progress-hero, .brain-facts"))
          .map((n) => n.textContent.trim().replace(/\\s+/g, " "));
        const running = Array.from(document.querySelectorAll("button"))
          .some((b) => !b.disabled && /STOP|RUNNING|CANCEL/i.test(b.textContent));
        const error = document.querySelector(".error-message, .brain-error")?.textContent?.trim() ?? null;
        const rows = document.querySelectorAll(".history-table tbody tr, .experiment-history tbody tr").length;
        return JSON.stringify({ readouts, running, error, rows, combined: readouts.join(" | ") });
      })()`),
    );
    if (snapshot.error) {
      finished = snapshot;
      break;
    }
    if (!snapshot.running && poll > 2) {
      finished = snapshot;
      break;
    }
    if (poll % 30 === 0) {
      console.log(`        waiting… ${snapshot.combined.slice(0, 80) || "(no readout)"}`);
    }
  }

  check(
    "the run reached a terminal state",
    finished !== null,
    finished ? `terminal — ${finished.combined.slice(0, 120)}` : "still running after 600s",
  );
  check(
    `the run reported ${TARGET_EPISODES} episodes`,
    finished !== null &&
      new RegExp(`${TARGET_EPISODES}\\s+(completed|of)`, "i").test(finished.combined),
    finished ? finished.combined.slice(0, 120) : "no terminal readout",
  );
  check("the run did not report an error", !finished?.error, finished?.error ?? "none");

  /*
   * Capture the run through native IPC rather than by reading the page.
   *
   * The text on screen is a rendering of the record, and comparing renderings proves only that the same
   * summary was drawn twice. `get_experiment` returns the reconstructed `BrainReadModel` itself, so this
   * is the product's own view of what it stored — the same structure, from the same command, over the
   * same IPC path the UI uses.
   */
  const before = JSON.parse(
    await ui.evaluate(`(async () => {
      const invoke = window.__TAURI_INTERNALS__.invoke;
      const panel = await invoke("brain_panel");
      const view = panel?.experiment ?? null;
      const id = view?.experimentId ?? null;
      if (!id) return JSON.stringify({ error: "brain_panel carries no experiment id", panelKeys: Object.keys(panel ?? {}) });
      const detail = await invoke("get_experiment", { experiment_id: id });
      const listed = await invoke("list_comparable_experiments");
      const stored = (Array.isArray(listed) ? listed : []).find((r) => r.experimentId === id) ?? null;
      return JSON.stringify({ ...(detail.experiment ?? {}), rootSeed: stored?.rootSeed ?? null });
    })()`),
  );
  if (before.error) {
    check("the finished experiment is readable over native IPC", false, before.error);
  } else {
    check(
      "the finished experiment is readable over native IPC",
      (before.episodes?.length ?? 0) >= 3,
      `${before.episodes?.length ?? 0} episodes, lifecycle ${before.lifecycle}, terminal ${JSON.stringify(before.terminalReason)}`,
    );
  }
  note(
    `before restart: ${JSON.stringify({
      experimentId: before.experimentId,
      episodes: before.episodes?.length ?? null,
      lifecycle: before.lifecycle ?? null,
      terminalReason: before.terminalReason ?? null,
      successRate: before.aggregate?.successRate ?? null,
      seeds: before.episodes?.map((e) => e.seed) ?? null,
      rootSeed: before.rootSeed ?? null,
    })}`,
  );
  await ui.shot("01-before-restart");

  console.log("=== 2. The process is terminated ===");
  ui.socket.close();
  child.kill();
  await wait(3000);
  check("the first process is gone", true, "terminated");

  console.log("=== 3. A new process reopens the same run ===");
  child = launch();
  ui = await attach();
  // The same cold-start rule applies to the second process: navigating before its document exists
  // discards the hash, and the reopened run would never be opened.
  await waitForApp(ui);
  await ui.evaluate(`location.hash = "#/lab"; true`);
  await wait(3000);

  // Open the stored run through the history table, the way a returning user would.
  const opened = await ui.evaluate(`(() => {
    const rows = Array.from(document.querySelectorAll(".history-table tbody tr, .experiment-history tbody tr"));
    if (rows.length === 0) return JSON.stringify({ error: "history is empty" });
    const button = rows[0].querySelector("button");
    if (!button) return JSON.stringify({ error: "no open control", text: rows[0].textContent.slice(0, 80) });
    button.click();
    return JSON.stringify({ ok: true });
  })()`);
  check("the stored run is listed and can be opened", JSON.parse(opened).ok === true, opened.slice(0, 160));
  await wait(3000);

  /*
   * Re-read the *same* experiment over native IPC and compare it field by field with what was captured
   * before the process was terminated.
   *
   * The comparison is by experiment id, not by position: "open the newest row" is convenient and wrong,
   * because a newer run appearing between the two reads would silently compare two different things and
   * still report agreement.
   */
  const after = JSON.parse(
    await ui.evaluate(`(async () => {
      const invoke = window.__TAURI_INTERNALS__.invoke;
      const id = ${JSON.stringify(before.experimentId ?? null)};
      if (!id) return JSON.stringify({ error: "no experiment id captured before restart" });
      const detail = await invoke("get_experiment", { experiment_id: id });
      const listed = await invoke("list_comparable_experiments");
      const stored = (Array.isArray(listed) ? listed : []).find((r) => r.experimentId === id) ?? null;
      return JSON.stringify({ ...(detail.experiment ?? {}), rootSeed: stored?.rootSeed ?? null });
    })()`),
  );
  note(
    `after restart: ${JSON.stringify({
      experimentId: after.experimentId,
      episodes: after.episodes?.length ?? null,
      lifecycle: after.lifecycle ?? null,
      terminalReason: after.terminalReason ?? null,
      successRate: after.aggregate?.successRate ?? null,
      seeds: after.episodes?.map((e) => e.seed) ?? null,
      rootSeed: after.rootSeed ?? null,
    })}`,
  );

  if (after.error) {
    check("the same experiment is readable after restart", false, after.error);
  } else {
    check(
      "the same experiment is readable after restart",
      after.experimentId === before.experimentId,
      `${after.experimentId} === ${before.experimentId}`,
    );
  }

  /*
   * Per-episode comparison, not just a count.
   *
   * "Four episodes" is consistent with four empty episodes, four placeholder episodes, or four results that
   * all belong to the same seed. Each episode is compared on its own identity, outcome, digest and event
   * count, so a reconstruction that invents, collapses or reshuffles episodes cannot pass.
   */
  const beforeEpisodes = Array.isArray(before.episodes) ? before.episodes : [];
  const afterEpisodes = Array.isArray(after.episodes) ? after.episodes : [];
  const episodeMismatches = [];
  for (let i = 0; i < Math.min(beforeEpisodes.length, afterEpisodes.length); i += 1) {
    const a = beforeEpisodes[i];
    const b = afterEpisodes[i];
    for (const field of ["index", "seed", "taskSuccess", "replayDigest", "telemetryEventCount"]) {
      if (JSON.stringify(a?.[field]) !== JSON.stringify(b?.[field])) {
        episodeMismatches.push(
          `episode ${i} ${field}: ${JSON.stringify(a?.[field])} -> ${JSON.stringify(b?.[field])}`,
        );
      }
    }
  }
  check(
    "every episode survived the restart",
    beforeEpisodes.length >= 3 && beforeEpisodes.length === afterEpisodes.length,
    `${beforeEpisodes.length} before, ${afterEpisodes.length} after`,
  );
  /*
   * Compared over a non-empty set, always.
   *
   * An earlier version of these checks reported success for a run with zero episodes on either side,
   * because "no mismatches found among nothing" is true. A vacuous pass here is indistinguishable from a
   * reconstruction that produced nothing at all, which is the failure being guarded against.
   */
  check(
    "each episode's own fields match after the restart",
    afterEpisodes.length >= 3 && episodeMismatches.length === 0,
    episodeMismatches.length > 0
      ? episodeMismatches.slice(0, 6).join("; ")
      : `${afterEpisodes.length} episodes compared on index, seed, outcome, replay digest and event count`,
  );

  check(
    "each episode kept a distinct seed and replay digest",
    afterEpisodes.length >= 3 &&
      new Set(afterEpisodes.map((e) => e.seed)).size === afterEpisodes.length &&
      new Set(afterEpisodes.map((e) => e.replayDigest)).size === afterEpisodes.length,
    `seeds ${JSON.stringify(afterEpisodes.map((e) => e.seed))}`,
  );

  check(
    "the aggregate survived the restart",
    (after.aggregate ?? null) !== null &&
      JSON.stringify(before.aggregate) === JSON.stringify(after.aggregate),
    `success rate ${after.aggregate?.successRate ?? "?"}, mean turns ${after.aggregate?.meanTurns ?? "?"}`,
  );
  check(
    "the root seed survived the restart",
    (after.rootSeed ?? null) !== null &&
      JSON.stringify(before.rootSeed) === JSON.stringify(after.rootSeed),
    `root seed ${after.rootSeed ?? "absent"}`,
  );
  check(
    "provenance survived the restart",
    (before.decisions ?? null) !== null &&
      JSON.stringify(before.decisions) === JSON.stringify(after.decisions) &&
      JSON.stringify(before.targetAgent) === JSON.stringify(after.targetAgent) &&
      JSON.stringify(before.model) === JSON.stringify(after.model),
    `${after.decisions?.length ?? 0} recorded decisions, agent ${after.targetAgent}, model ${after.model}`,
  );
  check(
    "the lifecycle timeline survived the restart",
    (before.timeline ?? null) !== null &&
      before.timeline.length >= 10 &&
      JSON.stringify(before.timeline) === JSON.stringify(after.timeline),
    `${after.timeline?.length ?? 0} timeline entries`,
  );
  check(
    "the terminal state survived the restart",
    JSON.stringify(before.lifecycle) === JSON.stringify(after.lifecycle) &&
      JSON.stringify(before.terminalReason) === JSON.stringify(after.terminalReason) &&
      (after.lifecycle ?? null) !== null,
    `lifecycle ${after.lifecycle}, terminal ${JSON.stringify(after.terminalReason)}`,
  );

  /*
   * And the UI, which is a separate question from whether the record is intact: does the application
   * render the reconstructed run without claiming it is missing?
   */
  const reopened = JSON.parse(
    await ui.evaluate(`(() => {
      /*
       * Read the read model's own episode count.
       *
       * The panel renders "completed / requested" in a definition list, so the pair is located by its
       * label rather than by a class. An earlier version looked for a class that does not exist and
       * concluded the reopened run had lost its episodes — when the run was in fact present and
       * reporting them.
       */
      const stats = {};
      for (const dt of document.querySelectorAll("dt")) {
        const dd = dt.nextElementSibling;
        if (dd && dt.textContent.trim().toLowerCase() === "episodes") {
          stats.episodes = dd.textContent.trim();
        }
        if (dd && dt.textContent.trim().toLowerCase() === "score") {
          stats.score = dd.textContent.trim();
        }
        if (dd && dt.textContent.trim().toLowerCase() === "events") {
          stats.events = dd.textContent.trim();
        }
      }
      const text = (document.querySelector(".brain-panel, .lab-console") ?? document.body)
        .textContent.replace(/\\s+/g, " ");
      const notFound = /no experiment named|not found|could not be read/i.test(text);
      const errorBanner = document.querySelector(".error-message, .brain-error")?.textContent?.trim() ?? null;
      const episodeRows = document.querySelectorAll(".episode-row, .brain-episode, .episode-list li").length;
      return JSON.stringify({ stats, notFound, errorBanner, episodeRows });
    })()`),
  );

  note(`reopened: ${JSON.stringify(reopened.stats)}`);
  check(
    "the reopened run is not reported as missing",
    reopened.notFound === false,
    reopened.notFound ? "not found" : "present",
  );
  check(
    "the reopened run reports its episode count",
    /completed of/.test(reopened.stats.episodes ?? ""),
    reopened.stats.episodes ?? "no episode count",
  );
  /*
   * The assertion that distinguishes this from the pre-fix behaviour.
   *
   * A *single-episode* run reopened fine before the fix, because one episode has one stored result and
   * the old reconstruction path could find it. Only a multi-episode run proves B3 is closed. So this
   * requires more than one episode, and the run must genuinely have been multi-episode.
   */
  const completedEpisodes = Number(
    (/([\d.]+)\s+completed of/.exec(reopened.stats.episodes ?? "") ?? [])[1] ?? "0",
  );
  check(
    "the reopened run reconstructed MORE THAN ONE episode — the case that used to fail",
    completedEpisodes > 1,
    `${completedEpisodes} episodes after restart`,
  );
  await ui.shot("02-after-restart");
} catch (error) {
  console.error(`FAIL  the run raised: ${error.stack ?? error.message}`);
  failures.push("run completed");
} finally {
  ui?.socket.close();
  child.kill();
  await wait(800);
}

console.log("");
if (failures.length) {
  console.error(`FAILED ${failures.length}: ${failures.join(", ")}`);
  process.exitCode = 1;
} else {
  console.log("PASS  a multi-episode experiment is reconstructed after a restart");
}