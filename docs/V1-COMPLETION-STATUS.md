# ProbeLab v1.0 — implementation checkpoint

Work in progress toward v1.0. This file is the running continuation state: what is done, what is
verified, what is broken, and the exact next step. It is updated after each phase so a future
session never has to reconstruct the state by reading the diff.

**Current gate state — all green, re-verified this session:**

| Gate | Result |
| --- | --- |
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets` | 0 warnings |
| `cargo test --workspace` | **932 passed, 0 failed** |
| `tsc -b` | 0 errors |
| `vitest run` | **443 passed** (22 files) |
| `vite build` | succeeds |
| `npm run check:styles` | legibility floor OK · **contrast contract OK** |
| `npm run check:icons` | 9 outputs match |
| `node apps/desktop/scripts/check-tauri-config.mjs` | **valid**, production policy clean |
| `python3 scripts/check-version-provenance.py` | three manifests and STATUS.md agree on 0.5.0 |
| `scripts/check-feature-matrix.py` | capabilities consistent, commands reachable |
| `scripts/check-unstyled-classes.py` | **0 unstyled** |
| Grid stage geometry at 1366×768 / 1600×900 / 1920×1080 | board square, cells uniform, whole board above the fold, no horizontal overflow |
| axe-core on **all 11 routes** | **0 violations** everywhere |
| Keyboard traversal on **all 11 routes** | 0 without a visible focus ring, 0 unnamed, 0 inside `aria-hidden`, 0 clipped panels, 0 horizontal overflow |
| `prefers-reduced-motion` | clamped **and proven unclamped when the preference is off** |
| `cancel-through-ui.mjs` | 19 checks pass against a controlled slow provider |
| `replay-integrity.mjs` | digests identical across a real process restart |
| `grid-durability.mjs` | restart durability passes, scrubber genuinely exercised |

---

## Phase 1 — Recovery and inspection: **COMPLETE**

Baseline re-verified on arrival: 893 Rust tests, 333 frontend tests, build clean.

### 1. The ReadinessPanel hook-order violation was real and unfixed
`useMemo` sat at `ReadinessPanel.tsx:142` *after* an early `return` at line 114. Clearing the
discovery error changed the hook count and React throws. The effect: pressing **RETRY DISCOVERY** and
succeeding — the single action the button exists for — crashed the panel.
Every hook now precedes every early return.
**Verified the regression test is real**: reintroducing the defect makes it fail with
`Rendered more hooks than during the previous render`; the fix restores it.

### 2. No React error boundary existed anywhere
`ErrorBoundary.tsx` now provides `PanelErrorBoundary` / `PanelBoundary`: per-panel containment, a
named subject, an alert so content below is not read as current, deliberate-only recovery, and a
collapsed technical detail. 11 routes wrapped in `App.tsx`.

A **root** boundary was added in `main.tsx` as well, after discovering the gap is real: the per-panel
boundaries sit *inside* `App`'s render output, so an exception in `App`'s own body — and its route
table is built from fetched data — had nothing above it and blanked the window. Confirmed in the
browser before fixing.

### 3. The Arena CSS gap was quantified and closed
60 of 80 `arena-*` classes had no styles. See Phase 2.

---

## Phase 2 — Arena visual design: **COMPLETE**

`ArenaPanel.tsx` was functionally complete and tested but unstyled, and measuring it in a real
browser found **eight** defects that reading the CSS would not have.

### Defects found by measurement, and fixed

| # | Defect | How it presented |
| --- | --- | --- |
| 1 | Board collapsed to 125px wide | Circular percentage: an auto-width flex item sized by `min(100%, …)` against its own contents resolves to min-content. Fixed with a definite column width. |
| 2 | Tray misaligned with the board | `--arena-board-width` was declared on the board; the tray is its *sibling*, and custom properties do not inherit sideways. Moved the derivation to `.arena-panel`. |
| 3 | Cells stretched into ovals | Height was fixed and width clamped, so a narrow column gave 250×452 — 36px columns in 75px rows. Inverted to `aspect-ratio` from the width. |
| 4 | **Transport invisible** | `.arena-controls` holds three blocks and was declared as two columns. The third spilled into an implicit track, the first collapsed to `0px`, and the play/step/scrub controls laid out at **26px** wide — present, focusable, invisible. |
| 5 | **Primary action below the fold** | RUN AGAIN measured at `top=799` in a 768px window. Setup stacked under the board; the panel is now two columns above 1360px. |
| 6 | Winning line erased side identity | The four winning cells were recoloured uniform cyan. When the *model* lost, the board appeared to show the model winning. Now a white ring + glow over the disc's own colour. |
| 7 | 1500px breakpoint was non-monotonic | It made the 1600px board (464×398) *worse* than the 1366px one. Replaced with the width calculation that actually decides it, documented in the stylesheet. |
| 8 | Page scrolled sideways | Fixed 64px cost column could not hold "4.1 s" and did not wrap. |

### Also fixed in Phase 2

- **The board was completely invisible to screen readers.** `ConnectFourFrame` put `aria-hidden` on
  the wrapper that *contains* the board, so all 42 cells and their spoken names dropped out of the
  accessibility tree — connect four only, because tic-tac-toe has no wrapper. `aria-hidden` moved to
  the tray alone. Regression test verified in both directions.
- **Invalid ARIA grid structure (2 critical axe violations).** `role="grid"` was given 42 `gridcell`
  children and no `role="row"`, which is invalid by definition; axe reported
  `aria-required-children` and `aria-required-parent` on all 42 cells. Rows are now real elements.
  This was invisible to the unit tests because role queries find the cells without validating the
  tree.
- **10 serious contrast violations**, all one root cause: `--ink-4` — documented as *placeholder,
  disabled label*, 3.33:1 on the rail surfaces — was carrying enabled, meaningful text in five
  selectors. Those five now use `--ink-3` (5.58:1). The other ~130 uses of `--ink-4` (borders,
  placeholders, disabled, bullets) are correct and untouched.
- **`<aside>` nested inside a landmark** → plain `div`, label kept.
- **`.brand-text` outside every landmark** (axe `region`) → the rail's brand lockup is now
  `aria-hidden`, scoped to that call site. It is decorative chrome that duplicates the document
  title, and it is deliberately not a navigation destination.
- **A comment-delimiter bug this session introduced and repaired.** Replacing old Arena CSS clipped
  the opening `/*` of section 19. A stray `*/` is discarded by the CSS parser, so the file still
  rendered and still passed every test — the imbalance is invisible to a raw `/*`/`*/` count, which
  matched, because the stray closer silently absorbed a later opener. Found with a state-machine
  check, not a tally.

### Files changed in Phase 2
`styles.css` (section 18 rewritten, 828 obsolete lines replaced by ~1250) · `ArenaPanel.tsx` (rows,
tray `aria-hidden`, `aside`→`div`) · `AppShell.tsx` (brand `aria-hidden`) · `main.tsx` (root boundary)
· `App.tsx` (11 route boundaries) · `ErrorBoundary.tsx` / `.test.tsx` (new) ·
`ArenaPanel.test.tsx` (+2 regression tests) · `ReadinessPanel.tsx` / `.test.tsx`

---

## Phase 3 — Batch statistics: **COMPLETE**

Durable batch evaluation with distributions, Wilson intervals, and honest denominators.

### What was built

**`crates/lab/src/arena_stats.rs`** (new) — the aggregation. It takes `&[ArenaMatchView]` and nothing
else, so a statistic cannot mix in a half-played match or an in-memory counter.

- `OutcomeCounts { win, draw, loss, noResult }` — **four** buckets, and `noResult` is excluded from
  every rate denominator.
- Two rates, because they answer different questions: `win_rate` = wins ÷ decided (a draw is in the
  denominator and not the numerator), and `decisive_win_rate` = wins ÷ (wins + losses).
- Wilson intervals via the existing `harness::wilson_interval`, **reused** rather than reimplemented —
  the repo already had a deliberate comment against two drifting copies.
- `costs` with latency/token averages over *measured turns only*; a policy seat's absence stays
  `None`, never `0`.
- `sample_is_small` below 10 **decided** matches; undecided matches do not count toward the threshold.
- `match_ids` + `provenance_digest` per group, so a number can be traced to its inputs.
- Grouping by (game, tested seat, opponent seat). **Nothing sums across games.**

**`apps/desktop/src-tauri/src/arena_batch.rs`** (new) — `run_arena_batch`, `cancel_arena_batch`,
`arena_statistics`. Each match is archived before the next starts; statistics are computed by **reading
the recordings back out of the archive** rather than from the values the worker held.

**`apps/desktop/src/BatchStatistics.tsx`** (new) + tests — distribution bars (dashed fill for "no
result"), two rates with denominators and interval bars, facts grid, provenance disclosure.

### Defects found and fixed in Phase 3

1. **The board was pushed off the screen by the statistics panel** (found by screenshot, not by tests).
   `BatchStatistics` was nested inside `.arena-controls`, so the right column grew to 2084px; the panel
   grid stretches the left column to match, making `.arena-stage` **3062px** tall and the board
   `y=1157` in a 900px window. The board — the panel's subject — was entirely below the fold. Fixed by
   giving the batch its own full-width grid row.
2. **Statistics were only fetched after a batch**, so a session with existing recordings sat on "nothing
   recorded yet" — including matches from before the last restart, which is the case the durable
   archive exists for.
3. **`plural()` produced "12 matchs"** and pushed whole verb phrases through it, yielding "1 matchs …
   were stopped". Rewritten to pluralise nouns only, with agreement decided separately.
4. **A newline inside an `aria-label`** (JSX source wrapping) put a pause mid-sentence in the spoken
   interval. Now built from joined parts.
5. My own test asserted `!sample_is_small` for 4 decided matches, which is a small sample by the
   module's own rule. The test was wrong; the code was right.

### Verification
- 30 new Rust tests (`arena_stats.rs`, `arena_batch.rs`, plus a wire-format pin).
- 27 new frontend tests (`BatchStatistics.test.tsx`).
- `cargo test --workspace`: **923 passed, 0 failed** (was 893).
- `vitest run`: **372 passed** (was 345). `tsc -b` clean. `vite build` clean.
- `cargo clippy --workspace --all-targets --all-features`: 0 diagnostics. `cargo fmt` clean.
- `check-feature-matrix.py`: 34 capabilities consistent, 42 commands all reachable.
- Geometry measured at 1366×768 / 1600×900 / 1920×1080: 2 groups, 4 buckets each, 2 rates each,
  the caution shown once, no horizontal overflow, no page errors.

### Deliberately not done
No cross-game leaderboard, and no agent ranking. A win rate against one opponent does not measure an
agent; that claim belongs to `comparison.rs`, which requires evidence this does not gather.

---

## Phase 4 — Grid-world navigation trace: **COMPLETE**

The grid world was a 25-cell static tile map: no motion, no turn identity, and a legend that called the
union of everything the run had ever sensed "observed only" without saying which turn you were looking
at. It is now an animated, steppable trace of a run's committed turns, and it survives the process that
made it.

### The one rule everything rests on

**A tile appears on screen only because some recorded observation contained it.** Not from the seed,
not from the benchmark definition, not from a neighbour-of-a-neighbour inference, not across an episode
boundary. This is enforced by construction in `gridFrames.ts` and pinned by a test that walks all 25
coordinates of a recording in which 18 were never sensed.

The old `buildGrid` in `App.tsx` violated it, quietly: when it placed the agent it wrote
`tile: previous?.tile ?? latestEvent?.observation.current_tile ?? "safe"` — a tile no observation ever
carried, painted as flat ground. That function is deleted.

### What was built

**Backend — the durable read.**
- `SqliteResultRepository::telemetry_events(run_id)` (`crates/telemetry/src/lib.rs`). A run's events
  were always written append-only, and the only reader was `RecentEventBuffer` — bounded to 256 events,
  scoped to one run, **empty after a restart**. So a recording was only ever replayable inside the window
  in which it happened, and a long run replayed from the middle. The read is `ORDER BY sequence`, holds
  no engine or provider, and an unknown identifier is an empty vector rather than an error.
- `get_run_telemetry` (`apps/desktop/src-tauri/src/commands.rs`), registered in `generate_handler![]`.
  **No recording format changed** — the events needed to reconstruct every frame were already
  committed, so the read adds no second trace that could diverge from the first. The replay digest is
  untouched, and no grid-world number moved.

**Frontend.**
- `gridFrames.ts` — one frame per recorded turn, plus one for the position the final recorded move
  landed on (which appears in *no* observation, only in the transition outcome; without it every episode
  stops a square short). Knowledge resets at an episode boundary.
- `useGridPlayback.ts` — two modes. Live derives the position from the newest frame, so a view cannot
  fall a turn behind the engine; recorded steps, plays and scrubs. Nothing here executes anything.
- `GridWorldStage.tsx` — the board, the readout, the sensing list, the turn log, the transport, and the
  verdict band. It computes **no verdict**: success is the independent evaluator's stored answer and
  arrives as a prop.
- Opening a stored run from the history table now opens its trajectory too, which is the only way a
  returning user can reach a past episode at all.

### Defects found by measurement, and fixed

| # | Defect | How it presented |
| --- | --- | --- |
| 1 | **The board was entirely below the fold.** With the progress hero between the controls and the board, the board's top edge measured `y=748` in a 900px window. Reordered the stage to sit directly under the launch controls. |
| 2 | **`max-height` on the board made it a rectangle.** `width: 100%` resolved first, so the height clamp produced a **402×323** board with 72×56 cells — a stretched map. Replaced with `width: min(100%, 440px, 36vh)` so `aspect-ratio` keeps it square by construction. The driver's "board stays square" assertion caught this. |
| 3 | **A 154px board at 1366×768.** The stage is given 436px there and 990px at 1920 — a viewport media query gets both ends wrong. Replaced with a **size container**, so the breakpoint follows the panel's own width. |
| 4 | **A 34-character run id in the chip wrapped the panel heading to two lines**, which pushed the board 42px further down and off the fold at 768px. The chip shows 12 characters with the full value on its tooltip. |
| 5 | **The chip said "AWAITING A RUN" over a board with seven turns on it.** Four states now: live / last-run-this-session / recorded / awaiting. |
| 6 | **Observed ground and an empty cell were the same square.** A safe tile now carries a centred mark. |
| 7 | **`.visually-hidden` did not exist.** Two table captions carrying that class rendered as visible text. The utility is written, clipped rather than `display: none`. |
| 8 | **The stored-run table had no styles at all** — it rendered as unstyled text and the lifecycle pill ran into the terminal reason: "Completedreached the goal". Found by looking at a capture. |
| 9 | **That table scrolled sideways inside a 216px well** (381px of content in 246px). Below 420px of panel width it stops being a table and lays each row out as a wrapped line. |
| 10 | **27 serious `color-contrast` violations on `#/lab`** that no earlier audit had looked for. `--ink-4` — documented as *placeholder, disabled label* at 3.33:1 — was carrying labels over measured values in nine rules. All now `--ink-3`. |
| 11 | **`scrollable-region-focusable` on the turn log**: a list a keyboard user cannot focus is a list a keyboard user cannot scroll. |

### Verification
- **9 new Rust tests**, in four files, covering the four properties the phase was asked to test
  **separately**: live execution, recorded playback, cancellation, restart durability
  (`apps/desktop/src-tauri/tests/grid_recording.rs`), plus the store-level read
  (`crates/telemetry/tests/grid_readback.rs`).
- **58 new frontend tests** across `gridFrames.test.ts`, `useGridPlayback.test.ts` and
  `GridWorldStage.test.tsx`, over one shared fixture that is 18/25 unknown on purpose.
- `cargo test --workspace`: **932 passed, 0 failed**.
- `vitest run`: **430 passed**. `tsc -b` clean. `vite build` clean. `check-unstyled-classes.py`: **0**.
- **Benchmark scores and evaluation semantics are unchanged.** Nothing in this phase reads a rate, a
  budget or a completion status from the stage; `MockModelStrategy`, the harness, the evaluator and the
  prompt are untouched, and no published grid-world number was re-measured.

### What this file does not cover, and why
`grid_recording.rs` does not invoke `get_run_telemetry` through Tauri. Building the managed `AppState`
pulls the whole Tauri runtime into that test binary, which on this Windows host then imports two
casings of the same system libraries (`advapi32.dll` and `ADVAPI32.dll`) and fails to load with
`STATUS_ENTRYPOINT_NOT_FOUND` before a single test runs. Measured, not assumed — every other test binary
in the crate links and runs. The read is pinned by `crates/telemetry/tests/grid_readback.rs`, the
registration and argument casing by `lib.rs`'s guards and `check-feature-matrix.py`, and the command
itself is exercised through the product by `scripts/grid-durability.mjs`.

---

## Phase 5 — Validation and evidence: **COMPLETE for the engine path**

### A real model, through the desktop application

The stated gap was that the engine path was proven by an example binary but never driven from the
product. It now is. `scripts/grid-connected-run.mjs` drives the built executable over the DevTools
protocol and reports what the run actually produced:

| | |
| --- | --- |
| model | `qwen2.5:0.5b` over `http://127.0.0.1:11434/v1`, loopback, no credential sent |
| prompt | `grid_world_v2` · environment `grid_world_environment_v2` · seed 2000 |
| committed events | 11 · model calls 11 · valid responses 11 (**100.0% protocol rate**) |
| turns used | 11 of 12 · **invalid actions 8** · reward +3 |
| tokens | 11,511 |
| evaluator verdict | **Ran out of budget** (`turn_limit`) |
| tiles never sensed | **16 of 25** |

The stage drew the episode from those 11 events, and the verdict band printed the evaluator's answer
rather than one derived from the final position. The 8 invalid actions are the interesting part: a
0.5B model produced well-formed `MOVE_*` protocol for every turn and still mostly walked into walls,
which is exactly the thing the partial observation makes hard and exactly what the board now shows.

### Cancellation, through the product

The Phase 5 statement was that no stop signal could land on a run through the product. That is no
longer true, and the reason it was true is gone: **the command center had no stop control at all.**

| | |
| --- | --- |
| control | **STOP RUN**, native, present only while a run is in flight |
| terminal state | `cancelled` — not `error`, not `completed` |
| recorded reason | `STOPPED BY OPERATOR · engine reached turn 2 · 2 events committed` |
| persisted events | 2, sequences `[1,2]`, turns `[1,2]`, all belonging to the stopped run |
| cancel handle after the run | `cancel_experiment` answers `false` |
| provider after the stop | 2 calls at the stop, 2 calls 1.5s later — **no further calls** |
| a second run | starts immediately and reaches the provider |

The blocker was that a mock episode commits its whole turn budget in milliseconds, so there is no
window in which to stop one. That is a property of the benchmark, and the fix is to stop testing
cancellation against a benchmark that cannot be stopped: `scripts/slow-provider.mjs` is a controlled
OpenAI-compatible loopback provider that takes 3000ms per response, and `scripts/cancel-through-ui.mjs`
drives the built executable against it. **19 checks, all passing.**

The change is typed end to end rather than signalled with prose. `RunOutcome` is now
`Completed(Box<CompletedRun>) | Cancelled { turns_used } | Failed(String)`, so every consumer has to say
which of the three it is handling and the default case is no longer "assume it worked" — folding a stop
into `Err` is exactly what made it look like a failure before. `RunCancelledEnvelope` and
`RUN_CANCELLED_EVENT` carry it to the frontend, and a cancelled run's lifecycle row is written as
`Interrupted` with **no score**, because recording a partial run's partial reward as a score would put a
number in the history that no benchmark ever produced.

`CompletedRun` is boxed because it carries a whole `TestResult` — some nine hundred bytes — and the
outcome crosses a thread boundary and an async task to reach the emitter. Unboxed, every cancel and
every failure moved that much memory to carry a `u32`; clippy said so.

### Replay integrity, by digest rather than by eye

`scripts/replay-integrity.mjs` answers a question a screenshot cannot: *is what is on screen the thing
that is on disk?*

- An FNV-1a content digest is computed in-page over the recorded frame fields and compared with the
  engine's own `replay_digest`.
- Every painted tile is checked against the recording's observations, frame by frame: no frame paints
  unobserved terrain, no frame paints the wrong terrain, no frame omits an observed tile, and the tile
  count never falls as the recording advances.
- Hidden tiles stay `unseen`; the unseen count on the board is checked against the unseen count in the
  label.
- Both digests are compared **across a real process restart**, not within one session.

Result: content digest `7686cbf11e2902a8` before and after, engine digest
`bf211808998d914e…` before and after, rendered tile map identical, marker at 4,4 in both sessions.

### Restart durability, through the product

`scripts/grid-durability.mjs` runs an episode, **terminates the process**, launches it again against the
same on-disk database, and reopens the run from the history table. Measured after the restart:

- the fresh session's live feed holds nothing (`AWAITING A RUN`) — the buffer is genuinely empty;
- the run is still listed, and reopens (25 cells, 8 rows, highest turn 7);
- the feed is labelled `RECORDED …` and a transport is offered;
- scrubbing to the last frame moves the marker 1,0 → 4,4, scrubbing back returns it to 1,0, and
  stepping forward from the first frame moves it again;
- **no run was written while replaying** (58 stored rows before, 58 after).

Cancellation was removed from this script rather than duplicated in it, so the weaker claim has one
home instead of two.

### Geometry at all three target resolutions, inspected

`scripts/grid-trace.mjs` measures and `scripts/measure-grid-stage.mjs` re-measures independently. All
three resolutions were also **looked at**, which is how defects 1–6 in Phase 4 were found.

| | 1366×768 | 1600×900 | 1920×1080 |
| --- | --- | --- | --- |
| board | 276×276 | 320×320 | 389×389 |
| cells | 47×47, uniform | 56×56, uniform | 69×69, uniform |
| whole board above the fold | yes (449–725 of 80–738) | yes (449–769 of 80–870) | yes (449–838 of 80–1050) |
| horizontal overflow | 0px | 0px | 0px |

`scripts/probe-grid-widths.mjs` now opens the newest recording before measuring. It used to report
`board: null` on a session with no run loaded, which is to say it omitted exactly the numbers it exists
to report on any machine that had not just finished a run.

### Accessibility and motion, on every route

`scripts/audit-a11y.mjs` used to scan `#/lab` and `#/arena` — the two routes a phase had touched — and
every later audit inherited that scope. **Nine routes went unmeasured for the life of the project.**
They are not clean. The route list is now parsed out of `AppShell.tsx`'s own `RouteId`, so a new route is
audited the day it is registered rather than the day someone remembers.

What that found on nine previously-unmeasured routes:

| Defect | Routes | Measurement |
| --- | --- | --- |
| `--ink-4` text below the floor | readiness, lab, agents, benchmarks, tasks, settings, security, replay | **2.72:1 – 3.36:1** |
| `--ink-3` on a tinted surface | benchmarks, settings | **4.36:1, 4.46:1** |
| focus indicator suppressed | agents | the library filter killed its own ring |
| scrollable region not focusable | every route whose content overflows | axe `scrollable-region-focusable` |

Three of those were the checker lying rather than the application being wrong, and fixing the checker
mattered more than the finding:

- The accessible-name check ignored `<label for>`, so it reported **sixteen correctly-labelled inputs on
  five routes** as nameless. A checker that cries wolf gets switched off, which is worse than having no
  checker.
- The geometry check flagged panels that extend below the fold. Content areas scroll; that is not a
  defect. It failed on nine of eleven routes for the crime of having content. It now walks outward and
  stops at the first ancestor that can scroll, so it only reports a panel whose tail is genuinely
  unreachable.
- The reduced-motion check read the grid stage's selectors, which are absent when no recording is
  loaded, and counted `not present` as a pass. A check that cannot fail is not a check.

The reduced-motion check is now measured with an injected probe carrying a one-second animation and a
one-second transition, and measured in **both directions**: with the preference forced on the probe must
come back clamped, and with the preference forced off it must come back at its full second. Reading only
the clamped case would pass just as happily against a stylesheet where nothing animated at all.

Result: **11 routes, 0 axe violations, 0 unnamed controls, 0 without a focus ring, 0 hidden focus traps,
0 clipped panels, 0 horizontal overflow.**

### The contrast defect, at its source

`--ink-3` and `--ink-4` were tuned against the panel they were designed in and shipped without ever
being measured against the surfaces they also land on — sixty declarations across nine routes, which
passed every check because every check looked at two routes.

Hand-editing sixty sites is the obvious move and the wrong one: the next person to add a chip in a tint
has no way of knowing the floor. So the tokens now carry the guarantee and
`apps/desktop/scripts/check-color-contrast.mjs` holds them to it against **all six surfaces, both
wells and all four tints**, wired into `pnpm check:styles` and therefore into CI.

| | panel | inset | lightest surface | blue tint |
| --- | --- | --- | --- | --- |
| `--ink-3 #99a5b6` | 7.0:1 | 7.6:1 | 5.2:1 | 6.3:1 |
| `--ink-4 #8e9aad` | 6.0:1 | 6.8:1 | 4.6:1 | 5.5:1 |

**The cost, stated rather than hidden:** a fourth gray dark enough to read as clearly *dimmer* than
tertiary text cannot clear 4.5:1 on this palette's hovered and pressed surfaces — measured at 4.33:1 and
3.88:1 while writing the check. So the distance between "meta" and "placeholder" compressed from sixty
steps of red to a handful, and that hierarchy is now carried by weight and size. That is a real
regression to the visual hierarchy and it is the correct trade: a token that reads as dimmer at the
price of being unreadable is not a tier. The check also asserts the ink scale stays **ordered**, so the
compressed tier stays a deliberate choice rather than drifting.

### Protocol compliance versus action legality

The two rates now come from `apps/desktop/src/capabilityRates.ts` rather than from arithmetic inline in
`App.tsx`, so the wording is tested against the shapes a real run produces — including the measured
connected run where a model answered every turn in perfect protocol (100.0%) and had 8 of its 11 moves
refused. That divergence is the interesting result and the panel has to be able to say it. Evaluator
semantics are unchanged.

### New and repaired verification scripts

| Script | State |
| --- | --- |
| `scripts/grid-trace.mjs` | passing |
| `scripts/grid-durability.mjs` | passing; was **broken** — it referenced an `app` it never defined after cancellation was removed, and its scrub test had never driven the scrubber |
| `scripts/replay-integrity.mjs` | passing |
| `scripts/cancel-through-ui.mjs` + `scripts/slow-provider.mjs` | passing, 19 checks |
| `scripts/audit-a11y.mjs` | passing on all 11 routes |
| `scripts/grid-connected-run.mjs` | passing against `qwen2.5:0.5b` |
| `scripts/measure-grid-stage.mjs`, `scripts/probe-grid-widths.mjs` | passing |
| `apps/desktop/scripts/check-color-contrast.mjs` | new, gated on CI |
| `apps/desktop/scripts/check-tauri-config.mjs` | new, gated on CI |
| `scripts/check-version-provenance.py` | new, gated on CI |
| `scripts/release-provenance.mjs` | new, manual — writes and verifies a manifest, does not sign or publish |

`grid-durability.mjs`'s scrub check is worth its own note. It set `.value` on the range input and
dispatched `change`; React maps a range input's `onChange` to the native `input` event *and* keeps a
tracker of the last value it saw, which assigning `.value` updates too. So the handler was never called
and the marker never moved — and the script reported **a failure in a product that was working**. The
value now goes through the prototype setter, both events are dispatched, and the position is read in a
later evaluation after React has re-rendered. It moves 1,0 → 4,4.

`scripts/audit-a11y.mjs` takes an axe-core path and expects one installed outside the repository; the
workspace does not depend on it.

---

## Phase 6 — Release engineering: **PREPARATION COMPLETE; the release itself is NOT authorized**

No installer was generated, nothing was signed, and nothing was tagged, pushed, or published.
`docs/RELEASE-CHECKLIST.md` records the remaining steps and what each one is waiting for.

### The packaging configuration is now gated rather than merely written down

`tauri build` is the only thing in this project that reads `tauri.conf.json`, and it is also the step
that produces installers — so it runs last, by hand, and never on CI. A misspelled key in that file
therefore survived every gate in the repository and surfaced as a failed release build.
`apps/desktop/scripts/check-tauri-config.mjs` validates it against the schema **that ships inside
`node_modules/@tauri-apps/cli`** — read locally, so it cannot drift from the version in use — and is on
CI.

Validated that it catches its own class of mistake: a `widht` typo in a window is reported as
`tauri.conf.json.app.windows[0].widht: not a key the Tauri schema defines`. It also took two fixes to
make true. `$ref` resolution without `allOf` flattening walked straight past `app`, `bundle` and `build`
— every top-level field is declared as `{"allOf": [{"$ref": "..."}]}` — found no declared properties
underneath, and accepted any key at all. And a schema `type` that is `["string", "null"]` was being
compared as one string, so the three most obviously correct fields in the file were reported as type
errors.

Beyond the schema it asserts the properties the schema cannot express: no development-server origin in
the shipped CSP, no `unsafe-eval` in either policy, `freezePrototype` on, and bundle metadata present.

### What changed in the configuration

- **The development server's websocket moved out of the shipped CSP.** `ws://localhost:1420` was in
  `app.security.csp`, which means every release build shipped a policy admitting an origin it can never
  connect to. It is now in `devCsp`, and the check fails if it returns to `csp` **or if it ever leaves
  `devCsp`** — a configuration that silently breaks hot module replacement while every build gate stays
  green is the same class of failure as the one being fixed.
- `freezePrototype: true`.
- Bundle metadata: `publisher`, `shortDescription`, `longDescription`, `category`, `copyright`. An
  installer without a publisher is an unsigned default template, and that is what a user deciding
  whether to run it sees.

### Version provenance is checked, not assumed

The version was declared in three places with nothing between them: `Cargo.toml` (stamped into the
binary by `CARGO_PKG_VERSION`), `tauri.conf.json` (stamped into the installer), and `package.json`. A
disagreement ships an application and an installer that report different versions.

`scripts/check-version-provenance.py` asserts all three agree and reads the version claims out of
`docs/STATUS.md`. It found a real defect on its first run: STATUS.md said **0.4.1** and asserted that
"`Cargo.toml` is the authority; nothing else declares it", which was true when written and false now.
Both are corrected.

It deliberately does not bump anything. Cutting a release is a judgement about what changed, and a
script that notices three files disagree is not the thing that should decide the answer.

### Checksums and provenance

`scripts/release-provenance.mjs write|verify` records the SHA-256, size, commit, and **whether the tree
was clean** for each artifact, and re-hashes on `verify`. Verified in both directions: it reports a
match on an untouched file, and `recorded 00f9a3a4…, file hashes 75f9a3a4…` on a tampered manifest.

It does not sign, upload, or publish, and it says so in its own output. Signing needs a key that does
not live on a developer machine; no script here can honestly prepare that, so it is recorded in the
checklist as a step with an owner rather than as work in progress.

### What Phase 6 deliberately does not contain

No workflow builds or uploads a release artifact. No workflow signs anything. No version is bumped
automatically. Nothing is published from CI. Each of those is a decision with a name attached to it, and
automating them would remove the name.

## Phase 7 — Documentation: **COMPLETE**

`docs/STATUS.md` updated: the version, and item 28's packaging claim, which had drifted.
`docs/RELEASE-CHECKLIST.md` written. `README.md`, `ARCHITECTURE.md`, and `docs/feature-matrix.json`
brought back in line with what ships.

### The README was telling users to verify a file that does not exist

`Install` said "Download the latest release… Verify the download against the published
`SHA256SUMS.txt`." Nothing has been published. A README is the first thing a person reads and the last
thing anybody re-reads, so a confident instruction to fetch a nonexistent checksum file is worse than
no install section: it reads as evidence that a release exists.

It now states plainly that there is no published release, explains how to build one, and points at
`docs/RELEASE-CHECKLIST.md`. The features list had also not been touched since before the grid-world
work, so it now covers the navigation trace, the STOP RUN control, the registries, integrity
reporting, and the security sweep.

### The feature matrix was understating and overstating in different places

`run-cancellation` was marked `complete` while describing only the library-level guarantee — nothing
about the control that now exists. It now names the native STOP RUN control, the typed outcome, the
score-less `Interrupted` row, and what the UI-level check actually asserts. Its `tests` list takes
only Rust sources by the matrix checker's design, so the product-level script is named in the summary
rather than faked into that field.

### One more real defect, found by looking at the screenshot

The Phase 5 geometry and accessibility passes were both green while the sidebar's collapse toggle was
rendering **with its outer eleven pixels cut off**. `.rail-toggle` is a 22px circle at `right: -11px`,
positioned to straddle the rail's edge, and `.shell-rail` had `overflow: hidden` — so the rail amputated
half of a focusable button, leaving a sliced chevron that looked like a font-loading fault.

Nothing measured it: the button is inside the rail, so it does not straddle a *scroll container*, and
the audit's clipping check only fires on content that cannot scroll to. `.nav-items` already sets
`overflow-y: auto; overflow-x: hidden` and clips itself, so the rail's clip was buying nothing. Removed
from the rail; the toggle now draws whole, confirmed by screenshot. This is the third time this project
has found a visual defect by looking rather than by measuring, which is an argument for keeping the
screenshot step in the loop rather than trusting the green.

### A tracked file had been deleted, and the build gate had been red ever since

`apps/desktop/src/vite-env.d.ts` — the Vite type reference, part of Vite's own scaffold — is tracked in
`HEAD` and was absent from the working tree. `Brand.test.tsx` imports the application icon as text to
assert the shipped icon matches its source, and without the `?raw` module declaration TypeScript
reports that import as an unresolvable module. The test still *ran* and passed under Vitest, so the
behaviour was never wrong — but `tsc -b` was failing, and nothing in this repository re-ran a full
`tsc -b` outside the one command that greps for specific errors. Restored, with the reason written into
the file so the next person to delete it can see what it is holding up.

---

Everything below was re-run against the final tree, with the release binary rebuilt after the last
source change.

| Gate | Result |
| --- | --- |
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | 0 diagnostics |
| `cargo test --workspace --all-targets --all-features --locked` | **932 passed, 0 failed** |
| `tsc -b` | 0 errors |
| `vitest run` | **443 passed, 22 files** |
| `pnpm build` | succeeds |
| `pnpm check:styles` | legibility floor OK · contrast contract OK |
| `pnpm check:icons` | 9 outputs match |
| `check-tauri-config.mjs` | valid against the bundled schema; production policy clean |
| `check-version-provenance.py` | three manifests, `STATUS.md` and `ARCHITECTURE.md` agree on 0.5.0 |
| `check-feature-matrix.py` | 35 capabilities consistent; 43 commands reachable |
| `check-unstyled-classes.py` | 0 unstyled |
| `grid-trace.mjs` | passing (board 389×389 at 1920×1080, no overflow, no new runs while replaying) |
| `cancel-through-ui.mjs` | 19 checks passing |
| `replay-integrity.mjs` | digests identical across a real restart |
| `grid-durability.mjs` | passing; scrubber moves 1,0 → 4,4 → 1,0 |
| `audit-a11y.mjs` | 11 routes, 0 failures |

### What remains, and what it is waiting on

`docs/RELEASE-CHECKLIST.md`. In order: generate the installer (authorization), sign it (a certificate),
verify install and uninstall on a clean machine (authorization), then tag, push and publish (each needs
its own authorization). No workflow builds, signs, or publishes anything, and no version is bumped
automatically.

---

## Constraints held throughout
No commit, push, tag, release, or installer. No assertion weakened to reach green. Every defect above
was confirmed by measurement before being fixed, and the two that had regression tests added were
verified to fail with the defect reintroduced.

### Known accepted limitations
- **The command center scrolls.** The board and both launch controls are above the fold at every target
  resolution; the progress hero and the evidence panels below them are not, and reaching them costs a
  scroll. The progress hero now largely duplicates what the stage's readout already states, which makes
  it the obvious candidate for that treatment.
- **The contrast meta tier is compressed.** `--ink-3` and `--ink-4` are now nine steps of red apart
  rather than sixty, because no value dark enough to read as clearly dimmer clears 4.5:1 on the hovered
  and pressed surfaces this app uses. The hierarchy between "meta" and "placeholder" is carried by weight
  and size instead. Deliberate, measured, and gated — but it is a cost, not a free win.
- **`cargo test` occasionally fails to link on this host** with `STATUS_ENTRYPOINT_NOT_FOUND` from
  `link.exe` under parallel load, and a stale `cookie` rlib diagnostic. Every affected binary links and
  runs on a second attempt; it is a host resource contention, not a defect in the tree, and no test has
  ever failed because of it.
- **Native integration tests that construct `AppState` cannot execute on this host.** They link, then
  fail to load with `STATUS_ENTRYPOINT_NOT_FOUND` from duplicate Windows DLL casings
  (`advapi32.dll`/`ADVAPI32.dll`, `ws2_32.dll`/`WS2_32.dll`) introduced by the full Tauri runtime. The
  command-level behaviour those binaries would cover is driven instead through the real executable by
  `scripts/cancel-through-ui.mjs` and `scripts/replay-integrity.mjs`, which is a stronger check but a
  different one — the Rust integration target remains unexecuted on this machine.
- **`--arena-chrome` is a hand-fitted constant, not a derived value.** Isolated in one declaration with
  its two components named separately; re-measure at 1366×768 after any change to the furniture above
  the Arena.
- **The installer is unsigned.** No code-signing certificate exists in this environment, and the
  remaining release steps — generate, sign, clean-install test, tag, push, publish — are each waiting on
  a person. `docs/RELEASE-CHECKLIST.md` says which.

### Retired limitations, and how

Recorded because "this was a known gap" and "this was quietly assumed fine" are different states, and
the second is how the contrast defect survived.

| Was | Now |
| --- | --- |
| "A single grid episode cannot be cancelled through the product… the UI offers no stop control at all. This is a gap, not a property." | STOP RUN exists, `cancelled` is a first-class terminal state with its own lifecycle row and no score, and 19 UI-level checks pass against a provider slow enough to stop. |
| "`scripts/audit-a11y.mjs` covers two routes… the others are unmeasured, and 'unmeasured' is not the same as 'clean'." | All 11 routes, parsed from the shell's own route type. Nine of them had real defects. |
| "roughly 80 declarations still use `--ink-4`, most correctly and some not." | The tokens themselves now meet 4.5:1 against every surface in the palette, and a CI gate keeps them there. |
| "Packaging configuration itself is complete and audited" — asserted in prose, gated by nothing. | `check-tauri-config.mjs` on CI, validated against the schema that ships in this repository, and verified to catch a deliberate typo. |