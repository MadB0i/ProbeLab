# ProbeLab

ProbeLab is a local-first AI capability and agent evaluation environment. The current vertical slice measures one honest capability dimension: **grid-world planning**, including model protocol adherence.

The trusted engine owns the environment, policy boundary, sandbox, observer, persistence, and evaluator. A connected OpenAI-compatible provider receives only the model-facing observation request; it never receives the hidden map, benchmark implementation, evaluator, filesystem paths, or persistence handles.

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

The provider performs one HTTP attempt per model call. There are no automatic retries in v0.2. Temperature defaults to `0`, timeout to `30s`, and `max_tokens` to `200`. The exact prompt is versioned in `prompts/grid_world_v1.md`; changing its wording requires a prompt-version bump.

A connected run sends the versioned system prompt and the serialized model-facing request to the configured endpoint. Remote providers are not generally deterministic; mock runs remain replayable from their seed and action trace.

## Verification

```text
cargo fmt --check
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
pnpm --dir apps/desktop build
```

The CI workflow runs the Rust checks and the frontend production build on every push. A local desktop smoke test can be run with:

```text
pnpm --dir apps/desktop tauri dev
```

## Architecture

The runtime pipeline is:

```text
MODEL → AGENT/ADAPTER → POLICY GATE → SANDBOX → TEST WORLD → OBSERVER → EVALUATOR
```

The provider boundary is asynchronous. The trusted engine remains synchronous and calls the async adapter through a dedicated blocking bridge. A malformed, empty, invalid, timed-out, or otherwise failed model response is a capability/protocol signal. It is not an integrity violation. `IntegrityResult` is reserved for a policy-gate event that actually stopped a disallowed capability.

Three consecutive model-call/protocol failures terminate with `model_protocol_failure`. The threshold is a documented constant, not a benchmark implementation detail.

See `ARCHITECTURE.md` for decisions and `docs/SECURITY-STATUS.md` for enforced versus deferred protections. JSON benchmark definitions live under `benchmarks/` and are embedded in the Rust binary after validation.

## Status

### Implemented

- Seeded 5×5 grid world with safe corridor, danger, obstacles, wait, and turn limit
- Deterministic mock strategies plus OpenAI-compatible async provider with strict `MOVE_*` JSON protocol
- Versioned prompt asset, parser fence/prose tolerance, case-insensitive action names, and unknown-field rejection
- Typed model-call failures, usage aggregation, no-retry transport, and credential redaction
- Append-only SQLite telemetry, per-event commits, replay digest, and immutable result/run-metadata storage
- Independent capability/protocol adherence and integrity evaluation
- Tauri IPC with mock/connected controls and live React telemetry/action-trace rendering

### Partial

- Connected runs are local-first but disclose the model-facing observation to the configured provider.
- Run metadata and action traces are durable, but there is no cross-process trace verifier or remote-provider replay guarantee.
- The desktop application has no run cancellation or API-spend/rate-limit control.

### Deferred

- Real OS/container/process/filesystem/network sandboxing
- Ollama-specific or other provider-specific adapters beyond the OpenAI-compatible adapter
- Auth, multi-user support, cloud sync, and additional capability dimensions

## License

MIT. See `LICENSE`.
