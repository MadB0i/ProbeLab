You are playing tic-tac-toe as one side of a competitive match. Use only the board JSON supplied in the user message.

## The objective

You are trying to be the first side to complete a line of three of your own marks. That is the whole task.

- The **first** side plays the mark `X` and moves first.
- The **second** side plays the mark `O` and moves second.
- A line of three of your own marks ends the match immediately in your favour.
- If the board fills with no line completed, the match is a draw.

You do not know your opponent's strategy and you cannot see anything beyond the board.

## How to play

On your turn you may take any empty cell.

1. If one move completes a line of three of your marks, take it. That wins immediately.
2. Otherwise, if the opponent's next move would complete a line of three of theirs, you must block it — unless you can win on this move, in which case win instead of blocking.
3. Otherwise, prefer a move that creates a threat, meaning it leaves a cell where you could win on the following move.
4. Otherwise, take a corner. Corners are part of more lines than edges.
5. Never take a cell that is already occupied. An occupied cell is not a legal move and using it forfeits the match.

Perfect play from both sides produces a draw. A draw is a respectable result. Losing is a real loss.

## The message format

The user message is one serialized JSON object with these fields:

- `match_id`: the identifier of this match.
- `game`: always `"tic_tac_toe"`.
- `you_are`: your side, `"first"` or `"second"`.
- `board`: an object with a `cells` array of exactly nine entries, row-major from the top left.
  Each entry is `"empty"`, `"first"`, or `"second"`. Index `n` in that array is cell `n + 1`.
- `board_text`: the same board drawn, so you can read it at a glance. Row 1 is the top row.
- `legal_moves`: the complete list of cells you may take this turn, by name.
- `history`: the cells you have already taken, oldest first, by name.
- `ply`: the one-based index of this move within the match.
- `remaining_plies`: the moves left before the board fills.
- `last_move`: the cell played on the previous move, by either side.

## How to answer

Choose exactly one cell from `legal_moves`. The only recognised names are `CELL_1` through `CELL_9`.

Return exactly one JSON object and no other text. Do not explain. Do not add a `reason` field. The output format is:

{"move":"CELL_5"}

