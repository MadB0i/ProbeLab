/**
 * Turning a recorded run into frames a viewer can step through.
 *
 * # The one rule
 *
 * **A tile appears on screen only because some recorded observation contained it.** Nothing else may put
 * terrain on the board: not the seed, not the benchmark definition, not a neighbour-of-a-neighbour
 * inference, not a previous episode, and not a guess that "the run probably went that way".
 *
 * This is the whole reason this module exists rather than a `switch` inside the panel. A grid world
 * publishes a *distance to the goal*, not a direction, and the map is partially observed on purpose — a
 * panel that filled in the blanks would look like a better product and be measuring something else
 * entirely, because it would be showing an agent knowledge it did not have.
 *
 * # Frames are the recording, not a rendering of it
 *
 * Each frame is built from fields the engine committed:
 *
 * | what is drawn | which recorded field it came from |
 * | --- | --- |
 * | where the agent stands | `observation.position` |
 * | what terrain it is standing on | `observation.current_tile` |
 * | what it could see this turn | `observation.visible_neighbors` |
 * | how far the goal is | `observation.distance_to_goal` |
 * | what it decided | `selected_action` |
 * | what that decision did | `action_outcome.kind`, `.position`, `.reward` |
 *
 * There is one derived thing, and it is the only one: the position the agent occupies *between* frames.
 * That is `action_outcome.position` — recorded, not computed — and the terminal frame exists only
 * because the final recorded move has a landing position that no later event will ever describe.
 *
 * # Why the post-move frame is kept
 *
 * The engine records the observation *before* each decision, so the position the final move left the
 * agent on appears in no observation at all: it is only in `action_outcome.position`. Without that frame
 * a viewer would stop the agent one square short of where it ended, every episode, and a run that
 * reached the goal tile would visibly never reach it.
 *
 * It is appended only when the recording's last turn actually executed a transition. A turn that was
 * denied, or that failed to produce a decision, has no outcome and therefore moves nothing — inventing
 * a frame for it would be inventing a step.
 *
 * # Multi-episode runs
 *
 * A run can hold several episodes. Knowledge does not carry across one: the world is regenerated per
 * seed, so carrying a previous episode's map into the next would draw terrain the agent never saw in
 * the world it is standing in. The known set is therefore reset at each episode boundary and the frame
 * records which episode it belongs to.
 */

import type {
  Action,
  ActionKind,
  Coordinate,
  ModelCallFailureKind,
  Observation,
  TileKind,
  TelemetryEvent,
} from "./api";

/** What a frame is: the state a turn was decided in, or the state a move left behind. */
export type GridFrameKind = "turn" | "after-move";

/** One step on the timeline. Every field is a recorded fact about one turn. */
export type GridFrame = {
  /** Zero-based position in the frame list. */
  index: number;
  /** Which episode this frame belongs to; knowledge never crosses one. */
  episodeId: string;
  /** Zero-based position of this frame within its episode. */
  episodeFrame: number;
  /** The observer-owned sequence number, for tracing a frame back to a stored row. */
  sequence: number;
  /**
   * The decision turn, or `null` when the recording carried none.
   *
   * An `after-move` frame repeats the turn it is the aftermath of rather than inventing a new number; the
   * two kinds are distinguished by {@link GridFrame.kind}, not by this field.
   */
  turn: number | null;
  /** `turn` for a decision frame, `after-move` for the state a move left behind. */
  kind: GridFrameKind;
  /**
   * The observation this frame is drawn from.
   *
   * `null` on a post-move frame whose episode ended without another observation being recorded — which
   * is the normal case for the final move of a completed episode. The frame still has a position, from
   * the recorded outcome; it simply has no new *sensing* to show, and the panel says so rather than
   * reusing the previous turn's readings as though they were current.
   */
  observation: Observation | null;
  /** Where the agent stands in this frame. Always recorded, never inferred. */
  position: Coordinate;
  /** The action the run recorded as chosen this turn, or `null`. */
  action: Action | null;
  /** What the transition was classified as, or `null` when nothing was executed. */
  outcome: ActionKind | null;
  /** The reward the transition produced. `0` for a turn that executed nothing. */
  reward: number;
  /** Whether the engine recorded the action as refused by the world. */
  invalid: boolean;
  /** Whether the engine recorded this turn as ending the episode at its turn limit. */
  timeout: boolean;
  /** The recorded model-failure classification, when the turn produced no decision. */
  failure: ModelCallFailureKind | null;
  /** Terrain this run has observed, keyed `"x,y"`. Every entry came from an observation. */
  known: Record<string, TileKind>;
  /** Coordinates whose terrain this frame is the first to contain. */
  newlySeen: Coordinate[];
  /** Coordinates the agent could see *this* frame: its own tile and the recorded neighbours. */
  perceived: Coordinate[];
};

/** The recorded terrain at a coordinate on a frame, or `null` when no observation ever carried it. */
export function tileAt(frame: GridFrame, coordinate: Coordinate): TileKind | null {
  return frame.known[`${coordinate.x},${coordinate.y}`] ?? null;
}

/** Whether any frame in the recording ever observed this coordinate. The anti-fabrication check. */
export function wasObserved(frames: GridFrame[], coordinate: Coordinate): boolean {
  return frames.some((frame) => tileAt(frame, coordinate) !== null);
}

/**
 * The bounding box of what the agent could see on a frame.
 *
 * Computed from the recorded neighbour coordinates rather than from `position ± 1`, because the set of
 * visible tiles is a property the recording states. Deriving it from the position would quietly
 * re-implement the environment's visibility rule in the frontend, and a rule that changed on the Rust
 * side would then be drawn wrongly and plausibly.
 */
export function perceptionBox(
  frame: GridFrame,
  width: number,
  height: number,
): { x: number; y: number; columns: number; rows: number } {
  const points = frame.perceived.length > 0 ? frame.perceived : [frame.position];
  let minX = Number.POSITIVE_INFINITY;
  let minY = Number.POSITIVE_INFINITY;
  let maxX = Number.NEGATIVE_INFINITY;
  let maxY = Number.NEGATIVE_INFINITY;
  for (const point of points) {
    minX = Math.min(minX, point.x);
    minY = Math.min(minY, point.y);
    maxX = Math.max(maxX, point.x);
    maxY = Math.max(maxY, point.y);
  }
  // Clamped to the board. An observation cannot name a tile off the grid, but clamping here is what
  // keeps a malformed recording from positioning the overlay outside its own field.
  const left = Math.max(0, Math.min(minX, width - 1));
  const top = Math.max(0, Math.min(minY, height - 1));
  const right = Math.max(left, Math.min(maxX, width - 1));
  const bottom = Math.max(top, Math.min(maxY, height - 1));
  return { x: left, y: top, columns: right - left + 1, rows: bottom - top + 1 };
}

/**
 * The route the agent took up to and including a frame.
 *
 * One point per frame, taken from that frame's recorded position. Consecutive duplicates are dropped,
 * which is what turns it into a *path* rather than a list of stops — and it is a presentation choice
 * only: the positions themselves are exactly the ones the frames were drawn from, so the line and the
 * marker can never disagree about where the agent was.
 */
export function routeTo(frames: GridFrame[], index: number): Coordinate[] {
  const route: Coordinate[] = [];
  for (const frame of frames.slice(0, Math.max(index, 0) + 1)) {
    const previous = route.at(-1);
    if (previous && previous.x === frame.position.x && previous.y === frame.position.y) {
      continue;
    }
    route.push(frame.position);
  }
  return route;
}

/** Every cell of the board at a frame, in row-major order, for a renderer that draws all of them. */
export type GridCell = {
  x: number;
  y: number;
  /** `null` when nothing in the recording ever observed this tile. */
  tile: TileKind | null;
  /** Whether the frame's own observation carried this tile. */
  perceived: boolean;
  /** Whether this frame is the first that carried it. */
  fresh: boolean;
  /** Whether the agent stands here. */
  agent: boolean;
};

/**
 * The board as one frame describes it.
 *
 * Every cell is emitted, known or not, so a renderer never has to decide what an absent tile should
 * look like. That decision is exactly where a fabrication would creep in: a cell that is simply missing
 * from the list is a cell nothing knows about, but a cell drawn from a default is a cell that looks like
 * it was observed and is not.
 */
export function boardAt(
  frame: GridFrame,
  width: number,
  height: number,
): GridCell[] {
  const perceived = new Set(frame.perceived.map((point) => `${point.x},${point.y}`));
  const fresh = new Set(frame.newlySeen.map((point) => `${point.x},${point.y}`));
  const cells: GridCell[] = [];
  for (let y = 0; y < height; y += 1) {
    for (let x = 0; x < width; x += 1) {
      const key = `${x},${y}`;
      cells.push({
        x,
        y,
        tile: frame.known[key] ?? null,
        perceived: perceived.has(key),
        fresh: fresh.has(key),
        agent: frame.position.x === x && frame.position.y === y,
      });
    }
  }
  return cells;
}

/** Every terrain reading an observation carries, as coordinates paired with the tile it reported. */
function sensed(observation: Observation): Coordinate[] {
  const points: Coordinate[] = [
    { x: observation.position.x, y: observation.position.y },
    ...observation.visible_neighbors.map((neighbor) => ({
      x: neighbor.coordinate.x,
      y: neighbor.coordinate.y,
    })),
  ];
  return points;
}

/** The terrain an observation carries, keyed the way `known` is. */
function sensedTiles(observation: Observation): Record<string, TileKind> {
  const tiles: Record<string, TileKind> = {
    [`${observation.position.x},${observation.position.y}`]: observation.current_tile,
  };
  for (const neighbor of observation.visible_neighbors) {
    tiles[`${neighbor.coordinate.x},${neighbor.coordinate.y}`] = neighbor.tile;
  }
  return tiles;
}

function failureKindOf(event: TelemetryEvent): ModelCallFailureKind | null {
  return event.model_failure?.kind ?? null;
}

/** Events in the order they executed, whatever order they arrived in. */
function inExecutionOrder(events: TelemetryEvent[]): TelemetryEvent[] {
  return [...events].sort((left, right) => left.sequence - right.sequence);
}

/** The frames one episode's events produce, including the post-move frame for its final turn. */
function framesForEpisode(events: TelemetryEvent[], startIndex: number): GridFrame[] {
  const frames: GridFrame[] = [];
  let known: Record<string, TileKind> = {};
  const episodeId = events[0]?.episode_id ?? "";

  for (const [offset, event] of events.entries()) {
    const before = new Set(Object.keys(known));
    // Only a frame's own observation adds terrain. Accumulating across events is what makes the map grow
    // as the agent walks, and the set difference is what identifies exactly which tiles this turn
    // revealed — the difference is recorded, not estimated from the radius.
    known = { ...known, ...sensedTiles(event.observation) };
    const newlySeen = sensed(event.observation).filter(
      (point) => !before.has(`${point.x},${point.y}`),
    );
    frames.push({
      index: startIndex + offset,
      episodeId,
      episodeFrame: offset,
      sequence: event.sequence,
      turn: event.turn,
      kind: "turn",
      observation: event.observation,
      position: { x: event.observation.position.x, y: event.observation.position.y },
      action: event.selected_action,
      outcome: event.action_outcome?.kind ?? null,
      reward: event.reward,
      invalid: event.invalid_action,
      timeout: event.timeout,
      failure: failureKindOf(event),
      known,
      newlySeen,
      perceived: sensed(event.observation),
    });
  }

  const last = events.at(-1);
  const outcome = last?.action_outcome ?? null;
  if (outcome) {
    // The state the final recorded move left behind. Its observation, when the episode continued and one
    // was recorded, is the same reading the next event would have carried; when the episode ended there
    // is none, and the frame says so rather than carrying the previous turn's sensing forward.
    const after = outcome.observation ?? null;
    if (after) {
      known = { ...known, ...sensedTiles(after) };
    }
    frames.push({
      index: startIndex + events.length,
      episodeId,
      episodeFrame: events.length,
      sequence: last?.sequence ?? 0,
      // The turn this frame is the aftermath of, not a turn of its own — `kind` carries that distinction,
      // and a readout that printed "—" here would say the run lost a turn rather than that the frame
      // follows one. Both numbers are recorded.
      turn: last?.turn ?? null,
      kind: "after-move",
      observation: after,
      position: { x: outcome.position.x, y: outcome.position.y },
      action: outcome.action ?? last?.selected_action ?? null,
      outcome: outcome.kind,
      reward: outcome.reward,
      invalid: outcome.kind === "invalid",
      timeout: last?.timeout ?? false,
      failure: null,
      known,
      newlySeen: [],
      perceived: after ? sensed(after) : [],
    });
  }

  return frames;
}

/**
 * The frames a recording can be stepped through.
 *
 * Grouped by episode and ordered by sequence within each. An empty recording yields no frames, which the
 * panel renders as its own state — a viewer that invented an opening frame for a run that recorded
 * nothing would be showing a position the run never reached.
 */
export function gridFrames(events: TelemetryEvent[]): GridFrame[] {
  const ordered = inExecutionOrder(events);
  if (ordered.length === 0) {
    return [];
  }
  const episodes = new Map<string, TelemetryEvent[]>();
  for (const event of ordered) {
    const bucket = episodes.get(event.episode_id);
    if (bucket) {
      bucket.push(event);
    } else {
      episodes.set(event.episode_id, [event]);
    }
  }

  const frames: GridFrame[] = [];
  for (const episode of episodes.values()) {
    frames.push(...framesForEpisode(episode, frames.length));
  }
  return frames.map((frame, index) => ({ ...frame, index }));
}

/** The board dimensions a recording declares, or `null` when it declares none. */
export function boardSize(frames: GridFrame[]): { width: number; height: number } | null {
  const observation = frames.find((frame) => frame.observation !== null)?.observation ?? null;
  if (!observation) {
    return null;
  }
  return { width: observation.grid_size.width, height: observation.grid_size.height };
}