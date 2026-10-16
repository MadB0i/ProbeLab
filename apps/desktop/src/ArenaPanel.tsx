/**
 * The Arena: a competitive board for two seats, live telemetry, and a replayable recording.
 *
 * # What this panel is and is not
 *
 * It renders a recording and drives playback. It does **not** implement a game, decide a winner, or
 * hold board state. Every board it draws came from the engine's `Game::view`, every move from the
 * telemetry the engine recorded, and every result from the engine's own match result. There is no
 * second opinion anywhere in this file: if the panel says someone won, the engine said so first.
 *
 * That is why the board is a *recording* rather than a state machine. `play_arena_match` runs the
 * whole match and returns it; there is no "your turn", because the match is over by the time anything
 * is drawn. A click that looks like a move would need a human-turn execution path with its own state
 * transitions and its own validation, and that is exactly the ambiguity this panel's credibility
 * depends on not having.
 *
 * # Two games, one component
 *
 * Tic-tac-toe and connect four are drawn by the same component through `boards.ts`. The board geometry
 * comes from the recording and from the catalog, and the two must agree or the panel says so rather
 * than stretching a three-by-three grid over a seven-by-six board.
 *
 * # Nothing here is invented
 *
 * No score, no rating, no percentage, no progress bar for work that is not happening, no
 * "thinking…" bubble. Every number on screen is either a recorded fact or is absent. A model that
 * failed to answer shows that failure; it does not show a spinner and then a zero.
 */

import { useMemo, type ReactNode } from "react";

import {
  boardCell,
  cellGlyph,
  cellSide,
  columnHeight,
  columnIsPlayable,
  geometryFor,
  geometryOf,
  sideMark,
  winningCoordinates,
  type BoardGeometry,
} from "./boards";
import {
  IconPlay,
  IconStop,
  IconArrowRight,
  IconRefresh,
  IconBoard,
  IconInfo,
  IconAlert,
  IconCheck,
  IconMark,
} from "./Icons";
import {
  lastMoveCell,
  usePlayback,
  PLAYBACK_SPEEDS,
  type PlaybackSpeedLabel,
  type PlaybackState,
} from "./usePlayback";
import { BatchStatistics, type BatchStatisticsProps } from "./BatchStatistics";
import type {
  ArenaAgentSummary,
  ArenaGameSummary,
  ArenaMatchSummary,
  ArenaMatchView,
  ArenaSeatRequest,
  GameBoardView,
  TurnTelemetry,
} from "./api";

/* -------------------------------------------------------------------------------------------- */
/* Labels                                                                                          */
/* -------------------------------------------------------------------------------------------- */

/**
 * The IPC contract version shared with the Rust `ModelRunConfig`.
 *
 * Duplicated here rather than imported because the arena's model seat is built from a *literal* in the
 * picker, and a literal needs a version. The Rust side refuses any other value, so a drift here
 * surfaces as a typed `unsupported_model_run_config_schema_version` refusal rather than as a run
 * against the wrong contract.
 */
export const MODEL_RUN_CONFIG_SCHEMA_VERSION = 1;

/** What a match produced, in words that do not blame a policy for a harness problem. */
export function outcomeLabel(outcome: ArenaMatchView["outcome"]): string {
  switch (outcome) {
    case "win":
      return "WIN";
    case "draw":
      return "DRAW";
    case "loss":
      return "LOSS";
    case "no_result":
      return "NO RESULT";
  }
}

/** One sentence saying what the outcome means, so a badge is never the only statement. */
export function outcomeGloss(view: ArenaMatchView): string {
  if (view.cancelled) {
    return `Stopped after ${view.plies} ${view.plies === 1 ? "turn" : "turns"}. The board is the one that was actually reached.`;
  }
  switch (view.outcome) {
    case "win":
      return `${view.testedAgent} took the ${view.testedSide} seat and completed a line.`;
    case "loss":
      return `${view.testedAgent} played the ${view.testedSide} seat and lost to ${view.opponentAgent}.`;
    case "draw":
      return `Neither seat completed a line across ${view.plies} ${
        view.plies === 1 ? "turn" : "turns"
      }.`;
    case "no_result":
      return view.failure
        ? "The match did not reach a result. Nothing was won or lost."
        : "The match did not reach a result.";
  }
}

/** The result as the tested seat's own record states it. */
export function testedResultLabel(view: ArenaMatchView): string {
  if (view.cancelled) {
    return "CANCELLED";
  }
  return outcomeLabel(view.outcome);
}

/** A move, as a person would say it. */
export function moveLabel(played: unknown): string {
  const move = (played as { move?: unknown } | null)?.move;
  if (typeof move !== "number") {
    return "—";
  }
  const game = (played as { game?: string } | null)?.game;
  if (game === "connect_four") {
    return `column ${move + 1}`;
  }
  return `cell ${move + 1}`;
}

/** A run identifier, abbreviated for display but never truncated in the accessible name. */
export function shortId(id: string): string {
  return id.length <= 18 ? id : `${id.slice(0, 10)}…${id.slice(-6)}`;
}

/** A wall-clock label for a recorded match, or a refusal when the timestamp is absurd. */
export function recordedLabel(unixMs: number): string {
  if (!Number.isFinite(unixMs) || unixMs <= 0) {
    return "time not recorded";
  }
  const date = new Date(unixMs);
  if (Number.isNaN(date.getTime())) {
    return "time not recorded";
  }
  return date.toLocaleString();
}

/** Token usage, or an explicit absence. Zero is never used to mean "unknown". */
function usageLabel(turn: TurnTelemetry): string {
  if (turn.usage === null) {
    return turn.seat === "model" ? "tokens not reported" : "policy — no tokens";
  }
  const total = turn.usage.totalTokens;
  if (total === null) {
    return "tokens not reported";
  }
  const prompt = turn.usage.promptTokens;
  const completion = turn.usage.completionTokens;
  if (prompt !== null && completion !== null) {
    return `${total} tokens (${prompt} in, ${completion} out)`;
  }
  return `${total} tokens`;
}

/* -------------------------------------------------------------------------------------------- */
/* The board                                                                                       */
/* -------------------------------------------------------------------------------------------- */

/**
 * The board, drawn from recorded data.
 *
 * Every cell is a button-shaped element with an accessible name, because a board is information a
 * screen reader has to be able to read. The `role="img"` wrapper carries a summary for the layout and
 * the individual cells carry the detail; a board with no names at all is a picture of a board.
 */
function Board({
  board,
  geometry,
  winningLine,
  lastMove,
  activeSide,
  dimmed,
}: {
  board: GameBoardView | null;
  geometry: BoardGeometry | null;
  winningLine: [number, number][];
  lastMove: { column: number; row: number } | null;
  activeSide: "first" | "second" | null;
  dimmed: boolean;
}) {
  if (!geometry || !board) {
    return (
      <div className="arena-board arena-board--empty">
        <p className="muted">This recording has no board this build can draw.</p>
      </div>
    );
  }
  /*
   * Grouped into rows, because `role="grid"` requires it.
   *
   * `grid` has required owned children of role `row`, and `gridcell` has a required parent of role
   * `row`. The cells were rendered flat -- 42 `gridcell` spans directly inside the grid -- so the
   * structure was invalid by the spec's own definition. It read as nothing at all: axe reported
   * `aria-required-children` on the board and `aria-required-parent` on all 42 cells. A screen reader
   * cannot announce "row 3, column 4" from a structure that has no row 3.
   *
   * The names on each cell were already correct, which is why the defect was invisible to the unit
   * tests: they queried by role and found the cells, and role queries do not validate the tree.
   */
  const rows: ReactNode[] = [];
  for (let row = 0; row < geometry.rows; row += 1) {
    const rowCells: ReactNode[] = [];
    for (let column = 0; column < geometry.columns; column += 1) {
      const cell = boardCell(board, column, row);
      const side = cellSide(cell);
      const onWinningLine = winningLine.some(([c, r]) => c === column && r === row);
      const isLastMove = lastMove?.column === column && lastMove.row === row;
      const classes = [
        "arena-cell",
        side ? `arena-cell--${side}` : "arena-cell--empty",
        geometry.game === "connect_four" ? "arena-cell--disc" : "",
        onWinningLine ? "arena-cell--winning" : "",
        isLastMove ? "arena-cell--last" : "",
        activeSide && side === activeSide ? "arena-cell--active" : "",
      ]
        .filter(Boolean)
        .join(" ");
      // The name is spelled out rather than left to a glyph: "row 2, column 4, second side's disc"
      // reads correctly to a screen reader and still matches what is painted.
      const name = `${geometry.game === "connect_four" ? "Row" : "Cell"} ${row + 1}, column ${
        column + 1
      }${side ? `, ${side} side${onWinningLine ? ", winning line" : ""}${
        isLastMove ? ", last move played" : ""
      }` : ", empty"}`;
      rowCells.push(
        <span
          key={`${column}-${row}`}
          className={classes}
          role="gridcell"
          aria-label={name}
          aria-current={isLastMove ? "true" : undefined}
        >
          <span className="arena-cell__mark" aria-hidden="true">
            {cellGlyph(cell)}
          </span>
        </span>,
      );
    }
    rows.push(
      <div key={`row-${row}`} role="row" className="arena-row">
        {rowCells}
      </div>,
    );
  }
  return (
    <div
      className={`arena-board arena-board--${geometry.game}${dimmed ? " is-dimmed" : ""}`}
      role="grid"
      aria-label={`${geometry.game === "connect_four" ? "Connect four" : "Tic-tac-toe"} board, ${geometry.columns} by ${geometry.rows}`}
      aria-rowcount={geometry.rows}
      aria-colcount={geometry.columns}
    >
      {rows}
    </div>
  );
}

/**
 * A connect-four board drawn with visible column structure.
 *
 * The column bars are `aria-hidden` decoration. They exist because a 7x6 grid of discs is much harder
 * to read as a *game* than the same grid with the columns outlined — and because the thing a connect
 * four player reads is column height, which an outlined column makes immediate. They carry no
 * information the cells do not already carry in their own names.
 */
function ConnectFourFrame({
  geometry,
  children,
}: {
  geometry: BoardGeometry;
  children: ReactNode;
}) {
  // Only the tray is hidden. `aria-hidden` on this wrapper would also hide the board it contains, and
  // the board is the one thing on the panel a screen reader most needs: each of its 42 cells carries a
  // spoken name like "Row 6, column 4, second side's disc". Hiding it left connect four entirely
  // unreadable while tic-tac-toe, which has no wrapper, read correctly.
  return (
    <div className="arena-c4">
      <div className="arena-c4__tray" aria-hidden="true">
        {Array.from({ length: geometry.columns }, (_, column) => (
          <span key={column} className="arena-c4__slot" />
        ))}
      </div>
      {children}
    </div>
  );
}

/* -------------------------------------------------------------------------------------------- */
/* Seats                                                                                           */
/* -------------------------------------------------------------------------------------------- */

function Seat({
  title,
  agent,
  side,
  seatKind,
  active,
  lines,
}: {
  title: string;
  agent: string;
  side: "first" | "second";
  seatKind: "reference_policy" | "model";
  active: boolean;
  lines: [string, string][];
}) {
  return (
    <div
      className={`arena-seat arena-seat--${side}${active ? " is-active" : ""}`}
      data-seat={seatKind}
    >
      <header className="arena-seat__head">
        <span className="arena-seat__mark" aria-hidden="true">
          {sideMark(side)}
        </span>
        <div>
          <p className="arena-seat__title">{title}</p>
          <p className="arena-seat__agent">{agent}</p>
        </div>
      </header>
      <p className="arena-seat__kind">
        {seatKind === "model" ? "REAL MODEL" : "REFERENCE POLICY"}
      </p>
      <dl className="arena-seat__meta">
        {lines.map(([label, value]) => (
          <div key={label}>
            <dt>{label}</dt>
            <dd>{value}</dd>
          </div>
        ))}
      </dl>
      <span className="visually-hidden">
        {active ? `${title} is the active seat.` : `${title} is not the active seat.`}
      </span>
    </div>
  );
}

/* -------------------------------------------------------------------------------------------- */
/* Telemetry                                                                                       */
/* -------------------------------------------------------------------------------------------- */

/**
 * What one turn cost.
 *
 * Shown from the engine's own per-turn record. `null` is rendered as an explicit "not reported"
 * rather than as a zero, because a provider that does not report tokens and a policy that has none
 * are different facts and a zero would read as either.
 */
function TurnTelemetryPanel({ turn }: { turn: TurnTelemetry | null }) {
  if (!turn) {
    return (
      <section className="arena-telemetry" aria-labelledby="arena-telemetry-heading">
        <h3 id="arena-telemetry-heading" className="arena-subhead">
          LAST TURN
        </h3>
        <p className="muted">Move the timeline to a recorded turn to see what it cost.</p>
      </section>
    );
  }
  return (
    <section className="arena-telemetry" aria-labelledby="arena-telemetry-heading">
      <h3 id="arena-telemetry-heading" className="arena-subhead">
        TURN {turn.ply} TELEMETRY
      </h3>
      <dl className="arena-telemetry__grid">
        <div>
          <dt>Seat</dt>
          <dd>
            {turn.seatLabel}
            <span className="arena-tag">{turn.seat === "model" ? "MODEL" : "POLICY"}</span>
          </dd>
        </div>
        <div>
          <dt>Side to move</dt>
          <dd>{turn.side === "first" ? "First" : "Second"}</dd>
        </div>
        <div>
          <dt>Decision time</dt>
          <dd>{formatMillis(turn.elapsedMs)}</dd>
        </div>
        <div>
          <dt>Token usage</dt>
          <dd>{usageLabel(turn)}</dd>
        </div>
        <div>
          <dt>Prompt template</dt>
          <dd>{turn.promptTemplateVersion ?? "not applicable"}</dd>
        </div>
      </dl>
      {turn.failure ? (
        <p className="arena-telemetry__failure" role="status">
          <IconAlert size={14} /> {turn.failure.message}
        </p>
      ) : null}
      {turn.responseText ? (
        <details className="arena-telemetry__reply">
          <summary>Model reply</summary>
          <pre>{turn.responseText}</pre>
        </details>
      ) : null}
    </section>
  );
}

/** A duration in the largest two units that stay readable. */
export function formatMillis(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) {
    return "—";
  }
  if (ms < 1000) {
    return `${Math.round(ms)} ms`;
  }
  if (ms < 60_000) {
    return `${(ms / 1000).toFixed(1)} s`;
  }
  const minutes = Math.floor(ms / 60_000);
  const seconds = Math.round((ms % 60_000) / 1000);
  return `${minutes} m ${seconds.toString().padStart(2, "0")} s`;
}

/* -------------------------------------------------------------------------------------------- */
/* Playback                                                                                        */
/* -------------------------------------------------------------------------------------------- */

/**
 * The transport.
 *
 * Every control is an SVG icon with an accessible name and a tooltip. The previous transport used
 * Unicode transport glyphs (`⏮ ◀ ▶ ⏭ ❚❚`), which read as decoration to a screen reader, rendered
 * differently by every font, and looked like a media player bolted onto scientific software. These
 * are drawn, labelled, and sized with the rest of the interface.
 */
function Playback({
  match,
  playback,
}: {
  match: ArenaMatchView | null;
  /**
   * The panel's single playback state.
   *
   * Passed in rather than created here, and that is the whole point: a transport that owned its own
   * position would move to the final frame while the board sat on the opening one, and both would
   * render confidently. One hook, one position, one frame counter.
   */
  playback: PlaybackState;
}) {
  if (!match || playback.frames.length < 2) {
    /*
     * Fewer than two positions is not a replay, it is a single board.
     *
     * The alternative is a transport with every step button disabled, a scrubber pinned at both ends,
     * and a play control that does nothing — three dead controls and a lie about what the panel can
     * do. A match that recorded no plies, or exactly one, says so plainly instead.
     */
    return (
      <section className="arena-playback" aria-labelledby="arena-replay-heading">
        <h3 id="arena-replay-heading" className="arena-subhead">
          REPLAY
        </h3>
        <p className="muted">
          {match
            ? "This match recorded one position, so there is nothing to step through."
            : "Nothing to step through yet."}
        </p>
      </section>
    );
  }
  const frameCount = playback.frames.length;
  return (
    <section className="arena-playback" aria-labelledby="arena-replay-heading">
      <h3 id="arena-replay-heading" className="arena-subhead">
        REPLAY
      </h3>
      <div className="arena-transport">
        <button
          type="button"
          className="btn btn-icon"
          onClick={playback.toStart}
          disabled={playback.atStart}
          aria-label="Go to the opening position"
          title="Go to the opening position"
        >
          <IconArrowRight className="arena-icon-flip" />
        </button>
        <button
          type="button"
          className="btn btn-icon"
          onClick={() => playback.step(-1)}
          disabled={playback.atStart}
          aria-label="Step back one turn"
          title="Step back one turn"
        >
          <IconArrowRight className="arena-icon-flip" />
        </button>
        <button
          type="button"
          className="btn btn-primary arena-transport__play"
          onClick={playback.toggle}
          aria-label={playback.playing ? "Pause playback" : "Play the recorded match"}
        >
          {playback.playing ? <IconStop size={14} /> : <IconPlay size={14} />}
          <span>{playback.playing ? "PAUSE" : "PLAY"}</span>
        </button>
        <button
          type="button"
          className="btn btn-icon"
          onClick={() => playback.step(1)}
          disabled={playback.atEnd}
          aria-label="Step forward one turn"
          title="Step forward one turn"
        >
          <IconArrowRight />
        </button>
        <button
          type="button"
          className="btn btn-icon"
          onClick={playback.toEnd}
          disabled={playback.atEnd}
          aria-label="Go to the final position"
          title="Go to the final position"
        >
          <IconArrowRight />
        </button>
      </div>
      <label className="arena-scrub">
        <span className="visually-hidden">
          Frame position, {playback.position + 1} of {frameCount}
        </span>
        <input
          type="range"
          min={0}
          max={Math.max(frameCount - 1, 0)}
          value={playback.position}
          onChange={(event) => playback.setPosition(Number(event.target.value))}
          aria-valuetext={`Frame ${playback.position + 1} of ${frameCount}${
            playback.current?.ply ? `, after turn ${playback.current.ply.ply}` : ", the opening position"
          }`}
        />
      </label>
      <div className="arena-transport__meta">
        <p className="arena-frame-counter" aria-live="polite">
          FRAME {playback.position + 1} / {frameCount}
          {playback.current?.ply ? ` · after turn ${playback.current.ply.ply}` : " · opening position"}
        </p>
        <label className="arena-speed">
          <span className="visually-hidden">Playback speed</span>
          <select
            value={playback.speed}
            onChange={(event) => playback.setSpeed(event.target.value as PlaybackSpeedLabel)}
            aria-label="Playback speed"
          >
            {PLAYBACK_SPEEDS.map((speed) => (
              <option key={speed.label} value={speed.label}>
                {speed.label}
              </option>
            ))}
          </select>
        </label>
      </div>
    </section>
  );
}

/* -------------------------------------------------------------------------------------------- */
/* History and archive                                                                             */
/* -------------------------------------------------------------------------------------------- */

function History({ match }: { match: ArenaMatchView | null }) {
  if (!match || match.history.length === 0) {
    return (
      <section aria-labelledby="arena-history-heading">
        <h3 id="arena-history-heading" className="arena-subhead">
          TURN RECORD
        </h3>
        <p className="muted">This match recorded no turns.</p>
      </section>
    );
  }
  return (
    <section aria-labelledby="arena-history-heading">
      <h3 id="arena-history-heading" className="arena-subhead">
        TURN RECORD
      </h3>
      <ol className="arena-history">
        {match.history.map((ply) => (
          <li key={`${ply.ply}-${ply.side}`} className={`arena-history__row arena-history__row--${ply.side}`}>
            <span className="arena-history__ply">{ply.ply}</span>
            <span className="arena-history__side">{ply.side === "first" ? "FIRST" : "SECOND"}</span>
            <span className="arena-history__move">{moveLabel(ply.played)}</span>
            <span className="arena-history__cost">
              {ply.ply - 1 < match.turns.length
                ? formatMillis(match.turns[ply.ply - 1].elapsedMs)
                : "—"}
            </span>
          </li>
        ))}
      </ol>
    </section>
  );
}

function MatchArchive({
  summaries,
  openingId,
  onOpen,
}: {
  summaries: ArenaMatchSummary[];
  openingId: string | null;
  onOpen: (matchId: string) => void;
}) {
  return (
    <section className="arena-archive" aria-labelledby="arena-archive-heading">
      <h3 id="arena-archive-heading" className="arena-subhead">
        MATCH ARCHIVE
      </h3>
      {summaries.length === 0 ? (
        <p className="muted">No matches recorded yet.</p>
      ) : (
        <ul className="arena-archive__list">
          {summaries.map((summary) => (
            <li key={summary.matchId}>
              <button
                type="button"
                className="arena-archive__row"
                aria-current={summary.matchId === openingId ? "true" : undefined}
                disabled={openingId !== null}
                onClick={() => onOpen(summary.matchId)}
              >
                <span className={`arena-archive__outcome arena-archive__outcome--${summary.outcome}`}>
                  {outcomeLabel(summary.outcome)}
                </span>
                <span className="arena-archive__meta">
                  {summary.game} · {summary.plies} plies · seed {summary.seed}
                </span>
                <span className="arena-archive__seats">
                  {summary.testedAgent} vs {summary.opponentAgent}
                </span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

/* -------------------------------------------------------------------------------------------- */
/* The panel                                                                                       */
/* -------------------------------------------------------------------------------------------- */

export type ArenaPanelProps = {
  games: ArenaGameSummary[];
  agents: ArenaAgentSummary[];
  match: ArenaMatchView | null;
  summaries: ArenaMatchSummary[];
  game: string;
  testedSeat: ArenaSeatRequest;
  opponentSeat: ArenaSeatRequest;
  testedSide: "first" | "second";
  seed: number;
  /** Locally installed models, for the model-seat picker. */
  localModels: string[];
  /** The provider configuration the model seats are built from. */
  modelConfig: { baseUrl: string; apiKeyEnv: string; timeoutSeconds: number };
  onGameChange: (game: string) => void;
  onTestedSeatChange: (seat: ArenaSeatRequest) => void;
  onOpponentSeatChange: (seat: ArenaSeatRequest) => void;
  onSideChange: (side: "first" | "second") => void;
  onSeedChange: (seed: number) => void;
  onPlay: () => void;
  onCancel: () => void;
  onOpen: (matchId: string) => void;
  playing: boolean;
  openingId: string | null;
  error: string | null;
  canPlay: boolean;
  /**
   * Why a single match cannot start, rendered beside the disabled control.
   *
   * Required rather than optional: an empty array would hide the explanation, and the whole point of
   * carrying the reasons out of `arenaMatchEligibility` is that a refusal is never silent.
   */
  eligibilityReasons: string[];
  /** Everything the batch statistics panel needs, passed through so this panel stays one component. */
  batch: BatchStatisticsProps;
};

export function ArenaPanel(props: ArenaPanelProps) {
  const {
    games,
    agents,
    match,
    summaries,
    game,
    testedSeat,
    opponentSeat,
    testedSide,
    seed,
    localModels,
    modelConfig,
    onGameChange,
    onTestedSeatChange,
    onOpponentSeatChange,
    onSideChange,
    onSeedChange,
    onPlay,
    onCancel,
    onOpen,
    playing,
    openingId,
    error,
    canPlay,
    eligibilityReasons,
    batch,
  } = props;

  const playback = usePlayback(match, {
    stale: staleFor(match, testedSeat, opponentSeat, game, testedSide),
    autoPlay: true,
  });
  const frame = playback.current;
  const board = frame?.board ?? match?.finalBoard ?? null;
  const geometry = geometryOf(board) ?? geometryFor(match?.game ?? game);

  // The winning line is only shown on the final frame, and only when it matches the board it is
  // claimed against. Showing it earlier would highlight cells that are not yet the winner's.
  const atFinalFrame = playback.atEnd;
  const winningLine = useMemo(() => {
    if (!match || !atFinalFrame || match.outcome === "draw" || match.cancelled) {
      return [];
    }
    return winningCoordinates(board, match.winningLine, match.winner);
  }, [match, atFinalFrame, board]);

  const lastMove = frame?.ply ? lastMoveCell(board, frame.ply.played) : null;
  const activeSide = frame?.ply?.side ?? null;

  if (error) {
    return (
      <section className="panel arena-panel" aria-labelledby="arena-heading">
        <h2 id="arena-heading">ARENA</h2>
        <p className="brain-error" role="alert">
          <IconAlert size={14} /> {error}
        </p>
      </section>
    );
  }

  const seatLines = (view: ArenaMatchView | null, side: "first" | "second"): [string, string][] => {
    if (!view) {
      return [];
    }
    const totalMs = view.turns.reduce((total, turn) => total + turn.elapsedMs, 0);
    const tokens = view.turns.reduce<number | null>((carry, turn) => {
      if (turn.usage?.totalTokens === null || turn.usage === null) {
        return carry;
      }
      return (carry ?? 0) + turn.usage.totalTokens;
    }, null);
    return [
      ["Turns", String(view.plies)],
      ["Decision time", formatMillis(totalMs)],
      ["Tokens", tokens === null ? "not reported" : String(tokens)],
      [
        "Invalid moves",
        view.illegalMoves.length === 0 ? "none" : String(view.illegalMoves.length),
      ],
    ];
  };

  const testedAgentLabel = match?.testedAgent ?? seatLabel(testedSeat);
  const opponentAgentLabel = match?.opponentAgent ?? seatLabel(opponentSeat);
  const testedSeatKind =
    match?.testedSeat ?? (testedSeat.kind === "model" ? "model" : "reference_policy");
  const opponentSeatKind =
    match?.opponentSeat ?? (opponentSeat.kind === "model" ? "model" : "reference_policy");
  const testedSideNow = match?.testedSide ?? testedSide;

  return (
    <section className="panel arena-panel" aria-labelledby="arena-heading">
      <header className="arena-head">
        <div>
          <p className="eyebrow">
            <IconBoard size={14} /> COMPETITIVE BENCH
          </p>
          <h2 id="arena-heading">{match?.gameName ?? "Arena"}</h2>
        </div>
        {match ? (
          <div className="arena-head__verdict">
            <span className={`arena-verdict arena-verdict--${match.outcome}`}>
              {testedResultLabel(match)}
            </span>
            <button
              type="button"
              className="btn btn-ghost btn-sm"
              onClick={() => {
                void navigator.clipboard?.writeText(match.matchId);
              }}
              aria-label={`Match identifier ${match.matchId}. Activate to copy.`}
              title={match.matchId}
            >
              <span className="mono">{shortId(match.matchId)}</span>
            </button>
          </div>
        ) : (
          <span className="arena-head__verdict arena-verdict--none">NO MATCH</span>
        )}
      </header>

      {match ? (
        <p className={`arena-gloss arena-gloss--${match.outcome}`} role="status">
          {match.cancelled ? <IconStop size={14} /> : null}
          {match.outcome === "win" ? <IconCheck size={14} /> : null}
          {match.outcome === "no_result" ? <IconAlert size={14} /> : null}
          {outcomeGloss(match)}
        </p>
      ) : null}

      {match?.failure ? (
        <p className="arena-failure" role="note">
          <IconAlert size={14} />
          <span>
            <strong>Why there is no result:</strong> {match.failure}
          </span>
        </p>
      ) : null}

      {playback.stale && match ? (
        <p className="arena-stale" role="status">
          <IconInfo size={14} /> The selectors have changed since this match was played. The recording
          below is what actually happened.
        </p>
      ) : null}

      <div className="arena-stage">
        <div className="arena-board-column">
          {match ? (
            geometry && geometry.game === "connect_four" ? (
              <ConnectFourFrame geometry={geometry}>
                <Board
                  board={board}
                  geometry={geometry}
                  winningLine={winningLine}
                  lastMove={lastMove}
                  activeSide={activeSide}
                  dimmed={false}
                />
              </ConnectFourFrame>
            ) : (
              <Board
                board={board}
                geometry={geometry}
                winningLine={winningLine}
                lastMove={lastMove}
                activeSide={activeSide}
                dimmed={false}
              />
            )
          ) : (
            <div className="arena-board arena-board--idle">
              <p className="arena-board__idle-text">NO ACTIVE MATCH</p>
              <p className="muted">
                Seat two competitors and start one. Every move is played by the engine; nothing here
                simulates a decision.
              </p>
            </div>
          )}
        </div>

        {/*
       * A plain `div`, not `<aside>`.
       *
       * A complementary landmark is only meaningful at the top level of a page. This rail sits inside
       * the arena's own labelled `<section>`, and nesting a landmark inside another landmark is what
       * axe calls `landmark-complementary-is-top-level`. The label is kept, so the rail is still
       * named and announced; only the landmark role is dropped.
       */}
      <div className="arena-rail" aria-label="Competitors and live statistics">
          <Seat
            title="UNDER TEST"
            agent={testedAgentLabel}
            side={testedSideNow}
            seatKind={testedSeatKind}
            active={activeSide === testedSideNow}
            lines={seatLines(match, testedSideNow)}
          />
          <Seat
            title="OPPONENT"
            agent={opponentAgentLabel}
            side={testedSideNow === "first" ? "second" : "first"}
            seatKind={opponentSeatKind}
            active={activeSide !== null && activeSide !== testedSideNow}
            lines={match ? [] : []}
          />
          <TurnTelemetryPanel turn={frame?.turn ?? null} />
        </div>
      </div>

      <div className="arena-controls">
        <Playback match={match} playback={playback} />

        <section className="arena-setup" aria-labelledby="arena-setup-heading">
          <h3 id="arena-setup-heading" className="arena-subhead">
            MATCH SETUP
          </h3>
          <div className="arena-setup__grid">
            <label className="field">
              <span>Game</span>
              <select
                value={game}
                onChange={(event) => onGameChange(event.target.value)}
                disabled={playing}
              >
                {games.map((entry) => (
                  <option key={entry.id} value={entry.id}>
                    {entry.name} · {entry.width}×{entry.height}
                  </option>
                ))}
              </select>
            </label>
            <SeatPicker
              label="Under test"
              role="tested"
              agents={agents}
              seat={testedSeat}
              localModels={localModels}
              modelConfig={modelConfig}
              disabled={playing}
              onChange={onTestedSeatChange}
            />
            <SeatPicker
              label="Opponent"
              role="opponent"
              agents={agents}
              seat={opponentSeat}
              localModels={localModels}
              modelConfig={modelConfig}
              disabled={playing}
              onChange={onOpponentSeatChange}
            />
            <label className="field">
              <span>Tested side</span>
              <select
                value={testedSide}
                onChange={(event) => onSideChange(event.target.value as "first" | "second")}
                disabled={playing}
              >
                <option value="first">First — moves first</option>
                <option value="second">Second — moves second</option>
              </select>
            </label>
            <label className="field">
              <span>Seed</span>
              <input
                type="number"
                min={0}
                step={1}
                value={seed}
                onChange={(event) => onSeedChange(Number(event.target.value) || 0)}
                disabled={playing}
              />
            </label>
          </div>
          <div className="arena-setup__actions">
            <button
              type="button"
              className="btn btn-primary btn-lg"
              onClick={onPlay}
              disabled={!canPlay || playing}
            >
              {playing ? (
                <>
                  <IconRefresh size={14} /> PLAYING…
                </>
              ) : match ? (
                <>
                  <IconPlay size={14} /> RUN AGAIN
                </>
              ) : (
                <>
                  <IconBoard size={14} /> PLAY MATCH
                </>
              )}
            </button>
            {playing ? (
              <button type="button" className="btn btn-danger" onClick={onCancel}>
                <IconStop size={14} /> STOP
              </button>
            ) : null}
          </div>
          {/*
            Why Play is unavailable, beside the control that is unavailable.

            A disabled button with no stated reason is a control the operator has to reverse-engineer.
            `arenaEligibility` supplies a sentence for every refusal, including the one that made the
            model workflow unreachable in the first place.
          */}
          {!canPlay && eligibilityReasons.length > 0 ? (
            <ul className="arena-setup__blockers" role="status">
              {eligibilityReasons.map((reason) => (
                <li key={reason}>{reason}</li>
              ))}
            </ul>
          ) : null}
          {playing ? (
            <p className="arena-setup__note" role="status">
              A match against a real model runs one provider call per turn, so it takes real time.
              Turn events stream in as the engine applies them; stopping takes effect at the next turn
              boundary and the match is recorded as far as it actually got.
            </p>
          ) : null}
        </section>

        <div className="arena-secondary">
          <History match={match} />
          <MatchArchive summaries={summaries} openingId={openingId} onOpen={onOpen} />
        </div>
      </div>

      {/*
       * Batch statistics sit BELOW the panel's two columns, spanning the full width, rather than
       * inside the control column.
       *
       * Nested inside `.arena-controls` first, which was wrong in a way no unit test could see: the
       * panel grid stretches its left column to match the taller one, so a statistics block ~2000px tall
       * dragged the stage to 3000px and pushed the board itself to y=1157 — entirely below the fold of
       * a 900px window. The board is the panel's subject; a reading of many matches must never be able
       * to push the subject off the screen.
       */}
      <BatchStatistics {...batch} />
    </section>
  );
}

/** The label for a seat, whether or not it has been played yet. */
export function seatLabel(seat: ArenaSeatRequest): string {
  return seat.kind === "policy" ? seat.id : seat.label;
}

/**
 * Whether the recording on screen belongs to the selectors as they stand now.
 *
 * The seed is deliberately excluded, exactly as it was before: changing the seed changes what *would*
 * be played, not what *was* recorded, and flagging the recording as stale for an unused control would
 * be a warning about a change the user never asked for. The seat identity, the game, and the side are
 * the three things that change what a match means.
 */
export function staleFor(
  match: ArenaMatchView | null,
  tested: ArenaSeatRequest,
  opponent: ArenaSeatRequest,
  game: string,
  testedSide: "first" | "second",
): boolean {
  if (!match) {
    return false;
  }
  return (
    match.game !== game ||
    match.testedAgent !== seatLabel(tested) ||
    match.opponentAgent !== seatLabel(opponent) ||
    match.testedSide !== testedSide
  );
}

/**
 * A picker that can seat either a shipped policy or a locally installed model.
 *
 * The model list is populated from what the local executor actually reported, so a name in the picker
 * is a model that exists. A free-text field would let an operator type a name that has not been pulled
 * and then discover it at ply one.
 */
function SeatPicker({
  label,
  role,
  agents,
  seat,
  localModels,
  modelConfig,
  disabled,
  onChange,
}: {
  label: string;
  /**
   * Which seat this picker fills — `tested` or `opponent`.
   *
   * Passed rather than derived from `label`, because the visible label is copy that may be reworded and a
   * test hook derived from copy breaks the moment it is. This is the stable name.
   */
  role: "tested" | "opponent";
  agents: ArenaAgentSummary[];
  seat: ArenaSeatRequest;
  localModels: string[];
  modelConfig: { baseUrl: string; apiKeyEnv: string; timeoutSeconds: number };
  disabled: boolean;
  onChange: (seat: ArenaSeatRequest) => void;
}) {
  const value = seat.kind === "policy" ? `policy:${seat.id}` : `model:${seat.label}`;
  return (
    /*
     * `data-seat` names which picker this is.
     *
     * The two pickers are visually and semantically identical — same label shape, same option groups —
     * so an end-to-end script driving the real panel had no way to address "the tested seat" as opposed
     * to whichever came first in the document. That is a testability gap caused by a missing label, not
     * a product defect, and it is cheaper to label the control than to have the script guess by index.
     */
    <label className="field" data-seat={role}>
      <span>{label}</span>
      <select
        value={value}
        disabled={disabled}
        aria-label={`${label} participant`}
        onChange={(event) => {
          const chosen = event.target.value;
          if (chosen.startsWith("policy:")) {
            onChange({ kind: "policy", id: chosen.slice("policy:".length) });
            return;
          }
          const model = chosen.slice("model:".length);
          onChange({
            kind: "model",
            label: model,
            config: {
              kind: "openai_compatible",
              // The IPC payload for a nested request is snake_case throughout: the arena command's own
              // fields are camelCase, but the provider configuration is a Rust struct that is not
              // `rename_all`-ed, and sending camelCase here would decode to the schema default and
              // silently run against the wrong contract version.
              schema_version: MODEL_RUN_CONFIG_SCHEMA_VERSION,
              base_url: modelConfig.baseUrl,
              api_key_env: modelConfig.apiKeyEnv,
              model,
              timeout_seconds: modelConfig.timeoutSeconds,
              temperature: 0,
              max_tokens: 200,
            },
          });
        }}
      >
        <optgroup label="Shipped policies">
          {agents.map((agent) => (
            <option key={agent.id} value={`policy:${agent.id}`}>
              {agent.name} — {agent.categoryLabel}
            </option>
          ))}
        </optgroup>
        <optgroup label="Local models">
          {localModels.length === 0 ? (
            <option value="model:" disabled>
              none detected
            </option>
          ) : (
            localModels.map((model) => (
              <option key={model} value={`model:${model}`}>
                {model}
              </option>
            ))
          )}
        </optgroup>
      </select>
    </label>
  );
}

/** Whether a column can accept a disc, used by the board's hover affordance. */
export function playableColumn(board: GameBoardView | null, column: number): boolean {
  return columnIsPlayable(board, column);
}
