/**
 * A controlled OpenAI-compatible model server, for driving the real product end to end.
 *
 * # What this is for
 *
 * Discovery is hardwired to Ollama's loopback port and reads `GET /v1/models`, so a journey that needs a
 * *known* model name has to answer on that port. This server does, and it answers exactly what the
 * product's discovery command reads: an OpenAI-compatible `data[].id` list.
 *
 * It binds to 127.0.0.1 explicitly. A test endpoint reachable from the network would be a different thing
 * entirely, and the audit's provider-restriction requirement is that a loopback stand-in stays loopback.
 *
 * # What it guarantees
 *
 * * `GET /v1/models` returns exactly the configured model name, so "the product discovered the model" is
 *   a fact about the product's discovery path rather than about whatever happens to be running locally.
 * * `POST /v1/chat/completions` returns one choice, one message, and real usage counts, in the shape the
 *   adapter parses.
 * * Every response is held for `delayMs`, so a match takes measurable wall-clock time and a cancellation
 *   test has a real window to act in.
 * * Every request is logged with its offset, so a test can assert the run genuinely called the provider
 *   rather than sitting elsewhere.
 * * The move it returns is always legal for the arena's action vocabulary, so a match that produces no
 *   plies is attributable to something other than the provider refusing to play.
 *
 * # Usage
 *
 *     node scripts/controlled-model-server.mjs <port> <modelName> [delayMs] [controlPort]
 *
 * `controlPort` serves `GET /stats` and `POST /reset` so a test can read the call log between phases
 * without restarting the server.
 */
import { createServer } from "node:http";

const port = Number(process.argv[2] ?? 11434);
const model = process.argv[3] ?? "controlled-test-model";
const delayMs = Number(process.argv[4] ?? 40);
const controlPort = Number(process.argv[5] ?? port + 1);

/** Every chat-completions request this server has served, in order. */
const calls = [];
/** Set by `POST /abort-next`; the next answer is an invalid action rather than a legal one. */
let answerNextInvalidly = false;

/**
 * Sustained behaviour mode, set through the control surface.
 *
 * `alwaysInvalidly` exists for audit finding B6. The one-shot switch could only spoil a single request,
 * which produces one forfeited match — enough to show the path exists but not enough to show what a
 * *population* of forfeits reports. A model that forfeits every match is the case that was invisible
 * before the fix: five `NO RESULT` rows, a win rate over an empty denominator, and no sign anywhere
 * that the seat had been measured five times and lost five times.
 *
 * `failEveryCall` is the opposite case and equally necessary: the provider cannot deliver an answer at
 * all, so the model is never reached. Those matches must not become forfeits.
 *
 *   alwaysInvalidly — answer with text that is not a legal move, every time.
 *   failEveryCall   — answer with a 503, every time (a provider that is up but not serving).
 */
let mode = "valid";

/**
 * The legal arena action.
 *
 * The arena's protocol names moves by their position, one-based: `COLUMN_1` for the first column of
 * connect four, `CELL_1` for the first cell of tic-tac-toe. Two earlier attempts got this wrong in ways
 * worth recording, because both produced a `NO RESULT` with zero plies that a weaker check would have
 * reported as a pass:
 *
 * * `MOVE_LEFT` is a grid-world action name and is not an arena move at all.
 * * `{"move":"0"}` is a bare index. The adapter does accept a bare number, but only as a fallback
 *   matched against the *currently legal* move list, and `0` is not one-based so it never matched.
 *
 * **The move is chosen from the request's own `legal_moves`.** A fixed column eventually fills: a
 * six-row board holds six discs, so always answering column 1 guarantees a refused move part-way through
 * and the match ends as `NO RESULT` rather than as a played game.
 *
 * The parsing is deliberately forgiving because the request is a JSON *payload inside a chat message*:
 * the outer body is an OpenAI chat-completions request whose `messages[].content` is itself a serialized
 * `GamePromptRequest` containing a `legal_moves` array. Regexing the whole body for `"COLUMN_n"` finds
 * them wherever they ended up, which is the only thing that works without reimplementing the prompt
 * serializer in a test double. An earlier version parsed the outer envelope and found no vocabulary at
 * all, silently fell back to a hardcoded column, and every match died on the seventh ply.
 *
 * The first legal move is chosen. For connect four the legal list is the set of columns with room, so
 * this always plays a real game to its real end.
 */
function decide(body) {
  /*
   * The invalid answer, keyed on the same condition that is logged.
   *
   * These two were once different expressions. `decide` checked only `answerNextInvalidly` while the
   * request log recorded `answerNextInvalidly || mode === "alwaysInvalidly"` — so in `alwaysInvalidly`
   * mode the server **wrote `invalid: true` to its own log on every call and answered with a perfectly
   * legal move on every call**. The test that used it asserted "every call was invalid" and passed,
   * because it read the log, while four matches played out normally and produced a 0.5 win rate.
   *
   * That is the whole failure mode this server exists to avoid: a harness whose bookkeeping disagrees
   * with its behaviour reports the behaviour it wishes for. Deriving both from one function means a
   * future mode cannot make them diverge again.
   */
  if (answerNextInvalidly || mode === "alwaysInvalidly") return '{"move":"NOT_A_MOVE"}';

  /*
   * Where the legal vocabulary actually is.
   *
   * The request is an OpenAI chat-completions envelope whose user message is itself a serialized
   * `GamePromptRequest`, so the wire body is JSON inside a JSON string. Everything therefore arrives
   * backslash-escaped: the literal text contains `\"legal_moves\":[\"COLUMN_1\",...]`, and a search for
   * `"COLUMN_1"` with real quotes finds nothing at all.
   *
   * Two earlier attempts failed differently, and both failed silently:
   *
   *  * Taking the first *bare* `COLUMN_n` anywhere in the body. That matches the system prompt's worked
   *    examples before it reaches the payload, so the stand-in always answered `COLUMN_1`, filled that
   *    column, and every match ended as `NO RESULT` on the seventh ply. It looked like the product
   *    refusing the match.
   *  * Hardcoding a column. The same outcome, for the same reason.
   *
   * So the `legal_moves` array is located *by name*, the escapes are undone, and only names inside that
   * array are candidates. This is the only version that is reading the request rather than pattern
   * matching over it.
   */
  const unescaped = String(body).replace(/\\"/g, '"').replace(/\\\\/g, "\\");
  const array = /"legal_moves"\s*:\s*\[([^\]]*)\]/.exec(unescaped);
  if (array) {
    const columns = [...array[1].matchAll(/COLUMN_(\d+)/g)].map((m) => `COLUMN_${m[1]}`);
    if (columns.length > 0) {
      return JSON.stringify({ move: columns[0] });
    }
    const cells = [...array[1].matchAll(/CELL_(\d+)/g)].map((m) => `CELL_${m[1]}`);
    if (cells.length > 0) {
      return JSON.stringify({ move: cells[0] });
    }
  }

  /*
   * No readable vocabulary. Answering a guess here produced a refused move that reads like a model
   * failure rather than a test-double failure, so it says so instead — and the journey's plies assertion
   * fails loudly, which is the correct outcome for a stand-in that cannot read its request.
   */
  return '{"move":"NO_LEGAL_MOVES_SEEN"}';
}

const server = createServer((request, response) => {
  const url = new URL(request.url ?? "/", `http://127.0.0.1:${port}`);

  /*
   * Discovery. The product reads this document and nothing else: `data[].id`, capped at 128 entries.
   * Answering it is what puts a *known* name into the seat picker.
   */
  if (url.pathname === "/v1/models" || url.pathname === "/models") {
    response.writeHead(200, { "content-type": "application/json" });
    response.end(
      JSON.stringify({
        object: "list",
        data: [{ id: model, object: "model", owned_by: "controlled-test-server" }],
      }),
    );
    return;
  }

  if (url.pathname.endsWith("/chat/completions")) {
    let body = "";
    request.on("data", (chunk) => {
      body += chunk;
    });
    request.on("end", () => {
      const turn = calls.length + 1;
      const invalid = answerNextInvalidly || mode === "alwaysInvalidly";
      const failed = mode === "failEveryCall";
      calls.push({ at: Date.now(), turn, invalid, failed, mode });
      answerNextInvalidly = false;

      /*
       * A provider that is reachable but not serving.
       *
       * 503 rather than a dropped socket, because a refused connection is a *different* failure kind in
       * this codebase and the point here is the class that is neither a model fault nor a transport
       * fault: the request arrived, the server declined, and the model was never consulted.
       */
      if (failed) {
        response.writeHead(503, { "content-type": "application/json" });
        response.end(
          JSON.stringify({
            error: { message: "model is loading, try again shortly", type: "server_error" },
          }),
        );
        return;
      }

      const content = decide(body);
      calls[calls.length - 1].answered = content;
      setTimeout(() => {
        if (response.writableEnded) {
          return;
        }
        response.writeHead(200, { "content-type": "application/json" });
        response.end(
          JSON.stringify({
            id: `controlled-${turn}`,
            object: "chat.completion",
            created: Math.floor(Date.now() / 1000),
            model,
            choices: [
              { index: 0, message: { role: "assistant", content }, finish_reason: "stop" },
            ],
            usage: {
              prompt_tokens: 180 + turn,
              completion_tokens: 8,
              total_tokens: 188 + turn,
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

server.listen(port, "127.0.0.1", () => {
  process.stdout.write(
    `controlled model server on 127.0.0.1:${port}, model "${model}", ${delayMs}ms per response\n`,
  );
});

const control = createServer((request, response) => {
  const url = new URL(request.url ?? "/", `http://127.0.0.1:${controlPort}`);
  if (url.pathname === "/stats") {
    response.writeHead(200, { "content-type": "application/json" });
    response.end(JSON.stringify({ calls: calls.length, delayMs, mode, log: calls }));
    return;
  }
  if (url.pathname === "/reset") {
    calls.length = 0;
    answerNextInvalidly = false;
    mode = "valid";
    response.writeHead(200, { "content-type": "application/json" });
    response.end(JSON.stringify({ calls: 0 }));
    return;
  }
  if (url.pathname === "/answer-next-invalidly") {
    answerNextInvalidly = true;
    response.writeHead(200, { "content-type": "application/json" });
    response.end(JSON.stringify({ answerNextInvalidly: true }));
    return;
  }
  if (url.pathname === "/mode") {
    const requested = (url.searchParams.get("as") ?? "valid").trim();
    if (!["valid", "alwaysInvalidly", "failEveryCall"].includes(requested)) {
      response.writeHead(400, { "content-type": "application/json" });
      response.end(
        JSON.stringify({ error: `unknown mode ${requested}`, allowed: ["valid", "alwaysInvalidly", "failEveryCall"] }),
      );
      return;
    }
    mode = requested;
    response.writeHead(200, { "content-type": "application/json" });
    response.end(JSON.stringify({ mode }));
    return;
  }
  response.writeHead(404, { "content-type": "application/json" });
  response.end(JSON.stringify({ error: "not found" }));
});

control.listen(controlPort, "127.0.0.1", () => {
  process.stdout.write(`control surface on 127.0.0.1:${controlPort}\n`);
});