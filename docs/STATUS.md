# Status

The single source of truth for what ProbeLab does, what is verified, and what is not.

**Read this before trusting any other document here.** `README.md`, `ARCHITECTURE.md`, `SECURITY.md`,
and `PROGRESS.md` describe the design and the reasoning. This file describes the state. Where they
disagree, this file is correct and the other is stale.

- **Workspace version:** 0.4.1 (`Cargo.toml` is the authority; nothing else declares it)
- **Toolchain:** 1.97.0, pinned by `rust-toolchain.toml`
- **Licence:** MIT
- **Verified on:** Windows 11, `x86_64-pc-windows-msvc`, on the tree described by the change record
  below (parent `7dd872e`). Every gate in `.github/workflows/ci.yml` was re-executed on that tree
  immediately before it was committed: `cargo fmt --check`, `clippy -D warnings`, `cargo test
  --workspace --all-targets --all-features --locked`, `cargo build --workspace`, `cargo deny check`,
  `python3 scripts/check-feature-matrix.py`, `pnpm test`, `pnpm build`, `pnpm check:styles`,
  `pnpm check:icons` — all passing.
- **Change record:** universal-execution change set on top of `7dd872e`, plus the release-gate pass:
  six dead `State` commands fixed, six camelCase argument keys fixed,
  protocol-verified discovery, served-model enumeration, read-only guest-isolation inspection,
  automatic CLI-agent execution refused at the product boundary.
  New crates `probelab-executors` and `probelab-tasks`, new
  Tauri commands `list_executors`, `preflight_executor`, `cancel_experiment`,
  `export_experiment`, `list_task_benchmarks`, `run_task_eval`,
  `guided_task_package`, `score_guided_submission`, `list_local_models`,
  `inspect_guest_isolation`, new readiness and task
  routes. Executed: **831 Rust tests, 0 failed**; **262 frontend tests,
  0 failed**. Previous counts (769 / 237) are superseded by these.
- **Release state:** FEATURE COMPLETE / AUDIT COMPLETE. Known limitations are listed below and are
  accepted; none is a blocker.

---

## 1. What this project is

A controlled evaluation laboratory for language-model agents. A model is given a task in an environment
where its actions are independently observed, its requests for extra capability pass through a broker
that is the only thing that can authorize them, and the result is evaluated by code the model cannot
reach. Runs are recorded well enough to inspect, replay, and compare.

The thesis is the *boundary*, not the number of features. What the product asserts is that the broker
is the sole authorization authority, the sandbox is the sole way a process starts, the evaluator is
independent, and a refused probe is a successful test rather than a vulnerability. Everything in
"Known limitations" below is a limit on those claims, stated plainly.

---

## 2. Verified areas

Everything in this section was executed on the host named above. Commands and their results are in
section 9.

| Area | State | Evidence |
| --- | --- | --- |
| Rust workspace builds, lints, formats | verified | `cargo build --workspace --all-features --locked`, `clippy -D warnings`, `cargo fmt --check` |
| 831 Rust tests | passing | `cargo test --workspace --all-targets --all-features --locked` (executed, 0 failed) |
| 262 frontend tests | passing | `pnpm test` in `apps/desktop` (executed, 0 failed) |
| Windows process containment | verified | 82 tests in `crates/sandbox/`, including 9 that launch a real child under a real AppContainer identity |
| Dependency and licence policy | verified | `cargo deny check` — advisories, bans, licences, sources all `ok` |
| Wire format | pinned | `crates/lab/tests/wire_format.rs`, 23 tests against hand-written literals |
| IPC boundary | covered | `apps/desktop/src-tauri/tests/ipc_boundary.rs`, 27 tests |
| IPC command registration | verified | `every_command_the_frontend_invokes_is_registered` compares the frontend's real `invoke` calls against the real `generate_handler!` list |
| Feature matrix | verified | `python scripts/check-feature-matrix.py` |
| Adversarial self-tests | passing | `crates/lab/tests/adversarial.rs`, 24 tests including live loopback servers |
| End-to-end user paths | passing | `crates/lab/tests/end_to_end.rs`, 11 tests across experiment, security, arena, settings, comparison, and provider ||
| Cross-language fixtures | pinned | `crates/lab/tests/emit_fixture.rs` emits the read model the frontend tests render and asserts the committed bytes still match live serialization |
| Legibility floor | enforced | `apps/desktop/scripts/check-style-legibility.mjs`; a font size below 11px in either `px` or `rem` fails |
| Icon freshness | enforced | `apps/desktop/scripts/check-icons-current.mjs`; the platform icons must match `app-icon.svg` |
| Connected-model measurement | passing | `apps/.../brain.rs::a_connected_model_is_measured_across_every_planned_episode` -- 4 planned episodes, 4 completed, 12 real HTTP calls to a local provider |
| Universal executor discovery | passing | `crates/executors/src/lib.rs` + `isolation.rs`; `list_executors` / `preflight_executor` / `list_local_models` / `inspect_guest_isolation` commands |
| Task benchmarks (coding-repair, memory-latch) | passing | `crates/tasks/src/`, 19 tests; optimal 70/70 vs noop 0/70 (coding), memory 70/70 vs stateless at chance (latch), disjoint Wilson intervals |
| Run cancellation | passing | `crates/lab/tests/runner.rs::a_cancelled_run_stops_between_episodes_and_says_so`; `cancel_experiment` command; cooperative between episodes with `Cancelled` terminal reason |
| Result export | passing | `apps/.../executors.rs` tests; `export_experiment` command (JSON read model, CSV per-episode rows) |

### 2.1 Supply chain

`cargo deny check` passes. **Zero known vulnerabilities.**

Six advisory exceptions exist, all `unmaintained` notices rather than vulnerabilities, all transitive,
all reached through Tauri, and none with an available upgrade. Each names its dependency and its
reason in `deny.toml`:

- `proc-macro-error` — a build-time proc-macro for Tauri's Linux GTK backend. Not linked into the
  shipped binary, never executed at run time.
- five `unic-*` crates — Unicode lookup tables, compile-time only, reached via `urlpattern` ->
  `tauri-utils`. Pure data.

Licences are an explicit allowlist at 0.93 confidence. Unknown, unlicensed, and unknown-source
dependencies are all hard failures. Duplicate versions are reported as warnings (30 transitive
duplicates, all from the Tauri and proc-macro trees).

### 2.2 Provider support — stated precisely

| Provider | Adapter | Selectable in the desktop app | Notes |
| --- | --- | --- | --- |
| OpenAI-compatible (chat completions) | `crates/model-adapters/src/lib.rs` | **yes** | The only provider the run configuration admits |
| Anthropic (Messages API) | `crates/model-adapters/src/anthropic.rs` | **no** | Built and tested as a library; unreachable from the product |
| Gemini (`generateContent`) | `crates/model-adapters/src/gemini.rs` | **no** | Same |

`crates/lab/src/settings.rs::ConnectedProvider` has exactly one variant, and that is the honest state:
the desktop product can configure one provider. The Anthropic and Gemini adapters exist, are unit
tested against the same transport rules (HTTPS/loopback, no redirects, no retries, env-only
credentials), and are **not** wired into the IPC layer. `docs/feature-matrix.json` marks all three
`complete` as *adapters*; the `command` it names is a witness command, not a selector.

Deliberately absent: a native Ollama adapter, and OpenCode as a provider. OpenCode is a coding-agent
tooling environment, not a model endpoint, so adding it as one would be a category error. The
OpenAI-compatible transport reaches any server that exposes `/v1/chat/completions`; that is a
compatibility statement, not a tested claim, and no such server is part of CI.

### 2.3 Benchmark support

| Benchmark | State | Replay | Platform |
| --- | --- | --- | --- |
| `grid-world` (v2) | implemented | deterministic | any |
| `sandbox-process` (v1) | implemented | deterministic | Windows only; refused at plan time elsewhere |
| `coding-repair` (v1) | implemented | deterministic | any (reference + guided in the product; automatic CLI-agent execution is refused at the product boundary — see §8) |
| `memory-latch` (v1) | implemented | deterministic | any (reference + guided in the product; automatic CLI-agent execution is refused at the product boundary — see §8) |

Both are in the built-in catalogue and reachable from the command center. That second claim was false
until this cycle: `App.tsx` read `benchmarks[0]`, the catalog's select button rendered only when
`onSelect` was passed, and it never was — so `sandbox-process`, the only benchmark that exercises a
containment boundary, could not be reached from the UI at all while the README listed it as implemented.
Seed pools are 40 dev and 70 evaluation seeds, validated as disjoint both when the definition loads and
again in the harness constructor. Each summary now also carries a sha256 of the definition's own bytes,
and a difference in that digest blocks a comparison.

### 2.4 Arena support

**Tic-Tac-Toe only.** It is playable end to end through the engine, with a perfect minimax reference and
a first-legal floor as the two seats.

`GameKind` also declares `ConnectFour`, `Rps`, and `Battleship`, and `GameMove`/`GameBoard` carry their
payloads. Those are **schema only**. No engine module implements them, and
`crates/lab/src/arena.rs` refuses any `GameKind` other than `TicTacToe` with a typed
`ArenaRunError::UnknownGame` before a match starts. The Providers/Arena panel states this in the UI
rather than offering a picker that would fail.

There is no human seat. `play_match` is atomic and agent-against-agent.

---

## 3. Security guarantees

Each of these is asserted by a named test. A guarantee listed here without a test would be a claim, and
this file does not carry claims.

| Guarantee | Enforced by | Test |
| --- | --- | --- |
| The broker is the only authorization authority | `crates/broker` depends only on `core`; it holds no model handle | `crates/lab/tests/runner.rs::the_runner_is_not_a_security_authority` |
| The Brain cannot override an authorization decision | No broker handle is reachable from `BrainSurface` | `apps/.../brain.rs::no_command_can_override_an_authorization_decision` |
| The Arena cannot steer an authorization or a result | `crates/lab/src/arena.rs` imports no broker or policy | `crates/lab/tests/arena.rs::the_arena_is_not_a_capability_authority` |
| Replay holds no execution or authorization capability | `crates/lab/src/replay.rs` reads recorded data only | `crates/lab/tests/replay.rs::replay_holds_no_execution_or_authorization_capability` |
| The Security Lab cannot declare a breach | `apps/.../security.rs` contains no broker or policy type | `apps/.../security.rs::no_command_can_declare_a_breach_or_reclassify_an_outcome` |
| The evaluator trusts nothing the model says | Integrity counters derive from typed observations, never from model output | `crates/engine/tests/vertical_slice.rs` |
| `environment.inspect` can never be authorized | The broker denies it as its first match arm; `grant_permission` for it is a hard error | `crates/broker/tests/broker.rs::environment_inspection_is_denied_even_for_a_malformed_policy` |
| A model's decision cannot carry authorization | `ModelDecision` has no field that could hold a broker, policy, or grant | `crates/lab/tests/adversarial.rs::a_model_decision_carries_no_authorization_handle` |
| Broker refusals and policy violations are counted in disjoint categories | `IntegrityResult` splits five and five; `total_policy_violations` and `total_broker_denials` | `crates/lab/tests/adversarial.rs::broker_categories_and_policy_categories_are_disjoint` |
| A refused spawn is distinguishable from one that ran | `ProcessLifecycle.execution` is `None` exactly when denied | `crates/lab/tests/adversarial.rs::a_refused_spawn_and_a_run_spawn_are_distinguishable` |
| A model cannot reach the broker without offering | `REQUEST_PROCESS` parses only when `process_spawn_available`; otherwise it is `UnknownAction`, identical to any invented token | `crates/model-adapters/src/lib.rs` tests; `apps/.../tests/ipc_boundary.rs` |
| A contained child cannot inherit the host environment | The spawner has no inheritance method at all | `crates/sandbox/src/containment/spawn.rs::the_spawner_exposes_no_way_to_inherit_the_host_environment` |
| Every contained child runs under a verified AppContainer identity | The `TokenAppContainerSid` is read back and compared before resume; a mismatch kills the child | `crates/sandbox/tests/containment_identity.rs` |
| No credential crosses the IPC boundary | `deny_unknown_fields` on both credential-adjacent types; the adapter reads the key from the environment | `apps/.../tests/ipc_boundary.rs::a_credential_cannot_hide_in_an_undeclared_field` |
| No failure message carries a credential | The transport redacts response text at source | `crates/lab/tests/adversarial.rs::no_transport_failure_message_ever_carries_the_credential` |

### 3.1 Process containment — what it actually does

On Windows, a contained child:

- starts **suspended**;
- is assigned to a **job object** with per-job and per-process CPU limits, per-process and whole-job
  memory limits, an active-process limit, and `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`;
- is given a **derived AppContainer package identity** whose `TokenAppContainerSid` is read back and
  verified before the child is resumed. The package name is `ProbeLab.ContainedChild`, installation-wide,
  derived via `DeriveAppContainerSidFromAppContainerName` rather than registered;
- is placed at a **low mandatory label** (`S-1-16-4096`);
- can write only to a **designated directory**, protected by an ACE naming the package SID;
- cannot read the benchmark's **declared protected paths** (a read-up refusal on the path's integrity
  level);
- has outbound **network refused by the OS**, with an empty capability list;
- runs with an **empty environment** plus the `LOCALAPPDATA` Windows requires — and no credential the
  parent held;
- is killed on a **declared wall-clock timeout** with `TerminateJobObject`, recording a `timeout`
  termination reason and no exit code;
- has stdout/stderr redirected into the run's own directory, stdin pointed at `NUL`, each stream's byte
  count plus a 4 KiB-capped preview recorded, and the full output kept in the sandbox file.

The AppContainer SID **is** the file-access boundary, and the OS refusing network for a package token
with no capabilities **is** the network boundary. A granted capability is passed through
`SECURITY_CAPABILITIES` for real, so the reported list cannot claim a grant no token carried.

Non-Windows platforms return `UnsupportedPlatform` and a spawn **fails closed**. There is no weaker
substitute.

### 3.2 Security limitations — read these

**This is not a code sandbox.** There is no syscall filtering, no filesystem or registry
virtualization, and no control over native code loading. A job object bounds resources; an AppContainer
identity bounds file access by identity. Neither intercepts a syscall.

- `LOCALAPPDATA` must be present in the child's environment or the launch fails with
  `ERROR_ENVVAR_NOT_FOUND` (203), so the spawner always supplies it.
- `TEMP`/`TMP` **cannot** be aimed at the designated directory. Windows rewrites them to the package's
  per-user folder and discards what the parent set. The spawner does not pretend otherwise, and the
  test asserts what the child actually observes.
- A granted capability is not verified end to end from outside. `whoami.exe` is refused inside the
  AppContainer and the child is resumed before a caller could inspect its token, so the evidence is the
  `SECURITY_CAPABILITIES` being passed for real plus the absence of the network grant, not a token dump.
- A contained child's stdout/stderr preview (4 KiB each) is persisted **unredacted**. A child that
  printed an inherited secret would persist it. The child cannot inherit the parent's environment, so
  this is a narrow exposure, but it is not redacted and that is a known gap.
- `CreateAppContainerProfile` is never called: it fails on this host with `HRESULT 0x80070017`
  (`ERROR_INVALID_NAME`) and the derived SID is what the ACE names. This is deliberate and recorded.

### 3.3 Capability execution

| Permission | Broker | Executed |
| --- | --- | --- |
| `process.spawn` | program allowlist, no default, empty program always refused | **yes**, under the containment above |
| `filesystem.read` | read roots, canonicalized, traversal and symlink escape refused | no — `CapabilityExecutionUnsupported` |
| `filesystem.write` | write roots, independent of read roots | no — `CapabilityExecutionUnsupported` |
| `network.connect` | endpoint allowlist, no default | no — `CapabilityExecutionUnsupported` |
| `environment.inspect` | **denied unconditionally, no grant path exists** | unreachable |

The last row is not "authorized and refused at execution time" — it cannot be authorized at all, so
that code path is unreachable for it.

---

## 4. Persistence semantics

Two SQLite files, deliberately separate:

- `probelab.sqlite3` — results, run metadata, telemetry events, and experiment lifecycle rows. Results
  carry an immutability trigger.
- `probelab-settings.sqlite3` — the settings document only. A settings reset is structurally incapable
  of reaching a measurement, and a corrupt settings row cannot be read through the results API.

| Fact | Value |
| --- | --- |
| Telemetry | append-only, gap-free per run, committed in sequence order |
| Results | immutable once written; a second write to the same id is refused |
| Action trace | durable in `RunMetadata.action_trace`, written before the completion event is emitted |
| Crash recovery | on start-up, any row still saying `running` is marked `interrupted` and logged. Provably wrong to leave alone: nothing is running. |
| Interruption | an interrupted run's partial episodes are preserved but are **not** a complete measurement |

---

## 5. Replay semantics

**Deterministic replay reconstructs. It does not re-run.** The replay path holds no engine, no provider,
and no broker, so it cannot produce a different result than the recording and cannot call a model even by
accident. A replay that re-queried a provider would be a second, different experiment wearing a familiar
name.

| Recording | Capability | Why |
| --- | --- | --- |
| Arena match | `full` — RE-EXECUTABLE | Every ply board was recorded, so it can be stepped frame by frame |
| Experiment | `projected_only` — INSPECTION ONLY | The recording is an event stream; intermediate environment states were never captured |

The panel states which one it has rather than implying both, and renders every limitation in full. A
replay that quietly filled a gap would be fabricating history.

`ArenaMatchView::replay_digest` is a change detector computed from the recording, not stored, covering
every field a reader treats as the result — identity, seats, outcome, winner, ply count, final board,
illegal moves, failure reason, and the ply history. It is an FNV-style 64-bit hash, **not** a security
primitive; the security-relevant digest is the engine's own SHA-256 `replay_digest` in `TestResult`.

---

## 6. Reproducibility

### 6.1 What a stored result records

| Field | Recorded | Where |
| --- | --- | --- |
| Benchmark and version | yes | `TestResult.benchmark_id`, `.benchmark_version` |
| Provider and model | yes | `ReproducibilityMetadata.provider`, `.model` |
| Model configuration fingerprint | yes | `configuration_fingerprint` |
| Prompt template version | yes | `prompt_template_version` |
| Environment seed | yes | `TestResult.seed` |
| Model RNG seed | yes | `ModelDescriptor.seed` |
| Root seed | partially | `ExperimentRecord.root_seed`; **not** in the result bundle |
| Result, reward, turns, completion status | yes | `CapabilityResult` |
| Protocol adherence | yes | `ProtocolAdherence` |
| Integrity and capability counters | yes | `IntegrityResult` |
| Token usage | yes | `ReproducibilityMetadata.token_usage` |
| Action trace | partial | `RunMetadata.action_trace` — turn, selected action, outcome, reward. No observation, so the prompt cannot be rebuilt from the bundle alone |
| Terminal reason | yes | `ExperimentRecord.terminal_reason`, derived from the serde representation |
| Software version | yes | `ReproducibilityMetadata.probelab_version`, from `CARGO_PKG_VERSION` |
| Source commit | yes | `ReproducibilityMetadata.commit_sha`, stamped by `crates/core/build.rs`; `-dirty` when the working tree did not match it, `unknown` when there was no git to ask |
| Benchmark definition digest | yes | `TestResult.benchmark_definition_digest` — sha256 over the definition's own bytes, so it cannot drift from what was loaded |
| Episode configuration | **no** | no `max_turns` or turn-budget ratio in a result bundle |
| Policy / capability configuration | **no** | a stored result cannot name the allowlisted program, granted roots, or protected paths |

### 6.2 Deterministic replay vs external rerun

These are distinct and the code keeps them distinct.

- **A mock run replays exactly.** Same seed, same strategy, same prompt: the digest is reproducible.
  `MockModelConfig` policies are pure functions of a seeded ChaCha8 generator.
- **A connected model run does not.** A remote model is not deterministic even at temperature zero.
  ProbeLab never claims otherwise, and a mock run is the replayability baseline. A connected run's
  `replay_digest` is a comparison token for that recording, not a promise that re-running reproduces it.
- **Every descriptor carries its execution kind.** `ModelDescriptor.adapter` distinguishes
  `in_process_deterministic` from `remote_model`, and the frontend refuses to present a probe's integrity
  count as a model capability score because of it.

### 6.3 Reproducibility limitations

Four of the six gaps that used to be here are closed. What remains is a limit on what a stored bundle
can rebuild, not on what it can identify.

- **The action trace has no observation**, so a prompt cannot be rebuilt from the result bundle. The
  per-turn observations are in the `telemetry_events` table, joined by `run_id`. This is the largest
  remaining gap: a bundle now says exactly which build, which task, and which model produced it, but not
  what was said.
- **The configuration fingerprint does not cover the endpoint**, deliberately — an endpoint must not be
  recorded — so two runs against different servers with the same model name share a fingerprint. It is
  secret-safe and therefore not sufficient to re-run. The fingerprint is scoped to the *vendor*, so an
  Anthropic run and an OpenAI-compatible run configured identically cannot collide.
- **A stored timeline that exceeded 10 000 events is truncated**, and the loss is now recorded:
  `LabTimeline::dropped_events` counts what the retention bound removed and `is_truncated`
  distinguishes a truncated recording from a short one. The limit is still not encoded in the
  recording itself.
- **`commit_sha` is `unknown` for a build with no git** — a vendored tree, a published crate, a release
  pipeline that passed nothing. That is a recorded absence rather than a guessed value, and it is the
  only honest answer available.

Closed in this cycle, and previously listed as the largest gaps in this section:

- ~~No software version or commit is recorded.~~ `probelab_version` and `commit_sha` are stamped on every
  run by `ReproducibilityMetadata::for_run`, which is the single constructor that means a run is
  happening. `Default` deliberately does not stamp a commit, because `Default` means "nothing is known
  about a run that does not exist". `crates/core/build.rs` appends `-dirty` when the tree did not match
  `HEAD`, so a build is never stamped with a commit that does not describe it.
- ~~The benchmark "version" is a hand-bumped string.~~ A sha256 over the definition's bytes is now
  carried on `TestResult`, `RunMetadata`, and `ComparableRun`, and a difference in it **blocks** a
  comparison. The digest is computed from the file's own bytes rather than a re-serialization, so a pure
  reformat also moves it — the conservative direction, since a changed digest costs a re-baseline and a
  missed edit costs a wrong comparison.
- ~~Anthropic and Gemini descriptors omit three reproducibility fields.~~ Both now record the
  fingerprint, prompt template version, and environment version. The fingerprint function was typed to
  `OpenAiCompatibleConfig` and so could not have been called for them; it is now vendor-scoped.
- ~~A comparison could not see which model produced a result.~~ `ComparableRun` carries `provider`,
  `model`, and the task digest, and differences block the comparison.

---

## 7. Measured results

Measured on the host named at the top, at commit `6e7c7c5`, over the 70 held-out evaluation seeds
(disjoint from the 40 dev seeds), reproducible with:

```
cargo run --release -p probelab-engine --example grid_world_harness -- --episodes 70
```

| Policy | Task success | 95% CI | Mean reward | Mean turns |
| --- | --- | --- | --- | --- |
| random (baseline) | 0/70 = 0.000 | [0.000, 0.052] | 1.41 | 8.21 |
| greedy, one-step (baseline) | 24/70 = 0.343 | [0.242, 0.460] | 5.69 | 7.27 |
| lookahead search (reference) | 68/70 = 0.971 | [0.902, 0.992] | 14.91 | 5.83 |
| stateless reactive | 9/70 = 0.129 | [0.069, 0.227] | 2.76 | 7.90 |
| memory reactive | 9/70 = 0.129 | [0.069, 0.227] | 2.76 | 7.90 |

Task benchmarks, same 70-seed evaluation shape (reproducible in-process —
the reference policies are library calls, asserted in
`crates/tasks/src/` tests):

| Benchmark | Policy | Task success | 95% CI |
| --- | --- | --- | --- |
| `coding-repair` v1 | optimal (reference) | 70/70 = 1.000 | [0.948, 1.000] |
| `coding-repair` v1 | noop (baseline) | 0/70 = 0.000 | [0.000, 0.052] |
| `memory-latch` v1 | memory (reference) | 70/70 = 1.000 | [0.948, 1.000] |
| `memory-latch` v1 | stateless (baseline) | at chance (~1/3) | strictly below memory, disjoint |

**No connected model and no installed CLI agent has been measured on any
benchmark.** The capability to do so now exists and is
tested end to end — `run_experiment` accepts a model configuration, registers it for that run only, and
runs the full episode pool through the same `LabRunner` the mock uses, so a connected result carries an
episode count, a Wilson interval, and a recorded provider, model, fingerprint, and task digest. The test
that exercises it reports 4 planned episodes, 4 completed, and 12 real HTTP calls to a local stand-in
provider.

What has *not* happened is a measurement of a real vendor's model. The stand-in is a `std::net` listener
returning a fixed response, so the path is proven but no real model's behaviour has been observed, and no
table above contains a connected row. Adding one requires a credential this host does not have, and a
number produced without one would be a fabrication rather than a measurement.

The reference policy is a scripted searcher, not a model. Beating it is a low bar, and a pass on a
connected model would be a precondition for an interesting result rather than a result worth reporting.

Two findings these numbers support, and one they do not:

- The greedy-to-search gap is wide and the intervals are disjoint. The gap comes from the per-seed start
  and the tight per-seed budget; an intermediate branch-free generator still showed a wide gap, so
  branching and hidden hazards add difficulty on top and are not yet shown to discriminate.
- **Memory is not load-bearing on the grid-world benchmark.** The stateless and memory-reactive policies score
  identically, take the same action on every turn of every seed, and have identical reward, turns, and
  status breakdown. The reason is structural: the observation radius equals the step size, so the cell
  the policy is about to enter was adjacent on the turn it chose the move, and a goal seen on any turn
  ends the episode on that turn. Making memory matter needs a larger observation radius, a changing
  environment, or moving hazards — all of which would change what the benchmark measures. Recorded in
  `ARCHITECTURE.md` and proved by `crates/engine/tests/phase0_memory.rs`.

The `sandbox-process` benchmark's published numbers previously in `PROGRESS.md` have **no reproduction
command in this repository** and have been removed rather than restated. See section 8.

---

## 8. Known limitations

Accepted. None is a release blocker.

### Correctness and reproducibility

1. The action trace has no observation, so a prompt cannot be rebuilt from the result bundle alone (6.3).
2. ~~`LabEvent::detail` is truncated to 512 characters and is **not** redacted.~~ Event details now
   pass through a secret-agnostic sanitizer (`sanitize_detail` in `crates/lab/src/events.rs`:
   `sk-…`/bearer/`api_key=` shapes and control characters removed, 512-char bound kept), and the
   docs state the two-layer design honestly. Provider response text is still redacted with the real
   secret at source by the transport layer, which is the only place the secret value exists.
3. ~~A contained child's stdout/stderr preview is persisted unredacted.~~ Stream previews now pass
   through the same secret-agnostic sanitizer (`sanitize_preview` in
   `crates/sandbox/src/containment/spawn.rs`). The child environment still holds no credentials, so
   this was and remains a narrow exposure — now a filtered one.
4. An Anthropic or Gemini descriptor records `base_url` and the credential variable *name* in
   `ModelDescriptor.parameters`, which the OpenAI-compatible adapter deliberately omits. Not a secret
   value, but the two conventions disagree about what is publishable.
5. `ExperimentRecord` exists twice — `crates/lab/src/experiment.rs` (not persisted) and
   `crates/telemetry/src/lib.rs` (persisted), with different field casing. Only the second reaches
   SQLite; nothing enforces that.
6. ~~`put_experiment`'s UPSERT omits `benchmark_id` and `episodes_requested`.~~ The UPSERT now
   writes `benchmark_id` and `episodes_requested` on conflict. The terminal write also records a
   JSON-encoded score in `result_json` and the real wall-clock time in `updated_unix_ms`.
7. ~~`updated_unix_ms` is always written as `0`.~~ Both the running and terminal writes record
   real wall-clock milliseconds (`now_unix_ms` in `apps/desktop/src-tauri/src/brain.rs`), so
   ordering by update time works.
8. ~~**`BrainState::Running` is unreachable.**~~ `LabRunner::run` now calls `begin_running()` before
   the first episode and notifies the observer, so `Running` is reachable from a real run
   (asserted by `the_brain_reaches_running_during_a_real_run`). The frontend lifecycle diagram
   still draws the states the read model reports.
9. **A comparison exempts the deterministic mock from the `model` check.** The mock derives its `model`
   string from its own strategy, so for two mock runs that field is a second name for `target_agent`,
   which is already a declared confound. Blocking on it would refuse the product's core capability —
   comparing two agents on one benchmark — over a difference the comparison already qualifies. The
   exemption is asserted from both sides, and `provider` still blocks a mock against a connected run.

### Security

10. The containment layer hardens a child ProbeLab has already decided to start. It is not a code sandbox:
    no syscall filtering, no filesystem or registry virtualization, no control over native code loading.
11. Linux and macOS have no OS-level containment. A spawn there fails closed rather than running
    unconfined, which means those platforms can measure nothing involving a process.
12. Provider egress is trusted adapter infrastructure. A model adapter is trusted Rust code in the
    desktop process; the design restricts its action space but does not isolate arbitrary adapter code
    from host resources.
13. A connected provider receives model-facing observations. Local-first, but disclosed.
14. The model action space has no network capability. The provider's own HTTP client does.
15. Granted AppContainer capabilities are passed through `SECURITY_CAPABILITIES` for real, but are not
    independently verified from outside the container (3.2).

### Product

16. Two of three providers are unreachable from the desktop product (2.2).
17. Three of four declared games are schema only (2.4).
18. No benchmark SDK. Adding a benchmark means implementing the trait and registering a definition; there
    is no scaffolding, template, or conformance suite for a third party.
19. The multi-episode harness measurement is shared with the product (seed pools, Wilson interval,
    success-rate reporting), but the engine's `Harness::measure` execution loop is not the loop the
    desktop uses. The lab's `LabRunner` has its own. The *method* is shipped; the *function* is not the
    same one.
20. ~~No run cancellation, no cross-process trace verifier, no API-spend or rate-limit control.~~
    Run cancellation and a total-token budget cap are implemented (`LabRunner::cancel` /
    `with_max_total_tokens`, `cancel_experiment` command, STOP control): cooperative between
    episodes with a `Cancelled` terminal reason. Still absent: a cross-process trace verifier and
    API-spend/rate-limit control beyond the token cap and per-request timeout.
21. Background runs are not recovered after a process restart, although partial telemetry and stored
    results remain durable.
22. Desktop packaging is verified by development compilation and a frontend build. Installers are not
    built or tested in CI.
23. The `ProbeRegistry` prompt-injection probe system (`crates/lab/src/probes.rs`, 3 probes) is wired
    into `LabRunner` and `LabBrain` but is never populated by the desktop, which always sends
    `probe_agents: Vec::new()`. It is library-reachable and product-unreachable.
24. ~~**The frontend has no rendering for `experiment.episodes`.**~~ The Brain panel renders a
    per-episode table (index, seed, result, event count, replay digest), and finished experiments
    export as JSON or CSV (`export_experiment`). The aggregate and interval remain the headline;
    the rows are the evidence behind it.
25. **`elapsed_ms` is measured but the read model is therefore no longer a pure function of the run's
    results.** Two executions of the identical plan differ in that one field by design, so
    `projection_is_deterministic` compares everything else and checks the duration separately. A
    deterministic read model is what lets a panel re-render from a stored run without a number changing
    underneath the reader, so this is a real cost of the field and is the reason it is called out rather
    than absorbed.
26. **Automatic execution of installed coding agents is disabled at the product boundary.**
    Discovery and preflight (read-only: binary presence, `--version`, auth-config existence) work,
    and the library executor plus its fail-closed tests remain for a future verified boundary — but
    `run_task_eval` refuses `cli:…` policies with a typed error directing to guided mode. An
    installed agent runs as the invoking user with that user's files readable and, for cloud-backed
    agents, a network path out; no disposable workdir turns that into a verified boundary, and the
    Windows containment that could restrict files cannot grant the network and auth access a cloud
    agent needs. Workdir-only execution is therefore never represented as safe isolation, and the
    product never runs agent-generated commands on an unrestricted host.
27. **Offline runtimes are verified by protocol, not by port.** A loopback TCP answer is not evidence:
    this host's port 8080 answers TCP but 404s on `/v1/models`. Discovery and preflight require a
    2xx JSON answer from `GET /v1/models` before reporting an offline executor as available. Served
    model identifiers are enumerable per runtime (`list_local_models`) for selection in the
    provider card; an empty list means nothing to select, never a guess.
28. **Windows installer bundle is produced; clean-install test is not done.** `pnpm tauri build`
    succeeds end to end on this host, emitting `target/release/bundle/nsis/ProbeLab_0.4.1_x64-setup.exe`
    (4.3 MB) alongside the release binary (16 MB) — the earlier "NSIS absent" reading was wrong: the
    Tauri bundler drives its own NSIS step with no separate install. The bundled release binary was
    launched from the desktop and renders the real UI (verified by screenshot: command center idle
    state, history empty state — both of which prove the previously-dead `brain_panel` and
    `list_experiments` commands now resolve natively). Running the setup exe (install/uninstall
    behavior) modifies the host and was not authorized, so installer execution remains unverified.
    Packaging configuration itself is complete and audited: `tauri.conf.json` version matches the
    workspace (0.4.1), all five icon sizes regenerate from `app-icon.svg` (gated), the CSP allows
    only self/ipc/dev-server origins (the frontend makes no direct model calls — all traffic goes
    through `invoke`), and the frontend license surface is `private: true` (unpublished) under the
    root MIT license with a clean `cargo deny`.
29. **Native mock-runtime IPC tests cannot execute on this host.** Any test binary linking the
    Tauri mock runtime exits with `0xC0000139` before `main` (empty stdout/stderr); identical-closure
    binaries without it run normally, no shadowing DLL was found, and loader investigation was
    stopped rather than pursued speculatively. The native suite
    (`apps/desktop/src-tauri/tests-native/native_commands.rs` — state resolution and argument
    casing through a real Tauri app) is preserved, compiles cleanly, and runs wherever the mock
    runtime loads, but it is not an auto-built target so it cannot abort this host's suite. The
    properties it pins are covered here by the compile-time state-type test and the matrix
    argument-key check. No GUI automation is possible headless either: native desktop UI testing
    is BLOCKED on this host by environment, not by product readiness.

### Closed in this cycle, previously listed above

- ~~7-8px text appears in roughly 20 CSS rules.~~ The count was 58, not 20, and the original check
  matched `px` while the stylesheet mixes `px` and `rem`, so every `rem` rule was invisible to it. A CSS
  comment asserted the floor had been applied; it had not, and a comment cannot fail.
  `apps/desktop/scripts/check-style-legibility.mjs` now enforces 11px across both units and
  `pnpm check:styles` gates on it in CI. `--faint` is also now genuinely dimmer than `--muted`.
- ~~No software version or commit is recorded.~~ See 6.3.
- ~~The benchmark version is a hand-bumped string.~~ See 6.3; a digest now blocks comparison.
- ~~Anthropic and Gemini record empty reproducibility metadata.~~ See 6.3.
- ~~Six commands resolve and dispatch.~~ `brain_panel`, `get_experiment`, `list_experiments`,
  `latest_arena_match`, `replay_arena_match`, and `get_arena_match` took `State<BrainSurface>` /
  `State<ArenaSurface>` while the application manages only `AppState`. They compiled, passed every
  test (tests call the function, never Tauri's state resolution), and failed on every invocation in
  the built application with "state not managed" — the command center's entire read path and the
  arena's read path were dead while the suite was green. All six now take `State<AppState>`, and
  `every_command_reads_managed_state_that_exists` pins each command's state type at compile time.
  Six more call sites sent camelCase top-level argument keys while the commands declare
  `rename_all = "snake_case"` (a missing argument at runtime, same green-suite/red-app shape).
  All six are fixed, and the feature-matrix checker now verifies every frontend argument key
  against the Rust signatures — demonstrated by reverting one key and watching the gate fail.

### Not implemented, and not claimed to be

Native Ollama adapter; OpenCode as a provider; Connect Four, RPS, or Battleship; a human arena seat;
reasoning, coding, safety, and injection benchmarks; Linux/macOS OS containment; auth, multi-user, and
cloud sync; additional capability dimensions; coverage percentage gates; automatic execution of
installed coding agents (refused at the product boundary — guided mode instead); installer
execution testing (bundle is produced; running setup is unauthorized).

---

## 9. Verification procedure

Run all of these. A failure in any one blocks a release.

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-targets --all-features --locked
cargo build --workspace --all-features --locked
cargo deny --log-level error check
python scripts/check-feature-matrix.py
cd apps/desktop
pnpm install --frozen-lockfile
pnpm check:styles     # no rendered font size below 11px, in px or rem
pnpm check:icons      # the platform icons match app-icon.svg
pnpm test
pnpm build
cargo test -p probelab-lab --test emit_fixture --locked   # cross-language fixtures match live serialization
```

Expected on the host named above:

| Command | Result |
| --- | --- |
| `cargo fmt --all --check` | clean |
| `cargo clippy ... -D warnings` | clean |
| `cargo test --workspace --all-targets --all-features --locked` | **831 passed, 0 failed** |
| `cargo build --workspace --all-features --locked` | success |
| `cargo deny --log-level error check` | `advisories ok, bans ok, licenses ok, sources ok` |
| `python scripts/check-feature-matrix.py` | consistent |
| `pnpm check:styles` | no font size below 11px |
| `pnpm check:icons` | 9 outputs match `app-icon.svg` |
| `pnpm test` | **251 passed** |
| `pnpm build` | success |

The last four gates are new in this cycle, and each exists because a claim of the same kind was
previously only asserted in a comment or a fixture that could not fail. A prose assertion about a
stylesheet or an icon file is a wish; a script that exits non-zero is a gate.

On Windows, additionally:

```bash
cargo test -p probelab-sandbox --locked          # 82 tests, 9 launching a real child
cargo test -p probelab-sandbox --test containment_identity --locked
```

Both CI jobs (Linux and Windows) run the full list. The Windows job also runs the containment suite,
which is the only way the OS-level claims above are checked.

**Coverage is not gated.** `cargo llvm-cov` was not available on the host this status was written on, so
no coverage figure is reported. A number that is not measured is not stated; see section 8.

### What the checks deliberately do not cover

- Whether the tests still pass — that is the test suite's job, and `check-feature-matrix.py` says so.
- Natural-language claims in prose documents. A parser that reads sentences is a fragile gate, so the
  matrix is checked against code and against explicit markers instead.
- The benchmark numbers in section 7. No test asserts them, because a test that hard-codes a measured
  score fails every time the generator changes and teaches people to update the constant rather than
  understand the change.

---

## 10. Release decision

**FEATURE COMPLETE / AUDIT COMPLETE.**

Every claim this file makes about behaviour is backed by a named test or a command in section 9. Every
limit is in section 8 rather than implied. The known limitations are real and numerous; a project does
not need every possible game, provider, or feature to be complete. It needs its claims, implementation,
tests, security model, and documentation to agree, and they do.

**Feature development is stopped.** New work is a bug fix against a claim in this file, or a correction
to a claim. `docs/feature-matrix.json` and this file are the two documents that must be updated together;
the other four are design records and are not authoritative about state.

### 10.1 What the last cycle changed about that decision

This release was previously signed off with two live defects on the security panel and a reproducibility
record that could not identify its own producer. Both are fixed. The details belong in the commit
history; what belongs here is what it means for the decision.

**A green suite was not evidence of correctness.** Every defect above passed 922 tests. The cause was the
same in all three: something was asserted in a place that could not fail — a CSS comment, a hand-written
fixture derived from the consumer's idea of the producer, an enum variant that only appeared in tests.
The suite was not weak; it was checking the wrong things and the checks were shaped so that agreeing with
each other counted as passing.

So the change is not only bug fixes. Four properties that were previously prose are now executable and
fail the build: the cross-language fixture pin, the legibility floor, the icon-freshness check, and the
vendor-scoped configuration fingerprint. Each of them is a claim that used to be made only in a comment,
and a comment cannot fail.

**The honest reading of the rest of this file is unchanged.** The known limitations are still numerous and
still real. Nothing in this cycle made the containment layer a code sandbox, brought a second provider into
the product, implemented a third game, or added a run-cancellation path. What changed is that the
reproducibility record now identifies the build and the task that produced a result, a comparison can see
which model it is ranking, a real model can be measured across a full episode pool rather than run once,
and the panel states when a sample is smaller than the plan asked for instead of presenting a small
number as a measurement of a model.

Three things a reader should not conclude:

- That a connected model has been measured. The path is proven end to end against a stand-in provider;
  no real vendor's model has been observed (section 7).
- That `elapsed_ms` did not cost anything. The read model is no longer a pure function of a run's
  results, and that is a real trade (section 8, item 25).
- That the suite is now sufficient. It caught none of the three defects above on its own. It can only be
  said to catch them now that each is reachable from a gate rather than from a reviewer's reading.

