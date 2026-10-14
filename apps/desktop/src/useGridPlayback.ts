/**
 * Stepping through a recording of a grid episode.
 *
 * # Two modes, and why they are not one mode with a flag
 *
 * **Live.** While a run is executing, the frames arrive as the engine commits them. The recording is not
 * finished, so there is nothing to step *through*: the viewer is watching the run happen. The position is
 * therefore derived from the last frame and the transport is not offered — offering a scrub bar over a
 * recording that is still being written would invite someone to scrub to an end that does not exist yet.
 *
 * **Recorded.** A finished run, or one read back from the durable store, is complete. Stepping, playing
 * and scrubbing are meaningful, and the transport is offered.
 *
 * Splitting them here rather than inside the panel keeps the rule where it can be tested: the panel
 * renders whatever position it is handed and never decides where that position should be.
 *
 * # Nothing here executes anything
 *
 * The controller owns a frame list and an index. There is no engine, no provider, and no command in this
 * file, so a replay cannot re-run a decision, re-pay for a model call, or produce a frame the recording
 * did not contain. That is the same argument `usePlayback` makes for arena matches, and it is the reason
 * the frames arrive as data rather than as a callback that advances a world.
 *
 * # What happens when the recording changes underneath the viewer
 *
 * Any change to the frame list returns the position to the start and stops playback. Leaving the position
 * where it was would show a new recording's opening frame as though it were a late one, and would let a
 * timer keep advancing an index that no longer exists.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import type { GridFrame } from "./gridFrames";

/** The speeds offered. Milliseconds per frame. */
export const GRID_PLAYBACK_SPEEDS = [
  { label: "0.5×", delayMs: 1100 },
  { label: "1×", delayMs: 520 },
  { label: "2×", delayMs: 260 },
  { label: "FAST", delayMs: 80 },
] as const;

export type GridSpeedLabel = (typeof GRID_PLAYBACK_SPEEDS)[number]["label"];

function delayFor(label: GridSpeedLabel): number | null {
  return GRID_PLAYBACK_SPEEDS.find((speed) => speed.label === label)?.delayMs ?? null;
}

export type GridPlaybackState = {
  frames: GridFrame[];
  /** Index of the frame on screen. In `live` mode this is always the last frame. */
  position: number;
  current: GridFrame | null;
  playing: boolean;
  speed: GridSpeedLabel;
  /** Whether the shown frame is the newest one the recording holds. */
  atEnd: boolean;
  atStart: boolean;
  /** Whether stepping is offered at all — false while a run is live. */
  steppable: boolean;
  setPosition: (index: number) => void;
  play: () => void;
  pause: () => void;
  toggle: () => void;
  step: (delta: number) => void;
  toStart: () => void;
  toEnd: () => void;
  setSpeed: (label: GridSpeedLabel) => void;
};

/**
 * Drives the position over one recording.
 *
 * `mode` is the only product rule in this file, and it is passed in rather than derived, because whether
 * a recording is still being written is a fact about the run rather than about the frame list.
 */
export function useGridPlayback(
  frames: GridFrame[],
  mode: "live" | "recorded",
): GridPlaybackState {
  const steppable = mode === "recorded";
  const lastIndex = frames.length - 1;
  const [position, setPositionState] = useState(0);
  const [playing, setPlaying] = useState(false);
  const [speed, setSpeed] = useState<GridSpeedLabel>("1×");
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const clearTimer = useCallback(() => {
    if (timer.current !== null) {
      clearTimeout(timer.current);
      timer.current = null;
    }
  }, []);

  /*
   * A live run's position is the end, by definition — it is whatever the engine has committed so far.
   * Derived rather than stored so a newly committed event cannot leave the marker a frame behind, which
   * is the failure that produced a live view that was plausible and wrong.
   */
  const effective = useMemo(() => {
    if (!steppable) {
      return Math.max(lastIndex, 0);
    }
    return Math.min(Math.max(position, 0), Math.max(lastIndex, 0));
  }, [steppable, position, lastIndex]);

  const setPosition = useCallback(
    (index: number) => {
      if (!steppable) {
        return;
      }
      clearTimer();
      setPlaying(false);
      setPositionState(Math.min(Math.max(index, 0), Math.max(lastIndex, 0)));
    },
    [clearTimer, lastIndex, steppable],
  );

  /*
   * Every way the recording can change ends playback and returns to the start. `frames.length` rather
   * than the frame objects themselves, because the identity of the events changes on every commit and
   * resetting on that would restart playback on every single turn of a running episode.
   */
  const recordingKey = `${mode}:${frames.length}:${frames.at(-1)?.sequence ?? 0}`;
  useEffect(() => {
    clearTimer();
    setPlaying(false);
    setPositionState(0);
  }, [recordingKey, clearTimer]);

  /*
   * The advance itself. Scheduled rather than interval-based so a slow frame cannot queue a backlog of
   * catch-ups, and cleared on every dependency change and on unmount.
   *
   * Reaching the end stops rather than loops: a viewer that silently restarted would hide the last frame
   * of the episode, which is the frame most worth seeing.
   */
  useEffect(() => {
    if (!playing || !steppable) {
      return;
    }
    const delay = delayFor(speed);
    if (delay === null) {
      setPlaying(false);
      return;
    }
    if (effective >= lastIndex) {
      setPlaying(false);
      return;
    }
    timer.current = setTimeout(() => {
      setPositionState((current) => Math.min(current + 1, lastIndex));
    }, delay);
    return () => {
      if (timer.current !== null) {
        clearTimeout(timer.current);
        timer.current = null;
      }
    };
  }, [playing, speed, effective, lastIndex, steppable]);

  // Unmount cleanup, so a timer started just before navigating away cannot set state on a dead tree.
  useEffect(() => clearTimer, [clearTimer]);

  const pause = useCallback(() => {
    clearTimer();
    setPlaying(false);
  }, [clearTimer]);

  const play = useCallback(() => {
    if (!steppable || frames.length < 2) {
      return;
    }
    setPlaying(true);
  }, [steppable, frames.length]);

  const toggle = useCallback(() => {
    if (!steppable) {
      return;
    }
    setPlaying((current) => !current);
  }, [steppable]);

  const step = useCallback(
    (delta: number) => {
      if (!steppable) {
        return;
      }
      clearTimer();
      setPlaying(false);
      setPositionState((current) => Math.min(Math.max(current + delta, 0), Math.max(lastIndex, 0)));
    },
    [clearTimer, lastIndex, steppable],
  );

  const toStart = useCallback(() => setPosition(0), [setPosition]);
  const toEnd = useCallback(() => setPosition(lastIndex), [setPosition, lastIndex]);

  return {
    frames,
    position: effective,
    current: frames[effective] ?? null,
    playing,
    speed,
    atStart: effective <= 0,
    atEnd: effective >= lastIndex && frames.length > 0,
    steppable,
    setPosition,
    play,
    pause,
    toggle,
    step,
    toStart,
    toEnd,
    setSpeed,
  };
}

/** A frame's turn as a two-digit label, or `—` for a frame that decided nothing. */
export function frameTurn(frame: GridFrame | null): string {
  return frame?.turn === null || frame?.turn === undefined
    ? "—"
    : frame.turn.toString().padStart(2, "0");
}