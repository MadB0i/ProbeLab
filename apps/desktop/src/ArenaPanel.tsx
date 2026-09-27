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
 * # No winning line
 *
 * The backend's `GameBoard` does not expose a winning line, and one is not synthesised here for the
 * sake of an animation. If a future game needs it, it belongs in the read model as a real field the
 * engine produces — not in the renderer.
 *
 * # No score
 *
 * Tic-tac-toe has no meaningful performance metric, so none is shown. The panel reports the outcome,
 * the plies, and the illegal moves, which are the facts the engine actually produced.
 */

import type {
  ArenaAgentSummary,
  ArenaGameSummary,
  ArenaMatchView,
  GameCell,
  PlyView,
} from "./api";

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
  const cells = (board as { cells?: unknown }).cells;
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

/** Describes a move in the game's own vocabulary, or reports the absence honestly. */
export function moveLabel(ply: PlyView): string {
  if (!ply.played) {
    return "no move on this ply";
  }
  const index = (ply.played as { tic_tac_toe?: number }).tic_tac_toe;
  if (typeof index !== "number") {
    return "move not identified";
  }
  return `cell ${index + 1}`;
}

function Board({ cells }: { cells: GameCell[] }) {
  return (
    <div className="arena-board" role="img" aria-label="Tic-tac-toe board">
      {cells.map((cell, index) => (
        <div
          key={index}
          className={`arena-cell arena-cell--${cell}`}
          // The cell index is the backend's own coordinate, so the accessible name matches the rules
          // the engine applied rather than a UI-invented numbering.
          aria-label={`Cell ${index + 1}: ${cell === "empty" ? "empty" : cellGlyph(cell)}`}
        >
          <span className="arena-cell-mark">{cellGlyph(cell)}</span>
        </div>
      ))}
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
  return (
    <div className="arena-telemetry">
      <div className="arena-telemetry-card">
        <span className="telemetry-label">Match</span>
        <span className="telemetry-value mono">{match.matchId}</span>
      </div>
      <div className="arena-telemetry-card">
        <span className="telemetry-label">Seed</span>
        <span className="telemetry-value mono">{match.seed}</span>
      </div>
      <div className="arena-telemetry-card">
        <span className="telemetry-label">Plies</span>
        <span className="telemetry-value mono">
          {match.plies} / {match.history.length > 0 ? match.history.length : match.plies}
        </span>
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

export function ArenaPanel({
  games,
  agents,
  match,
  error,
  playing,
  selectedGame,
  selectedTested,
  selectedOpponent,
  selectedSide,
  onPlay,
  onReset,
  onGameChange,
  onTestedChange,
  onOpponentChange,
  onSideChange,
  canPlay,
}: {
  games: ArenaGameSummary[];
  agents: ArenaAgentSummary[];
  match: ArenaMatchView | null;
  error: string | null;
  playing: boolean;
  selectedGame: string;
  selectedTested: string;
  selectedOpponent: string;
  selectedSide: "first" | "second";
  onPlay: () => void;
  onReset: () => void;
  onGameChange: (game: string) => void;
  onTestedChange: (agent: string) => void;
  onOpponentChange: (agent: string) => void;
  onSideChange: (side: "first" | "second") => void;
  canPlay: boolean;
}) {
  const game = games.find((candidate) => candidate.id === selectedGame) ?? games[0];
  const tested = agents.find((agent) => agent.id === selectedTested);
  const opponent = agents.find((agent) => agent.id === selectedOpponent);
  const cells = ticTacToeCells(match?.finalBoard ?? null);

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

  return (
    <section className="panel arena-panel" aria-labelledby="arena-heading">
      <div className="panel-heading">
        <h2 id="arena-heading">ARENA</h2>
        <span className="arena-header-game">{game?.name ?? "—"}</span>
        <span className={match ? `arena-status arena-status--${match.outcome}` : "arena-status arena-status--idle"}>
          {match ? outcomeLabel(match.outcome) : "NO ACTIVE MATCH"}
        </span>
      </div>

      <div className="arena-layout">
        <div className="arena-main">
          {match && cells ? (
            <>
              <div className="arena-board-frame">
                <Board cells={cells} />
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
            <Seat
              label="Under test"
              agent={tested}
              side={selectedSide}
              isTested
            />
            <Seat
              label="Opponent"
              agent={opponent}
              side={selectedSide === "first" ? "second" : "first"}
              isTested={false}
            />
          </div>

          {match ? <Telemetry match={match} /> : null}

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
          {playing ? "PLAYING…" : "PLAY MATCH"}
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

      {match ? (
        <section className="brain-section">
          <h3>Action history</h3>
          <p className="section-note">
            One row per ply, from the engine&rsquo;s own telemetry. Each row carries the board the
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
