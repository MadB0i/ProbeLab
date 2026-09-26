# Progress

## Done

- Rust 2021 workspace, pinned 1.97.0 toolchain, MIT license, and CI for Rust/frontend verification.
- Documented core domain contracts for sync/async model providers, agents, observations, actions, environments, episodes, benchmarks, evaluators, sandbox, policy, runtime guardian, telemetry, capability results, integrity results, and reproducibility metadata.
- Seeded 5×5 grid world with safe corridor, danger, obstacles, wait, reward, invalid transitions, distinct turn-limit termination, and non-mutating protocol-failure turns.
- Deterministic mock strategies: optimal, random, greedy mistake, invalid-action spam, and five independent security probes.
- Fail-closed policy/runtime authorization and non-executable denied-turn transitions.
- Deny-by-default capability broker (`crates/broker`) as the primary v0.3 boundary: `filesystem.read`, `filesystem.write`, `process.spawn`, `network.connect`, and `environment.inspect`, with canonicalized path checks, independent read/write roots, explicit program and endpoint allowlists, and an unconditional `environment.inspect` denial that has no grant path.
- Value-free typed broker denials that consume a turn with no action, movement, or reward, are recorded as non-executable telemetry, and count only in five integrity counters disjoint from the five policy-gate counters.
- Async OpenAI-compatible adapter with env-only credentials, HTTPS/loopback validation, no redirects/retries, typed failures, usage metadata, strict `MOVE_*` JSON protocol, and versioned `prompts/grid_world_v1.md`.

- Append-only SQLite telemetry, semantic replay digest, immutable result/run-metadata storage, token aggregation, and secret-redaction tests.
- Adapter-neutral engine, independent grid-world evaluator, protocol adherence classification, and `model_protocol_failure` after three consecutive model-call failures.
- Tauri v2 IPC with mock/connected provider controls, provider preflight, blocking worker execution, live post-commit telemetry, durable action trace, and React command center.
- `process.spawn` wired as the first genuinely executable capability: the broker reviews the benchmark's trusted plan, an unauthorized spawn never reaches containment or starts a process, and an authorized spawn launches contained on Windows with the full lifecycle recorded.
- Path protection that makes the benchmark's declared paths unreadable to a contained child, using a read-up refusal on the path's existing integrity level, verified by a real child process, reversible on drop, and recoverable after a hard kill from an on-disk journal.
- A contained child starts with an empty environment, so it never inherits the host's provider credentials.
- Windows OS-level defense-in-depth containment in `crates/sandbox`, below the broker: children are started suspended inside a job object with per-job and per-process CPU limits, per-process and whole-job memory limits, an active-process limit, and `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`; the primary token is set to the low mandatory label `S-1-16-4096` before the child is resumed; a designated sandbox directory is labeled low and protected so a contained child can write only there. Non-Windows platforms return `UnsupportedPlatform` and a spawn fails closed. This is not a code sandbox.


## Verified

- `cargo fmt --check`
- `cargo build --workspace --locked`
- `cargo test --workspace --locked` — 212 tests passing
- `cargo clippy --workspace --all-targets -- -D warnings`
- `pnpm install` in `apps/desktop`
- `pnpm build` in `apps/desktop`
- Local wiremock provider tests cover success, usage, timeout, HTTP error, malformed/empty/invalid output, taxonomy, empty credential, and redaction.
- Broker tests cover per-permission allow/deny, independent read and write roots, canonicalization and traversal refusal, the unconditional `environment.inspect` denial, denial telemetry validation, and an end-to-end forbidden-action-spam run whose integrity counts are asserted against recorded denial telemetry.
- Windows host: `cargo test -p probelab-sandbox --locked`, including the real low-integrity child escape test (`%ComSpec%` writes one file inside the designated directory and is refused outside it), the job-object limit read-back, the CPU-limit kill, and the drop-kills-child check. All 10 containment tests pass.
- `cargo fmt --all`, `cargo test --workspace --locked`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo build --workspace --locked` pass on a Windows host, including the Tauri desktop crate.


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

1. Expose an authorized `filesystem.read` to a contained child through the same executor, so the child can be given a read-only view of a granted root while the evaluator stays refused.
2. Add a restricted-token or AppContainer layer so a contained child stops sharing the parent's user and groups.
3. Add persisted trace read/recompute verification and explicit run cancellation/time budgets.
4. Add a second benchmark only after the capability/integrity classification remains stable.



## Known limitations

- A model adapter is trusted Rust code in the desktop process. Interface design restricts the model-facing action space but does not isolate arbitrary adapter code from host resources.
- The Windows containment layer hardens a child ProbeLab already decided to start. It is not a code sandbox: no syscall filtering, no filesystem or registry virtualization, no network restriction, and the child keeps the parent's user and privileges.
- Remote provider responses are not deterministic even at temperature zero; mock runs are the replayability baseline.
- Background runs are not recovered after a desktop process restart, although partial telemetry and stored results remain durable.
- Desktop packaging and platform-specific installers have not been fully verified beyond development compilation and frontend build.
