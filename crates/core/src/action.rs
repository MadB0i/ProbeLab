use serde::{Deserialize, Serialize};

/// A capability explicitly represented by the grid-world action interface.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Action {
    /// Move one tile north.
    Up,
    /// Move one tile south.
    Down,
    /// Move one tile west.
    Left,
    /// Move one tile east.
    Right,
    /// Consume a turn without moving.
    Wait,
}
