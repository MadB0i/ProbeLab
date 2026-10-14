<p align="left">
  <img src="assets/probelab-lockup-reversed.svg" alt="ProbeLab" width="420">
</p>

# ProbeLab

A desktop workstation for running AI models and agents against fixed tasks and benchmarks, observing what they actually do, and replaying the result — locally, with no account and no cloud.

<p align="left">
  <a href="https://github.com/MadB0i/ProbeLab/releases/latest"><img src="https://img.shields.io/github/v/release/MadB0i/ProbeLab?style=flat-square&label=Download" alt="Download latest release"></a>
  <a href="https://github.com/MadB0i/ProbeLab/releases/latest"><img src="https://img.shields.io/github/downloads/MadB0i/ProbeLab?style=flat-square&label=Downloads" alt="Total downloads"></a>
  <img src="https://img.shields.io/badge/platform-Windows-0078D4?style=flat-square&logo=windows&logoColor=white" alt="Windows">
  <img src="https://img.shields.io/badge/license-MIT-blue?style=flat-square" alt="MIT licensed">
</p>

## Demo

Recorded from the built Windows executable. The benchmark run, the match, and the playback are the engine's real work.

![ProbeLab demo](assets/media/probelab-demo.gif)

## What it does

<p align="center">
  <img src="assets/media/command-center.png" alt="Command Center: run context, execution controls, live agent progress, and stored runs" width="820">
</p>

<p align="center">
  <img src="assets/media/arena-match.png" alt="Arena: a completed Tic-Tac-Toe match with the winning line and full match statistics" width="820">
</p>

<p align="center">
  <img src="assets/media/arena-playback.png" alt="Arena playback: stepping through the positions the engine recorded, mid-match" width="820">
</p>

<p align="center">
  <img src="assets/media/readiness.png" alt="Readiness: preflight checks with the next blocking step and its remedy" width="820">
</p>

## Features

- **Benchmark execution** — runs a selected model or agent against an embedded benchmark catalog, with a live progress feed and the run's event stream, and a **STOP RUN** control that ends the run at a committed turn boundary rather than killing it.
- **Grid-world navigation trace** — a partially observable 5×5 grid drawn turn by turn. The board shows only what the agent has actually sensed; tiles it has never seen stay marked as never seen, and no frame is drawn from terrain the run did not record.
- **Durable run history** — completed runs persist to SQLite and reopen after restart, with lifecycle state, episode counts, and scores. A recorded run reopens from disk with a full transport: play, step, and a scrubber.
- **Match arena** — plays two policies against each other through the engine and records every position, with frame-by-frame playback, a scrubber, and a winning line from the engine.
- **Agent and benchmark registries** — browsable catalogues of what the product can run and what each entry measures, with honest completion state per entry.
- **Task packages** — save a guided task to disk as a self-contained, portable package you can open and run elsewhere.
- **Comparison** — selects two stored runs and states, in plain language, why they may or may not be comparable.
- **Integrity reporting** — protocol compliance and action legality reported as two separate rates from one record, because a model can answer every turn in perfect protocol and still have most of its moves refused. Both are shown; neither is derived from the other.
- **Security sweep** — probes the execution boundary itself and reports what was blocked as a passing result, separately from what a model attempted.
- **Readiness preflight** — checks the machine before a run and names the specific blocking step and its fix.
- **Replay** — re-reads recorded run events without contacting anything.

## Execution modes

| Mode | What runs | Status |
|---|---|---|
| Baseline policy | A deterministic built-in policy. No network, no key, no model. | Supported, default |
| Connected model | An OpenAI-compatible `/v1/chat/completions` endpoint over HTTPS or loopback. | Supported |

The baseline mode is a real execution path, not a placeholder: the benchmark definition, engine, scoring, and telemetry are identical, and the run is labelled with the policy it used.

## Limitations

These are the actual boundaries of the current build, stated plainly.

- **No vendor model has been measured.** The connected path has been driven end to end through the built application against a local `qwen2.5:0.5b` over loopback and against a controlled slow provider. That the transport speaks the right protocol is not the same claim as a benchmark result from a specific vendor's model.
- **One provider is selectable.** The OpenAI-compatible adapter is the only one the desktop product admits. Anthropic and Gemini adapters exist as library artifacts with unit tests but are not wired into the product.
- **One Arena game.** Tic-Tac-Toe only. Connect Four, RPS, Battleship, and a human seat are declared in the feature matrix as not implemented.
- **Replay of a stored run shows measurements and provenance, not the original planner's decisions or live timeline.** Those were never persisted; the panel leaves them absent rather than reconstructing them.
- **Not independently audited.** The security model is designed and enforced in-process; it has not been through an external review, and no independent security guarantee is claimed.
- **No scientific validation.** Benchmarks are internally consistent and reproducible from a recorded seed. Nothing here has been validated as a measure of model capability in the research sense.
- **No published release.** Nothing has been signed, tagged, or published, so there is no download to verify. See [docs/RELEASE-CHECKLIST.md](docs/RELEASE-CHECKLIST.md) for exactly which release steps remain and what each is waiting on.

## Install

**There is no published release.** No installer has been generated, signed, or published, and this
repository deliberately has no automation that would do any of those things without a person deciding
to.

To build one yourself:

```powershell
pnpm --dir apps/desktop tauri build
```

which produces `target/release/bundle/nsis/ProbeLab_<version>_x64-setup.exe` and the MSI alongside it.
Record what you built and verify it later with:

```powershell
node scripts/release-provenance.mjs write artifacts/provenance.json target\release\probelab-desktop.exe
node scripts/release-provenance.mjs verify artifacts/provenance.json
```

No installation is required to build or run from source — see below.

## Build from source

Requires [Rust](https://www.rust-lang.org/tools/install) (edition 2024, Rust 1.97+), Node.js 20+, and the WebView2 runtime (present by default on Windows 11).

```bash
git clone https://github.com/MadB0i/ProbeLab.git
cd ProbeLab
cargo test --workspace          # Rust engine, lab, sandbox, broker, telemetry
pnpm --dir apps/desktop install
pnpm --dir apps/desktop test    # Frontend
pnpm --dir apps/desktop tauri build
```

The build produces `target/release/probelab-desktop.exe` plus NSIS and MSI bundles under `target/release/bundle/`.

### Layout

| Path | Contents |
|---|---|
| `crates/engine` | Deterministic game and environment logic |
| `crates/lab` | Experiment orchestration, SQLite persistence, Arena store |
| `crates/sandbox` | Process isolation and the execution boundary |
| `crates/broker` | The single sanctioned path from the UI to an execution |
| `crates/telemetry` | Append-only event recording |
| `apps/desktop` | Tauri shell and React frontend |
| `scripts` | Capture, audit, and release verification scripts |

The Rust crates are the product; the desktop shell is one front end over them.

### Verification

Everything below runs on a normal development build and is enforced on every commit by
`.github/workflows/ci.yml`.

| Check | What it actually verifies |
| --- | --- |
| `cargo test --workspace` | 932 tests across the engine, lab, sandbox, broker, telemetry and the desktop command surface |
| `pnpm --dir apps/desktop test` | 443 frontend tests |
| `pnpm --dir apps/desktop check:styles` | no font size below 11px, **and** every text colour clears 4.5:1 against every surface in the palette |
| `node apps/desktop/scripts/check-tauri-config.mjs` | the packaging configuration matches the Tauri schema that ships in this repository, and the shipped CSP carries no development origin |
| `python3 scripts/check-version-provenance.py` | the workspace manifest, the Tauri config, the frontend manifest, and `docs/STATUS.md` all state the same version |
| `python3 scripts/check-feature-matrix.py` | every registered Tauri command is reachable from the frontend, and every capability claim matches what is wired |

Beyond the build gates, `scripts/` contains the checks that drive the **real executable** over the
DevTools protocol, because several of the properties that matter cannot be observed from a test harness:

- `grid-trace.mjs` — a live run and a recorded replay, with board geometry measured at 1366×768, 1600×900 and 1920×1080.
- `cancel-through-ui.mjs` with `slow-provider.mjs` — presses STOP against a provider slow enough to stop, then checks that the committed events are the only ones persisted and that a second run starts immediately.
- `replay-integrity.mjs` — compares an in-page content digest against the engine's own replay digest, frame by frame and across a real process restart, and checks that no frame paints terrain the run did not record.
- `grid-durability.mjs` — terminates the process, restarts it against the same database, and drives the scrubber.
- `audit-a11y.mjs` — axe-core plus focus visibility, accessible names, `aria-hidden` traps, clipping and horizontal overflow, across **all eleven routes**, and a reduced-motion check measured in both directions.

## License

[MIT](LICENSE)