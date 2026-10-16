/**
 * Diagnostic for the unresolved `run_arena_batch`.
 *
 * Establishes the **last successful boundary** by listening for the batch progress event the command
 * already emits, rather than by guessing. That event fires once per archived match, after `runner.play`
 * returns and after `archive.put` succeeds, so the observation pins the fault to a phase:
 *
 *   events arrive, promise never resolves  -> the worker ran and archived everything; the fault is in
 *                                            the return path.
 *   no events, provider never called       -> the block is before the first model call.
 *   no events, provider called             -> the block is inside `runner.play`.
 *
 * Every boundary is stamped with a monotonic clock so a gap can be attributed to a phase rather than
 * to "the batch was slow".
 */

import { spawn } from "node:child_process";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const providerPort = Number(process.argv[2] ?? "11510");
const controlPort = Number(process.argv[3] ?? "11511");
const debugPort = Number(process.argv[4] ?? "9371");
const mode = process.argv[5] ?? "alwaysInvalidly";
const matches = Number(process.argv[6] ?? "2");
const model = "controlled-test-model";

const wait = (ms) => new Promise((r) => setTimeout(r, ms));
const started = Date.now();
const since = () => `${String(Date.now() - started).padStart(6, " ")}ms`;

/* -------------------------------------------------------------------------- */
/* Provider                                                                     */
/* -------------------------------------------------------------------------- */

const provider = spawn(
  "node",
  [join(import.meta.dirname, "controlled-model-server.mjs"), String(providerPort), model, "5", String(controlPort)],
  { stdio: "ignore" },
);

async function waitForProvider() {
  for (let attempt = 0; attempt < 60; attempt += 1) {
    try {
      const response = await fetch(`http://127.0.0.1:${providerPort}/v1/models`);
      if (response.ok) return true;
    } catch {
      /* not listening */
    }
    await wait(250);
  }
  throw new Error("provider never answered");
}

const providerStats = async () => (await fetch(`http://127.0.0.1:${controlPort}/stats`)).json();

/* -------------------------------------------------------------------------- */
/* Application                                                                  */
/* -------------------------------------------------------------------------- */

const appDir = mkdtempSync(join(tmpdir(), "probelab-diag-"));
/*
 * The application's stdio is piped, not ignored.
 *
 * Every earlier attempt at this diagnostic ran the binary with `stdio: "ignore"`, which silently
 * discarded exactly the evidence it was looking for: `RUST_LOG` output goes to stderr, and a piped
 * handle is how a GUI-subsystem process can still be observed from outside.
 */
const child = spawn("D:\\Projects\\ProbeLab\\target\\release\\probelab-desktop.exe", [], {
  env: {
    ...process.env,
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${debugPort}`,
    RUST_LOG: process.env.RUST_LOG ?? "info",
  },
  stdio: ["ignore", "pipe", "pipe"],
});
child.stdout.on("data", (chunk) => process.stdout.write(`[app out] ${chunk}`));
child.stderr.on("data", (chunk) => process.stderr.write(`[app err] ${chunk}`));

const wait_ = (ms) => wait(ms);
let page = null;
for (let attempt = 0; attempt < 80 && !page; attempt += 1) {
  try {
    page = (await fetch(`http://127.0.0.1:${debugPort}/json/list`).then((r) => r.json())).find(
      (t) => t.type === "page" && t.webSocketDebuggerUrl,
    );
  } catch {
    /* not up */
  }
  if (!page) await wait_(400);
}
if (!page) throw new Error("no WebView");

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

for (let attempt = 0; attempt < 80; attempt += 1) {
  if ((await evaluate('!!document.querySelector(".app-shell")')) === true) break;
  await wait_(250);
}
console.log(`[${since()}] application shell rendered`);

/*
 * Installs the event tap before anything is invoked.
 *
 * Tauri events are delivered to `window` by name, so the tap has to be in place first or the first
 * match's progress is missed — and the first event is the one that distinguishes "never started" from
 * "started and hung".
 */
const tap = await evaluate(`(async () => {
  const internals = window.__TAURI_INTERNALS__;
  window.__batchProgress = [];
  try {
    /*
     * Tauri's event plugin does not accept a JS function as the handler; it wants the identifier that
     * \`transformCallback\` produces, which is how the frontend's own listener works. Passing a function
     * straight through JSON serialises to nothing, and the plugin answers "missing required key
     * handler" — which is what the first attempt of this diagnostic did.
     */
    const handlerId = internals.transformCallback((event) => {
      window.__batchProgress.push({ at: Date.now(), payload: event.payload });
    });
    const unlisten = await internals.invoke("plugin:event|listen", {
      event: "probe-lab/arena-batch",
      target: { kind: "Any" },
      handler: handlerId,
    });
    window.__batchUnlisten = unlisten;
    return "tap installed";
  } catch (error) {
    return "tap failed: " + String(error);
  }
})()`);
console.log(`[${since()}] ${tap}`);

console.log(`[${since()}] provider mode -> ${mode}`);
await fetch(`http://127.0.0.1:${controlPort}/mode?as=${mode}`);

/*
 * The invocation is started WITHOUT awaiting it in the page, and its result is parked on `window`.
 *
 * This decouples the observation from the harness. Every earlier attempt used `awaitPromise: true` on
 * the whole call, so "unresolved" could have meant either that the native command never replied or that
 * something in the evaluation channel swallowed the reply — and those two have completely different
 * fixes. Parking the result and polling for it with separate round trips answers the question the
 * harness cannot otherwise answer about itself.
 */
await evaluate(`(() => {
  window.__invocation = { state: "pending", at: Date.now(), value: null, error: null };
  const invoke = window.__TAURI_INTERNALS__.invoke;
  window.__invocation.state = "started";
  invoke("run_arena_batch", {
    request: {
      game: "connect_four",
      testedAgent: {
        kind: "model",
        label: "controlled-model",
        config: {
          kind: "openai_compatible",
          schema_version: 1,
          base_url: "http://127.0.0.1:${providerPort}/v1",
          api_key_env: "PROBELAB_UNUSED_KEY",
          model: "${model}",
          timeout_seconds: 8,
          temperature: 0,
          max_tokens: 64,
        },
      },
      opponentAgent: { kind: "policy", id: "depth_bounded_connect_four" },
      matches: ${matches},
      seed: 4242,
      alternateSides: true,
    },
  })
    .then((value) => {
      window.__invocation.state = "resolved";
      window.__invocation.value = value;
    })
    .catch((error) => {
      window.__invocation.state = "rejected";
      window.__invocation.error = JSON.stringify(error).slice(0, 300);
    });
  return true;
})()`);
console.log(`[${since()}] invocation started (not awaited in-page)`);

let settled = null;
for (let tick = 0; tick < 45; tick += 1) {
  await wait_(1000);
  const state = JSON.parse(await evaluate(`JSON.stringify(window.__invocation)`));
  if (state.state !== "pending" && state.state !== "started") {
    settled = state;
    console.log(`[${since()}] invocation ${state.state} after ${tick + 1}s`);
    break;
  }
  if (tick === 9 || tick === 29) {
    console.log(`[${since()}] still ${state.state} after ${tick + 1}s`);
  }
}

console.log(
  `[${since()}] invocation ${settled ? settled.state.toUpperCase() : "STILL PENDING after 45s"}`,
);
if (settled?.value) {
  console.log(`         batchId ${settled.value.batchId}, completed ${settled.value.completed}`);
}
if (settled?.error) {
  console.log(`         ${settled.error}`);
}

/*
 * Is the whole async runtime wedged, or only this command?
 *
 * A second, known-good command is issued while the batch is outstanding. If that also fails to
 * answer, the runtime or the IPC layer is blocked and the fault is not in the batch at all. If it
 * answers normally, the runtime is healthy and the batch specifically never reaches its worker.
 *
 * This is the difference between "the batch is broken" and "the application stopped responding",
 * which are entirely different bugs with entirely different fixes.
 */
const concurrent = await Promise.race([
  evaluate(`(async () => {
    const invoke = window.__TAURI_INTERNALS__.invoke;
    const t = Date.now();
    const games = await invoke("list_arena_games");
    return JSON.stringify({ answered: true, elapsedMs: Date.now() - t, games: games.length });
  })()`),
  wait_(8000).then(() => "UNANSWERED after 8s"),
]);
console.log(`[${since()}] concurrent list_arena_games: ${concurrent}`);

/*
 * Is the command dispatched at all?
 *
 * The boundary trace printed nothing, not even the first line inside the command body. That leaves two
 * possibilities with opposite fixes: the handler is never entered, or it returns before the first
 * trace. An oversized batch is refused in the validation block, before any tracing exists — so if that
 * refusal comes back promptly, the handler *is* being dispatched and the fault lies earlier than the
 * first trace. If even this hangs, the handler is not running at all.
 *
 * A deliberately non-existent command is issued alongside it to show what a missing handler looks like
 * from the outside, since "no response" and "no such command" are otherwise easy to confuse.
 */
const probes = JSON.parse(
  await evaluate(`(async () => {
    const invoke = window.__TAURI_INTERNALS__.invoke;
    const seat = {
      kind: "model",
      label: "controlled-model",
      config: {
        kind: "openai_compatible",
        schema_version: 1,
        base_url: "http://127.0.0.1:${providerPort}/v1",
        api_key_env: "PROBELAB_UNUSED_KEY",
        model: "${model}",
        timeout_seconds: 8,
        temperature: 0,
        max_tokens: 64,
      },
    };
    const probe = async (label, run) => {
      const t = Date.now();
      try {
        const value = await Promise.race([
          run(),
          new Promise((_, reject) => setTimeout(() => reject(new Error("timeout-10s")), 10_000)),
        ]);
        return { label, answered: true, ms: Date.now() - t, value: JSON.stringify(value).slice(0, 160) };
      } catch (error) {
        return { label, answered: false, ms: Date.now() - t, error: String(error).slice(0, 200), serialized: (() => { try { return JSON.stringify(error); } catch { return "unserialisable"; } })().slice(0, 300) };
      }
    };
    return JSON.stringify([
      await probe("oversized-batch", () =>
        invoke("run_arena_batch", {
          request: {
            game: "connect_four",
            testedAgent: seat,
            opponentAgent: { kind: "policy", id: "depth_bounded_connect_four" },
            matches: 9999,
            seed: 1,
            alternateSides: true,
          },
        }),
      ),
      await probe("nonexistent-command", () => invoke("probe_lab_no_such_command", {})),
      await probe("policy-versus-policy-batch", () =>
        invoke("run_arena_batch", {
          request: {
            game: "connect_four",
            testedAgent: { kind: "policy", id: "first_legal_tictactoe" },
            opponentAgent: { kind: "policy", id: "depth_bounded_connect_four" },
            matches: 2,
            seed: 7,
            alternateSides: true,
          },
        }),
      ),
      /*
       * The same model seat, but through the *single-match* command.
       *
       * The policy-versus-policy batch above completes in milliseconds, which proves the worker, the
       * registry and the report path are all healthy. So the difference is the model seat itself. This
       * probe asks whether that seat is the problem in general or only inside a batch: the
       * single-match command builds a seat through the same helper but runs no loop and no report.
       */
      await probe("model-seat-single-match", () =>
        invoke("play_arena_match", {
          request: {
            game: "connect_four",
            testedAgent: seat,
            opponentAgent: { kind: "policy", id: "depth_bounded_connect_four" },
            testedSide: "first",
            seed: 11,
          },
        }),
      ),
    ]);
  })()`),
);
for (const probe of probes) {
  console.log(`[${since()}] probe ${probe.label}: ${JSON.stringify(probe).slice(0, 220)}`);
}


if (settled) {
  console.log(`         ${JSON.stringify(settled).slice(0, 400)}`);
}

const progress = JSON.parse(await evaluate(`JSON.stringify(window.__batchProgress ?? [])`));
console.log(`[${since()}] progress events observed: ${progress.length}`);
for (const event of progress.slice(0, 8)) {
  console.log(`         +${event.at - started}ms ${JSON.stringify(event.payload).slice(0, 160)}`);
}

const stats = await providerStats();
console.log(`[${since()}] provider chat calls: ${stats.calls} (mode ${stats.mode})`);
for (const call of stats.log.slice(0, 8)) {
  console.log(`         call turn=${call.turn} invalid=${call.invalid} failed=${call.failed}`);
}

const archive = JSON.parse(
  await evaluate(`(async () => {
    const invoke = window.__TAURI_INTERNALS__.invoke;
    try {
      const list = await invoke("list_arena_matches", { limit: 5 });
      return JSON.stringify({ ok: true, count: list.length, first: list[0]?.matchId ?? null });
    } catch (error) {
      return JSON.stringify({ ok: false, error: String(error) });
    }
  })()`),
);
console.log(`[${since()}] archive: ${JSON.stringify(archive)}`);

console.log(`\nVERDICT:`);
console.log(`  progress events: ${progress.length}, provider calls: ${stats.calls}, promise: ${settled ? "resolved" : "unresolved"}`);
if (progress.length > 0 && !settled) {
  console.log("  -> the worker completed and archived matches; the fault is in the RETURN PATH");
} else if (progress.length === 0 && stats.calls === 0) {
  console.log("  -> nothing started; the block is BEFORE the first model call");
} else if (progress.length === 0 && stats.calls > 0) {
  console.log("  -> the provider was reached but no match completed; the block is INSIDE runner.play");
}

socket.close();
child.kill();
provider.kill();
await wait_(500);
process.exit(0);
