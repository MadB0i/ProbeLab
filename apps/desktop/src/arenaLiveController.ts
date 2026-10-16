import { describe, expect, it } from "vitest";

import type { ArenaMatchView } from "./api";

/**
 * The live Arena controller, extracted from `App.tsx` and tested against the event stream.
 *
 * # Why this exists (audit finding B5)
 *
 * The live view used to be assembled inline inside `App.tsx`, where it is untestable without mounting
 * the entire application against a real IPC surface. The audit's finding was that a real-model match
 * never initialised its live view and had no cancellation target before its first turn — and neither
 * was reachable by a test, which is why both were true at once.
 *
 * So the rules are here, in pure functions over events, and `App.tsx` calls them. That makes three
 * things directly assertable:
 *
 * * a turn belonging to a *different* match is never folded into the displayed match;
 * * a redelivered turn replaces its earlier entry instead of inflating the count;
 * * a match is installed from its start event, with the engine's own opening board, before any turn.
 */

/** A match as the controller holds it while it runs. Mirrors the wire shape. */
export type LiveArenaView = ArenaMatchView;

export const ARENA_OPENING_OUTCOME = "no_result" as const;

type SeatSnapshot = { kind: "reference_policy" | "model" };

/**
 * Builds the match a start event installs.
 *
 * `outcome` is `no_result` rather than a win or a loss: nothing has been played. A live board that
 * claimed an outcome before the first ply would be asserting a result the engine has not produced.
 */
export function arenaStartView(params: {
  matchId: string;
  game: string;
  gameName: string;
  openingBoard: LiveArenaView["finalBoard"];
  testedSide: "first" | "second";
  seed: number;
  testedAgent: string;
  opponentAgent: string;
  testedSeat: SeatSnapshot["kind"];
  opponentSeat: SeatSnapshot["kind"];
}): LiveArenaView {
  return {
    matchId: params.matchId,
    game: params.game,
    gameName: params.gameName,
    outcome: ARENA_OPENING_OUTCOME,
    testedAgent: params.testedAgent,
    opponentAgent: params.opponentAgent,
    testedSeat: params.testedSeat,
    opponentSeat: params.opponentSeat,
    testedSide: params.testedSide,
    seed: params.seed,
    plies: 0,
    finalBoard: params.openingBoard,
    winner: null,
    winningLine: null,
    illegalMoves: [],
    history: [],
    turns: [],
    timeline: [],
    failure: null,
    cancelled: false,
  };
}

/** The fields a live turn contributes to the displayed match. */
export type ArenaTurnLike = {
  ply: number;
  side: "first" | "second";
  requestBoard: LiveArenaView["finalBoard"];
};

/**
 * Folds one live turn into the displayed match.
 *
 * Returns the current view unchanged when the turn belongs to another match. That is the whole point of
 * the function existing separately: the batch path publishes turns on the same channel, and without
 * this check a batch match would paint itself onto the single-match stage.
 *
 * A repeated ply *replaces* its entry. A transport that redelivers an event is ordinary, and appending
 * it would make the turn count disagree with the board.
 */
export function applyArenaTurn(
  current: LiveArenaView | null,
  matchId: string,
  turn: ArenaTurnLike,
): LiveArenaView | null {
  if (!current || current.matchId !== matchId) {
    return current;
  }
  const turns = [...current.turns.filter((entry) => entry.ply !== turn.ply), turn as never].sort(
    (left, right) => left.ply - right.ply,
  );
  const history = [
    ...current.history.filter((entry) => entry.ply !== turn.ply),
    { ply: turn.ply, side: turn.side, board: turn.requestBoard, played: null },
  ].sort((left, right) => left.ply - right.ply);
  return {
    ...current,
    turns,
    history,
    // The engine's own post-move position, never a locally derived one.
    finalBoard: turn.requestBoard,
    plies: turn.ply,
  };
}

/**
 * Whether a terminal recording may be installed.
 *
 * `expected` is the match this panel launched, or null when it is not following one. A batch plays
 * matches through the same engine and used to publish on the terminal channel, so without this a
 * batch's recording would replace the single-match view mid-batch.
 */
export function shouldInstallArenaResult(
  expected: string | null,
  matchId: string,
): boolean {
  return expected === null || expected === matchId;
}