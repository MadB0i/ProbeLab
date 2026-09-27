# PROBE//LAB

A controlled environment where AI systems are placed into measurable tasks and
security-sensitive environments, their actions are independently observed, authorization is
enforced separately from evaluation, and the resulting behaviour can be inspected, replayed, and
compared.

That sentence is the design. Everything below it exists to make one of its clauses true in code.

## What it is

PROBE//LAB is a desktop laboratory for evaluating AI agents. It runs a chosen agent through
deterministic benchmarks, watches what it tries to do, enforces a capability boundary
*independently* of the evaluation, and records enough to reconstruct the run afterwards.

The separation is the point. An agent cannot influence whether it is allowed to do something; it
can only ask. Whether it is allowed is decided by a broker that the agent has no handle to. Whether
it succeeded is decided by an evaluator that trusts nothing the agent says about itself. And
whether it *could* have done something is decided by a sandbox the agent cannot reach.

## Architecture

```
Brain            plans, monitors, analyses. Holds no execution or authorization capability.
  │
ExperimentManager  owns the lifecycle of one run. Cleanup is in `Drop`, so every terminal state
  │                releases resources -- including on an early return or a panic.
Runner           the courier. Executes episodes; authorizes nothing.
  │
Engine           benchmark execution logic: request construction, validation, turn loop.
  │
Broker           THE ONLY capability authorization authority. Reviews every request against an
  │              explicit policy. Says yes or no.
Sandbox          THE ONLY enforcement authority. AppContainer containment, designated writable
  │              directory, protected paths, job limits, low integrity.
Observer         independent telemetry. Witnesses; does not interpret.
Evaluator        THE ONLY source of truth for results. Never accepts model-reported outcomes.
```

A component may not become a second authorization or enforcement authority. This is enforced
structurally, not by convention: the runner holds no broker handle and no policy object at all, so
it is not *able* to authorize. The arena, the Brain panel, the Security Lab, and the replay
subsystem are all asserted to contain no broker, policy, capability, or process-launch
vocabulary — checked on every build, not at review time.

## Running it

```bash
# Everything, in one go.
cargo fmt --check
cargo test --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo build --workspace --all-features

# The desktop application.
cd apps/desktop
pnpm install
pnpm test
pnpm build
pnpm tauri dev
```

CI runs all of the above, plus a named step per structural security guarantee, and refuses to
pass if the [feature matrix](docs/feature-matrix.json) disagrees with the repository.

## What is actually implemented

The honest list. Anything not here does not exist, and the UI does not offer it.
[docs/STATUS.md](docs/STATUS.md) is the authoritative statement of state; this section is the
summary, and where they differ STATUS.md is right.

### The command center

Nine destinations in a persistent sidebar, hash-routed and deep-linkable. Each one answers a single
question instead of stacking all of them into one long page:

| Destination | What it shows |
|---|---|
| Command center | The orchestrator's state, the experiment it is supervising, the launch controls, and the live run's event stream. |
| Agents | Every agent from the authoritative registry, with what each one's number measures. |
| Benchmarks | Every registered benchmark, and the selected one. |
| Comparison | Two or more stored experiments, with a refusal and its reason when they cannot be compared. |
| Settings | The defaults future experiments and runs start from, stored and validated by the backend. |
| Providers | The provider protocols the lab speaks. Credentials are read from the environment by the backend and never reach this page. |
| Arena | Real matches through the engine. The board shown is the position the engine recorded. |
| Security | Registered probes run through the real broker and sandbox path. |
| Replay | Deterministic reconstruction of recorded runs. |

The active destination is marked by colour, a left bar, and `aria-current`, so it is not signalled by
colour alone. The nav is a real button list in a labelled landmark: `Tab` leaves it, the arrow keys
and `Home`/`End` move within it, and focus follows the selection. There is no routing dependency, and
no nav item that leads nowhere — a disabled entry would be a claim the product cannot verify. That
principle is checked, not asserted: `every_command_the_frontend_invokes_is_registered` compares the
names `api.ts` really invokes against the names `generate_handler![]` really dispatches, in both
directions. Nine commands were once defined but never registered, which left three panels dead in the
built application while this file described them as working.

### Benchmarks

| Benchmark | Status | Notes |
|---|---|---|
| Grid World | Implemented | Partially observable planning on a replayable seed. |
| Sandbox Process | Implemented | A real contained process, on Windows only. `filesystem.*` and `network.connect` are **authorized and refused at execution time** — not executed. `environment.inspect` is **denied at the broker and has no grant path at all**, so it is never authorized. Only `process.spawn` has a real executor. |

### Arena

| Game | Status |
|---|---|
| Tic-Tac-Toe | Implemented, with a perfect minimax ceiling and a first-legal floor. |
| Connect Four | Schema only. Not offered. |
| Rock Paper Scissors | Schema only. Not offered. |
| Battleship | Schema only. Not offered. |

A game appears in the UI only when it actually plays. Per-ply board history comes from the
engine's own telemetry, so the board shown is a recording — the frontend holds no game state.

There is no human seat, and none was invented. `play_match` is atomic: a match runs agent
against agent to completion, so there is never a turn for a user to take.

### Providers

Three adapters exist. **One is selectable in the application.**

| Provider | Protocol | Selectable in the app |
|---|---|---|
| OpenAI-compatible | Chat Completions. | **yes** |
| Anthropic | Messages API. Multi-block content joined, so a reply split across blocks is not misreported as unparseable. | no — built and tested as a library, not wired into the IPC layer |
| Gemini | `generateContent`. The model name is constrained, because it goes into a URL path. | no — same |

The OpenAI-compatible transport reaches any server exposing `/v1/chat/completions`, which is a
compatibility statement about the protocol rather than a tested claim: no such server is part of CI.
There is no native Ollama adapter and no OpenCode provider, and the reasons are recorded in STATUS.md
§2.2.

Deliberately absent: a native Ollama adapter (the verified OpenAI-compatible path already reaches
it — a second adapter would be a second thing to test for no new capability), and an OpenCode
provider (a coding agent, not a model endpoint; there is no API to point at).

Keys are read from the environment at call time, never stored, never sent to the frontend, and
redacted from anything that could reach a log or a result bundle.

### Security probes

Six, all pre-existing in the agent registry and all executed through the real broker and sandbox
path: evaluator-access, policy-violation, sandbox-violation, environment-tampering, network, and
forbidden-action-spam.

The Security Lab reports **which layer acted**, because the distinction is the substance:

- the policy gate recorded an attempt and denied it → the boundary held;
- the broker refused a capability request → enforcement working;
- neither → nothing was tested, which is *not* a pass.

A security probe is a **test of the boundary**, not a subject under evaluation. An attempt it
provokes is evidence the enforcement layers work. The same recorded event is a genuine finding when
a model under evaluation produces it, which is why the Brain panel reports it as a finding and the
Security Lab does not. There is no combined "security events" number, because such a number rises
as the sandbox works correctly.

## Replay

Replay reconstructs recorded data. It does not re-run a match, re-query a model, or contact any
provider. It holds no engine, no broker, and no process launcher, so it cannot execute or authorize
anything — asserted structurally.

- A recording that captured every intermediate board is **re-executable**: you can step it.
- A recording that captured only an event stream is **inspection only**: you can read what was
  reported and when. The intermediate states were never recorded and are not reconstructed.

Where a recording is incomplete, the gap is named rather than filled. Interpolation would be
fabricating history.

## Reproducibility

Every completed run records a benchmark identity and version, an agent identity, a seed, a
configuration fingerprint, a prompt-template version, a replay digest, and a token count where the
provider supplied one.

Distinguish two things that are easy to conflate:

- **Deterministic replay** — reconstructing a recorded run. Always reproduces.
- **Rerunning an external model** — a new measurement. An external model's output is not
  guaranteed to repeat, and PROBE//LAB does not claim it is. Its digests are recorded so a
  difference is visible, not so it can be waved away.

**No software version or commit is recorded in a result bundle**, and the benchmark "version" is a
hand-bumped string rather than a digest of the definition file. Two runs from different builds of
this repository therefore cannot be told apart from their stored results alone. This is the largest
gap in the section, and it is listed in STATUS.md §6.3 with the rest.

## Limitations

Stated plainly, because a laboratory that hides its limits is not one you can rely on. The complete,
numbered list with evidence is in **[docs/STATUS.md](docs/STATUS.md) §8**.

- **AppContainer containment is not complete OS isolation.** It restricts the token and package
  access. It says nothing about every kernel surface, and no test claims otherwise.
- **`filesystem.read`, `filesystem.write`, and `network.connect` are authorization-only.** The broker
  authorizes and the execution is refused at run time. Only `process.spawn` really executes.
  `environment.inspect` is a fourth case: the broker denies it unconditionally and it has no grant
  path, so it is never authorized at all.
- **Containment is Windows-only.** Other platforms fail closed rather than running unconfined, so
  they can measure nothing involving a process.
- **An interrupted experiment is not resumed.** A crash mid-run transitions the stored lifecycle to
  `interrupted` on the next start, and the partial episodes are preserved as evidence. They are not a
  complete measurement, and they are not re-run: the sandbox root, process identity, and provider
  session they depended on no longer exist, so appending a second measurement to the first would be
  worse than an honest gap.
- **The engine's `Harness::measure` is not the loop the UI runs.** The product path shares the
  Harness's evaluation seed pools and Wilson interval, and keeps the per-episode failure handling the
  Harness lacks, because a real run must survive a failed episode. So the *measurement method* is on
  the product's path; that particular function is not.
- **No connected model has been measured on any benchmark.** The reference policy is a scripted
  searcher. Beating it is a low bar, and a pass would be a precondition for an interesting result
  rather than a result worth reporting.
- **External model nondeterminism** is not reproducible, by anyone.
- **Settings cannot move stored data or hold a credential.** The storage paths, the credential policy,
  log verbosity, and platform containment support are shown read-only with the reason they are fixed.
  Sampling temperature and completion ceiling are not configurable at all, because fixing them is what
  makes two runs comparable. There is no field anywhere in the settings document that could hold an
  API key — the key is read from the process environment at run time.
- **A settings change affects future runs, not recorded ones.** A result already stored is immutable
  and nothing on the settings page can reach it; reset replaces one row in the settings store, which is
  a separate file from the results database.
- **No human seat in the arena**, and no LLM-powered Brain. The Brain is deterministic
  orchestration — it has no model behind it and does not pretend to.

See [docs/feature-matrix.json](docs/feature-matrix.json) for the machine-readable version, which CI
checks against the repository, and [docs/STATUS.md](docs/STATUS.md) for what is verified.

## Further reading

- **[docs/STATUS.md](docs/STATUS.md)** — what is implemented, what is verified, and what is not. Authoritative.
- [ARCHITECTURE.md](ARCHITECTURE.md) — the component contracts and why each one is where it is
- [SECURITY.md](SECURITY.md) — trust boundaries, containment, and the integrity taxonomy
- [docs/feature-matrix.json](docs/feature-matrix.json) — what is implemented, verified by CI
- [docs/SECURITY-STATUS.md](docs/SECURITY-STATUS.md) — the implementation record for each boundary
- [PROGRESS.md](PROGRESS.md) — the milestone history. A design record, not a statement of state.
- [docs/AUDIT-2026-09-26.md](docs/AUDIT-2026-09-26.md) — a historical audit, pinned to the commit it
  examined. Superseded on most points; retained because the reasoning behind several decisions is
  recorded nowhere else.

