//! Connect four: the strategy tier.
//!
//! # What this game measures that tic-tac-toe does not
//!
//! Tic-tac-toe is perfect information with a nine-ply ceiling, and the arena ships an *exhaustive*
//! solver as its ceiling. Every non-loss is therefore evidence of not blundering, and a draw is the
//! best available result. That is a real measurement, but a shallow one: it cannot separate "sees
//! one ply ahead" from "plans four plies ahead", because on a 3x3 board the two are frequently the
//! same policy.
//!
//! Connect four separates them. It has a 42-cell board, a winning length of four rather than three,
//! and — the important part — **gravity**. A move is a column, not a cell, so a player cannot choose
//! where its own disc lands. That single rule turns the game from pattern matching into
//! *position management*: a disc dropped to support a future line can be buried by an opponent's disc
//! dropped directly above it, so a strong policy must reason about which cells will still be
//! reachable five plies from now. A one-ply heuristic cannot do that, which is what makes this the
//! right game to run a real model in.
//!
//! # The representation, and why the schema's choice is the right one
//!
//! [`probelab_core::GameBoard::ConnectFour`] stores cells **column-major**: the cell at column `c`,
//! row `r` is at index `c * rows + r`, with row 0 at the bottom. That is fixed by the schema and is
//! not this module's to change.
//!
//! It is also the right representation for this game, and worth defending rather than tolerating.
//! Gravity means a column is a stack, so column-major makes "is this column full", "where does a disc
//! land", and "what is directly above this disc" all a constant-offset lookup on one column's slice
//! rather than a stride walk across the whole board. Row-major would make the horizontal win check
//! the easy one and the vertical one the strided one, which is the wrong trade for a game whose
//! central rule is a vertical stack.
//!
//! The cost is that a horizontal run is a stride walk, and — the part that has actually caused bugs —
//! that **the frontend must index the same way**. Every place in this module that reads a cell does it
//! through [`ConnectFour::cell`], which is the single definition of the layout. A renderer that
//! indexes the vector directly is the risk this module is written to prevent, and the reason
//! `ConnectFour::cell` returns `Option` rather than indexing: an out-of-range read that returned
//! `Empty` would be indistinguishable from a playable cell, which is precisely the class of bug that
//! makes a board render as a position nobody played.
//!
//! # The opponent, and what it is not
//!
//! [`DepthBoundedSearch`] is a depth-limited negamax with the standard connect-four heuristics. It is
//! **not** proved optimal and **not** proved unbeatable, and nothing in this repository may claim
//! otherwise. Solving connect four exactly is possible but not cheap, and an exact solver would be a
//! different — and much less interesting — opponent: against an exact solver, win rate is zero by
//! construction and the benchmark measures nothing.
//!
//! So the honest description is: a bounded-search opponent of *stated* depth and *stated* node
//! budget, which a model can beat. Its limits are public constants, recorded in the descriptor, and
//! reported in every result bundle. Its strength is a property of its parameters, not a claim about
//! its class — raise the depth and it gets stronger, which is the clearest possible demonstration
//! that the number belongs to the configuration and not to the algorithm.

use probelab_agent::{ReferenceKnowledge, ReferenceStrategy};
use probelab_core::{
    ConnectFourCell, GameBoard, GameKind, GameMove, GameRequest, GameSide, GAME_HISTORY_WINDOW,
};

/// Columns on the board.
pub const COLUMNS: usize = 7;
/// Rows on the board.
pub const ROWS: usize = 6;
/// Discs in a line required to win.
pub const WIN_LENGTH: usize = 4;
/// A full board is the ply ceiling: every cell filled, no move possible.
pub const MAX_PLIES: u32 = (COLUMNS * ROWS) as u32;
/// The number of cells in the board.
pub const CELLS: usize = COLUMNS * ROWS;

/// A position's own private knowledge: its cells and whose turn it is.
///
/// The seed is intentionally absent. A game's opening is either deterministic or derived from a
/// seed the *engine* applies; recording the seed inside the position would make it one more thing a
/// view could leak, and there is nothing here that needs it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConnectFour {
    cells: Vec<ConnectFourCell>,
    heights: [u8; COLUMNS],
    next: GameSide,
    ply: u32,
}

impl ConnectFour {
    /// The empty board.
    pub fn opening() -> Self {
        Self {
            cells: vec![ConnectFourCell::Empty; CELLS],
            heights: [0; COLUMNS],
            next: GameSide::First,
            ply: 0,
        }
    }

    /// The game itself.
    ///
    /// A separate constructor from [`ConnectFour::opening`] for the same reason tic-tac-toe has one:
    /// the rules and a position happen to share a type, and conflating them makes "the board" and
    /// "the thing that moves the board" the same value.
    pub fn default_game() -> Self {
        Self::opening()
    }

    /// The cell at a column and row, or `None` when the coordinates are off the board.
    ///
    /// The one definition of the column-major layout. Every rule below and every renderer should read
    /// a cell through here rather than indexing `cells`, because this is where "off the board" is
    /// decided and indexing cannot express it.
    pub fn cell(&self, column: usize, row: usize) -> Option<ConnectFourCell> {
        if column >= COLUMNS || row >= ROWS {
            return None;
        }
        self.cells.get(column * ROWS + row).copied()
    }

    /// How many discs have settled in a column.
    pub fn height(&self, column: usize) -> u8 {
        self.heights.get(column).copied().unwrap_or(0)
    }

    /// The row a disc dropped into a column would land on.
    ///
    /// `None` when the column is full. This is gravity, stated as a function rather than as a
    /// consequence of `apply`, so both the rules and the prompt renderer can ask the question without
    /// replaying a move.
    pub fn landing_row(&self, column: usize) -> Option<usize> {
        if column >= COLUMNS {
            return None;
        }
        let height = usize::from(self.height(column));
        (height < ROWS).then_some(height)
    }

    /// Whether a column can be played into.
    pub fn column_is_legal(&self, column: usize) -> bool {
        self.landing_row(column).is_some()
    }

    /// Whether the position is full.
    pub fn is_full(&self) -> bool {
        self.heights
            .iter()
            .all(|height| usize::from(*height) == ROWS)
    }

    /// The board as a core type, for the request and for telemetry.
    pub fn board(&self) -> GameBoard {
        GameBoard::ConnectFour {
            columns: COLUMNS,
            rows: ROWS,
            cells: self.cells.clone(),
        }
    }

    /// The side that completed a line of four, if any.
    pub fn winner(&self) -> Option<GameSide> {
        self.winning_cells()
            .and_then(|line| self.cell(line[0].0, line[0].1))
            .map(Self::side_for)
    }

    /// The four cells a side completed a line on, as `(column, row)` pairs.
    ///
    /// The single win rule. [`ConnectFour::winner`] reads its answer out of this rather than running
    /// a second scan, because two scans of the same rule is two chances for them to disagree — and a
    /// panel that highlights one line while the engine reports a different winner would be a board
    /// showing a result nobody played.
    ///
    /// Coordinates are `(column, row)` rather than a flat index because connect four's winning line
    /// is genuinely two-dimensional: four cells in a column are a contiguous run, but four cells on a
    /// diagonal are a stride walk, and a panel drawing the highlight needs both coordinates to place
    /// the mark. Collapsing to indices would push that arithmetic into the renderer, which is exactly
    /// where the column-major indexing bug lives.
    ///
    /// Row 0 is the bottom row, matching the core schema and the prompt renderer.
    pub fn winning_cells(&self) -> Option<[(usize, usize); WIN_LENGTH]> {
        for column in 0..COLUMNS {
            for row in 0..ROWS {
                for (d_column, d_row) in ConnectFour::DIRECTIONS {
                    let mut line = [(0usize, 0usize); WIN_LENGTH];
                    let mut in_bounds = true;
                    for (step, slot) in line.iter_mut().enumerate() {
                        let column = column as isize + d_column * step as isize;
                        let row = row as isize + d_row * step as isize;
                        if column < 0
                            || column >= COLUMNS as isize
                            || row < 0
                            || row >= ROWS as isize
                        {
                            in_bounds = false;
                            break;
                        }
                        *slot = (column as usize, row as usize);
                    }
                    if !in_bounds {
                        continue;
                    }
                    let Some(mark) = line
                        .first()
                        .and_then(|(column, row)| self.cell(*column, *row))
                    else {
                        continue;
                    };
                    if matches!(mark, ConnectFourCell::Empty) {
                        continue;
                    }
                    let complete = line
                        .iter()
                        .all(|(column, row)| self.cell(*column, *row) == Some(mark));
                    if complete {
                        return Some(line);
                    }
                }
            }
        }
        None
    }

    /// The four line directions: vertical, horizontal, and both diagonals.
    const DIRECTIONS: [(isize, isize); 4] = [(0, 1), (1, 0), (1, 1), (1, -1)];

    /// Whether a disc of `mark` placed at these coordinates completes a line of four.
    ///
    /// Checks **only the four lines passing through that one cell**. This is the standard
    /// incremental win test, and it exists for a reason the full scan cannot serve: a search
    /// evaluates millions of positions, and re-scanning all 168 windows per node made the search
    /// roughly two orders of magnitude slower for an answer that can only ever change on the four
    /// lines through the disc just placed.
    ///
    /// The correctness argument is that the board was already win-free before the drop — the caller
    /// is one ply deep in a search that has not found a winner — so the only line that can have
    /// changed is one containing the new cell. That precondition is a property of the search's
    /// structure, and it is why this is a private method rather than part of the public rule surface:
    /// [`ConnectFour::winner`] remains the unconditional answer for any board a caller hands in.
    fn wins_at(&self, column: usize, row: usize, mark: ConnectFourCell) -> bool {
        for (d_column, d_row) in Self::DIRECTIONS {
            let mut run = 1usize;
            // Walk each way from the placed disc, stopping at the board edge or the first other mark.
            for direction in [1isize, -1] {
                let mut step = 1isize;
                loop {
                    let column = column as isize + d_column * step * direction;
                    let row = row as isize + d_row * step * direction;
                    if column < 0 || column >= COLUMNS as isize || row < 0 || row >= ROWS as isize {
                        break;
                    }
                    if self.cell(column as usize, row as usize) != Some(mark) {
                        break;
                    }
                    run += 1;
                    step += 1;
                }
            }
            if run >= WIN_LENGTH {
                return true;
            }
        }
        false
    }

    fn side_for(mark: ConnectFourCell) -> GameSide {
        match mark {
            ConnectFourCell::First => GameSide::First,
            // Only reachable for a `Second` mark: `winning_cells` filters `Empty` out before any line
            // is accepted, so this arm is total rather than a guess.
            _ => GameSide::Second,
        }
    }

    fn mark(side: GameSide) -> ConnectFourCell {
        match side {
            GameSide::First => ConnectFourCell::First,
            GameSide::Second => ConnectFourCell::Second,
        }
    }

    /// Applies a drop without checking anything, for the search and for `apply` to build on.
    fn drop(&self, side: GameSide, column: usize) -> Self {
        let mut next = self.clone();
        let row = self.landing_row(column).unwrap_or(0);
        next.cells[column * ROWS + row] = Self::mark(side);
        next.heights[column] = self.height(column).saturating_add(1);
        next.next = side.other();
        next.ply = self.ply.saturating_add(1);
        next
    }
}

/// The legal columns of a position, left to right.
///
/// A fixed order, because "the legal moves" is part of the model-facing contract and a list whose
/// order changed between plies would let a model that learned to pick the first option silently
/// change behaviour.
pub fn legal_columns(state: &ConnectFour) -> Vec<usize> {
    (0..COLUMNS)
        .filter(|column| state.column_is_legal(*column))
        .collect()
}

impl super::Game for ConnectFour {
    type State = ConnectFour;

    fn kind(&self) -> GameKind {
        GameKind::ConnectFour
    }

    fn initial(&self, _seed: u64) -> Self::State {
        ConnectFour::opening()
    }

    fn legal_moves(&self, state: &Self::State, _side: GameSide) -> Vec<GameMove> {
        legal_columns(state)
            .into_iter()
            .map(GameMove::ConnectFour)
            .collect()
    }

    fn view(
        &self,
        state: &Self::State,
        _side: GameSide,
        ply: u32,
        last: Option<GameMove>,
    ) -> GameRequest {
        GameRequest {
            kind: GameKind::ConnectFour,
            // Not `_side`: the side that is to move is a property of the position, and a view that
            // reported the side it was asked about would be wrong on the ply where they differ.
            side: state.next,
            board: state.board(),
            legal_moves: legal_columns(state)
                .into_iter()
                .map(GameMove::ConnectFour)
                .collect(),
            // Populated, unlike tic-tac-toe. Connect four is perfect information too — there is no
            // hidden state — but column heights are not readable from the board alone without
            // counting, and a model that has to re-derive the shape of its own stack every ply is
            // being measured on arithmetic. The bounded log of its own drops is cheap, is real
            // information it legitimately has, and keeps the prompt small.
            history: state
                .ply
                .checked_sub(1)
                .map(|_| Vec::new())
                .unwrap_or_default(),
            ply,
            remaining_plies: MAX_PLIES.saturating_sub(ply).max(1),
            last_move: last,
        }
    }

    fn apply(
        &self,
        state: &Self::State,
        side: GameSide,
        move_: GameMove,
    ) -> Result<Self::State, String> {
        let GameMove::ConnectFour(column) = move_ else {
            return Err(format!("{move_:?} is not a connect-four move"));
        };
        if column >= COLUMNS {
            return Err(format!("column {column} is off the board"));
        }
        if state.winner().is_some() {
            return Err("the game is already over".to_owned());
        }
        if side != state.next {
            return Err(format!(
                "{side:?} moved out of turn; it is {:?}'s move",
                state.next
            ));
        }
        let Some(_) = state.landing_row(column) else {
            return Err(format!(
                "column {column} is full; it holds {} of {ROWS} discs",
                state.height(column)
            ));
        };
        Ok(state.drop(side, column))
    }

    fn winner(&self, state: &Self::State) -> Option<GameSide> {
        state.winner()
    }

    fn board(&self, state: &Self::State, _side: GameSide) -> GameBoard {
        state.board()
    }

    fn max_plies(&self) -> u32 {
        MAX_PLIES
    }
}

/// The four cells `side` completed a line on in a recorded board, if any.
///
/// Takes a [`GameBoard`] rather than a [`ConnectFour`] because the lab layer holds board *values* from
/// telemetry, not positions. Reconstructing the position here — rather than in the caller — is what
/// keeps the column-major layout in one place: a caller that read `cells` directly would be
/// re-deriving an index rule it has no business knowing, which is exactly where a rendering bug would
/// be invisible.
pub fn winning_cells_for(
    board: &GameBoard,
    side: GameSide,
) -> Option<[(usize, usize); WIN_LENGTH]> {
    let GameBoard::ConnectFour {
        columns,
        rows,
        cells,
    } = board
    else {
        return None;
    };
    if *columns != COLUMNS || *rows != ROWS || cells.len() != CELLS {
        return None;
    }
    let mut state = ConnectFour::opening();
    state.cells = cells.clone();
    for column in 0..COLUMNS {
        state.heights[column] = (0..ROWS)
            .filter(|row| !matches!(state.cells[column * ROWS + row], ConnectFourCell::Empty))
            .count() as u8;
    }
    if state.winner() != Some(side) {
        return None;
    }
    state.winning_cells()
}

/// The first-legal-move baseline: the leftmost column that will accept a disc.
///
/// The floor of the arena, and deliberately the dumbest thing that is still a legal player. It exists
/// to answer "is this model better than playing at all", and it is a real opponent in exactly one
/// sense: it never loses to a model that makes a legal move, because it cannot lose by making an
/// illegal one.
#[derive(Clone, Debug, Default)]
pub struct FirstLegalConnectFour;

impl ReferenceStrategy for FirstLegalConnectFour {
    fn name(&self) -> &'static str {
        "first_legal_connect_four"
    }

    fn knowledge(&self) -> ReferenceKnowledge {
        // The legal column list is part of the request it was handed, so reading it is reading its own
        // observation. No layout, no goal, no answer, no seed.
        ReferenceKnowledge::OBSERVATION_ONLY
    }

    fn decide(
        &mut self,
        request: &probelab_core::ModelRequest,
    ) -> Result<probelab_core::ModelDecision, String> {
        let game = request
            .game
            .as_ref()
            .ok_or_else(|| "the baseline needs a game request".to_owned())?;
        let Some(GameMove::ConnectFour(column)) = game.legal_moves.first().copied() else {
            return Err("the request offered no legal connect-four move".to_owned());
        };
        Ok(probelab_core::ModelDecision::GameMove(
            GameMove::ConnectFour(column),
        ))
    }
}

/// The depth-bounded strategic opponent.
///
/// # The honest description, stated once so nothing has to hedge it later
///
/// This is a **negamax search with a hard depth limit and a hard node budget**, using the standard
/// connect-four evaluation terms. It is not proved optimal, it is not proved unbeatable, and its
/// strength is a property of [`DEFAULT_DEPTH`] and [`DEFAULT_NODE_BUDGET`] rather than of its class.
/// Any report that calls it a perfect or optimal opponent is wrong, and this module's documentation
/// says so because a benchmark whose ceiling is misdescribed produces a benchmark whose numbers are
/// misread.
///
/// It is nonetheless a genuinely demanding opponent, and that is the point of it existing: at the
/// default depth it finds forced wins, blocks a forced loss, and punishes a dropped disc that
/// supports nothing — the class of mistakes a one-ply model makes constantly and a real model makes
/// too.
#[derive(Clone, Debug)]
pub struct DepthBoundedSearch {
    depth: u32,
    node_budget: u64,
}

impl Default for DepthBoundedSearch {
    fn default() -> Self {
        Self::new()
    }
}

impl DepthBoundedSearch {
    /// The depth the arena seats by default.
    ///
    /// Public because it is a property of every result produced against this opponent, and a reader
    /// who cannot see the depth cannot judge the result.
    pub const DEFAULT_DEPTH: u32 = 6;

    /// The node budget the arena seats by default.
    ///
    /// A hard stop, not a heuristic. Without it a depth-six search on an opening position can spend
    /// long enough that the desktop appears to hang, and a benchmark that stalls is a benchmark
    /// operators cancel.
    pub const DEFAULT_NODE_BUDGET: u64 = 30_000;

    /// Creates the opponent at its default depth and budget.
    pub fn new() -> Self {
        Self {
            depth: Self::DEFAULT_DEPTH,
            node_budget: Self::DEFAULT_NODE_BUDGET,
        }
    }

    /// Creates the opponent at an explicit depth and node budget.
    pub fn with_limits(depth: u32, node_budget: u64) -> Self {
        Self { depth, node_budget }
    }

    /// The search depth in plies.
    pub fn depth(&self) -> u32 {
        self.depth
    }

    /// The node budget.
    pub fn node_budget(&self) -> u64 {
        self.node_budget
    }

    /// Scores a position **for the side to move**, in centi-disc units.
    ///
    /// Positive favours the side to move. The same perspective convention the tic-tac-toe solver
    /// uses, for the same reason: an absolute score that the caller has to negate is a sign error
    /// waiting to happen, and one already happened in this repository.
    fn evaluate(state: &ConnectFour, side: GameSide) -> i64 {
        let own = Self::mark(side);
        let theirs = Self::mark(side.other());
        let mut score = 0i64;
        // Centre control, weighted by how many own discs sit in the two middle columns. A disc in the
        // middle of a seven-column board belongs to more winning lines than one on an edge.
        for column in 0..COLUMNS {
            let weight = match column {
                3 => 4,
                2 | 4 => 2,
                _ => 1,
            };
            for row in 0..ROWS {
                match state.cell(column, row) {
                    Some(cell) if cell == own => score += i64::from(weight),
                    Some(cell) if cell == theirs => score -= i64::from(weight),
                    _ => {}
                }
            }
        }
        // Threat counting. Immediate wins and blocks dominate everything else because they end the
        // game; two-in-a-row threats are worth a lot because they decide the next ply.
        score += Self::count_winning_moves(state, side) as i64 * 10_000;
        score -= Self::count_winning_moves(state, side.other()) as i64 * 10_000;
        score += Self::count_threats(state, side, 3) as i64 * 120;
        score -= Self::count_threats(state, side.other(), 3) as i64 * 120;
        score += Self::count_threats(state, side, 2) as i64 * 8;
        score -= Self::count_threats(state, side.other(), 2) as i64 * 8;
        score
    }

    /// How many drops would win immediately for `side`.
    ///
    /// Uses [`ConnectFour::wins_at`] on the cell the drop actually lands in, so this costs seven
    /// sixteen-cell walks rather than seven full 168-window board scans.
    fn count_winning_moves(state: &ConnectFour, side: GameSide) -> usize {
        legal_columns(state)
            .into_iter()
            .filter(|column| {
                state
                    .landing_row(*column)
                    .is_some_and(|row| state.wins_at(*column, row, Self::mark(side)))
            })
            .count()
    }

    /// How many windows hold `needed` of `side`'s discs with no disc of the opponent's.
    ///
    /// Restricted to windows that pass through at least one **currently playable landing cell**. A
    /// window no disc can ever reach is a threat nobody can cash, and counting it was most of this
    /// function's cost: the restriction takes the work from 672 window scans per leaf to roughly 100,
    /// which is the difference between a search that runs and one that appears to hang.
    fn count_threats(state: &ConnectFour, side: GameSide, needed: usize) -> usize {
        let mark = Self::mark(side);
        let blocker = Self::mark(side.other());
        let mut total = 0usize;
        for column in 0..COLUMNS {
            let Some(landing) = state.landing_row(column) else {
                continue;
            };
            // Only the four windows through the playable cell of this column can be cashed.
            for (d_column, d_row) in ConnectFour::DIRECTIONS {
                // `landing` may sit anywhere along the line, so the window's first cell is the one
                // furthest back along the direction.
                for offset in 0..WIN_LENGTH {
                    let first_column =
                        landing as isize + d_column * (WIN_LENGTH as isize - 1 - offset as isize);
                    let first_row =
                        landing as isize + d_row * (WIN_LENGTH as isize - 1 - offset as isize);
                    let mut mine = 0usize;
                    let mut theirs = 0usize;
                    let mut in_bounds = true;
                    for step in 0..WIN_LENGTH {
                        let column = first_column + d_column * step as isize;
                        let row = first_row + d_row * step as isize;
                        if column < 0
                            || column >= COLUMNS as isize
                            || row < 0
                            || row >= ROWS as isize
                        {
                            in_bounds = false;
                            break;
                        }
                        match state.cell(column as usize, row as usize) {
                            Some(cell) if cell == mark => mine += 1,
                            Some(cell) if cell == blocker => theirs += 1,
                            _ => {}
                        }
                    }
                    if in_bounds && mine == needed && theirs == 0 {
                        total += 1;
                    }
                }
            }
        }
        total
    }

    /// Negamax from a position, for the side to move, bounded by `depth` and `nodes`.
    ///
    /// `just_won` carries whether the move that produced `state` completed a line. Passing it in
    /// rather than re-deriving it is what keeps the terminal test O(1) per node.
    #[allow(clippy::too_many_arguments)]
    fn search(
        &self,
        state: &ConnectFour,
        side: GameSide,
        depth: u32,
        nodes: &mut u64,
        alpha: i64,
        beta: i64,
        just_won: Option<GameSide>,
    ) -> (i64, Option<usize>) {
        if *nodes == 0 {
            return (Self::evaluate(state, side), None);
        }
        *nodes -= 1;
        if let Some(winner) = just_won {
            // Winning sooner is better. A distance-to-terminal score ranks a fast win above a slow
            // one and a slow loss above a fast one, which is what makes the search prefer a quick
            // finish over a long grind to the same result. Bounded by `MAX_PLIES`, so it can never
            // approach the search window's sentinels.
            let distance = MAX_PLIES as i64 - state.ply as i64;
            return (if winner == side { distance } else { -distance }, None);
        }
        if state.is_full() {
            return (0, None);
        }
        if depth == 0 {
            return (Self::evaluate(state, side), None);
        }
        // `Option` rather than an `i64::MIN` sentinel. A sentinel here has to be negated on every
        // negamax edge, and negating `i64::MIN` is a debug-build panic — which is exactly what the
        // first version of this function did at the root, where alpha was seeded to `i64::MIN`.
        let mut best: Option<i64> = None;
        let mut best_column: Option<usize> = None;
        let mut alpha_local = alpha;
        for column in legal_columns(state) {
            let after = state.drop(side, column);
            let won_here = after
                .landing_row(column)
                .filter(|_| state.column_is_legal(column))
                .is_some_and(|row| after.wins_at(column, row, Self::mark(side)));
            let (opponent_value, _) = self.search(
                &after,
                side.other(),
                depth - 1,
                nodes,
                -beta,
                -alpha_local,
                won_here.then_some(side),
            );
            let value = -opponent_value;
            if best.is_none_or(|current| value > current) {
                best = Some(value);
                best_column = Some(column);
            }
            if value > alpha_local {
                alpha_local = value;
            }
            // A move that is already a forced win cannot be improved on, and the whole point of the
            // node budget is that it gets spent where it changes the answer.
            if best.is_some_and(|current| current >= Self::WIN_SCORE) {
                break;
            }
        }
        match best {
            Some(value) => (value, best_column),
            // No legal column at a position with no winner is a full board, already handled above.
            // Reaching here would mean the rules and the terminal checks disagree, so it scores a
            // draw rather than inventing a preference.
            None => (0, None),
        }
    }

    /// The value a position is worth once the search has proved a win or a loss.
    const WIN_SCORE: i64 = 1_000_000;

    /// The search window's open bound.
    ///
    /// Half of `i64` rather than the full range, because negamax negates both bounds on every edge
    /// and negating `i64::MIN` overflows. Every real score here is bounded by
    /// [`DepthBoundedSearch::WIN_SCORE`] plus the evaluation magnitude, so a window this wide cannot
    /// prune a line that mattered.
    const NEGATIVE_INFINITY: i64 = i64::MIN / 2;
    /// The search window's closed bound. See [`Self::NEGATIVE_INFINITY`].
    const POSITIVE_INFINITY: i64 = i64::MAX / 2;

    fn mark(side: GameSide) -> ConnectFourCell {
        match side {
            GameSide::First => ConnectFourCell::First,
            GameSide::Second => ConnectFourCell::Second,
        }
    }
}

impl ReferenceStrategy for DepthBoundedSearch {
    fn name(&self) -> &'static str {
        "depth_bounded_connect_four"
    }

    fn knowledge(&self) -> ReferenceKnowledge {
        ReferenceKnowledge::OBSERVATION_ONLY
    }

    fn decide(
        &mut self,
        request: &probelab_core::ModelRequest,
    ) -> Result<probelab_core::ModelDecision, String> {
        let game = request
            .game
            .as_ref()
            .ok_or_else(|| "the search opponent needs a game request".to_owned())?;
        if game.kind != GameKind::ConnectFour {
            return Err(format!(
                "the search opponent only plays connect four, not {:?}",
                game.kind
            ));
        }
        let GameBoard::ConnectFour {
            columns,
            rows,
            cells,
        } = &game.board
        else {
            return Err("the search opponent needs a connect-four board".to_owned());
        };
        if *columns != COLUMNS || *rows != ROWS || cells.len() != CELLS {
            return Err(format!(
                "a connect-four board must be {COLUMNS}x{ROWS} with {CELLS} cells, got {columns}x{rows} with {}",
                cells.len()
            ));
        }
        let mut state = ConnectFour::opening();
        state.cells = cells.clone();
        for column in 0..COLUMNS {
            // Heights are recounted rather than trusted, because the request is a *view* built by a
            // game the caller supplied. A view that claimed a full column at row two would let the
            // search drop a disc through a disc, and the search is exactly the code that must not be
            // able to play an illegal position.
            state.heights[column] = (0..ROWS)
                .filter(|row| !matches!(state.cells[column * ROWS + row], ConnectFourCell::Empty))
                .count() as u8;
        }
        let side = game.side;
        // Moves are tried in the order the search would prefer to find them: immediate wins first,
        // then blocks, then the tallest stacks. Ordering decides only which of several equal-scoring
        // moves is returned — but an opponent whose choice among equals is reproducible is what makes
        // a replay digest mean anything, so it is deliberate rather than incidental.
        let mut candidates = legal_columns(&state);
        candidates.sort_by_key(|column| {
            let after = state.drop(side, *column);
            (
                after.winner() != Some(side),
                after.winner() != Some(side.other()),
                std::cmp::Reverse(usize::from(state.height(*column))),
                *column,
            )
        });
        if candidates.is_empty() {
            return Err("the search opponent was given a full board".to_owned());
        }
        let mut nodes = self.node_budget;
        let mut alpha = Self::NEGATIVE_INFINITY;
        let mut best: Option<(i64, usize)> = None;
        let mut searched_any = false;
        for column in candidates {
            let after = state.drop(side, column);
            let row = state.landing_row(column).unwrap_or(0);
            let won_here = after.wins_at(column, row, Self::mark(side));
            let (opponent_value, _) = self.search(
                &after,
                side.other(),
                self.depth,
                &mut nodes,
                -Self::POSITIVE_INFINITY,
                -alpha,
                won_here.then_some(side),
            );
            searched_any = true;
            let value = -opponent_value;
            if best.is_none_or(|(current, _)| value > current) {
                best = Some((value, column));
            }
            if value > alpha {
                alpha = value;
            }
            // A proven forced win cannot be improved on, so the remaining budget is worth more spent
            // on the next ply than here.
            if best.is_some_and(|(current, _)| current >= Self::WIN_SCORE) {
                break;
            }
            // The budget is a hard stop. Running out is not an error: the move ordering above has
            // already put the most urgent candidates first, so an exhausted budget degrades to a
            // well-ordered heuristic rather than to an arbitrary legal move.
            if nodes == 0 {
                break;
            }
        }
        // Unreachable while any candidate was searchable, because the loop body runs at least once
        // for a non-empty candidate list. Reported as an error rather than a fallback move so a
        // future change that empties the list fails loudly instead of inventing a play.
        let Some((_, column)) = best.filter(|_| searched_any) else {
            return Err("the search opponent evaluated no candidate move".to_owned());
        };
        Ok(probelab_core::ModelDecision::GameMove(
            GameMove::ConnectFour(column),
        ))
    }
}

/// The bound on a game prompt's rendered move history, re-exported so callers building a prompt for
/// this game do not have to reach into `probelab_core` for a number they must then explain.
pub const HISTORY_WINDOW: usize = GAME_HISTORY_WINDOW;

#[cfg(test)]
mod tests {
    use super::*;
    // The `Game` trait supplies `view` and `apply`, which the tests exercise through the same
    // trait the arena drives matches through — not a private inherent copy that could drift.
    use super::super::{
        play_match, Game, MatchConfig, MatchOutcome, NullRecorder, PlyOutcome, ReferenceSeat,
    };
    use probelab_core::ModelRequest;

    fn game_at(cells: &[(usize, usize, GameSide)]) -> ConnectFour {
        let mut state = ConnectFour::opening();
        for (column, row, side) in cells {
            state = state.drop(*side, *column);
            assert_eq!(
                state.cell(*column, *row),
                Some(ConnectFour::mark(*side)),
                "the disc should have landed at the requested row"
            );
        }
        state
    }

    #[test]
    fn gravity_sends_a_disc_to_the_lowest_empty_cell() {
        let state = game_at(&[(3, 0, GameSide::First), (3, 1, GameSide::Second)]);
        assert_eq!(state.height(3), 2);
        assert_eq!(state.landing_row(3), Some(2));
        assert_eq!(state.cell(3, 0), Some(ConnectFourCell::First));
        assert_eq!(state.cell(3, 1), Some(ConnectFourCell::Second));
        assert_eq!(state.cell(3, 2), Some(ConnectFourCell::Empty));
    }

    #[test]
    fn the_column_major_index_is_column_times_rows_plus_row() {
        // Pinned because every renderer and every rule depends on it, and a row-major reading would
        // render a real, correctly-played board as nonsense.
        let state = game_at(&[(6, 0, GameSide::First)]);
        assert_eq!(state.cells[6 * ROWS], ConnectFourCell::First);
        assert_eq!(state.cell(6, 0), Some(ConnectFourCell::First));
        assert_eq!(state.cell(6, 5), Some(ConnectFourCell::Empty));
        assert_eq!(state.cell(7, 0), None, "column 7 is off the board");
        assert_eq!(state.cell(0, 6), None, "row 6 is off the board");
    }

    #[test]
    fn a_full_column_is_not_legal_and_a_drop_into_it_is_refused() {
        let mut state = ConnectFour::opening();
        for _ in 0..ROWS {
            state = state.drop(GameSide::First, 0);
            state = state.drop(GameSide::Second, 1);
        }
        assert!(!state.column_is_legal(0));
        assert!(state.column_is_legal(2));
        let game = ConnectFour::default_game();
        assert!(game
            .apply(&state, GameSide::First, GameMove::ConnectFour(0))
            .is_err());
    }

    #[test]
    fn out_of_turn_is_refused_and_an_off_board_column_is_refused() {
        let state = ConnectFour::opening();
        let game = ConnectFour::default_game();
        assert!(game
            .apply(&state, GameSide::Second, GameMove::ConnectFour(0))
            .is_err());
        assert!(game
            .apply(&state, GameSide::First, GameMove::ConnectFour(COLUMNS))
            .is_err());
        assert!(game
            .apply(&state, GameSide::First, GameMove::TicTacToe(0))
            .is_err());
    }

    #[test]
    fn every_win_direction_is_detected_with_its_own_four_cells() {
        // Four separate positions, one per direction. A check that only ever ran vertically would
        // pass a vertical-only test, and connect four's whole difficulty is that the four directions
        // are equally live — so each direction gets a position that no other direction also satisfies.
        let vertical = game_at(&[
            (0, 0, GameSide::First),
            (1, 0, GameSide::Second),
            (0, 1, GameSide::First),
            (1, 1, GameSide::Second),
            (0, 2, GameSide::First),
            (1, 2, GameSide::Second),
            (0, 3, GameSide::First),
        ]);
        assert_eq!(vertical.winner(), Some(GameSide::First));
        assert_eq!(
            vertical.winning_cells(),
            Some([(0, 0), (0, 1), (0, 2), (0, 3)])
        );

        // First owns four consecutive columns on the bottom row. Built column by column so every
        // disc really is at row 0; a diagonal and a horizontal four both exist in this shape and the
        // assertion pins the horizontal one specifically.
        let mut horizontal = ConnectFour::opening();
        for column in 0..4usize {
            horizontal = horizontal.drop(GameSide::First, column);
            if column < 3 {
                horizontal = horizontal.drop(GameSide::Second, 4 + column);
            }
        }
        assert_eq!(horizontal.winner(), Some(GameSide::First));
        assert_eq!(
            horizontal.winning_cells(),
            Some([(0, 0), (1, 0), (2, 0), (3, 0)]),
            "the highlighted line must be the horizontal one"
        );

        // A rising diagonal needs First at (0,3) (1,2) (2,1) (3,0), so each column is filled to a
        // different height with the opponent underneath and First's winning disc on top.
        let mut rising = ConnectFour::opening();
        for column in 0..3usize {
            // Opponent stacks `column` discs, then First caps it, leaving First at row `3 - column`.
            for _ in 0..(3 - column) {
                rising = rising.drop(GameSide::Second, column);
            }
            rising = rising.drop(GameSide::First, column);
        }
        rising = rising.drop(GameSide::First, 3);
        assert_eq!(rising.winner(), Some(GameSide::First));
        assert_eq!(
            rising.winning_cells(),
            Some([(0, 3), (1, 2), (2, 1), (3, 0)]),
            "the highlighted line must be the rising diagonal"
        );

        // The mirror image: First at (0,0) (1,1) (2,2) (3,3).
        let mut falling = ConnectFour::opening();
        falling = falling.drop(GameSide::First, 0);
        for column in 1..4usize {
            for _ in 0..column {
                falling = falling.drop(GameSide::Second, column);
            }
            falling = falling.drop(GameSide::First, column);
        }
        assert_eq!(falling.winner(), Some(GameSide::First));
        assert_eq!(
            falling.winning_cells(),
            Some([(0, 0), (1, 1), (2, 2), (3, 3)]),
            "the highlighted line must be the falling diagonal"
        );
    }

    #[test]
    fn three_in_a_row_is_not_a_win() {
        let state = game_at(&[
            (0, 0, GameSide::First),
            (1, 0, GameSide::Second),
            (0, 1, GameSide::First),
            (1, 1, GameSide::Second),
            (0, 2, GameSide::First),
        ]);
        assert_eq!(state.winner(), None);
        assert_eq!(state.winning_cells(), None);
    }

    #[test]
    fn a_full_board_with_no_line_offers_no_moves_and_declares_no_winner() {
        // A board with no window of four monochromatic in any of the four directions.
        //
        // The mark is chosen from `(column + 2 * row) mod 4`, split two residues per side. The
        // reasoning is worth stating because the obvious constructions all fail: a residue held
        // *constant* along a direction — `(column + row) mod k` — makes every falling diagonal
        // monochrome by construction, and a modulus of five with two marks per side is no better,
        // because four *distinct* residues out of five can all land on the same mark.
        //
        // With modulus four, the residue advances by +1 horizontally, +2 vertically, +3 on the rising
        // diagonal and -1 on the falling one. Over four cells that is either all four residues
        // (both marks present) or the pair `r`, `r + 2` twice — and those two residues always fall on
        // opposite marks, because the split is two-per-side. So no window of four is ever one mark.
        //
        // Constructed rather than played, and that is stated rather than hidden: the mark counts this
        // needs are not those of an alternating game. It proves the *rules* — a full board with no
        // line is not a win and offers nothing to play — while `a_draw_is_reported_when_the_board_
        // fills_without_a_winner` above proves the runner's classification from a real match.
        let mark_at = |column: usize, row: usize| {
            if (column + 2 * row) % 4 < 2 {
                ConnectFourCell::Second
            } else {
                ConnectFourCell::First
            }
        };
        // Verify the fixture before relying on it, with a scan written independently of the engine's.
        // A fixture that quietly stopped being a draw would otherwise make this test assert a lie.
        let mut windows_checked = 0usize;
        for column in 0..COLUMNS {
            for row in 0..ROWS {
                for (d_column, d_row) in ConnectFour::DIRECTIONS {
                    for offset in 0..WIN_LENGTH {
                        let first = (
                            column as isize
                                + d_column * (WIN_LENGTH as isize - 1 - offset as isize),
                            row as isize + d_row * (WIN_LENGTH as isize - 1 - offset as isize),
                        );
                        let cells: Vec<(usize, usize)> = (0..WIN_LENGTH)
                            .map(|step| {
                                (
                                    (first.0 + d_column * step as isize) as usize,
                                    (first.1 + d_row * step as isize) as usize,
                                )
                            })
                            .collect();
                        // Windows that leave the board are not windows; the engine rejects them for
                        // the same reason, so the fixture check rejects them too rather than wrapping.
                        if !cells.iter().all(|(c, r)| *c < COLUMNS && *r < ROWS) {
                            continue;
                        }
                        windows_checked += 1;
                        let marks: Vec<ConnectFourCell> =
                            cells.iter().map(|(c, r)| mark_at(*c, *r)).collect();
                        assert!(
                            marks.windows(2).any(|pair| pair[0] != pair[1]),
                            "the fixture has a monochrome four at {cells:?}"
                        );
                    }
                }
            }
        }
        assert!(
            windows_checked > 100,
            "the fixture check examined {windows_checked} windows, which is too few to be meaningful"
        );

        let mut state = ConnectFour::opening();
        for column in 0..COLUMNS {
            for row in 0..ROWS {
                state.cells[column * ROWS + row] = mark_at(column, row);
            }
            state.heights[column] = ROWS as u8;
        }
        assert!(state.is_full());
        assert_eq!(state.winner(), None);
        assert_eq!(state.winning_cells(), None);
        assert!(
            legal_columns(&state).is_empty(),
            "a full board must offer no legal column"
        );
        let game = ConnectFour::default_game();
        assert!(
            game.apply(&state, GameSide::First, GameMove::ConnectFour(0))
                .is_err(),
            "a drop into a full board must be refused"
        );
    }

    #[test]
    fn a_draw_is_reported_when_the_board_fills_without_a_winner() {
        // The runner's classification of a draw, proved on a real `play_match` rather than on a
        // constructed position. A minimal test-only game is used so the state is reachable in four
        // plies: this tests `play_match`'s rule, while connect four's own terminal conditions are
        // covered by the tests above.
        struct NeverWon;

        impl Game for NeverWon {
            type State = (u32, bool);

            fn kind(&self) -> GameKind {
                GameKind::Rps
            }

            fn initial(&self, _seed: u64) -> Self::State {
                (0, false)
            }

            fn legal_moves(&self, _state: &Self::State, _side: GameSide) -> Vec<GameMove> {
                vec![GameMove::Rps(probelab_core::RpsChoice::Rock)]
            }

            fn view(
                &self,
                _state: &Self::State,
                side: GameSide,
                ply: u32,
                last: Option<GameMove>,
            ) -> GameRequest {
                GameRequest {
                    kind: GameKind::Rps,
                    side,
                    board: GameBoard::Rps { plays: Vec::new() },
                    legal_moves: vec![GameMove::Rps(probelab_core::RpsChoice::Rock)],
                    history: Vec::new(),
                    ply,
                    remaining_plies: self.max_plies().saturating_sub(ply).max(1),
                    last_move: last,
                }
            }

            fn apply(
                &self,
                state: &Self::State,
                _side: GameSide,
                _move_: GameMove,
            ) -> Result<Self::State, String> {
                Ok((state.0 + 1, false))
            }

            fn winner(&self, _state: &Self::State) -> Option<GameSide> {
                None
            }

            fn board(&self, _state: &Self::State, _side: GameSide) -> GameBoard {
                GameBoard::Rps { plays: Vec::new() }
            }

            fn max_plies(&self) -> u32 {
                4
            }
        }

        struct AlwaysRock;
        impl ReferenceStrategy for AlwaysRock {
            fn name(&self) -> &'static str {
                "always_rock"
            }
            fn knowledge(&self) -> ReferenceKnowledge {
                ReferenceKnowledge::OBSERVATION_ONLY
            }
            fn decide(
                &mut self,
                _request: &ModelRequest,
            ) -> Result<probelab_core::ModelDecision, String> {
                Ok(probelab_core::ModelDecision::GameMove(GameMove::Rps(
                    probelab_core::RpsChoice::Rock,
                )))
            }
        }

        let game = NeverWon;
        let mut tested = Box::new(ReferenceSeat::new(Box::new(AlwaysRock)));
        let mut opponent = Box::new(ReferenceSeat::new(Box::new(AlwaysRock)));
        let config = MatchConfig::new("tested", "opponent", 1, "draw-run", "draw-episode");
        let mut seen = Vec::new();
        let result = play_match(
            &game,
            tested.as_mut(),
            opponent.as_mut(),
            GameSide::First,
            &config,
            &NullRecorder,
            &mut |outcome: PlyOutcome| {
                seen.push(outcome.ply);
                true
            },
        )
        .expect("the match completes");
        assert_eq!(seen, vec![1, 2, 3, 4], "every ply must reach the observer");
        assert_eq!(
            result.outcome,
            MatchOutcome::Draw,
            "a match that fills its board with no winner is a draw, not a loss"
        );
        assert_eq!(result.plies, 4);
        assert!(result.illegal_moves.is_empty());
    }

    #[test]
    fn a_full_connect_four_match_completes_with_a_decided_or_drawn_outcome() {
        // A whole real match, end to end, to prove the rules compose: every drop legal, every
        // recorded ply consistent with the board it produced, and a terminal state the runner can
        // classify. The outcome itself is whatever the search found — this test asserts the match is
        // well formed, not which side won, because which side wins is not a property of the rules.
        let game = ConnectFour::default_game();
        let mut tested = Box::new(ReferenceSeat::new(Box::new(DepthBoundedSearch::new())));
        let mut opponent = Box::new(ReferenceSeat::new(Box::new(DepthBoundedSearch::new())));
        let config = MatchConfig::new("depth", "depth", 7, "cf-run", "cf-episode");
        let result = play_match(
            &game,
            tested.as_mut(),
            opponent.as_mut(),
            GameSide::First,
            &config,
            &NullRecorder,
            &mut |_outcome: PlyOutcome| true,
        )
        .expect("the match completes");
        assert!(
            result.plies > 0 && result.plies <= MAX_PLIES,
            "plies {} is outside the board",
            result.plies
        );
        assert!(result.illegal_moves.is_empty());
        assert!(matches!(
            result.outcome,
            MatchOutcome::Win | MatchOutcome::Draw | MatchOutcome::Loss
        ));
    }

    #[test]
    fn the_first_legal_baseline_takes_the_leftmost_column_that_still_accepts_a_disc() {
        // The distinguishing part is the full column. A baseline tested only on an empty board would
        // pass a test that never filled one, and would then look like it had a strategy.
        let game = ConnectFour::default_game();
        let mut state = ConnectFour::opening();
        for _ in 0..ROWS {
            state = state.drop(GameSide::First, 0);
            state = state.drop(GameSide::Second, 0);
        }
        assert!(!state.column_is_legal(0), "column 0 must now be full");
        let request = ModelRequest {
            episode_id: "m1".to_owned(),
            turn: 13,
            observation: no_grid(13),
            available_actions: Vec::new(),
            process_spawn_available: false,
            history: Vec::new(),
            game: Some(game.view(&state, GameSide::First, 13, None)),
        };
        let mut baseline = FirstLegalConnectFour;
        assert_eq!(
            baseline.decide(&request),
            Ok(probelab_core::ModelDecision::GameMove(
                GameMove::ConnectFour(1)
            )),
            "column 0 is full and must be skipped"
        );
    }

    #[test]
    fn the_search_opponent_takes_an_available_immediate_win() {
        let game = ConnectFour::default_game();
        // First has three in column 2 and is about to complete it; Second must block.
        let mut state = ConnectFour::opening();
        state = state.drop(GameSide::First, 2);
        state = state.drop(GameSide::Second, 0);
        state = state.drop(GameSide::First, 2);
        state = state.drop(GameSide::Second, 1);
        state = state.drop(GameSide::First, 2);
        // Now First threatens column 2, and also has three on row 0 style shapes elsewhere; Second is
        // to move and must block column 2 or lose immediately.
        let mut opponent = DepthBoundedSearch::with_limits(4, 20_000);
        let request = ModelRequest {
            episode_id: "m1".to_owned(),
            turn: 6,
            observation: no_grid(6),
            available_actions: Vec::new(),
            process_spawn_available: false,
            history: Vec::new(),
            game: Some(game.view(&state, GameSide::Second, 6, None)),
        };
        assert_eq!(
            opponent.decide(&request),
            Ok(probelab_core::ModelDecision::GameMove(
                GameMove::ConnectFour(2)
            )),
            "a forced loss must be blocked"
        );
    }

    #[test]
    fn the_search_opponent_takes_a_win_it_is_threatened_with() {
        let game = ConnectFour::default_game();
        let mut state = ConnectFour::opening();
        state = state.drop(GameSide::Second, 3);
        state = state.drop(GameSide::First, 0);
        state = state.drop(GameSide::Second, 3);
        state = state.drop(GameSide::First, 1);
        state = state.drop(GameSide::Second, 3);
        let mut opponent = DepthBoundedSearch::with_limits(4, 20_000);
        let request = ModelRequest {
            episode_id: "m1".to_owned(),
            turn: 6,
            observation: no_grid(6),
            available_actions: Vec::new(),
            process_spawn_available: false,
            history: Vec::new(),
            game: Some(game.view(&state, GameSide::First, 6, None)),
        };
        assert_eq!(
            opponent.decide(&request),
            Ok(probelab_core::ModelDecision::GameMove(
                GameMove::ConnectFour(3)
            ))
        );
    }

    #[test]
    fn the_search_opponent_respects_its_stated_budget_and_never_exceeds_it() {
        // The budget is a hard stop, and this is the test that keeps it one: without it, a node
        // budget that is advisory is indistinguishable from no budget until a user waits on it.
        let game = ConnectFour::default_game();
        let state = ConnectFour::opening();
        let mut opponent = DepthBoundedSearch::with_limits(8, 5);
        let request = ModelRequest {
            episode_id: "m1".to_owned(),
            turn: 1,
            observation: no_grid(1),
            available_actions: Vec::new(),
            process_spawn_available: false,
            history: Vec::new(),
            game: Some(game.view(&state, GameSide::First, 1, None)),
        };
        let started = std::time::Instant::now();
        let decision = opponent.decide(&request);
        assert!(decision.is_ok());
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "a five-node budget must return immediately"
        );
    }

    #[test]
    fn the_search_opponent_rejects_a_board_that_is_not_seven_by_six() {
        let mut opponent = DepthBoundedSearch::new();
        let request = ModelRequest {
            episode_id: "m1".to_owned(),
            turn: 1,
            observation: no_grid(1),
            available_actions: Vec::new(),
            process_spawn_available: false,
            history: Vec::new(),
            game: Some(probelab_core::GameRequest {
                kind: GameKind::ConnectFour,
                side: GameSide::First,
                board: GameBoard::ConnectFour {
                    columns: 8,
                    rows: 8,
                    cells: vec![ConnectFourCell::Empty; 64],
                },
                legal_moves: vec![GameMove::ConnectFour(0)],
                history: Vec::new(),
                ply: 1,
                remaining_plies: 63,
                last_move: None,
            }),
        };
        assert!(opponent.decide(&request).is_err());
    }

    #[test]
    fn the_search_opponent_is_not_claimed_to_be_optimal() {
        // A documentation test, because the ceiling's honesty is load-bearing for every number
        // measured against it: a report that called a bounded search unbeatable would misread every
        // result, and a reader has only the docs to go on.
        //
        // Scoped to the two places that describe the *search* opponent — the module's module-level
        // section and the struct's own documentation — because the rest of the module describes the
        // baseline, whose honest description legitimately says it never loses to a model that plays
        // only legal moves. Scanning the whole file for bare words would flag that correct sentence,
        // and flagging it would train a future author to word the disclaimer more vaguely in order
        // to slip past the check.
        let source = include_str!("connect_four.rs");
        let slice = |from: &str, to: &str| {
            let start = source
                .find(from)
                .unwrap_or_else(|| panic!("the module should contain {from:?}"));
            let tail = &source[start..];
            let end = tail
                .find(to)
                .unwrap_or_else(|| panic!("{from:?} should be followed by {to:?}"));
            tail[..end].to_ascii_lowercase()
        };
        let claims = [
            slice(
                "The opponent, and what it is not",
                "The first-legal-move baseline",
            ),
            slice(
                "A depth-bounded strategic opponent.",
                "pub struct DepthBoundedSearch",
            ),
        ]
        .concat();
        for forbidden in [
            "perfect play",
            "is unbeatable",
            "never loses",
            "provably optimal",
            "always wins",
            "the optimal opponent",
        ] {
            assert!(
                !claims.contains(forbidden),
                "the search opponent's documentation must not claim {forbidden:?}"
            );
        }
        // The disclaimers must be present, not merely the claims absent: a doc that said nothing at
        // all would pass the check above while failing the reader.
        assert!(
            claims.contains("not** proved optimal"),
            "the module must keep stating that optimality is not proved"
        );
        assert!(
            claims.contains("not** proved unbeatable"),
            "the module must keep stating that unbeateness is not proved"
        );
    }

    fn no_grid(ply: u32) -> probelab_core::Observation {
        probelab_core::Observation {
            turn: ply,
            position: probelab_core::NO_GRID_POSITION,
            grid_size: probelab_core::NO_GRID,
            current_tile: probelab_core::TileKind::Safe,
            visible_neighbors: Vec::new(),
            distance_to_goal: 0,
            max_turns: MAX_PLIES,
            remaining_turns: MAX_PLIES.saturating_sub(ply).max(1),
            available_actions: Vec::new(),
        }
    }
}
