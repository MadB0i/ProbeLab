use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::{Action, GridSize, Observation};

/// Stable, serializable description of a model adapter configuration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelDescriptor {
    /// Adapter identifier, such as `deterministic-mock-v1`.
    pub adapter: String,
    /// Named strategy within the adapter.
    pub strategy: String,
    /// Seed controlling every random choice in the strategy.
    pub seed: u64,
    /// Explicit strategy parameters.
    pub parameters: BTreeMap<String, serde_json::Value>,
}

/// A capability request that is not executable in the grid-world sandbox.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "target")]
pub enum RequestedCapability {
    /// Attempt to read trusted evaluator or benchmark implementation.
    EvaluatorAccess,
    /// Attempt to invoke a disallowed tool.
    ToolUse(String),
    /// Attempt to leave the controlled sandbox.
    SandboxEscape,
    /// Attempt to modify trusted environment state directly.
    EnvironmentMutation(String),
    /// Attempt network access.
    NetworkAccess(String),
}

/// The only context a model provider receives.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelRequest {
    /// Stable benchmark episode identifier.
    pub episode_id: String,
    /// One-based decision turn.
    pub turn: u32,
    /// Legitimate environment observation.
    pub observation: Observation,
    /// Actions accepted by the model-facing action vocabulary.
    pub available_actions: Vec<Action>,
}

impl ModelRequest {
    /// Validates that a request contains internally consistent, bounded model-facing data.
    pub fn validate(&self) -> Result<(), String> {
        if self.turn == 0 || self.turn != self.observation.turn {
            return Err(
                "model request and observation turns must match and be one-based".to_owned(),
            );
        }
        validate_grid_size(self.observation.grid_size)?;
        if usize::from(self.observation.position.x) >= usize::from(self.observation.grid_size.width)
            || usize::from(self.observation.position.y)
                >= usize::from(self.observation.grid_size.height)
        {
            return Err("model observation position is outside the grid".to_owned());
        }
        if self.available_actions != self.observation.available_actions {
            return Err("model and observation action vocabularies do not match".to_owned());
        }
        if self.observation.max_turns == 0
            || self.observation.remaining_turns == 0
            || self.observation.remaining_turns > self.observation.max_turns
            || self.observation.remaining_turns
                != self.observation.max_turns.saturating_sub(self.turn - 1)
        {
            return Err("model observation has invalid turn budget".to_owned());
        }
        for neighbor in &self.observation.visible_neighbors {
            if usize::from(neighbor.coordinate.x) >= usize::from(self.observation.grid_size.width)
                || usize::from(neighbor.coordinate.y)
                    >= usize::from(self.observation.grid_size.height)
            {
                return Err("model observation contains an out-of-bounds neighbor".to_owned());
            }
            let horizontal = self.observation.position.x.abs_diff(neighbor.coordinate.x);
            let vertical = self.observation.position.y.abs_diff(neighbor.coordinate.y);
            if horizontal + vertical != 1 {
                return Err("model observation contains a non-adjacent tile".to_owned());
            }
        }
        Ok(())
    }
}

fn validate_grid_size(grid_size: GridSize) -> Result<(), String> {
    if grid_size.width == 0 || grid_size.height == 0 {
        Err("model observation grid dimensions must be non-zero".to_owned())
    } else {
        Ok(())
    }
}

/// A model decision that must pass policy and runtime authorization.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "action")]
pub enum ModelDecision {
    /// Request a grid-world action.
    GridAction(Action),
    /// Request a capability unavailable to the sandbox.
    Capability(RequestedCapability),
}
