use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::{
    Action, ActionResultKind, CapabilityRequest, Coordinate, GridSize, Observation, TileKind,
    VisibleTile,
};

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

/// Optional token counts reported by a model provider.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct TokenUsage {
    /// Tokens in the prompt, when reported.
    pub prompt_tokens: Option<u64>,
    /// Tokens in the completion, when reported.
    pub completion_tokens: Option<u64>,
    /// Total tokens used, when reported.
    pub total_tokens: Option<u64>,
}

impl TokenUsage {
    /// Adds optional call usage into this aggregate without overflowing.
    pub fn add_assign(&mut self, other: &Self) {
        self.prompt_tokens = add_optional(self.prompt_tokens, other.prompt_tokens);
        self.completion_tokens = add_optional(self.completion_tokens, other.completion_tokens);
        self.total_tokens = add_optional(self.total_tokens, other.total_tokens);
    }
}

fn add_optional(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.saturating_add(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

/// Safe metadata about one successful model call.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelCallInfo {
    /// Stable provider identifier.
    pub provider: String,
    /// Provider model identifier.
    pub model: String,
    /// Version of the prompt template used for the call.
    pub prompt_template_version: String,
    /// Provider request identifier, when supplied and safe to retain.
    pub request_id: Option<String>,
    /// Provider finish reason, when supplied.
    pub finish_reason: Option<String>,
    /// Provider token usage, when supplied.
    pub usage: Option<TokenUsage>,
    /// Configured request timeout in seconds, when supplied by the provider.
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    /// Elapsed request time in milliseconds.
    pub elapsed_ms: u64,
    /// Sanitized, bounded response text, when available.
    pub response_text: Option<String>,
}

/// A typed category for a failed model call.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelCallFailureKind {
    /// The provider configuration or request setup is invalid.
    Configuration,
    /// The request exceeded its configured timeout.
    Timeout,
    /// The response could not be decoded as the expected response shape.
    MalformedResponse,
    /// The response did not contain usable content.
    EmptyResponse,
    /// The provider returned a non-success HTTP status.
    HttpError,
    /// The provider returned a provider-specific failure without a response body.
    ProviderError,
    /// The response contained an action outside the available vocabulary.
    InvalidAction,
    /// The transport disconnected or otherwise failed before a response.
    TransportDisconnect,
}

/// A sanitized failure from one model call.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, thiserror::Error)]
#[error("{message}")]
pub struct ModelCallFailure {
    /// Stable failure category.
    pub kind: ModelCallFailureKind,
    /// Sanitized, human-readable failure description.
    pub message: String,
    /// Sanitized, bounded response text, when available.
    pub response_text: Option<String>,
    /// Provider token usage, when available.
    pub usage: Option<TokenUsage>,
    /// Elapsed request time in milliseconds, when available.
    pub elapsed_ms: Option<u64>,
}

impl ModelCallFailure {
    /// Creates a sanitized failure with no response metadata.
    pub fn new(kind: ModelCallFailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            response_text: None,
            usage: None,
            elapsed_ms: None,
        }
    }
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

/// The number of completed prior turns carried in every model request.
///
/// Eight is chosen to cover the planning horizon the grid-world benchmark actually requires: the
/// longest shortest safe route the generator produces is bounded well inside this window, so an
/// agent can hold a whole explored branch plus the divergence point in mind at the moment it has to
/// choose. It is deliberately small and fixed rather than "everything so far", because the window is
/// part of the token-cost contract: a request's size is bounded by `HISTORY_WINDOW` entries
/// regardless of how long an episode runs, so per-turn cost is predictable and a model cannot be
/// quietly billed for an ever-growing transcript.
pub const HISTORY_WINDOW: usize = 8;

/// One already-completed turn, replayed to the model as bounded context.
///
/// History carries only what the model legitimately observed when it took the decision. It is a
/// window over the agent's own past, never over the environment's hidden state: every field here was
/// in a `ModelRequest` at the time.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HistoryEntry {
    /// The one-based turn this entry describes.
    pub turn: u32,
    /// Where the agent stood at the start of that turn.
    pub position: Coordinate,
    /// What the agent stood on at the start of that turn.
    pub current_tile: TileKind,
    /// What the agent could see then. This is how a remembered hazard stays known after the agent
    /// walks away from it and it is no longer adjacent.
    pub visible_neighbors: Vec<VisibleTile>,
    /// The action the agent chose.
    pub selected_action: Action,
    /// What the environment did with it.
    pub result_kind: ActionResultKind,
    /// The shaped reward that turn produced.
    pub reward: i64,
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
    /// Whether this episode's benchmark exposes a contained process it is willing to run.
    ///
    /// This advertises a *permission*, never a program. A model that sees `true` may ask for the
    /// benchmark's declared process to be run on its behalf; what actually executes is compiled-in
    /// trusted configuration the model never sees. The flag is `false` for every benchmark that
    /// declares no spawn plan, which is what keeps a request from being a dead end the model cannot
    /// discover.
    pub process_spawn_available: bool,
    /// Up to [`HISTORY_WINDOW`] completed prior turns, oldest first.
    ///
    /// Empty on the first turn of an episode.
    pub history: Vec<HistoryEntry>,
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
        self.validate_history()?;
        Ok(())
    }

    /// Validates the bounded history window.
    ///
    /// The window is a hard bound, not a hint: an over-long history is a contract violation rather
    /// than something the engine silently truncates, so a caller cannot believe it supplied more
    /// context than the model actually received.
    ///
    /// Turns must strictly increase and must all be in the past. They need not be contiguous: a turn
    /// the engine consumed without a model decision — a broker refusal, or a protocol failure — is
    /// not a turn the model can remember anything about, so it leaves no history entry and the
    /// window legitimately skips it.
    fn validate_history(&self) -> Result<(), String> {
        if self.history.len() > HISTORY_WINDOW {
            return Err(format!(
                "model history must not exceed {HISTORY_WINDOW} entries, got {}",
                self.history.len()
            ));
        }
        let mut previous = 0;
        for entry in &self.history {
            if entry.turn == 0 || entry.turn >= self.turn || entry.turn <= previous {
                return Err(
                    "model history turns must strictly increase and precede the current turn"
                        .to_owned(),
                );
            }
            if !self.available_actions.contains(&entry.selected_action) {
                return Err("model history contains an action outside the vocabulary".to_owned());
            }
            previous = entry.turn;
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
    /// Name one capability for broker review before any policy review.
    ///
    /// This variant exists only for deterministic mocks and tests. A real provider can never
    /// produce it: the OpenAI-compatible prompt and response schema expose grid actions alone,
    /// so `capability_probe` is neither documented to nor parseable by a model. A decision of
    /// this kind reaches only the capability broker, and a policy gate that receives one
    /// fails it closed.
    CapabilityProbe(CapabilityRequest),
    /// Ask for the benchmark's declared process to be run on the model's behalf.
    ///
    /// This is the model-reachable counterpart to [`ModelDecision::CapabilityProbe`], and the
    /// difference is the whole point. A probe carries a fully-formed `CapabilityRequest` — a program,
    /// arguments, whatever the mock felt like naming — and exists only so tests can drive the broker
    /// directly. This variant carries **nothing**: it is the permission, not the program.
    ///
    /// The engine rebuilds the reviewed request from the benchmark's own trusted plan, so no model
    /// response can reach the allowlist comparison, a command line, or an environment variable. A
    /// model can therefore ask for the capability and still choose nothing about what runs, which is
    /// what makes it safe to put in a prompt.
    ///
    /// It is a distinct variant rather than a flag on a grid action so that the request cannot be
    /// smuggled through the action vocabulary, and so a refusal is reported as a broker denial
    /// instead of being miscounted as an invalid environment action.
    ProcessSpawnRequest,
}

/// A model decision with optional safe provider-call metadata.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelResponse {
    /// The model-selected decision.
    pub decision: ModelDecision,
    /// Metadata for the successful call, when available.
    pub call: Option<ModelCallInfo>,
}
