use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::{
    Action, ActionResultKind, CapabilityRequest, Coordinate, GameKind, GameRequest, GridSize,
    Observation, TileKind, VisibleTile,
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
///
/// The categories are chosen to separate *the model went wrong* from *the harness could not reach the
/// model at all*, because those two are reported to the operator as different problems and collapsing
/// them produces a result bundle that blames a model for a network fault. Every variant below is one
/// an operator can act on differently.
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
    /// The endpoint actively refused the connection.
    ///
    /// Split from [`ModelCallFailureKind::TransportDisconnect`] because the two have opposite
    /// meanings and opposite fixes. A refused connection means nothing is listening on that port —
    /// the provider is not running, or the port is wrong, and the fix is on the operator's machine.
    /// A disconnect means something answered and then the connection broke mid-flight. Reporting
    /// both as one generic transport failure tells the operator nothing actionable.
    ConnectionRefused,
    /// The provider reported that the named model does not exist.
    ///
    /// Split out because it is by far the most common local-model mistake and it is invisible
    /// otherwise: a server that has never heard of `qwen2.5:3b` answers `404` with a small JSON body,
    /// which arrives as an anonymous `HttpError` and reads like a broken endpoint. This names the
    /// actual problem, which is that the model name is wrong or has not been pulled.
    ModelNotFound,
    /// The provider rejected the request because the prompt exceeded its context window.
    ///
    /// Distinct from a generic HTTP failure because it is a *budget* problem, not a *transport*
    /// problem, and because the remedy differs: a shorter history window or a smaller board, never a
    /// retry of the same request.
    ContextLengthExceeded,
}

impl ModelCallFailureKind {
    /// Whether this failure means the harness could not reach or could not use the provider at all.
    ///
    /// The distinction the whole enum exists to protect. A harness failure is not evidence about the
    /// model: the model never got to answer. Any statistic that counts these as model errors
    /// understates a model that was merely unreachable, and the arena's batch reporting has to keep
    /// them apart for that reason.
    pub fn is_harness_failure(self) -> bool {
        matches!(
            self,
            ModelCallFailureKind::Configuration
                | ModelCallFailureKind::Timeout
                | ModelCallFailureKind::TransportDisconnect
                | ModelCallFailureKind::ConnectionRefused
        )
    }

    /// Whether this failure is the model failing to answer in the required shape.
    ///
    /// The complement of [`is_harness_failure`](Self::is_harness_failure) for the cases where the
    /// provider did answer: a malformed body, an empty body, a hallucinated action, a model that is
    /// not installed, or a prompt that did not fit.
    pub fn is_model_failure(self) -> bool {
        !self.is_harness_failure()
    }
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
    /// The HTTP status the provider returned, when it returned one.
    ///
    /// Absent for a failure that never produced a response — a refused connection or a timeout have
    /// no status, and reporting `0` would be a fabricated datum. Kept separate from `message` so a
    /// caller can branch on it without parsing prose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
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
            http_status: None,
        }
    }

    /// Attaches the HTTP status a provider returned.
    pub fn with_status(mut self, status: u16) -> Self {
        self.http_status = Some(status);
        self
    }

    /// Attaches the elapsed time for a call that reached the transport.
    pub fn with_elapsed(mut self, elapsed: u64) -> Self {
        self.elapsed_ms = Some(elapsed);
        self
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

/// The grid size a request carries when it describes no grid at all.
///
/// A game has no position, no terrain, and no distance to a goal, but [`Observation`] is a struct with
/// no "absent" case. Rather than widen it into an enum — which would touch every field read in the
/// evaluator, the telemetry digest, and the desktop UI — a game request carries this one degenerate
/// value, and `validate` requires exactly it. That turns "a game request has some meaningless grid
/// numbers in it" from a silent hazard into a validation error.
pub const NO_GRID: GridSize = GridSize {
    width: 1,
    height: 1,
};

/// The position a request carries when it describes no grid.
pub const NO_GRID_POSITION: Coordinate = Coordinate { x: 0, y: 0 };

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
    /// Empty on the first turn of an episode. Also empty for a game request, which carries its own
    /// bounded move history on [`GameRequest::history`] because a game move is not a grid `Action`.
    pub history: Vec<HistoryEntry>,
    /// The game half of this request, when the benchmark is a game.
    ///
    /// `None` for every grid benchmark, which is what leaves the grid-world path and its published
    /// numbers untouched. When this is `Some`, the grid fields on this request are required to be
    /// empty, so it is never ambiguous which world a model is looking at.
    pub game: Option<GameRequest>,
}

impl ModelRequest {
    /// Validates that a request contains internally consistent, bounded model-facing data.
    pub fn validate(&self) -> Result<(), String> {
        // Which world this request belongs to is decided first, because the rest of the rules differ.
        // A request is one or the other, never a mixture, and saying so here is what stops a
        // half-populated game request from reaching a model as if it were a grid.
        match &self.game {
            // A game request is fully checked by its own rules; the grid half must be empty, which
            // `validate_game_request` requires rather than assumes.
            Some(game) => self.validate_game_request(game),
            None => self.validate_grid_request(),
        }
    }

    /// Validates the grid half, which is unchanged from before games existed.
    fn validate_grid_request(&self) -> Result<(), String> {
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

    /// Validates a request that carries a game.
    ///
    /// The grid half must be empty, and the observation must be the canonical "no grid" one. That is
    /// stricter than it looks and deliberate: `Observation` is a struct with no variant for "there is
    /// no board here", so a game benchmark has to put *something* in it. Requiring the one degenerate
    /// value means a grid field can never carry a plausible-looking but meaningless position into a
    /// game prompt, where a model would have no way to know it was noise.
    fn validate_game_request(&self, game: &GameRequest) -> Result<(), String> {
        if !self.available_actions.is_empty() {
            return Err(
                "a game request must not carry a grid action vocabulary; the two shapes are exclusive"
                    .to_owned(),
            );
        }
        if !self.history.is_empty() {
            return Err(
                "a game request must not carry grid history; game moves live on the game request"
                    .to_owned(),
            );
        }
        if self.observation.grid_size != NO_GRID {
            return Err(format!(
                "a game request must carry the no-grid observation, got {}x{}",
                self.observation.grid_size.width, self.observation.grid_size.height
            ));
        }
        if self.observation.position != NO_GRID_POSITION {
            return Err("a game request must carry the no-grid position".to_owned());
        }
        if !self.observation.visible_neighbors.is_empty() {
            return Err("a game request must not carry grid neighbours".to_owned());
        }
        if self.observation.current_tile != TileKind::Safe {
            return Err("a game request must not carry grid terrain".to_owned());
        }
        if self.observation.distance_to_goal != 0 {
            return Err("a game request must not carry a distance to a goal".to_owned());
        }
        if self.turn != game.ply {
            return Err(format!(
                "model request turn {} does not match the game ply {}",
                self.turn, game.ply
            ));
        }
        if self.observation.turn != game.ply {
            return Err("observation turn does not match the game ply".to_owned());
        }
        game.validate(self.game_kind()?)?;
        Ok(())
    }

    /// The game kind this request is for, for validating a decision against it.
    fn game_kind(&self) -> Result<GameKind, String> {
        self.game
            .as_ref()
            .map(|game| game.kind)
            .ok_or_else(|| "this request carries no game".to_owned())
    }

    /// Validates that a decision is legal for the request it answers.
    ///
    /// The counterpart to [`ModelRequest::validate`], and the reason a `GameMove` is safe to put in a
    /// prompt. A grid action is checked against the request's vocabulary by the engine; a game move
    /// has to be checked against the game's own legal set here, because `Action` cannot express it.
    pub fn validate_decision(&self, decision: &ModelDecision) -> Result<(), String> {
        match (decision, &self.game) {
            (ModelDecision::GameMove(_), None) => {
                Err("a game move was returned for a request that offered no game".to_owned())
            }
            (ModelDecision::GameMove(m), Some(game)) => {
                if m.kind() != game.kind {
                    return Err(format!(
                        "a {:?} move was returned for a {:?} game",
                        m.kind(),
                        game.kind
                    ));
                }
                if !game.legal_moves.contains(m) {
                    return Err(format!("{m:?} is not among the legal moves for this turn"));
                }
                Ok(())
            }
            // A capability request carries no game payload and is judged by the broker, not here.
            (ModelDecision::Capability(_) | ModelDecision::CapabilityProbe(_), _) => Ok(()),
            (ModelDecision::ProcessSpawnRequest, Some(_)) => {
                Err("a contained-process request is meaningless in a game episode".to_owned())
            }
            (ModelDecision::ProcessSpawnRequest, None) => Ok(()),
            // A grid action is validated against the grid vocabulary by the engine's existing check.
            (ModelDecision::GridAction(_), _) => Ok(()),
        }
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
    /// Play one move in the game this request offered.
    ///
    /// Valid only when [`ModelRequest::game`] is `Some`, and the move must be among that request's
    /// legal moves. Both are enforced by [`ModelRequest::validate_decision`], not by convention, so a
    /// provider cannot return an illegal board play and have it applied.
    GameMove(crate::GameMove),
}

/// A model decision with optional safe provider-call metadata.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelResponse {
    /// The model-selected decision.
    pub decision: ModelDecision,
    /// Metadata for the successful call, when available.
    pub call: Option<ModelCallInfo>,
}
