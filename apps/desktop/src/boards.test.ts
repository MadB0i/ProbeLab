/**
 * Board geometry and cell access.
 *
 * The column-major rule is the whole reason this module exists, so the tests are mostly about that one
 * fact plus the total behaviour of `boardCell`: an off-board read returns `null`, never a clamped or
 * defaulted cell. A `boardCell` that returned `"empty"` for an out-of-range coordinate would make a
 * bug render as a playable square, which is the one class of error a board cannot hide.
 */

import { describe, expect, it } from "vitest";

import {
  CONNECT_FOUR,
  TICTACTOE,
  boardCell,
  boardCellsRowMajor,
  cellGlyph,
  cellSide,
  columnHeight,
  columnIsPlayable,
  geometryFor,
  geometryOf,
  landingRow,
  sideMark,
  winningCoordinates,
} from "./boards";
import type { GameBoardView } from "./api";

/** A connect-four board from a row-major picture, so a test reads like the board it describes. */
function connectFour(picture: string[] | string[][]): GameBoardView {
  const rows = picture.length;
  const columns = picture[0].length;
  const cells = new Array(columns * rows).fill("empty");
  for (let row = 0; row < rows; row += 1) {
    for (let column = 0; column < columns; column += 1) {
      const glyph = picture[row][column];
      if (glyph === "X") {
        cells[column * rows + row] = "first";
      } else if (glyph === "O") {
        cells[column * rows + row] = "second";
      }
    }
  }
  return {
    game: "connect_four",
    state: { columns, rows, cells },
  } as GameBoardView;
}

describe("board geometry", () => {
  it("knows the two games it can draw and refuses the rest", () => {
    expect(geometryFor("tic_tac_toe")).toEqual(TICTACTOE);
    expect(geometryFor("connect_four")).toEqual(CONNECT_FOUR);
    expect(geometryFor("battleship")).toBeNull();
    expect(geometryFor("not-a-game")).toBeNull();
  });

  it("refuses a connect-four board whose cell count disagrees with its dimensions", () => {
    // A truncated array would otherwise index out of range, or render a partial board that looks
    // finished. Both are worse than an explicit "cannot draw".
    const truncated = {
      game: "connect_four",
      state: { columns: 7, rows: 6, cells: new Array(41).fill("empty") },
    } as unknown as GameBoardView;
    expect(geometryOf(truncated)).toBeNull();

    const wrongDimensions = {
      game: "connect_four",
      state: { columns: 8, rows: 8, cells: new Array(64).fill("empty") },
    } as unknown as GameBoardView;
    expect(geometryOf(wrongDimensions)).toBeNull();
  });

  it("refuses a tic-tac-toe board that is not nine cells", () => {
    expect(
      geometryOf({
        game: "tic_tac_toe",
        state: { cells: new Array(8).fill("empty") },
      } as unknown as GameBoardView),
    ).toBeNull();
  });
});

describe("tic-tac-toe cell access", () => {
  const board = {
    game: "tic_tac_toe",
    state: {
      cells: ["first", "second", "empty", "empty", "first", "empty", "second", "empty", "empty"],
    },
  } as GameBoardView;

  it("reads row-major from the top left", () => {
    expect(boardCell(board, 0, 0)).toBe("first");
    expect(boardCell(board, 1, 0)).toBe("second");
    expect(boardCell(board, 0, 1)).toBe("empty");
    expect(boardCell(board, 2, 2)).toBe("empty");
  });

  it("returns null rather than a cell for anything off the board", () => {
    expect(boardCell(board, -1, 0)).toBeNull();
    expect(boardCell(board, 3, 0)).toBeNull();
    expect(boardCell(board, 0, -1)).toBeNull();
    expect(boardCell(board, 0, 3)).toBeNull();
  });
});

/**
 * A real seven-by-six board, written as it looks on screen.
 *
 * `picture[0]` is the **bottom** row, matching the core schema and the engine. Every fixture here is
 * the full 7x6 shape, because `geometryOf` refuses any other size on purpose — a board whose
 * dimensions disagree with its cell count is not a connect-four board, and accepting one would let a
 * malformed recording render as a plausible one.
 */
const BLANK: string[] = [
  ".......",
  ".......",
  ".......",
  ".......",
  ".......",
  ".......",
];

/**
 * The blank board with the given rows replaced, counted from the bottom.
 *
 * Rows may be written short — `"O.."` means columns 0, 1 and 2 — and are padded with empty cells, so
 * a fixture can name only the columns a test cares about. A row that is *not* padded would make the
 * array shorter than the board claims, which `geometryOf` refuses; padding here keeps the fixtures
 * readable without weakening that check.
 */
function withRows(rows: Record<number, string>): GameBoardView {
  const picture = BLANK.map((row) => row.split(""));
  for (const [fromBottom, text] of Object.entries(rows)) {
    const padded = text.padEnd(7, ".").slice(0, 7);
    picture[Number(fromBottom)] = padded.split("");
  }
  return connectFour(picture);
}

describe("connect-four cell access", () => {
  /*
   * A physically valid position: every column is a stack from the bottom with no gap in it, because
   * that is the only shape gravity can produce.
   *
   *   bottom   O X O . . . .
   *   row 2    . . . . . . .
   *   row 1    X . O . . . .
   *   row 0    . . . . . . .
   */
  const board = withRows({ 0: "OXO....", 1: "X.O...." });

  it("reads column-major with row 0 at the bottom", () => {
    // Column 0: an O on the bottom row, an X above it, nothing above that.
    expect(boardCell(board, 0, 0)).toBe("second");
    expect(boardCell(board, 0, 1)).toBe("first");
    expect(boardCell(board, 0, 2)).toBe("empty");
    // Column 1: one disc on the bottom row.
    expect(boardCell(board, 1, 0)).toBe("first");
    expect(boardCell(board, 1, 1)).toBe("empty");
    // Column 2: two discs from the bottom.
    expect(boardCell(board, 2, 0)).toBe("second");
    expect(boardCell(board, 2, 1)).toBe("second");
    expect(boardCell(board, 2, 2)).toBe("empty");
    // The far columns are empty all the way up.
    expect(boardCell(board, 6, 0)).toBe("empty");
    expect(boardCell(board, 6, 5)).toBe("empty");
  });

  it("returns null rather than a cell for a column or row off the board", () => {
    expect(boardCell(board, 7, 0)).toBeNull();
    expect(boardCell(board, -1, 0)).toBeNull();
    expect(boardCell(board, 0, 6)).toBeNull();
    expect(boardCell(board, 0, -1)).toBeNull();
  });

  it("counts a column's height from the bottom", () => {
    expect(columnHeight(board, 0)).toBe(2);
    expect(columnHeight(board, 1)).toBe(1);
    expect(columnHeight(board, 2)).toBe(2);
    expect(columnHeight(board, 3)).toBe(0);
    expect(columnHeight(board, 7)).toBe(0);
  });

  it("reports a full column as unplayable and its landing row as absent", () => {
    const full = withRows({ 0: "O", 1: "X", 2: "O", 3: "X", 4: "O", 5: "X" });
    expect(columnHeight(full, 0)).toBe(6);
    expect(columnIsPlayable(full, 0)).toBe(false);
    expect(landingRow(full, 0)).toBeNull();
    expect(columnIsPlayable(full, 1)).toBe(true);
    expect(landingRow(full, 1)).toBe(0);
  });

  it("places the next disc one above the stack", () => {
    expect(landingRow(board, 0)).toBe(2);
    expect(landingRow(board, 1)).toBe(1);
    expect(landingRow(board, 3)).toBe(0);
  });

  it("renders row-major for a reader, which is the opposite index order on purpose", () => {
    // This is the conversion a naive renderer gets wrong: read row-major, the bottom row of this board
    // is O X O . . . . — and read column-major it is the same here by coincidence of the fixture, so
    // the test pins the *shape* of the output rather than a lucky coincidence.
    const cells = boardCellsRowMajor(board) as string[];
    expect(cells).toHaveLength(42);
    // Bottom row.
    expect(cells.slice(0, 7)).toEqual(["second", "first", "second", "empty", "empty", "empty", "empty"]);
    // Row one.
    expect(cells.slice(7, 14)).toEqual(["first", "empty", "second", "empty", "empty", "empty", "empty"]);
    // Rows two through five are untouched.
    expect(cells.slice(14)).toEqual(new Array(28).fill("empty"));
  });

  it("orders a column-major board differently from a row-major one, which is the bug it prevents", () => {
    /*
     * A staircase, which gravity can actually produce: each column's stack is one shorter than the
     * one to its left, so every column is filled from the bottom with no gap.
     *
     *   bottom   . X X X . . .
     *   row 2    . . . X . . .
     *   row 1    . . X X . . .
     */
    const staircase = withRows({ 0: ".XXX...", 1: "..XX...", 2: "...X..." });
    // Heights are the defining fact of this position, and they are only readable column-major.
    expect(columnHeight(staircase, 0)).toBe(0);
    expect(columnHeight(staircase, 1)).toBe(1);
    expect(columnHeight(staircase, 2)).toBe(2);
    expect(columnHeight(staircase, 3)).toBe(3);
    // Column 3's bottom cell is a disc; column 0's is empty.
    expect(boardCell(staircase, 3, 0)).toBe("first");
    expect(boardCell(staircase, 0, 0)).toBe("empty");
    // Read row-major, the three discs of the staircase's tallest column appear as one cell on row 0
    // and one on row 2 — a different position entirely, and the exact shape of the indexing bug.
    const rowMajor = boardCellsRowMajor(staircase) as string[];
    expect(rowMajor.slice(0, 7).filter((cell) => cell !== "empty")).toHaveLength(3);
    expect(rowMajor.slice(7, 14).filter((cell) => cell !== "empty")).toHaveLength(2);
    expect(rowMajor.slice(14, 21).filter((cell) => cell !== "empty")).toHaveLength(1);
  });
});

describe("winning coordinates", () => {
  /*
   * Four first-side discs along the bottom of columns 0 through 3, and four more up column 3.
   *
   *   bottom   X X X X . . .
   *   row 2    . . . X . . .
   *   row 1    . . . X . . .
   *   row 0    . . . X . . .
   */
  const board = withRows({ 0: "XXXX...", 1: "...X...", 2: "...X...", 3: "...X..." });

  it("accepts a horizontal line the engine reported", () => {
    const line = winningCoordinates(board, [[0, 0], [1, 0], [2, 0], [3, 0]], "first");
    expect(line).toHaveLength(4);
  });

  it("accepts a vertical line the engine reported", () => {
    // The other shape a connect-four win takes. Both must render, because a renderer that only handled
    // one would show a real win as no win at all.
    const line = winningCoordinates(board, [[3, 0], [3, 1], [3, 2]], "first");
    expect(line).toHaveLength(3);
  });

  it("rejects a line whose cells do not hold the winner's mark", () => {
    // The backend and the renderer must not disagree about who won. If they do, showing nothing is the
    // only honest option: highlighting the reported cells would draw a line nobody completed.
    expect(winningCoordinates(board, [[0, 1], [1, 1]], "second")).toEqual([]);
    expect(winningCoordinates(board, [[0, 0], [1, 0], [2, 0], [3, 0]], "second")).toEqual([]);
  });

  it("rejects a line that falls off the board", () => {
    expect(winningCoordinates(board, [[0, 0], [9, 0]], "first")).toEqual([]);
    expect(winningCoordinates(board, [[0, 99]], "first")).toEqual([]);
    expect(winningCoordinates(board, [[-1, 0]], "first")).toEqual([]);
  });

  it("shows nothing for a draw, a missing line, or no winner", () => {
    expect(winningCoordinates(board, null, "first")).toEqual([]);
    expect(winningCoordinates(board, [], "first")).toEqual([]);
    expect(winningCoordinates(board, [[0, 0], [1, 0], [2, 0], [3, 0]], null)).toEqual([]);
  });
});

describe("marks and glyphs", () => {
  it("gives each side a stable mark and a readable glyph", () => {
    expect(sideMark("first")).toBe("first");
    expect(sideMark("second")).toBe("second");
    expect(cellGlyph("first")).toBe("X");
    expect(cellGlyph("second")).toBe("O");
    expect(cellGlyph("empty")).toBe("");
    expect(cellGlyph(null)).toBe("");
  });

  it("reports an empty cell as belonging to no side", () => {
    expect(cellSide("empty")).toBeNull();
    expect(cellSide(null)).toBeNull();
    expect(cellSide("first")).toBe("first");
  });
});
