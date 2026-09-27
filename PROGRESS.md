# Progress

*Describes workspace version 0.4.1. Test counts below were produced by running the commands on a
Windows host at `main` `0608e92`; see `docs/AUDIT-2026-09-26.md` for the full audit that produced them.*

## Done

- Rust 2021 workspace, pinned 1.97.0 toolchain, MIT license, and CI for Rust/frontend verification.
- Documented core domain contracts for sync/async model providers, agents, observations, actions, environments, episodes, benchmarks, evaluators, sandbox, policy, runtime guardian, telemetry, capability results, integrity results, and reproducibility metadata.
- Seeded 5×5 grid world with safe corridor, danger, obstacles, wait, reward, invalid transitions, distinct turn-limit termination, and non-mutating protocol-failure turns.
- **v0.5 benchmark redesign.** The audit found the grid world did not measure reasoning, so six things changed, each load-bearing for a different reason:
  - Start **and** goal are drawn per seed. This is the change that mattered most: with the start at a corner, "prefer DOWN then RIGHT" was an exact distance descent, not a heuristic, and the greedy baseline scored 60 of 70 evaluation seeds for that reason alone.
  - The safe region branches. A monotone spine guarantees a route and a seeded dead-end spur is hung off every eligible spine cell, so the safe tiles form a tree with real forks. Branch hazards are placed only on spine cells off the shortest route, never on a spur — an earlier version dropped hazards onto spur tips and deleted the branches it was guarding.
  - The turn budget is derived per seed as `ceil(3/2 × that seed's shortest hazard-free route)`. The catalog `max_turns` is an upper bound over every seed, and a definition whose ceiling any admitted seed exceeds is refused at load time.
  - Observation is genuinely partial. The agent sees its own tile, its four neighbours, and `distance_to_goal` — a scalar, never a direction. Hazards more than one step away are absent, and a hazard leaves the payload again once the agent walks away, so remembering it is the agent's job.
  - `ModelRequest` carries a bounded rolling history of up to `HISTORY_WINDOW = 8` completed turns, enforced as a hard limit in `ModelRequest::validate`. The bound is part of the token-cost contract: request size is capped regardless of episode length.
  - Reward is shaped by distance-to-goal progress, so two runs that both end in `turn_limit` are distinguishable. `CompletionStatus` is unchanged and remains the categorical pass/fail signal.
- **v0.5 measurement harness** in `crates/engine/src/harness.rs`: runs N episodes over a declared seed pool and reports a success rate with a Wilson score interval. The 40 dev seeds and 70 eval seeds are disjoint, and that disjointness is validated when the definition loads and again in the harness constructor, not left to convention. Random and greedy baselines are measured through the identical harness with identical telemetry.
- A scripted search-based reference solver (`LookaheadSolver`) that is **not privileged**: it receives the same `ModelRequest` as any other agent, is never told the layout, goal, start, or seed, and learns the map only from tiles it has seen plus the distance readings it has taken. Its knowledge scope is verified by a test that replays one request stream against two solvers built with different seeds over different layouts and requires identical decisions.
- `prompts/grid_world_v2.md` states the objective explicitly. v1 never mentioned reward, danger, or winning anywhere, so it was measuring whether a model could infer the goal from English field names.

### v0.5 phase 0 — memory load-bearing proof

- `StatelessReactive` and `MemoryReactive` added as the two halves of a controlled experiment: same
  policy, same rules, same current-observation information, differing only in whether the policy can
  read `ModelRequest::history`. Their descriptors are compared key-by-key in a test so nothing else can
  be confounded.
- **Result: memory is not load-bearing on this benchmark.** Both score 9/70 with identical mean reward,
  mean turns, and status breakdown, and they choose the same action on every turn of every evaluation
  seed. Reported as found; see Known limitations for the structural reason and the three proof tests.
- Deterministic mock strategies: optimal, random, greedy mistake, invalid-action spam, and five independent security probes.
- Fail-closed policy/runtime authorization and non-executable denied-turn transitions.
- Deny-by-default capability broker (`crates/broker`) as the primary v0.3 boundary: `filesystem.read`, `filesystem.write`, `process.spawn`, `network.connect`, and `environment.inspect`, with canonicalized path checks, independent read/write roots, explicit program and endpoint allowlists, and an unconditional `environment.inspect` denial that has no grant path.
- Value-free typed broker denials that consume a turn with no action, movement, or reward, are recorded as non-executable telemetry, and count only in five integrity counters disjoint from the five policy-gate counters.
- Async OpenAI-compatible adapter with env-only credentials, HTTPS/loopback validation, no redirects/retries, typed failures, usage metadata, strict `MOVE_*` JSON protocol, and versioned `prompts/grid_world_v2.md`.

- Append-only SQLite telemetry, semantic replay digest, immutable result/run-metadata storage, token aggregation, and secret-redaction tests.
- Adapter-neutral engine, independent grid-world evaluator, protocol adherence classification, and `model_protocol_failure` after three consecutive model-call failures.
- Tauri v2 IPC with mock/connected provider controls, provider preflight, blocking worker execution, live post-commit telemetry, durable action trace, and React command center.
- `process.spawn` wired as the first genuinely executable capability: the broker reviews the benchmark's trusted plan, an unauthorized spawn never reaches containment or starts a process, and an authorized spawn launches contained on Windows with the full lifecycle recorded. This path is exercised by `crates/engine/tests/process_spawn.rs` through a test-only benchmark; no shipped benchmark declares a spawn plan or a spawn grant, so the containment executor is not yet on the desktop product's live path.
- Path protection that makes the benchmark's declared paths unreadable to a contained child, using a read-up refusal on the path's existing integrity level, verified by a real child process, reversible on drop, and recoverable after a hard kill from an on-disk journal.
- A contained child always starts with an empty environment, so it never inherits the host's provider credentials. This is an invariant of the spawner: the method that would re-enable inheritance has been removed rather than left as an unused public escape hatch.
- Wall-clock containment: every spawn plan must declare its own timeout, an undeclared or zero timeout is refused at plan-validation time rather than defaulted, and expiry terminates the whole job with `TerminateJobObject` and records a distinct `timeout` termination reason with no exit code.
- stdout/stderr redirected into the run's existing designated directory instead of inherited, with standard input pointed at the `NUL` device; telemetry keeps each stream's total byte count plus a 4 KiB-capped preview, the full output stays in the sandbox file, and neither the capture nor the byte count enters the replay digest.
- Windows OS-level defense-in-depth containment in `crates/sandbox`, below the broker: children are started suspended inside a job object with per-job and per-process CPU limits, per-process and whole-job memory limits, an active-process limit, and `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`; the primary token is set to the low mandatory label `S-1-16-4096` before the child is resumed; a designated sandbox directory is labeled low and protected so a contained child can write only there; and since phase 1 the child also runs under a derived AppContainer package identity whose `TokenAppContainerSid` is verified before resume, which is what bounds file access by identity and gets outbound network refused by the OS. Non-Windows platforms return `UnsupportedPlatform` and a spawn fails closed. This is not a code sandbox.


### v0.5 phase 1 — AppContainer identity and network isolation

- **Every contained child now runs as a package identity, not as the invoking user.** A job object
  bounds CPU, memory, process count, and wall clock; it was never an identity boundary, so a child
  could read anything that user could read and could open network connections. Both are now closed:
  file access is bounded by package identity, and the OS refuses outbound network for a package token
  with an empty capability list.
- The SID is **derived** via `DeriveAppContainerSidFromAppContainerName`, not registered.
  `CreateAppContainerProfile` still fails on this host with `ERROR_INVALID_NAME` and no longer blocks
  anything. One profile name for the whole installation, because the package SID is what an ACE names.
- **The identity is verified, not assumed.** The child starts suspended, its `TokenAppContainerSid` is
  read back and compared against the derived SID, and only a match resumes it. A mismatch kills the
  child. No opt-out exists on `ContainedProcessSpawner`, and non-Windows fails closed with
  `AppContainerUnavailable`. The comparison runs on every launch, so all 9 real-child tests exercise
  it — a broken comparison would kill every child rather than pass quietly.
- Real-child evidence, all in `crates/sandbox/tests/appcontainer.rs`: a dynamically linked `cmd.exe`
  launches; a real user file outside the sandbox is unreadable; the designated directory stays writable
  via an ACE naming the package SID, added at run start; and loopback is refused — proven by a real
  loopback listener that an uncontained control reaches and the contained `curl.exe` cannot (exit `7`).
- **A capability is parsed, not derived.** `capability_sid` used to route through
  `DeriveAppContainerSidFromAppContainerName`, which turned the well-known `internetClient` capability
  `S-1-15-3-1` into an unrelated `S-1-15-2-*` package SID — granting nothing while appearing to grant
  something. It now uses `ConvertStringSidToSidW`, and a test pins the distinction.
- **A grant that is requested but not delivered is refused, not dropped.** The parsed SIDs are passed
  through `SECURITY_CAPABILITIES` for real, so the reported capability list cannot claim a grant no
  token carried. `docs/SECURITY-STATUS.md` records what is *not* proven: the measured refusal is
  loopback, and a granted capability is not verified end-to-end, because `whoami.exe` is refused inside
  the AppContainer and the child is resumed before a caller could inspect its token.
- **Two measured findings contradicted the plan, and both are recorded rather than papered over.**
  `LOCALAPPDATA` is mandatory in the child environment or the launch fails with `ERROR_ENVVAR_NOT_FOUND`
  (203), so it is always supplied. And `TEMP`/`TMP` *cannot* be aimed at the designated directory:
  Windows rewrites them to the package's per-user folder and discards what the parent set, so the
  spawner does not pretend otherwise and the test asserts what the child actually observes.
- One existing test changed because containment got stronger, not because it got weaker: the
  wall-clock kill test used `ping -n 60 127.0.0.1` to manufacture a hang, which the new network denial
  now kills instantly. It uses a bounded `for /L` spin instead, which needs no network and makes the
  assertion stronger — the child is consuming CPU against a 60-second budget, so a pass at 1.5 seconds
  is specifically evidence about elapsed time.

### v0.5 phase 2 — the contained-spawn path a real model can actually reach

- **The blocker was the wire encoding, not the containment.** A shipped benchmark could declare a spawn
  plan, grant the program, and protect its answer key, and still never be asked: the only way to
  request a process was `ModelDecision::CapabilityProbe`, which is deliberately unreachable from any
  real provider. The phase adds `ModelDecision::ProcessSpawnRequest`, a model-reachable permission
  request that **carries nothing** — no program, no arguments, no environment. The engine rebuilds the
  reviewed request from the benchmark's own plan, so no model response can reach the allowlist
  comparison or a command line. A separate `deny_all` policy-gate arm fails it closed if it ever
  arrives there, kept distinct from the probe arm so the two bypasses stay distinguishable in
  telemetry.
- **`REQUEST_PROCESS` is on the wire only when the episode offers it.** `ModelRequest` carries
  `process_spawn_available`, read from `benchmark.process_spawn_plan()` — the same call the execution
  path uses, so a model is never told a process is available unless one could actually run. Asked for
  in a benchmark that declares no plan, it is refused as `UnknownAction`, the same as any invented
  token, so a model cannot probe for what is installed by comparing error messages.
- **A second versioned prompt, not an amended one.** `prompts/sandbox_process_v1.md` sits alongside
  `grid_world_v2.md`. The grid-world prompt's published numbers were measured with it byte-identical,
  and a test asserts `REQUEST_PROCESS` is *absent* from it — editing that file to mention a process the
  grid world never offers would invalidate those numbers without changing what it measures.
- **New shipped benchmark `sandbox-process` (v1), 40 dev / 70 disjoint eval seeds.** Success requires
  **both** an authorized contained child that ran to completion **and** the per-seed goal reached. The
  request costs a turn out of a budget derived from the seed's own shortest safe route, so a policy
  that spends its budget carelessly still fails. The child's output is verified from containment's own
  captured stream, not the filesystem, because the evaluator is given no sandbox root.
- **Measured over the 70 held-out eval seeds**, identical solver, identical budget, differing only in
  whether the one request is made:

  | policy | task success | 95% CI | mean turns |
  | --- | --- | --- | --- |
  | search, never asks | **0/70 = 0.000** | `[0.000, 0.052]` | 5.93 |
  | search + one process request | **66/70 = 0.943** | `[0.862, 0.978]` | 6.87 |

  The intervals are disjoint. Both policies reached the goal on nearly the same number of episodes
  (67 and 66 `success` terminations), so the entire gap is the capability and nothing else. The one
  episode the requesting policy loses is the turn the request costs — the tradeoff the benchmark was
  built to expose, visible as 6.87 against 5.93 mean turns.

- **The harness was measuring the wrong thing, and this phase caught it.** `AggregateScore::successes`
  counted `CompletionStatus::Success`, not the benchmark's own `task_success`. Those coincide for the
  grid world, so it was invisible; here they part company, and the never-asking policy initially
  scored 67/70 at a benchmark whose actual task it never performed. The harness now counts
  `task_success`, and `EpisodeOutcome` carries the distinction explicitly.

- **What this benchmark does *not* claim.** The child's work is fixed, not seed-varying, because
  `Benchmark::process_spawn_plan` takes no seed — a per-seed command line is not expressible today. So
  this measures *whether a policy uses the capability it is offered and still finishes under a budget
  that charges it a turn*. It is not a coding measurement, and a seed-aware plan accessor is the
  prerequisite for that.

### v0.5 phase 3 — the reusable cross-benchmark reference agent

- **`ReferenceStrategy` + `ReferenceAgent`**, in `crates/agent`, depending only on `core`. A strategy
  answers exactly one question — given this `ModelRequest`, what do you do — and knows nothing about
  harnesses, telemetry, evaluators, or containment. The wrapper owns request validation, the descriptor
  contract, and the per-episode reset, so a new benchmark supplies four methods and inherits the rest.
  This is a component, not a competing product: it has no objective of its own and no way to be
  configured into looking better than it is.
- **It replaced real duplication, not hypothetical duplication.** The `sandbox-process` reference policy
  had been a `ModelProvider` wrapper written inline inside a test, so the reference policy for a shipped
  benchmark was not a shipped artifact. It is now `SandboxProcessReference`, and
  `GridWorldReference` wraps the existing `LookaheadSolver` rather than reimplementing it — so the
  published grid-world numbers still describe the same policy they always did. The phase-2 measurement
  reproduces identically through the new strategies.
- **The non-privilege guarantee is structural, and published.** A strategy receives `&ModelRequest` and
  `&mut self`, so there is no layout in scope even by accident. The seed is *not* passed to `decide` — it
  is recorded and never consulted. `ReferenceKnowledge` states this as structured data, and
  `ReferenceAgent` writes all four flags into the run's descriptor, so a result bundle carries its own
  guarantee instead of asking the reader to trust a comment.
- **The proof is written once, generically, over a registry.** Phase 0 proved this about one solver in a
  test that named it, which does not scale — the next benchmark would either copy it or skip it. A new
  strategy is now covered the moment it is registered. Three properties: two instances from different
  seeds agree on every turn of one request stream (checked against seeds that are first *proved* to
  describe different worlds, or the check would pass regardless); replaying one episode twice gives
  identical answers; and every strategy declares and publishes observation-only knowledge.
- **The tests caught two real defects in the design, not just in the code.** `ReferenceAgent` originally
  reset only when `episode_id` changed, so a caller reusing an id would carry the previous episode's
  inferences into the next; it now also resets on turn one, which cannot be faked. And a test asserting
  the two strategies "differ only on turn one" was simply false — the request costs the inner solver its
  first observation, which is the effect the benchmark measures and must not be engineered away. The
  test now asserts the claim that actually matters for attribution: after the request, the two run the
  *same policy*, compared against a grid-world reference that was likewise not consulted on turn one.
- **A flaky test found and fixed, root-caused.** `a_child_that_outlives_its_declared_timeout_is_killed_with_the_job`
  failed about one full-suite run in five with "the timed-out child is still running". The assertion was
  stronger than the API it was testing: `TerminateJobObject` *marks* a job's processes for termination
  and returns, and the kernel finishes asynchronously. It now polls a bounded grace period, which holds
  the property worth holding — the child does not *survive* — instead of one that fails on load rather
  than on regressions. Four consecutive full runs are clean.

## Verified

- `cargo fmt --check` — the gate. Bare form is correct here because the workspace root is a *virtual* manifest, so `cargo fmt` already covers every member; `--all` is equivalent at this root.
- `cargo build --workspace --locked`
- `cargo test --workspace --all-targets --locked` — 284 tests passing (277 after phase 2, +7 reference-agent tests)
- `cargo clippy --workspace --all-targets -- -D warnings`
- `pnpm install` in `apps/desktop`
- `pnpm build` in `apps/desktop`
- Local wiremock provider tests cover success, usage, timeout, HTTP error, malformed/empty/invalid output, taxonomy, empty credential, and redaction.
- Broker tests cover per-permission allow/deny, independent read and write roots, canonicalization and traversal refusal, the unconditional `environment.inspect` denial, denial telemetry validation, and an end-to-end forbidden-action-spam run whose integrity counts are asserted against recorded denial telemetry.
- Windows host: `cargo test -p probelab-sandbox --locked`, including the real low-integrity child escape test (`%ComSpec%` writes one file inside the designated directory and is refused outside it), the job-object limit read-back, the CPU-limit kill, and the drop-kills-child check. All 10 containment tests pass.
- `cargo test --workspace --all-targets --locked`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo build --workspace --locked` all pass on a Windows host, including the Tauri desktop crate. `cargo fmt --all` (without `--check`) is the command that *applies* formatting; it is not a verification gate and is deliberately not listed as one above.


## Status by area

### Implemented

- Real model boundary with synchronous engine bridge.
- Prompt/protocol versioning and reproducibility metadata.
- Capability/protocol adherence separated from integrity.
- Deny-by-default capability broker as the primary capability boundary, with no grant path for `environment.inspect`.
- Local mock replay and durable result/action-trace persistence.

### Partial

- Only `process.spawn` is performed. Authorized `filesystem.read`, `filesystem.write`, and `network.connect` still stop the run with an explicit unsupported error, deliberately.
- A contained child keeps its parent's user and privileges; it is refused writes outside its designated directory and reads of declared protected paths, but not networking or native code loading.
- Windows containment hardens the children a benchmark's own plan starts. There is no Linux or macOS equivalent, so a spawn there fails closed.
- Connected providers are local-first but disclose model-facing observations to the configured endpoint.
- Provider egress is trusted adapter infrastructure; the model action space still has no network capability.
- No cross-process trace verifier, run cancellation, or API-spend/rate-limit control.

### Deferred

- Executing authorized filesystem and network capabilities, and syscall-level isolation: seccomp/namespaces on Linux, filesystem virtualization, and syscall filtering. The Windows layer is now a job object, a mandatory integrity label, **and** an AppContainer package identity with the OS refusing outbound network — but it is still refusal rather than filtering, so native code loads and anything the package identity is granted remains reachable. Restricted tokens remain unusable without elevation.
- Linux and macOS OS-level containment. There is no AppContainer equivalent ProbeLab can rely on there, and a weaker substitute is not offered in its place.
- Provider-specific adapters beyond OpenAI-compatible HTTP.
- Auth, multi-user support, cloud sync, and additional capability dimensions.

## Next recommended milestone

The audit in `docs/AUDIT-2026-09-26.md` ordered this milestone ahead of Windows identity isolation,
and the ordering held: the containment stack is real, well tested, and empirically holds, but it is
**unreachable from the product** because no shipped benchmark declares a spawn plan. Hardening it
further would harden a code path no run can invoke. The benchmark, by contrast, is the project's actual
output, and it was measuring nothing.

1. **Run real models against the v2 benchmark and publish the numbers.** The instrument now has a
   measured range — greedy 0.286 `[0.193, 0.401]` against a search reference at 0.943
   `[0.862, 0.978]` over 70 held-out seeds. What is missing is any real model on that scale. This is
   the first step that produces a capability claim rather than a calibration of the instrument.
2. **Surface the aggregate in the desktop app.** The harness is library-and-CLI only by deliberate
   scope, so the UI still runs one episode and shows no success rate, no Wilson interval, and no
   baseline. That gap is real and is the obvious next UI milestone.
3. ~~**Add the AppContainer identity layer**~~ — **done in phase 1.** Every contained child now runs
   under a derived package identity with a verified `TokenAppContainerSid`, and outbound network is
   refused by the OS. The recorded `CreateAppContainerProfile` failure and the reason not to retry the
   restricted-token approach both still stand in `docs/SECURITY-STATUS.md`.
4. **Expose an authorized `filesystem.read`** to a contained child through the same executor, so a
   child can be given a read-only view of a granted root while the evaluator stays refused. This is now
   the natural next step on the spawn path, because the identity layer that would enforce it exists.
5. **Add the remaining capability dimensions** — reasoning, coding, safety, injection — and the
   Benchmark SDK, now that one benchmark is known to measure something. Multiplies correctly only
   because of step 1.
6. Persisted trace read/recompute verification and explicit run cancellation.

## Known limitations

- **The bounded history window is not load-bearing on the grid world.** Phase 0 measured it: a
  stateless reactive policy and an otherwise identical memory-reading reactive policy both score
  9/70, with identical mean reward, mean turns, and terminal-status breakdown, and they take the same
  action on every turn of every evaluation seed. The reason is structural — the observation radius
  equals the step size, so the cell a policy is about to enter was adjacent on the turn it chose the
  move, and a goal seen on any turn ends the episode on that turn. Making memory matter needs an
  observation radius larger than one step, a changing environment, or moving hazards. That would change
  what the benchmark measures, so it was recorded rather than forced. Full analysis and the three
  supporting proof tests are in `ARCHITECTURE.md` and `crates/engine/tests/phase0_memory.rs`.
- **The multi-episode harness is not wired into the desktop app.** `run_benchmark` still runs one
  episode against one seed. No success rate, confidence interval, or baseline appears in the UI, and
  the presence of `crates/engine/src/harness.rs` should not be read otherwise.
- **No real model has been measured on the v2 benchmark.** The reference solver is a scripted searcher,
  not a model. Beating it is a low bar, and a pass here is a precondition for being interesting rather
  than a result worth reporting.
- **The greedy/search gap comes from the per-seed start and the tight per-seed budget, not from the
  branches.** An intermediate branch-free generator still showed a wide gap. Branching and hidden
  hazards add difficulty on top; if they are later found not to discriminate, they are calibration
  rather than the measurement.
- A model adapter is trusted Rust code in the desktop process. Interface design restricts the model-facing action space but does not isolate arbitrary adapter code from host resources.
- The Windows containment layer hardens a child ProbeLab already decided to start. It is not a code sandbox: no syscall filtering, no filesystem or registry virtualization, no network restriction, and the child keeps the parent's user and privileges.
- Remote provider responses are not deterministic even at temperature zero; mock runs are the replayability baseline.
- Background runs are not recovered after a desktop process restart, although partial telemetry and stored results remain durable.
- Desktop packaging and platform-specific installers have not been fully verified beyond development compilation and frontend build.
