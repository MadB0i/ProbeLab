//! Tic-tac-toe: the warm-up tier.
//!
//! # Why this game is here, and what it is not allowed to claim
//!
//! It is the easiest possible probe that the arena works end to end, and against a *perfect* opponent
//! the only non-losing outcomes are draws. So the honest headline is **loss rate**: any loss is a
//! blunder, and a policy that draws every game has shown it does not blunder, which is a floor cleared
//! and not a target hit.
//!
//! Draw rate is reported separately and is explicitly not treated as ceiling-setting. A 3x3 board has
//! a nine-ply ceiling and both sides draw easily; a benchmark that scored a strong model at "1.0" here
//! would be measuring tic-tac-toe rather than the model. That is why this is a warm-up tier.

use probelab_agent::{ReferenceKnowledge, ReferenceStrategy};
use probelab_core::{GameBoard, GameKind, GameMove, GameRequest, GameSide, TicTacToeCell};

/// The board is three by three and a cell index is row-major from the top left.
pub const SIDE: usize = 3;
/// Nine cells.
pub const CELLS: usize = SIDE * SIDE;
/// A full game cannot exceed nine plies.
pub const MAX_PLIES: u32 = 9;

/// The eight winning lines, as index triples.
const LINES: [[usize; 3]; 8] = [
    [0, 1, 2],
    [3, 4, 5],
    [6, 7, 8],
    [0, 3, 6],
    [1, 4, 7],
    [2, 5, 8],
    [0, 4, 8],
    [2, 4, 6],
];

/// A tic-tac-toe position. The whole board is public information, so a side's view is the board.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TicTacToe {
    cells: [TicTacToeCell; CELLS],
    next: GameSide,
    ply: u32,
}

impl TicTacToe {
    /// The opening position.
    ///
    /// The seed is deliberately unused. Tic-tac-toe has no opening variety to seed, and pretending
    /// otherwise would imply a source of variation that does not exist.
    pub fn opening() -> Self {
        Self {
            cells: [TicTacToeCell::Empty; CELLS],
            next: GameSide::First,
            ply: 0,
        }
    }

    /// The game itself.
    ///
    /// A separate constructor from [`TicTacToe::opening`] on purpose, because the two are different
    /// things that happen to share a type: one is the rule set the arena drives matches with, the
    /// other is a position on a board. `opening` is a state; this is the game.
    pub fn default_game() -> Self {
        Self::opening()
    }

    /// Returns the mark belonging to a side.
    fn mark(side: GameSide) -> TicTacToeCell {
        match side {
            GameSide::First => TicTacToeCell::First,
            GameSide::Second => TicTacToeCell::Second,
        }
    }

    /// The cell as a side sees it. Public information, so this is the same for both sides.
    ///
    /// Returns `None` for an index outside the board rather than indexing into it. This is the one
    /// place a bare `usize` from outside reaches the array, and the index in question is a
    /// *model-supplied* move -- so an unchecked read here would be a way for a model to abort the match
    /// with a move the engine would otherwise refuse as illegal. `apply` and `decide` both bounds-check
    /// explicitly for exactly that reason; this accessor had no callers and no check, which is a latent
    /// version of the same hazard rather than a live one.
    ///
    /// `None` is the right answer rather than `Empty`: an out-of-range read that returned `Empty` would
    /// be indistinguishable from a legal empty cell.
    pub fn cell(&self, index: usize) -> Option<TicTacToeCell> {
        self.cells.get(index).copied()
    }

    /// The board as a core type, for the request and for telemetry.
    pub fn board(&self) -> GameBoard {
        GameBoard::TicTacToe {
            cells: self.cells.to_vec(),
        }
    }
}

/// Whether a side has completed a line.
pub fn has_won(cells: &[TicTacToeCell; CELLS], side: GameSide) -> bool {
    let mark = TicTacToe::mark(side);
    LINES
        .iter()
        .any(|line| line.iter().all(|index| cells[*index] == mark))
}

/// The three cells a side completed a line on, or `None` when it has not won.
///
/// This is the same [`LINES`] table [`has_won`] decides from, read out rather than reduced to a
/// boolean, so the cells a panel highlights are the cells the engine decided on. A caller that
/// wanted to draw a winning line had to re-derive it in the renderer; that is exactly the kind of
/// duplication that agrees with the engine until the two drift apart.
///
/// Returns the first completed line in [`LINES`] order when more than one is complete, which cannot
/// happen in legal play — the move that completes a second line is legal, so it is a property of the
/// recorded position rather than an assertion about how the match was played.
pub fn winning_line(cells: &[TicTacToeCell; CELLS], side: GameSide) -> Option<[usize; 3]> {
    let mark = TicTacToe::mark(side);
    LINES
        .iter()
        .find(|line| line.iter().all(|index| cells[*index] == mark))
        .copied()
}

impl super::Game for TicTacToe {
    type State = TicTacToe;

    fn kind(&self) -> GameKind {
        GameKind::TicTacToe
    }

    fn initial(&self, _seed: u64) -> Self::State {
        TicTacToe::opening()
    }

    fn legal_moves(&self, state: &Self::State, _side: GameSide) -> Vec<GameMove> {
        if state.winner().is_some() {
            return Vec::new();
        }
        (0..CELLS)
            .filter(|index| state.cells[*index] == TicTacToeCell::Empty)
            .map(GameMove::TicTacToe)
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
            kind: GameKind::TicTacToe,
            side: state.next,
            board: state.board(),
            legal_moves: self.legal_moves(state, state.next),
            // Empty on purpose, not because it was forgotten. Tic-tac-toe is perfect information: the
            // whole board is in `board`, so a move log would be a strictly redundant copy of what is
            // already there. A game that *does* have hidden state — battleship — is the case that
            // needs the history, and it is the case that gets it.
            history: Vec::new(),
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
        let GameMove::TicTacToe(index) = move_ else {
            return Err(format!("{move_:?} is not a tic-tac-toe move"));
        };
        if index >= CELLS {
            return Err(format!("cell {index} is off the board"));
        }
        if state.cells[index] != TicTacToeCell::Empty {
            return Err(format!("cell {index} is already taken"));
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
        let mut cells = state.cells;
        cells[index] = TicTacToe::mark(side);
        Ok(TicTacToe {
            cells,
            next: side.other(),
            ply: state.ply + 1,
        })
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

impl TicTacToe {
    /// The winner, if the position is decided.
    fn winner(&self) -> Option<GameSide> {
        if has_won(&self.cells, GameSide::First) {
            Some(GameSide::First)
        } else if has_won(&self.cells, GameSide::Second) {
            Some(GameSide::Second)
        } else {
            None
        }
    }
}

/// A perfect tic-tac-toe policy: never loses, and prefers the fastest win.
///
/// Backed by exhaustive minimax over the nine cells, which is small enough to solve outright. That is
/// the whole justification for using minimax here rather than a bounded search: for this board the
/// exact answer is cheap, and an approximate "strong" opponent would make a loss ambiguous between the
/// model blundering and the opponent missing something. A loss against this policy is a loss, full
/// stop.
#[derive(Clone, Debug, Default)]
pub struct PerfectTicTacToe;

impl PerfectTicTacToe {
    /// Creates the perfect policy.
    pub fn new() -> Self {
        Self
    }

    /// The value of a position **for the side to move**: 1 win, 0 draw, -1 loss.
    ///
    /// The perspective is the whole correctness question here, and an earlier version got it wrong by
    /// returning an absolute score (First wins = +1) and letting the caller negate it. That silently
    /// inverts every decision whenever the first player is the one moving, and the symptom was a
    /// "perfect" policy that lost to itself. Expressed relative to `side` here so the negation at the
    /// call site is the standard one and cannot be applied twice.
    fn minimax(cells: &mut [TicTacToeCell; CELLS], side: GameSide, depth: u32) -> i32 {
        if has_won(cells, GameSide::First) {
            return if side == GameSide::First { 1 } else { -1 };
        }
        if has_won(cells, GameSide::Second) {
            return if side == GameSide::Second { 1 } else { -1 };
        }
        let mut best = i32::MIN;
        for index in 0..CELLS {
            if cells[index] != TicTacToeCell::Empty {
                continue;
            }
            cells[index] = TicTacToe::mark(side);
            // Faster wins score higher, so the policy takes a win now over a win later. Depth is what
            // is left, so a deeper line is a faster one.
            let value = -Self::minimax(cells, side.other(), depth.saturating_sub(1));
            cells[index] = TicTacToeCell::Empty;
            if value > best {
                best = value;
            }
            if best == 1 {
                break;
            }
        }
        if best == i32::MIN {
            return 0;
        }
        best
    }
}

impl ReferenceStrategy for PerfectTicTacToe {
    fn name(&self) -> &'static str {
        "perfect_tic_tac_toe"
    }

    fn knowledge(&self) -> ReferenceKnowledge {
        // Reads the board, which is public information, and nothing else. It is told neither the
        // tested model's plan nor any hidden state, because there is none to tell it.
        ReferenceKnowledge::OBSERVATION_ONLY
    }

    fn decide(
        &mut self,
        request: &probelab_core::ModelRequest,
    ) -> Result<probelab_core::ModelDecision, String> {
        let Some(game) = request.game.as_ref() else {
            return Err("the perfect policy needs a game request".to_owned());
        };
        let GameBoard::TicTacToe { cells: board_cells } = &game.board else {
            return Err("the perfect policy only plays tic-tac-toe".to_owned());
        };
        if board_cells.len() != CELLS {
            return Err(format!(
                "a tic-tac-toe board must carry {CELLS} cells, got {}",
                board_cells.len()
            ));
        }
        let mut cells: [TicTacToeCell; CELLS] = [TicTacToeCell::Empty; CELLS];
        for (index, cell) in cells.iter_mut().enumerate() {
            *cell = board_cells[index];
        }
        let side = game.side;
        // Depth is the plies actually left, not the board's maximum. Using the maximum made the
        // "prefer the fastest win" tiebreak score against lines the game will never finish playing.
        let remaining = game
            .legal_moves
            .len()
            .min(MAX_PLIES.saturating_sub(game.ply.saturating_sub(1)) as usize);

        let mut best: Option<(i32, usize)> = None;
        for candidate in &game.legal_moves {
            let GameMove::TicTacToe(index) = *candidate else {
                continue;
            };
            if index >= CELLS {
                continue;
            }
            let mut trial = cells;
            trial[index] = TicTacToe::mark(side);
            let value = -PerfectTicTacToe::minimax(
                &mut trial,
                side.other(),
                u32::try_from(remaining).unwrap_or(1),
            );
            if best.is_none_or(|(current, _)| value > current) {
                best = Some((value, index));
            }
        }
        let Some((_, index)) = best else {
            return Err("the perfect policy was given no legal tic-tac-toe move".to_owned());
        };
        Ok(probelab_core::ModelDecision::GameMove(GameMove::TicTacToe(
            index,
        )))
    }
}
