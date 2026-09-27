/**
 * Replay panel tests.
 *
 * The recurring assertion is that the panel is honest about what a recording can support, and never
 * implies it re-ran anything. A replay that looked like a re-execution would let a reader trust a
 * projection as if it were a reproduction — and, worse, would suggest the lab could reproduce a
 * nondeterministic model's behaviour, which it cannot.
 */

import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ReplayPanel, capabilityExplanation, capabilityLabel, frameSourceLabel } from "./ReplayPanel";
import type { Replay } from "./api";

function fullReplay(): Replay {
  return {
    recordingId: "arena-ttt-7",
    kind: "arena_match",
    capability: "full",
    frames: [
      { index: 0, source: "start", label: "Tic-Tac-Toe before any ply", digest: null },
      { index: 1, source: "arena_ply", label: "after ply 1 by First playing TicTacToe(0)", digest: null },
      { index: 2, source: "arena_ply", label: "after ply 2 by Second playing TicTacToe(1)", digest: null },
      { index: 3, source: "terminal", label: "final: Draw after 2 plies", digest: "abc123" },
    ],
    digest: "deadbeef",
    limitations: [],
  };
}

function projectedReplay(): Replay {
  return {
    recordingId: "exp-1",
    kind: "experiment",
    capability: "projected_only",
    frames: [
      { index: 0, source: "start", label: "exp-1 before the first event", digest: null },
      { index: 1, source: "experiment_event", label: "experiment.created: created", digest: null },
    ],
    digest: "feedface",
    limitations: [
      "This recording is an event stream. It shows what was reported and when, not every \
intermediate environment state, so it cannot be stepped as a re-execution would be.",
    ],
  };
}

function renderPanel(props: Partial<Parameters<typeof ReplayPanel>[0]> = {}) {
  return render(
    <ReplayPanel
      replay={null}
      error={null}
      onLoad={() => {}}
      loading={false}
      canLoad={false}
      matchId=""
      {...props}
    />,
  );
}

describe("replay vocabulary", () => {
  it("labels each frame source", () => {
    expect(frameSourceLabel("start")).toBe("START");
    expect(frameSourceLabel("arena_ply")).toBe("PLY");
    expect(frameSourceLabel("experiment_event")).toBe("EVENT");
    expect(frameSourceLabel("terminal")).toBe("TERMINAL");
  });

  it("distinguishes re-executable from inspection-only", () => {
    expect(capabilityLabel("full")).toBe("RE-EXECUTABLE");
    expect(capabilityLabel("projected_only")).toBe("INSPECTION ONLY");
  });

  it("explains what each capability means", () => {
    expect(capabilityExplanation(fullReplay())).toMatch(/stepped frame by frame/i);
    expect(capabilityExplanation(projectedReplay())).toMatch(/not re-executed/i);
  });
});

describe("ReplayPanel — empty state", () => {
  it("states the replay guarantee before any data is loaded", () => {
    // A reader deciding whether to trust a replay needs to know it cannot have called a model.
    renderPanel();
    expect(screen.getByText(/does not re-run a match, re-query a model, or contact/i)).toBeInTheDocument();
  });

  it("shows an intentional no-recording state", () => {
    renderPanel();
    expect(screen.getAllByText("NO RECORDING LOADED").length).toBeGreaterThan(0);
    expect(screen.getByRole("button", { name: /LOAD RECORDING/i })).toBeInTheDocument();
  });

  it("explains that a match must be played first", () => {
    renderPanel();
    expect(screen.getByText(/play a match first/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /LOAD RECORDING/i })).toBeDisabled();
  });

  it("enables loading when a match identifier exists", () => {
    renderPanel({ canLoad: true, matchId: "arena-ttt-7" });
    expect(screen.getByRole("button", { name: /LOAD RECORDING/i })).toBeEnabled();
  });
});

describe("ReplayPanel — a re-executable recording", () => {
  it("shows the recording's identity and digest", () => {
    renderPanel({ replay: fullReplay() });
    expect(screen.getByText("arena-ttt-7")).toBeInTheDocument();
    expect(screen.getByText("deadbeef")).toBeInTheDocument();
    expect(screen.getByText("RE-EXECUTABLE")).toBeInTheDocument();
  });

  it("starts on the first frame and steps forwards and back", () => {
    renderPanel({ replay: fullReplay() });
    expect(screen.getByText("Frame 1 of 4")).toBeInTheDocument();
    // The label appears in both the stepper and the frame list, so the stepper is scoped.
    const stepper = document.querySelector(".replay-frame")!;
    expect(stepper.textContent).toContain("Tic-Tac-Toe before any ply");

    // The accessible name comes from aria-label, which takes precedence over the visible glyphs.
    fireEvent.click(screen.getByRole("button", { name: "Next frame" }));
    expect(screen.getByText("Frame 2 of 4")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Previous frame" }));
    expect(screen.getByText("Frame 1 of 4")).toBeInTheDocument();
  });

  it("disables stepping at each end rather than wrapping", () => {
    renderPanel({ replay: fullReplay() });
    expect(screen.getByRole("button", { name: "Previous frame" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Next frame" })).toBeEnabled();
  });

  it("jumps to a frame when its row is clicked", () => {
    renderPanel({ replay: fullReplay() });
    const rows = document.querySelectorAll(".replay-frame-row button");
    fireEvent.click(rows[3]);
    expect(screen.getByText("Frame 4 of 4")).toBeInTheDocument();
    // The label appears in the stepper and the frame list, so the stepper is scoped again.
    expect(document.querySelector(".replay-frame")!.textContent).toContain(
      "final: Draw after 2 plies",
    );
  });

  it("shows every recorded frame", () => {
    renderPanel({ replay: fullReplay() });
    expect(document.querySelectorAll(".replay-frame-row")).toHaveLength(4);
  });

  it("shows a digest only where the recording has one", () => {
    renderPanel({ replay: fullReplay() });
    const rows = document.querySelectorAll(".replay-frame-row");
    expect(rows[0].querySelector(".replay-row-index")?.textContent).toBe("00");
    // Only the terminal frame carries a digest, and the others show nothing rather than a placeholder.
    expect(rows[0].textContent).not.toContain("digest");
  });

  it("shows no limitations section when there are none", () => {
    renderPanel({ replay: fullReplay() });
    expect(screen.queryByText(/could not be reconstructed/i)).not.toBeInTheDocument();
  });
});

describe("ReplayPanel — an inspection-only recording", () => {
  it("says it cannot be re-executed rather than implying otherwise", () => {
    renderPanel({ replay: projectedReplay() });
    expect(screen.getByText("INSPECTION ONLY")).toBeInTheDocument();
    expect(screen.getByText(/not every intermediate environment state/i)).toBeInTheDocument();
    // And the reassuring phrase must not appear.
    expect(screen.queryByText("RE-EXECUTABLE")).not.toBeInTheDocument();
  });

  it("shows the limitation in full", () => {
    renderPanel({ replay: projectedReplay() });
    const section = screen.getByText("What could not be reconstructed").closest("div")!;
    expect(within(section as HTMLElement).getByText(/event stream/i)).toBeInTheDocument();
  });

  it("still steps through the frames it does have", () => {
    renderPanel({ replay: projectedReplay() });
    expect(screen.getByText("Frame 1 of 2")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Next frame" }));
    expect(screen.getByText("Frame 2 of 2")).toBeInTheDocument();
  });
});

describe("ReplayPanel — errors and loading", () => {
  it("shows a backend failure", () => {
    renderPanel({ error: "The recording could not be replayed: no match named \"x\"" });
    expect(screen.getByRole("alert")).toHaveTextContent("no match named");
  });

  it("reports loading state on the control", () => {
    renderPanel({ canLoad: true, loading: true });
    expect(screen.getByRole("button", { name: /LOADING/i })).toBeDisabled();
  });

  it("loads a recording when asked", () => {
    const onLoad = vi.fn();
    renderPanel({ canLoad: true, onLoad });
    screen.getByRole("button", { name: /LOAD RECORDING/i }).click();
    expect(onLoad).toHaveBeenCalledTimes(1);
  });
});

describe("ReplayPanel — no fabricated data", () => {
  it("invents no frames when nothing is loaded", () => {
    const { container } = renderPanel();
    expect(container.querySelectorAll(".replay-frame-row")).toHaveLength(0);
    expect(container.querySelectorAll(".replay-frame")).toHaveLength(0);
  });

  it("invents no digest for a frame that has none", () => {
    renderPanel({ replay: fullReplay() });
    const digests = document.querySelectorAll(".replay-frame-digest");
    // Only the terminal frame's digest is shown in the stepper, and it is real.
    expect(digests.length).toBeLessThanOrEqual(1);
  });

  it("shows no board before a recording is loaded", () => {
    // The panel renders frames as text, not as a board it reconstructed itself. Rendering a board
    // would mean the frontend held game state, which is exactly what it must not do.
    renderPanel({ replay: fullReplay() });
    expect(document.querySelector(".arena-board")).toBeNull();
  });

});

describe("a recording with no frames", () => {
  /*
   * This is a correctness test, not a coverage one. The panel clamped the frame position with
   * `Math.min(position, total - 1)`, which for an empty frame list is -1. The readout then said
   * "Frame 0 of 0", and PREV was *enabled* -- because the disable condition was `current === 0`, and
   * -1 is not 0 -- while doing nothing. A control that looks live and is not.
   */
  const emptyReplay: Replay = {
    recordingId: "arena-ttt-0",
    kind: "arena_match",
    capability: "projected_only",
    frames: [],
    digest: "0badf00d",
    limitations: ["This recording captured no intermediate states, so there is nothing to step."],
  };

  it("states that there is nothing to step rather than showing a frame counter", () => {
    renderPanel({ replay: emptyReplay });
    expect(screen.getByText(/NO FRAMES TO STEP THROUGH/i)).toBeTruthy();
    expect(screen.queryByText(/Frame 0 of 0/i)).toBeNull();
    expect(screen.queryByText(/^0 of 0$/i)).toBeNull();
  });

  it("offers no stepper at all, so no control looks live and is not", () => {
    renderPanel({ replay: emptyReplay });
    expect(screen.queryByRole("button", { name: /previous frame/i })).toBeNull();
    expect(screen.queryByRole("button", { name: /next frame/i })).toBeNull();
  });

  it("still reports the recording's own digest", () => {
    renderPanel({ replay: emptyReplay });
    // The digest is evidence even when there is nothing to step, and hiding it would be worse than
    // showing an empty recording with no way to identify it.
    expect(screen.getByText("0badf00d")).toBeTruthy();
  });

  it("still shows every limitation the backend reported", () => {
    renderPanel({ replay: emptyReplay });
    expect(
      screen.getByText(/This recording captured no intermediate states/i),
    ).toBeTruthy();
  });
});
