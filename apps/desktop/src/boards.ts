/**
 * Board geometry and cell access, in one place.
 *
 * # Why this module exists
 *
 * The backend stores a connect-four board **column-major**: the cell at column `c`, row `r` is at
 * index `c * rows + r`, with row 0 at the bottom. Every renderer that indexes that array has to get
 * this right, and a renderer that reads it *row-major* produces a correctly-shaped grid of the wrong
 * discs — which looks like a real position nobody played rather than like a bug.
 *
 * Concentrating every read here means there is exactly one expression of that rule in the frontend,
 * and it is the same rule the engine uses. `boardCell` is total: it returns `null` for a coordinate
 * that is off the board rather than clamping or returning `empty`, because an out-of-range read that
 * produced `empty` would be indistinguishable from a playable cell — the exact class of defect this
 * module is written to make impossible.
 *
 * # Two shapes, one accessor
 *
 * Tic-tac-toe is row-major and nine cells; connect four is column-major and `columns * rows`. They are
 * kept as distinct branches rather than unified behind an index formula, because a shared formula
 * would have to be conditional on the board anyway and would then be wrong for one of them.
 */

import type { ConnectFourCell, GameBoardView, GameCell } from "./api";

/** A game this build can draw. Anything else renders as an explicit unsupported state. */
export type DrawnGame = "tic_tac_toe" | "connect_four";

/** The geometry of a drawn board. */
export type BoardGeometry = {
  game: DrawnGame;
  columns: number;
  rows: number;
  /** Total cells, and `columns * rows`. */
  cells: number;
};

/** The three-by-three geometry. */
export const TICTACTOE: BoardGeometry = {
  game: "tic_tac_toe",
  columns: 3,
  rows: 3,
  cells: 9,
};

/** The seven-by-six geometry. */
export const CONNECT_FOUR: BoardGeometry = {
  game: "connect_four",
  columns: 7,
  rows: 6,
  cells: 42,
};

const GEOMETRY: Record<DrawnGame, BoardGeometry> = {
  tic_tac_toe: TICTACTOE,
  connect_four: CONNECT_FOUR,
};

/**
 * The geometry for a game identifier, or `null` for one this build cannot draw.
 *
 * Takes an identifier rather than a board so the panel can lay out from the catalog's dimensions and
 * from the recording and get the same answer both times.
 */
export function geometryFor(game: string): BoardGeometry | null {
  if (game === "tic_tac_toe") {
    return TICTACTOE;
  }
  if (game === "connect_four") {
    return CONNECT_FOUR;
  }
  return null;
}

/**
 * The geometry a board actually is, derived from the board itself.
 *
 * Returns `null` when the board is missing, is a game this build cannot draw, or has a cell count that
 * disagrees with its own dimensions. That last check matters: a truncated or over-long array would
 * otherwise index out of range or, worse, render a partial board that looks finished.
 */
export function geometryOf(board: GameBoardView | null | undefined): BoardGeometry | null {
  if (!board) {
    return null;
  }
  const known = geometryFor(board.game);
  if (!known) {
    return null;
  }
  if (board.game === "tic_tac_toe") {
    const cells = (board as { state?: { cells?: unknown } }).state?.cells;
    return Array.isArray(cells) && cells.length === TICTACTOE.cells ? known : null;
  }
  const state = (board as { state?: { columns?: unknown; rows?: unknown; cells?: unknown } }).state;
  const columns = typeof state?.columns === "number" ? state.columns : 0;
  const rows = typeof state?.rows === "number" ? state.rows : 0;
  if (columns !== known.columns || rows !== known.rows) {
    return null;
  }
  const cells = state?.cells;
  return Array.isArray(cells) && cells.length === columns * rows ? known : null;
}

/** The raw cell array, or `null` when the board is not one this build can draw. */
function cellArray(board: GameBoardView | null | undefined): GameCell[] | ConnectFourCell[] | null {
  if (!geometryOf(board)) {
    return null;
  }
  const cells = (board as { state?: { cells?: unknown } }).state?.cells;
  return Array.isArray(cells) ? (cells as GameCell[]) : null;
}

/**
 * The cell at a column and row.
 *
 * Row 0 is the **top** row for tic-tac-toe and the **bottom** row for connect four — the same
 * convention the core schema uses, and the one the prompt renderer and the engine both follow.
 * Returns `null` for anything off the board. Never clamps, never wraps, never guesses.
 */
export function boardCell(
  board: GameBoardView | null | undefined,
  column: number,
  row: number,
): GameCell | ConnectFourCell | null {
  const geometry = geometryOf(board);
  const cells = cellArray(board);
  if (!geometry || !cells) {
    return null;
  }
  if (column < 0 || column >= geometry.columns || row < 0 || row >= geometry.rows) {
    return null;
  }
  // The two indexing rules, stated once each.
  const index =
    geometry.game === "connect_four"
      ? // Column-major: the whole column is contiguous, so gravity is a stride of `rows`.
        column * geometry.rows + row
      : // Row-major, so a row is contiguous.
        row * geometry.columns + column;
  return cells[index] ?? null;
}

/** Every cell of a board, row-major from the top-left, or `null` when it cannot be drawn. */
export function boardCellsRowMajor(
  board: GameBoardView | null | undefined,
): GameCell[] | null {
  const geometry = geometryOf(board);
  if (!geometry) {
    return null;
  }
  const out: GameCell[] = [];
  for (let row = 0; row < geometry.rows; row += 1) {
    for (let column = 0; column < geometry.columns; column += 1) {
      out.push((boardCell(board, column, row) as GameCell | null) ?? "empty");
    }
  }
  return out;
}

/**
 * How many discs have settled in a column, counting from the bottom.
 *
 * This is the number a connect-four panel needs for gravity, falling discs, and the "is this column
 * full" affordance. It counts *consecutive* filled cells from the bottom rather than counting all of
 * them, so a recording with an inconsistent column cannot make the renderer draw a disc in mid-air.
 */
export function columnHeight(
  board: GameBoardView | null | undefined,
  column: number,
): number {
  const geometry = geometryOf(board);
  if (!geometry || column < 0 || column >= geometry.columns) {
    return 0;
  }
  let height = 0;
  while (height < geometry.rows && boardCell(board, column, height) !== "empty") {
    height += 1;
  }
  return height;
}

/** Whether a column can accept a disc. */
export function columnIsPlayable(board: GameBoardView | null | undefined, column: number): boolean {
  const geometry = geometryOf(board);
  if (!geometry) {
    return false;
  }
  return columnHeight(board, column) < geometry.rows;
}

/** The mark a side plays, as a cell value. */
export function sideMark(side: "first" | "second"): GameCell {
  return side === "first" ? "first" : "second";
}

/** The side a cell belongs to, or `null` for an empty cell. */
export function cellSide(cell: GameCell | ConnectFourCell | null): "first" | "second" | null {
  if (cell === "first" || cell === "second") {
    return cell;
  }
  return null;
}

/**
 * The two-character glyph for a cell, or a space for an empty one.
 *
 * Text rather than colour, so a board is readable without colour vision and survives a monochrome
 * screenshot. X and O are the marks both games use on the wire, which is what the recorded boards
 * actually contain; this maps them to glyphs a person reads faster.
 */
export function cellGlyph(cell: GameCell | ConnectFourCell | null): string {
  if (cell === "first") {
    return "X";
  }
  if (cell === "second") {
    return "O";
  }
  return "";
}

/**
 * The cells a winner completed, as `[column, row]` pairs, when they are on this board.
 *
 * The backend's own winning-line rule is authoritative and its coordinates are used directly. This
 * only *validates* them against the geometry, because a line whose coordinates fall off the board
 * would produce a highlight in the margin or an off-by-one on the wrong cell — which is worse than no
 * highlight, since it looks like a real result.
 */
export function winningCoordinates(
  board: GameBoardView | null | undefined,
  line: readonly [number, number][] | null | undefined,
  winner: "first" | "second" | null,
): [number, number][] {
  const geometry = geometryOf(board);
  if (!geometry || !line || line.length === 0 || !winner) {
    return [];
  }
  const mark = sideMark(winner);
  const onBoard: [number, number][] = [];
  for (const [column, row] of line) {
    if (column < 0 || column >= geometry.columns || row < 0 || row >= geometry.rows) {
      return [];
    }
    if (boardCell(board, column, row) !== mark) {
      // The engine said this cell is the winner's mark and it is not. Returning nothing is the honest
      // response: the alternative is highlighting cells that do not show what the engine decided.
      return [];
    }
    onBoard.push([column, row]);
  }
  return onBoard;
}

/**
 * The landing row for a disc dropped into a column, counting from the bottom.
 *
 * `null` when the column is full. The renderer uses this to animate a drop from above the board to
 * its resting place, and it is the same "one above the tallest stack" rule the engine applies.
 */
export function landingRow(board: GameBoardView | null | undefined, column: number): number | null {
  const geometry = geometryOf(board);
  if (!geometry || !columnIsPlayable(board, column)) {
    return null;
  }
  return columnHeight(board, column);
}
