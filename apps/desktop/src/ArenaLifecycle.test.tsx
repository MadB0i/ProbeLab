/**
 * The Arena panel's lifecycle: what survives a configuration change, and how a recording stays
 * identifiable and reopenable.
 *
 * These are the properties that broke before. A selector change used to clear the displayed match,
 * which destroyed the recording's identifier and orphaned its replay even though the backend still
 * held both — and the identifier is the only handle on a stored match. The tests here pin the two
 * halves of that fix: the recording survives, and it stays addressable.
 */

import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ArenaPanel, shortId } from "./ArenaPanel";
import type {
  ArenaAgentSummary,
  ArenaGameSummary,
  ArenaMatchSummary,
  ArenaMatchView,
  GameBoardView,
} from "./api";

function board(cells: string[]): GameBoardView {
  return { game: "tic_tac_toe", state: { cells } };
}

const FINISHED: string[] = ["first", "first", "first", "empty", "second", "empty", "empty", "empty", "empty"];

function match(overrides: Partial<ArenaMatchView> = {}): ArenaMatchView {
  return {
    matchId: "arena-tic_tac_toe-7-1700000000000-1",
    game: "tic_tac_toe",
    gameName: "Tic-Tac-Toe",
    outcome: "win",
    testedAgent: "first_legal_tictactoe",
    opponentAgent: "perfect_tictactoe",
    testedSeat: "reference_policy",
    opponentSeat: "reference_policy",
    testedSide: "first",
    seed: 7,
    plies: 3,
    finalBoard: board(FINISHED),
    winner: "first",
    winningLine: [
      [0, 0],
      [1, 0],
      [2, 0],
    ],
    illegalMoves: [],
    history: [
      { ply: 1, side: "first", board: board(["first", "empty", "empty", "empty", "empty", "empty", "empty", "empty", "empty"]), played: { game: "tic_tac_toe", move: 0 } },
      { ply: 2, side: "second", board: board(["first", "empty", "empty", "empty", "second", "empty", "empty", "empty", "empty"]), played: { game: "tic_tac_toe", move: 4 } },
      { ply: 3, side: "first", board: board(FINISHED), played: { game: "tic_tac_toe", move: 1 } },
    ],
    turns: [],
    timeline: [],
    failure: null,
    cancelled: false,
    ...overrides,
  };
}

function summary(overrides: Partial<ArenaMatchSummary> = {}): ArenaMatchSummary {
  return {
    matchId: "arena-tic_tac_toe-7-1700000000000-1",
    game: "tic_tac_toe",
    outcome: "win",
    testedAgent: "first_legal_tictactoe",
    opponentAgent: "perfect_tictactoe",
    testedSide: "first",
    seed: 7,
    plies: 3,
    recordedUnixMs: 1_700_000_000_000,
    digest: "abcdef0123456789",
    ...overrides,
  };
}

const GAMES: ArenaGameSummary[] = [
  { id: "tic_tac_toe", name: "Tic-Tac-Toe", description: "Three by three.", width: 3, height: 3, maxPlies: 9 },
  { id: "connect_four", name: "Connect Four", description: "Seven by six.", width: 7, height: 6, maxPlies: 42 },
];

const AGENTS: ArenaAgentSummary[] = [
  { id: "perfect_tictactoe", name: "Perfect Tic-Tac-Toe", description: "Minimax.", categoryLabel: "REFERENCE / CEILING", canBeOpponent: true },
  { id: "first_legal_tictactoe", name: "First Legal Tic-Tac-Toe", description: "Lowest legal cell.", categoryLabel: "BASELINE", canBeOpponent: true },
];

function renderPanel(overrides: Partial<React.ComponentProps<typeof ArenaPanel>> = {}) {
  const props: React.ComponentProps<typeof ArenaPanel> = {
    games: GAMES,
    agents: AGENTS,
    match: match(),
    summaries: [summary()],
    game: "tic_tac_toe",
    testedSeat: { kind: "policy", id: "first_legal_tictactoe" },
    opponentSeat: { kind: "policy", id: "perfect_tictactoe" },
    testedSide: "first",
    seed: 7,
    localModels: [],
    modelConfig: {
      baseUrl: "http://127.0.0.1:11434/v1",
      apiKeyEnv: "OPENAI_COMPATIBLE_API_KEY",
      timeoutSeconds: 120,
    },
    onGameChange: () => {},
    onTestedSeatChange: () => {},
    onOpponentSeatChange: () => {},
    onSideChange: () => {},
    onSeedChange: () => {},
    onPlay: () => {},
    onCancel: () => {},
    onOpen: () => {},
    playing: false,
    openingId: null,
    error: null,
    canPlay: true,
    eligibilityReasons: [],
    batch: {
      statistics: null,
      running: false,
      progress: null,
      requestedMatches: 10,
      alternateSides: true,
      canRun: true,
      onRunBatch: () => {},
      onCancelBatch: () => {},
      onAlternateSidesChange: () => {},
      onMatchesChange: () => {},
    },
    ...overrides,
  };
  return render(<ArenaPanel {...props} />);
}

/* ------------------------------------------------------------------ survival */

describe("a completed match survives a configuration change", () => {
  it("keeps the board and the result when the selectors no longer describe it", () => {
    // The regression this file exists for: the recording and its identifier used to be destroyed by a
    // seat change, which made the replay unreachable even though the backend still held it.
    const { container } = renderPanel({ testedSide: "second" });
    expect(container.querySelectorAll(".arena-cell")).toHaveLength(9);
    expect(screen.getAllByText("WIN").length).toBeGreaterThan(0);
    // And the mismatch is stated rather than hidden.
    expect(screen.getByText(/selectors have changed/i)).toBeInTheDocument();
  });

  it("does not claim staleness when the selectors still describe it", () => {
    renderPanel();
    expect(screen.queryByText(/selectors have changed/i)).not.toBeInTheDocument();
  });

  it("still offers to run again over a stale recording", () => {
    renderPanel({ testedSide: "second" });
    expect(screen.getByRole("button", { name: /run again/i })).toBeEnabled();
  });

  it("names the first match differently from a re-run, so the two are not confused", () => {
    const { unmount } = renderPanel({ match: null });
    expect(screen.getByRole("button", { name: /play match/i })).toBeInTheDocument();
    unmount();
    renderPanel();
    expect(screen.getByRole("button", { name: /run again/i })).toBeInTheDocument();
  });
});

/* ------------------------------------------------------------------ identifiers */

describe("match identifiers", () => {
  it("abbreviates a long identifier for display", () => {
    const long = "arena-tic_tac_toe-7-1700000000000-1";
    const short = shortId(long);
    expect(short.length).toBeLessThan(long.length);
    expect(short).toContain("…");
  });

  it("keeps the tail, because that is what separates two matches of one seed", () => {
    // Two replays of the same seed differ only in the trailing timestamp and counter. Dropping the
    // tail would render them as the same recording.
    const short = shortId("arena-tic_tac_toe-7-1700000000000-1");
    expect(short.endsWith("0000-1")).toBe(true);
  });

  it("leaves a short identifier alone", () => {
    expect(shortId("m-1")).toBe("m-1");
  });

  it("keeps the full identifier in the accessible name even though it is abbreviated on screen", () => {
    const long = "arena-tic_tac_toe-7-1700000000000-1";
    renderPanel();
    const button = screen.getByRole("button", { name: new RegExp(long) });
    expect(button).toHaveAccessibleName(new RegExp(long));
    // The painted text is the abbreviation, not the whole thing, so the header does not wrap.
    expect(button.textContent).toContain("…");
  });
});

/* ------------------------------------------------------------------ the archive */

describe("the match archive", () => {
  it("says plainly that nothing is recorded yet", () => {
    renderPanel({ summaries: [] });
    expect(screen.getByText(/no matches recorded yet/i)).toBeInTheDocument();
  });

  it("lists a stored match with the facts a reader needs to identify it", () => {
    renderPanel();
    const archive = screen.getByRole("region", { name: /match archive/i });
    const row = within(archive).getByRole("button");
    expect(row).toHaveTextContent(/tic_tac_toe/);
    expect(row).toHaveTextContent(/3 plies/);
    expect(row).toHaveTextContent(/seed 7/);
    expect(row).toHaveTextContent(/first_legal_tictactoe vs perfect_tictactoe/);
  });

  it("opens a stored match by its identifier", () => {
    const onOpen = vi.fn();
    renderPanel({ onOpen });
    const archive = screen.getByRole("region", { name: /match archive/i });
    fireEvent.click(within(archive).getByRole("button"));
    expect(onOpen).toHaveBeenCalledWith("arena-tic_tac_toe-7-1700000000000-1");
  });

  it("marks the row of the match currently on screen", () => {
    renderPanel({ openingId: "arena-tic_tac_toe-7-1700000000000-1" });
    const archive = screen.getByRole("region", { name: /match archive/i });
    expect(within(archive).getByRole("button")).toHaveAttribute("aria-current", "true");
  });

  it("locks the archive while another match is being opened, so two opens cannot race", () => {
    renderPanel({ openingId: "arena-tic_tac_toe-7-1700000000000-1" });
    const archive = screen.getByRole("region", { name: /match archive/i });
    expect(within(archive).getByRole("button")).toBeDisabled();
  });
});

/* ------------------------------------------------------------------ replay */

describe("playback", () => {
  it("offers a transport, a scrub position, a speed, and a frame counter", () => {
    renderPanel();
    expect(screen.getByRole("slider")).toBeInTheDocument();
    expect(screen.getByLabelText(/playback speed/i)).toBeInTheDocument();
    expect(screen.getByText(/FRAME \d+ \/ \d+/)).toBeInTheDocument();
  });

  it("announces the frame position politely rather than interrupting", () => {
    renderPanel();
    expect(screen.getByText(/FRAME \d+ \/ \d+/)).toHaveAttribute("aria-live", "polite");
  });

  it("disables stepping back at the start and stepping forward at the end", () => {
    renderPanel();
    expect(screen.getByRole("button", { name: /step back one turn/i })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: /go to the final position/i }));
    expect(screen.getByRole("button", { name: /step forward one turn/i })).toBeDisabled();
  });

  it("says so rather than offering a transport for a single recorded position", () => {
    renderPanel({
      match: match({ history: [], finalBoard: board(FINISHED), winningLine: null, winner: null, outcome: "no_result" }),
    });
    expect(screen.getByText(/recorded one position, so there is nothing to step through/i)).toBeInTheDocument();
    // And no transport is offered at all, rather than one with every control disabled.
    expect(screen.queryByRole("slider")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /step forward one turn/i })).not.toBeInTheDocument();
  });

  it("does not offer a control that would re-run the match while playing back a recording", () => {
    // "Open a recording" must never mean "pay for the same decisions again". The only transport
    // controls are ones that read stored data.
    const { container } = renderPanel();
    const buttons = Array.from(container.querySelectorAll("button"));
    const labels = buttons.map((button) => (button.textContent ?? "").trim().toLowerCase());
    expect(labels.some((label) => /replay the match|re-run|replay match/.test(label))).toBe(false);
  });
});
