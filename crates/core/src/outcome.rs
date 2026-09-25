use serde::{Deserialize, Serialize};

use crate::{Action, Coordinate, Observation};

/// Why an episode is running or terminal.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EpisodeStatus {
    /// More turns are permitted.
    Running,
    /// The reward tile was reached.
    Success,
    /// A danger tile was reached.
    Failure,
    /// The configured turn limit ended the episode.
    TurnLimit,
}

/// Distinction between a valid transition and an invalid obstacle attempt.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionResultKind {
    /// The agent moved to a valid tile.
    Moved,
    /// The agent waited successfully.
    Waited,
    /// The action targeted an obstacle or boundary and consumed a turn.
    Invalid,
}

/// Immutable result of a sandboxed environment transition.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ActionOutcome {
    /// Requested action, absent for a policy-denied turn.
    pub action: Option<Action>,
    /// Transition classification.
    pub kind: ActionResultKind,
    /// Position after the transition.
    pub position: Coordinate,
    /// Reward from this transition.
    pub reward: i64,
    /// Observation after the transition when the episode continues.
    pub observation: Option<Observation>,
}

/// Trusted environment state plus public transition output.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StepOutcome {
    /// Current episode status.
    pub status: EpisodeStatus,
    /// Transition result, absent when policy denied the model decision.
    pub action_outcome: Option<ActionOutcome>,
}

/// Accumulated trusted facts used by the independent evaluator.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Episode {
    /// Stable episode identifier.
    pub episode_id: String,
    /// Environment seed.
    pub seed: u64,
    /// Current environment status.
    pub status: EpisodeStatus,
    /// Turns consumed.
    pub turns_used: u32,
    /// Sum of environment rewards.
    pub total_reward: i64,
    /// Invalid transition count.
    pub invalid_action_count: u32,
}
