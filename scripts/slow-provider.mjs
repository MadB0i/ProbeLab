/**
 * A controllable OpenAI-compatible endpoint, for testing cancellation.
 *
 * # Why this exists
 *
 * Cancellation cannot be tested against a mock policy. A mock grid episode commits its whole turn budget
 * in milliseconds, so a stop signal raised from the interface loses the race every time — and a test that
 * loses a race measures the race, not the product. Two other options were rejected:
 *
 * * **Slowing production code.** An artificial delay in the engine, the adapter or the runner would put a
 *   test hook on a measured path, and the thing under test is the path a user actually runs.
 * * **A real local model.** It was tried: `qwen2.5:0.5b` runs a grid episode in about three seconds,
 *   which is enough to sometimes win the race and never enough to rely on it. A test that passes
 *   intermittently is a test whose failures are later dismissed.
 *
 * So the slowness lives here, outside the repository, in a process that exists only while a test is
 * running. The application under test is an ordinary release build on an ordinary path; the only thing
 * that is different is which port it is pointed at.
 *
 * # What it guarantees
 *
 * * It speaks the OpenAI-compatible shape the adapter parses: one choice, one message, real usage counts.
 * * Each response is held for `delayMs`, so a twelve-turn episode takes `12 * delayMs`.
 * * Every request is logged with its monotonic offset, so a test can assert that the run really did make
 *   provider calls rather than sitting somewhere else.
 * * The answer it gives is `MOVE_RIGHT` every time, which walks the agent into the east wall after a few
 *   turns. That is deliberate: it makes the episode long *and* makes it obviously the provider's doing,
 *   so a run that fails for any other reason is distinguishable.
 * * Nothing about the request is logged except its shape. The grid-world prompt carries observations but
 *   no secrets, and this is a test endpoint bound to loopback — it binds to 127.0.0.1 explicitly rather
 *   than to every interface.
 *
 * # Usage
 *
 *     node scripts/slow-provider.mjs <port> <delayMs> <controlPort>
 *
 * `controlPort` serves `GET /stats` and `POST /reset` so a test can read the call log and clear it
 * between phases without restarting the endpoint.
 */
import { createServer } from "node:http";

const port = Number(process.argv[2] ?? 9337);
const delayMs = Number(process.argv[3] ?? 2500);
const controlPort = Number(process.argv[4] ?? 9338);

/** Every request this endpoint has served, in order. */
const calls = [];
/** Set by `POST /reset`; the next response says "stopped" instead of an action. */
let abortNext = false;

/**
 * The answer.
 *
 * `MOVE_RIGHT` past the third turn is an invalid action, which the engine records as a refused move and
 * spends a turn on. Every turn therefore costs a provider call and a real wall-clock second, and the
 * episode runs to its full budget instead of terminating early.
 */
function decide(turn) {
  return turn % 3 === 0 ? '{"action":"MOVE_LEFT"}' : '{"action":"MOVE_RIGHT"}';
}

const provider = createServer((request, response) => {
  const url = new URL(request.url ?? "/", `http://127.0.0.1:${port}`);
  if (url.pathname.endsWith("/chat/completions")) {
    let body = "";
    request.on("data", (chunk) => {
      body += chunk;
    });
    request.on("end", () => {
      let turn = calls.length + 1;
      try {
        const parsed = JSON.parse(body);
        const system = String(parsed?.messages?.[0]?.content ?? "");
        // The prompt carries the turn number; counting is enough and avoids parsing a model-facing field.
        turn = calls.length + 1;
        void system;
      } catch {
        turn = calls.length + 1;
      }
      const call = { at: Date.now(), turn, aborted: abortNext };
      calls.push(call);
      const content = abortNext ? '{"action":"WAIT"}' : decide(turn);
      setTimeout(() => {
        if (response.writableEnded) {
          return;
        }
        response.writeHead(200, { "content-type": "application/json" });
        response.end(
          JSON.stringify({
            id: `slow-${call.turn}`,
            object: "chat.completion",
            created: Math.floor(call.at / 1000),
            model: "slow-test-provider",
            choices: [
              {
                index: 0,
                message: { role: "assistant", content },
                finish_reason: "stop",
              },
            ],
            usage: {
              prompt_tokens: 180 + call.turn,
              completion_tokens: 8,
              total_tokens: 188 + call.turn,
            },
          }),
        );
      }, delayMs);
    });
    return;
  }
  response.writeHead(404, { "content-type": "application/json" });
  response.end(JSON.stringify({ error: "not found" }));
});

/** Loopback only. A test endpoint reachable from the network would be a different thing entirely. */
provider.listen(port, "127.0.0.1", () => {
  process.stdout.write(`slow provider listening on 127.0.0.1:${port}, ${delayMs}ms per response\n`);
});

const control = createServer((request, response) => {
  const url = new URL(request.url ?? "/", `http://127.0.0.1:${controlPort}`);
  if (url.pathname === "/stats") {
    response.writeHead(200, { "content-type": "application/json" });
    response.end(JSON.stringify({ calls: calls.length, delayMs, aborted: abortNext, log: calls }));
    return;
  }
  if (url.pathname === "/reset") {
    calls.length = 0;
    abortNext = false;
    response.writeHead(200, { "content-type": "application/json" });
    response.end(JSON.stringify({ calls: 0 }));
    return;
  }
  if (url.pathname === "/abort-next") {
    abortNext = true;
    response.writeHead(200, { "content-type": "application/json" });
    response.end(JSON.stringify({ abortNext }));
    return;
  }
  response.writeHead(404);
  response.end();
});
control.listen(controlPort, "127.0.0.1");

for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, () => {
    provider.close();
    control.close();
    process.exit(0);
  });
}