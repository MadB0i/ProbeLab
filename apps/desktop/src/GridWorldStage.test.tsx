/**
 * The grid-world stage: what it draws, what it refuses to draw, and what it will not claim.
 *
 * Most of these assertions are about absence. A navigation panel that renders one tile the agent never
 * sensed, or one word about whether the run succeeded, has made a claim about the world or about the
 * evaluation that nothing in the recording supports — and both of those read as *helpful*.
 */

import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { GridWorldStage } from "./GridWorldStage";
import {
  completedEpisode,
  failedFinalTurnEpisode,
  GRID_SIZE,
  truncatedEpisode,
} from "./test-fixtures/gridRecording";

/** The board element, which carries the spoken description of the frame. */
function board(): HTMLElement {
  return screen.getByRole("img", { name: /tiles observed/ });
}

/** The agent marker, positioned by the recorded coordinate of the frame on screen. */
function marker(): HTMLElement {
  return document.querySelector(".gw-agent") as HTMLElement;
}

/**
 * The recorded coordinate the marker is placed at.
 *
 * Read from the board rather than the marker, because that is where the coordinate is written: the marker
 * inherits the custom property and derives its own position from it, so a test that read the marker would
 * pass on an empty string and prove nothing.
 */
function markerAt(): { x: number; y: number } {
  const style = (document.querySelector(".gw-board") as HTMLElement).style;
  return {
    x: Number(style.getPropertyValue("--gw-x")),
    y: Number(style.getPropertyValue("--gw-y")),
  };
}

/** Every cell drawn as never observed. */
function unseenCells(): HTMLElement[] {
  return Array.from(document.querySelectorAll('.gw-cell[data-tile="unseen"]'));
}

describe("a stage with nothing to draw", () => {
  it("says so, rather than drawing a board for a run that never started", () => {
    render(<GridWorldStage events={[]} mode="live" running={false} />);
    expect(screen.getByText(/Nothing recorded yet/)).toBeInTheDocument();
    expect(document.querySelector(".gw-board")).toBeNull();
  });

  it("names the source of the feed even when it is empty", () => {
    render(<GridWorldStage events={[]} mode="live" running={false} />);
    expect(screen.getByText("AWAITING A RUN")).toBeInTheDocument();
  });
});

describe("a live run", () => {
  it("draws every tile the recording observed and nothing else", () => {
    render(<GridWorldStage events={truncatedEpisode()} mode="live" running />);
    // 25 cells, 7 of which carry recorded terrain across the two turns and the move that followed them.
    expect(document.querySelectorAll(".gw-cell")).toHaveLength(GRID_SIZE * GRID_SIZE);
    expect(unseenCells()).toHaveLength(GRID_SIZE * GRID_SIZE - 7);
  });

  it("places the marker where the recording says the agent is", () => {
    render(<GridWorldStage events={completedEpisode()} mode="live" running />);
    // The live position is the newest frame: the landing position of the final recorded move.
    expect(markerAt()).toEqual({ x: 1, y: 2 });
  });

  it("states in its description how much of the world was never seen", () => {
    render(<GridWorldStage events={completedEpisode()} mode="live" running />);
    expect(board()).toHaveAccessibleName(expect.stringContaining("18 never seen"));
  });

  it("offers no scrub bar over a recording that is still being written", () => {
    render(<GridWorldStage events={completedEpisode()} mode="live" running />);
    expect(screen.queryByRole("slider")).toBeNull();
    expect(screen.getAllByText(/Streaming from the engine/).length).toBeGreaterThan(0);
  });

  it("labels the feed as live while the run is going", () => {
    render(<GridWorldStage events={completedEpisode()} mode="live" running />);
    expect(screen.getByText("LIVE RECORDING")).toBeInTheDocument();
  });

  it("says a finished run is the last one in this session rather than that nothing has happened", () => {
    // "AWAITING A RUN" over a board with seven turns on it reads as though the run never happened, which
    // is the opposite of what a returning user is looking at.
    render(<GridWorldStage events={completedEpisode()} mode="live" running={false} />);
    expect(screen.getByText("LAST RUN · SESSION")).toBeInTheDocument();
  });
});

describe("a recorded run", () => {
  it("labels the feed as a recording, so a replay is never read as progress", () => {
    render(
      <GridWorldStage
        events={completedEpisode()}
        mode="recorded"
        running={false}
        sourceLabel="1791932568098-0000000000000000"
      />,
    );
    // Shortened for the chip, which is what keeps the panel's heading on one line; the full identifier is
    // on the chip's tooltip and on the control deck's RUN ID line.
    expect(screen.getByText("RECORDED 179193256809…")).toBeInTheDocument();
  });

  it("steps forward and back, moving the marker to the recorded position each time", () => {
    render(<GridWorldStage events={completedEpisode()} mode="recorded" running={false} />);
    expect(markerAt()).toEqual({ x: 0, y: 0 });
    fireEvent.click(screen.getByRole("button", { name: "Next turn" }));
    expect(markerAt()).toEqual({ x: 0, y: 1 });
    fireEvent.click(screen.getByRole("button", { name: "Previous turn" }));
    expect(markerAt()).toEqual({ x: 0, y: 0 });
  });

  it("scrubs, and the marker follows the scrub rather than the scrub the marker", () => {
    render(<GridWorldStage events={completedEpisode()} mode="recorded" running={false} />);
    const scrub = screen.getByRole("slider", { name: /Turn within the recording/ });
    fireEvent.change(scrub, { target: { value: "3" } });
    expect(markerAt()).toEqual({ x: 1, y: 2 });
    expect(scrub).toHaveValue("3");
  });

  it("reveals tiles as the recording reaches the turn that sensed them", () => {
    const { rerender } = render(<GridWorldStage events={[]} mode="recorded" running={false} />);
    expect(screen.queryByText(/Nothing recorded yet/)).toBeInTheDocument();
    rerender(<GridWorldStage events={completedEpisode()} mode="recorded" running={false} />);
    // Turn one sensed the hazard at 1,0; nothing before it in this recording did.
    expect(document.querySelector('.gw-cell[data-tile="danger"]')).not.toBeNull();
    // And the goal tile is only painted once a recorded observation carried it.
    expect(document.querySelector('.gw-cell[data-tile="reward"]')).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "First turn" }));
    fireEvent.click(screen.getByRole("button", { name: "Next turn" }));
    fireEvent.click(screen.getByRole("button", { name: "Next turn" }));
    expect(document.querySelector('.gw-cell[data-tile="reward"]')).not.toBeNull();
  });

  it("says the distance was not recorded on a frame that carries no observation", () => {
    render(<GridWorldStage events={completedEpisode()} mode="recorded" running={false} />);
    fireEvent.click(screen.getByRole("button", { name: "Last turn" }));
    // The readout refuses to carry an earlier reading forward.
    const readout = document.querySelector(".gw-readout") as HTMLElement;
    expect(within(readout).getByText("not recorded")).toBeInTheDocument();
    expect(board()).toHaveAccessibleName(expect.stringContaining("distance to the goal was not recorded"));
  });

  it("lists the turns it holds, newest first, and marks the one on screen", () => {
    render(<GridWorldStage events={completedEpisode()} mode="recorded" running={false} />);
    const log = document.querySelector(".gw-log") as HTMLElement;
    const rows = within(log).getAllByRole("listitem");
    expect(rows).toHaveLength(4);
    // Newest first, so the reading order is the order they arrived in.
    expect(rows[0].textContent).toContain("03");
    // And the row on screen is marked as such, whichever end of the list it is on.
    expect(within(log).getByText("01").closest("li")).toHaveAttribute("aria-current", "step");
  });
});

describe("what the stage refuses to claim", () => {
  it("prints no verdict when the evaluator's stored result was not supplied", () => {
    // A panel that worked the outcome out from the last position would agree most of the time and
    // disagree in exactly the cases that matter, so there is no such derivation to call.
    render(<GridWorldStage events={completedEpisode()} mode="recorded" running={false} />);
    expect(document.querySelector(".gw-verdict")).toBeNull();
    expect(screen.queryByText("Reached the goal")).toBeNull();
  });

  it("prints the evaluator's verdict when one was supplied, and nothing more", () => {
    render(
      <GridWorldStage
        events={completedEpisode()}
        mode="recorded"
        running={false}
        verdict={{ status: "turn_limit", turnsUsed: 11, totalReward: -4 }}
      />,
    );
    const verdict = document.querySelector(".gw-verdict") as HTMLElement;
    expect(within(verdict).getByText("Ran out of budget")).toBeInTheDocument();
    expect(within(verdict).getByText(/11 turns/)).toBeInTheDocument();
    expect(verdict).toHaveClass("gw-verdict--fail");
  });

  it("reports a turn where the model failed as no decision, rather than a move", () => {
    render(<GridWorldStage events={failedFinalTurnEpisode()} mode="live" running={false} />);
    expect(screen.getByText("nothing")).toBeInTheDocument();
    expect(document.querySelector(".gw-readout")?.textContent).toContain("malformed response");
  });

  it("announces a failed read instead of silently showing an empty board", () => {
    render(
      <GridWorldStage events={[]} mode="recorded" running={false} error="the store could not be read" />,
    );
    expect(screen.getByRole("alert")).toHaveTextContent("the store could not be read");
  });
});

describe("reaching the durable recording", () => {
  it("offers to replay a run by identifier, while the run is still live", () => {
    const onLoadRecording = vi.fn();
    render(
      <GridWorldStage
        events={completedEpisode()}
        mode="live"
        running={false}
        recordingId="run-0001"
        onLoadRecording={onLoadRecording}
      />,
    );
    expect(screen.getByText(/Replay it to step through every turn/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "REPLAY RECORDING" }));
    expect(onLoadRecording).toHaveBeenCalledWith("run-0001");
  });

  it("says a loaded recording is read back rather than re-executed", () => {
    // The one claim a replay must never make: that opening a recording costs another run.
    render(
      <GridWorldStage
        events={completedEpisode()}
        mode="recorded"
        running={false}
        sourceLabel="1791932568098-0000000000000000"
        onClearRecording={vi.fn()}
      />,
    );
    expect(screen.getByText(/Nothing is re-executed/)).toBeInTheDocument();
  });

  it("offers a way back to the live feed once a recording is loaded", () => {
    const onClearRecording = vi.fn();
    render(
      <GridWorldStage
        events={completedEpisode()}
        mode="recorded"
        running={false}
        sourceLabel="1791932568098-0000000000000000"
        onClearRecording={onClearRecording}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "BACK TO LIVE" }));
    expect(onClearRecording).toHaveBeenCalled();
  });
});