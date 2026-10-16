import { describe, expect, it } from "vitest";

import {
  applyArenaTurn,
  arenaStartView,
  shouldInstallArenaResult,
  type LiveArenaView,
} from "./arenaLiveController";
import type { GameBoardView } from "./api";

/**
 * Regression tests for audit finding B5.
 *
 * The finding was that live Arena display never initialises and early cancellation has no target: the
 * match identifier was learned from the first completed turn, and the live board was built by folding
 * turns into a match that did not exist yet. Neither was testable where it lived.
 *
 * The audit additionally required: a start event, event filtering, and duplicate handling — all four are
 * asserted here rather than argued for in prose.
 */

const OPENING_TTT: GameBoardView = {
  game: "tic_tac_toe",
  state: { cells: new Array(9).fill("empty") },
};

const AFTER_FIRST_MOVE: GameBoardView = {
  game: "tic_tac_toe",
  state: { cells: ["first", "empty", "empty", "empty", "empty", "empty", "empty", "empty", "empty"] },
};

const start = (matchId = "m-1"): LiveArenaView =>
  arenaStartView({
    matchId,
    game: "tic_tac_toe",
    gameName: "Tic-Tac-Toe",
    openingBoard: OPENING_TTT,
    testedSide: "first",
    seed: 7,
    testedAgent: "greedy",
    opponentAgent: "optimal",
    testedSeat: "reference_policy",
    opponentSeat: "reference_policy",
  });

describe("a match is live before it has played a turn", () => {
  /*
   * The defect. Before the fix the panel had no match to draw until the first turn event arrived, which
   * for a real model is a full provider round trip away — so the stage stayed empty, and STOP had no
   * identifier to address.
   */
  it("installs a drawable match from the start event alone", () => {
    const view = start();
    expect(view.matchId).toBe("m-1");
    expect(view.finalBoard).toBe(OPENING_TTT);
    expect(view.turns).toHaveLength(0);
  });

  it("does not claim an outcome before anything has been played", () => {
    expect(start().outcome).toBe("no_result");
    expect(start().winner).toBeNull();
    expect(start().winningLine).toBeNull();
  });

  it("carries no illegal moves and no failure, because none occurred", () => {
    const view = start();
    expect(view.illegalMoves).toEqual([]);
    expect(view.failure).toBeNull();
    expect(view.cancelled).toBe(false);
  });

  it("uses the engine's own opening board rather than an empty placeholder", () => {
    const cells = (start().finalBoard as { state: { cells: string[] } }).state.cells;
    expect(cells).toHaveLength(9);
  });
});

describe("turns are correlated to the match being displayed", () => {
  /*
   * The batch overlap. A batch publishes turns on the same channel; folding one into the single-match
   * view would draw two different games on one stage.
   */
  it("ignores a turn belonging to another match", () => {
    const before = start("m-1");
    const after = applyArenaTurn(
      before,
      "some-other-match",
      { ply: 0, side: "first", requestBoard: AFTER_FIRST_MOVE },
    );
    expect(after).toBe(before);
    expect(after?.turns).toHaveLength(0);
  });

  it("folds in a turn belonging to the displayed match", () => {
    const after = applyArenaTurn(start("m-1"), "m-1", {
      ply: 0,
      side: "first",
      requestBoard: AFTER_FIRST_MOVE,
    });
    expect(after?.turns).toHaveLength(1);
    expect(after?.finalBoard).toBe(AFTER_FIRST_MOVE);
    expect(after?.history).toHaveLength(1);
  });

  it("ignores every turn when no match has been installed", () => {
    const after = applyArenaTurn(null, "m-1", {
      ply: 0,
      side: "first",
      requestBoard: AFTER_FIRST_MOVE,
    });
    expect(after).toBeNull();
  });
});

describe("duplicate turns do not corrupt the view", () => {
  /*
   * Required explicitly by the audit's event-correlation finding. A redelivered event is a transport
   * artefact; appending it would make the turn count disagree with the board drawn beside it.
   */
  it("replaces an earlier entry for the same ply instead of appending", () => {
    const turn = { ply: 0, side: "first" as const, requestBoard: AFTER_FIRST_MOVE };
    const once = applyArenaTurn(start(), "m-1", turn);
    const twice = applyArenaTurn(once, "m-1", turn);
    expect(twice?.turns).toHaveLength(1);
    expect(twice?.history).toHaveLength(1);
  });

  it("keeps turns ordered by ply regardless of arrival order", () => {
    const view = [
      { ply: 2, side: "first" as const },
      { ply: 0, side: "first" as const },
      { ply: 1, side: "second" as const },
    ].reduce<LiveArenaView | null>(
      (current, turn) => applyArenaTurn(current, "m-1", { ...turn, requestBoard: AFTER_FIRST_MOVE }),
      start(),
    );
    expect(view?.turns.map((entry) => entry.ply)).toEqual([0, 1, 2]);
    expect(view?.history.map((entry) => entry.ply)).toEqual([0, 1, 2]);
  });

  it("tracks the highest ply it has seen as the match's current position", () => {
    const view = [0, 4].reduce<LiveArenaView | null>(
      (current, ply) =>
        applyArenaTurn(current, "m-1", {
          ply,
          side: "first",
          requestBoard: AFTER_FIRST_MOVE,
        }),
      start(),
    );
    expect(view?.plies).toBe(4);
  });
});

describe("a terminal recording is accepted only from the match being followed", () => {
  it("accepts the terminal recording of the match it launched", () => {
    expect(shouldInstallArenaResult("m-1", "m-1")).toBe(true);
  });

  it("rejects another match's terminal recording", () => {
    expect(shouldInstallArenaResult("m-1", "m-2")).toBe(false);
  });

  it("accepts any recording when it is not following a specific match", () => {
    expect(shouldInstallArenaResult(null, "anything")).toBe(true);
  });

  it("rejects a batch recording while a single match is being followed", () => {
    expect(shouldInstallArenaResult("m-1", "arena-connect_four-1-2-3")).toBe(false);
  });
});

describe("a new match clears the previous one", () => {
  /*
   * Not a controller function, but the behaviour the audit described as "the UI can retain an old board
   * throughout a real-model match". Asserted as a property of the start view: it carries no turns and no
   * outcome from any predecessor, because it is constructed rather than inherited.
   */
  it("does not inherit the previous match's turns or outcome", () => {
    const view = applyArenaTurn(start("old"), "old", {
      ply: 3,
      side: "first",
      requestBoard: AFTER_FIRST_MOVE,
    });
    expect(view?.turns).toHaveLength(1);

    const fresh = start("new");
    expect(fresh.turns).toHaveLength(0);
    expect(fresh.history).toHaveLength(0);
    expect(fresh.plies).toBe(0);
    expect(fresh.matchId).toBe("new");
  });
});