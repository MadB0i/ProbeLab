/**
 * The Arena panel: a controlled game laboratory.
 *
 * # The board is a recording, not a state machine
 *
 * `play_match` in the engine is atomic — it runs a whole match and returns a result. So the board here
 * is a *frame* from the engine's own per-ply telemetry, not state the frontend is stepping. There is no
 * optimistic update, no local legality check, and no client-side game loop; React holds the current
 * frame and the backend holds the truth.
 *
 * That has a visible consequence worth being explicit about: **there is no "your turn" here.** A match
 * runs to completion, so a cell is never clickable. The panel says so rather than rendering an
 * inviting-looking board that does nothing, because a grid of apparently-interactive squares that
 * silently ignore clicks is worse than a static one.
 *
 * # A match survives a configuration change
 *
 * Changing a seat does not discard the match on screen. An earlier build cleared it, which meant that
 * adjusting a selector after a match silently destroyed the recording and orphaned its replay: the
 * backend still held it and the panel had thrown away the only key to it. The configuration and the
 * displayed match are now separate facts, and a mismatch is *stated* — a stale board is dimmed and
 * labelled with the configuration it came from, so the user decides when to replace it rather than
 * discovering later that it is gone.
 *
 * # Playback replays; it does not re-run
 *
 * `usePlayback` steps through the recorded positions. It contacts no backend and holds no engine, so
 * it cannot produce a match the recording does not already contain.
 *
 * # No winning line is synthesised
 *
 * `ArenaMatchView.winningLine` comes from the engine's own winning-line rule. If it is absent, no line
 * is drawn: a board nobody completed a line on has no line, and inferring one in the renderer would be
 * a second opinion about the result.
 *
 * # No score
 *
 * Tic-tac-toe has no meaningful performance metric, so none is shown. The panel reports the outcome,
 * the plies, and the illegal moves, which are the facts the engine actually produced.
 */

import { useMemo, useState } from "react";

import type {
  ArenaAgentSummary,
  ArenaGameSummary,
  ArenaMatchSummary,
  ArenaMatchView,
  GameCell,
  PlyView,
} from "./api";
import { PLAYBACK_SPEEDS, playbackFrames, usePlayback, type PlaybackSpeedLabel } from "./usePlayback";

/** The glyph a cell shows. Derived from the backend's own enum names. */
export function cellGlyph(cell: GameCell): string {
  switch (cell) {
    case "first":
      return "X";
    case "second":
      return "O";
    default:
      return "";
  }
}

/** Which mark a side plays, for the turn indicator. */
export function sideMark(side: "first" | "second"): string {
  return side === "first" ? "X" : "O";
}

/**
 * Reads the cells out of a board, or `null` when the board is not a tic-tac-toe board.
 *
 * Switching on the tag rather than casting is what keeps an unimplemented game from rendering as a
 * three-by-three grid of `undefined`.
 */
export function ticTacToeCells(board: PlyView["board"] | null): GameCell[] | null {
  if (!board || board.game !== "tic_tac_toe") {
    return null;
  }
  const cells = (board as { state?: { cells?: unknown } }).state?.cells;
  if (!Array.isArray(cells) || cells.length !== 9) {
    return null;
  }
  return cells as GameCell[];
}

/** Wording for the outcome, kept distinct from "loss". */
export function outcomeLabel(outcome: ArenaMatchView["outcome"]): string {
  switch (outcome) {
    case "win":
      return "WIN";
    case "loss":
      return "LOSS";
    case "draw":
      return "DRAW";
    default:
      return "NO RESULT";
  }
}

/** A short explanation of what the outcome means, shown under the headline. */
export function outcomeGloss(match: ArenaMatchView): string {
  if (match.failure) {
    return "The match produced no result. This is a harness outcome, not a loss — nothing was won or lost.";
  }
  switch (match.outcome) {
    case "draw":
      return "Neither side could force a win within the game's limit. Tic-tac-toe is fully solvable, so a draw means both sides played perfectly or the board filled.";
    case "win":
      return `${match.testedAgent} won playing ${match.testedSide}.`;
    case "loss":
      return `${match.opponentAgent} won. The tested policy played ${match.testedSide}.`;
    default:
      return "No result was recorded.";
  }
}

/** How the tested policy did, in words a reader can act on. */
export function testedResultLabel(match: ArenaMatchView): string {
  switch (match.outcome) {
    case "win":
      return "Under test WON";
    case "loss":
      return "Under test LOST";
    case "draw":
      return "Under test DREW";
    default:
      return "Under test: no result";
  }
}

/** Describes a move in the game's own vocabulary, or reports the absence honestly. */
export function moveLabel(ply: PlyView): string {
  if (!ply.played) {
    return "no move on this ply";
  }
  const move = (ply.played as { move?: unknown }).move;
  if (typeof move !== "number") {
    return "move not identified";
  }
  return `cell ${move + 1}`;
}

/** The cell index a ply played in, when the engine named one. */
function playedCell(ply: PlyView | null): number | null {
  if (!ply?.played) {
    return null;
  }
  const move = (ply.played as { move?: unknown }).move;
  return typeof move === "number" && move >= 0 && move <= 8 ? move : null;
}

/**
 * An identifier short enough to read on a panel, with the rest still reachable.
 *
 * A match id now carries a timestamp and a counter, so the full string runs past twenty characters
 * and used to wrap across four lines inside a narrow telemetry card. The tail is kept because it is
 * the part that distinguishes two matches of the same seed.
 */
export function shortId(id: string, keep = 14): string {
  if (id.length <= keep + 1) {
    return id;
  }
  return `${id.slice(0, keep)}…`;
}

/** When a match was played, or an honest "unknown" rather than a fabricated relative time. */
export function recordedLabel(unixMs: number, now = Date.now()): string {
  if (!Number.isFinite(unixMs) || unixMs <= 0) {
    return "time not recorded";
  }
  const elapsed = now - unixMs;
  if (elapsed < 0) {
    return new Date(unixMs).toLocaleString();
  }
  const minutes = Math.floor(elapsed / 60_000);
  if (minutes < 1) {
    return "just now";
  }
  if (minutes < 60) {
    return `${minutes} min ago`;
  }
  const hours = Math.floor(minutes / 60);
  if (hours < 24) {
    return `${hours} h ago`;
  }
  const days = Math.floor(hours / 24);
  if (days < 30) {
    return `${days} d ago`;
  }
  return new Date(unixMs).toLocaleDateString();
}

function Board({
  cells,
  winningLine,
  lastMove,
}: {
  cells: GameCell[];
  winningLine: readonly number[] | null;
  lastMove: number | null;
}) {
  const winning = useMemo(() => new Set(winningLine ?? []), [winningLine]);
  return (
    <div className="arena-board" role="img" aria-label="Tic-tac-toe board">
      {cells.map((cell, index) => {
        const classes = ["arena-cell", `arena-cell--${cell}`];
        if (winning.has(index)) {
          classes.push("arena-cell--winning");
        }
        if (lastMove === index) {
          classes.push("arena-cell--latest");
        }
        return (
          <div
            key={index}
            className={classes.join(" ")}
            // The cell index is the backend's own coordinate, so the accessible name matches the rules
            // the engine applied rather than a UI-invented numbering.
            aria-label={`Cell ${index + 1}: ${cell === "empty" ? "empty" : cellGlyph(cell)}${
              winning.has(index) ? ", winning line" : ""
            }${lastMove === index ? ", last move played" : ""}`}
          >
            <span className="arena-cell-mark">{cellGlyph(cell)}</span>
          </div>
        );
      })}
    </div>
  );
}

function Seat({
  label,
  agent,
  side,
  isTested,
}: {
  label: string;
  agent: ArenaAgentSummary | undefined;
  side: "first" | "second";
  isTested: boolean;
}) {
  return (
    <div className={`arena-seat${isTested ? " arena-seat--tested" : ""}`}>
      <span className="seat-mark">{sideMark(side)}</span>
      <div className="seat-body">
        <span className="seat-name">{agent?.name ?? "Not selected"}</span>
        <span className="seat-meta">
          {label}
          {agent ? ` · ${agent.categoryLabel}` : ""}
        </span>
      </div>
    </div>
  );
}

function Telemetry({ match }: { match: ArenaMatchView }) {
  const [copied, setCopied] = useState(false);

  /*
   * Copying the full identifier is the only way to get it out of the panel, and it is worth having:
   * the id is what ties a match to its recording and to a support question.
   */
  const copyId = async () => {
    try {
      await navigator.clipboard.writeText(match.matchId);
      setCopied(true);
      setTimeout(() => setCopied(false), 1600);
    } catch {
      setCopied(false);
    }
  };

  return (
    <div className="arena-telemetry">
      <div className="arena-telemetry-card arena-telemetry-card--wide">
        <span className="telemetry-label">Match</span>
        <button
          type="button"
          className="telemetry-value telemetry-copy"
          onClick={() => void copyId()}
          title={match.matchId}
          aria-label={`Match identifier ${match.matchId}. Activate to copy.`}
        >
          <span className="mono">{shortId(match.matchId)}</span>
          <span className="telemetry-copy-hint">{copied ? "copied" : "copy"}</span>
        </button>
      </div>
      <div className="arena-telemetry-card">
        <span className="telemetry-label">Seed</span>
        <span className="telemetry-value mono">{match.seed}</span>
      </div>
      <div className="arena-telemetry-card">
        <span className="telemetry-label">Plies</span>
        <span className="telemetry-value mono">{match.plies}</span>
      </div>
      <div className="arena-telemetry-card">
        <span className="telemetry-label">Winner</span>
        <span className="telemetry-value">
          {match.winner ? sideMark(match.winner) : "None"}
        </span>
      </div>
      <div className="arena-telemetry-card">
        <span className="telemetry-label">Illegal moves</span>
        <span className="telemetry-value mono">{match.illegalMoves.length}</span>
      </div>
      <div className="arena-telemetry-card">
        <span className="telemetry-label">Tested seat</span>
        <span className="telemetry-value">{match.testedSide}</span>
      </div>
    </div>
  );
}

/** The transport: first, previous, play/pause, next, last, a scrubber, and a speed. */
function Playback({ playback }: { playback: ReturnType<typeof usePlayback> }) {
  const { frames, position, current, playing, speed, atStart, atEnd } = playback;
  if (frames.length < 2) {
    return (
      <p className="panel-note">
        This match recorded {frames.length === 1 ? "one position" : "no positions"}, so there is
        nothing to step through. The final board above is the whole recording.
      </p>
    );
  }

  const ply = current?.ply;
  const moveCell = playedCell(ply ?? null);

  return (
    <div className="arena-playback">
      <div className="arena-playback-transport">
        <button
          type="button"
          className="arena-transport-button"
          onClick={playback.toStart}
          disabled={atStart}
          aria-label="Go to the first frame"
          title="First frame"
        >
          ⏮
        </button>
        <button
          type="button"
          className="arena-transport-button"
          onClick={() => playback.step(-1)}
          disabled={atStart}
          aria-label="Previous frame"
          title="Previous frame"
        >
          ◀
        </button>
        <button
          type="button"
          className="arena-transport-button arena-transport-button--primary"
          onClick={playback.toggle}
          aria-label={playing ? "Pause playback" : "Play the recorded match"}
          title={playing ? "Pause" : "Play"}
        >
          {playing ? "❚❚ PAUSE" : "▶ PLAY"}
        </button>
        <button
          type="button"
          className="arena-transport-button"
          onClick={() => playback.step(1)}
          disabled={atEnd}
          aria-label="Next frame"
          title="Next frame"
        >
          ▶
        </button>
        <button
          type="button"
          className="arena-transport-button"
          onClick={playback.toEnd}
          disabled={atEnd}
          aria-label="Go to the last frame"
          title="Last frame"
        >
          ⏭
        </button>
      </div>

      <label className="arena-scrubber">
        <span className="arena-scrubber-label">Frame</span>
        <input
          type="range"
          min={0}
          max={frames.length - 1}
          step={1}
          value={position}
          onChange={(event) => playback.setPosition(Number(event.target.value))}
          aria-label={`Frame position, ${position + 1} of ${frames.length}`}
        />
        <span className="arena-frame-counter mono" aria-live="polite">
          {position + 1} / {frames.length}
        </span>
      </label>

      <div className="arena-playback-meta">
        <span className="arena-now-playing">
          {ply
            ? `Ply ${ply.ply} · ${sideMark(ply.side)} (${ply.side}) · ${moveLabel(ply)}`
            : "Opening position — before any move"}
        </span>
        <label className="arena-speed">
          <span className="arena-speed-label">Speed</span>
          <select
            value={speed}
            onChange={(event) => playback.setSpeed(event.target.value as PlaybackSpeedLabel)}
            aria-label="Playback speed"
          >
            {PLAYBACK_SPEEDS.map((option) => (
              <option key={option.label} value={option.label}>
                {option.label}
              </option>
            ))}
          </select>
        </label>
      </div>

      {moveCell !== null ? (
        <p className="arena-move-note">The highlighted cell is the move the engine recorded on this ply.</p>
      ) : null}
    </div>
  );
}

function History({ match }: { match: ArenaMatchView }) {
  if (match.history.length === 0) {
    return <p className="panel-note">This match recorded no plies.</p>;
  }
  return (
    <ol className="arena-history">
      {match.history.map((ply) => (
        <li key={ply.ply} className="arena-ply">
          <span className="ply-number">{ply.ply}</span>
          <span className="ply-side">{sideMark(ply.side)}</span>
          <span className="ply-move">{moveLabel(ply)}</span>
        </li>
      ))}
    </ol>
  );
}

function MatchArchive({
  summaries,
  openId,
  loadingId,
  onOpen,
}: {
  summaries: ArenaMatchSummary[];
  openId: string | null;
  loadingId: string | null;
  onOpen: (matchId: string) => void;
}) {
  if (summaries.length === 0) {
    return (
      <p className="panel-note">
        No matches recorded yet. A match is written here the moment it finishes, and stays readable
        after the application is closed.
      </p>
    );
  }
  return (
    <ul className="arena-archive">
      {summaries.map((summary) => (
        <li key={summary.matchId}>
          <button
            type="button"
            className={`arena-archive-row${summary.matchId === openId ? " arena-archive-row--open" : ""}`}
            onClick={() => onOpen(summary.matchId)}
            disabled={loadingId !== null}
            aria-current={summary.matchId === openId ? "true" : undefined}
          >
            <span className={`arena-archive-outcome arena-archive-outcome--${summary.outcome}`}>
              {outcomeLabel(summary.outcome)}
            </span>
            <span className="arena-archive-main">
              <span className="arena-archive-pairing">
                {summary.testedAgent} vs {summary.opponentAgent}
              </span>
              <span className="arena-archive-meta">
                {summary.testedSide} · {summary.plies} {summary.plies === 1 ? "ply" : "plies"} ·{" "}
                {recordedLabel(summary.recordedUnixMs)}
              </span>
            </span>
            <span className="arena-archive-action">
              {loadingId === summary.matchId ? "OPENING…" : "OPEN"}
            </span>
          </button>
        </li>
      ))}
    </ul>
  );
}

export function ArenaPanel({
  games,
  agents,
  match,
  matchSummaries,
  openingId,
  error,
  playing,
  stale,
  selectedGame,
  selectedTested,
  selectedOpponent,
  selectedSide,
  onPlay,
  onReset,
  onOpenMatch,
  onGameChange,
  onTestedChange,
  onOpponentChange,
  onSideChange,
  canPlay,
}: {
  games: ArenaGameSummary[];
  agents: ArenaAgentSummary[];
  match: ArenaMatchView | null;
  matchSummaries: ArenaMatchSummary[];
  /** The identifier currently being fetched from the archive, if any. */
  openingId: string | null;
  error: string | null;
  playing: boolean;
  /** Whether the shown match came from a different configuration than the selectors describe. */
  stale: boolean;
  selectedGame: string;
  selectedTested: string;
  selectedOpponent: string;
  selectedSide: "first" | "second";
  onPlay: () => void;
  onReset: () => void;
  onOpenMatch: (matchId: string) => void;
  onGameChange: (game: string) => void;
  onTestedChange: (agent: string) => void;
  onOpponentChange: (agent: string) => void;
  onSideChange: (side: "first" | "second") => void;
  canPlay: boolean;
}) {
  const game = games.find((candidate) => candidate.id === selectedGame) ?? games[0];
  const tested = agents.find((agent) => agent.id === selectedTested);
  const opponent = agents.find((agent) => agent.id === selectedOpponent);
  const playback = usePlayback(match, { stale, autoPlay: true });
  const { frames, current } = playback;
  const shownCells = current?.cells ?? ticTacToeCells(match?.finalBoard ?? null);

  /*
   * The winning line is shown only on the final frame, where it is the line that decided the match.
   * Drawing it on an intermediate frame would be true of that board too, but it would read as "this
   * is how the match ended" on a board that has not ended, which is a different claim.
   */
  const atFinalFrame = frames.length > 0 && playback.position === frames.length - 1;
  const shownWinningLine =
    match && stale === false && atFinalFrame && match.outcome !== "draw" ? match.winningLine : null;

  if (error) {
    return (
      <section className="panel arena-panel" aria-labelledby="arena-heading">
        <div className="panel-heading">
          <h2 id="arena-heading">ARENA</h2>
          <span className="tone-error">COMMAND FAILED</span>
        </div>
        <p className="brain-error" role="alert">
          {error}
        </p>
        <p className="panel-note">
          The arena refuses a match rather than simulating one. Nothing on this panel is a placeholder.
        </p>
      </section>
    );
  }

  if (games.length === 0) {
    return (
      <section className="panel arena-panel" aria-labelledby="arena-heading">
        <div className="panel-heading">
          <h2 id="arena-heading">ARENA</h2>
          <span className="muted">NO GAMES</span>
        </div>
        <p className="panel-note">
          This build has no playable game registered. Connect four, rock-paper-scissors, and battleship
          have types in the schema but no implementation, and are deliberately not offered rather than
          offered and left to fail.
        </p>
      </section>
    );
  }

  const configurationSummary =
    `${tested?.name ?? "Not selected"} vs ${opponent?.name ?? "Not selected"} · ${selectedSide === "first" ? "First (X)" : "Second (O)"}`;

  return (
    <section className="panel arena-panel" aria-labelledby="arena-heading">
      <div className="panel-heading">
        <h2 id="arena-heading">ARENA</h2>
        <span className="arena-header-game">{game?.name ?? "—"}</span>
        <span
          className={
            match ? `arena-status arena-status--${match.outcome}` : "arena-status arena-status--idle"
          }
        >
          {match ? outcomeLabel(match.outcome) : "NO ACTIVE MATCH"}
        </span>
      </div>

      <div className="arena-layout">
        <div className="arena-main">
          {match && shownCells ? (
            <>
              {stale ? (
                <div className="arena-stale" role="status">
                  <span className="arena-stale-badge">PREVIOUS CONFIGURATION</span>
                  <p>
                    This board is the match that was played, not one your selectors describe now. It was{" "}
                    <strong>{match.testedAgent}</strong> against <strong>{match.opponentAgent}</strong>{" "}
                    playing <strong>{match.testedSide}</strong>. Your selectors now read{" "}
                    <strong>{configurationSummary}</strong>. Play a match to record the new pairing.
                  </p>
                </div>
              ) : null}
              {/*
                * The transport sits above the board rather than below it.
                *
                * A board is the tallest thing on this panel, and putting the controls after it meant
                * they landed below the fold on a 900px window — the controls for watching a match were
                * the one thing you had to scroll to find. Above the board they are visible while the
                * board is, which is what a viewer needs: the position, and the means to move it, at the
                * same time.
                */}
              {frames.length > 1 ? (
                <section className="brain-section arena-playback-section" aria-labelledby="arena-replay-heading">
                  <h3 id="arena-replay-heading">Watch this match</h3>
                  <p className="section-note">
                    Recorded positions from the engine&rsquo;s own telemetry, played back in order.
                    Playback reads the recording and contacts nothing — it cannot re-run the match or
                    reach a model.
                  </p>
                  <Playback playback={playback} />
                </section>
              ) : null}
              <div className="arena-board-frame">
                <Board
                  cells={shownCells}
                  winningLine={shownWinningLine}
                  lastMove={playedCell(current?.ply ?? null)}
                />
              </div>
              <p className="arena-outcome-gloss">{outcomeGloss(match)}</p>
            </>
          ) : match ? (
            <p className="panel-note">
              This match reports a board the panel cannot draw. It is shown in the telemetry below
              rather than guessed at.
            </p>
          ) : (
            <div className="arena-empty">
              <h3>NO ACTIVE MATCH</h3>
              <p>
                Choose a pairing below and start one. A match runs to completion in the engine, and the
                board shown is the position the engine recorded — not a local simulation.
              </p>
            </div>
          )}
        </div>

        <aside className="arena-side">
          <div className="arena-seats">
            <Seat label="Under test" agent={tested} side={selectedSide} isTested />
            <Seat
              label="Opponent"
              agent={opponent}
              side={selectedSide === "first" ? "second" : "first"}
              isTested={false}
            />
          </div>

          {match ? <Telemetry match={match} /> : null}

          {match && match.outcome !== "no_result" ? (
            <div className={`arena-result arena-result--${match.outcome}`}>
              <span className="arena-result-label">{testedResultLabel(match)}</span>
              <span className="arena-result-detail">
                {match.outcome === "draw"
                  ? "No side completed a line; the board filled."
                  : `${sideMark(match.winner ?? "first")} (${match.winner}) completed the winning line.`}
              </span>
            </div>
          ) : null}

          {match && match.failure ? (
            <div className="arena-result arena-result--failure">
              <span className="arena-result-label">MATCH FAILED</span>
              <span className="arena-result-detail">{match.failure}</span>
            </div>
          ) : null}

          {match && match.illegalMoves.length > 0 ? (
            <div className="arena-illegal">
              <h4>Refused moves</h4>
              <ul>
                {match.illegalMoves.map((illegal, index) => (
                  <li key={`${illegal.ply}-${index}`}>
                    <span className="illegal-ply">ply {illegal.ply}</span>
                    <span className="illegal-reason">{illegal.reason}</span>
                  </li>
                ))}
              </ul>
            </div>
          ) : null}
        </aside>
      </div>

      <div className="arena-controls">
        <div className="field-group">
          <label htmlFor="arena-game">GAME</label>
          <select
            id="arena-game"
            value={selectedGame}
            onChange={(event) => onGameChange(event.target.value)}
            disabled={playing}
          >
            {games.map((candidate) => (
              <option key={candidate.id} value={candidate.id}>
                {candidate.name}
              </option>
            ))}
          </select>
        </div>
        <div className="field-group">
          <label htmlFor="arena-tested">UNDER TEST</label>
          <select
            id="arena-tested"
            value={selectedTested}
            onChange={(event) => onTestedChange(event.target.value)}
            disabled={playing}
          >
            {agents.map((agent) => (
              <option key={agent.id} value={agent.id}>
                {agent.name}
              </option>
            ))}
          </select>
        </div>
        <div className="field-group">
          <label htmlFor="arena-opponent">OPPONENT</label>
          <select
            id="arena-opponent"
            value={selectedOpponent}
            onChange={(event) => onOpponentChange(event.target.value)}
            disabled={playing}
          >
            {agents
              .filter((agent) => agent.canBeOpponent)
              .map((agent) => (
                <option key={agent.id} value={agent.id}>
                  {agent.name}
                </option>
              ))}
          </select>
        </div>
        <div className="field-group">
          <label htmlFor="arena-side">TESTED SEAT</label>
          <select
            id="arena-side"
            value={selectedSide}
            onChange={(event) => onSideChange(event.target.value as "first" | "second")}
            disabled={playing}
          >
            <option value="first">First (X)</option>
            <option value="second">Second (O)</option>
          </select>
        </div>
        <button
          type="button"
          className="brain-run-button"
          onClick={onPlay}
          disabled={!canPlay || playing}
        >
          {playing ? "PLAYING…" : match ? "RUN AGAIN" : "PLAY MATCH"}
        </button>
        <button
          type="button"
          className="btn btn-ghost"
          onClick={onReset}
          disabled={!match || playing}
          title="Clear the board and return to setup. Stored matches are kept."
        >
          NEW MATCH
        </button>
      </div>

      {/*
       * There is no "submit a move" control and no "your turn" indicator, and both absences are
       * deliberate. `play_match` is atomic: a match runs to completion, so no turn is ever the
       * user's to take. A clickable board that silently ignored clicks would look like a working
       * human-versus-agent mode that does not exist.
       */}
      <p className="arena-no-human-note">
        Matches run agent against agent to completion. There is no human seat — the environment is the
        only thing that moves the board.
      </p>

      {match && frames.length < 2 ? (
        <section className="brain-section" aria-labelledby="arena-replay-heading">
          <h3 id="arena-replay-heading">Watch this match</h3>
          <Playback playback={playback} />
        </section>
      ) : null}

      <section className="brain-section" aria-labelledby="arena-archive-heading">
        <h3 id="arena-archive-heading">Match archive</h3>
        <p className="section-note">
          Every match this machine has played, newest first. Stored matches are never deleted by the
          controls on this page.
        </p>
        <MatchArchive
          summaries={matchSummaries}
          openId={match?.matchId ?? null}
          loadingId={openingId}
          onOpen={onOpenMatch}
        />
      </section>

      {match ? (
        <section className="brain-section">
          <h3>Action history</h3>
          <p className="section-note">
            One row per ply, from the engine&apos;s own telemetry. Each row carries the board the
            engine recorded for that position.
          </p>
          <History match={match} />
        </section>
      ) : null}

      {match && match.timeline.length > 0 ? (
        <section className="brain-section">
          <h3>Match events</h3>
          <ul className="brain-timeline">
            {match.timeline.map((event) => (
              <li key={event.sequence} className="brain-event tone-info">
                <span className="event-sequence">
                  {String(event.sequence).padStart(2, "0")}
                </span>
                <span className="event-code">{event.code}</span>
                <span className="event-severity">{event.severity}</span>
                <span className="event-detail">{event.detail}</span>
              </li>
            ))}
          </ul>
        </section>
      ) : null}
    </section>
  );
}