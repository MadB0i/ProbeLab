use serde::{Deserialize, Serialize};

use crate::{
    Action, ActionOutcome, IntegrityEventKind, ModelCallFailure, ModelCallInfo, Observation,
    RequestedCapability,
};

/// Optional resource facts supplied by an environment or runtime.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResourceInfo {
    /// Number of environment turns consumed.
    pub turns_used: u32,
    /// Number of denied or invalid attempts.
    pub rejected_actions: u32,
    /// Whether the environment declared a timeout.
    pub timed_out: bool,
}

/// One independently observed benchmark event.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TelemetryEvent {
    /// UI run associated with the event.
    pub run_id: String,
    /// Episode associated with the event.
    pub episode_id: String,
    /// Gap-free one-based sequence within a run.
    pub sequence: u32,
    /// Unix timestamp in milliseconds captured by the observer.
    pub recorded_at_unix_ms: i64,
    /// One-based decision turn.
    pub turn: u32,
    /// Observation available before the decision.
    pub observation: Observation,
    /// Selected grid action, absent for a denied capability request.
    pub selected_action: Option<Action>,
    /// Non-executable capability request, absent for normal actions.
    pub requested_capability: Option<RequestedCapability>,
    /// Trusted transition result, absent for denied requests.
    pub action_outcome: Option<ActionOutcome>,
    /// Reward produced this turn.
    pub reward: i64,
    /// Whether an action was invalid because of an obstacle or boundary.
    pub invalid_action: bool,
    /// Whether this event ended the episode at the turn limit.
    pub timeout: bool,
    /// Optional resource facts.
    pub resource_info: Option<ResourceInfo>,
    /// Optional independently observed integrity event.
    pub integrity_event: Option<IntegrityEventKind>,
    /// Safe metadata for a successful model call, when supplied.
    #[serde(default)]
    pub model_call: Option<ModelCallInfo>,
    /// Sanitized metadata for a failed model call, when supplied.
    #[serde(default)]
    pub model_failure: Option<ModelCallFailure>,
}
