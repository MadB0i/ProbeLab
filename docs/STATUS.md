# Status

The single source of truth for what ProbeLab does, what is verified, and what is not.

**Read this before trusting any other document here.** `README.md`, `ARCHITECTURE.md`, `SECURITY.md`,
and `PROGRESS.md` describe the design and the reasoning. This file describes the state. Where they
disagree, this file is correct and the other is stale.

- **Workspace version:** 0.4.1 (`Cargo.toml` is the authority; nothing else declares it)
- **Toolchain:** 1.97.0, pinned by `rust-toolchain.toml`
- **Licence:** MIT
- **Verified on:** Windows 11, `x86_64-pc-windows-msvc`, at commit `38f6578`
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
| 720 Rust tests | passing | `cargo test --workspace --all-targets --all-features --locked` |
| 180 frontend tests | passing | `pnpm test` in `apps/desktop` |
| Windows process containment | verified | 30 tests in `crates/sandbox/tests/`, including 9 that launch a real child under a real AppContainer identity |
| Dependency and licence policy | verified | `cargo deny check` — advisories, bans, licences, sources all `ok` |
| Wire format | pinned | `crates/lab/tests/wire_format.rs`, 21 tests against hand-written literals |
| IPC boundary | covered | `apps/desktop/src-tauri/tests/ipc_boundary.rs`, 27 tests |
| IPC command registration | verified | `every_command_the_frontend_invokes_is_registered` compares the frontend's real `invoke` calls against the real `generate_handler!` list |
| Feature matrix | verified | `python scripts/check-feature-matrix.py` |
| Adversarial self-tests | passing | `crates/lab/tests/adversarial.rs`, 24 tests including live loopback servers |

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

Both are in the built-in catalogue and reachable from the command center. Seed pools are 40 dev and 70
evaluation seeds, validated as disjoint both when the definition loads and again in the harness
constructor.

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
| Software version / commit | **no** | see limitations |
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

- **No software version or commit is recorded.** `RunMetadata.schema_version` is a schema version, not a
  build identity. A change to the engine loop or an adapter between two builds is undetectable from a
  stored bundle. This is the single largest gap in this section.
- **The benchmark "version" is a hand-bumped string**, not a digest of the definition file. Editing
  `turn_budget_numerator` in `crates/benchmarks/games/grid-world.v1.json` while leaving `"version": "2"`
  changes every episode's behaviour while every stored result still claims the old definition.
- **The configuration fingerprint does not cover the endpoint**, deliberately — an endpoint must not be
  recorded — so two runs against different servers with the same model name share a fingerprint. It is
  secret-safe and therefore not sufficient to re-run.
- **Anthropic and Gemini descriptors omit `configuration_fingerprint`, `prompt_template_version`, and
  `environment_version`**, so a run through either records those three as empty. They are also not
  selectable in the product (section 2.2), so this does not affect any shipped path — but it is a real
  gap in the adapter, not a documented limitation of the record.
- **The action trace has no observation**, so a prompt cannot be rebuilt from the result bundle. The
  per-turn observations are in the `telemetry_events` table, joined by `run_id`.
- **A stored timeline that exceeded 10 000 events is truncated** and is indistinguishable from a short
  one. The truncation is documented in `crates/lab/src/events.rs`; the limit is not encoded in the
  recording.

---

## 7. Measured results

Measured on the host named at the top, at commit `38f6578`, over the 70 held-out evaluation seeds
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

**No connected model has been measured on any benchmark.** The reference policy is a scripted searcher,
not a model. Beating it is a low bar, and a pass on a connected model would be a precondition for an
interesting result rather than a result worth reporting.

Two findings these numbers support, and one they do not:

- The greedy-to-search gap is wide and the intervals are disjoint. The gap comes from the per-seed start
  and the tight per-seed budget; an intermediate branch-free generator still showed a wide gap, so
  branching and hidden hazards add difficulty on top and are not yet shown to discriminate.
- **Memory is not load-bearing on this benchmark.** The stateless and memory-reactive policies score
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

1. No software version or commit is recorded in any result bundle (6.3).
2. The benchmark version is a hand-bumped string, not a digest of the definition file (6.3).
3. The Anthropic and Gemini adapters record an empty fingerprint and empty prompt/environment version
   (6.3). Unreachable from the product today.
4. The action trace has no observation, so a prompt cannot be rebuilt from the result bundle alone (6.3).
5. `LabEvent::detail` is truncated to 512 characters and is **not** redacted, despite a doc comment
   claiming it is. The transport redacts provider response text at source; event details are a separate
   path and have no credential filter.
6. A contained child's stdout/stderr preview is persisted unredacted (3.2).
7. An Anthropic or Gemini descriptor records `base_url` and the credential variable *name* in
   `ModelDescriptor.parameters`, which the OpenAI-compatible adapter deliberately omits. Not a secret
   value, but the two conventions disagree about what is publishable.
8. `ExperimentRecord` exists twice — `crates/lab/src/experiment.rs` (not persisted) and
   `crates/telemetry/src/lib.rs` (persisted), with different field casing. Only the second reaches
   SQLite; nothing enforces that.
9. `put_experiment`'s UPSERT omits `benchmark_id` and `episodes_requested`, so an update changing either
   is a silent no-op. `result_json` is always `None`.
10. `updated_unix_ms` is always written as `0`, so ordering stored experiments by update time
    degenerates to the `experiment_id` tiebreak.

### Security

11. The containment layer hardens a child ProbeLab has already decided to start. It is not a code sandbox:
    no syscall filtering, no filesystem or registry virtualization, no control over native code loading.
12. Linux and macOS have no OS-level containment. A spawn there fails closed rather than running
    unconfined, which means those platforms can measure nothing involving a process.
13. Provider egress is trusted adapter infrastructure. A model adapter is trusted Rust code in the
    desktop process; the design restricts its action space but does not isolate arbitrary adapter code
    from host resources.
14. A connected provider receives model-facing observations. Local-first, but disclosed.
15. The model action space has no network capability. The provider's own HTTP client does.
16. Granted AppContainer capabilities are passed through `SECURITY_CAPABILITIES` for real, but are not
    independently verified from outside the container (3.2).

### Product

17. Two of three providers are unreachable from the desktop product (2.2).
18. Three of four declared games are schema only (2.4).
19. No benchmark SDK. Adding a benchmark means implementing the trait and registering a definition; there
    is no scaffolding, template, or conformance suite for a third party.
20. The multi-episode harness measurement is shared with the product (seed pools, Wilson interval,
    success-rate reporting), but the engine's `Harness::measure` execution loop is not the loop the
    desktop uses. The lab's `LabRunner` has its own. The *method* is shipped; the *function* is not the
    same one.
21. No run cancellation, no cross-process trace verifier, no API-spend or rate-limit control.
22. Background runs are not recovered after a process restart, although partial telemetry and stored
    results remain durable.
23. Desktop packaging is verified by development compilation and a frontend build. Installers are not
    built or tested in CI.
24. The `ProbeRegistry` prompt-injection probe system (`crates/lab/src/probes.rs`, 3 probes) is wired
    into `LabRunner` and `LabBrain` but is never populated by the desktop, which always sends
    `probe_agents: Vec::new()`. It is library-reachable and product-unreachable.
25. 7-8px text appears in roughly 20 CSS rules. `--faint` is lighter than `--muted` while being used for
    strictly secondary information, inverting the intended hierarchy.

### Not implemented, and not claimed to be

Native Ollama adapter; OpenCode as a provider; Connect Four, RPS, or Battleship; a human arena seat;
reasoning, coding, safety, and injection benchmarks; Linux/macOS OS containment; auth, multi-user, and
cloud sync; additional capability dimensions; coverage percentage gates.

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
cd apps/desktop && pnpm install --frozen-lockfile && pnpm test && pnpm build
```

Expected on the host named above:

| Command | Result |
| --- | --- |
| `cargo fmt --all --check` | clean |
| `cargo clippy ... -D warnings` | clean |
| `cargo test --workspace --all-targets --all-features --locked` | **720 passed, 0 failed** |
| `cargo build --workspace --all-features --locked` | success |
| `cargo deny --log-level error check` | `advisories ok, bans ok, licenses ok, sources ok` |
| `python scripts/check-feature-matrix.py` | consistent |
| `pnpm test` | **180 passed** |
| `pnpm build` | success |

On Windows, additionally:

```bash
cargo test -p probelab-sandbox --locked          # 30 tests, 9 launching a real child
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
