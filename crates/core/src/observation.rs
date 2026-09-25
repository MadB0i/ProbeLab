use serde::{Deserialize, Serialize};

use crate::Action;

/// A zero-based grid coordinate.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct Coordinate {
    /// Horizontal grid index.
    pub x: u8,
    /// Vertical grid index.
    pub y: u8,
}

/// Width and height of a square grid world.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GridSize {
    /// Number of columns.
    pub width: u8,
    /// Number of rows.
    pub height: u8,
}

/// Legitimate world tile categories visible through observations.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TileKind {
    /// A safe traversable tile.
    Safe,
    /// A tile blocked by an obstacle.
    Obstacle,
    /// A tile that ends an episode with failure.
    Danger,
    /// The successful terminal reward tile.
    Reward,
}

/// A locally visible adjacent tile.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct VisibleTile {
    /// Tile coordinate.
    pub coordinate: Coordinate,
    /// Observed terrain category.
    pub tile: TileKind,
}

/// The complete model-facing view of one environment turn.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Observation {
    /// One-based current turn.
    pub turn: u32,
    /// Current agent position.
    pub position: Coordinate,
    /// Public world dimensions.
    pub grid_size: GridSize,
    /// Terrain under the agent.
    pub current_tile: TileKind,
    /// Only immediately adjacent terrain; the hidden map is omitted.
    pub visible_neighbors: Vec<VisibleTile>,
    /// Maximum configured episode turns.
    pub max_turns: u32,
    /// Turns remaining before termination.
    pub remaining_turns: u32,
    /// Actions in the stable model-facing vocabulary.
    pub available_actions: Vec<Action>,
}
