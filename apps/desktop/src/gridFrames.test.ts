/**
 * Frame derivation: the tests are all about what the derivation is *not* allowed to do.
 *
 * A navigation view that fills in the blanks would look better and mean something different. The
 * benchmark publishes a distance to the goal rather than a direction, and a tile the agent never sensed
 * is genuinely unknown — so every assertion below is about a tile that *could* have been invented and
 * must not be.
 */

import { describe, expect, it } from "vitest";

import {
  boardAt,
  boardSize,
  gridFrames,
  perceptionBox,
  routeTo,
  tileAt,
  wasObserved,
  type GridFrame,
} from "./gridFrames";
import {
  ALL_COORDINATES,
  completedEpisode,
  failedFinalTurnEpisode,
  FIRST_OBSERVATION,
  GRID_SIZE,
  gridEvent,
  gridObservation,
  movedOutcome,
  SECOND_OBSERVATION,
  THIRD_OBSERVATION,
  truncatedEpisode,
} from "./test-fixtures/gridRecording";
import { frameDescription, outcomeLabel, trendFor, verdictLabel } from "./GridWorldStage";

describe("frame derivation", () => {
  it("produces one frame per turn plus the position the final move left behind", () => {
    const frames = gridFrames(completedEpisode());
    expect(frames).toHaveLength(4);
    expect(frames.map((frame) => frame.kind)).toEqual(["turn", "turn", "turn", "after-move"]);
  });

  it("puts the last frame where the last recorded move landed", () => {
    const frames = gridFrames(completedEpisode());
    // The recorded outcome position, not the last observation position. A viewer that stopped a square
    // short would show a successful episode never reaching the tile it reached.
    expect(frames.at(-1)?.position).toEqual({ x: 1, y: 2 });
  });

  it("reports an absent observation rather than carrying the previous turn's sensing forward", () => {
    const frames = gridFrames(completedEpisode());
    const last = frames.at(-1);
    expect(last?.observation).toBeNull();
    expect(last?.turn).toBe(3);
  });

  it("adds no position frame for a turn that executed nothing", () => {
    // A protocol failure consumes a turn and moves nothing, so there is no position to record after it.
    const frames = gridFrames(failedFinalTurnEpisode());
    expect(frames).toHaveLength(3);
    expect(frames.at(-1)?.kind).toBe("turn");
    expect(frames.at(-1)?.outcome).toBeNull();
  });

  it("orders by the recorded sequence rather than by arrival order", () => {
    const shuffled = [completedEpisode()[2], completedEpisode()[0], completedEpisode()[1]];
    const frames = gridFrames(shuffled);
    expect(frames.map((frame) => frame.turn)).toEqual([1, 2, 3, 3]);
    expect(frames.map((frame) => frame.index)).toEqual([0, 1, 2, 3]);
  });

  it("yields nothing for a recording that holds nothing", () => {
    // No invented opening frame: a viewer that showed one would be showing a position no run reached.
    expect(gridFrames([])).toEqual([]);
    expect(boardSize(gridFrames([]))).toBeNull();
  });

  it("ends a cancelled recording where it was cut, with no frame past the stop", () => {
    const frames = gridFrames(truncatedEpisode());
    expect(frames).toHaveLength(3);
    expect(frames.at(-1)?.position).toEqual({ x: 0, y: 2 });
  });
});

describe("what the board is allowed to show", () => {
  const frames = gridFrames(completedEpisode());

  it("leaves every tile no observation carried unseen", () => {
    const observed = new Set<string>();
    for (const frame of frames) {
      for (const cell of boardAt(frame, GRID_SIZE, GRID_SIZE)) {
        if (cell.tile !== null) {
          observed.add(`${cell.x},${cell.y}`);
        }
      }
    }
    expect([...observed].sort()).toEqual(
      ["0,0", "0,1", "0,2", "0,3", "1,0", "1,1", "1,2"].sort(),
    );
    expect(observed.size).toBe(7);
  });

  it("never places terrain on a tile no frame observed", () => {
    // (4,4) is off every recorded sensing. If anything in the derivation invented terrain, this is where
    // it would show: the far corner, the tile a reader would most expect to see painted.
    for (const frame of frames) {
      for (const coordinate of ALL_COORDINATES) {
        if (!["0,0", "0,1", "0,2", "0,3", "1,0", "1,1", "1,2"].includes(`${coordinate.x},${coordinate.y}`)) {
          expect(tileAt(frame, coordinate)).toBeNull();
        }
      }
    }
    expect(wasObserved(frames, { x: 4, y: 4 })).toBe(false);
    expect(wasObserved(frames, { x: 1, y: 2 })).toBe(true);
  });

  it("draws an unobserved tile as a cell that says nothing was sensed", () => {
    const cell = boardAt(frames.at(-1) as GridFrame, GRID_SIZE, GRID_SIZE).find((c) => c.x === 4 && c.y === 4);
    expect(cell).toEqual({ x: 4, y: 4, tile: null, perceived: false, fresh: false, agent: false });
  });

  it("only ever grows the map by what that turn's observation carried", () => {
    let previous = new Set<string>();
    for (const frame of frames) {
      const current = new Set(Object.keys(frame.known));
      for (const key of current) {
        if (!previous.has(key)) {
          const [x, y] = key.split(",").map(Number);
          expect(frame.newlySeen).toContainEqual({ x, y });
        }
      }
      // Nothing is ever forgotten: a tile once sensed is part of what the run knows it saw.
      for (const key of previous) {
        expect(current.has(key)).toBe(true);
      }
      previous = current;
    }
  });

  it("carries no knowledge between episodes", () => {
    const secondEpisode = gridObservation({ x: 3, y: 3 }, [[{ x: 2, y: 3 }, "obstacle"]], 1, 1);
    const frames = gridFrames([
      ...completedEpisode(),
      gridEvent({
        turn: 1,
        episode_id: "episode-2",
        observation: secondEpisode,
        selected_action: "WAIT",
        action_outcome: movedOutcome("WAIT", "waited", { x: 3, y: 3 }, null),
      }),
    ]);
    const opened = frames.find((frame) => frame.episodeId === "episode-2") as GridFrame;
    // A different episode is a different world. Carrying the first episode's map in would put a hazard
    // the agent never sensed, in this episode, on this board.
    expect(tileAt(opened, { x: 1, y: 0 })).toBeNull();
    expect(tileAt(opened, { x: 2, y: 3 })).toBe("obstacle");
  });
});

describe("the perception window", () => {
  const frames = gridFrames(completedEpisode());

  it("is the recorded neighbourhood, not an assumed radius", () => {
    // Turn two sensed three neighbours, so the window is two columns wide by three rows tall.
    expect(perceptionBox(frames[1], GRID_SIZE, GRID_SIZE)).toEqual({ x: 0, y: 0, columns: 2, rows: 3 });
    // Turn one sensed two, both to the right and below.
    expect(perceptionBox(frames[0], GRID_SIZE, GRID_SIZE)).toEqual({ x: 0, y: 0, columns: 2, rows: 2 });
  });

  it("stays inside the board when the agent is in a corner", () => {
    const corner = gridFrames([
      gridEvent({
        turn: 1,
        observation: gridObservation({ x: 0, y: 0 }, [[{ x: 1, y: 0 }, "safe"]], 4, 1),
        selected_action: "RIGHT",
        action_outcome: movedOutcome("RIGHT", "moved", { x: 1, y: 0 }, null),
      }),
    ]);
    const box = perceptionBox(corner[0], GRID_SIZE, GRID_SIZE);
    expect(box.x + box.columns).toBeLessThanOrEqual(GRID_SIZE);
    expect(box.y + box.rows).toBeLessThanOrEqual(GRID_SIZE);
  });
});

describe("the route", () => {
  const frames = gridFrames(completedEpisode());

  it("is the recorded positions with repeated stops collapsed", () => {
    expect(routeTo(frames, frames.length - 1)).toEqual([
      { x: 0, y: 0 },
      { x: 0, y: 1 },
      { x: 0, y: 2 },
      { x: 1, y: 2 },
    ]);
  });

  it("stops at the frame it is asked for, so the line never runs ahead of the marker", () => {
    expect(routeTo(frames, 1)).toEqual([
      { x: 0, y: 0 },
      { x: 0, y: 1 },
    ]);
    expect(routeTo(frames, 0)).toEqual([{ x: 0, y: 0 }]);
  });

  it("collapses a refused move rather than drawing a line through a wall", () => {
    const refused = gridFrames([
      gridEvent({
        turn: 1,
        observation: FIRST_OBSERVATION,
        selected_action: "RIGHT",
        invalid_action: true,
        action_outcome: movedOutcome("RIGHT", "invalid", { x: 0, y: 0 }, SECOND_OBSERVATION),
      }),
    ]);
    // One frame, one point: the agent did not move, and the route says so.
    expect(routeTo(refused, refused.length - 1)).toEqual([{ x: 0, y: 0 }]);
  });
});

describe("wording", () => {
  const frames = gridFrames(completedEpisode());

  it("says how much of the world has never been seen", () => {
    const described = frameDescription(frames.at(-1) as GridFrame, GRID_SIZE, GRID_SIZE);
    expect(described).toContain("7 of 25 tiles observed");
    expect(described).toContain("18 never seen");
  });

  it("says the distance was not recorded rather than repeating an earlier reading", () => {
    const described = frameDescription(frames.at(-1) as GridFrame, GRID_SIZE, GRID_SIZE);
    expect(described).toContain("distance to the goal was not recorded");
  });

  it("reports a turn's outcome rather than assuming one", () => {
    expect(outcomeLabel(frames[0])).toBe("moved");
    expect(outcomeLabel(gridFrames([gridEvent({ turn: 1, observation: FIRST_OBSERVATION })])[0])).toBe(
      "no decision",
    );
    expect(outcomeLabel(gridFrames(failedFinalTurnEpisode()).at(-1) as GridFrame)).toBe(
      "malformed response",
    );
  });

  it("wordses a verdict as a claim about the run", () => {
    expect(verdictLabel("success")).toBe("Reached the goal");
    expect(verdictLabel("turn_limit")).toBe("Ran out of budget");
  });

  it("distinguishes getting closer from a stall from having nothing to compare against", () => {
    expect(trendFor(frames, 0)).toBe("gw-trend");
    expect(trendFor(frames, 1)).toBe("gw-trend gw-trend--closer");
    expect(trendFor(frames, frames.length - 1)).toBe("gw-trend");
  });

  it("describes the board as an image, with the unseen count stated", () => {
    const board = boardAt(frames[1], GRID_SIZE, GRID_SIZE);
    expect(board).toHaveLength(GRID_SIZE * GRID_SIZE);
    // Turn two sensed four tiles, but two of them were already known, so only two are new. Counting the
    // whole sensing as "revealed" every turn would replay the first sight of a tile forever.
    expect(board.filter((cell) => cell.perceived).length).toBe(4);
    expect(board.filter((cell) => cell.fresh).length).toBe(2);
  });

  it("keeps the goal tile recognisable once the agent has sensed it", () => {
    expect(tileAt(frames[2], { x: 1, y: 2 })).toBe("reward");
    expect(tileAt(frames[1], { x: 1, y: 2 })).toBeNull();
    expect(SECOND_OBSERVATION.visible_neighbors).toHaveLength(3);
    expect(THIRD_OBSERVATION.current_tile).toBe("safe");
  });
});