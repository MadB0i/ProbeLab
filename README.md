# ProbeLab

*Workspace version 0.4.1.*

ProbeLab is a local-first AI capability and agent evaluation environment. The current vertical slice measures two honest dimensions: **grid-world planning** and **protocol adherence**. Both claims are under active scrutiny — see `docs/AUDIT-2026-09-26.md`, which found that the grid-world benchmark as shipped did not measure reasoning ability, and `ARCHITECTURE.md`, which describes the v0.5 redesign that addresses it.

**Where the benchmark stands.** The grid-world benchmark was rebuilt so that a one-step-lookahead
heuristic is beatable by a search over the agent's own observations, with confidence intervals that do
not overlap: greedy 0.286 `[0.193, 0.401]` against a scripted search reference at 0.943
`[0.862, 0.978]`, over 70 held-out seeds. **No real model has been measured on it yet.** Run
`cargo run -p probelab-engine --example grid_world_harness -- --episodes 70` to reproduce those numbers.
The desktop app still runs a single episode and shows no success rate, no interval, and no baseline.

The trusted engine owns the environment, capability broker, policy boundary, sandbox, observer, persistence, and evaluator. A connected OpenAI-compatible provider receives only the model-facing observation request; it never receives the hidden map, benchmark implementation, evaluator, filesystem paths, or persistence handles.

## Security model

Two independent layers refuse capabilities, and they are not the same thing:

- **Capability broker (primary boundary).** `crates/broker` is deny-by-default. It authorizes `filesystem.read`, `filesystem.write`, `process.spawn`, `network.connect`, and `environment.inspect` only when a benchmark's own policy grants them, with canonicalized path checks, independent read and write roots, and explicit program and endpoint allowlists. `environment.inspect` is denied unconditionally and has no grant path.
- **Capability executor (`process.spawn` only).** An authorization is not an execution. Exactly one permission has an implementation: an authorized `process.spawn` starts a child contained — suspended, inside a job object with CPU/memory/process limits, at the low mandatory integrity label `S-1-16-4096`, in a designated low-labeled directory it alone may write to, with an **empty** environment so it never inherits your API keys, and with the benchmark's declared paths made unreadable to it. On non-Windows platforms a spawn fails closed with `UnsupportedPlatform` rather than running unconfined. The other four permissions stop the run with an explicit unsupported error; none of them has a pretend implementation.

`docs/SECURITY-STATUS.md` records exactly what is enforced, what is partial, and what is deferred.

## Prerequisites

- Rust 1.97.0 through the pinned `rust-toolchain.toml`
- Node.js 20 or newer
- pnpm 9.15.4 or newer

Tauri desktop development also requires the platform WebView/toolchain packages. On Windows, the normal Rust MSVC and WebView2 dependencies are required. On Linux, install the WebKitGTK and AppIndicator development packages for your distribution before launching Tauri.

## Quick start

```text
pnpm install --dir apps/desktop
pnpm --dir apps/desktop tauri dev
```

The desktop application opens the `PROBE//LAB` command center. Select the built-in Grid World benchmark and use **Run with Mock** for a fully local deterministic run. The UI displays live post-commit telemetry, the durable action trace, and the stored result.

The database is stored in the Tauri application data directory as `probelab.sqlite3`. The exact platform path is managed by Tauri; no cloud sync or authentication is included.

## Connected provider

The connected provider is optional. Configure an OpenAI-compatible `/chat/completions` endpoint in the UI, then export the API key under the environment-variable name shown in the UI. The key is read by trusted Rust adapter code and is never accepted by IPC, displayed, logged, fingerprinted, or persisted.

PowerShell example:

```powershell
$env:OPENAI_COMPATIBLE_API_KEY = "your-key"
```

The provider performs one HTTP attempt per model call. There are no automatic retries. Temperature defaults to `0`, timeout to `30s`, and `max_tokens` to `200`. The exact prompt is versioned in `prompts/grid_world_v2.md`; changing its wording requires a prompt-version bump.

A connected run sends the versioned system prompt and the serialized model-facing request to the configured endpoint. Remote providers are not generally deterministic; mock runs remain replayable from their seed and action trace.

## Verification

```text
cargo fmt --check
cargo build --workspace --locked
cargo test --workspace --all-targets --locked
cargo clippy --workspace --all-targets -- -D warnings
pnpm --dir apps/desktop build
```

Use `--all-targets` on the test command. `cargo build --workspace` does **not** compile test targets, so
it will happily pass with a broken test in the tree; `--all-targets` is what actually builds them.

The CI workflow runs the Rust checks and the frontend production build on every push, plus the containment suite on both `ubuntu-latest` and `windows-latest`. A local desktop smoke test can be run with:

```text
pnpm --dir apps/desktop tauri dev
```

## Architecture

The runtime pipeline is:

```text
MODEL → AGENT/ADAPTER → CAPABILITY BROKER → CAPABILITY EXECUTOR → CONTAINMENT → TEST WORLD → OBSERVER → EVALUATOR
```

The provider boundary is asynchronous. The trusted engine remains synchronous and calls the async adapter through a dedicated blocking bridge. A malformed, empty, invalid, timed-out, or otherwise failed model response is a capability/protocol signal. It is not an integrity violation. `IntegrityResult` is reserved for a real refusal: a policy-gate denial or a capability-broker denial. Those two record in disjoint category sets, so a broker refusal can never inflate a policy-violation count.

Three consecutive model-call/protocol failures terminate with `model_protocol_failure`. The threshold is a documented constant, not a benchmark implementation detail.

See `ARCHITECTURE.md` for decisions and `docs/SECURITY-STATUS.md` for enforced versus deferred protections. JSON benchmark definitions live under `benchmarks/` and are embedded in the Rust binary after validation.

## Status

### Implemented

- Seeded 5×5 grid world with a per-seed start and goal, a branching safe region, hazards hidden until the agent is adjacent, a turn budget derived from each seed's own shortest safe route, and shaped reward alongside the categorical completion status
- Deterministic mock strategies plus OpenAI-compatible async provider with strict `MOVE_*` JSON protocol
- Deny-by-default capability broker as the primary capability boundary, with no grant path for `environment.inspect`
- `process.spawn` as the first genuinely executable capability: broker-authorized, contained on Windows, empty child environment, declared protected paths refused to the child, full process lifecycle telemetry. Implemented and empirically proven by `crates/engine/tests/process_spawn.rs` through a test-only benchmark; no shipped benchmark declares a spawn plan or grant yet, so this becomes load-bearing when a benchmark that genuinely needs process execution (a coding-capability benchmark, say) is built
- Versioned prompt asset, parser fence/prose tolerance, case-insensitive action names, and unknown-field rejection
- Typed model-call failures, usage aggregation, no-retry transport, and credential redaction
- Append-only SQLite telemetry, per-event commits, replay digest, and immutable result/run-metadata storage
- Independent capability/protocol adherence and integrity evaluation
- Tauri IPC with mock/connected controls and live React telemetry/action-trace rendering

### Partial

- The broker decides authorization, but only `process.spawn` is performed. Authorized filesystem and network capabilities still stop the run.
- A contained child keeps its parent's user and privileges. It is refused writes outside its designated directory and reads of the benchmark's declared protected paths, but it can still open network connections, load native code, and read anything else its user can read.
- There is no Linux or macOS containment, so a spawn on those platforms fails closed.
- Connected runs are local-first but disclose the model-facing observation to the configured provider.
- Run metadata and action traces are durable, but there is no cross-process trace verifier or remote-provider replay guarantee.
- The desktop application has no run cancellation or API-spend/rate-limit control.

### Deferred

- Executing authorized `filesystem.read`, `filesystem.write`, and `network.connect`
- Syscall-level and platform-wide OS sandboxing (seccomp/namespaces on Linux, AppContainer/restricted tokens, filesystem virtualization, network isolation)
- Ollama-specific or other provider-specific adapters beyond the OpenAI-compatible adapter
- Auth, multi-user support, cloud sync, and additional capability dimensions

## License

MIT. See `LICENSE`.
