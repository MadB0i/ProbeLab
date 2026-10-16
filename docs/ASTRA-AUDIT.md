# A. Executive verdict

**NO-GO for v1.0.** ProbeLab has a substantial technical foundation, but its principal promise—observable, reproducible evaluation of real models—is not reliable across the complete application workflow.

**The single biggest weakness is the connection between execution, durable evidence, and presentation.** Individual modules work well enough to pass extensive tests, while important boundaries fail:

- The UI disables single-match Arena execution when either participant is a real model.
- Arena constructs an incorrect endpoint from the default provider URL.
- Existing completed multi-episode experiments cannot be reopened.
- Older Arena recordings appear in history but cannot be loaded.
- Model-invalid moves can become excluded “no result” matches instead of contributing to the expected performance denominator.
- Live Arena events do not initialize the displayed match.

These are consequential product and measurement defects, not cosmetic objections.

### What I verified

| Previous claim | Independent conclusion |
|---|---|
| 932 passing Rust tests | **Not independently rerun.** I inspected tests and exercised focused counterexamples against existing compiled libraries. That does not establish the full current workspace result. |
| 443 passing frontend tests | **Verified:** 443 passed across 22 files, approximately 20 seconds. |
| 11/11 routes passing accessibility checks | **Not independently certified.** All 11 routes were rendered at the three requested desktop sizes; this is not equivalent to an axe, keyboard, or screen-reader acceptance pass. |
| Real Ollama integration | Adapter and discovery infrastructure exist. I did not repeat real inference. Arena’s actual UI configuration path has defects. |
| AI-vs-AI execution | Backend support exists; the single-match UI prevents selecting models and starting a match successfully. |
| Completed batch statistics and replay | Implemented, but important persistence, population-selection, provenance, and failure-accounting gaps remain. |
| Release preparation completed | Configuration and documentation exist. CI contains blocking configuration errors; installer, signing, upgrade, and production-bundle acceptance remain unverified. |

### Audit scope and preservation

I inspected Rust execution, adapters, games, scoring, persistence, sandbox boundaries, Tauri commands, React orchestration, tests, scripts, and release configuration.

I inspected the existing native WebView at **1366×768, 1600×900, and 1920×1080** across all 11 routes. Its frontend assets were development-served, so this was **native-host UI inspection, not production-bundle acceptance**. Active inference, installer behavior, and every animated gameplay state were not exercised.

No project edits, dependency installations, model downloads, installers, or Git mutations were performed. The final check found **all 342 baseline workspace files unchanged and no new non-ignored project files**.

Audit artifacts:

- [Native UI evidence](C:/Users/rupjy/.codex/visualizations/2026/10/14/01a13908-0599-7632-adc6-f574e024a973/native-ui-evidence.json)
- [Arena archive readback](C:/Users/rupjy/.codex/visualizations/2026/10/14/01a13908-0599-7632-adc6-f574e024a973/native-readback.json)
- [Experiment readback](C:/Users/rupjy/.codex/visualizations/2026/10/14/01a13908-0599-7632-adc6-f574e024a973/experiment-readback.json)
- [Counterexample output](C:/Users/rupjy/.codex/visualizations/2026/10/14/01a13908-0599-7632-adc6-f574e024a973/counterexamples.txt)
- [Preservation verification](C:/Users/rupjy/.codex/visualizations/2026/10/14/01a13908-0599-7632-adc6-f574e024a973/preservation-check.json)

# B. Confirmed bugs and vulnerabilities

“Reproduced” below means observed through the running application, read-only database inspection, or a bounded counterexample. “Source-confirmed” means the defect follows directly from the inspected code; I did not perform the potentially state-changing reproduction.

## B1. HIGH — Real-model single-match Arena execution is disabled

**Source-confirmed.**

[App.tsx:1904](D:/Projects/ProbeLab/apps/desktop/src/App.tsx:1904) supplies:

```tsx
canPlay={
  arenaTestedSeat.kind === "policy" &&
  arenaOpponentSeat.kind === "policy" &&
  arenaGames.length > 0
}
```

[ArenaPanel.tsx:983](D:/Projects/ProbeLab/apps/desktop/src/ArenaPanel.tsx:983) disables the button when `!canPlay`.

**Impact:** selecting a real model in either seat disables “Play Match.” The central advertised AI competition workflow is unreachable through this control.

**Reproduction:** discover an installed model, select it for either Arena seat, and inspect the disabled play button. No inference is necessary.

**Why tests missed it:** [ArenaPanel.test.tsx:163](D:/Projects/ProbeLab/apps/desktop/src/ArenaPanel.test.tsx:163) supplies `canPlay: true`. The component is tested independently of the actual parent condition.

## B2. HIGH — Arena adds a duplicate `/v1` to the default provider URL

**Source-confirmed; the configured input was verified through native IPC.**

[App.tsx:563](D:/Projects/ProbeLab/apps/desktop/src/App.tsx:563) unconditionally constructs:

```tsx
baseUrl: `${baseUrl || "http://127.0.0.1:11434"}/v1`
```

The observed settings already contain `http://127.0.0.1:11434/v1`. [ArenaPanel.tsx:1114](D:/Projects/ProbeLab/apps/desktop/src/ArenaPanel.tsx:1114) passes that constructed URL into the model configuration.

The adapter subsequently appends `/chat/completions` in [lib.rs:142](D:/Projects/ProbeLab/crates/model-adapters/src/lib.rs:142).

**Result:** `/v1/v1/chat/completions`.

**Impact:** fixing B1 alone will not restore the default Arena model workflow. Batch execution, whose button has a different eligibility condition, is also exposed to this configuration error.

**Reproduction:** follow the default setting through the Arena configuration builder. A provider request is unnecessary to establish the malformed endpoint.

## B3. HIGH — Completed multi-episode experiments cannot be reconstructed from storage

**Reproduced against existing data and native IPC.**

The runner produces episode results, but the command retains the complete read model in memory. Its durable experiment record stores only the scalar mean:

[brain.rs:409](D:/Projects/ProbeLab/apps/desktop/src-tauri/src/brain.rs:409):

```rust
result_json: serde_json::to_string(&experiment.mean_success_fraction).ok(),
```

[runner.rs:711](D:/Projects/ProbeLab/crates/lab/src/runner.rs:711) returns the episode result without persisting the complete experiment structure.

On reopening, [brain.rs:156](D:/Projects/ProbeLab/apps/desktop/src-tauri/src/brain.rs:156) instead looks for a result and metadata under the parent experiment ID.

Three existing records marked completed, with five completed episodes, returned `not_found` through `get_experiment`:

- `1791703511656-0000000000000001`
- `1791705125089-0000000000000001`
- `1791711627291-0000000000000001`

Their durable summary is not sufficient to reconstruct what happened.

**Impact:** a result can appear successful during execution and become unavailable after its in-memory representation disappears. Comparison and replay cannot reliably recover its evidence.

**Reproduction:** reopen one of the existing IDs above. For a future regression test, complete a multi-episode experiment, discard application state, reopen the database, and reconstruct every episode.

This is the most important durability defect in the audit.

## B4. HIGH — Older Arena recordings remain listed but fail deserialization

**Reproduced.**

[ArenaMatchView](D:/Projects/ProbeLab/crates/lab/src/arena.rs:289) requires newer fields including `testedSeat`, `opponentSeat`, `turns`, and `cancelled`.

[arena_store.rs:190](D:/Projects/ProbeLab/crates/lab/src/arena_store.rs:190) directly deserializes stored JSON without a versioned migration for older shapes.

The four newest summaries checked from the existing 24-match archive all failed to open. Their stored JSON lacks those fields. The Tauri lookup in [arena.rs:379](D:/Projects/ProbeLab/apps/desktop/src-tauri/src/arena.rs:379) collapses archive read errors into “not found.”

**Impact:**

- History advertises recordings that cannot be opened.
- Replay is unavailable for those recordings.
- Statistics silently skip unreadable matches.
- Users receive the wrong diagnosis.

**Reproduction:** list the archive, then call `get_arena_match` for one of the affected listed IDs.

A migration must preserve absence honestly. Missing historical token usage must remain unknown, not become fabricated zero usage.

## B5. HIGH — Live Arena display never initializes, and early cancellation has no target

**Source-confirmed.**

There are three related defects:

1. [App.tsx:1258](D:/Projects/ProbeLab/apps/desktop/src/App.tsx:1258) starts the asynchronous match without creating a live match representation.
2. [App.tsx:1613](D:/Projects/ProbeLab/apps/desktop/src/App.tsx:1613) discards each turn update when the current match is absent or belongs to the previous run.
3. The new match is installed only when the whole match completes.

**Impact:** the UI can retain an old board or an empty stage throughout a real-model match despite receiving turn events.

Cancellation has a similar initialization problem:

- [App.tsx:1293](D:/Projects/ProbeLab/apps/desktop/src/App.tsx:1293) silently returns when no match ID is known.
- That ID is learned from the first completed turn.
- Batch cancellation similarly returns before a batch ID is known; that ID arrives with progress after the first match.

**Reproduction:** use a delayed fake provider and press Stop before its first completed turn; for a batch, press Stop during its first match.

There is also no shared UI exclusion between single-match and batch activity. Their global event handlers can receive overlapping jobs.

**Required contract:** execution should acknowledge a job ID immediately, publish an initial state, identify every subsequent event, and define cancellation separately for queued work and an in-flight provider call.

## B6. HIGH — Invalid model actions escape normal loss accounting and lose failure evidence

**Reproduced with bounded fake-seat counterexamples; adapter-to-engine path confirmed in source.**

The adapter rejects illegal vocabulary in [game.rs:257](D:/Projects/ProbeLab/crates/model-adapters/src/game.rs:257). The model seat converts the failure into `SeatRefused` in [arena.rs:323](D:/Projects/ProbeLab/crates/engine/src/arena.rs:323).

The engine then returns through:

```rust
seat.decide(request)?
```

at [arena.rs:608](D:/Projects/ProbeLab/crates/engine/src/arena.rs:608), before recording the normal turn telemetry.

The lab projection maps this to `NoResult` in [arena.rs:934](D:/Projects/ProbeLab/crates/lab/src/arena.rs:934).

Counterexamples showed:

- Refusal on the tested seat’s first turn: `NoResult`, zero recorded turns.
- Refusal after the opponent has moved: `NoResult`, with the refusal recorded at ply zero.
- A raw illegal move from the tested seat: `Loss`.
- A raw illegal move from the opponent: `NoResult`.

**Impact:** equivalent invalid behavior receives different scoring depending on which boundary detects it and which seat performs it. Failing calls can also lose response, timing, and usage evidence.

Because `NoResult` is excluded from the win-rate denominator, a model’s difficult failures can disappear from the headline score.

**Required distinction:** model-invalid action, malformed protocol response, infrastructure failure, and user cancellation need separate classifications. Publish both conditional game results and reliability over all attempted matches.

## B7. MEDIUM — A completed batch’s statistics are replaced by archive-wide statistics

**Source-confirmed.**

[App.tsx:1355](D:/Projects/ProbeLab/apps/desktop/src/App.tsx:1355) installs the returned batch statistics. Its `finally` block immediately calls `refreshArenaStatistics()` at line 1364.

That backend query reads only the newest 200 summaries:

[arena_batch.rs:66](D:/Projects/ProbeLab/apps/desktop/src-tauri/src/arena_batch.rs:66), [arena_batch.rs:354](D:/Projects/ProbeLab/apps/desktop/src-tauri/src/arena_batch.rs:354).

Unreadable recordings are skipped, without returning their count or a truncation indicator.

**Impact:** the report a user sees after a batch can include previous matches and exclude older or unreadable ones. The displayed population no longer necessarily matches the batch they just requested.

**Reproduction:** retain prior matches for the same labels, run another batch, and compare its returned match IDs with the refreshed report population.

Keep explicit scopes: **this batch**, **selected recordings**, and **archive window**.

## B8. MEDIUM — Recording digests do not commit to the complete evidence

**Reproduced.**

[arena.rs:366](D:/Projects/ProbeLab/crates/lab/src/arena.rs:366) computes an FNV-based digest over selected concatenated fields.

It omits evidence including turn costs, response text, usage, and cancellation status. Strings are not consistently length-delimited.

Counterexamples established:

- Changing recorded elapsed time and cancellation leaves the digest unchanged.
- Agent labels `ab` / `c` and `a` / `bc` produce the same digest when the remaining fields match.

**Impact:** the digest cannot substantiate a claim that two complete recordings are identical. Statistics may differ while their purported recording identity remains unchanged.

This is **not** evidence that an external attacker bypassed SQLite protections. An attempted update against an audit-owned archive was correctly rejected by its append-only trigger.

Use versioned canonical serialization and a cryptographic digest for complete recording identity. State separately whether replay merely displays recorded frames or independently validates transitions.

## B9. HIGH — Provider response bodies are not bounded before allocation

**Source-confirmed resource-exhaustion exposure; no exhaustion attack performed.**

[lib.rs:717](D:/Projects/ProbeLab/crates/model-adapters/src/lib.rs:717) and [lib.rs:730](D:/Projects/ProbeLab/crates/model-adapters/src/lib.rs:730) call `response.text().await` before sanitizing or truncating.

The Gemini adapter has the same pattern at [gemini.rs:447](D:/Projects/ProbeLab/crates/model-adapters/src/gemini.rs:447).

**Impact:** a malfunctioning or hostile configured provider can make the application allocate a large body before the retained preview limit applies. Timeouts and requested output-token limits do not establish a response-byte limit.

**Bounded reproduction:** use a fake provider that streams beyond a proposed cap and assert that the client stops reading at that cap. Do not test by exhausting the host.

The fix should bound the received body, including chunked responses, rather than only its logged representation.

## B10. MEDIUM — Truncation before redaction leaks credential prefixes

**Reproduced using a fake credential.**

[transport.rs:339](D:/Projects/ProbeLab/crates/model-adapters/src/transport.rs:339) truncates the text before replacing the complete secret.

When a credential crosses the truncation boundary, only its prefix remains; the complete-secret replacement no longer matches.

The counterexample retained a suffix containing `AUDIT-FA` from the fake secret.

**Impact:** partial credentials can enter stored diagnostics. This does not demonstrate recovery of a complete real credential, and no real credential was used.

Redact before truncation, with tests for every boundary position. Implement the response-byte cap independently so redaction does not require accepting an unlimited body.

## B11. MEDIUM — Some saved settings do not govern subsequent runs

**Source-confirmed.**

[App.tsx:1445](D:/Projects/ProbeLab/apps/desktop/src/App.tsx:1445) loads defaults once. Saving through `SettingsPanel` does not refresh the execution state held by `App`.

The experiment request explicitly supplies:

```tsx
rootSeed: null
```

at [App.tsx:911](D:/Projects/ProbeLab/apps/desktop/src/App.tsx:911), rather than applying the saved seed behavior/root seed. The stored default run mode is likewise not consistently consumed by the launch path.

**Impact:** controls describe future-run behavior that does not reliably occur. Seed settings are especially consequential in an evaluation tool.

**Reproduction:** save a distinctive root seed or provider default, start a subsequent run, and inspect the request and resolved plan.

## B12. MEDIUM — Some execution errors leave durable lifecycle and cancellation state unfinished

**Source-confirmed.**

In [brain.rs:374](D:/Projects/ProbeLab/apps/desktop/src-tauri/src/brain.rs:374), error propagation occurs before cancellation-registry cleanup at line 390 and before terminal persistence.

The single-run command also has failure branches after persisting `Running` that emit an error without persisting the corresponding terminal lifecycle:

[commands.rs:252](D:/Projects/ProbeLab/apps/desktop/src-tauri/src/commands.rs:252), [commands.rs:343](D:/Projects/ProbeLab/apps/desktop/src-tauri/src/commands.rs:343).

**Impact:** a failed run can remain “running” until startup reconciliation later classifies it as interrupted. This misstates what happened and can retain stale cancellation entries.

Result persistence and lifecycle persistence also occur across separate operations, leaving a crash window between them.

**Reproduction:** inject planning, worker, and persistence failures after registration; assert terminal storage and cleanup on every exit.

## B13. MEDIUM — Comment-only “repairs” receive a perfect coding score

**Reproduced.**

[coding.rs:224](D:/Projects/ProbeLab/crates/tasks/src/coding.rs:224) checks markers with substring tests, alongside superficial syntax checks.

For three task seeds, prefixing every line of the reference fix with `# ` produced:

```text
fixed=true
score=1
```

The resulting Python consists entirely of comments.

**Impact:** the displayed score can be perfect without a functioning repair.

The code documents static checking; this is not a sandbox escape. It is a demonstrated limitation of what the score can claim.

Either label the metric narrowly as a static-pattern check or introduce behavioral evaluation inside a verified containment boundary.

## B14. LOW — Three-way comparison ties receive different ranks

**Reproduced.**

[comparison.rs:387](D:/Projects/ProbeLab/crates/lab/src/comparison.rs:387) uses the preceding position rather than the preceding assigned rank.

Three equal measurements yield:

```text
1, 1, 2
```

**Impact:** identical results can be presented as ordered.

Add three-way and longer tie groups, including ties after a higher-ranked group.

## B15. HIGH — The configured CI gate cannot substantiate release readiness

**Source-confirmed configuration defects; remote workflow execution was not performed.**

[ci.yml](D:/Projects/ProbeLab/.github/workflows/ci.yml) uses:

```yaml
uses: EmbarkStudios/cargo-deny-action@v2
with:
  manifest-path: Cargo.lock
```

Problems:

- `manifest-path` expects a manifest, not `Cargo.lock`.
- The action is a Docker action, but it also appears in the Windows job.
- Calling the action does not establish that `cargo deny` has been installed into the host for the following shell step.
- `--log-level error` does not promote warning-level policy findings into errors, despite the surrounding comment claiming that effect.

These conclusions are supported by the [action metadata](https://raw.githubusercontent.com/EmbarkStudios/cargo-deny-action/v2/action.yml), [GitHub’s Linux requirement for container actions](https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax), and [cargo-deny’s check documentation](https://embarkstudios.github.io/cargo-deny/cli/check.html).

**Impact:** the workflow’s documented gate semantics do not match what it executes. A collection of local green commands cannot substitute for a successfully executed release workflow.

### Security risks requiring further verification

These are **not confirmed exploits**:

- **Export-directory junctions/symlinks:** [tasks.rs:439](D:/Projects/ProbeLab/apps/desktop/src-tauri/src/tasks.rs:439) performs lexical containment checks. It does not establish filesystem containment through an existing reparse point. Exploitation would require an appropriately planted local path; model-controlled arbitrary path escape was not demonstrated.
- **Concurrent AppContainer isolation:** the shared profile identity and directory grants deserve a two-process cross-workspace isolation test.
- **Inherited handles:** verify that concurrent child launches cannot inherit unrelated inheritable handles.
- **Build provenance freshness:** [build.rs](D:/Projects/ProbeLab/crates/core/build.rs:33) needs tests covering branch-ref movement, worktrees, and untracked compiled sources. I am not labeling stale provenance as reproduced.

### Security strengths

Several boundaries are meaningfully designed:

- Legal actions are parsed into a constrained vocabulary.
- Model output is not treated as arbitrary host execution authority.
- Broker and sandbox responsibilities are separate from evaluation and presentation.
- Windows process startup uses containment checks before resuming the child.
- Credentials are referenced through environment-variable names rather than ordinary frontend secret fields.
- Production CSP excludes the development-server origin.
- SQLite uses structured access and append-only protection for Arena recordings.
- Automatic external-agent execution is explicitly unavailable in the product rather than silently running with host privileges.

I found no demonstrated sandbox escape, arbitrary SQL injection, or unrestricted model-controlled host execution.

# C. Dead code and unnecessary complexity

| Candidate | Assessment | Safe direction |
|---|---|---|
| `App.tsx`, approximately 2,986 lines | Substantial orchestration coupling. Settings, jobs, global event listeners, history, providers, and multiple execution modes share state. B1, B5, and B11 are concrete consequences. | Separate execution controllers and durable read models by workflow. This is a correctness refactor, not a stylistic one. |
| `styles.css`, approximately 8,269 lines | Large accumulated styling surface makes visual contracts difficult to maintain. | Consolidate after screen redesigns; do not rewrite the stylesheet merely to reduce line count. |
| Frontend `listProviders()` in [api.ts:80](D:/Projects/ProbeLab/apps/desktop/src/api.ts:80) | No production caller found. The backend command remains registered. | Candidate for removal or explicit compatibility documentation after checking external consumers. |
| Single-run versus multi-episode persistence | Similar user concepts have materially different durability semantics. | Establish one experiment/episode/result contract; this directly addresses B3 and B12. |
| Engine batch support versus Tauri batch orchestration | Parallel paths have differing validation and execution behavior. | Share validation and scheduling primitives while retaining UI-specific event emission. |
| Endpoint construction in different UI paths | Duplicated interpretation has already caused B2. | One canonical provider configuration boundary. |
| Separate Arena and Grid playback hooks | Some timing logic overlaps, but their frame semantics differ. | Share only genuine transport/timing mechanics; do not force both into an artificial universal player. |
| Anthropic/Gemini adapter implementations | Tested extension points, not automatically dead code because the current UI does not expose every adapter. | Retain with accurate supported-product documentation, or explicitly feature-gate. |
| Placeholder game variants | Declared possibilities are not all executable product features. | Keep them out of capability claims and user navigation until implemented. |
| Automatic CLI-agent execution machinery | Intentionally restricted product functionality, not an invitation to remove security gates. | Quarantine behind explicit capability and acceptance requirements. |
| Multiple manual CDP audit scripts | Useful evidence tools, but fragmented and not equivalent to CI acceptance. | Consolidate a native acceptance harness with one route/state inventory. |

I did **not** establish a reliable list of removable dependencies. Removing dependencies based only on search results would be unjustified.

# D. Functional gaps

1. **Arena model discovery is disconnected from the task.** The selector consumes discovered models but does not provide a complete discover/select/validate journey where the user needs it.

2. **“Connected” can overstate readiness.** The inspected provider screen showed a connected state with a placeholder model name. Configuration acceptance, server reachability, installed-model discovery, and successful inference should be separate states.

3. **Replay availability depends on current selection and readable storage.** The separate Replay route needs a recording browser and actionable explanations for incompatible recordings.

4. **“Re-executable” overstates the current replay capability.** [replay.rs:55](D:/Projects/ProbeLab/crates/lab/src/replay.rs:55) uses that label, while the inspected path constructs frames from recorded history. Playback, transition verification, and rerunning a model are different capabilities.

5. **Batch recovery lacks a complete durable job model.** Individual completed matches may survive, but the requested schedule, remaining work, cancellation reason, and resumability need explicit persistence.

6. **Cross-model comparison is deliberately refused.** [comparison.rs:493](D:/Projects/ProbeLab/crates/lab/src/comparison.rs:493) blocks different recorded providers/models. That is conservative, but it prevents a central expected use case: comparing models on the same controlled task set.

7. **Task evaluation is narrower than its presentation can suggest.** Coding is static heuristic evaluation; guided memory packages permit rereading the cue; automatic external-agent execution is unavailable.

8. **Failure explanations are not consistently preserved.** “Missing,” “unreadable schema,” “provider failed,” “model invalid,” “cancelled,” and “interrupted” need distinct durable meanings.

# E. UI/UX deficiencies and premium transformation plan

The application has a coherent dark visual language, but it does **not yet feel like premium desktop evaluation software**. Too much space is devoted to explanatory copy, repeated containers, and configuration. The actual experiment is often visually secondary.

Observed strengths:

- Consistent navigation and broad styling vocabulary.
- No horizontal content overflow at the three requested desktop sizes.
- Clear attempts to distinguish policies, models, and missing evidence.
- Existing focus styling and reduced-motion rules.

Observed weaknesses:

- Small uppercase labels and dense monospace compete with important information.
- Empty stages occupy substantial space without directing the next action.
- Catalog and analysis screens reuse similar card treatments despite different tasks.
- Technical policy identifiers are exposed where human-readable names are needed.
- Configuration appears repeatedly across readiness, providers, settings, and Lab.
- History is difficult to scan and compare.

Actual captures: [Lab](C:/Users/rupjy/.codex/visualizations/2026/10/14/01a13908-0599-7632-adc6-f574e024a973/native-lab-1366.png), [Arena](C:/Users/rupjy/.codex/visualizations/2026/10/14/01a13908-0599-7632-adc6-f574e024a973/native-arena-1600.png), [Readiness](C:/Users/rupjy/.codex/visualizations/2026/10/14/01a13908-0599-7632-adc6-f574e024a973/native-readiness-1600.png), [route contact sheet](C:/Users/rupjy/.codex/visualizations/2026/10/14/01a13908-0599-7632-adc6-f574e024a973/route-contact-sheet.png).

### Screen-by-screen priorities

| Screen | Evidence or deficiency | Recommended change | Impact / effort |
|---|---|---|---|
| Readiness / first launch | “2/9 ready” emphasizes missing optional systems; next-step guidance favored installing another runtime despite Ollama being ready. | Lead with the usable path: “Ollama available → choose installed model → first experiment.” Put optional integrations below. | Very high / medium |
| Lab / Command Center | Three competing columns, prominent mock execution, dense configuration, and an empty observation area. | One experiment workspace with a compact setup bar, dominant stage, evidence inspector, and integrated history. | Very high / high |
| Arena | Large empty stage, long internal identifiers, crowded seat headings, setup separated from the visual subject. | Board-first composition, concise seat cards, immediate job state, and meaningful turn evidence. | Very high / high |
| Grid-World | The available empty state does not explain what observation will appear or how to start. Active animation quality was not independently exercised. | Treat the grid as the main instrument, with step-linked observation and action evidence. | Very high / medium–high |
| Comparison | Many small cards with repeated “no recorded plan identity” warnings; poor scanability. | Filterable experiment table, explicit eligibility reasons, then a focused comparison workspace. | High / medium |
| Replay | Large empty panel with disabled loading and no direct archive browser. | Recording picker, compatibility status, timeline, board, and selected-turn evidence in one screen. | High / medium |
| Tasks | Long explanatory text followed by package generation and manual JSON entry. | A guided package → external attempt → import → score flow, with scoring limitations attached to the result. | High / medium |
| Providers | Configuration fills the width; “connected” does not communicate distinct readiness stages. | Compact provider cards with endpoint, reachability, installed models, and last verified inference. | High / medium |
| Benchmarks | Definitions and implementation facts dominate the catalog. | Lead with capability measured, expected duration, difficulty, and evidence produced. Expand implementation details on demand. | Medium / medium |
| Agents | Repetitive infrastructure entries look like a product catalog of models. | Group reference policies, adversarial probes, and model adapters; expose technical IDs secondarily. | Medium / low–medium |
| Security | Large empty area and repeated explanations. | Show boundary inventory, probe coverage, latest evidence, and a clear distinction between blocked attempts and breaches. | Medium / medium |
| Settings | Unequal columns, substantial unused space, and controls with unreliable execution effects. | Group actual user decisions; show whether changes apply immediately or to the next run. | Medium / low after B11 |
| History and batch reports | Nested scrolling and ambiguous report population. | Shared searchable history; batch-specific report heading, manifest, sample count, exclusions, and archive scope. | High / medium |

### Proposed screen 1: Arena

**Layout**

- Compact top bar: game, two participant selectors, seat order, primary Start/Stop.
- Approximately 60–65% of the content width for the board.
- Remaining width for a selected-turn inspector.
- Participant cards directly above the board, each separating friendly model name, model/policy type, seat, and actual status.
- Timeline immediately below the board.
- Batch and archive views in a lower panel or tab, rather than permanently competing with live play.

**Visual style**

Use a distinctive measurement-instrument identity: restrained blue accents, warm opposing-seat colors, crisp geometry, and readable sentence-case text. Reserve monospace for coordinates, seeds, IDs, and measured values.

Connect Four benefits from a restrained recessed board and clear disc depth. Tic-Tac-Toe benefits from excellent spacing, large marks, and strong last-move emphasis. Neither needs a 3D engine.

**Interactions and animation**

- Disc placement: short, interruptible 160–240 ms motion.
- Last move: transient outline, followed by a stable marker.
- Winning line: subtle emphasis with a textual outcome.
- Provider wait: real elapsed time and “awaiting response,” not simulated thought.
- Reduced motion: immediate placement and static emphasis.
- Selecting a turn reveals the actual request, returned action, validation result, latency, and reported usage.

**Remove or simplify**

Internal identifiers as headings, repeated empty telemetry messages, large permanent explanatory paragraphs, and redundant framing around the board.

### Proposed screen 2: Lab / Command Center

**Layout**

- Top: a concise experiment summary—benchmark, model, dataset/seed plan, episodes, estimated scope.
- Center: active environment or experiment overview.
- Right: current episode and evidence inspector.
- Bottom: episode strip and event timeline.
- History in a collapsible drawer or dedicated table, not a competing setup column.

**Interaction**

A clear sequence: **choose capability → choose model → review plan → run → inspect result**.

Reference policies should be clearly identified as calibration tools. They should not visually impersonate the primary real-model workflow.

During execution, show completed/failed episodes and current operation. On completion, retain the same workspace and replace progress with the recorded result; do not make users reconstruct the journey across unrelated routes.

**Visual style and motion**

Use larger outcome typography, restrained progress transitions, and consistent spacing. Animate changes in recorded state, not decorative background machinery.

**Remove or simplify**

Duplicate provider settings, repeated “what this panel does” text, multiple competing run controls, and technical metadata that can move into an inspector.

### Proposed screen 3: Grid-World observation workspace

**Layout**

- Main grid occupying roughly two-thirds of the workspace.
- Side-by-side inspector sections for observation, selected action, and resulting transition.
- Persistent episode/step controls below.
- Optional overlays for visited cells, legal moves, and the recorded path.

**Scientific honesty**

Separate **what the agent observed** from **what the evaluator knows**. Any omniscient world view must be explicitly labeled and supported by recorded data.

Do not draw an inferred intention, search tree, or belief state unless the system actually recorded it.

**Visual style and motion**

Use clean 2D tiles, modest elevation for walls, a distinct goal marker, and a visible agent token. Show movement and failed-action feedback briefly. Avoid camera rotation and decorative particles.

**Remove or simplify**

Empty dashboard furniture, repeated legends, and timelines detached from the selected grid frame.

### Accessibility and desktop ergonomics

The source includes global reduced-motion clamping, but checking a media query is not proof that playback, auto-advance, and all active states are accessible.

Required acceptance should include:

- Keyboard-only discovery, launch, cancellation, history selection, and replay.
- Visible focus after route changes and asynchronous completion.
- Screen-reader announcements that identify meaningful state changes without reading every telemetry event.
- Non-color identification of seats, outcomes, and errors.
- 125% and 150% Windows scaling, not only viewport dimensions.
- Disabled-control explanations adjacent to the control.
- Pointer-target checks including the full label hit area; a small checkbox alone is not sufficient evidence of a target-size failure.

### A credible 30-second demo

1. Select an already installed local model.
2. Show one actual decision in Arena or Grid-World.
3. Inspect its recorded action and cost.
4. Scrub the corresponding replay.
5. Compare against a reference or paired run.

A prepared recording is appropriate if clearly labeled **Replay**. Do not hide unreliable live behavior behind a demo that appears live.

# F. Benchmark and research credibility

**Yes: a user can currently obtain an impressive score that is misleading or insufficiently supported.** B6 and B13 are direct demonstrations.

### Methodological strengths

- Explicit reference policies provide useful calibration.
- Legal-action validation creates a clear protocol boundary.
- Unknown token usage is represented separately from reported usage.
- Security probes distinguish attempted requests from successful boundary violations.
- Comparison has explicit compatibility checks rather than indiscriminately ranking everything.
- Recording seeds and benchmark identities is directionally correct.

### Principal weaknesses

**1. Different seed labels do not guarantee independent tasks.**

Tic-Tac-Toe and Connect Four start from the same board. The inspected policy paths are deterministic, and the Tauri seat builder supplies seed zero.

A bounded Connect Four check with seeds 7, 8, and 9 produced the same 15-ply sequence and outcome.

Repeated deterministic games can test repeatability, but they do not become independent strategic samples because their metadata contains different seeds. A narrowing Wilson interval over such repeats does not establish broad game ability.

**Better design:** fixed, versioned opening-position suites; balanced seats; difficulty strata; paired comparisons on the same positions.

**2. Arena grouping uses display labels as identity.**

[arena_stats.rs:255](D:/Projects/ProbeLab/crates/lab/src/arena_stats.rs:255) groups by game and participant labels. Those labels do not identify weights, quantization, endpoint implementation, generation parameters, or adapter version.

A changed model under the same tag can silently join previous results.

**Better design:** immutable configuration fingerprints with human-readable labels layered above them.

**3. Failure exclusion biases interpretation.**

A win rate conditional on valid completion can be useful. It is misleading when presented without the rate of protocol failures, invalid moves, infrastructure failures, and cancellations.

Report attempted matches and mutually exclusive terminal categories before conditional performance.

**4. Coding repair does not establish executable correctness.**

The comment-only counterexample is decisive. Static markers may serve as a tutorial or parser exercise; they are not a meaningful coding benchmark.

**5. Guided “memory” permits retrieval.**

[runner.rs:166](D:/Projects/ProbeLab/crates/tasks/src/runner.rs:166) exposes both `CUE.md` and `CHOICE.md` in the package. A participant can reread the cue.

That measures instruction-following and retrieval under the package workflow, not retention after the cue is unavailable.

**6. Cross-model comparison needs controlled variation, not blanket refusal.**

Holding task set, scoring version, prompt protocol, and resource budget constant while varying the model is a valid intended experiment. The comparison layer needs to distinguish that intended independent variable from uncontrolled differences.

**7. Replay fidelity and reproducibility are different.**

Recorded-frame playback establishes what was stored. Transition verification establishes whether the stored game is internally consistent. Reproducing inference additionally requires model and provider provenance and may remain nondeterministic.

These should be separate claims.

**8. Security-probe success does not measure general model safety.**

A blocked capability request demonstrates boundary enforcement for that probe. It does not establish broad resistance to prompt injection or comprehensive sandbox isolation.

**Research verdict:** useful experimental infrastructure, but not yet a research-grade evaluation system. Narrower, explicitly stated claims would substantially improve credibility.

# G. Performance opportunities

### Observed or established

- The frontend suite completed in approximately 20 seconds.
- Requested desktop layouts showed no horizontal overflow.
- Focused debug-library Connect Four policy matches took roughly 10 seconds each in the audit harness. This is not a release-build UI benchmark.
- HTTP bodies are unbounded before preview truncation: B9.
- Experiment listing retrieves the entire table in [telemetry/lib.rs:1006](D:/Projects/ProbeLab/crates/telemetry/src/lib.rs:1006).
- Arena statistics deserialize up to 200 complete recordings rather than querying a dedicated summary representation.

### Source-based concerns requiring measurement

| Concern | Evidence | Targeted response |
|---|---|---|
| Memory grows across many runs | [RecentEventBuffer](D:/Projects/ProbeLab/apps/desktop/src-tauri/src/lib.rs:91) caps each run at 256 events, but its map has no overall run bound in the inspected implementation. | Bound retained run count or evict terminal runs once durable storage is available. |
| Frontend event retention | [App.tsx:1516](D:/Projects/ProbeLab/apps/desktop/src/App.tsx:1516) retains per-run pending telemetry. | Define lifecycle-based eviction and test many sequential runs. |
| Repeated telemetry copying/sorting | [App.tsx:317](D:/Projects/ProbeLab/apps/desktop/src/App.tsx:317) rebuilds a map and sorts accumulated events on updates. | Measure long episodes; use ordered incremental storage if the profile justifies it. |
| Broad rerender scope | Central orchestration and derived metrics live in `App`. | Profile before/after isolating active workflow state. |
| Uncoordinated provider concurrency | Single and batch controls are not mutually exclusive. | A provider-aware queue with explicit concurrency and cancellation policies. |
| History scaling | Entire experiment lists and large DOM collections. | Pagination/filtering first; virtualization only when measured scale warrants it. |
| Slow deterministic policy batches | Repeated identical games and debug search cost. | Avoid redundant trials; profile release search before changing algorithms. |

I did not measure cold startup, a sustained memory leak, GPU utilization, or 16 GB host exhaustion. Those remain acceptance tasks, not observed failures.

For a 16 GB PC, predictable scheduling and bounded responses matter more immediately than speculative UI micro-optimizations.

# H. Test-suite weaknesses

The important failures survive because the tests often stop at module boundaries.

| Gap | Why existing green tests are insufficient | Test that exposes it |
|---|---|---|
| Parent-to-Arena eligibility | Panel tests inject `canPlay: true`. | Render the application workflow, select a discovered model, and assert launch eligibility. |
| Provider URL contract | Configuration and endpoint construction are tested separately. | Feed the actual default settings through the Arena picker into adapter endpoint construction. |
| Restart durability | In-memory success does not prove reconstruction. | Complete multiple episodes, destroy state, reopen storage, and verify complete results and provenance. |
| Historical schema compatibility | Current-format round trips do not test old recordings. | Load fixtures from every supported persisted version. |
| Live events | Final-match rendering can pass while intermediate events are dropped. | Send start, first turn, duplicate turn, reordered turn, and terminal events through the application controller. |
| Early cancellation | A cancellation token test does not prove that the UI knows the job ID. | Delay first provider response and cancel before any turn or batch progress. |
| Error classification | Parser and engine tests can each be correct locally. | Drive malformed and illegal model outputs through adapter → engine → projection → statistics. |
| Statistical population | Aggregator tests do not cover UI replacement of batch results. | Run two synthetic batches and assert the selected report contains exactly the requested batch IDs. |
| Digest completeness | Stable digest tests may only verify repeatability. | Mutate every evidence-bearing field; require identity changes or document intentional exclusions. |
| Credential redaction | Testing complete secrets inside short strings misses truncation boundaries. | Place a fake credential across every cap boundary. |
| Response limits | A bounded retained preview does not imply bounded reading. | Stream beyond the cap and assert early termination and bounded buffering. |
| Coding scoring | Reference fix versus broken program misses nonprogram marker matches. | Comments, strings, unreachable code, and incorrect programs containing all required markers. |
| Ranking | Two-item ties miss the defect. | Three or more equal values and multiple tie groups. |
| Architecture/security tests | Source-name assertions establish structure, not runtime isolation. | Real adversarial boundary tests, including concurrent contained children. |
| Native release acceptance | Browser/development frontend checks omit packaged CSP, assets, and WebView behavior. | Launch the packaged production build with isolated test data and execute the core journey. |

The frontend run also emitted a React `act(...)` warning in a TasksPanel test. That is secondary to the integration gaps, but it should not become normalized test noise.

# I. New product ideas

Ranked for practical value and reuse of the existing architecture:

| Rank | Proposal | Problem solved and capability measured | Difficulty / dependencies | Timing |
|---|---|---|---|---|
| 1 | **Portable experiment evidence bundle** | Lets users share and inspect a result with its plan, model identity, scoring version, events, exclusions, and recording hash. Makes reproducibility tangible. | Medium–high; requires B3/B4/B8 and a versioned export/import format. | Minimal form in v1.0 |
| 2 | **Paired local-model comparison** | Compares models on identical task instances and budgets; measures capability differences without confounding task selection. | Medium–high; experiment manifests, fingerprints, paired analysis, clear missing-result handling. | v1.1 |
| 3 | **Strategic position suite** | Replaces repetitive empty-board matches with tactical and planning challenges: forced wins, defense, traps, and constrained horizons. | Medium; reuse game engines and reference policies; curate/version positions. | v1.1 |
| 4 | **Evidence-based failure explorer** | Groups failures by illegal action, protocol error, repeated state, wasted turn, or missed tactical opportunity. Helps developers understand actual behavior. | Medium; normalize failures and link them to recorded turns. Explanations must cite evidence. | Basic version in v1.0 |
| 5 | **Robustness matrix** | Tests stability across prompt paraphrases, equivalent action encodings, irrelevant context, and repeated trials. Measures sensitivity rather than a single lucky score. | Medium; controlled variants and manifest support. | v1.1 |
| 6 | **Partial-observation planning challenges** | Extends Grid-World with hidden information, delayed cues, limited observations, and resource constraints. Measures planning and memory under enforceable rules. | High; observation-boundary enforcement and strong reference baselines. | Later |
| 7 | **Declarative benchmark authoring with validation** | Lets contributors create meaningful environments without granting arbitrary host execution. Expands community contribution safely. | High; bounded schema, deterministic validation, baseline checks, and versioned scoring contracts. | Later |

The most distinctive combination is **observable local execution + portable evidence + paired comparisons**. It aligns with the existing architecture and gives users something valuable to publish and reproduce.

# J. Final prioritized roadmap

## 1. Must fix before v1.0

1. Restore the real-model Arena path: B1 and B2.
2. Make experiment and episode persistence reconstructable: B3 and B12.
3. Migrate supported historical Arena records without inventing missing data: B4.
4. Introduce immediate job identity, correct live initialization, event filtering, and cancellation semantics: B5.
5. Normalize invalid-action/protocol/infrastructure outcomes and retain failing-call evidence: B6.
6. Make batch scope explicit and preserve its exact population: B7.
7. Establish honest recording identity and model/configuration provenance: B8 and F.
8. Bound provider response bodies and fix redaction order: B9 and B10.
9. Apply saved execution settings correctly: B11.
10. Correct or narrowly relabel coding and memory metrics.
11. Repair CI and execute the relevant gates.
12. Validate the production-packaged application and historical-data upgrade path.

## 2. High-value v1.0 polish

- Ready-provider-first onboarding.
- Readable model names and clear model/policy distinctions.
- Board-first Arena and observation-first Lab layouts.
- Searchable history with actionable incompatible-recording messages.
- Batch-specific headings and visible exclusions.
- Keyboard, scaling, reduced-motion, and screen-reader acceptance.
- Three-way rank correction.
- Compact failure explorer with links to recorded evidence.

## 3. Worth adding in v1.1

- Paired local-model comparison.
- Strategic position suites.
- Robustness matrices.
- Durable batch resumption.
- Richer evidence exports and comparisons.
- Performance work guided by realistic long-session profiles.

## 4. Future experimental ideas

- Partial-observation planning and memory tasks.
- Declarative benchmark authoring.
- Sandboxed tool-use challenges after containment and scoring are independently validated.
- Independent replay transition verification.

## 5. Features not worth building now

- Decorative 3D boards or cinematic camera systems.
- Fabricated “AI thought” visualizations.
- More games without a capability hypothesis and calibrated task distribution.
- Large leaderboards before provenance and comparability are reliable.
- A plugin marketplace before benchmark contracts stabilize.
- More provider logos before the default local-model journey works.
- Automatic host-level agent execution that bypasses current restrictions for convenience.

# K. Final recommendation

**NO-GO for a public v1.0 release.**

The current project is suitable for continued development and a clearly labeled technical preview with limited claims. It is not ready to promise durable, reproducible AI evaluation as a finished product.

Explicit release gates:

1. **Core journey:** discover an installed model, run model-vs-policy and model-vs-model, observe turns, cancel early, and receive an accurate terminal result.
2. **Durability:** reopen multi-episode results after process restart; preserve episode evidence and resolved configuration.
3. **Upgrade compatibility:** supported historical recordings remain readable, or receive an explicit migration/compatibility explanation.
4. **Measurement integrity:** invalid actions, protocol failures, infrastructure failures, and cancellations have tested denominators and retained evidence.
5. **Provenance:** results identify the actual model/configuration, task set, prompt/scoring versions, and application build.
6. **Security:** response-byte caps, boundary-safe redaction, and relevant concurrent containment checks pass.
7. **CI:** the corrected Linux and Windows workflows execute successfully; previously reported totals are reproduced against the release candidate.
8. **Packaged UI:** the production bundle passes native workflow and accessibility acceptance.
9. **Packaging:** clean install, upgrade, uninstall, data preservation, and WebView requirements are tested on an isolated Windows environment.
10. **Distribution:** version metadata is consistent—currently it is `0.5.0`—and artifacts have verified hashes and provenance.

Code signing provides publisher verification and a better Windows installation experience. If a certificate is unavailable, an explicitly unsigned preview with checksums is a possible distribution choice; it is not equivalent to signed-release verification. No installer or signing checks were performed in this audit.

**The priority is to make execution, evidence, and displayed conclusions agree. Once that is reliable, the proposed visual redesign can turn a technically substantial project into a convincing product.**