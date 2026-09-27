use serde::{Deserialize, Serialize};

use crate::{
    Action, ActionOutcome, CapabilityDenial, GameBoard, GameKind, GameMove, GameSide,
    IntegrityEventKind, ModelCallFailure, ModelCallInfo, Observation, ProcessLifecycle,
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
    /// The game board and the move played, absent for every grid benchmark.
    ///
    /// `skip_serializing_if` is load-bearing rather than cosmetic. This struct is hashed into the
    /// replay digest, and an `Option` that always serialised would write `null` into every grid event
    /// and change the digest of every existing run. Omitting the key entirely when it is absent leaves
    /// grid-world bytes exactly as they were, which is what lets the published numbers stay valid
    /// across this change. A test asserts that byte-for-byte.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game: Option<GameTelemetry>,
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
    /// Capability-broker refusal, when the trusted broker denied a named capability.
    ///
    /// A broker denial is non-executable: such an event carries no selected action, no
    /// requested capability, no action outcome, no reward, and no invalid-action flag.
    #[serde(default)]
    pub broker_denial: Option<CapabilityDenial>,
    /// The lifecycle of a requested process spawn, when one was requested on this turn.
    ///
    /// Present only for a spawn that reached the containment layer. A refusal that never got that
    /// far is recorded as a `broker_denial` instead, so the two cases stay distinguishable.
    #[serde(default)]
    pub process_lifecycle: Option<ProcessLifecycle>,
}

/// The game half of a telemetry event.
///
/// Enough for a live match view to draw a board and both players' moves without reconstructing state
/// or reaching back into the engine, which is the whole reason the arena needs a field here. Carries
/// the board *after* the move was applied, so a viewer can render each event as the next frame rather
/// than replaying the game to work out where it got to.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GameTelemetry {
    /// Which game was played.
    pub kind: GameKind,
    /// Which side the model was playing this ply.
    pub side: GameSide,
    /// The board after this ply.
    pub board: GameBoard,
    /// The move that was played, absent on a ply the model did not get to move on.
    pub played: Option<GameMove>,
    /// One-based index of this ply.
    pub ply: u32,
}
