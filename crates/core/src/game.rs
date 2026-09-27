//! Game moves and the model-facing game request.
//!
//! # Why this is a side channel and not a widened vocabulary
//!
//! `Action` is five grid movements, and `Observation` is a grid: a position, a terrain tile, four
//! adjacent tiles, and a Manhattan distance to a goal. None of that survives contact with a game
//! board. Tic-tac-toe needs nine cells and eight-way adjacency; connect four needs a column drop, not
//! a step, and a whole visible column rather than four neighbours; rock-paper-scissors has no board at
//! all; battleship has two boards and a hidden fleet.
//!
//! Rather than widen `Action` — which every grid benchmark, the evaluator, the telemetry digest, and
//! the desktop UI all read — the game world arrives as a separate, optional channel on
//! [`ModelRequest`](crate::ModelRequest). A grid benchmark sends `game: None` and nothing about it
//! changes. A game benchmark sends `game: Some(..)` and leaves the grid fields empty.
//!
//! The cost of that choice is real and worth stating: it is a second shape rather than one sum type,
//! and it will keep being a second shape. The agreed trigger for consolidating into an enum is a
//! *third* distinct request shape — a coding benchmark's diffs, or safety/injection payloads. This
//! file is the first shape, not the third.
//!
//! # Validation is not a convention
//!
//! A `GameMove` is only legal when the request actually offered a game, and a game request only makes
//! sense with the grid vocabulary empty. Both are enforced in
//! [`ModelRequest::validate`](crate::ModelRequest::validate), in the same style as the existing
//! `available_actions == observation.available_actions` rule, so neither can be satisfied by
//! convention and then quietly relied on.

use serde::{Deserialize, Serialize};

/// A game move, one case per game.
///
/// Exactly one new variant on `ModelDecision` covers every game now and any future one: only this
/// inner enum grows when a game is added. The alternative — a `TicTacToeMove` variant, a
/// `ConnectFourMove` variant — would put a top-level variant per game on the model boundary forever.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "game", content = "move")]
pub enum GameMove {
    /// A cell index on a three-by-three board, row-major from the top left.
    TicTacToe(usize),
    /// A column index to drop into on a connect-four board.
    ///
    /// A column, not a cell: gravity is the rule, so the landing row is the board's business and
    /// naming it here would let a model express a move the rules do not allow.
    ConnectFour(usize),
    /// One of the three hands.
    Rps(RpsChoice),
    /// A target cell to fire at.
    Battleship(Position),
}

impl GameMove {
    /// Returns which game this move belongs to.
    pub fn kind(&self) -> GameKind {
        match self {
            GameMove::TicTacToe(_) => GameKind::TicTacToe,
            GameMove::ConnectFour(_) => GameKind::ConnectFour,
            GameMove::Rps(_) => GameKind::Rps,
            GameMove::Battleship(_) => GameKind::Battleship,
        }
    }
}

/// The four games the arena ships.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GameKind {
    /// Three-by-three tic-tac-toe.
    TicTacToe,
    /// Connect four on a standard seven-by-six board.
    ConnectFour,
    /// Iterated rock-paper-scissors.
    Rps,
    /// Battleship on a ten-by-ten grid.
    Battleship,
}

impl GameKind {
    /// Returns a stable identifier, used in benchmark ids and descriptor parameters.
    pub fn as_str(self) -> &'static str {
        match self {
            GameKind::TicTacToe => "tic_tac_toe",
            GameKind::ConnectFour => "connect_four",
            GameKind::Rps => "rps",
            GameKind::Battleship => "battleship",
        }
    }
}

/// One of the three rock-paper-scissors hands.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RpsChoice {
    /// Rock.
    Rock,
    /// Paper.
    Paper,
    /// Scissors.
    Scissors,
}

impl RpsChoice {
    /// The three hands, in a fixed order, for building a legal-move list.
    pub const ALL: [RpsChoice; 3] = [RpsChoice::Rock, RpsChoice::Paper, RpsChoice::Scissors];

    /// Whether `self` beats `other`.
    pub fn beats(self, other: RpsChoice) -> bool {
        matches!(
            (self, other),
            (RpsChoice::Rock, RpsChoice::Scissors)
                | (RpsChoice::Paper, RpsChoice::Rock)
                | (RpsChoice::Scissors, RpsChoice::Paper)
        )
    }

    /// Returns the hand that beats `self`.
    pub fn countered_by(self) -> RpsChoice {
        match self {
            RpsChoice::Rock => RpsChoice::Paper,
            RpsChoice::Paper => RpsChoice::Scissors,
            RpsChoice::Scissors => RpsChoice::Rock,
        }
    }

    /// Returns a stable wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            RpsChoice::Rock => "rock",
            RpsChoice::Paper => "paper",
            RpsChoice::Scissors => "scissors",
        }
    }
}

/// A cell on a game board.
///
/// Deliberately not the grid-world `Coordinate`. That type means "where the agent is standing", and
/// its bounds are the agent's step radius; a battleship target is a square of ocean, and reusing the
/// type would quietly couple the games' geometry to the grid world's vocabulary.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Position {
    /// Column, zero-based from the left.
    pub x: u8,
    /// Row, zero-based from the top.
    pub y: u8,
}

impl Position {
    /// Creates a position.
    pub const fn new(x: u8, y: u8) -> Self {
        Self { x, y }
    }

    /// Returns true when this position lies inside a `width` by `height` board.
    pub fn is_inside(&self, width: usize, height: usize) -> bool {
        usize::from(self.x) < width && usize::from(self.y) < height
    }
}

/// Which side of a match a player is.
///
/// Shared by every game rather than per-game, because "first" and "second" are the only distinctions
/// the rules need. The board carries the marks, so a game's own symbol — X and O, or a fleet — is a
/// property of the game rather than of the side.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GameSide {
    /// The player who moves first.
    First,
    /// The player who moves second.
    Second,
}

impl GameSide {
    /// Returns the other side.
    pub fn other(self) -> GameSide {
        match self {
            GameSide::First => GameSide::Second,
            GameSide::Second => GameSide::First,
        }
    }
}

/// The state of a game board, as the player sees it.
///
/// Hidden information is represented by the *absence* of a value: a battleship enemy cell is
/// `Unknown` until fired at, never a peek. That is the whole point of the variant, so it is stated
/// rather than left to each game's implementation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "game", content = "state")]
pub enum GameBoard {
    /// A three-by-three board, row-major, index `y * 3 + x`.
    TicTacToe {
        /// The nine cells.
        cells: Vec<TicTacToeCell>,
    },
    /// A connect-four board, stored column-major so gravity is a per-column stack.
    ConnectFour {
        /// Column count.
        columns: usize,
        /// Row count.
        rows: usize,
        /// `columns * rows` cells, column-major.
        cells: Vec<ConnectFourCell>,
    },
    /// Rock-paper-scissors has no board, only what each side has played.
    Rps {
        /// Every hand played so far, oldest first.
        plays: Vec<RpsChoice>,
    },
    /// Battleship, with both boards as the firing player sees them.
    Battleship {
        /// The firing player's own fleet, including hits taken.
        own_fleet: Vec<BattleshipCell>,
        /// What is known of the opponent's water.
        enemy_water: Vec<BattleshipCell>,
        /// Own fleet width, which is 1 on a ten-by-ten board.
        width: usize,
        /// Own fleet height.
        height: usize,
    },
}

/// One cell of a tic-tac-toe board.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TicTacToeCell {
    /// Nobody has played here.
    #[default]
    Empty,
    /// The first side's mark.
    First,
    /// The second side's mark.
    Second,
}

/// One cell of a connect-four board.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectFourCell {
    /// Nothing has settled here yet.
    #[default]
    Empty,
    /// Settled by the first side.
    First,
    /// Settled by the second side.
    Second,
}

/// One cell of a battleship board, as the player looking at it sees it.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BattleshipCell {
    /// No shot has landed here and nothing is known.
    #[default]
    Unknown,
    /// A shot that hit nothing.
    Miss,
    /// A shot that hit a ship.
    Hit,
    /// The player's own water, where a ship sits.
    OwnShip,
    /// The player's own water where a shot has hit.
    OwnHit,
    /// The player's own water that has been searched and found empty.
    OwnMiss,
}

/// The game half of a model request.
///
/// Present only for a game benchmark. The grid fields on the same request are then required to be
/// empty, which is what makes it unambiguous which world a model is looking at.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GameRequest {
    /// Which game is being played.
    pub kind: GameKind,
    /// Which side the model is playing.
    pub side: GameSide,
    /// The board as this side sees it.
    pub board: GameBoard,
    /// Every move currently legal, in a fixed order.
    ///
    /// Listed explicitly rather than implied by the board so a model is never asked to infer the
    /// rules, and so the legal set is a value the request validator can check a chosen move against.
    pub legal_moves: Vec<GameMove>,
    /// This side's own earlier moves, oldest first, bounded exactly as the grid history is.
    ///
    /// Lives here rather than in `ModelRequest::history`, because that field carries a grid `Action`
    /// and a game move is not one. Leaving `history` empty for a game request is what lets the grid
    /// field keep its meaning untouched.
    pub history: Vec<GameMove>,
    /// One-based index of this ply within the match.
    pub ply: u32,
    /// Plies remaining before the match is abandoned.
    pub remaining_plies: u32,
    /// The move just played, by either side, when there was one.
    pub last_move: Option<GameMove>,
}

impl GameRequest {
    /// Validates the game half of a request.
    ///
    /// Separate from `ModelRequest::validate` so the rules stay readable, and called from it so a
    /// caller cannot get one without the other.
    pub fn validate(&self, kind: GameKind) -> Result<(), String> {
        if self.kind != kind {
            return Err(format!(
                "game request is for {:?} but the decision is for {kind:?}",
                self.kind
            ));
        }
        if self.ply == 0 {
            return Err("a game request must be at a one-based ply".to_owned());
        }
        if self.remaining_plies == 0 {
            return Err("a game request must have at least one ply remaining".to_owned());
        }
        if self.legal_moves.is_empty() {
            return Err("a game request must offer at least one legal move".to_owned());
        }
        for (index, entry) in self.history.iter().enumerate() {
            if entry.kind() != kind {
                return Err(format!(
                    "game history entry {index} is a {:?} move in a {kind:?} request",
                    entry.kind()
                ));
            }
        }
        if self.history.len() > GAME_HISTORY_WINDOW {
            return Err(format!(
                "game history must not exceed {GAME_HISTORY_WINDOW} entries, got {}",
                self.history.len()
            ));
        }
        if let Some(last) = self.last_move {
            if last.kind() != kind {
                return Err(format!(
                    "last move is a {:?} move in a {kind:?} request",
                    last.kind()
                ));
            }
        }
        Ok(())
    }
}

/// The bound on a game's own move history.
///
/// The same number as the grid history window, and for the same reason: it is part of the token-cost
/// contract, so a request's size is bounded no matter how long a match runs. It is a separate
/// constant rather than a reuse of `HISTORY_WINDOW` so that the two bounds can diverge if a game's
/// turns genuinely warrant a different window, without silently changing the grid one.
pub const GAME_HISTORY_WINDOW: usize = 8;
