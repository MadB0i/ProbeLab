# ProbeLab architecture

## Trust boundary

The runtime data flow is:

`MODEL → AGENT/ADAPTER → POLICY GATE → SANDBOX → TEST WORLD → OBSERVER → EVALUATOR`

A model receives only a `ModelRequest` containing its current turn and a local `Observation`. It never receives benchmark definitions, the complete map, evaluator logic, expected routes, SQLite handles, filesystem paths, or security telemetry. `Action` has no filesystem, process, network, or evaluator variant. Security requests can be represented as non-executable `RequestedCapability` values so policy denial is observable and testable.

A connected provider is trusted infrastructure code, not a model capability. The adapter may make outbound HTTP to its configured API endpoint; the model's action space still has zero network access. A denied request consumes a trusted turn-budget transition without an action outcome or world movement.

## Crate boundaries

- `core`: serializable domain values and explicit sync/async traits; no game or persistence implementation.
- `agent`: synchronous `ProviderAgent` and `AsyncProviderAgent` blocking bridge. The bridge owns a dedicated Tokio runtime and never exposes provider internals to the model.
- `model-adapters`: deterministic mocks, strict action protocol parser, prompt asset, and OpenAI-compatible async adapter.
- `sandbox`: seeded grid generation, transition rules, deny-by-default policy, and the interface-restricted sandbox adapter.
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

## v0.2 security posture

Policy and runtime guardian interfaces fail closed. Filesystem and network isolation is enforced by omission in the model action interface, not by an OS sandbox. The connected adapter's outbound HTTP is trusted provider infrastructure and is intentionally outside the sandbox action space. Real process, container, filesystem, and network isolation remain future work and are recorded in `docs/SECURITY-STATUS.md`.
