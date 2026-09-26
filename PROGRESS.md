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
- Windows OS-level defense-in-depth containment in `crates/sandbox`, below the broker: children are started suspended inside a job object with per-job and per-process CPU limits, per-process and whole-job memory limits, an active-process limit, and `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`; the primary token is set to the low mandatory label `S-1-16-4096` before the child is resumed; a designated sandbox directory is labeled low and protected so a contained child can write only there. Non-Windows platforms return `UnsupportedPlatform` and a spawn fails closed. This is not a code sandbox.


## Verified

- `cargo fmt --check` — the gate. Bare form is correct here because the workspace root is a *virtual* manifest, so `cargo fmt` already covers every member; `--all` is equivalent at this root.
- `cargo build --workspace --locked`
- `cargo test --workspace --all-targets --locked` — 256 tests passing
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

- Executing authorized filesystem and network capabilities, and syscall-level isolation: seccomp/namespaces on Linux, AppContainer or restricted tokens, filesystem virtualization, and network isolation. The v0.3/v0.4 Windows layer is a job object plus a mandatory integrity label only.
- Linux and macOS OS-level containment.
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
3. **Add the AppContainer identity layer**, using `DeriveAppContainerSidFromAppContainerName`.
   `CreateAppContainerProfile` fails with `HRESULT 0x80070017` on this host. Do **not** retry the
   restricted-token approach — it is unusable without elevation, for the reason recorded in
   `docs/SECURITY-STATUS.md`.
4. **Expose an authorized `filesystem.read`** to a contained child through the same executor, so a
   child can be given a read-only view of a granted root while the evaluator stays refused.
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
