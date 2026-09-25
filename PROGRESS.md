# Progress

## Done

- Rust 2021 workspace, pinned 1.97.0 toolchain, MIT license, and ignore/line-ending rules.
- Documented core domain contracts for model providers, agents, observations, actions, environments, episodes, benchmarks, evaluators, sandbox, policy, runtime guardian, telemetry, capability results, and integrity results.
- Seeded 5×5 grid world with a guaranteed shortest safe corridor, danger, obstacles, wait, reward, invalid transitions, and distinct turn-limit termination.
- Deterministic mock strategies: optimal, random, greedy mistake, invalid-action spam, and five independent security probes.
- Fail-closed policy/runtime authorization and non-executable denied-turn transitions.
- Per-event append-only SQLite telemetry, result repository, replay digest, live callback, and trigger hardening.
- Versioned JSON grid-world definition and static benchmark catalog.
- Adapter-neutral engine and independent grid-world evaluator.
- Tauri v2 IPC, background run lifecycle, SQLite result retrieval, live event delivery, and React command center.
- CI workflow for Rust format/test/clippy/build and frontend build.

## Verified

- `cargo build --workspace`
- `cargo test --workspace` — 50 tests passing
- `cargo clippy --workspace -- -D warnings`
- `cargo fmt --check`
- `pnpm install` in `apps/desktop`
- `pnpm build` in `apps/desktop`

## Known limitations

- No real OS/container/process/filesystem/network sandbox exists. v0 capability requests fail closed and grid actions are restricted by interface design.
- Only deterministic mock providers exist. Online and local LLM adapters are intentionally deferred.
- Capability reporting contains only `grid-world planning`.
- Background runs are not recovered after a desktop process restart; SQLite retains partial telemetry and stored results.
- The replay digest commits to semantic event content but is not a cryptographic signature or persisted trace replay verifier.
- Desktop packaging and platform-specific installers have not been fully verified beyond development compilation.

## Next

1. Add an OS-isolated sandbox and capability broker before any provider can request host resources.
2. Add persisted trace read/replay verification if replay must work across process restarts.
3. Add benchmark and environment extension tests before adding a second capability dimension.
4. Add platform packaging smoke tests for Windows, macOS, and Linux.
