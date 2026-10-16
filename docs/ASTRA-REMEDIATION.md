# Astra audit — remediation closure report

Working record of audit findings from [`ASTRA-AUDIT.md`](ASTRA-AUDIT.md): what has been independently
verified, what has been fixed with a regression test, and what is still open.

**This is not a release sign-off.** It records the state of remediation work in progress. Nothing here has
been signed, packaged, installed, or published, and no installer has been generated.

Branch: `dev/probelab-v1`.

**Checkpoint.** This work is committed as a single checkpoint. The commit is a save point for work in
progress, **not** a completed B6 fix, and the NO-GO verdict below is unchanged by it.

### Where this checkpoint actually stands

| | State |
|---|---|
| **B3** multi-episode experiment reconstruction | **CLOSED** — product-level proof passes |
| **B11** saved settings do not govern subsequent runs | **CLOSED** — fixed because it blocked B3 |
| **B1, B2, B5** Arena eligibility, `/v1` duplication, live init and early cancellation | **CLOSED** |
| **B9, B10** bounded provider reads, redaction before truncation | **CLOSED** |
| **B6** failure accounting | **BLOCKED** — implemented and unit-tested; **no product-level proof** (response delivery unresolved) |
| **B4, B7, B8, B12–B15** | **OPEN** |

Two things must not be misread from this commit:

1. **B6 is not closed.** The accounting is fixed at the engine, lab, archive and panel layers with 19
   tests, but no batch command has yet returned a report through the real application, so the claim has
   never been observed end to end. See the native batch diagnosis below.
2. **`run_arena_batch` is unresolved.** It completes all of its work and never returns to the caller.
   The last successful boundary is `worker-joined`; the fault is after the await, in report assembly.

Also still missing: **test-storage isolation**. `app.path().app_data_dir()` resolves through the Windows
known-folder APIs and ignores `APPDATA`/`LOCALAPPDATA`, so scripted runs open the developer's real
archive. Until that is fixed with a test-only override, no scripted product test here is reproducible or
non-destructive.

---

## Status at a glance

| Finding | Severity | State |
|---|---|---|
| **B1** real-model Arena execution disabled | HIGH | **CLOSED** — regression tested |
| **B2** duplicated `/v1` on the default endpoint | HIGH | **CLOSED** — regression tested |
| **B5** live match never initialises; early cancellation has no target | HIGH | **CLOSED** — verified from the UI against a 9s provider |
| **B9** provider response bodies unbounded before allocation | HIGH | **CLOSED** — bounded streaming read, 8 tests |
| **B10** truncation before redaction leaks credential prefixes | MEDIUM | **CLOSED** — redacted before truncation, boundary sweep |
| **B3** multi-episode experiments cannot be reconstructed | HIGH | **CLOSED** — persistence tested; 4-episode run survives process restart, compared per-episode over IPC |
| **B4** historical Arena recordings fail to deserialize | HIGH | OPEN |
| **B6** invalid model actions escape loss accounting | HIGH | **PARTIAL** — engine/lab/store fixed and tested; IPC + panel surface added and tested; **product-level proof BLOCKED** (see below) |
| **B7** batch statistics replaced by archive-wide statistics | MEDIUM | OPEN |
| **B8** recording digest does not commit to the evidence | MEDIUM | OPEN |
| **B11** saved settings do not govern subsequent runs | MEDIUM | **CLOSED** — fixed because it blocked B3's product proof; 3 tests |
| **B12** failed runs leave lifecycle/cancel state unfinished | MEDIUM | OPEN |
| **B13** comment-only repairs receive a perfect coding score | MEDIUM | OPEN |
| **B14** three-way comparison ties receive different ranks | LOW | OPEN |
| **B15** the configured CI gate cannot substantiate release readiness | HIGH | OPEN |

**5 of 15 closed, 1 partial.** A NO-GO assessment is unchanged; see the end of this document.

---

## B3 — multi-episode experiment reconstruction — **CLOSED**

### The defect, confirmed

`brain.rs` wrote `result_json: serde_json::to_string(&experiment.mean_success_fraction)` — one number.
Every episode, decision, failure, timeline entry and security total lived only in the running process.
`get_experiment` rebuilt a read model by looking for a stored result under the run identifier; a
multi-episode run writes no such single result, so the lookup found nothing and answered "not found".

### What is fixed and verified

* `crates/lab/src/view.rs` gains `StoredExperimentDocument` — a **versioned** envelope holding the whole
  `ExperimentView`. The read model is persisted rather than a bespoke subset, so there is one definition
  of what an experiment contains.
* `brain.rs` writes the document instead of the scalar.
* `load_stored_experiment` reads three shapes explicitly: a current document (restored in full), a
  **historical scalar** (reconstructed on the old narrower path and *labelled* as such), and anything
  unrecognisable (refused with a reason, never guessed).
* `BrainReadModel::from_experiment_view` restores the experiment **exactly**. Findings and signals are
  empty rather than reconstructed — they were never persisted, and inventing them would be a second
  unaudited opinion about the run.
* Historical rows are **not rewritten and not upgraded**. A scalar row still reconstructs one episode,
  and says so in `next_intent`, because that is all the row supports.

**Evidence:** `crates/lab/tests/experiment_reconstruction.rs` — 7 tests, built on the **real** planner,
runner and evaluator over the real grid benchmark, not a hand-assembled fixture:

| Test | Establishes |
|---|---|
| `a_completed_multi_episode_experiment_survives_a_restart_in_full` | a new repository handle over the same file recovers all five episodes |
| `the_restored_experiment_is_field_for_field_the_one_that_was_persisted` | a round trip adds, drops and alters nothing |
| `the_read_model_is_rebuilt_from_the_whole_persisted_experiment` | the read model comes from the document, with `observed_events` from the persisted timeline |
| `episode_indices_survive_the_round_trip` | `[0, 1, 2, 3, 4]` preserved |
| `a_historical_scalar_row_is_still_recognised` | `"0.75"` and `"null"` still classify as legacy |
| `an_unknown_document_version_is_refused_rather_than_parsed_optimistically` | version 99 is refused, not downgraded to a scalar |
| `unrecognisable_bytes_are_reported` | garbage is reported as garbage |

### The product-level proof — CLOSED

`scripts/experiment-restart-reconstruction.mjs` drives the **real release executable**: it sets the
episode default through the Settings UI, starts a multi-episode experiment from the Lab page, terminates
the process, relaunches against the same database, reopens the run, and reads it back over **native IPC**
(`get_experiment`) as well as the UI.

Passing evidence from the final run:

```
PASS  the episode setting mounted — present
PASS  the settings page shows the episode default the next run will use — shows 4, asked for 4
PASS  the experiment was started from the Lab page — clicked
PASS  the run reached a terminal state — terminal — Episodes 4 / 4 Score 50.0%
PASS  the run reported 4 episodes
PASS  the finished experiment is readable over native IPC — 4 episodes, lifecycle completed
      before restart: {"experimentId":"1792152007129-…","episodes":4,"lifecycle":"completed",
                        "terminalReason":{"kind":"completed"},"successRate":0.5,
                        "seeds":[2000,2001,2002,2003],"rootSeed":0}
=== 2. The process is terminated ===
=== 3. A new process reopens the same run ===
      after restart:  {"experimentId":"1792152007129-…","episodes":4,"lifecycle":"completed",
                       "terminalReason":{"kind":"completed"},"successRate":0.5,
                       "seeds":[2000,2001,2002,2003],"rootSeed":0}
PASS  the same experiment is readable after restart — 1792152007129-… === 1792152007129-…
PASS  every episode survived the restart — 4 before, 4 after
PASS  each episode's own fields match after the restart — 4 episodes compared on index, seed,
      outcome, replay digest and event count
PASS  each episode kept a distinct seed and replay digest — seeds [2000,2001,2002,2003]
PASS  the aggregate survived the restart — success rate 0.5, mean turns 7.25
PASS  the root seed survived the restart — root seed 0
PASS  provenance survived the restart — 5 recorded decisions, agent optimal, model native-mock-optimal
PASS  the lifecycle timeline survived the restart — 16 timeline entries
PASS  the terminal state survived the restart — lifecycle completed, terminal {"kind":"completed"}
PASS  the reopened run reconstructed MORE THAN ONE episode — 4 episodes after restart
```

The comparison is by **experiment id**, not by list position, and every check is over a non-empty set.
Both of those guards exist because the earlier version of this script produced convincing-looking passes
that meant nothing:

* **Comparing renderings proves little.** The first version scraped "1 completed of 1" from the page before
  and after. That is two renderings of the same summary. The proof now reads the reconstructed
  `ExperimentView` over IPC and compares per-episode `index`, `seed`, `taskSuccess`, `replayDigest` and
  `telemetryEventCount` individually.
* **A vacuous pass is indistinguishable from a total failure.** With the argument name wrong,
  `get_experiment` returned an object with no episodes, so "no episode mismatches" was **true** — the
  checks reported success for a reconstruction that produced nothing. Every episode check now requires at
  least three episodes on both sides.

### Four harness defects that were masquerading as product defects

Each of these produced a confident, wrong conclusion. They are recorded because the failure mode — a
green or red result that describes the test rather than the product — is the thing this report exists to
prevent.

1. **Navigating before the document exists.** The DevTools endpoint answers with a page *before* the
   WebView finishes loading. Setting `location.hash` on that document is discarded when the real document
   loads; `location.hash` reads back as `""`. The script believed it had opened Settings and waited eight
   seconds for a control that was never going to appear. Now waits for `.app-shell`.

2. **Reading `disabled` in the same expression that edited the field.** React had not re-rendered when
   `dispatchEvent` returned, so the observed `disabled` was the *previous* render — where the draft still
   matched the stored document — and the click landed on a disabled button that ignores it.
   Interaction and observation must be separate round trips.

3. **Asking for a value that was already stored.** The default is five; the script asked for five. The
   draft genuinely did not differ, Save was correctly disabled, and the script reported a broken form —
   while the panel was displaying "Saved." and the document header read "CUSTOMISED · SCHEMA V1", which
   is the evidence that an earlier write had landed. The script now reads the stored value first and, if
   it is already the target, says so instead of failing. It types with **trusted CDP keystrokes** rather
   than synthetic events, because React's value tracker can see an assigned value as no change at all.

4. **Starting the wrong run, and counting a disabled button as running.** "RUN WITH MOCK" is a
   single-episode benchmark run; "RUN EXPERIMENT" is the multi-episode path. The script started the
   former, so every result read "1 completed of 1" no matter what was fixed underneath. Separately, the
   STOP control is always rendered and merely *disabled* when idle, and the loop matched its label
   without checking `disabled` — so it reported "still running" for six hundred seconds about a run that
   finished in about three, which had frozen a one-episode partial and reopened it as a tidy experiment.

### A build trap that silently disables every product-level test

`cargo build --release -p probelab-desktop` produces a binary that **ignores the embedded frontend and
tries to load `http://localhost:1420`**, so every scripted test sees `ERR_CONNECTION_REFUSED` against a
page with no application on it. Release binaries need Tauri's `custom-protocol` feature, which
`tauri build` passes and plain `cargo build` does not:

```powershell
cargo build --release -p probelab-desktop --locked --features tauri/custom-protocol
```

`docs/RELEASE-CHECKLIST.md` documents `pnpm tauri build`, which is currently **blocked on this host** by a
dependency mismatch — the CLI is `tauri@2.12.3` while the installed `@tauri-apps/api` is `2.11.1`, and
the CLI refuses to build with mismatched Tauri packages. Recorded as evidence for **B15**. No installer
was produced.

**One correction to an earlier claim in this report.** I stated that `api.ts` sent `experimentId` to a
command requiring `experiment_id`, and that opening a stored run from the history table was therefore
broken in the product. That was wrong. `api.ts` already sends `experiment_id`, with a comment explaining
that Tauri resolves top-level argument names exactly. Only the test script used the wrong key. A sweep of
all 40-odd `invoke` call sites found no other case of this.

### Residual, not fixed

`experimentDefaults.seedBehaviour` is still not sent to `run_experiment`, because the request contract has
no field for it. The stored value governs new runs for `episodes` and `rootSeed` only. Recorded rather
than papered over with a cast.

---

## Closed findings

### B1 — Real-model single-match Arena execution is disabled — **FIXED**

**Verified.** `App.tsx` computed:

```tsx
canPlay={arenaTestedSeat.kind === "policy" && arenaOpponentSeat.kind === "policy" && arenaGames.length > 0}
```

which requires **both** seats to be reference policies. Selecting a discovered model for either seat
disabled Play Match, making the advertised model workflow unreachable.

**Fixed by** extracting the rule into [`arenaEligibility.ts`](../apps/desktop/src/arenaEligibility.ts) and
replacing the inline expression. The rule now permits model-vs-policy and model-vs-model, refuses a seat
playing itself, and — deliberately — only requires a usable provider endpoint when a model is actually
seated, so the reference-policy calibration workflow still runs without one configured.

Every refusal now carries a sentence, rendered beside the disabled control in a new
`.arena-setup__blockers` list. A disabled button that explains nothing is a control the operator has to
reverse-engineer.

**Evidence:** [`arenaEligibility.test.ts`](../apps/desktop/src/arenaEligibility.test.ts) — 11 tests,
including the three configurations that were unreachable. Two existing test files supplied the new
required prop, and their comment now records that `canPlay: true` was what hid this.

### B2 — Arena adds a duplicate `/v1` to the default provider URL — **FIXED**

**Verified.** The persisted provider default is `http://127.0.0.1:11434/v1`
(`crates/lab/src/settings.rs`), the UI loads it into `baseUrl`, and `App.tsx` built the Arena model
configuration as `` `${baseUrl || "http://127.0.0.1:11434"}/v1` ``. The adapter then appends
`/chat/completions`, so the composed request was `/v1/v1/chat/completions`.

**Fixed by** [`providerEndpoint.ts`](../apps/desktop/src/providerEndpoint.ts), one canonical boundary used
by both callers. `/v1` is appended only when the configured URL has no path; a URL that already carries a
path is preserved, because a reverse-proxied root is a legitimate configuration. A URL the operator typed
badly is **not** silently rewritten — repairing it would hide a typo that would otherwise surface as a
clear 404 from their own endpoint.

**Evidence:** [`providerEndpoint.test.ts`](../apps/desktop/src/providerEndpoint.test.ts) — 15 tests,
including a composed-endpoint assertion that no supported input can produce `/v1/v1`.

### B5 — Live Arena display never initialises; early cancellation has no target — **FIXED**

**Verified.** Three linked defects:

1. `playArena` awaited the whole match before installing any view.
2. The match identifier was learned from the **first completed turn**.
3. The live board was built by folding turns into a match that had to already exist, so every turn
   arriving before the terminal event was discarded.

A match against a real model spends a full provider round trip on its opening call, so for that entire
window there was no identity to stop and no board to draw.

**Fixed by** publishing the identity *before* execution begins:

- A new `ARENA_MATCH_STARTED_EVENT` (`probe-lab/arena-match-started`) carries the identifier and the
  **engine's own opening board**, emitted by `play_arena_match` immediately after the run is registered.
  `crates::lab::empty_board` was made public rather than duplicated in the command layer.
- It is deliberately **not** on `ARENA_MATCH_EVENT`: that channel carries terminal recordings, and a batch
  plays matches through the same engine, so a shared channel let a batch's match install itself as the live
  single-match view.
- Turn and terminal events are now **correlated** against the match the panel is following
  (`shouldInstallArenaResult`).
- A **duplicate ply replaces** its earlier entry instead of being appended, so a redelivered event cannot
  inflate the turn count against the board drawn beside it.
- Stopping before an identity is known now **says so** instead of silently returning, for both a single
  match and a batch.
- Starting a new match clears the previous view immediately rather than leaving a stale board on screen.

The rules were extracted into [`arenaLiveController.ts`](../apps/desktop/src/arenaLiveController.ts) so
they are testable where they are not buried in a 3,000-line component.

**Evidence:** [`arenaLiveController.test.ts`](../apps/desktop/src/arenaLiveController.test.ts) — 15 tests
covering start-init, correlation, duplicates, ordering, and terminal acceptance.

---

## End-to-end regression test

[`scripts/arena-model-journey.mjs`](../scripts/arena-model-journey.mjs) closes the seam the audit
identified as untested: the panel and the adapter were each tested correctly in isolation, and both defects
lived between them.

It drives the **real release executable** over the DevTools protocol and asserts, in order:

1. The endpoint and model name are entered into the real provider card.
2. The product's **own** discovery control (`LIST MODELS`) is pressed and the model is discovered.
3. The discovered model appears in the seat picker.
4. Selecting it leaves **Play Match enabled** with no blocker (B1).
5. The composed endpoint carries exactly one `/v1`, and the composed chat-completions URL is one the
   provider actually answers (B2).
6. The stage is **live while the match runs** — observed during, not inferred after (B5).
7. The provider was called repeatedly, so the match resolved nowhere else.
8. The match is archived, names the model, and reached a **decided outcome from real plies**.

**Latest run:** `WIN · connect_four · 19 plies · seed 7 · journey-test-provider vs first_legal_connect_four`
· 11 provider calls · all checks passing.

Supported by [`scripts/controlled-model-server.mjs`](../scripts/controlled-model-server.mjs), a loopback
OpenAI-compatible stand-in that answers discovery and reads `legal_moves` out of the request.

**Three findings about the test itself, recorded because each produced a false PASS first:**

- A **verdict is not a result.** An early version accepted any verdict string; a stand-in that could not
  play returned `NO RESULT` with zero plies and satisfied it.
- The **archive row lags** by design, so reading it during a match reports the *previous* match's row.
- The stand-in answered in ways the engine correctly **refused** — first a grid-world action name, then
  always column one until it filled — both producing `NO RESULT` at six plies. The cause was that the
  `legal_moves` array arrives **backslash-escaped inside a JSON string**, and the first bare `COLUMN_n` in
  the body belongs to the system prompt's worked examples, not the payload. Only reading the array *by
  name* after unescaping fixed it. A stand-in that cannot read its request now says so rather than guessing.

---

## B5 re-verification — early cancellation, driven from the UI

The first pass closed B5 on the strength of a start event existing. That is inference, not evidence, and
the instruction was right to challenge it: an identity published before execution is only useful if a
stop sent during a provider round trip actually lands.

[`scripts/arena-early-cancel.mjs`](../scripts/arena-early-cancel.mjs) drives the real release executable
against a stand-in configured with a **9,000 ms** per-response delay, so the opening call is genuinely
outstanding when STOP is pressed.

| Check | Result |
|---|---|
| STOP has a target before the first turn exists | **PASS** — provider calls at the moment of the press: **1** |
| The opening board is drawn before any turn | **PASS** — 42 cells, `arena-board--idle` absent |
| STOP pressed from the real interface | **PASS** |
| The match reached a terminal state | **PASS** |
| No "not available yet" notice | **PASS** |
| A durable terminal recording exists | **PASS** — `NO RESULT · 1 plies · early-cancel-test-model` |
| Provider calls after the stop | **1 → 1** — no further requests |
| A second match starts and reaches the provider | **PASS** — `1 then 2` |

**B5 is CLOSED.** The stop landed while the first response was still outstanding, produced a durable
recording, released the handle, and left the runner able to start a second match.

Two notes on how this test was reached, because both first produced a **false failure**:

- The cell selector searched for `button`/`li`/`[data-cell]`; the cell class is `arena-cell`. The count
  was zero, which read as "no board was drawn" and reported the identity fix as unproven — while the same
  run had a live STOP button on screen.
- The real distinction the audit asked about is now measured directly: **provider calls were 1 when STOP
  was pressed**, so the first request was still in flight. That is the claim, and it is asserted.

Note on classification: the cancelled match surfaces as `NO RESULT`. The durable `cancelled` flag is set
on the recording, but the archive *summary* does not distinguish a stop from a seat refusal or an
infrastructure failure. That is a real gap and it belongs to **B6**, which is open.

## B9 and B10 — provider response limits and credential redaction — **CLOSED**

Both were verified against the code before being changed.

### B9 — unbounded response bodies

`response.text().await` read a body to completion and allocated all of it before any bound applied. The
retained-preview limit bounded what was *kept*, not what was *allocated*. Three call sites were affected:
the OpenAI-compatible adapter, Gemini, and **Anthropic** — the audit named two, and the third was found
by searching for the pattern rather than trusting the list.

**Fixed** with [`transport::read_bounded_response`](../crates/model-adapters/src/transport.rs), which
reads the decoded body in chunks and stops at `RESPONSE_BYTE_LIMIT` (1 MiB), dropping the stream rather
than draining it. A request timeout bounds how long, not how much; this bounds the allocation.

A truncated body is reported as its own diagnosis, not as "malformed JSON" — otherwise the operator is
sent hunting a syntax error in a response that was merely too large.

A separate `ResponseReadError` is used rather than adding a variant to `TransportConfigError`, because
every variant of that enum is raised by validating a setting *before* any request is made.

### B10 — truncation before redaction

`sanitize_response_text` truncated to `SANITIZED_LIMIT` and *then* replaced the secret, so a credential
straddling the cut left its prefix behind as though it were provider output.

**Fixed** by redacting first, then bounding. Redaction alone is not enough — that would fix the leak while
leaving the allocation exposure — so the two limits are separate and both are required.

**Evidence:** [`tests/response_limits.rs`](../crates/model-adapters/tests/response_limits.rs) — 8 tests:

- a fake credential placed at **every offset across the boundary**, asserting no prefix of any length from
  4 characters upward survives. A test at one offset proves nothing about the other two thousand.
- the same property for a short credential at 200 offsets, and for one beyond the retained prefix.
- a 64 MiB **chunked** body: retained exactly **1,048,576** bytes; the server wrote **2,098,176** of an
  attempted 64 MiB before the read stopped.

The streaming assertion measures the server's progress, not merely the length returned. A cap applied
after full buffering would return a short string and let the server finish all 64 MiB — so that test
fails against the defect it is written for.

The credential-free loopback path is untouched: `credential_posture` still reports
`NotSentToLoopback` for a loopback endpoint, and no credential requirement was added.

Every credential in the new tests is a synthetic `AUDIT-FAKE-…` string. No real secret is used.

These are reproduced in the audit and **not** fixed in this pass. Nothing below has been re-verified or
contradicted by this work.

| Finding | Why it still matters |
|---|---|
| **B3** multi-episode experiments cannot be reconstructed from storage | The most serious durability defect. `brain.rs` persists only the scalar mean, so completed results become unavailable once the in-memory representation is gone. |
| **B4** historical Arena recordings fail to deserialize | Archive advertises recordings that cannot be opened; replay unavailable; statistics silently skip them; the Tauri lookup reports "not found". |
| **B6** invalid model actions escape normal loss accounting | `SeatRefused` returns before turn telemetry, projecting to `NoResult`, which is **excluded from the win-rate denominator** — so a model's difficult failures can vanish from the headline score. A cancelled Arena match also surfaces as the same `NO RESULT` as a seat refusal. |
| **B7** batch statistics replaced by archive-wide statistics | The `finally` block refreshes archive stats over the newest 200 summaries, so the report after a batch may not be the batch. |
| **B8** recording digest does not commit to the complete evidence | Elapsed time, cancellation, response text and usage are omitted; strings are not length-delimited. |
| **B11** saved settings do not govern subsequent runs | `rootSeed: null` ignores the saved seed; a settings save does not refresh execution state. |
| **B12** failed runs leave lifecycle and cancellation state unfinished | A failed run can stay `running` and retain a stale cancellation entry. |
| **B13** comment-only repairs receive a perfect coding score | Marker substring tests accept a file of comments. |
| **B14** three-way ties receive different ranks | Rank uses the preceding position, not the preceding assigned rank. |
| **B15** the CI gate cannot substantiate release readiness | `manifest-path` points at `Cargo.lock`; a Docker action is used in the Windows job; `--log-level error` does not promote warnings. |

**Next is the durability cluster — B3, B4 and B12.** These are the findings that make a result
disappear, and they share one root cause: state that exists only in memory during a session and is not
written to disk in a form that can be read back.

---

## Risks requiring verification — neither confirmed nor dismissed

Carried from the audit unchanged. **No exploitation has been attempted.**

- **Export-directory junctions/symlinks** (`tasks.rs:439`) — lexical containment does not establish
  filesystem containment through an existing reparse point.
- **Concurrent AppContainer isolation** — shared profile identity and directory grants.
- **Inherited handles** — concurrent child launches.
- **Build provenance freshness** (`crates/core/build.rs:33`) — branch-ref movement, worktrees, untracked
  compiled sources.

Also unresolved from the audit's own verification gaps: the **production bundle has not been
acceptance-tested**, no installer has been generated or executed, and the audit did not independently
rerun the Rust suite or certify the accessibility claims.

---

## Gate state after this work

| Gate | Result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | 0 diagnostics |
| `cargo test --workspace` | **959 passed, 0 failed** (was 947 — 12 new B6 tests) |
| `tsc -b` | 0 errors |
| `vitest run` | **494 passed, 25 files** (was 487 — 7 new B6 population tests) |
| `scripts/arena-model-journey.mjs` | passing — WIN, 19 plies, 11 provider calls |
| `scripts/arena-early-cancel.mjs` | passing — stop honoured with 1 provider call outstanding |
| `scripts/experiment-restart-reconstruction.mjs` | **passing** — 4 episodes survive restart, per-episode and provenance compared |

`cargo test` needs `CARGO_BUILD_JOBS=2` and `RUSTFLAGS=-C debuginfo=0` on this host: at full parallelism
the linker fails with `LNK1102` (out of memory). That is a host resource limit, not a code failure.

No assertion was weakened to reach green.

---

## Continuation checkpoint

Work in progress. Where the next session should start.

**Branch** `dev/probelab-v1`, HEAD `58302e7`, **uncommitted** — all remediation is in the working tree.
Do not commit, push, tag, or generate an installer without explicit instruction.

**Completed clusters**

1. *Priority 1 — core Arena workflow.* B1, B2, B5, plus `scripts/arena-model-journey.mjs` and
   `scripts/arena-early-cancel.mjs`.
2. *Security.* B9, B10.

**Next, in order**

3. **B3 (finish)** — `scripts/experiment-restart-reconstruction.mjs` currently produces a single-episode
   run, because `#setting-episodes` does not mount on a cold start. Get the episode default to five
   through the Settings route or the same command the panel uses, then re-run. The script already asserts
   `> 1 episode` and fails loudly until this is true. **Do not relax that assertion.**
4. **B4** — versioned compatibility for historical Arena recordings. `arena_store.rs:190` deserializes
   stored JSON directly into the current `ArenaMatchView`. Migration must preserve absence honestly —
   unknown historical token usage must stay unknown, not become zero.
5. **B12** — lifecycle and cancellation cleanup on every exit path. `brain.rs:374` propagates errors before
   the registry cleanup at 390; `commands.rs:252,343` emit an error after persisting `Running` without a
   terminal row.
6. **B6, B7, B8** — measurement integrity. B6 also owns the fact that a cancelled Arena match surfaces as
   the same `NO RESULT` as a seat refusal, which the early-cancel work exposed.
7. **B11, B13, B14, B15** — remaining.
8. **P6** — targeted UI only, after correctness is stable. No redesign.

**New files added**

- `apps/desktop/src/providerEndpoint.ts` (+ test) — canonical endpoint boundary
- `apps/desktop/src/arenaEligibility.ts` (+ test) — single-match eligibility, with reasons
- `apps/desktop/src/arenaLiveController.ts` (+ test) — live event correlation and identity
- `scripts/controlled-model-server.mjs` — loopback OpenAI-compatible stand-in
- `scripts/arena-model-journey.mjs`, `scripts/arena-early-cancel.mjs`
- `scripts/experiment-restart-reconstruction.mjs` — B3 product-level proof: run, terminate, relaunch,
  reopen, compare per-episode over IPC
- `crates/model-adapters/tests/response_limits.rs` — B9/B10 regression net
- `crates/lab/tests/experiment_reconstruction.rs` — B3 persistence regression net

**Verification notes worth keeping**

- The arena move vocabulary is `COLUMN_n` / `CELL_n`, one-based, and it arrives **backslash-escaped**
  inside the chat message. A stand-in that regexes the raw body picks up the system prompt's worked
  examples instead and plays one column until it fills.
- Arena cells are `.arena-cell`; the board is `.arena-board`. An empty-match board is
  `.arena-board--idle`.
- Discovery is hardwired to ports 11434 / 8080.
- The archive row lags: it refreshes only after a match is stored, so it is the wrong place to read
  mid-match state.
- The Lab's episode count is the stored default `experimentDefaults.episodes`, set on `#/settings`, and
  the settings panel mounts from fetched state — a two-second wait is not always enough.
- **"RUN EXPERIMENT" is the multi-episode path; "RUN WITH MOCK" is a single benchmark run.** They look
  alike in the button list and produce indistinguishable-looking results until you read the episode count.
- The STOP control is always rendered and merely disabled when idle. "Is the run active" means *an enabled*
  stop control, not the presence of the word.
- A release binary needs `--features tauri/custom-protocol` or it silently loads the Vite dev server and
  every scripted test sees a browser error page.
- `get_experiment` takes `experiment_id`, not `experimentId` — Tauri resolves top-level command argument
  names exactly. Its response is a `BrainReadModel`, so the experiment is under `.experiment`, and every
  field inside is camelCase.
- `crates/telemetry` cannot depend on `crates/lab` (the dependency runs the other way), so cross-cutting
  persistence tests belong in `crates/lab/tests`.

---

## B11 — saved settings do not govern subsequent runs — **CLOSED**

Found while proving B3, and fixed because it blocked that proof: with settings not reaching execution
state, there was no way to make a run use more than one episode without editing the database by hand,
which the product-level test is explicitly not allowed to do.

**The defect.** `App` read the settings once, in an effect with no dependencies, and copied the execution
defaults into state at that moment. `SettingsPanel` saved a new document and the next run used the values
the application happened to start with. The panel's confirmation read *"They will be used for future
experiments and runs"*, and they were not — a confirmation that is not true is worse than no confirmation.

**The fix.**

* `apps/desktop/src/settingsSync.ts` — a one-writer/one-reader notification. A module rather than a prop,
  because the panel and the execution state are in different parts of the tree and the alternative is
  threading a callback through `AppShell`. Deliberately a counter rather than the document: `App` re-reads
  from the backend, so the values it applies are the values the backend actually stored, including any it
  adjusted or refused.
* `SettingsPanel` announces **after a successful write only**. Announcing on failure would re-read the
  unchanged document and make a rejection look like it had taken effect.
* `App` subscribes and re-reads; the effect still reads once on mount.
* `rootSeed` was `null` hardcoded at run construction, ignoring a stored setting the request contract
  already accepted. Now read from the stored document.

**Tests** — three in `SettingsPanel.test.tsx`:

1. a save the backend accepted announces;
2. a save the backend refused stays silent and leaves the stored document alone;
3. the execution state must not trust the *submitted* document — a backend that adjusts 4 to 6 must lead
   to 6, which is the distinction between re-reading and being handed a payload.

**Not fixed.** `seedBehaviour` has no field in the `run_experiment` request contract, so it is stored and
displayed but not sent. Recorded rather than cast around.

---

## B6 — invalid model actions escape loss accounting — **PARTIAL**

### Original reproduction

A tested model seat that is reached, answers, and returns text no legal move can be read from produced
`ArenaError::SeatRefused`. `crates/lab/src/arena.rs` mapped **every** error in that arm to
`outcome: ArenaOutcome::NoResult`. `OutcomeCounts::decided()` is `win + draw + loss`, so the match left
the win-rate denominator entirely.

The consequence is not that a failure is reported wrongly — it is that it is **not reported at all**:

* A model that forfeited every match it played reported *no results*, not *zero wins out of every match*.
* A batch of three clean matches and seven fortresses reported a **100%** win rate, over a population of
  three, with the seven failures invisible.

The engine's own code comment claimed "the engine already treats a refused move from the tested seat as
a forfeited match". That was true only of `ArenaError::IllegalMove`, the path where a move *parsed* and
the board then refused it (engine `arena.rs:617`, `finish(..., MatchOutcome::Loss, ...)`). The refusal
path never had that treatment. The comment described the intended behaviour, and the gap between the
comment and the code was the defect.

### Root cause

`NoResult` was a single bucket for four different things that had to stay apart: a decided match, a
forfeit, an infrastructure failure, and an operator cancellation. Once merged, no downstream layer could
separate them, and the statistics layer was right to exclude `NoResult` from the denominator — that was
its only correct response to a bucket with mixed meaning.

### Changes

* **`crates/engine/src/arena.rs`**
  * New `MatchTermination` enum: `Decided`, `ModelRuleViolation`, `InfrastructureFailure`, `Cancelled`.
  * `MatchResult.termination`, derived from the outcome and the stopping error, so it cannot drift.
  * `IllegalMove.attempted` is now `Option<GameMove>`. A refusal has no parsed move; the previous
    non-optional field would have forced a placeholder cell, which is exactly the fabricated evidence
    this pass forbids.
  * A tested-seat `SeatRefused` now records the refusal and returns `finish_with(Loss,
    ModelRuleViolation, ...)` — the same treatment the illegal-move path already had.
* **`crates/lab/src/arena.rs`** — new `ArenaTermination`, mirrored on `ArenaMatchView.termination` as
  `Option<…>` with `#[serde(default)]`. `None` means the recording predates the field and the reason is
  **not known**; it is never defaulted to `Decided`, because a historical `NoResult` row says nothing
  about whether it was cancelled or the provider was down.
* **`crates/lab/src/arena_stats.rs`** — new `MatchPopulation { attempted, conditional, forfeited,
  infrastructure_failures, cancelled, unclassified }` with `excluded()` and `reliability()`, carried on
  `MatchStatistics`. Classification reads `termination` only: the outcome is never used to infer a
  reason, because `NoResult` cannot distinguish the three cases on its own.

### Regression tests — 12 new, all passing

`crates/engine/tests/b6_forfeit_accounting.rs` (4) — the four boundary cases:

| Test | Asserts |
|---|---|
| `a_tested_seat_that_answers_unusably_forfeits_and_stays_in_the_result` | `Loss` + `ModelRuleViolation`, conditional |
| `the_forfeit_records_why_without_inventing_a_move` | refusal recorded, `attempted == None`, reason carries seat and provider message |
| `an_unreachable_provider_is_not_a_forfeit` | `SeatFailed`, `is_harness_failure`, **not** a protocol failure |
| `a_legal_but_refused_move_is_still_recorded_with_the_move_it_played` | `Decided`, `attempted.is_some()` — the move is known and reported |

`crates/lab/tests/arena_stats.rs` (6) — the population scenarios the mission names:

* `a_forfeit_is_a_loss_and_stays_in_the_denominator` — 1 win + 1 forfeit → win rate `0.5` over 2.
* `zero_success_reports_a_zero_win_rate_rather_than_no_results` — 5 forfeits → **`0.0`, not "no
  results"`**. This is the headline case.
* `infrastructure_failures_are_excluded_from_the_rate_but_counted_beside_it` — 3 decided + 7 provider
  failures → rate over 3, `attempted = 10`, `reliability = 0.3`.
* `a_cancellation_is_not_an_infrastructure_failure_and_not_a_loss` — the three cases stay in three
  buckets; cancelling never becomes a loss.
* `a_historical_recording_without_a_termination_is_unclassified_never_assumed_decided` — legacy rows land
  in `unclassified`, not `conditional`.
* `the_two_populations_are_reported_together_and_sum_to_the_attempted_count` — the buckets sum to
  `attempted`; a dropped match fails the test.

`crates/lab/tests/arena_store.rs` (2) — durability:

* `a_termination_survives_the_archive_and_a_restart` — four recordings with four different terminations
  round-trip through SQLite and a fresh store, and the population computed from the reloaded records is
  `attempted 4, conditional 2, forfeited 1, infrastructure 1, cancelled 1, unclassified 0,
  reliability 0.5`.
* `a_recording_with_no_termination_reopens_as_unknown` — an absent reason is not filled in.

### What is not yet done — product-level proof is BLOCKED

`MatchPopulation` is now on the native IPC surface and rendered in the panel, with 7 frontend tests
(`BatchStatistics.test.tsx`). The backend behaviour is proven at the engine, lab and archive layers.
**The product-level proof has not been obtained, so B6 is not closed.**

`scripts/b6-failure-accounting.mjs` is written and drives the real release executable. It currently
fails for two reasons, both recorded here rather than worked around:

**1. `run_arena_batch` does not return.** With the controlled provider confirmed up
(`/v1/models` answered before the application was started, so this is not a race), a four-match batch
against a loopback provider at 5 ms per response produced **no response within 60 seconds** and made
**zero provider calls**. The block is therefore *before* any model call — the archive was never reached.
The failure is in the batch command's own path, not in the provider, the harness, or the B6 changes.
`build_seat` validation, `state.arena.begin`, or the worker spawn are the candidates; none has been
isolated. The script now reports a timeout rather than hanging, so the next run yields a verdict
instead of silence.

**2. The test cannot isolate its own archive.** The database path comes from
`app.path().app_data_dir()` (`apps/desktop/src-tauri/src/lib.rs:340`), which on Windows resolves through
the known-folder APIs rather than the `APPDATA` environment variable. Setting `APPDATA`, `LOCALAPPDATA`
and a `PROBELAB_DATA_DIR` override in the child environment had no effect: the application opened the
**developer's real archive** and reported 41 pre-existing arena recordings. Any further run of this
script writes to real user data until the path is made overridable — which is itself worth fixing,
because no scripted product test in this repository can be made reproducible without it.

Until both are resolved, B6's claim is bounded precisely: the accounting is correct and durable, and
visible in the panel, but it has **not** been observed end to end through the real application.

---
### Native batch diagnosis — the hang is a real defect, now bounded

`scripts/diagnose-arena-batch.mjs` instruments the lifecycle with a monotonic clock and a correlation
id. Instrumentation was added to `run_arena_batch` itself (`arena_batch boundary` / `arena batch match
beginning` / `arena batch match returned`, all at `info` so they exist at the log level the investigation
actually used).

**A harness error found first.** The original invocation used `tic_tac_toe` with a model seat. The
product refuses that by design:

```json
{"code":"incompatible_agent","message":"a real model can only be seated in connect four;
 tic-tac-toe's nine-ply ceiling..."}
```

The earlier `arena-model-journey.mjs` worked precisely because it used `connect_four`. Any earlier claim
that the batch "hangs" was partly about an invalid seat configuration. **Corrected.**

**The runtime and IPC are healthy.** While the batch was outstanding, `list_arena_games` answered in
**0–1 ms**. The fault is not a wedged runtime, a held-across-await lock in the IPC layer, or the
provider.

**The worker, the registry and the report path all work.** A policy-versus-policy batch completed
end-to-end in **35 ms** through the identical code path, returning a full report and emitting two
progress events. An oversized batch was refused in 1 ms; a non-existent command returned
`Command probe_lab_no_such_command not found` in 0 ms. So the handler is dispatched and validation works.

**With a valid configuration the work completes and the caller is still never told.** With
`connect_four`, `depth_bounded_connect_four`, and the controlled provider in `alwaysInvalidly` mode:

```
boundary="worker-about-to-spawn"        elapsed_ms=0
arena batch match beginning  index=0     elapsed_ms=0
arena batch match returned   index=0     elapsed_ms=638
arena batch match beginning  index=1     elapsed_ms=640
arena batch match returned   index=1     elapsed_ms=1243
boundary="worker-joined"                elapsed_ms=1245
provider chat calls: 10     progress events observed: 2
invocation UNRESOLVED after 45s
```

**The command body completes entirely.** Instrumenting every step after the await shows the
full sequence running to completion:

```
boundary="worker-joined"          elapsed_ms=1200
boundary="registry-released"      elapsed_ms=1200
boundary="worker-outcome-taken"   elapsed_ms=1200
boundary="statistics-computed"    elapsed_ms=1201
boundary="report-built"           elapsed_ms=1201
arena batch report encoded        encoded_bytes=1297
```

Two suspects were ruled out by measurement rather than by argument:

* `statistics_for` is **not** the cause. It reads only the batch's own match identifiers rather than the
  archive window, so it is O(batch size) and cannot stall on historical records.
* `wilson_interval` is zero-trial safe, so no NaN reaches the payload.

**So the fault is no longer inside the command body.** The command returns and the response encodes
successfully, yet the caller's promise never settles. Re-running with the invocation started *without*
awaiting it in the page — parking the result on `window` and polling from separate round trips, so the
harness cannot itself be the thing that is broken — reports `state: "started"` at 10s, at 30s and at
45s. `resolve` and `reject` are simply never called.

A concurrent `list_arena_games` answers in 0ms throughout, and a **policy-versus-policy batch returns a
complete report through the same command**, so the runtime, the IPC layer, the command handler and the
report type are all healthy. What remains is the response *delivery* path for this command with a model
seat — not its execution, and not its serialisation.

That path is the same one the archive errors come from: `read_recordings` (`arena_batch.rs:365`) lists
summaries and then deserializes each full recording. On this machine that archive contains **dozens of
historical records that cannot be deserialized** —

```
ERROR probelab_desktop_lib::arena_batch: an archived match could not be read and is excluded from
these statistics match_id=arena-ttt-7-1791715869599-3
error=stored match "arena-ttt-7-1791715869599-3" could not be read:
      invalid type: integer `2`, expected an array of length 2 at line 1 column 387
```

— which is **B4 reproducing in the real archive**, and B7's statistics-refresh target. Narrowing further
requires instrumenting the report assembly after `worker-joined`; that is the next step, and it is a
rebuild this session did not reach.

---
## Assessment

**NO-GO for v1.0. Unchanged.**

Seven of fifteen findings are closed. B3 was the durability core and it is now closed at the product
level, which removes the "a result exists during execution and is unavailable afterwards" case for
experiments. That is real progress, and it is not close to sufficient:

- **B4 is still open**: the Arena archive advertises recordings that cannot be opened. Replay is
  unavailable for them and statistics skip them silently.
- **Measurement integrity is still broken** (B6). A model's failures can disappear from the denominator,
  which means the headline win rate does not mean what it says. This is the single most damaging open
  finding, because everything built on top of it inherits the error.
- **The CI gate does not substantiate release readiness** (B15), and this round surfaced concrete evidence
  for it: the documented build command is broken on this host.
- B7, B8, B12, B13 and B14 remain open.
- The production bundle, an installer, and the historical-data upgrade path have still not been
  acceptance-tested.

The audit's core judgement — that the failure is in the connection between execution, durable evidence,
and presentation — is now addressed in execution and in experiment durability. The half still outstanding
is **evidence integrity**: B4 (unreadable history), B6 (failures leaving the denominator), B7 (batch
statistics replaced by archive-wide statistics) and B8 (a digest that does not commit to the evidence it
is supposed to protect). Until those are closed, the numbers the product reports cannot be fully
trusted — including the ones this report cites.