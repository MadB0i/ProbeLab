/**
 * Frame-by-frame playback of a recorded arena match.
 *
 * # What this plays back, and what it does not
 *
 * It steps through `ArenaMatchView.history` — the positions the engine recorded, in order — and shows
 * one of them. It does **not** re-run the match, re-query a model, or recompute a move. There is no
 * engine in the browser and no provider, so playback cannot produce a result the recording did not
 * already contain. That is the same guarantee the Replay panel makes, and it is why the two are
 * built on the same data rather than on two different notions of "replay".
 *
 * # Why this is a view concern
 *
 * The recording is immutable and the backend is never consulted while the user moves between frames,
 * which is what makes seeking instant and provably free of side effects. A "replay" that had to ask
 * the backend for each frame would be a second execution path wearing the same name.
 *
 * # Frame 0 is the opening position
 *
 * `history[0]` is the board *after* the first ply. The position before any move was made is what a
 * viewer expects to see first, so it is reconstructed as a frame of its own by emptying the cells
 * the first recorded ply filled. A recording with no plies has exactly one frame: the opening.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import type { ArenaMatchView, GameCell, PlyView } from "./api";

/** One stop on the timeline: a position, and the move that produced it when there was one. */
export type PlaybackFrame = {
  /** Zero-based index into the frame list. */
  index: number;
  /** The recorded board for this frame. */
  cells: GameCell[];
  /** The ply this frame records, or `null` for the opening position. */
  ply: PlyView | null;
};

/** The playback speeds offered. Milliseconds per frame; `null` means "step only". */
export const PLAYBACK_SPEEDS = [
  { label: "0.5×", delayMs: 1100 },
  { label: "1×", delayMs: 550 },
  { label: "2×", delayMs: 275 },
  { label: "FAST", delayMs: 90 },
] as const;

export type PlaybackSpeedLabel = (typeof PLAYBACK_SPEEDS)[number]["label"];

function delayFor(label: PlaybackSpeedLabel): number | null {
  return PLAYBACK_SPEEDS.find((speed) => speed.label === label)?.delayMs ?? null;
}

/**
 * The frames a recording can be stepped through.
 *
 * Exported because the panel and the tests both need the same list, and a panel that built its own
 * would be free to disagree with the stepper about how many frames there are.
 *
 * Three kinds of frame, in order: the opening position, one per recorded ply, and the terminal
 * position the match ended on. The last of those is appended only when it differs from the final ply —
 * a recording that captured the terminal board as its own ply does not get a duplicated frame, and a
 * recording that did not still reaches the position the match actually ended on. Without it, a
 * recording whose ply list stops one move short would play through to a position that was never the
 * result of the match.
 */
export function playbackFrames(match: ArenaMatchView | null): PlaybackFrame[] {
  if (!match) {
    return [];
  }
  const recorded = match.history;
  const finalCells = cellsOf(match.finalBoard);
  if (recorded.length === 0) {
    // A match that recorded no plies still has exactly one position to show: where it started, or
    // where it ended if that is the only board there is.
    const opening = emptyOpening(match) ?? finalCells;
    return opening ? [{ index: 0, cells: opening, ply: null }] : [];
  }
  const frames: PlaybackFrame[] = [];
  const opening = emptyOpening(match);
  if (opening) {
    frames.push({ index: 0, cells: opening, ply: null });
  }
  for (const ply of recorded) {
    const cells = cellsOf(ply.board);
    if (!cells) {
      // A frame the panel cannot draw is not stepped through silently: it is skipped, and the frame
      // list stays shorter than `history.length` rather than rendering a blank board.
      continue;
    }
    frames.push({ index: frames.length, cells, ply });
  }
  const lastDrawn = frames[frames.length - 1];
  if (finalCells && (!lastDrawn || !sameCells(lastDrawn.cells, finalCells))) {
    frames.push({
      index: frames.length,
      cells: finalCells,
      ply: lastDrawn?.ply ?? null,
    });
  }
  return frames.map((frame, index) => ({ ...frame, index }));
}

function sameCells(left: readonly GameCell[], right: readonly GameCell[]): boolean {
  return left.length === right.length && left.every((cell, index) => cell === right[index]);
}

/** The recorded board's cells, or `null` when it is not a board this build can draw. */
function cellsOf(board: PlyView["board"]): GameCell[] | null {
  if (!board || board.game !== "tic_tac_toe") {
    return null;
  }
  const cells = (board as { state?: { cells?: unknown } }).state?.cells;
  return Array.isArray(cells) && cells.length === 9 ? (cells as GameCell[]) : null;
}

/**
 * The opening position, rebuilt from the first recorded ply.
 *
 * The engine records the board *after* each move, so the position before any move was played has to
 * come from somewhere. It is derived by clearing the cells the first ply filled, which is only sound
 * because that ply's move names the cell it changed. If it does not, no opening frame is produced
 * and the viewer starts at the first recorded position rather than inventing a board.
 */
function emptyOpening(match: ArenaMatchView): GameCell[] | null {
  const first = match.history[0];
  if (!first) {
    return cellsOf(match.finalBoard);
  }
  const cells = cellsOf(first.board);
  if (!cells) {
    return null;
  }
  const move = (first.played as { move?: unknown } | null)?.move;
  if (typeof move !== "number" || move < 0 || move > 8) {
    return null;
  }
  const opening = [...cells];
  opening[move] = "empty";
  return opening;
}

export type PlaybackState = {
  /** The frames available to step through. */
  frames: PlaybackFrame[];
  /** Which frame is showing. */
  position: number;
  /** The frame showing, or `null` when there are none. */
  current: PlaybackFrame | null;
  /** Whether playback is advancing on its own. */
  playing: boolean;
  /** The chosen speed. */
  speed: PlaybackSpeedLabel;
  /** Whether the shown match was produced by the configuration currently in the selectors. */
  stale: boolean;
  setPosition: (index: number) => void;
  play: () => void;
  pause: () => void;
  toggle: () => void;
  step: (delta: number) => void;
  toStart: () => void;
  toEnd: () => void;
  setSpeed: (label: PlaybackSpeedLabel) => void;
  /** Stops playback and returns to the opening frame. */
  reset: () => void;
  canPlay: boolean;
  atStart: boolean;
  atEnd: boolean;
};

/**
 * Drives playback over one recording.
 *
 * `stale` is passed in rather than computed here: whether the shown match belongs to the current
 * selectors is a question about the panel's configuration, and answering it here would hide a
 * product rule inside a media-player helper.
 */
export function usePlayback(
  match: ArenaMatchView | null,
  options: { stale: boolean; autoPlay?: boolean } = { stale: false },
): PlaybackState {
  const frames = useMemo(() => playbackFrames(match), [match]);
  const [position, setPositionState] = useState(0);
  const [playing, setPlaying] = useState(false);
  const [speed, setSpeed] = useState<PlaybackSpeedLabel>("1×");
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const lastIndex = frames.length - 1;
  const clamped = Math.min(Math.max(position, 0), Math.max(lastIndex, 0));

  const clearTimer = useCallback(() => {
    if (timer.current !== null) {
      clearTimeout(timer.current);
      timer.current = null;
    }
  }, []);

  const setPosition = useCallback(
    (index: number) => {
      clearTimer();
      setPlaying(false);
      setPositionState(Math.min(Math.max(index, 0), Math.max(lastIndex, 0)));
    },
    [clearTimer, lastIndex],
  );

  /*
   * Every way the recording can change ends playback and returns to the opening frame.
   *
   * Leaving the position at, say, frame 6 of a previous match and applying it to a nine-frame one
   * would show the new match's opening as though it were a late position, and leaving the timer
   * running would keep advancing a frame index that no longer exists. This is the cleanup the panel
   * depends on, and it is why the effect watches the recording rather than only the position.
   */
  useEffect(() => {
    clearTimer();
    setPlaying(false);
    setPositionState(0);
  }, [match?.matchId, frames.length, clearTimer]);

  /*
   * The advance itself. Scheduled rather than interval-based so a slow frame does not queue a backlog
   * of catch-ups, and cleared on every dependency change and on unmount.
   *
   * Reaching the end stops playback instead of looping: a viewer that silently restarted would hide
   * the end of the match, which is the frame most worth seeing.
   */
  useEffect(() => {
    if (!playing) {
      return;
    }
    const delay = delayFor(speed);
    if (delay === null) {
      return;
    }
    if (clamped >= lastIndex) {
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
  }, [playing, speed, clamped, lastIndex]);

  // Unmount cleanup, so a timer started just before navigating away cannot set state on a dead tree.
  useEffect(() => clearTimer, [clearTimer]);

  const pause = useCallback(() => {
    clearTimer();
    setPlaying(false);
  }, [clearTimer]);

  const play = useCallback(() => {
    setPlaying(true);
  }, []);

  const toggle = useCallback(() => {
    setPlaying((current) => !current);
  }, []);

  const step = useCallback(
    (delta: number) => {
      clearTimer();
      setPlaying(false);
      setPositionState((current) => Math.min(Math.max(current + delta, 0), Math.max(lastIndex, 0)));
    },
    [clearTimer, lastIndex],
  );

  const toStart = useCallback(() => setPosition(0), [setPosition]);
  const toEnd = useCallback(() => setPosition(lastIndex), [setPosition, lastIndex]);

  const reset = useCallback(() => {
    clearTimer();
    setPlaying(false);
    setPositionState(0);
  }, [clearTimer]);

  /*
   * Auto-play once a fresh recording arrives. The effect above has already moved the position back
   * to the opening, so starting here plays the match rather than resuming somewhere inside it.
   */
  const autoPlayedFor = useRef<string | null>(null);
  useEffect(() => {
    if (!options.autoPlay || !match || frames.length < 2) {
      return;
    }
    if (autoPlayedFor.current === match.matchId) {
      return;
    }
    autoPlayedFor.current = match.matchId;
    setPositionState(0);
    setPlaying(true);
  }, [options.autoPlay, match, frames.length]);

  return {
    frames,
    position: clamped,
    current: frames[clamped] ?? null,
    playing,
    speed,
    stale: options.stale,
    setPosition,
    play,
    pause,
    toggle,
    step,
    toStart,
    toEnd,
    setSpeed,
    reset,
    canPlay: frames.length > 1 && playing,
    atStart: clamped <= 0,
    atEnd: clamped >= lastIndex && frames.length > 0,
  };
}