# ProbeLab architecture

*Describes workspace version 0.4.1. Sections are labelled with the milestone that introduced them; the
highest is **v0.5 (benchmark design)**. The security sections describe v0.4.1 and are unchanged by
v0.5, which touched no containment, broker, or telemetry internals.*

## Trust boundary

The runtime data flow is:

`MODEL → AGENT/ADAPTER → CAPABILITY BROKER → CAPABILITY EXECUTOR → CONTAINMENT → TEST WORLD → OBSERVER → EVALUATOR`

A model receives only a `ModelRequest` containing its current turn and a local `Observation`. It never receives benchmark definitions, the complete map, evaluator logic, expected routes, SQLite handles, filesystem paths, or security telemetry. `Action` has no filesystem, process, network, or evaluator variant, and no provider prompt or response schema exposes a raw OS process API.

Three layers refuse or perform capabilities, and they are independent of each other. The **capability broker** is the primary boundary and decides whether a `CapabilityRequest` is authorized at all. The **capability executor** performs an already-authorized capability, and only `process.spawn` has an implementation. The **policy gate** is the v0.1/v0.2 boundary over the modeled `RequestedCapability` probe values used by the deterministic security probes. A broker denial never increments a policy-gate integrity category, and a policy denial never increments a broker category.

A connected provider is trusted infrastructure code, not a model capability. The adapter may make outbound HTTP to its configured API endpoint; the model's action space still has zero network access. A denied request consumes a trusted turn-budget transition without an action outcome or world movement.

## Crate boundaries

- `core`: serializable domain values and explicit sync/async traits; no game or persistence implementation.
- `agent`: synchronous `ProviderAgent` and `AsyncProviderAgent` blocking bridge. The bridge owns a dedicated Tokio runtime and never exposes provider internals to the model.
- `model-adapters`: deterministic mocks, strict action protocol parser, prompt asset, and OpenAI-compatible async adapter.
- `broker`: the deny-by-default `CapabilityBroker` and its `CapabilityPolicy`. This is the primary capability boundary; it depends only on `core`, never on the engine or the UI.
- `sandbox`: seeded grid generation, transition rules, deny-by-default policy gate, runtime guardian, the interface-restricted sandbox adapter, and the Windows OS-level containment layer.
- `telemetry`: independent observer, semantic replay projection, append-only SQLite recorder, and immutable result/metadata repository.
- `evaluator`: independently converts trusted environment outcomes, protocol adherence, and integrity events into results.
- `benchmarks`: versioned JSON definitions and benchmark registry.
- `engine`: synchronous deterministic orchestration without UI concerns. The evaluator is injected, so adding a benchmark does not require rewriting the engine.
- `apps/desktop/src-tauri`: Tauri IPC, background run lifecycle, provider preflight, live event delivery, and SQLite ownership only.

## Provider boundary

`AsyncModelProvider` is object-safe and returns a boxed `Send` future. The engine accepts an ordinary `Box<dyn Agent>` and does not depend on a provider implementation. `AsyncProviderAgent` bridges the async trait to the synchronous engine using a dedicated current-thread Tokio runtime and a short-lived worker thread. Tauri runs the engine and its SQLite persistence on `spawn_blocking`.

The OpenAI-compatible adapter:

- accepts a base URL or complete `/chat/completions` URL;
- requires HTTPS except loopback HTTP for local providers;
- rejects URL userinfo, query strings, fragments, redirects, and embedded credentials;
- reads the API key only from a named environment variable;
- sends only the versioned system prompt and serialized `ModelRequest`;
- performs one attempt with no automatic retries;
- returns typed, sanitized failures for timeout, transport, HTTP, empty, malformed, and invalid-action responses;
- records provider/model/prompt/usage/timing metadata without secrets.

The strict protocol accepts exactly one `{ "action": "..." }` object. Markdown fences and surrounding prose are tolerated, action names are case-insensitive, and unknown fields, multiple JSON values, unknown actions, and out-of-vocabulary actions are parse failures. Protocol failures are capability signals, not integrity events.

## Reproducibility and results

Every completed run returns `CompletedRun { result, metadata }`. `TestResult` contains the trusted capability/integrity result and safe reproducibility metadata. `RunMetadata` is stored separately in the same SQLite transaction and contains provider/model, prompt version, non-secret configuration fingerprint, environment version, start/end time, token usage, and the ordered action trace.

The configuration fingerprint hashes six fields: model name, temperature, max tokens, prompt template version, benchmark version, and the declared request timeout. API keys, environment-variable names, endpoints, headers, raw request bodies, and response bodies are excluded.

The replay digest hashes a semantic projection of observations, decisions, transitions, rewards, integrity events, and protocol-failure kinds. It excludes timestamps, run IDs, response text, request IDs, elapsed time, and token usage. Mock runs with the same seed and strategy remain field-identical; remote-provider runs are not assumed reproducible.

The protocol-failure threshold is `PROTOCOL_FAILURE_THRESHOLD = 3` consecutive failures. Protocol failures populate `CapabilityResult.protocol_adherence`; `IntegrityResult` remains zero unless the policy gate denies a modeled capability.

## Benchmark definitions

Benchmark files use JSON because the workspace already depends on `serde_json` for telemetry and IPC. Definitions are versioned, embedded in the Rust binary, and validated before execution. The model prompt is a separate versioned artifact under `prompts/`; prompt changes require a version bump rather than silent in-place edits.

## v0.5 benchmark design: making the measurement mean something

`docs/AUDIT-2026-09-26.md` found that the grid-world benchmark did not measure reasoning. It was a
5×5 branching-free corridor with a goal pinned to a compile-time corner, a start pinned to the
opposite corner, a fixed 30-turn budget for an 8-move optimum, and a reward function that could only
return `{0, +10, −10}`. A three-comparison one-step heuristic solved it on every seed, and a "run" was
one episode on one hardcoded seed with no baseline and no interval. Six changes fix that, and each is
worth stating with its reason rather than its mechanics.

**Start and goal are drawn per seed.** This is the change that matters most, and it is not the obvious
one. With the start pinned at a corner, every other tile lies down-and-to-the-right of it, so
"prefer DOWN, then RIGHT" is not a heuristic — it is an exact distance descent. Measured on an
intermediate version of this generator that still had a fixed start, the greedy baseline scored **60 of
70** evaluation seeds *for that reason alone*. Drawing the start per seed means the goal is as often
up-and-to-the-left as down-and-to-the-right, so no fixed direction is correct.

**The safe region branches.** A monotone spine guarantees a route, and a seeded dead-end spur is hung
off every eligible spine cell, so the safe tiles form a tree with real forks rather than a path. At a
fork, both branches look identical and only one leads anywhere. Branch *hazards* are placed only on
spine cells off the shortest route, never on a spur: an earlier version picked from every safe cell
and routinely dropped a hazard onto a spur tip, deleting the very branch it was meant to guard and
quietly returning the seed to a corridor.

**The turn budget is derived per seed.** A seed's budget is `ceil(3/2 × its own shortest hazard-free
route)`, computed deterministically from the seed. The catalog's `max_turns` is an **upper bound over
every seed**, not any seed's budget, and the definition is refused at load time if any admitted seed
needs more. The 1.5 factor is the calibration: it affords one wrong turn and a correction, and nothing
more. Under the old fixed 30-turn budget an agent could waste 22 turns wandering and still win, which
is exactly why a search policy and a coin flip were indistinguishable.

**Observation is partial, and memory is therefore necessary.** The agent sees its own tile, its four
adjacent tiles, and `distance_to_goal` — a scalar, never a direction. A hazard two or more steps away
is invisible and does not appear at all; once the agent walks away from a hazard, it leaves the payload
again while remaining a real hazard. Nothing in the observation reminds the agent, so remembering it is
the agent's job. Disclosing a scalar distance is strictly *less* than what v1 leaked, where the goal
position was a compile-time constant an agent could hardcode.

**`ModelRequest` carries a bounded history.** Up to `HISTORY_WINDOW = 8` completed prior turns, each
carrying only what the agent legitimately observed when it decided: position, visible neighbours, the
action chosen, the result, and the reward. The bound is part of the token-cost contract — a request's
size is capped regardless of episode length — and it is enforced as a hard limit in
`ModelRequest::validate`, so a caller cannot believe it supplied more context than the model received.
Eight covers the planning horizon the generator produces: the longest shortest safe route fits inside
the window with room to spare. A turn the engine consumed without a model decision — a broker refusal
or a protocol failure — leaves no history entry, so window turns need not be contiguous.

**Reward is shaped, while completion status stays categorical.** Each move pays `+1` per tile of
Manhattan distance reduced toward the goal and `−1` per tile increased, with `+10` on reaching the
goal and `−10` on a hazard. `CompletionStatus` is unchanged and remains the pass/fail signal; the
shaped total is what distinguishes two runs that both end in `turn_limit`. Under the old terminal-only
score a run that walked 22 correct steps and timed out scored exactly the same as one that emitted
`WAIT` thirty times.

### The measurement harness is library and CLI only

`crates/engine/src/harness.rs` runs N episodes over a seed pool and reports a scalar with a Wilson
score interval. Two pools are declared in the benchmark definition — 40 dev seeds and 70 eval seeds —
and **their disjointness is validated when the definition loads** and again in the harness
constructor, not merely asserted by convention. A single shared seed would mean every "held out" number
had already been tuned against, and nothing downstream could detect it.

The interval is Wilson rather than the normal approximation because at 30–70 episodes with rates near 0
or 1 for a weak policy, the normal approximation is at its worst exactly where it matters: it collapses
to zero width at 0 % and runs outside `[0, 1]` at 100 %. Wilson stays inside the unit interval and keeps
sensible width at both ends.

**Scope, stated plainly: the desktop app does not use this.** `run_benchmark` in the Tauri layer still
runs exactly one episode against one seed. The UI shows no success rate, no confidence interval, and no
baseline. `crates/engine/examples/grid_world_harness.rs` is the command-line surface
(`cargo run -p probelab-engine --example grid_world_harness -- --episodes 70`), and the test suite is
the other consumer. Wiring multi-episode aggregates into the desktop app is follow-on UI work, and the
existence of this module should not be read as the app having gained a statistics panel.

### What the greedy-beats-search proof does and does not show

`crates/engine/tests/benchmark_quality.rs::b6_greedy_is_beaten_by_a_search_policy_with_disjoint_intervals`
measures a scripted one-step-lookahead baseline and a scripted search-based reference solver over the
70 evaluation seeds and asserts their Wilson intervals do not overlap. Measured on this build:

| Policy | Successes | Rate | 95 % CI | Mean reward | Mean turns |
| --- | --- | --- | --- | --- | --- |
| random (uniform valid moves) | 0 / 70 | 0.000 | [0.000, 0.052] | −2.79 | 7.60 |
| greedy (one-step lookahead) | 20 / 70 | 0.286 | [0.193, 0.401] | 4.93 | 7.59 |
| search (reference solver) | 66 / 70 | 0.943 | [0.862, 0.978] | 14.60 | 5.91 |

**What that shows:** the instrument has a measurable range. A trivial heuristic is beatable, and the
gap is large enough that it is not sampling noise. Whatever a real model scores on this benchmark is
therefore a statement about something the greedy baseline does not do.

**What it does not show:** that any real model reasons well. The reference solver is a scripted search
over the agent's own accumulated evidence — it is given no layout, no goal position, and no seed, and
it learns the map only from tiles it has seen. Beating a scripted searcher is a low bar for a competent
model, and passing this benchmark is a *precondition* for a model being interesting here, not a
result. No connected model has been run against the v2 benchmark yet; that is the next step.

A useful negative result from building this: the branches and the hidden hazards are *not* what
establish the gap. An intermediate generator with a per-seed start but a branch-free safe region still
showed a wide gap (greedy 0.357, search 1.000). Reverting to the full v1 design — corner start,
corner goal, fixed 30-turn budget — collapsed it to greedy 0.971 against search 1.000, with overlapping
intervals, and the proof test failed as it should. The load-bearing changes are the per-seed start and
the tight per-seed budget; the branching adds difficulty on top.

## v0.5 phase 0: is the bounded history window actually load-bearing?

**Answer: no, and the reason is structural rather than a bug.** The v0.5 measurement harness in
`crates/engine/tests/phase0_memory.rs` measures it rather than assuming it.

Two reactive policies were added, and they are deliberately the *same policy*. Identical move rules,
identical tie-breaks, identical information from the current observation. The only difference is that
`MemoryReactive` reads `ModelRequest::history` and `StatelessReactive` has no way to. Their
descriptors are compared key-by-key to prove nothing else differs, so the gap between their scores is
the value of the window with nothing confounded.

Measured over all 70 evaluation seeds:

| Policy | Successes | Rate | 95 % CI | Mean reward | Mean turns |
| --- | --- | --- | --- | --- | --- |
| stateless reactive | 9 / 70 | 0.129 | [0.069, 0.227] | 2.76 | 7.90 |
| memory reactive | 9 / 70 | 0.129 | [0.069, 0.227] | 2.76 | 7.90 |

Identical, down to the mean reward and the terminal-status breakdown. Stronger than the aggregate
tie: `p0_the_two_reactive_policies_take_identical_action_sequences` replays both policies on every
evaluation seed with a real reconstructed history window and asserts they choose the **same action on
every turn of every seed**. They are not merely tied; they are behaviourally identical.

### Why, proved rather than guessed

Three tests establish the mechanism, and each would fail if the explanation stopped being true.

**The window is delivered.** `p0_the_history_window_is_delivered_to_the_model` drives a real engine
run and records the history length the model receives on each turn: zero on turn one, non-empty
afterwards, never above the cap. Without this control, "memory made no difference" would be
unfalsifiable — an empty window would produce the same null result.

**The observation radius equals the step size.** `p0_the_observation_radius_equals_the_step_size`
walks every evaluation seed and asserts, for every cell: at Manhattan distance ≥ 2 it is *never* in
the observation, and at distance 1 it is *always* in the observation. That is the whole explanation. A
memory window can only pay off when a policy must act on information that is no longer in front of it.
Here the cell a policy is about to step onto was adjacent on the turn it chose the move, so it was
told what that cell is. A remembered hazard is never the binding constraint, because the visible hazard
already is.

**A seen goal can never outlive its own turn.** `p0_a_seen_goal_can_never_survive_its_own_turn` proves
the most obvious route to memory paying off is closed. Both reactive policies step onto an adjacent
reward the moment they see one, so a goal can never end up "remembered but not yet adjacent" — the
state in which remembering where you saw the goal would be worth something. The episode ends on the
turn the goal is first seen.

The search solver's knowledge scope was verified before any of this was built on top of it.
`p0_the_search_solver_decisions_depend_only_on_the_request_stream` replays one fixed request stream
against two `LookaheadSolver`s constructed with different seeds over different layouts and requires
identical decisions: if any part of the hidden layout reached the solver through the seed, the two
would diverge.

### What would make memory load-bearing, and why it was not done here

The three structural facts above point at the same lever: **the observation radius must exceed the step
size.** If the agent could see two or three tiles out, a hazard or a dead end would be visible before
it became the cell underfoot, and the window would carry real information across the gap. The
alternatives — a changing environment where a tile stops being safe once you leave it, or hazards that
move — also work. All three are genuine benchmark design changes that alter what the grid world
measures.

None of them were made. Phase 0's brief was to establish whether memory is load-bearing and to report
the result honestly **rather than forcing a fix**, and the honest result is that it is not, today, for
this environment and this policy class. Changing the generator to manufacture a result would have made
the benchmark measure something else while appearing to satisfy the brief. The finding is recorded
instead, pinned by a test that will fail if a future change makes memory matter, and the lever is
documented for whoever picks up the design next.

One incidental finding: the `HISTORY_WINDOW` added in v0.5 is consumed by **no** shipped policy. The
search solver accumulates its memory through `&mut self` across `predict` calls rather than from
`request.history`, because an in-process agent does not need the field. The window exists for a
connected model, which gets a fresh request each turn and has no other memory. That is a real and
intended use, but it means the window's *usefulness* is currently asserted only for real providers —
which is one more reason no connected model has been measured on this benchmark yet.

## v0.5 phase 1: AppContainer identity and network isolation

Before this phase a contained child ran as the *invoking user* under a job object, which bounded CPU,
memory, process count, and wall clock, and nothing else. It could read every file that user could read
and it could open network connections. The job object is a resource boundary; it is not an identity.

The phase adds the missing half. Every contained child is now launched with a derived AppContainer
identity, which bounds file access by *package identity* rather than by group membership, and which the
OS refuses outbound network access for unless a capability is granted. Both properties come from one
mechanism, which is why they are one phase.

The design decisions, and the two that did not survive contact with the machine:

- **Derive, do not register.** `CreateAppContainerProfile` fails on this host with
  `ERROR_INVALID_NAME`. `DeriveAppContainerSidFromAppContainerName` computes the same package SID
  arithmetically from the profile name and never touches registration, so it is unaffected. One name
  for the whole installation is deliberate: the package SID is what an ACE must name, so a per-run
  profile would mean rewriting run-directory ACLs with a SID that changes every run.
- **Verify the identity, do not assume it.** The child starts suspended, its token is opened,
  `TokenAppContainerSid` is read back, and only a match against the derived SID allows it to resume. A
  mismatch kills it. A containment mechanism that is never checked is a comment, not a boundary.
- **No opt-out.** `ContainedProcessSpawner` builds the AppContainer request itself and exposes no method
  to skip it, and a non-Windows build fails closed with `AppContainerUnavailable` instead of quietly
  launching uncontained.
- **`TEMP`/`TMP` could not be aimed at the designated directory.** The plan was to point them into the
  run's own directory. Measured, Windows rewrites them to the package's per-user folder and discards
  what the parent set. Forcing them would have been a guarantee in the docs and nothing in the machine,
  so the spawner supplies only what is genuinely required — `LOCALAPPDATA`, without which the launch
  fails with `ERROR_ENVVAR_NOT_FOUND` — and the test asserts the value the child really sees.
- **Explicit capabilities are real, and the limit is stated.** A capability is *parsed* as the SID it
  already is (`ConvertStringSidToSidW`), not derived as a package SID from its name — deriving would
  turn the well-known `internetClient` capability into an unrelated `S-1-15-2-*` package SID that grants
  nothing while looking granted. The parsed SIDs are passed through `SECURITY_CAPABILITIES` for real,
  and a grant that is requested but not delivered is refused rather than dropped. Two honest limits:
  the measured network refusal is **loopback** (AppContainer gates loopback separately, via
  `CheckNetIsolation`, not via a capability), and a *granted* capability is not proven end-to-end,
  because the child cannot report its own token from inside the container. Both are recorded in
  `docs/SECURITY-STATUS.md` rather than glossed.

The sharp remaining limit: this is **refusal, not filtering.** It is not a syscall filter, so it bounds
what the OS grants a child, not what a child may attempt. Native code still loads, and anything the
package identity is granted anywhere on the machine is still reachable. Full detail and the per-property
evidence table are in `docs/SECURITY-STATUS.md`.

## v0.3 capability broker

`crates/broker` is the primary capability boundary. It is deny-by-default and answers one question:
is this `CapabilityRequest` authorized?

The five permissions are `filesystem.read`, `filesystem.write`, `process.spawn`, `network.connect`,
and `environment.inspect`. A benchmark supplies a `CapabilityPolicy`, and the engine constructs the
broker from `benchmark.capability_policy()` before the first turn, so no run can widen its own
policy. The grid-world benchmark grants nothing.

- `filesystem.read` is authorized only under a granted read root. The path is canonicalized and must
  still be inside a granted root afterwards, so `..` segments and symlinks cannot escape.
- `filesystem.write` uses the same rule against a separate write root. Read and write grants are
  independent values: a read grant never implies write access, and a write grant never implies read
  access.
- `process.spawn` requires the program to be on an explicit allowlist; there is no default.
- `network.connect` requires the `host:port` to be on an explicit allowlist; there is no default.
- `environment.inspect` is denied unconditionally. No policy field or constructor argument can
  grant it, so there is no code path that would let a benchmark or a run enable it.

An `Allow` is an authorization decision, not an implementation. Executing an authorized capability is
a separate step that does not exist yet: the engine returns
`EngineError::CapabilityExecutionUnsupported` for an authorized non-action capability, and the grid
action path is unaffected.

A `Deny` becomes a typed `CapabilityDenial { permission, integrity_event, reason }`. The reason is a
fixed, value-free string: no requested path, program, host, or variable name is copied into the
denial, into telemetry, or into the replay digest. A denied request consumes a turn with no action
outcome, no world movement, and no reward, and the telemetry recorder rejects any broker-denial
event that tries to smuggle in an action, a reward, or a model call.

Broker integrity counters are derived from recorded denial telemetry rather than a local counter, so
a denial that was never persisted cannot be counted. The five broker categories
(`broker_unauthorized_reads`, `broker_unauthorized_writes`, `broker_process_spawn_blocks`,
`broker_network_attempts`, `broker_environment_inspect_denials`) are disjoint from the five
policy-gate categories. A refused write gets its own broker category rather than reusing
`sandbox_violations`, so a capability refusal can never inflate a policy-violation count.

## v0.4: `process.spawn` as the first executable capability

`process.spawn` is the only permission that runs anything. It is the permission that makes the containment
layer a real boundary rather than a library nothing calls — but that boundary is **not yet exercised by
any shipped benchmark**. `GridWorldBenchmark` grants no capabilities at all and declares no
`ProcessSpawnPlan`, so in the desktop product a spawn request is always refused before it reaches the
executor. The executable path is proven by the integration suite in
`crates/engine/tests/process_spawn.rs`, which drives it through a test-only benchmark. The capability
becomes load-bearing the moment a benchmark that genuinely needs process execution exists, such as a
future coding-capability benchmark.

A model can ask for the `process.spawn` **permission** and nothing more — though today only a
deterministic mock strategy can emit that request at all; the connected adapter's response schema
exposes grid actions exclusively, so a real model cannot reach the broker. The request the broker
reviews is rebuilt from the benchmark's compiled-in `ProcessSpawnPlan`, so the model cannot influence
the allowlist comparison, a command line, or an environment block. A plan is a *request* to be
authorized, never a grant: the broker still has to find the program on its explicit allowlist.

`ContainedProcessSpawner` implements `ProcessSpawner`, the only trait that can turn an authorization
into a running process. It prepares the run's designated writable directory, refuses lower-integrity
reads of the benchmark's declared `protected_paths`, starts the child suspended inside a job object,
lowers its primary token, resumes it last, and only then reads the running child's integrity level back
and re-verifies the protection. Every failure returns a typed `ContainmentFailure` and leaves no
process running; there is no branch that starts a child with weaker containment.

Two design points are worth stating because they are easy to get wrong:

- **A spawning benchmark must declare protected paths.** An empty declaration is refused rather than
  treated as "nothing to protect", because a child that can read the evaluator has not been isolated
  in any meaningful sense.
- **The child gets an empty environment.** It does not inherit the host's, so provider credentials are
  not handed to a process one argument away from exfiltrating them. This is an invariant of
  `ContainedProcessSpawner`, not a default: there is no public method that opts back into inheritance.
  One consequence worth knowing is that the child has no `PATH`, so a plan must name an absolute
  program path.

An authorized spawn consumes a turn without moving the world and records no integrity event: the
broker working correctly is not a violation. A refused spawn is a non-executable turn recorded in
`broker_process_spawn_blocks` and records a `broker_denial`, **not** a `ProcessLifecycle` — nothing was
ever requested of the operating system, so there is no lifecycle to describe. An authorized spawn
records a `ProcessLifecycle` carrying the requested flag, the authorization, whether a child spawned,
the PID, the exit code, the containment state, the termination reason, user and kernel CPU time, and
both stream captures. The replay projection deliberately excludes the PID, exit code, CPU time, and
captured output so a mock run's digest stays reproducible.

### Wall-clock bound and output capture

A job object bounds CPU, memory, and process count, but **not elapsed time**, so the plan carries its own
wall-clock timeout. `ProcessSpawnPlan::validate` refuses a plan that does not declare one, or declares
zero, and no code path substitutes a value on the plan's behalf. On expiry the executor terminates the
whole job with `TerminateJobObject`, so nothing the child started outlives the timeout, and the run
records `TerminationReason::Timeout` with no exit code because the child never chose one.

The child's stdout and stderr are redirected to files inside the run's existing designated directory
rather than inherited, which closes an information leak: an inheriting child could write into a
terminal or pipe the operator never agreed to expose. Standard input is pointed at the `NUL` device, so
a child can never block on or read from the operator's console. Telemetry keeps each stream's total
byte count and a preview capped at 4 KiB, while the full output stays in the sandbox file and follows
the directory's existing cleanup. Captured output is never placed in a model-facing value and does not
participate in the replay projection, because all of it differs between two otherwise identical runs.

The other four permissions are unchanged: they are authorized or refused by the broker and then
stop the run with `CapabilityExecutionUnsupported`. None of them was given a premature implementation.

## v0.2 security posture

Policy and runtime guardian interfaces fail closed. Filesystem and network isolation is enforced by omission in the model action interface, not by an OS sandbox. The connected adapter's outbound HTTP is trusted provider infrastructure and is intentionally outside the sandbox action space. Real process, container, filesystem, and network isolation remain future work and are recorded in `docs/SECURITY-STATUS.md`.

## v0.3 Windows containment

`crates/sandbox/src/containment.rs` adds an OS-level layer *below* the capability broker. The broker still decides what is authorized and is the primary boundary; containment only hardens a process that trusted code already decided to start. It is not a code sandbox: there is no syscall filtering, no filesystem or registry virtualization, and no network restriction. A contained child is still a normal Windows process with the same user and privileges.

On Windows, `RestrictedProcessGuard` starts a child in this order: create the job object, create the child with `CREATE_SUSPENDED`, assign it to the job, set the child's primary token to the low mandatory label `S-1-16-4096` with `SetTokenInformation(TokenIntegrityLevel, ...)`, then resume it. The job carries a per-job CPU time limit, a per-process CPU time limit, per-process and whole-job memory limits, an active-process limit, `JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION`, and `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, so dropping the guard terminates everything still in the job. Every step is checked: a failure terminates the still-suspended child and returns a typed `ContainmentError` rather than resuming it with a normal token.

`SandboxDirectory` creates the one directory a contained child may write in, with the documented SDDL `D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;OW)S:(ML;OICI;NW;;;S-1-16-4096)`: a protected DACL plus a low mandatory label. Everything around it keeps the default medium label, and mandatory integrity control refuses a low-integrity write into a higher-labeled object. Preparation fails closed when the label, the DACL, or the parent's label does not match.

On Linux and macOS every containment entry point returns `ContainmentError::UnsupportedPlatform`. The `windows-sys` dependency is target-specific, so the non-Windows dependency graph contains no Windows bindings.
