# ProbeLab architecture

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

The configuration fingerprint hashes only model name, temperature, max tokens, prompt template version, and benchmark version. API keys, environment-variable names, endpoints, headers, raw request bodies, and response bodies are excluded.

The replay digest hashes a semantic projection of observations, decisions, transitions, rewards, integrity events, and protocol-failure kinds. It excludes timestamps, run IDs, response text, request IDs, elapsed time, and token usage. Mock runs with the same seed and strategy remain field-identical; remote-provider runs are not assumed reproducible.

The protocol-failure threshold is `PROTOCOL_FAILURE_THRESHOLD = 3` consecutive failures. Protocol failures populate `CapabilityResult.protocol_adherence`; `IntegrityResult` remains zero unless the policy gate denies a modeled capability.

## Benchmark definitions

Benchmark files use JSON because the workspace already depends on `serde_json` for telemetry and IPC. Definitions are versioned, embedded in the Rust binary, and validated before execution. The model prompt is a separate versioned artifact under `prompts/`; prompt changes require a version bump rather than silent in-place edits.

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

`process.spawn` is the only permission that runs anything, and it is the permission that makes the
containment layer a real boundary rather than a library nothing calls.

A model can ask for the `process.spawn` **permission** and nothing more. The request the broker
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
  not handed to a process one argument away from exfiltrating them.

An authorized spawn consumes a turn without moving the world and records no integrity event: the
broker working correctly is not a violation. A refused spawn is a non-executable turn recorded in
`broker_process_spawn_blocks`. Every requested spawn records a `ProcessLifecycle` with the requested
flag, the authorization, whether a child spawned, the PID, the exit code, the containment state, and
the termination reason. The replay projection deliberately excludes the PID, exit code, and CPU time
so a mock run's digest stays reproducible.

The other four permissions are unchanged: they are authorized or refused by the broker and then
stop the run with `CapabilityExecutionUnsupported`. None of them was given a premature implementation.

## v0.2 security posture

Policy and runtime guardian interfaces fail closed. Filesystem and network isolation is enforced by omission in the model action interface, not by an OS sandbox. The connected adapter's outbound HTTP is trusted provider infrastructure and is intentionally outside the sandbox action space. Real process, container, filesystem, and network isolation remain future work and are recorded in `docs/SECURITY-STATUS.md`.

## v0.3 Windows containment

`crates/sandbox/src/containment.rs` adds an OS-level layer *below* the capability broker. The broker still decides what is authorized and is the primary boundary; containment only hardens a process that trusted code already decided to start. It is not a code sandbox: there is no syscall filtering, no filesystem or registry virtualization, and no network restriction. A contained child is still a normal Windows process with the same user and privileges.

On Windows, `RestrictedProcessGuard` starts a child in this order: create the job object, create the child with `CREATE_SUSPENDED`, assign it to the job, set the child's primary token to the low mandatory label `S-1-16-4096` with `SetTokenInformation(TokenIntegrityLevel, ...)`, then resume it. The job carries a per-job CPU time limit, a per-process CPU time limit, per-process and whole-job memory limits, an active-process limit, `JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION`, and `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, so dropping the guard terminates everything still in the job. Every step is checked: a failure terminates the still-suspended child and returns a typed `ContainmentError` rather than resuming it with a normal token.

`SandboxDirectory` creates the one directory a contained child may write in, with the documented SDDL `D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;OW)S:(ML;OICI;NW;;;S-1-16-4096)`: a protected DACL plus a low mandatory label. Everything around it keeps the default medium label, and mandatory integrity control refuses a low-integrity write into a higher-labeled object. Preparation fails closed when the label, the DACL, or the parent's label does not match.

On Linux and macOS every containment entry point returns `ContainmentError::UnsupportedPlatform`. The `windows-sys` dependency is target-specific, so the non-Windows dependency graph contains no Windows bindings.
