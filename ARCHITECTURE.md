# ProbeLab architecture

## Trust boundary

The runtime data flow is:

`MODEL → AGENT/BOT → POLICY GATE → SANDBOX → TEST WORLD → OBSERVER → EVALUATOR`

A model receives only a `ModelRequest` containing its current turn and a local `Observation`. It never receives benchmark definitions, the complete map, evaluator logic, expected routes, or persistence handles. `Action` has no filesystem, process, network, or evaluator variant. Security requests can be represented as non-executable `RequestedCapability` values so policy denial is observable and testable.

## Crate boundaries

- `core`: serializable domain values and explicit traits; no game or persistence implementation.
- `agent`: owns a model provider and turns only observation data into model decisions.
- `model-adapters`: deterministic mock providers; future online providers remain replaceable behind `ModelProvider`.
- `sandbox`: seeded grid generation, transition rules, deny-by-default policy, and the interface-restricted sandbox adapter.
- `telemetry`: independent observer and append-only SQLite recorder/result repository.
- `evaluator`: independently converts trusted environment outcomes and integrity events into results.
- `benchmarks`: versioned JSON definitions and benchmark registry.
- `engine`: deterministic orchestration without UI concerns.
- `apps/desktop/src-tauri`: Tauri IPC and background run lifecycle only.

## Decisions

### JSON benchmark definitions

Benchmark files use JSON because the workspace already depends on `serde_json` for telemetry and IPC. Definitions are versioned, embedded in the Rust binary, and validated before execution.

### `rusqlite` and append-only storage

`rusqlite` keeps the local-first storage path compact and synchronous. SQLite runs in WAL mode with bundled SQLite for portable builds. Every telemetry event is committed before notification is emitted. Database triggers reject `UPDATE` and `DELETE` against the telemetry table, while the public `TelemetryRecorder` interface only exposes `record`. A separate result repository stores immutable final results.

### Determinism

All benchmark randomness uses `ChaCha8Rng` seeded from explicit integers. A 5×5 map is generated with one guaranteed shortest monotonic corridor plus seeded danger and obstacle placement. Observations reveal only current and immediately adjacent terrain, so the optimal mock does not require hidden map access. `TestResult` contains no wall-clock timestamp or ephemeral run identifier. Its replay digest hashes canonical deterministic event content, not telemetry timestamps.

### Evaluation and integrity

Capability metrics come from environment state and transitions, never model reports. Integrity counters are separate and include unauthorized access, policy, sandbox, environment tampering, and network violation attempts. Security probes are denied and cannot reach the grid transition implementation.

### v0 security posture

Policy and runtime guardian interfaces fail closed. Filesystem and network isolation is enforced by omission in the action interface, not by an OS sandbox. Grid actions cannot request those capabilities. Real process, container, filesystem, and network isolation remain future work and are recorded in `docs/SECURITY-STATUS.md`.
