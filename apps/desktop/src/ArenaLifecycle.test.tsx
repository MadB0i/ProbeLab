/**
 * The Arena's experiment lifecycle: keeping a match, replacing it, and finding it again.
 *
 * # Why these exist
 *
 * An earlier build cleared the displayed match whenever a selector changed. Changing the tested seat
 * after a match therefore destroyed the recording on screen *and* discarded its identifier, which also
 * made the replay unreachable even though the backend still held both. Every test below is about that
 * class of failure: a match that was played must stay reachable until the user says otherwise, and no
 * view control may delete a stored recording.
 */

import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ArenaPanel, shortId, testedResultLabel } from "./ArenaPanel";
import type {
  ArenaAgentSummary,
  ArenaGameSummary,
  ArenaMatchSummary,
  ArenaMatchView,
} from "./api";

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

const FULL_BOARD = {
  game: "tic_tac_toe" as const,
  state: {
    cells: [
      "first",
      "second",
      "first",
      "empty",
      "empty",
      "empty",
      "empty",
      "empty",
      "empty",
    ] as const,
  },
};

function match(overrides: Partial<ArenaMatchView> = {}): ArenaMatchView {
  return {
    matchId: "arena-ttt-7-1700000000000-1",
    game: "tic_tac_toe",
    gameName: "Tic-Tac-Toe",
    outcome: "win",
    testedAgent: "first_legal_tictactoe",
    opponentAgent: "perfect_tictactoe",
    testedSide: "first",
    seed: 7,
    plies: 3,
    finalBoard: FULL_BOARD as never,
    winner: "first",
    winningLine: [0, 1, 2],
    illegalMoves: [],
    history: [
      {
        ply: 1,
        side: "first",
        board: {
          game: "tic_tac_toe",
          state: {
            cells: ["first", "empty", "empty", "empty", "empty", "empty", "empty", "empty", "empty"],
          },
        },
        played: { game: "tic_tac_toe", move: 0 },
      },
      {
        ply: 2,
        side: "second",
        board: {
          game: "tic_tac_toe",
          state: {
            cells: ["first", "second", "empty", "empty", "empty", "empty", "empty", "empty", "empty"],
          },
        },
        played: { game: "tic_tac_toe", move: 1 },
      },
      {
        ply: 3,
        side: "first",
        board: FULL_BOARD as never,
        played: { game: "tic_tac_toe", move: 2 },
      },
    ],
    timeline: [],
    failure: null,
    ...overrides,
  };
}

function summary(overrides: Partial<ArenaMatchSummary> = {}): ArenaMatchSummary {
  return {
    matchId: "arena-ttt-7-1700000000000-1",
    game: "tic_tac_toe",
    outcome: "win",
    testedAgent: "first_legal_tictactoe",
    opponentAgent: "perfect_tictactoe",
    testedSide: "first",
    seed: 7,
    plies: 3,
    recordedUnixMs: 1_700_000_000_000,
    digest: "c71fe9311f42bdde",
    ...overrides,
  };
}

function renderPanel(props: Partial<Parameters<typeof ArenaPanel>[0]> = {}) {
  return render(
    <ArenaPanel
      games={[GAME]}
      agents={[PERFECT, FIRST_LEGAL]}
      match={null}
      matchSummaries={[]}
      openingId={null}
      stale={false}
      error={null}
      playing={false}
      selectedGame="tic_tac_toe"
      selectedTested="first_legal_tictactoe"
      selectedOpponent="perfect_tictactoe"
      selectedSide="first"
      onPlay={() => {}}
      onReset={() => {}}
      onOpenMatch={() => {}}
      onGameChange={() => {}}
      onTestedChange={() => {}}
      onOpponentChange={() => {}}
      onSideChange={() => {}}
      canPlay
      {...props}
    />,
  );
}

describe("ArenaPanel — a completed match survives a configuration change", () => {
  /*
   * The regression test for the defect: a selector change used to clear the match, which destroyed
   * the recording and orphaned its replay. The panel now keeps the board and marks it stale.
   */
  it("keeps the board and the recorded match when the selectors no longer describe it", () => {
    renderPanel({ match: match(), stale: true, selectedSide: "second" });
    // The board is still there.
    expect(document.querySelectorAll(".arena-cell")).toHaveLength(9);
    // The outcome is still there.
    expect(screen.getByText("WIN")).toBeInTheDocument();
    // And the mismatch is stated rather than hidden.
    expect(screen.getByText(/PREVIOUS CONFIGURATION/i)).toBeInTheDocument();
    expect(screen.getByText(/Play a match to record the new pairing/i)).toBeInTheDocument();
  });

  it("does not mark a match stale when the configuration still matches", () => {
    renderPanel({ match: match(), stale: false });
    expect(screen.queryByText(/PREVIOUS CONFIGURATION/i)).not.toBeInTheDocument();
  });

  it("still offers to run again while showing a stale match", () => {
    renderPanel({ match: match(), stale: true, selectedSide: "second" });
    expect(screen.getByRole("button", { name: /run again/i })).toBeEnabled();
  });
});

describe("ArenaPanel — the controls after a match", () => {
  it("offers run again and new match, and says what new match does", () => {
    renderPanel({ match: match() });
    expect(screen.getByRole("button", { name: /run again/i })).toBeInTheDocument();
    const reset = screen.getByRole("button", { name: /new match/i });
    expect(reset).toBeEnabled();
    // The title states that this is a view operation and deletes nothing.
    expect(reset).toHaveAttribute(
      "title",
      expect.stringContaining("Stored matches are kept"),
    );
  });

  it("cannot reset with nothing on screen, because there is nothing to reset", () => {
    renderPanel({ match: null });
    expect(screen.getByRole("button", { name: /new match/i })).toBeDisabled();
    expect(screen.getByRole("button", { name: /play match/i })).toBeEnabled();
  });

  it("names a run-again action distinctly from the first match", () => {
    renderPanel({ match: null });
    expect(screen.getByRole("button", { name: /^play match$/i })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /run again/i })).not.toBeInTheDocument();
  });
});

describe("ArenaPanel — the outcome", () => {
  it("reports the result for the tested policy, not only the winner", () => {
    renderPanel({ match: match() });
    expect(screen.getByText("Under test WON")).toBeInTheDocument();
  });

  it("says the board filled for a draw rather than naming a line nobody completed", () => {
    renderPanel({
      match: match({ outcome: "draw", winner: null, winningLine: null }),
    });
    expect(screen.getByText("Under test DREW")).toBeInTheDocument();
    expect(screen.getByText(/No side completed a line/i)).toBeInTheDocument();
    expect(document.querySelector(".arena-cell--winning")).toBeNull();
  });

  it("keeps a harness failure distinct from a loss", () => {
    renderPanel({
      match: match({
        outcome: "no_result",
        winner: null,
        winningLine: null,
        failure: "the engine refused the opening move",
      }),
    });
    expect(screen.getByText("MATCH FAILED")).toBeInTheDocument();
    expect(screen.getByText("the engine refused the opening move")).toBeInTheDocument();
    expect(screen.queryByText("Under test LOST")).not.toBeInTheDocument();
  });

  it("highlights the winning line the engine recorded, on the final frame only", () => {
    renderPanel({ match: match() });
    expect(document.querySelectorAll(".arena-cell--winning")).toHaveLength(0);
    fireEvent.click(screen.getByLabelText("Go to the last frame"));
    expect(document.querySelectorAll(".arena-cell--winning")).toHaveLength(3);
  });

  it("does not highlight a line the engine did not report", () => {
    renderPanel({ match: match({ winningLine: null, outcome: "draw", winner: null }) });
    fireEvent.click(screen.getByLabelText("Go to the last frame"));
    expect(document.querySelectorAll(".arena-cell--winning")).toHaveLength(0);
  });

  it("describes the winning line and the latest move for assistive technology", () => {
    renderPanel({ match: match() });
    fireEvent.click(screen.getByLabelText("Go to the last frame"));
    // Three cells carry the line and one carries the move, so the query is scoped to the board.
    const board = screen.getByRole("img", { name: /board/i });
    expect(within(board).getAllByLabelText(/winning line/i)).toHaveLength(3);
    expect(within(board).getAllByLabelText(/last move played/i)).toHaveLength(1);
  });
});

describe("ArenaPanel — identifiers", () => {
  it("truncates a long identifier instead of wrapping it across lines", () => {
    renderPanel({ match: match() });
    const copy = screen.getByRole("button", { name: /match identifier/i });
    expect(copy.textContent).not.toContain(match().matchId);
    // The full identifier stays reachable through the accessible name and the tooltip.
    expect(copy).toHaveAttribute("title", match().matchId);
    expect(copy).toHaveAccessibleName(expect.stringContaining(match().matchId));
  });

  it("leaves a short identifier alone", () => {
    expect(shortId("arena-7")).toBe("arena-7");
    expect(shortId("arena-ttt-7-1")).toBe("arena-ttt-7-1");
  });

  it("keeps the tail, because that is what separates two matches of one seed", () => {
    expect(shortId("arena-ttt-7-1700000000000-1", 10)).toBe("arena-ttt-…");
    expect(shortId("arena-ttt-7-1700000000000-1", 14)).toBe("arena-ttt-7-17…");
  });
});

describe("ArenaPanel — playback controls", () => {
  it("offers a transport, a scrubber, a speed, and a frame counter", () => {
    renderPanel({ match: match() });
    expect(screen.getByLabelText("Go to the first frame")).toBeInTheDocument();
    expect(screen.getByLabelText("Previous frame")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /play|pause/i })).toBeInTheDocument();
    expect(screen.getByLabelText("Next frame")).toBeInTheDocument();
    expect(screen.getByLabelText("Go to the last frame")).toBeInTheDocument();
    expect(screen.getByLabelText(/frame position/i)).toBeInTheDocument();
    expect(screen.getByLabelText("Playback speed")).toBeInTheDocument();
    expect(screen.getByText(/\/ 4$/)).toBeInTheDocument();
  });

  it("names the move currently on screen", () => {
    renderPanel({ match: match() });
    fireEvent.click(screen.getByLabelText("Go to the last frame"));
    expect(screen.getByText(/Ply 3 · X \(first\) · cell 3/)).toBeInTheDocument();
  });

  it("says the opening frame is before any move", () => {
    renderPanel({ match: match() });
    expect(screen.getByText(/Opening position — before any move/i)).toBeInTheDocument();
  });

  it("disables previous at the start and next at the end", () => {
    renderPanel({ match: match() });
    expect(screen.getByLabelText("Previous frame")).toBeDisabled();
    fireEvent.click(screen.getByLabelText("Go to the last frame"));
    expect(screen.getByLabelText("Next frame")).toBeDisabled();
    expect(screen.getByLabelText("Previous frame")).toBeEnabled();
  });

  it("says so rather than offering a transport for a single recorded position", () => {
    renderPanel({
      match: match({ history: [], plies: 0, outcome: "no_result", winner: null, winningLine: null }),
    });
    expect(screen.queryByLabelText("Next frame")).not.toBeInTheDocument();
    expect(screen.getByText(/nothing to step through/i)).toBeInTheDocument();
  });

  it("states that playback contacts nothing", () => {
    renderPanel({ match: match() });
    expect(
      screen.getByText(/reads the recording and contacts nothing/i),
    ).toBeInTheDocument();
  });
});

describe("ArenaPanel — the match archive", () => {
  it("says there is nothing recorded, and that stored matches are not deleted from here", () => {
    renderPanel({ matchSummaries: [] });
    expect(screen.getByText(/No matches recorded yet/i)).toBeInTheDocument();
  });

  it("lists stored matches newest first with their real metadata", () => {
    renderPanel({
      matchSummaries: [
        summary(),
        summary({
          matchId: "arena-ttt-7-1700000000100-2",
          outcome: "draw",
          plies: 9,
          recordedUnixMs: 1_700_000_000_100,
        }),
      ],
    });
    const rows = screen.getAllByRole("button", { name: /OPEN/ });
    expect(rows).toHaveLength(2);
    expect(within(rows[0]).getByText("WIN")).toBeInTheDocument();
    expect(within(rows[0]).getByText(/first_legal_tictactoe vs perfect_tictactoe/)).toBeInTheDocument();
    expect(within(rows[0]).getByText(/3 plies/)).toBeInTheDocument();
  });

  it("opens a stored match by identifier", () => {
    const onOpenMatch = vi.fn();
    renderPanel({
      matchSummaries: [summary()],
      onOpenMatch,
    });
    fireEvent.click(screen.getByRole("button", { name: /OPEN/ }));
    expect(onOpenMatch).toHaveBeenCalledWith("arena-ttt-7-1700000000000-1");
  });

  it("marks the row of the match on screen", () => {
    renderPanel({ match: match(), matchSummaries: [summary()] });
    expect(screen.getByRole("button", { name: /OPEN/ })).toHaveAttribute("aria-current", "true");
  });

  it("says the archive is never deleted from this page", () => {
    renderPanel({ matchSummaries: [] });
    expect(screen.getByText(/never deleted by the controls on this page/i)).toBeInTheDocument();
  });
});

describe("result wording", () => {
  it("describes each outcome for the tested policy", () => {
    expect(testedResultLabel(match({ outcome: "win" }))).toBe("Under test WON");
    expect(testedResultLabel(match({ outcome: "loss", winner: "second" }))).toBe("Under test LOST");
    expect(testedResultLabel(match({ outcome: "draw", winner: null }))).toBe("Under test DREW");
    expect(testedResultLabel(match({ outcome: "no_result" }))).toBe("Under test: no result");
  });
});