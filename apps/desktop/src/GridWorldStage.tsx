/**
 * The grid-world stage: a run's recorded turns, drawn as a navigation trace.
 *
 * # What this component is allowed to know
 *
 * Only what a recorded event contains. Every tile on the board, every position of the marker, every
 * number in the readout, comes from `gridFrames`, which takes terrain exclusively from observations the
 * engine committed. This file draws; it decides nothing about the world.
 *
 * The one thing it will not compute is a verdict. "Did the run succeed?" is the independent evaluator's
 * answer, stored in the run's result, and it arrives here as `verdict`. A panel that derived its own
 * success label from the last position would agree with the evaluator most of the time and disagree in
 * exactly the cases where it matters — so no such derivation exists in this file, and when no verdict is
 * available the stage says the recording ends here and stops.
 *
 * # Why the board is an image and not a grid widget
 *
 * It is a single animated picture of a state, and the complete state is spoken in one sentence: where the
 * agent is, what it is standing on, how far the goal is, and how much of the world has never been seen.
 * The per-cell detail a screen-reader user actually acts on is in the readout and the turn log beside it,
 * both of which are ordinary text.
 *
 * The alternative — a `role="grid"` of 25 cells — was the previous design and it had the opposite
 * problem: a cell-by-cell reading of a partially observed world reads as a map the agent has, which is
 * precisely the claim this stage is built not to make.
 *
 * # Animation is an enhancement, never the message
 *
 * Every animated property is a transition or a decorative keyframe layered over a layout that is already
 * correct when static: the marker sits at a recorded coordinate whether or not it slid there, and a
 * revealed tile is drawn whether or not it popped. `prefers-reduced-motion` collapses the durations; it
 * does not change what is on screen.
 */

import { useId, useMemo, type CSSProperties } from "react";

import { boardAt, boardSize, gridFrames, perceptionBox, routeTo, type GridFrame } from "./gridFrames";
import {
  frameTurn,
  GRID_PLAYBACK_SPEEDS,
  useGridPlayback,
  type GridSpeedLabel,
} from "./useGridPlayback";
import type { CompletionStatus, Coordinate, TelemetryEvent, TileKind } from "./api";

/** The evaluator's stored answer for a run. Passed in; never derived here. */
export type GridVerdict = {
  status: CompletionStatus;
  turnsUsed: number;
  totalReward: number;
};

export type GridWorldStageProps = {
  events: TelemetryEvent[];
  /** `live` while the run is executing; `recorded` once it is finished and can be stepped. */
  mode: "live" | "recorded";
  running: boolean;
  verdict?: GridVerdict | null;
  /** What the recording is, so a replayed run is never mistaken for one in progress. */
  sourceLabel?: string | null;
  /** The run identifier a replay can be requested for. */
  recordingId?: string | null;
  loading?: boolean;
  error?: string | null;
  onLoadRecording?: (runId: string) => void;
  onClearRecording?: () => void;
};

/**
 * What the stage says it is showing.
 *
 * Four distinct states, because the one thing a viewer of a run must never have to guess is whether what
 * they are looking at is happening now, happened a moment ago in this session, or is being read back off
 * disk. Collapsing any two of them into one chip would make a replayed run read as progress.
 *
 * `sourceLabel` is only consulted for a loaded recording; the other three are facts about this component's
 * own props.
 */
export function sourceChipLabel(
  mode: "live" | "recorded",
  running: boolean,
  frameCount: number,
  label?: string | null,
): string {
  if (mode === "recorded") {
    return label ? `RECORDED ${shortRunId(label)}` : "RECORDED RUN";
  }
  if (running) {
    return "LIVE RECORDING";
  }
  return frameCount > 0 ? "LAST RUN · SESSION" : "AWAITING A RUN";
}

/**
 * A run identifier short enough to sit in a chip.
 *
 * The full identifier is 34 characters. Printed in full it is 380px of chrome, and it pushed the panel's
 * heading onto two lines at 1366x768 — which pushed the board 40px further down and off the fold. The
 * first twelve characters are the timestamp and identify the run to anyone reading it; the full value is
 * on the control deck's RUN ID line and on the chip's tooltip, so nothing is lost.
 */
export function shortRunId(runId: string, keep = 12): string {
  if (runId.length <= keep + 1) {
    return runId;
  }
  return `${runId.slice(0, keep)}…`;
}

/** The completion status as a word, not a stored enum spelling. */
export function verdictLabel(status: CompletionStatus): string {
  switch (status) {
    case "success":
      return "Reached the goal";
    case "turn_limit":
      return "Ran out of budget";
    case "model_protocol_failure":
      return "Protocol failure";
    case "failure":
    default:
      return "Did not reach the goal";
  }
}

/** How a turn's outcome is worded in the turn log. */
export function outcomeLabel(frame: GridFrame): string {
  if (frame.failure) {
    return frame.failure.replaceAll("_", " ");
  }
  if (frame.outcome === null) {
    return "no decision";
  }
  return frame.outcome;
}

/**
 * The spoken description of one frame.
 *
 * Four facts, in the order a person needs them: where the agent is, what it is on, how far the goal is,
 * and — last, because it is the one that matters for reading this honestly — how much of the world has
 * never been observed.
 */
export function frameDescription(frame: GridFrame | null, width: number, height: number): string {
  if (!frame) {
    return "No turn has been recorded yet.";
  }
  const cells = boardAt(frame, width, height);
  const known = cells.filter((cell) => cell.tile !== null).length;
  const distance =
    frame.observation === null
      ? "distance to the goal was not recorded for this position"
      : `distance to the goal ${frame.observation.distance_to_goal}`;
  const standing =
    frame.observation === null
      ? "terrain under the agent was not recorded for this position"
      : `standing on ${frame.observation.current_tile.replaceAll("_", " ")}`;
  return (
    `${width} by ${height} grid. Agent at ${frame.position.x}, ${frame.position.y}, ${standing}. ` +
    `${distance}. ${known} of ${width * height} tiles observed, ${width * height - known} never seen.`
  );
}

/**
 * The route as SVG points, at cell centres in a `0..columns` / `0..rows` space.
 *
 * The `viewBox` is unscaled and `preserveAspectRatio="none"` lets the stroke follow the board's real
 * shape; `vector-effect="non-scaling-stroke"` in the stylesheet keeps its width in screen pixels
 * instead of letting a five-unit viewBox squash it.
 */
function routePoints(route: Coordinate[]): string {
  return route.map((point) => `${point.x + 0.5},${point.y + 0.5}`).join(" ");
}

/** One small chip describing a tile the agent saw on a turn. */
function SensedChip({ x, y, tile }: { x: number; y: number; tile: TileKind }) {
  return (
    <li className={`gw-sensed gw-sensed--${tile}`}>
      <span className="gw-sensed-at">
        {x},{y}
      </span>
      <span className="gw-sensed-tile">{tile.replaceAll("_", " ")}</span>
    </li>
  );
}

/**
 * The stage.
 *
 * Owns its playback position and nothing else: it renders the frame it is handed, and every value it
 * prints is read off that frame.
 */
export function GridWorldStage({
  events,
  mode,
  running,
  verdict = null,
  sourceLabel = null,
  recordingId = null,
  loading = false,
  error = null,
  onLoadRecording,
  onClearRecording,
}: GridWorldStageProps) {
  const headingId = useId();
  const frames = useMemo(() => gridFrames(events), [events]);
  const playback = useGridPlayback(frames, mode);
  const size = boardSize(frames);
  const frame = playback.current;

  const className = [
    "panel",
    "canvas-dark",
    "gw-stage",
    running ? "gw-stage--live" : "",
    mode === "recorded" ? "gw-stage--recorded" : "",
  ]
    .filter(Boolean)
    .join(" ");

  return (
    <section className={className} aria-labelledby={headingId}>
      <div className="panel-heading gw-heading">
        <div>
          <p className="eyebrow">NAVIGATION TRACE</p>
          <h2 id={headingId}>Grid-world trajectory</h2>
        </div>
        <div className="gw-heading-actions">
          {mode === "live" && recordingId && onLoadRecording ? (
            <button
              type="button"
              className="gw-button"
              onClick={() => onLoadRecording(recordingId)}
              disabled={loading || events.length === 0}
            >
              {loading ? "READING…" : "REPLAY RECORDING"}
            </button>
          ) : null}
          <span
            className={mode === "recorded" ? "gw-source gw-source--recorded" : running ? "gw-source gw-source--live" : "gw-source"}
            title={mode === "recorded" ? (sourceLabel ?? "Recorded run") : undefined}
          >
            <span className="gw-source-dot" aria-hidden="true" />
            {sourceChipLabel(mode, running, frames.length, sourceLabel ?? null)}
          </span>
        </div>
      </div>

      {/*
        What the recording is, in one line, next to the board it produces. It is stated rather than
        implied because the two sources look identical otherwise: a run being replayed and a run being
        watched are the same events drawn the same way, and the only difference is where they came from.

        Suppressed when there is nothing to draw — the empty state below already says exactly that, and
        two sentences saying it is one too many.
      */}
      {frames.length > 0 ? (
        <p className="gw-source-note">
          {mode === "recorded"
            ? "Read back from the local store. Nothing is re-executed."
            : running
              ? "Streaming from the engine. Each frame is a turn the run committed."
              : "The last run in this session. Replay it to step through every turn."}
        </p>
      ) : null}

      {error ? (
        <p className="error-message" role="alert">
          {error}
        </p>
      ) : null}

      {frames.length === 0 || size === null ? (
        <p className="gw-empty">
          {events.length === 0
            ? "Nothing recorded yet. The board is drawn from committed turns, so it stays empty until the run commits its first observation."
            : "This recording carries no board dimensions, so there is nothing to draw."}
        </p>
      ) : (
        <>
          <div className="gw-body">
            <div className="gw-board-column">
              <div
                className="gw-board"
                role="img"
                aria-label={frameDescription(frame, size.width, size.height)}
                style={
                  {
                    "--gw-cols": size.width,
                    "--gw-rows": size.height,
                    "--gw-x": frame?.position.x ?? 0,
                    "--gw-y": frame?.position.y ?? 0,
                  } as CSSProperties
                }
              >
                <div className="gw-field">
                  {boardAt(frame ?? frames[0], size.width, size.height).map((cell) => (
                    <div
                      key={`${cell.x}-${cell.y}`}
                      aria-hidden="true"
                      data-tile={cell.tile ?? "unseen"}
                      className={[
                        "gw-cell",
                        cell.perceived ? "gw-cell--perceived" : "",
                        cell.fresh ? "gw-cell--fresh" : "",
                        cell.agent ? "gw-cell--agent" : "",
                      ]
                        .filter(Boolean)
                        .join(" ")}
                    />
                  ))}

                  {/* The route the agent took, up to this frame and no further. */}
                  {(() => {
                    const route = routeTo(playback.frames, playback.position);
                    if (route.length < 2) {
                      return null;
                    }
                    return (
                      <svg
                        className="gw-route"
                        viewBox={`0 0 ${size.width} ${size.height}`}
                        preserveAspectRatio="none"
                        aria-hidden="true"
                      >
                        <polyline
                          key={`${route.length}-${route.at(-1)?.x}-${route.at(-1)?.y}`}
                          className="gw-route-line"
                          points={routePoints(route)}
                        />
                      </svg>
                    );
                  })()}

                  {/* The window of world the agent could see, sized from the recorded neighbours. */}
                  {(() => {
                    const box = frame
                      ? perceptionBox(frame, size.width, size.height)
                      : { x: 0, y: 0, columns: 1, rows: 1 };
                    return (
                      <div
                        className="gw-perception"
                        aria-hidden="true"
                        style={
                          {
                            "--gw-px": box.x,
                            "--gw-py": box.y,
                            "--gw-pcols": box.columns,
                            "--gw-prows": box.rows,
                          } as CSSProperties
                        }
                      />
                    );
                  })()}

                  <div className="gw-agent" aria-hidden="true">
                    <span className="gw-agent-core" />
                  </div>

                  {running ? <div className="gw-sweep" aria-hidden="true" /> : null}
                </div>
                <div className="gw-axis" aria-hidden="true">
                  {Array.from({ length: size.width }, (_, x) => (
                    <span key={x}>{x}</span>
                  ))}
                </div>
              </div>

              {/*
                The terrain key. It is here because the cells are shapes rather than glyphs — a reader
                should not have to guess that a red hatch means the episode ends there — and because the
                last swatch is the one that matters most: a hatched void is not flat ground, it is a tile
                the agent never sensed.
              */}
              <div className="legend">
                <span><i className="legend-swatch safe" />SAFE</span>
                <span><i className="legend-swatch reward" />GOAL</span>
                <span><i className="legend-swatch danger" />DANGER</span>
                <span><i className="legend-swatch obstacle" />OBSTACLE</span>
                <span><i className="legend-swatch unknown" />NOT SENSED</span>
              </div>
            </div>

            <div className="gw-side">
              {/*
                The numbers for the frame on screen.

                Beside the board rather than under it, which is a layout decision with a measurement behind
                it: stacked, the board started roughly 370px lower and fell below the fold at every target
                resolution, so the subject of the panel was the one thing a user could not see.
              */}
              <dl className="gw-readout">
                <div>
                  <dt>TURN</dt>
                  <dd>
                    {frameTurn(frame)}
                    {/* The budget is only printed beside the turn when this frame carries an observation
                        to read it from. On the frame after a run's last move there is none, and
                        "07 / —" would look like a budget of nothing rather than a reading that was not
                        recorded. */}
                    {frame?.observation ? (
                      <span className="gw-readout-of"> / {frame.observation.max_turns}</span>
                    ) : null}
                  </dd>
                </div>
                <div>
                  <dt>DECIDED</dt>
                  <dd>{frame?.action ? frame.action.toLowerCase() : "nothing"}</dd>
                </div>
                <div>
                  <dt>RESULT</dt>
                  <dd>{frame ? outcomeLabel(frame) : "—"}</dd>
                </div>
                <div>
                  <dt>TURN REWARD</dt>
                  {/*
                    Named for what it is. The bare word "reward" beside the verdict band's "reward +3"
                    read as a contradiction — one turn produced +0, the episode produced +3, and a label
                    that does not say which is the kind of thing this panel exists not to do.
                  */}
                  <dd>
                    {frame ? `${frame.reward >= 0 ? "+" : ""}${frame.reward}` : "—"}
                  </dd>
                </div>
                <div>
                  <dt>DISTANCE</dt>
                  <dd className={trendFor(playback.frames, playback.position)}>
                    {distanceFor(frame)}
                  </dd>
                </div>
                <div>
                  <dt>SEEN</dt>
                  <dd>
                    {frame
                      ? `${boardAt(frame, size.width, size.height).filter((cell) => cell.tile !== null).length} / ${size.width * size.height}`
                      : "—"}
                  </dd>
                </div>
              </dl>
              {/*
                What the agent actually sensed on this turn. It is here rather than hidden because the
                partial observability is the point of the benchmark: a reader can see exactly how little
                the agent knew when it chose.
              */}
              <section aria-label="Sensed on this turn">
                <h3 className="gw-side-title">SENSED THIS TURN</h3>
                {frame && frame.perceived.length > 0 && frame.observation ? (
                  <ul className="gw-sensed-list">
                    <SensedChip
                      x={frame.observation.position.x}
                      y={frame.observation.position.y}
                      tile={frame.observation.current_tile}
                    />
                    {frame.observation.visible_neighbors.map((neighbor) => (
                      <SensedChip
                        key={`${neighbor.coordinate.x},${neighbor.coordinate.y}`}
                        x={neighbor.coordinate.x}
                        y={neighbor.coordinate.y}
                        tile={neighbor.tile}
                      />
                    ))}
                  </ul>
                ) : (
                  <p className="gw-side-empty">
                    {frame
                      ? "No observation was recorded for this position."
                      : "Nothing sensed yet."}
                  </p>
                )}
              </section>

              <section className="gw-log-block" aria-label="Recorded turns">
                <h3 className="gw-side-title">TURNS</h3>
                {frames.length === 0 ? (
                  <p className="gw-side-empty">No turns recorded.</p>
                ) : (
                  <ol
                    className="gw-log"
                    // Focusable because it scrolls. axe's `scrollable-region-focusable` is right: a
                    // region a keyboard user cannot focus cannot be scrolled by one, so a long recording
                    // would be reachable with a mouse and not with a keyboard. No `aria-label` here —
                    // the section above already names this list, and a second name would make a screen
                    // reader announce it twice.
                    tabIndex={0}
                  >
                    {playback.frames
                      .slice()
                      .reverse()
                      .slice(0, 9)
                      .map((entry) => (
                        <li
                          key={`${entry.episodeId}-${entry.index}`}
                          className={
                            entry.index === playback.position ? "gw-log-row gw-log-row--current" : "gw-log-row"
                          }
                          aria-current={entry.index === playback.position ? "step" : undefined}
                        >
                          <span className="gw-log-turn">{frameTurn(entry)}</span>
                          <span className="gw-log-action">
                            {entry.kind === "after-move" ? "→" : (entry.action ?? "—").toLowerCase()}
                          </span>
                          <span className="gw-log-outcome">{outcomeLabel(entry)}</span>
                          <span className="gw-log-reward">
                            {entry.reward >= 0 ? `+${entry.reward}` : entry.reward}
                          </span>
                        </li>
                      ))}
                  </ol>
                )}
              </section>
            </div>
          </div>

          {verdict ? (
            <p className={verdict.status === "success" ? "gw-verdict gw-verdict--pass" : "gw-verdict gw-verdict--fail"}>
              <span className="gw-verdict-label">{verdictLabel(verdict.status)}</span>
              <span className="gw-verdict-facts">
                {verdict.turnsUsed} turns · reward {verdict.totalReward >= 0 ? "+" : ""}
                {verdict.totalReward}
              </span>
            </p>
          ) : null}

          {/*
            The transport appears only for a complete recording. While a run is live the run itself is
            the transport, and a scrub bar over a recording still being written would invite someone to
            scrub toward an end that does not exist yet.
          */}
          {playback.steppable ? (
            <div className="gw-transport" role="group" aria-label="Recording playback">
              {onClearRecording ? (
                <button type="button" className="gw-button" onClick={onClearRecording}>
                  BACK TO LIVE
                </button>
              ) : null}
              <button
                type="button"
                className="gw-button"
                onClick={playback.toStart}
                disabled={playback.atStart}
                aria-label="First turn"
              >
                ⏮
              </button>
              <button
                type="button"
                className="gw-button gw-button--primary"
                onClick={playback.toggle}
                aria-label={playback.playing ? "Pause" : "Play"}
              >
                {playback.playing ? "❚❚" : "▶"}
              </button>
              <button
                type="button"
                className="gw-button"
                onClick={() => playback.step(-1)}
                disabled={playback.atStart}
                aria-label="Previous turn"
              >
                ◀
              </button>
              <button
                type="button"
                className="gw-button"
                onClick={() => playback.step(1)}
                disabled={playback.atEnd}
                aria-label="Next turn"
              >
                ▶
              </button>
              <button
                type="button"
                className="gw-button"
                onClick={playback.toEnd}
                disabled={playback.atEnd}
                aria-label="Last turn"
              >
                ⏭
              </button>
              <input
                className="gw-scrub"
                type="range"
                min={0}
                max={Math.max(playback.frames.length - 1, 0)}
                value={playback.position}
                onChange={(event) => playback.setPosition(Number(event.target.value))}
                aria-label="Turn within the recording"
                aria-valuetext={
                  playback.frames[playback.position]
                    ? `${playback.frames[playback.position].kind === "after-move" ? "after turn" : `turn ${playback.frames[playback.position].turn}`} of ${playback.frames.length} frames`
                    : "no frames"
                }
              />
              <span className="gw-position">
                {playback.frames.length === 0 ? "0 / 0" : `${playback.position + 1} / ${playback.frames.length}`}
              </span>
              <label className="gw-speed">
                <span className="gw-speed-label">SPEED</span>
                <select
                  className="gw-speed-select"
                  value={playback.speed}
                  onChange={(event) => playback.setSpeed(event.target.value as GridSpeedLabel)}
                >
                  {GRID_PLAYBACK_SPEEDS.map((speed) => (
                    <option key={speed.label} value={speed.label}>
                      {speed.label}
                    </option>
                  ))}
                </select>
              </label>
            </div>
          ) : (
            <div className="gw-transport gw-transport--passive" role="group" aria-label="Live recording">
              <span className="gw-live-note">
                {running
                  ? "Streaming from the engine. Each frame is a committed turn."
                  : "No run is in progress."}
              </span>
              <span className="gw-position">
                {frames.length === 0 ? "0 / 0" : `${frames.length} frames`}
              </span>
            </div>
          )}
        </>
      )}
    </section>
  );
}

/** The distance reading, or an explicit absence rather than a carried-over number. */
function distanceFor(frame: GridFrame | null): string {
  if (!frame || frame.observation === null) {
    return "not recorded";
  }
  return String(frame.observation.distance_to_goal);
}

/**
 * A movement class for the distance reading, from comparing two recorded readings.
 *
 * Closer, further, and unchanged are three distinct claims, and collapsing them into "the number went
 * down" would be enough — but colouring them is what makes a stalled policy visible at a glance, which is
 * the thing a navigation view is for. `null` when there is no previous frame to compare against, so the
 * first turn is never styled as progress.
 */
export function trendFor(frames: GridFrame[], position: number): string {
  const current = frames[position]?.observation?.distance_to_goal;
  const previous = frames[position - 1]?.observation?.distance_to_goal;
  if (current === undefined || previous === undefined || current === null || previous === null) {
    return "gw-trend";
  }
  if (current < previous) {
    return "gw-trend gw-trend--closer";
  }
  if (current > previous) {
    return "gw-trend gw-trend--further";
  }
  return "gw-trend gw-trend--flat";
}