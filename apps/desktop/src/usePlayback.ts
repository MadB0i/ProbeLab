/**
 * Frame-by-frame playback of a recorded arena match.
 *
 * # What this plays back, and what it does not
 *
 * It steps through `ArenaMatchView.history` — the boards the engine recorded, in order — and shows one
 * of them. It does **not** re-run the match, re-query a model, or recompute a move. There is no engine
 * in the browser and no provider, so playback cannot produce a result the recording did not already
 * contain.
 *
 * That guarantee is worth more now than it was. A match against a real model costs real provider time
 * and real tokens, so "open the recording" must never mean "pay for the same decisions again". The
 * frontend holds no way to do it: the only commands in this path are `get_arena_match` and
 * `replay_arena_match`, both of which read stored data.
 *
 * # Frames are boards, not cell arrays
 *
 * An earlier version reduced every frame to a flat cell array and only understood tic-tac-toe, which
 * made a connect-four recording unplayable. Frames now carry the board itself and are drawn through
 * `boards.ts`, which is the one place the column-major indexing rule lives.
 *
 * # The opening position is reconstructed, not invented
 *
 * The engine records the board *after* each move, so the position before any move was played has to be
 * derived. It is derived by clearing the cell the first ply filled — which is only sound because that
 * ply's move names the cell or column it changed. When it does not, no opening frame is produced and
 * the viewer starts at the first recorded position rather than showing a board nobody reached.
 *
 * For connect four, clearing a drop means clearing the *top* disc of the column, which is where a
 * dropped disc comes to rest. That is a different rule from tic-tac-toe's, and deriving it from the
 * game's own gravity rule is what keeps the opening frame correct for both.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { boardCell, columnHeight, geometryOf } from "./boards";
import type { ArenaMatchView, GameBoardView, PlyView } from "./api";

/** One stop on the timeline: a position, and the ply that produced it when there was one. */
export type PlaybackFrame = {
  /** Zero-based index into the frame list. */
  index: number;
  /** The board as of this frame. */
  board: GameBoardView;
  /** The ply this frame records, or `null` for the opening position. */
  ply: PlyView | null;
  /** The turn's own telemetry, when the backend recorded one. */
  turn: ArenaMatchView["turns"][number] | null;
};

/** The playback speeds offered. Milliseconds per frame. */
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

/** A copy of a board with one cell changed. Returns the original when the board is not drawable. */
function withCellCleared(board: GameBoardView, column: number, row: number): GameBoardView | null {
  const geometry = geometryOf(board);
  if (!geometry) {
    return null;
  }
  if (board.game === "tic_tac_toe") {
    const cells = [...((board as { state: { cells: string[] } }).state.cells)];
    cells[row * geometry.columns + column] = "empty";
    return { game: "tic_tac_toe", state: { cells } } as GameBoardView;
  }
  const state = (board as { state: { columns: number; rows: number; cells: string[] } }).state;
  const cells = [...state.cells];
  cells[column * state.rows + row] = "empty";
  return { game: "connect_four", state: { ...state, cells } } as GameBoardView;
}

/** A board of the right shape with every cell empty. */
function emptyBoard(game: string): GameBoardView | null {
  if (game === "tic_tac_toe") {
    return { game: "tic_tac_toe", state: { cells: new Array(9).fill("empty") } } as GameBoardView;
  }
  if (game === "connect_four") {
    return {
      game: "connect_four",
      state: { columns: 7, rows: 6, cells: new Array(42).fill("empty") },
    } as GameBoardView;
  }
  return null;
}

/** A structural fingerprint of a board, used only to detect that two frames are the same position. */
function fingerprint(board: GameBoardView): string {
  const cells = (board as { state?: { cells?: unknown } }).state?.cells;
  return Array.isArray(cells) ? cells.join(",") : "unreadable";
}

/**
 * The frames a recording can be stepped through.
 *
 * Exported because the panel and the tests both need the same list, and a panel that built its own
 * would be free to disagree with the stepper about how many frames there are.
 *
 * Three kinds of frame, in order: the opening position, one per recorded ply, and the terminal
 * position the match ended on. The last is appended only when it differs from the final ply, so a
 * recording that captured the terminal board as its own ply does not get a duplicated frame and a
 * recording that did not still reaches the position the match actually ended on.
 */
export function playbackFrames(match: ArenaMatchView | null): PlaybackFrame[] {
  if (!match) {
    return [];
  }
  const finalBoard = match.finalBoard;
  if (!geometryOf(finalBoard)) {
    return [];
  }
  const recorded = match.history;
  if (recorded.length === 0) {
    /*
     * A match that recorded no plies still has one position to show, and the honest one is whatever
     * board the recording actually holds.
     *
     * Falling back to an empty board here would be a small lie in the other direction: a recording
     * that captured a board — because the match failed on its first ply, or was cancelled — has a
     * real position, and replacing it with the opening would show a board the engine never reached at
     * the moment the recording was stored. Only when there is no board at all does an empty one stand
     * in, and then it is a real state rather than a guess.
     */
    return [{ index: 0, board: finalBoard, ply: null, turn: match.turns[0] ?? null }];
  }
  const frames: PlaybackFrame[] = [];
  const opening = emptyOpening(match);
  if (opening) {
    frames.push({ index: 0, board: opening, ply: null, turn: null });
  }
  for (const [offset, ply] of recorded.entries()) {
    if (!geometryOf(ply.board)) {
      // A frame the panel cannot draw is not stepped through silently: it is skipped, and the frame
      // list stays shorter than `history.length` rather than rendering a blank board.
      continue;
    }
    frames.push({
      index: frames.length,
      board: ply.board,
      ply,
      turn: match.turns[offset] ?? null,
    });
  }
  const lastDrawn = frames[frames.length - 1];
  if (!lastDrawn || fingerprint(lastDrawn.board) !== fingerprint(finalBoard)) {
    frames.push({
      index: frames.length,
      board: finalBoard,
      ply: lastDrawn?.ply ?? null,
      turn: match.turns[match.turns.length - 1] ?? null,
    });
  }
  return frames.map((frame, index) => ({ ...frame, index }));
}

/**
 * The opening position, rebuilt from the first recorded ply.
 *
 * Tic-tac-toe: clear the cell the move named. Connect four: clear the top disc of the column the move
 * named, which is where a drop comes to rest — the same rule the engine's gravity applies, read from
 * `boards.ts` so the two cannot disagree.
 */
function emptyOpening(match: ArenaMatchView): GameBoardView | null {
  const first = match.history[0];
  if (!first) {
    return emptyBoard(match.game);
  }
  if (!geometryOf(first.board)) {
    return null;
  }
  const move = (first.played as { move?: unknown } | null)?.move;
  if (typeof move !== "number") {
    return null;
  }
  if (match.game === "connect_four") {
    // The landing row is the one above the column's current stack. The board already holds the drop, so
    // "the stack" is the height including that disc.
    const height = columnHeight(first.board, move);
    if (height === 0) {
      return null;
    }
    return withCellCleared(first.board, move, height - 1);
  }
  if (move < 0 || move > 8) {
    return null;
  }
  const geometry = geometryOf(first.board);
  if (!geometry) {
    return null;
  }
  const column = move % geometry.columns;
  const row = Math.floor(move / geometry.columns);
  return withCellCleared(first.board, column, row);
}

/** The cell a move played into, or `null` when the move names no cell this board can draw. */
export function movedCell(
  board: GameBoardView | null,
  played: PlyView["played"],
): { column: number; row: number } | null {
  const move = (played as { move?: unknown } | null)?.move;
  if (typeof move !== "number") {
    return null;
  }
  const geometry = geometryOf(board);
  if (!geometry) {
    return null;
  }
  if (board?.game === "connect_four") {
    const row = columnHeight(board, move) - 1;
    return row >= 0 ? { column: move, row } : null;
  }
  if (move < 0 || move >= geometry.cells) {
    return null;
  }
  return { column: move % geometry.columns, row: Math.floor(move / geometry.columns) };
}

/** The last cell played on a board, read from the frame's own move rather than by diffing. */
export function lastMoveCell(
  board: GameBoardView | null,
  played: PlyView["played"],
): { column: number; row: number } | null {
  return movedCell(board, played);
}

export type PlaybackState = {
  frames: PlaybackFrame[];
  position: number;
  current: PlaybackFrame | null;
  playing: boolean;
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
   * running would keep advancing a frame index that no longer exists.
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
    if (delay === null || clamped >= lastIndex) {
      if (clamped >= lastIndex) {
        setPlaying(false);
      }
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

/** Whether a frame's board shows a cell, used by the panel's own emptiness checks. */
export function frameHasDiscs(board: GameBoardView | null): boolean {
  const geometry = geometryOf(board);
  if (!geometry) {
    return false;
  }
  for (let row = 0; row < geometry.rows; row += 1) {
    for (let column = 0; column < geometry.columns; column += 1) {
      if (boardCell(board, column, row) !== "empty") {
        return true;
      }
    }
  }
  return false;
}
