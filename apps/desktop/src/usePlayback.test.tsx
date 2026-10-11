/**
 * Replay playback: the frame list, the transport, and its cleanup.
 *
 * The theme is that playback is a view over recorded data and cannot produce a frame the recording does
 * not contain. These tests cover the three ways that could go wrong: inventing a frame, leaving a
 * timer running, and losing the position when the recording changes.
 */

import { act, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { ArenaMatchView } from "./api";
import { playbackFrames, PLAYBACK_SPEEDS, usePlayback } from "./usePlayback";

function board(...marks: ("first" | "second" | "empty")[]) {
  return {
    game: "tic_tac_toe" as const,
    state: { cells: marks },
  };
}

function aMatch(overrides: Partial<ArenaMatchView> = {}): ArenaMatchView {
  return {
    matchId: "arena-ttt-7-1",
    game: "tic_tac_toe",
    gameName: "Tic-Tac-Toe",
    outcome: "win",
    testedAgent: "first_legal_tictactoe",
    opponentAgent: "perfect_tictactoe",
    testedSide: "first",
    seed: 7,
    plies: 3,
    finalBoard: board("first", "second", "first", "empty", "empty", "empty", "empty", "empty", "empty"),
    winner: "first",
    winningLine: [0, 1, 2],
    illegalMoves: [],
    history: [
      {
        ply: 1,
        side: "first",
        board: board("first", "empty", "empty", "empty", "empty", "empty", "empty", "empty", "empty"),
        played: { game: "tic_tac_toe", move: 0 },
      },
      {
        ply: 2,
        side: "second",
        board: board("first", "second", "empty", "empty", "empty", "empty", "empty", "empty", "empty"),
        played: { game: "tic_tac_toe", move: 1 },
      },
      {
        ply: 3,
        side: "first",
        board: board("first", "second", "first", "empty", "empty", "empty", "empty", "empty", "empty"),
        played: { game: "tic_tac_toe", move: 2 },
      },
    ],
    timeline: [],
    failure: null,
    ...overrides,
  };
}

describe("playbackFrames", () => {
  it("opens on the position before the first recorded move", () => {
    const frames = playbackFrames(aMatch());
    expect(frames).toHaveLength(4);
    // The opening frame is the empty board, derived by clearing the cell the first ply filled.
    expect(frames[0].cells.every((cell) => cell === "empty")).toBe(true);
    expect(frames[0].ply).toBeNull();
  });

  it("carries each recorded board, in the order the engine recorded them", () => {
    const frames = playbackFrames(aMatch());
    expect(frames[1].cells[0]).toBe("first");
    expect(frames[2].cells[1]).toBe("second");
    expect(frames[3].cells[2]).toBe("first");
    expect(frames[1].ply?.ply).toBe(1);
    expect(frames[3].ply?.ply).toBe(3);
  });

  it("ends on the position the match actually finished on", () => {
    // A recording whose ply list stops one move short of its final board.
    const match = aMatch({
      plies: 2,
      finalBoard: board("first", "second", "first", "empty", "empty", "empty", "empty", "empty", "empty"),
      history: [
        {
          ply: 1,
          side: "first",
          board: board("first", "empty", "empty", "empty", "empty", "empty", "empty", "empty", "empty"),
          played: { game: "tic_tac_toe", move: 0 },
        },
        {
          ply: 2,
          side: "second",
          board: board("first", "second", "empty", "empty", "empty", "empty", "empty", "empty", "empty"),
          played: { game: "tic_tac_toe", move: 1 },
        },
      ],
    });
    const frames = playbackFrames(match);
    const last = frames[frames.length - 1];
    expect(last.cells[2]).toBe("first");
  });

  it("does not duplicate a frame when the last ply is already the final board", () => {
    const frames = playbackFrames(aMatch());
    // opening + three plies, with the terminal board already present as the third ply.
    expect(frames).toHaveLength(4);
  });

  it("gives a match with no recorded plies exactly one position", () => {
    // Nothing was played, so the only board there is is the opening one.
    const frames = playbackFrames(
      aMatch({
        history: [],
        plies: 0,
        finalBoard: board("empty", "empty", "empty", "empty", "empty", "empty", "empty", "empty", "empty"),
      }),
    );
    expect(frames).toHaveLength(1);
    expect(frames[0].cells.every((cell) => cell === "empty")).toBe(true);
  });

  it("shows the recorded final board when a match recorded no plies but has one", () => {
    // A match that failed on its first ply still has a board, and that board is the whole recording.
    const frames = playbackFrames(aMatch({ history: [], plies: 0 }));
    expect(frames).toHaveLength(1);
    expect(frames[0].cells[0]).toBe("first");
  });

  it("skips a frame it cannot draw rather than rendering a blank board", () => {
    const match = aMatch({
      history: [
        {
          ply: 1,
          side: "first",
          board: { game: "tic_tac_toe", state: { cells: ["first"] } } as never,
          played: { game: "tic_tac_toe", move: 0 },
        },
      ],
    });
    const frames = playbackFrames(match);
    // One opening frame plus the terminal board. The malformed ply is not drawn as nine blanks.
    for (const frame of frames) {
      expect(frame.cells).toHaveLength(9);
    }
  });

  it("refuses to invent an opening frame when the first move was not identified", () => {
    const match = aMatch({
      history: [
        {
          ply: 1,
          side: "first",
          board: board("first", "empty", "empty", "empty", "empty", "empty", "empty", "empty", "empty"),
          played: null,
        },
      ],
    });
    const frames = playbackFrames(match);
    // Without a named move there is no sound way to reconstruct what the board looked like before it,
    // so the viewer starts at the first recorded position rather than guessing.
    expect(frames[0].ply?.ply).toBe(1);
  });

  it("has no frames when there is no recording at all", () => {
    expect(playbackFrames(null)).toHaveLength(0);
  });
});

/** A harness that exposes the hook's state and controls to the test. */
function Harness({
  match,
  autoPlay = false,
}: {
  match: ArenaMatchView | null;
  autoPlay?: boolean;
}) {
  const playback = usePlayback(match, { stale: false, autoPlay });
  return (
    <div>
      <span data-testid="position">{playback.position}</span>
      <span data-testid="total">{playback.frames.length}</span>
      <span data-testid="playing">{String(playback.playing)}</span>
      <button onClick={playback.toggle}>toggle</button>
      <button onClick={() => playback.step(1)}>next</button>
      <button onClick={playback.toStart}>start</button>
      <button onClick={playback.toEnd}>end</button>
      <button onClick={playback.reset}>reset</button>
    </div>
  );
}

describe("usePlayback — transport", () => {
  it("starts on the opening frame, not the end", () => {
    render(<Harness match={aMatch()} />);
    expect(screen.getByTestId("position").textContent).toBe("0");
    expect(screen.getByTestId("total").textContent).toBe("4");
  });

  it("steps forward and stops at the end", () => {
    render(<Harness match={aMatch()} />);
    const next = screen.getByText("next");
    act(() => {
      next.click();
    });
    expect(screen.getByTestId("position").textContent).toBe("1");
    act(() => {
      next.click();
      next.click();
      next.click();
    });
    expect(screen.getByTestId("position").textContent).toBe("3");
    act(() => {
      next.click();
    });
    expect(screen.getByTestId("position").textContent).toBe("3");
  });

  it("jumps to the first and last frames", () => {
    render(<Harness match={aMatch()} />);
    act(() => {
      screen.getByText("end").click();
    });
    expect(screen.getByTestId("position").textContent).toBe("3");
    act(() => {
      screen.getByText("start").click();
    });
    expect(screen.getByTestId("position").textContent).toBe("0");
  });

  it("advances on its own while playing", () => {
    vi.useFakeTimers();
    try {
      render(<Harness match={aMatch()} autoPlay />);
      expect(screen.getByTestId("playing").textContent).toBe("true");
      // One step per frame interval, so the assertion is about the stepper rather than about how a
      // batched timer flush happens to land.
      for (let frame = 1; frame <= 3; frame += 1) {
        act(() => {
          vi.advanceTimersByTime(600);
        });
        expect(screen.getByTestId("position").textContent).toBe(String(frame));
      }
      // Reaching the end stops rather than looping, so the final frame is not overwritten.
      expect(screen.getByTestId("playing").textContent).toBe("false");
      act(() => {
        vi.advanceTimersByTime(5_000);
      });
      expect(screen.getByTestId("position").textContent).toBe("3");
    } finally {
      vi.useRealTimers();
    }
  });

  it("stops advancing when paused", () => {
    vi.useFakeTimers();
    try {
      render(<Harness match={aMatch()} autoPlay />);
      act(() => {
        vi.advanceTimersByTime(600);
      });
      expect(screen.getByTestId("position").textContent).toBe("1");
      act(() => {
        screen.getByText("toggle").click();
      });
      expect(screen.getByTestId("playing").textContent).toBe("false");
      act(() => {
        vi.advanceTimersByTime(5_000);
      });
      expect(screen.getByTestId("position").textContent).toBe("1");
    } finally {
      vi.useRealTimers();
    }
  });

  it("seeking cancels playback rather than racing the timer", () => {
    vi.useFakeTimers();
    try {
      render(<Harness match={aMatch()} autoPlay />);
      act(() => {
        screen.getByText("end").click();
      });
      expect(screen.getByTestId("position").textContent).toBe("3");
      expect(screen.getByTestId("playing").textContent).toBe("false");
      act(() => {
        vi.advanceTimersByTime(5_000);
      });
      expect(screen.getByTestId("position").textContent).toBe("3");
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("usePlayback — cleanup", () => {
  it("returns to the opening frame when the recording changes", () => {
    const { rerender } = render(<Harness match={aMatch()} />);
    act(() => {
      screen.getByText("end").click();
    });
    expect(screen.getByTestId("position").textContent).toBe("3");

    rerender(<Harness match={aMatch({ matchId: "arena-ttt-7-2" })} />);
    expect(screen.getByTestId("position").textContent).toBe("0");
    expect(screen.getByTestId("playing").textContent).toBe("false");
  });

  it("clears its timer on unmount, so nothing advances after navigating away", () => {
    vi.useFakeTimers();
    const cleared: unknown[] = [];
    const originalClear = globalThis.clearTimeout;
    vi.stubGlobal("clearTimeout", (handle: unknown) => {
      cleared.push(handle);
      return originalClear(handle as never);
    });
    try {
      const { unmount } = render(<Harness match={aMatch()} autoPlay />);
      unmount();
      expect(cleared.length).toBeGreaterThan(0);
      // Advancing after unmount must not throw, which is what a live timer would do.
      expect(() => vi.advanceTimersByTime(10_000)).not.toThrow();
    } finally {
      vi.unstubAllGlobals();
      vi.useRealTimers();
    }
  });

  it("resets to the opening frame on request", () => {
    render(<Harness match={aMatch()} />);
    act(() => {
      screen.getByText("end").click();
    });
    act(() => {
      screen.getByText("reset").click();
    });
    expect(screen.getByTestId("position").textContent).toBe("0");
  });

  it("survives a recording becoming empty", () => {
    const { rerender } = render(<Harness match={aMatch()} />);
    act(() => {
      screen.getByText("end").click();
    });
    rerender(<Harness match={null} />);
    expect(screen.getByTestId("total").textContent).toBe("0");
    expect(screen.getByTestId("position").textContent).toBe("0");
  });
});

describe("playback speeds", () => {
  it("offers a range from slow to near-instant", () => {
    const delays = PLAYBACK_SPEEDS.map((speed) => speed.delayMs);
    expect(delays).toHaveLength(4);
    // Every speed is faster than the one before it.
    for (let index = 1; index < delays.length; index += 1) {
      expect(delays[index]).toBeLessThan(delays[index - 1]);
    }
    expect(Math.min(...delays)).toBeLessThan(120);
  });
});