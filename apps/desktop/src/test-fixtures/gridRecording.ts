/**
 * A recorded grid episode, shaped the way the engine records one.
 *
 * Shared by the frame tests, the playback tests and the panel tests so that all three are looking at the
 * same recording. Three turns walking south then east onto the goal, with a hazard, an obstacle and the
 * goal tile all inside the sensing radius, and eighteen of the twenty-five tiles never observed at all.
 *
 * That last part is the point of the fixture. A recording in which everything is visible would pass a
 * whole family of wrong implementations, and this is the shape a partially observed benchmark actually
 * produces.
 */

import type { ActionOutcome, Coordinate, Observation, TelemetryEvent, TileKind } from "../api";

/** The five-by-five world every shipped grid benchmark declares. */
export const GRID_SIZE = 5;

/** Every coordinate on the board, for "nothing here was ever sensed" assertions. */
export const ALL_COORDINATES: Coordinate[] = Array.from({ length: GRID_SIZE * GRID_SIZE }, (_, index) => ({
  x: index % GRID_SIZE,
  y: Math.floor(index / GRID_SIZE),
}));

/** A five-by-five observation with the neighbours the run actually sensed. */
export function gridObservation(
  position: Coordinate,
  neighbours: [Coordinate, TileKind][],
  distance: number,
  turn: number,
): Observation {
  return {
    turn,
    position,
    grid_size: { width: GRID_SIZE, height: GRID_SIZE },
    current_tile: "safe",
    visible_neighbors: neighbours.map(([coordinate, tile]) => ({ coordinate, tile })),
    distance_to_goal: distance,
    max_turns: 12,
    remaining_turns: 12 - turn + 1,
    available_actions: ["UP", "DOWN", "LEFT", "RIGHT", "WAIT"],
  };
}

/** An executed move, with the observation that follows it when the episode continues. */
export function movedOutcome(
  action: "UP" | "DOWN" | "LEFT" | "RIGHT" | "WAIT",
  kind: "moved" | "waited" | "invalid",
  to: Coordinate,
  after: Observation | null,
  reward = 0,
): ActionOutcome {
  return { action, kind, position: to, reward, observation: after };
}

/** A committed turn, with the observer-owned fields filled in. */
export function gridEvent(
  over: Partial<TelemetryEvent> & Pick<TelemetryEvent, "turn" | "observation">,
): TelemetryEvent {
  return {
    run_id: "run-under-test",
    episode_id: "episode-1",
    sequence: over.turn,
    recorded_at_unix_ms: 1_700_000_000_000 + over.turn,
    selected_action: null,
    requested_capability: null,
    action_outcome: null,
    reward: 0,
    invalid_action: false,
    timeout: false,
    resource_info: null,
    integrity_event: null,
    ...over,
  };
}

export const FIRST_OBSERVATION = gridObservation(
  { x: 0, y: 0 },
  [
    [{ x: 0, y: 1 }, "safe"],
    [{ x: 1, y: 0 }, "danger"],
  ],
  5,
  1,
);

export const SECOND_OBSERVATION = gridObservation(
  { x: 0, y: 1 },
  [
    [{ x: 0, y: 0 }, "safe"],
    [{ x: 0, y: 2 }, "safe"],
    [{ x: 1, y: 1 }, "obstacle"],
  ],
  4,
  2,
);

export const THIRD_OBSERVATION = gridObservation(
  { x: 0, y: 2 },
  [
    [{ x: 0, y: 1 }, "safe"],
    [{ x: 1, y: 2 }, "reward"],
    [{ x: 0, y: 3 }, "danger"],
  ],
  3,
  3,
);

/**
 * Three turns, then the episode ends on the move onto the goal.
 *
 * The final move records a landing position and no further observation, which is what a completed episode
 * looks like in the store: where the agent ended is recorded, what it sensed afterwards is not.
 */
export function completedEpisode(): TelemetryEvent[] {
  return [
    gridEvent({
      turn: 1,
      observation: FIRST_OBSERVATION,
      selected_action: "DOWN",
      action_outcome: movedOutcome("DOWN", "moved", { x: 0, y: 1 }, SECOND_OBSERVATION),
    }),
    gridEvent({
      turn: 2,
      observation: SECOND_OBSERVATION,
      selected_action: "DOWN",
      action_outcome: movedOutcome("DOWN", "moved", { x: 0, y: 2 }, THIRD_OBSERVATION),
    }),
    gridEvent({
      turn: 3,
      observation: THIRD_OBSERVATION,
      selected_action: "RIGHT",
      action_outcome: movedOutcome("RIGHT", "moved", { x: 1, y: 2 }, null),
    }),
  ];
}

/**
 * The same walk with the third turn's model call failing.
 *
 * A protocol failure consumes a turn and moves nothing, so it must not produce a position frame — the
 * defect a viewer would show as an episode where the agent moved without deciding to.
 */
export function failedFinalTurnEpisode(): TelemetryEvent[] {
  const events = completedEpisode();
  return [
    events[0],
    events[1],
    gridEvent({
      turn: 3,
      observation: THIRD_OBSERVATION,
      selected_action: null,
      action_outcome: null,
      model_failure: {
        kind: "malformed_response",
        message: "the provider returned no action",
        response_text: null,
        usage: null,
        elapsed_ms: 4,
      },
    }),
  ];
}

/** A recording that stopped after two turns, as a cancelled run's recording does. */
export function truncatedEpisode(): TelemetryEvent[] {
  return completedEpisode().slice(0, 2);
}