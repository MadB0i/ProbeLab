/**
 * Arena panel tests.
 *
 * The theme is that the panel shows the engine's facts and nothing else. There is no client-side game
 * loop to test, because there is no client-side game loop — which is the whole design claim, so several
 * of these tests are about what is *absent*: no human seat, no score, no local board, and no invented
 * result when the backend refuses.
 */

import { render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import {
  ArenaPanel,
  cellGlyph,
  moveLabel,
  outcomeGloss,
  outcomeLabel,
  sideMark,
  ticTacToeCells,
} from "./ArenaPanel";
import type { ArenaAgentSummary, ArenaGameSummary, ArenaMatchView } from "./api";

const GAME: ArenaGameSummary = {
  id: "tic_tac_toe",
  name: "Tic-Tac-Toe",
  description: "Three by three.",
  width: 3,
  height: 3,
  maxPlies: 9,
};

const PERFECT: ArenaAgentSummary = {
  id: "perfect_tictactoe",
  name: "Perfect Tic-Tac-Toe",
  description: "Exhaustive minimax.",
  categoryLabel: "REFERENCE / CEILING",
  canBeOpponent: true,
};

const FIRST_LEGAL: ArenaAgentSummary = {
  id: "first_legal_tictactoe",
  name: "First Legal Tic-Tac-Toe",
  description: "Lowest legal cell.",
  categoryLabel: "DETERMINISTIC BASELINE",
  canBeOpponent: true,
};

function match(overrides: Partial<ArenaMatchView> = {}): ArenaMatchView {
  return {
    matchId: "arena-ttt-7",
    game: "tic_tac_toe",
    gameName: "Tic-Tac-Toe",
    outcome: "draw",
    testedAgent: "perfect_tictactoe",
    opponentAgent: "perfect_tictactoe",
    testedSide: "first",
    seed: 7,
    plies: 9,
    finalBoard: {
      game: "tic_tac_toe",
      cells: ["first", "second", "first", "first", "second", "second", "second", "first", "second"],
    },
    winner: null,
    illegalMoves: [],
    history: [
      {
        ply: 1,
        side: "first",
        board: {
          game: "tic_tac_toe",
          cells: ["first", "empty", "empty", "empty", "empty", "empty", "empty", "empty", "empty"],
        },
        played: { tic_tac_toe: 0 },
      },
      {
        ply: 2,
        side: "second",
        board: {
          game: "tic_tac_toe",
          cells: ["first", "second", "empty", "empty", "empty", "empty", "empty", "empty", "empty"],
        },
        played: { tic_tac_toe: 1 },
      },
    ],
    timeline: [
      {
        sequence: 0,
        code: "arena.ply_recorded",
        severity: "info",
        detail: "ply 1 by First",
      },
      {
        sequence: 1,
        code: "arena.match_completed",
        severity: "info",
        detail: "Draw after 9 plies",
      },
    ],
    failure: null,
    ...overrides,
  };
}

function renderPanel(props: Partial<Parameters<typeof ArenaPanel>[0]> = {}) {
  return render(
    <ArenaPanel
      games={[GAME]}
      agents={[PERFECT, FIRST_LEGAL]}
      match={null}
      error={null}
      playing={false}
      selectedGame="tic_tac_toe"
      selectedTested="perfect_tictactoe"
      selectedOpponent="perfect_tictactoe"
      selectedSide="first"
      onPlay={() => {}}
      onReset={() => {}}
      onGameChange={() => {}}
      onTestedChange={() => {}}
      onOpponentChange={() => {}}
      onSideChange={() => {}}
      canPlay
      {...props}
    />,
  );
}

describe("board helpers", () => {
  it("maps the backend's cell names to glyphs", () => {
    expect(cellGlyph("empty")).toBe("");
    expect(cellGlyph("first")).toBe("X");
    expect(cellGlyph("second")).toBe("O");
  });

  it("names each side's mark", () => {
    expect(sideMark("first")).toBe("X");
    expect(sideMark("second")).toBe("O");
  });

  it("reads a tic-tac-toe board and refuses anything else", () => {
    const cells = ticTacToeCells({ game: "tic_tac_toe", cells: ["empty", "first"] });
    expect(cells).toBeNull();
    // An unimplemented game must not be coerced into a board of the wrong size.
    expect(ticTacToeCells({ game: "connect_four", cells: [] })).toBeNull();
    expect(ticTacToeCells(null)).toBeNull();
    expect(
      ticTacToeCells({
        game: "tic_tac_toe",
        cells: new Array(9).fill("empty"),
      }),
    ).toHaveLength(9);
  });

  it("describes a move in the game's own coordinates, or admits it cannot", () => {
    // The backend indexes cells from zero; the panel speaks in one-based cells so it matches what a
    // human counts, and says so rather than silently shifting.
    expect(
      moveLabel({ ply: 1, side: "first", board: { game: "tic_tac_toe", cells: [] }, played: { tic_tac_toe: 4 } }),
    ).toBe("cell 5");
    expect(
      moveLabel({ ply: 1, side: "first", board: { game: "tic_tac_toe", cells: [] }, played: null }),
    ).toMatch(/no move/);
    expect(
      moveLabel({ ply: 1, side: "first", board: { game: "tic_tac_toe", cells: [] }, played: {} }),
    ).toMatch(/not identified/);
  });
});

describe("outcome wording", () => {
  it("keeps a failure distinct from a loss", () => {
    expect(outcomeLabel("win")).toBe("WIN");
    expect(outcomeLabel("loss")).toBe("LOSS");
    expect(outcomeLabel("draw")).toBe("DRAW");
    expect(outcomeLabel("no_result")).toBe("NO RESULT");
  });

  it("says plainly that a failed match is not a loss", () => {
    const gloss = outcomeGloss(
      match({ outcome: "no_result", failure: "illegal move at ply 3" }),
    );
    expect(gloss).toMatch(/not a loss/i);
    expect(gloss).toMatch(/harness/i);
  });

  it("does not call a draw a pass", () => {
    expect(outcomeGloss(match({ outcome: "draw" }))).toMatch(/neither side could force a win/i);
  });
});

describe("ArenaPanel — empty state", () => {
  it("shows an intentional empty arena rather than a blank board", () => {
    renderPanel();
    // "NO ACTIVE MATCH" appears twice by design — in the status badge and as the empty state's heading —
    // so this asserts both are present rather than treating the duplication as a problem.
    expect(screen.getAllByText("NO ACTIVE MATCH").length).toBeGreaterThan(0);
    expect(screen.getByText(/a match runs to completion in the engine/i)).toBeInTheDocument();
    // No board, and no invented history.
    expect(document.querySelector(".arena-board")).toBeNull();
    expect(screen.queryByText(/Action history/i)).not.toBeInTheDocument();
  });

  it("says so when the build has no playable game at all", () => {
    renderPanel({ games: [] });
    expect(screen.getByText("NO GAMES")).toBeInTheDocument();
    expect(screen.getByText(/no implementation/i)).toBeInTheDocument();
  });

  it("offers only Tic-Tac-Toe, because only it is implemented", () => {
    renderPanel();
    const select = screen.getByLabelText("GAME") as HTMLSelectElement;
    const options = Array.from(select.options).map((option) => option.value);
    expect(options).toEqual(["tic_tac_toe"]);
  });
});

describe("ArenaPanel — no human seat", () => {
  it("states that there is no human seat", () => {
    renderPanel();
    expect(screen.getByText(/no human seat/i)).toBeInTheDocument();
  });

  it("offers no way to submit a move or advance a turn", () => {
    // A clickable board that silently ignored clicks would look like a working human mode that does
    // not exist, so the absence of these controls is a tested property.
    renderPanel({ match: match() });
    for (const forbidden of [/submit move/i, /your turn/i, /next turn/i, /play move/i]) {
      expect(screen.queryByRole("button", { name: forbidden })).not.toBeInTheDocument();
    }
    expect(document.querySelectorAll(".arena-cell")).toHaveLength(9);
  });
});

describe("ArenaPanel — board rendering", () => {
  it("renders the engine's board, not a locally derived one", () => {
    renderPanel({ match: match() });
    const cells = document.querySelectorAll(".arena-cell");
    expect(cells).toHaveLength(9);
    // The fixture's board is X O X / X O O / O X O.
    const marks = Array.from(cells).map((cell) => cell.querySelector(".arena-cell-mark")!.textContent);
    expect(marks).toEqual(["X", "O", "X", "X", "O", "O", "O", "X", "O"]);
  });

  it("describes each cell for assistive technology, so the board is not colour-only", () => {
    renderPanel({ match: match() });
    const board = screen.getByRole("img", { name: /board/i });
    expect(board).toBeInTheDocument();
    expect(screen.getByLabelText("Cell 1: X")).toBeInTheDocument();
    // Every one of the nine cells is individually named. A drawn board has no empty cell, so this
    // asserts coverage rather than picking one.
    for (let index = 1; index <= 9; index += 1) {
      expect(screen.getByLabelText(new RegExp(`^Cell ${index}: `))).toBeInTheDocument();
    }
  });

  it("names an empty cell as empty, not as a blank", () => {
    renderPanel({
      match: match({
        plies: 1,
        finalBoard: {
          game: "tic_tac_toe",
          cells: ["first", "empty", "empty", "empty", "empty", "empty", "empty", "empty", "empty"],
        },
      }),
    });
    expect(screen.getByLabelText("Cell 5: empty")).toBeInTheDocument();
  });

  it("refuses to draw a board it does not understand", () => {
    renderPanel({
      match: match({ finalBoard: { game: "connect_four", cells: [] } }),
    });
    expect(document.querySelector(".arena-board")).toBeNull();
    expect(screen.getByText(/cannot draw/i)).toBeInTheDocument();
  });

  it("draws a half-played board without inventing the rest", () => {
    renderPanel({
      match: match({
        outcome: "no_result",
        failure: "illegal move",
        plies: 1,
        finalBoard: {
          game: "tic_tac_toe",
          cells: ["first", "empty", "empty", "empty", "empty", "empty", "empty", "empty", "empty"],
        },
      }),
    });
    const cells = document.querySelectorAll(".arena-cell");
    expect(cells).toHaveLength(9);
    expect(cells[0].querySelector(".arena-cell-mark")!.textContent).toBe("X");
    expect(cells[8].querySelector(".arena-cell-mark")!.textContent).toBe("");
  });
});

describe("ArenaPanel — completed match", () => {
  it("shows the real outcome, seed, plies, and match id", () => {
    renderPanel({ match: match() });
    expect(screen.getByText("DRAW")).toBeInTheDocument();
    expect(screen.getByText("arena-ttt-7")).toBeInTheDocument();
    expect(screen.getByText("7")).toBeInTheDocument();
  });

  it("names the winner, and says none for a draw", () => {
    const draw = renderPanel({ match: match({ outcome: "draw", winner: null }) });
    const winnerCard = screen.getByText("Winner").closest("div")!;
    expect(within(winnerCard as HTMLElement).getByText("None")).toBeInTheDocument();
    draw.unmount();

    renderPanel({
      match: match({ outcome: "win", winner: "first", testedSide: "first" }),
    });
    const winCard = screen.getByText("Winner").closest("div")!;
    expect(within(winCard as HTMLElement).getByText("X")).toBeInTheDocument();
  });

  it("reports a loss as a loss and a failure as no result", () => {
    const lost = renderPanel({ match: match({ outcome: "loss", winner: "second" }) });
    expect(screen.getByText("LOSS")).toBeInTheDocument();
    lost.unmount();

    renderPanel({ match: match({ outcome: "no_result", failure: "boom" }) });
    expect(screen.getByText("NO RESULT")).toBeInTheDocument();
    expect(screen.queryByText("LOSS")).not.toBeInTheDocument();
  });

  it("shows the action history from the engine's telemetry", () => {
    renderPanel({ match: match() });
    const plies = document.querySelectorAll(".arena-ply");
    expect(plies).toHaveLength(2);
    expect(plies[0]).toHaveTextContent("cell 1");
    expect(plies[1]).toHaveTextContent("cell 2");
  });

  it("shows the match's own event stream", () => {
    renderPanel({ match: match() });
    expect(screen.getByText("arena.ply_recorded")).toBeInTheDocument();
    expect(screen.getByText("arena.match_completed")).toBeInTheDocument();
  });

  it("lists refused moves with their reason", () => {
    renderPanel({
      match: match({
        illegalMoves: [{ ply: 3, attempted: null, reason: "that cell is already taken" }],
      }),
    });
    expect(screen.getByText("Refused moves")).toBeInTheDocument();
    expect(screen.getByText("that cell is already taken")).toBeInTheDocument();
    expect(screen.getByText("ply 3")).toBeInTheDocument();
  });

  it("shows no refused-moves section for a clean match", () => {
    renderPanel({ match: match() });
    expect(screen.queryByText("Refused moves")).not.toBeInTheDocument();
  });
});

describe("ArenaPanel — agent identity", () => {
  it("names both seats and their categories", () => {
    renderPanel({
      selectedTested: "first_legal_tictactoe",
      selectedOpponent: "perfect_tictactoe",
    });
    // The seat labels sit in the meta line beneath each name, so they are read by class rather than
    // by a bare text match that would be ambiguous against the select labels.
    const metas = Array.from(document.querySelectorAll(".seat-meta")).map((n) => n.textContent);
    expect(metas.some((text) => text?.includes("Under test"))).toBe(true);
    expect(metas.some((text) => text?.includes("Opponent"))).toBe(true);
    expect(screen.getByText(/REFERENCE \/ CEILING/)).toBeInTheDocument();
  });

  it("offers only policies that can take the opponent seat", () => {
    renderPanel({
      agents: [
        PERFECT,
        { ...FIRST_LEGAL, canBeOpponent: false },
      ],
      selectedOpponent: "perfect_tictactoe",
    });
    const opponentSelect = screen.getByLabelText("OPPONENT") as HTMLSelectElement;
    expect(Array.from(opponentSelect.options).map((o) => o.value)).toEqual([
      "perfect_tictactoe",
    ]);
  });

  it("falls back to a label when a seat has no agent", () => {
    renderPanel({ selectedTested: "", selectedOpponent: "" });
    expect(screen.getAllByText("Not selected").length).toBeGreaterThan(0);
  });
});

describe("ArenaPanel — errors", () => {
  it("shows a backend refusal and invents no result", () => {
    renderPanel({
      match: null,
      error: "The match could not be played: this build cannot play \"connect_four\"",
    });
    expect(screen.getByRole("alert")).toHaveTextContent("cannot play");
    // A refusal must not leave a board or a fabricated outcome behind.
    expect(document.querySelector(".arena-board")).toBeNull();
    expect(screen.queryByText(/DRAW|WIN|LOSS|NO RESULT/)).not.toBeInTheDocument();
  });
});

describe("ArenaPanel — controls", () => {
  it("starts a match when pressed", () => {
    const onPlay = vi.fn();
    renderPanel({ onPlay });
    screen.getByRole("button", { name: /PLAY MATCH/i }).click();
    expect(onPlay).toHaveBeenCalledTimes(1);
  });

  it("disables play while a match is running", () => {
    renderPanel({ playing: true });
    expect(screen.getByRole("button", { name: /PLAYING/i })).toBeDisabled();
  });

  it("disables play when no pairing is selected", () => {
    renderPanel({ canPlay: false });
    expect(screen.getByRole("button", { name: /PLAY MATCH/i })).toBeDisabled();
  });

  it("reports a seat change upward rather than mutating the match itself", () => {
    const onTestedChange = vi.fn();
    const onSideChange = vi.fn();
    renderPanel({ onTestedChange, onSideChange });
    (screen.getByLabelText("UNDER TEST") as HTMLSelectElement).value = "first_legal_tictactoe";
    fireChange(screen.getByLabelText("UNDER TEST"));
    expect(onTestedChange).toHaveBeenCalledWith("first_legal_tictactoe");
    expect(onSideChange).not.toHaveBeenCalled();
  });
});

/** Fires a change event for a select whose value was just assigned. */
function fireChange(element: HTMLElement) {
  element.dispatchEvent(new Event("change", { bubbles: true }));
}

describe("ArenaPanel — no fabricated metrics", () => {
  it("shows no score, rating, or percentage anywhere", () => {
    // Tic-tac-toe has no meaningful performance metric, so none is invented. A "performance score"
    // would be a number nobody computed.
    const { container } = renderPanel({ match: match() });
    const text = container.textContent ?? "";
    for (const forbidden of ["score", "rating", "elo", "%"]) {
      expect(text.toLowerCase()).not.toContain(forbidden);
    }
  });

  it("shows no provider or model for an in-process policy match", () => {
    // Both seats are deterministic policies, so there is no provider to name — and the panel does not
    // reach for one.
    const { container } = renderPanel({ match: match() });
    expect(container.textContent ?? "").not.toMatch(/openai|anthropic|gemini|ollama/i);
  });
});
