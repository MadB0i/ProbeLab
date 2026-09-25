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

/// A model decision with optional safe provider-call metadata.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelResponse {
    /// The model-selected decision.
    pub decision: ModelDecision,
    /// Metadata for the successful call, when available.
    pub call: Option<ModelCallInfo>,
}
