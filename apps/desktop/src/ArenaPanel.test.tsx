/**
 * The Arena panel: what it claims, what it refuses to claim, and what it can be operated by.
 *
 * The assertions here are deliberately about *honesty* as much as about rendering. A game panel's
 * credibility rests entirely on never inventing something: no score the engine did not produce, no
 * "your turn" on a match that is already over, no progress bar for work that is not happening, and no
 * statistic that reads like a capability claim when it is a count of recorded events. Those are the
 * properties worth pinning, and each one below names the defect it would catch.
 */

import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ArenaPanel, outcomeGloss, outcomeLabel, seatLabel, staleFor, testedResultLabel } from "./ArenaPanel";
import { boardCell } from "./boards";
import type {
  GameBoardView,
  ArenaAgentSummary,
  ArenaGameSummary,
  ArenaMatchSummary,
  ArenaMatchView,
  ArenaSeatRequest,
} from "./api";

/* ------------------------------------------------------------------ fixtures */

/**
 * A finished tic-tac-toe match the engine could genuinely have produced.
 *
 * First completes the top row on ply five. Every field is internally consistent: the board really
 * contains the line `winningLine` names, and the history is the sequence of moves that produced it.
 * An inconsistent fixture would be caught by `winningCoordinates`, but a test that relied on a
 * fixture being rejected would be testing the fixture rather than the panel.
 */
function wonMatch(overrides: Partial<ArenaMatchView> = {}): ArenaMatchView {
  const empty = (): GameBoardView => ({
    game: "tic_tac_toe",
    state: { cells: Array<string>(9).fill("empty") },
  });
  const withCells = (cells: string[]): GameBoardView => ({
    game: "tic_tac_toe",
    state: { cells },
  });
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
    plies: 5,
    finalBoard: withCells(["first", "first", "first", "empty", "second", "empty", "empty", "empty", "empty"]),
    winner: "first",
    winningLine: [
      [0, 0],
      [1, 0],
      [2, 0],
    ],
    illegalMoves: [],
    history: [
      { ply: 1, side: "first", board: withCells(["first", "empty", "empty", "empty", "empty", "empty", "empty", "empty", "empty"]), played: { game: "tic_tac_toe", move: 0 } },
      { ply: 2, side: "second", board: withCells(["first", "empty", "empty", "empty", "second", "empty", "empty", "empty", "empty"]), played: { game: "tic_tac_toe", move: 4 } },
      { ply: 3, side: "first", board: withCells(["first", "first", "empty", "empty", "second", "empty", "empty", "empty", "empty"]), played: { game: "tic_tac_toe", move: 1 } },
      { ply: 4, side: "second", board: withCells(["first", "first", "empty", "empty", "second", "empty", "empty", "empty", "empty"]), played: { game: "tic_tac_toe", move: 7 } },
      { ply: 5, side: "first", board: withCells(["first", "first", "first", "empty", "second", "empty", "empty", "empty", "empty"]), played: { game: "tic_tac_toe", move: 2 } },
    ],
    turns: [],
    timeline: [],
    failure: null,
    cancelled: false,
    ...overrides,
  };
}

function games(): ArenaGameSummary[] {
  return [
    {
      id: "tic_tac_toe",
      name: "Tic-Tac-Toe",
      description: "Three by three.",
      width: 3,
      height: 3,
      maxPlies: 9,
    },
    {
      id: "connect_four",
      name: "Connect Four",
      description: "Seven by six, played under gravity.",
      width: 7,
      height: 6,
      maxPlies: 42,
    },
  ];
}

function agents(): ArenaAgentSummary[] {
  return [
    {
      id: "perfect_tictactoe",
      name: "Perfect Tic-Tac-Toe",
      description: "Exhaustive minimax.",
      categoryLabel: "REFERENCE / CEILING",
      canBeOpponent: true,
    },
    {
      id: "first_legal_tictactoe",
      name: "First Legal Tic-Tac-Toe",
      description: "Lowest legal cell.",
      categoryLabel: "BASELINE",
      canBeOpponent: true,
    },
  ];
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

function renderPanel(overrides: Partial<React.ComponentProps<typeof ArenaPanel>> = {}) {
  const props: React.ComponentProps<typeof ArenaPanel> = {
    games: games(),
    agents: agents(),
    match: wonMatch(),
    summaries: [summary()],
    game: "tic_tac_toe",
    testedSeat: { kind: "policy", id: "first_legal_tictactoe" },
    opponentSeat: { kind: "policy", id: "perfect_tictactoe" },
    testedSide: "first",
    seed: 7,
    localModels: ["qwen2.5:0.5b", "qwen2.5:3b"],
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
    /*
     * Supplied explicitly, as `canPlay` itself always was. This is the gap audit finding B1 identified:
     * the panel was tested with an eligibility it was never actually given, so the parent's faulty
     * condition was invisible to it. The rule is now tested directly in `arenaEligibility.test.ts`;
     * what matters here is that the panel renders the reasons it is handed.
     */
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

/* ------------------------------------------------------------------ labels */

describe("outcome wording", () => {
  it("names a harness failure as no result rather than a loss", () => {
    // The distinction the whole design rests on: a policy did not lose a match the harness never
    // finished playing.
    const failed = wonMatch({ outcome: "no_result", failure: "the provider refused the connection" });
    expect(outcomeLabel(failed.outcome)).toBe("NO RESULT");
    expect(testedResultLabel(failed)).toBe("NO RESULT");
    expect(outcomeGloss(failed)).toMatch(/did not reach a result/i);
    // The gloss contains the word "lost" only inside "nothing was won or lost", which is a
    // denial. What must never appear is a claim that the tested policy lost.
    expect(outcomeGloss(failed)).not.toMatch(/\bfirst_legal_tictactoe\b.*\blost\b/i);
    expect(outcomeGloss(failed)).toMatch(/nothing was won or lost/i);
  });

  it("names a cancelled match as cancelled, not as a loss or a failure", () => {
    const stopped = wonMatch({ outcome: "no_result", cancelled: true, plies: 4 });
    expect(testedResultLabel(stopped)).toBe("CANCELLED");
    expect(outcomeGloss(stopped)).toMatch(/stopped after 4 turns/i);
  });

  it("says who won and from which seat", () => {
    expect(outcomeGloss(wonMatch())).toMatch(/first_legal_tictactoe.*first seat.*completed a line/i);
  });

  it("reports a draw as a draw, without calling it a result the model earned", () => {
    const drawn = wonMatch({ outcome: "draw", winner: null, winningLine: null });
    expect(outcomeGloss(drawn)).toMatch(/neither seat completed a line/i);
  });
});

/* ------------------------------------------------------------------ the board */

describe("the board", () => {
  it("draws one cell per position and names each of them", () => {
    const { container } = renderPanel();
    expect(container.querySelectorAll(".arena-cell")).toHaveLength(9);
    // A board a screen reader can read is a board, not a picture of one.
    const cell = container.querySelector(".arena-cell") as HTMLElement;
    expect(cell).toHaveAttribute("aria-label", expect.stringContaining("Cell 1, column 1"));
    expect(cell.getAttribute("aria-label")).toMatch(/empty|first|second/i);
  });

  it("highlights the engine's winning line only on the final frame", () => {
    // The panel must never mark a line mid-replay: those cells are not the winner's yet. Autoplay
    // opens on the first frame, so the opening render is exactly the mid-replay case.
    const { container } = renderPanel();
    expect(container.querySelectorAll(".arena-cell--winning")).toHaveLength(0);
    fireEvent.click(screen.getByRole("button", { name: /go to the final position/i }));
    expect(container.querySelectorAll(".arena-cell--winning")).toHaveLength(3);
  });

  it("shows no winning line for a draw", () => {
    const { container } = renderPanel({
      match: wonMatch({
        outcome: "draw",
        winner: null,
        winningLine: null,
        finalBoard: { game: "tic_tac_toe", state: { cells: Array<string>(9).fill("empty") } },
      }),
    });
    expect(container.querySelectorAll(".arena-cell--winning")).toHaveLength(0);
  });

  it("shows no winning line for a cancelled match", () => {
    // The board is where it actually got to, and nobody won it. A highlight here would be a claim.
    const { container } = renderPanel({
      match: wonMatch({ outcome: "no_result", cancelled: true, winner: null }),
    });
    expect(container.querySelectorAll(".arena-cell--winning")).toHaveLength(0);
  });

  it("renders a connect-four board at its real size, indexed column-major", () => {
    const cells = new Array(42).fill("empty");
    // Column 3's bottom cell.
    cells[3 * 6 + 0] = "first";
    const { container } = renderPanel({
      game: "connect_four",
      match: wonMatch({
        game: "connect_four",
        gameName: "Connect Four",
        testedSeat: "model",
        turns: [],
        finalBoard: { game: "connect_four", state: { columns: 7, rows: 6, cells } },
        history: [],
        winner: null,
        winningLine: null,
      }),
      testedSeat: { kind: "model", label: "qwen2.5:0.5b", config: MODEL_CONFIG },
      opponentSeat: { kind: "policy", id: "depth_bounded_connect_four" },
      agents: agents(),
    });
    expect(container.querySelectorAll(".arena-cell")).toHaveLength(42);
    // Read through the same accessor the renderer uses, so this test and the panel cannot disagree
    // about where a disc is.
    expect(
      boardCell({ game: "connect_four", state: { columns: 7, rows: 6, cells } }, 3, 0),
    ).toBe("first");
  });

  it("shows an explicit idle state rather than an empty board when nothing has been played", () => {
    const { container } = renderPanel({ match: null });
    expect(screen.getByText(/NO ACTIVE MATCH/i)).toBeInTheDocument();
    // An empty board plus a heading would read as a match that produced nothing.
    expect(container.querySelectorAll(".arena-cell")).toHaveLength(0);
  });
});

/* ------------------------------------------------------------------ honesty */

describe("what the panel refuses to invent", () => {
  it("never offers a score, a rating, or a percentage", () => {
    const { container } = renderPanel();
    const text = container.textContent ?? "";
    // A number that reads as a capability claim on a two-policy match would be a fabrication: the
    // engine produces an outcome, not a rating, and nothing on this page may imply otherwise.
    expect(text).not.toMatch(/\d+\s*%/);
    expect(text).not.toMatch(/\belo\b|\brating\b|\bgrade\b|\bscore\b/i);
    expect(text).not.toMatch(/\bthinking\b|\bconsidering\b/i);
  });

  it("never invites a human to take a turn", () => {
    const { container } = renderPanel();
    expect(container.textContent).not.toMatch(/your turn/i);
    // Every cell is a plain gridcell, not a button. A clickable board would imply an input the panel
    // cannot honour.
    expect(container.querySelectorAll(".arena-cell button")).toHaveLength(0);
  });

  it("keeps the connect-four board reachable, and hides only the column tray", () => {
    // The column tray is decoration. The board is not: each of its 42 cells carries a spoken name,
    // and for connect four that name is the only account of the position there is.
    //
    // This failed before. `aria-hidden` sat on the wrapper that also contains the board, so the whole
    // grid dropped out of the accessibility tree. It passed unnoticed because the tic-tac-toe board,
    // which has no wrapper, was the one the tests above were reading — the bug was specific to the
    // game that has the frame around it.
    //
    // `getByRole` queries the accessibility tree, so these assertions are about what a screen reader
    // would find, not about what happens to be in the DOM.
    const cells = new Array(42).fill("empty");
    renderPanel({
      game: "connect_four",
      match: wonMatch({
        game: "connect_four",
        gameName: "Connect Four",
        testedSeat: "model",
        turns: [],
        finalBoard: { game: "connect_four", state: { columns: 7, rows: 6, cells } },
        history: [],
        winner: null,
        winningLine: null,
      }),
      testedSeat: { kind: "model", label: "qwen2.5:0.5b", config: MODEL_CONFIG },
      opponentSeat: { kind: "policy", id: "depth_bounded_connect_four" },
      agents: agents(),
    });

    // All 42 cells are in the accessibility tree. With the bug present this found none.
    expect(screen.getAllByRole("gridcell")).toHaveLength(42);
    // And they are individually addressable, which is the point of naming each one.
    expect(screen.getByLabelText(/row 1, column 1, empty/i)).toBeInTheDocument();
    expect(screen.getByLabelText(/row 6, column 7, empty/i)).toBeInTheDocument();

    // The tray is the only thing allowed to be invisible, and it is the only thing that is.
    const tray = document.querySelector(".arena-c4__tray");
    expect(tray).toHaveAttribute("aria-hidden", "true");
    expect(tray?.querySelectorAll(".arena-c4__slot")).toHaveLength(7);
    expect(document.querySelector(".arena-c4")).not.toHaveAttribute("aria-hidden");
  });

  it("says nothing was reported rather than reporting zero when the provider gave no tokens", () => {
    const withTurn = {
      ply: 1,
      side: "first" as const,
      seat: "model" as const,
      seatLabel: "qwen2.5:0.5b",
      elapsedMs: 1420,
      usage: null,
      promptTemplateVersion: "connect_four_v1",
      responseText: null,
      failure: null,
      requestBoard: {
        game: "connect_four" as const,
        state: { columns: 7, rows: 6, cells: new Array(42).fill("empty") },
      },
    };
    const { container } = renderPanel({ match: wonMatch({ turns: [withTurn] }) });
    fireEvent.click(screen.getByRole("button", { name: /go to the final position/i }));
    expect(container.textContent).toMatch(/tokens\s*not\s*reported/i);
    expect(container.textContent).not.toMatch(/0\s*tokens/i);
  });

  it("surfaces a recorded failure without claiming the command failed", () => {
    // The match ran and produced no result. That is a status, not an alert: an alert would tell the
    // operator the command broke, which is a different problem with a different fix.
    const failed = wonMatch({
      outcome: "no_result",
      failure: "the provider refused the connection",
      turns: [],
    });
    renderPanel({ match: failed });
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.getByText(/did not reach a result/i)).toBeInTheDocument();
    expect(screen.getAllByText("NO RESULT").length).toBeGreaterThan(0);
  });

  it("shows the recorded failure reason somewhere the operator can read it", () => {
    const failed = wonMatch({
      outcome: "no_result",
      failure: "the provider refused the connection; nothing is listening on this address and port",
      turns: [],
    });
    const { container } = renderPanel({ match: failed });
    expect(container.textContent).toMatch(/refused the connection/i);
  });
});

/* ------------------------------------------------------------------ states */

describe("the panel's states", () => {
  it("shows a command failure as an alert rather than a blank board", () => {
    renderPanel({ error: "The match could not be played: the provider refused the connection" });
    const alert = screen.getByRole("alert");
    expect(alert).toHaveTextContent(/refused the connection/i);
  });

  it("offers a stop button only while a match is running", () => {
    const { unmount } = renderPanel({ playing: true });
    expect(screen.getByRole("button", { name: /^stop$/i })).toBeInTheDocument();
    unmount();
    renderPanel({ playing: false });
    expect(screen.queryByRole("button", { name: /^stop$/i })).not.toBeInTheDocument();
  });

  it("disables the setup controls while a match is running", () => {
    renderPanel({ playing: true });
    const setup = screen.getByRole("region", { name: /match setup/i });
    // The setup controls only. The replay transport stays live on purpose: watching a match you just
    // started is the point, and a panel that locked its own scrubber would make that impossible.
    for (const control of within(setup).getAllByRole("combobox")) {
      expect(control).toBeDisabled();
    }
    for (const control of within(setup).getAllByRole("spinbutton")) {
      expect(control).toBeDisabled();
    }
    expect(within(setup).getByRole("button", { name: /playing/i })).toBeDisabled();
  });

  it("explains why a real-model match takes time, instead of showing a bare spinner", () => {
    renderPanel({ playing: true });
    expect(screen.getByText(/one provider call per turn/i)).toBeInTheDocument();
  });
});

/* ------------------------------------------------------------------ replay */

describe("replay", () => {
  it("names every transport control for a screen reader", () => {
    renderPanel();
    for (const name of [
      /go to the opening position/i,
      /step back one turn/i,
      /step forward one turn/i,
      /go to the final position/i,
    ]) {
      expect(screen.getByRole("button", { name })).toBeInTheDocument();
    }
    // Autoplay starts the transport moving, so the play control is the pause control. Both names must
    // exist: a control that cannot be paused is not a transport.
    expect(screen.getByRole("button", { name: /pause playback/i })).toBeInTheDocument();
  });

  it("uses drawn icons for the transport rather than Unicode transport glyphs", () => {
    // The old transport used `⏮ ◀ ▶ ⏭ ❚❚`. Those render differently in every font, read as
    // decoration to a screen reader, and look like a media player on a scientific instrument.
    const { container } = renderPanel();
    expect(container.textContent).not.toMatch(/[⏮◀▶⏭❚]/);
    expect(container.querySelectorAll(".arena-transport svg").length).toBeGreaterThan(0);
  });

  it("offers a speed control and a scrub position with a readable value", () => {
    renderPanel();
    expect(screen.getByLabelText(/playback speed/i)).toBeInTheDocument();
    const slider = screen.getByRole("slider");
    expect(slider).toHaveAttribute("aria-valuetext", expect.stringContaining("Frame 1"));
  });

  it("says so when there is nothing to step through", () => {
    renderPanel({ match: null });
    expect(screen.getByText(/nothing to step through yet/i)).toBeInTheDocument();
  });
});

/* ------------------------------------------------------------------ staleness */

describe("staleness", () => {
  it("is not stale when the selectors still describe the recording", () => {
    expect(
      staleFor(
        wonMatch(),
        { kind: "policy", id: "first_legal_tictactoe" },
        { kind: "policy", id: "perfect_tictactoe" },
        "tic_tac_toe",
        "first",
      ),
    ).toBe(false);
  });

  it("is stale when the seat, the game, or the side changed", () => {
    const match = wonMatch();
    expect(
      staleFor(match, { kind: "policy", id: "perfect_tictactoe" }, { kind: "policy", id: "perfect_tictactoe" }, "tic_tac_toe", "first"),
    ).toBe(true);
    expect(
      staleFor(match, { kind: "policy", id: "first_legal_tictactoe" }, { kind: "policy", id: "perfect_tictactoe" }, "connect_four", "first"),
    ).toBe(true);
    expect(
      staleFor(match, { kind: "policy", id: "first_legal_tictactoe" }, { kind: "policy", id: "perfect_tictactoe" }, "tic_tac_toe", "second"),
    ).toBe(true);
  });

  it("is never stale without a recording", () => {
    expect(staleFor(null, { kind: "policy", id: "x" }, { kind: "policy", id: "y" }, "tic_tac_toe", "first")).toBe(false);
  });

  it("names a seat the same way whether or not it has been played", () => {
    expect(seatLabel({ kind: "policy", id: "perfect_tictactoe" })).toBe("perfect_tictactoe");
    expect(seatLabel({ kind: "model", label: "qwen2.5:3b", config: MODEL_CONFIG })).toBe("qwen2.5:3b");
  });
});

const MODEL_CONFIG = {
  kind: "openai_compatible",
  schema_version: 1,
  base_url: "http://127.0.0.1:11434/v1",
  api_key_env: "OPENAI_COMPATIBLE_API_KEY",
  model: "qwen2.5:3b",
  timeout_seconds: 120,
  temperature: 0,
  max_tokens: 200,
} as const;

/* ------------------------------------------------------------------ archive */

describe("the archive", () => {
  it("lists recorded matches with their result and their seats", () => {
    renderPanel();
    const archive = screen.getByRole("region", { name: /match archive/i });
    expect(within(archive).getByText(/WIN/)).toBeInTheDocument();
    expect(within(archive).getByText(/first_legal_tictactoe vs perfect_tictactoe/i)).toBeInTheDocument();
  });

  it("opens a recording through the control the user presses", () => {
    const onOpen = vi.fn();
    renderPanel({ onOpen });
    const archive = screen.getByRole("region", { name: /match archive/i });
    const row = within(archive).getByRole("button");
    row.click();
    expect(onOpen).toHaveBeenCalledWith("arena-tic_tac_toe-7-1700000000000-1");
  });

  it("says so plainly when nothing has been recorded", () => {
    renderPanel({ summaries: [] });
    expect(screen.getByText(/no matches recorded yet/i)).toBeInTheDocument();
  });
});

/* ------------------------------------------------------------------ seats */

describe("the seat pickers", () => {
  it("offers shipped policies and the locally installed models", () => {
    renderPanel();
    const tested = screen.getByLabelText(/under test/i);
    const labels = within(tested)
      .getAllByRole("option")
      .map((option) => option.textContent ?? "");
    expect(labels.some((label) => /perfect tic-tac-toe/i.test(label))).toBe(true);
    // A model in the list is one discovery actually reported.
    expect(labels.some((label) => /qwen2\.5:0\.5b/.test(label))).toBe(true);
  });

  it("says when no local model was detected rather than offering an empty option", () => {
    renderPanel({ localModels: [] });
    const tested = screen.getByLabelText(/under test/i);
    expect(within(tested).getByRole("option", { name: /none detected/i })).toBeInTheDocument();
  });
});
