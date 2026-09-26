//! Deterministic model providers for the ProbeLab model boundary.

#![deny(missing_docs)]

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use probelab_core::{
    Action, AsyncModelProvider, CapabilityRequest, Coordinate, GridSize, ModelCallFailure,
    ModelCallFailureKind, ModelCallInfo, ModelDecision, ModelDescriptor, ModelProvider,
    ModelProviderFuture, ModelRequest, ModelResponse, RequestedCapability, TileKind, TokenUsage,
    VisibleTile,
};
use rand::Rng;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use reqwest::redirect::Policy;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::{Host, Url};

/// Version of the embedded grid-world prompt template.
pub const GRID_WORLD_PROMPT_TEMPLATE_VERSION: &str = "grid_world_v1";

/// Short alias for the embedded grid-world prompt template version.
pub const PROMPT_TEMPLATE_VERSION: &str = GRID_WORLD_PROMPT_TEMPLATE_VERSION;

/// Version of the trusted grid-world environment recorded for reproducibility.
pub const GRID_WORLD_ENVIRONMENT_VERSION: &str = "grid_world_environment_v1";

/// Stable native prompt version used by deterministic mock providers.
pub const MOCK_PROMPT_TEMPLATE_VERSION: &str = "deterministic_mock_v1";

/// Version of the benchmark included in the adapter configuration fingerprint.
pub const GRID_WORLD_BENCHMARK_VERSION: &str = "1";

/// Configuration for an OpenAI-compatible chat-completions provider.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OpenAiCompatibleConfig {
    /// Base URL or complete chat-completions endpoint.
    pub base_url: String,
    /// Name of the environment variable containing the API key.
    pub api_key_env: String,
    /// Model identifier sent to the provider.
    pub model: String,
    /// Request timeout in seconds.
    pub timeout_seconds: u64,
    /// Sampling temperature sent to the provider.
    pub temperature: f64,
    /// Maximum completion tokens sent to the provider.
    pub max_tokens: u32,
}

impl Default for OpenAiCompatibleConfig {
    fn default() -> Self {
        Self {
            base_url: "http://127.0.0.1:11434/v1".to_owned(),
            api_key_env: "OPENAI_COMPATIBLE_API_KEY".to_owned(),
            model: "local-model".to_owned(),
            timeout_seconds: 30,
            temperature: 0.0,
            max_tokens: 200,
        }
    }
}

/// A typed configuration validation or client-construction failure.
#[derive(Debug, thiserror::Error)]
pub enum OpenAiCompatibleConfigError {
    /// The base URL was empty.
    #[error("base_url must not be empty")]
    EmptyBaseUrl,
    /// The base URL was not an absolute URL with a host.
    #[error("base_url must be a valid absolute URL with a host")]
    InvalidBaseUrl,
    /// The base URL contained user information, which could embed a key.
    #[error("base_url must not contain user information")]
    UrlUserInfo,
    /// The base URL contained query parameters.
    #[error("base_url must not contain query parameters")]
    UrlQuery,
    /// The base URL contained a fragment.
    #[error("base_url must not contain a fragment")]
    UrlFragment,
    /// The base URL was not HTTPS and was not loopback HTTP.
    #[error("base_url must use HTTPS except for loopback HTTP")]
    InsecureBaseUrl,
    /// The API key environment-variable name was empty.
    #[error("api_key_env must not be empty")]
    EmptyApiKeyEnv,
    /// The API key environment-variable name was not a simple name.
    #[error("api_key_env must contain only letters, digits, and underscores")]
    InvalidApiKeyEnv,
    /// The model identifier was empty.
    #[error("model must not be empty")]
    EmptyModel,
    /// The timeout was zero.
    #[error("timeout_seconds must be greater than zero")]
    InvalidTimeout,
    /// The temperature was not finite or was outside the provider range.
    #[error("temperature must be finite and between 0 and 2")]
    InvalidTemperature,
    /// The completion limit was zero.
    #[error("max_tokens must be greater than zero")]
    InvalidMaxTokens,
    /// The HTTP client could not be constructed.
    #[error("failed to construct the HTTP client")]
    ClientConstruction,
}

impl OpenAiCompatibleConfig {
    /// Validates URL, credential-name, model, timeout, and generation settings.
    pub fn validate(&self) -> Result<(), OpenAiCompatibleConfigError> {
        self.validated_url()?;
        if self.api_key_env.is_empty()
            || !self
                .api_key_env
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_')
            || self
                .api_key_env
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_digit)
        {
            return if self.api_key_env.is_empty() {
                Err(OpenAiCompatibleConfigError::EmptyApiKeyEnv)
            } else {
                Err(OpenAiCompatibleConfigError::InvalidApiKeyEnv)
            };
        }
        if self.model.trim().is_empty() {
            return Err(OpenAiCompatibleConfigError::EmptyModel);
        }
        if self.timeout_seconds == 0 {
            return Err(OpenAiCompatibleConfigError::InvalidTimeout);
        }
        if !self.temperature.is_finite() || !(0.0..=2.0).contains(&self.temperature) {
            return Err(OpenAiCompatibleConfigError::InvalidTemperature);
        }
        if self.max_tokens == 0 {
            return Err(OpenAiCompatibleConfigError::InvalidMaxTokens);
        }
        Ok(())
    }

    /// Resolves the configured base URL to a chat-completions endpoint.
    pub fn endpoint(&self) -> Result<Url, OpenAiCompatibleConfigError> {
        let mut endpoint = self.validated_url()?;
        let path = endpoint.path().trim_end_matches('/');
        let endpoint_path = if path.ends_with("/chat/completions") {
            path.to_owned()
        } else {
            format!("{path}/chat/completions")
        };
        endpoint.set_path(&endpoint_path);
        Ok(endpoint)
    }

    fn validated_url(&self) -> Result<Url, OpenAiCompatibleConfigError> {
        if self.base_url.trim().is_empty() {
            return Err(OpenAiCompatibleConfigError::EmptyBaseUrl);
        }
        let url =
            Url::parse(&self.base_url).map_err(|_| OpenAiCompatibleConfigError::InvalidBaseUrl)?;
        if url.host().is_none() {
            return Err(OpenAiCompatibleConfigError::InvalidBaseUrl);
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(OpenAiCompatibleConfigError::UrlUserInfo);
        }
        if url.query().is_some() {
            return Err(OpenAiCompatibleConfigError::UrlQuery);
        }
        if url.fragment().is_some() {
            return Err(OpenAiCompatibleConfigError::UrlFragment);
        }
        let secure = url.scheme() == "https";
        let loopback_http = url.scheme() == "http" && is_loopback_url(&url);
        if !secure && !loopback_http {
            return Err(OpenAiCompatibleConfigError::InsecureBaseUrl);
        }
        Ok(url)
    }
}

fn is_loopback_url(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(address)) => IpAddr::from(address).is_loopback(),
        Some(Host::Ipv6(address)) => IpAddr::from(address).is_loopback(),
        None => false,
    }
}

/// A typed protocol failure returned while parsing a model action.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ActionProtocolError {
    /// The response contained no content.
    #[error("action response is empty")]
    Empty,
    /// The response was not one well-formed JSON object.
    #[error("action response is not one valid JSON object")]
    Malformed,
    /// The response was a JSON array.
    #[error("action response must be a JSON object")]
    Array,
    /// The object did not contain an action field.
    #[error("action response is missing the action field")]
    MissingAction,
    /// The object contained a field other than action.
    #[error("action response contains an unknown field")]
    UnknownField,
    /// More than one JSON value was present.
    #[error("action response contains multiple JSON values")]
    MultipleValues,
    /// The action name was not one of the five supported names.
    #[error("action name is not supported")]
    UnknownAction,
    /// The action was supported but was not available for this request.
    #[error("action is not in the available action vocabulary")]
    OutOfVocabulary,
}

/// Alias exposing the protocol error kind under an explicit name.
pub type ActionProtocolErrorKind = ActionProtocolError;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ActionObject {
    action: String,
}

/// Parses exactly one action object and maps it to an available domain action.
pub fn parse_action_response(
    raw: &str,
    available_actions: &[Action],
) -> Result<Action, ActionProtocolError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ActionProtocolError::Empty);
    }
    if trimmed.starts_with('[') {
        return Err(ActionProtocolError::Array);
    }
    let Some(start) = trimmed.find('{') else {
        return Err(ActionProtocolError::Malformed);
    };
    let Some(end) = json_object_end(trimmed, start) else {
        return Err(ActionProtocolError::Malformed);
    };
    let suffix = trimmed[end + 1..].trim();
    if has_structured_json_prefix(suffix) {
        return Err(ActionProtocolError::MultipleValues);
    }
    let object = serde_json::from_str::<ActionObject>(&trimmed[start..=end]).map_err(|error| {
        if error.to_string().contains("missing field `action`") {
            ActionProtocolError::MissingAction
        } else if error.to_string().contains("unknown field") {
            ActionProtocolError::UnknownField
        } else {
            ActionProtocolError::Malformed
        }
    })?;
    let normalized = object.action.to_ascii_uppercase();
    let action = match normalized.as_str() {
        "MOVE_UP" => Action::Up,
        "MOVE_DOWN" => Action::Down,
        "MOVE_LEFT" => Action::Left,
        "MOVE_RIGHT" => Action::Right,
        "WAIT" => Action::Wait,
        _ => return Err(ActionProtocolError::UnknownAction),
    };
    if available_actions.contains(&action) {
        Ok(action)
    } else {
        Err(ActionProtocolError::OutOfVocabulary)
    }
}

fn json_object_end(input: &str, start: usize) -> Option<usize> {
    let bytes = input.as_bytes();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (index, byte) in bytes.iter().enumerate().skip(start) {
        let byte = *byte;
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

fn has_structured_json_prefix(input: &str) -> bool {
    let input = input.trim_start();
    input.contains('{')
        || input.contains('[')
        || input.starts_with('"')
        || input.starts_with('-')
        || input.starts_with(|character: char| character.is_ascii_digit())
        || input.starts_with("true")
        || input.starts_with("false")
        || input.starts_with("null")
}

#[derive(Serialize)]
struct PromptObservation<'a> {
    turn: u32,
    position: &'a Coordinate,
    grid_size: GridSize,
    current_tile: TileKind,
    visible_neighbors: &'a [VisibleTile],
    max_turns: u32,
    remaining_turns: u32,
    available_actions: Vec<&'static str>,
}

#[derive(Serialize)]
struct PromptRequest<'a> {
    episode_id: &'a str,
    turn: u32,
    observation: PromptObservation<'a>,
    available_actions: Vec<&'static str>,
}

impl<'a> From<&'a ModelRequest> for PromptRequest<'a> {
    fn from(request: &'a ModelRequest) -> Self {
        Self {
            episode_id: &request.episode_id,
            turn: request.turn,
            observation: PromptObservation {
                turn: request.observation.turn,
                position: &request.observation.position,
                grid_size: request.observation.grid_size,
                current_tile: request.observation.current_tile,
                visible_neighbors: &request.observation.visible_neighbors,
                max_turns: request.observation.max_turns,
                remaining_turns: request.observation.remaining_turns,
                available_actions: request
                    .observation
                    .available_actions
                    .iter()
                    .map(|action| protocol_action_name(*action))
                    .collect(),
            },
            available_actions: request
                .available_actions
                .iter()
                .map(|action| protocol_action_name(*action))
                .collect(),
        }
    }
}

fn protocol_action_name(action: Action) -> &'static str {
    match action {
        Action::Up => "MOVE_UP",
        Action::Down => "MOVE_DOWN",
        Action::Left => "MOVE_LEFT",
        Action::Right => "MOVE_RIGHT",
        Action::Wait => "WAIT",
    }
}

#[derive(Serialize)]
struct FingerprintInput<'a> {
    model: &'a str,
    temperature: f64,
    max_tokens: u32,
    timeout_seconds: u64,
    prompt_template_version: &'static str,
    benchmark_version: &'static str,
}

/// Hashes only model and benchmark-affecting provider settings.
pub fn configuration_fingerprint(config: &OpenAiCompatibleConfig) -> String {
    let input = FingerprintInput {
        model: &config.model,
        temperature: config.temperature,
        max_tokens: config.max_tokens,
        timeout_seconds: config.timeout_seconds,
        prompt_template_version: GRID_WORLD_PROMPT_TEMPLATE_VERSION,
        benchmark_version: GRID_WORLD_BENCHMARK_VERSION,
    };
    let bytes = serde_json::to_vec(&input).expect("fingerprint input is serializable");
    let digest = Sha256::digest(bytes);
    format!("{digest:x}")
}

const GRID_WORLD_PROMPT: &str = include_str!("../../../prompts/grid_world_v1.md");

/// An OpenAI-compatible asynchronous chat-completions provider.
pub struct OpenAiCompatibleProvider {
    config: OpenAiCompatibleConfig,
    client: Client,
}

impl OpenAiCompatibleProvider {
    /// Validates configuration and creates a redirect-disabled HTTP provider.
    pub fn new(config: OpenAiCompatibleConfig) -> Result<Self, OpenAiCompatibleConfigError> {
        config.validate()?;
        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout_seconds))
            .redirect(Policy::none())
            .build()
            .map_err(|_| OpenAiCompatibleConfigError::ClientConstruction)?;
        Ok(Self { config, client })
    }

    /// Returns the validated provider configuration without credentials.
    pub fn config(&self) -> &OpenAiCompatibleConfig {
        &self.config
    }

    /// Returns a descriptor that contains no endpoint, environment name, or key.
    pub fn descriptor_value(&self) -> ModelDescriptor {
        let mut parameters = BTreeMap::new();
        parameters.insert(
            "provider".to_owned(),
            serde_json::Value::String("openai-compatible".to_owned()),
        );
        parameters.insert(
            "model".to_owned(),
            serde_json::Value::String(self.config.model.clone()),
        );
        parameters.insert(
            "configuration_fingerprint".to_owned(),
            serde_json::Value::String(configuration_fingerprint(&self.config)),
        );
        parameters.insert(
            "prompt_template_version".to_owned(),
            serde_json::Value::String(GRID_WORLD_PROMPT_TEMPLATE_VERSION.to_owned()),
        );
        parameters.insert(
            "environment_version".to_owned(),
            serde_json::Value::String(GRID_WORLD_ENVIRONMENT_VERSION.to_owned()),
        );

        ModelDescriptor {
            adapter: "openai-compatible-v1".to_owned(),
            strategy: self.config.model.clone(),
            seed: 0,
            parameters,
        }
    }

    async fn predict_async(
        &self,
        request: ModelRequest,
    ) -> Result<ModelResponse, ModelCallFailure> {
        request.validate().map_err(|_| {
            ModelCallFailure::new(
                ModelCallFailureKind::Configuration,
                "model request validation failed",
            )
        })?;
        let api_key = std::env::var(&self.config.api_key_env).map_err(|_| {
            ModelCallFailure::new(
                ModelCallFailureKind::Configuration,
                "provider API key environment variable is not set",
            )
        })?;
        if api_key.is_empty() {
            return Err(ModelCallFailure::new(
                ModelCallFailureKind::Configuration,
                "provider API key environment variable is empty",
            ));
        }
        let request_json = serde_json::to_string(&PromptRequest::from(&request)).map_err(|_| {
            ModelCallFailure::new(
                ModelCallFailureKind::Configuration,
                "model request could not be serialized",
            )
        })?;
        let endpoint = self.config.endpoint().map_err(|_| {
            ModelCallFailure::new(
                ModelCallFailureKind::Configuration,
                "provider endpoint is invalid",
            )
        })?;
        let body = serde_json::json!({
            "model": self.config.model,
            "temperature": self.config.temperature,
            "max_tokens": self.config.max_tokens,
            "stream": false,
            "messages": [
                {"role": "system", "content": GRID_WORLD_PROMPT},
                {"role": "user", "content": request_json},
            ],
        });
        let started = Instant::now();
        let response = self
            .client
            .post(endpoint)
            .bearer_auth(&api_key)
            .json(&body)
            .send()
            .await;
        let response = match response {
            Ok(response) => response,
            Err(error) if error.is_timeout() => {
                return Err(ModelCallFailure {
                    kind: ModelCallFailureKind::Timeout,
                    message: "provider request timed out".to_owned(),
                    response_text: None,
                    usage: None,
                    elapsed_ms: Some(elapsed_ms(started)),
                });
            }
            Err(_) => {
                return Err(ModelCallFailure {
                    kind: ModelCallFailureKind::TransportDisconnect,
                    message: "provider transport failed before a response".to_owned(),
                    response_text: None,
                    usage: None,
                    elapsed_ms: Some(elapsed_ms(started)),
                });
            }
        };
        let request_id = response
            .headers()
            .get("x-request-id")
            .and_then(|value| value.to_str().ok())
            .map(|value| sanitize_response_text(value, &api_key));
        let status = response.status();
        if !status.is_success() {
            return Err(ModelCallFailure {
                kind: ModelCallFailureKind::HttpError,
                message: "provider returned a non-success HTTP status".to_owned(),
                response_text: None,
                usage: None,
                elapsed_ms: Some(elapsed_ms(started)),
            });
        }
        let response_text = response.text().await.map_err(|_| ModelCallFailure {
            kind: ModelCallFailureKind::TransportDisconnect,
            message: "provider response transport failed".to_owned(),
            response_text: None,
            usage: None,
            elapsed_ms: Some(elapsed_ms(started)),
        })?;
        let sanitized_response = sanitize_response_text(&response_text, &api_key);
        let elapsed = elapsed_ms(started);
        let payload = match serde_json::from_str::<ChatResponse>(&response_text) {
            Ok(payload) => payload,
            Err(_) => {
                return Err(ModelCallFailure {
                    kind: ModelCallFailureKind::MalformedResponse,
                    message: "provider response JSON is malformed".to_owned(),
                    response_text: Some(sanitized_response),
                    usage: None,
                    elapsed_ms: Some(elapsed),
                });
            }
        };
        let usage = payload.usage.map(convert_usage);
        let Some(choice) = payload.choices.into_iter().next() else {
            return Err(ModelCallFailure {
                kind: ModelCallFailureKind::EmptyResponse,
                message: "provider response contained no choices".to_owned(),
                response_text: Some(sanitized_response),
                usage,
                elapsed_ms: Some(elapsed),
            });
        };
        let Some(content) = choice
            .message
            .content
            .filter(|value| !value.trim().is_empty())
        else {
            return Err(ModelCallFailure {
                kind: ModelCallFailureKind::EmptyResponse,
                message: "provider response contained no message content".to_owned(),
                response_text: Some(sanitized_response),
                usage,
                elapsed_ms: Some(elapsed),
            });
        };
        let sanitized_content = sanitize_response_text(&content, &api_key);
        let action = match parse_action_response(&content, &request.available_actions) {
            Ok(action) => action,
            Err(error) => {
                let kind = match error {
                    ActionProtocolError::Empty => ModelCallFailureKind::EmptyResponse,
                    ActionProtocolError::Malformed
                    | ActionProtocolError::Array
                    | ActionProtocolError::MissingAction
                    | ActionProtocolError::UnknownField
                    | ActionProtocolError::MultipleValues => {
                        ModelCallFailureKind::MalformedResponse
                    }
                    ActionProtocolError::UnknownAction | ActionProtocolError::OutOfVocabulary => {
                        ModelCallFailureKind::InvalidAction
                    }
                };
                return Err(ModelCallFailure {
                    kind,
                    message: "provider response did not follow the action protocol".to_owned(),
                    response_text: Some(sanitized_content),
                    usage,
                    elapsed_ms: Some(elapsed),
                });
            }
        };
        Ok(ModelResponse {
            decision: ModelDecision::GridAction(action),
            call: Some(ModelCallInfo {
                provider: "openai-compatible".to_owned(),
                model: self.config.model.clone(),
                prompt_template_version: GRID_WORLD_PROMPT_TEMPLATE_VERSION.to_owned(),
                request_id,
                finish_reason: choice
                    .finish_reason
                    .map(|reason| sanitize_response_text(&reason, &api_key)),
                usage,
                timeout_seconds: Some(self.config.timeout_seconds),
                elapsed_ms: elapsed,
                response_text: Some(sanitized_content),
            }),
        })
    }
}

impl AsyncModelProvider for OpenAiCompatibleProvider {
    fn descriptor(&self) -> ModelDescriptor {
        self.descriptor_value()
    }

    fn predict(&self, request: ModelRequest) -> ModelProviderFuture<'_> {
        Box::pin(self.predict_async(request))
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

fn sanitize_response_text(value: &str, api_key: &str) -> String {
    let bounded = value.chars().take(2048).collect::<String>();
    if api_key.is_empty() {
        bounded
    } else {
        bounded.replace(api_key, "[REDACTED]")
    }
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
    usage: Option<ChatUsage>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatMessage,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct ChatMessage {
    content: Option<String>,
}

#[derive(Deserialize)]
struct ChatUsage {
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    total_tokens: Option<u64>,
}

fn convert_usage(usage: ChatUsage) -> TokenUsage {
    TokenUsage {
        prompt_tokens: usage.prompt_tokens,
        completion_tokens: usage.completion_tokens,
        total_tokens: usage.total_tokens,
    }
}

/// A deterministic strategy configuration for [`MockModelProvider`].
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "strategy")]
pub enum MockModelConfig {
    /// Chooses safe monotonic moves using only the current local observation.
    Optimal,
    /// Chooses an available action using a seeded ChaCha8 generator.
    Random,
    /// Waits on the first turn and then behaves optimally.
    GreedyMistake,
    /// Repeatedly chooses an action that targets a boundary from the local observation.
    InvalidActionSpam,
    /// Requests unauthorized evaluator access.
    #[serde(alias = "security_probe_unauthorized_evaluator_access")]
    SecurityProbeUnauthorizedAccess,
    /// Requests a disallowed tool.
    #[serde(alias = "security_probe_disallowed_tool")]
    SecurityProbePolicyViolation,
    /// Requests a sandbox escape.
    #[serde(alias = "security_probe_sandbox_escape")]
    SecurityProbeSandboxViolation,
    /// Requests direct environment mutation.
    SecurityProbeEnvironmentTampering,
    /// Requests network access.
    #[serde(alias = "security_probe_network")]
    SecurityProbeNetworkViolation,
    /// Names one forbidden capability per permission in a deterministic cycle, then waits.
    ///
    /// This is a test strategy only. It emits internal
    /// [`probelab_core::ModelDecision::CapabilityProbe`] decisions, which no real provider
    /// prompt or response schema exposes, and every one of them is refused by the
    /// deny-by-default capability broker.
    ForbiddenActionSpam,
}

impl MockModelConfig {
    /// Returns the stable strategy name used in a model descriptor.
    pub fn strategy_name(self) -> &'static str {
        match self {
            Self::Optimal => "optimal",
            Self::Random => "random",
            Self::GreedyMistake => "greedy_mistake",
            Self::InvalidActionSpam => "invalid_action_spam",
            Self::SecurityProbeUnauthorizedAccess => "security_probe_unauthorized_access",
            Self::SecurityProbePolicyViolation => "security_probe_policy_violation",
            Self::SecurityProbeSandboxViolation => "security_probe_sandbox_violation",
            Self::SecurityProbeEnvironmentTampering => "security_probe_environment_tampering",
            Self::SecurityProbeNetworkViolation => "security_probe_network_violation",
            Self::ForbiddenActionSpam => "forbidden_action_spam",
        }
    }
}

fn mock_configuration_fingerprint(config: MockModelConfig, seed: u64) -> String {
    #[derive(Serialize)]
    struct FingerprintInput {
        config: MockModelConfig,
        seed: u64,
        prompt_template_version: &'static str,
        benchmark_version: &'static str,
        environment_version: &'static str,
    }

    let input = FingerprintInput {
        config,
        seed,
        prompt_template_version: MOCK_PROMPT_TEMPLATE_VERSION,
        benchmark_version: GRID_WORLD_BENCHMARK_VERSION,
        environment_version: GRID_WORLD_ENVIRONMENT_VERSION,
    };
    let bytes = serde_json::to_vec(&input).expect("mock fingerprint input is serializable");
    let digest = Sha256::digest(bytes);
    format!("{digest:x}")
}

/// A deterministic provider that implements the core model-provider boundary.
pub struct MockModelProvider {
    config: MockModelConfig,
    seed: u64,
    rng: ChaCha8Rng,
}

impl MockModelProvider {
    /// Creates a provider with a fixed strategy and deterministic random seed.
    pub fn new(config: MockModelConfig, seed: u64) -> Self {
        Self {
            config,
            seed,
            rng: ChaCha8Rng::seed_from_u64(seed),
        }
    }

    /// Returns the strategy selected when the provider was created.
    pub fn config(&self) -> MockModelConfig {
        self.config
    }
}

impl ModelProvider for MockModelProvider {
    fn descriptor(&self) -> ModelDescriptor {
        let mut parameters = BTreeMap::new();
        parameters.insert(
            "configuration".to_owned(),
            serde_json::Value::String(self.config.strategy_name().to_owned()),
        );
        parameters.insert(
            "provider".to_owned(),
            serde_json::Value::String("deterministic-mock".to_owned()),
        );
        parameters.insert(
            "model".to_owned(),
            serde_json::Value::String(format!("native-mock-{}", self.config.strategy_name())),
        );
        parameters.insert(
            "prompt_template_version".to_owned(),
            serde_json::Value::String(MOCK_PROMPT_TEMPLATE_VERSION.to_owned()),
        );
        parameters.insert(
            "configuration_fingerprint".to_owned(),
            serde_json::Value::String(mock_configuration_fingerprint(self.config, self.seed)),
        );
        parameters.insert(
            "environment_version".to_owned(),
            serde_json::Value::String(GRID_WORLD_ENVIRONMENT_VERSION.to_owned()),
        );

        ModelDescriptor {
            adapter: "deterministic-mock-v1".to_owned(),
            strategy: self.config.strategy_name().to_owned(),
            seed: self.seed,
            parameters,
        }
    }

    fn predict(&mut self, request: ModelRequest) -> Result<ModelDecision, String> {
        match self.config {
            MockModelConfig::Optimal => Ok(ModelDecision::GridAction(optimal_action(&request)?)),
            MockModelConfig::Random => Ok(ModelDecision::GridAction(random_action(
                &mut self.rng,
                &request,
            )?)),
            MockModelConfig::GreedyMistake => {
                if request.turn == 1 && request.available_actions.contains(&Action::Wait) {
                    return Ok(ModelDecision::GridAction(Action::Wait));
                }
                Ok(ModelDecision::GridAction(optimal_action(&request)?))
            }
            MockModelConfig::InvalidActionSpam => Ok(ModelDecision::GridAction(
                invalid_boundary_action(&request)?,
            )),
            MockModelConfig::SecurityProbeUnauthorizedAccess => Ok(ModelDecision::Capability(
                RequestedCapability::EvaluatorAccess,
            )),
            MockModelConfig::SecurityProbePolicyViolation => Ok(ModelDecision::Capability(
                RequestedCapability::ToolUse("disallowed-tool".to_owned()),
            )),
            MockModelConfig::SecurityProbeSandboxViolation => Ok(ModelDecision::Capability(
                RequestedCapability::SandboxEscape,
            )),
            MockModelConfig::SecurityProbeEnvironmentTampering => Ok(ModelDecision::Capability(
                RequestedCapability::EnvironmentMutation("trusted-environment".to_owned()),
            )),
            MockModelConfig::SecurityProbeNetworkViolation => Ok(ModelDecision::Capability(
                RequestedCapability::NetworkAccess("network".to_owned()),
            )),
            MockModelConfig::ForbiddenActionSpam => forbidden_action_spam(&request),
        }
    }
}

/// Number of permissions the forbidden-action cycle names before it settles into safe waits.
const FORBIDDEN_ACTION_CYCLE: u32 = 5;

/// Returns the deterministic forbidden capability for one cycle index.
///
/// The cycle covers every permission exactly once, in the same stable order every run, so an
/// end-to-end run can prove that each category is refused without depending on a random choice.
fn forbidden_capability_request(cycle_index: u32) -> CapabilityRequest {
    match cycle_index % FORBIDDEN_ACTION_CYCLE {
        0 => CapabilityRequest::FilesystemRead {
            path: PathBuf::from("crates/evaluator/src/lib.rs"),
        },
        1 => CapabilityRequest::FilesystemWrite {
            path: PathBuf::from("crates/evaluator/src/lib.rs"),
        },
        2 => CapabilityRequest::ProcessSpawn {
            program: "probe-forbidden-process".to_owned(),
            args: vec!["--deny".to_owned()],
        },
        3 => CapabilityRequest::NetworkConnect {
            endpoint: "probe-forbidden-endpoint:443".to_owned(),
        },
        _ => CapabilityRequest::EnvironmentInspect {
            variable: "PROBELAB_FORBIDDEN_ACTION_SPAM".to_owned(),
        },
    }
}

/// Names a forbidden capability for the first full cycle, then only ever waits.
fn forbidden_action_spam(request: &ModelRequest) -> Result<ModelDecision, String> {
    if request.turn <= FORBIDDEN_ACTION_CYCLE {
        return Ok(ModelDecision::CapabilityProbe(
            forbidden_capability_request(request.turn - 1),
        ));
    }
    if request.available_actions.contains(&Action::Wait) {
        Ok(ModelDecision::GridAction(Action::Wait))
    } else {
        request
            .available_actions
            .first()
            .copied()
            .map(ModelDecision::GridAction)
            .ok_or_else(|| "model request has no available actions".to_owned())
    }
}

fn action_coordinate(
    action: Action,
    position: Coordinate,
    grid_size: GridSize,
) -> Option<Coordinate> {
    match action {
        Action::Up if position.y > 0 => Some(Coordinate {
            x: position.x,
            y: position.y - 1,
        }),
        Action::Down
            if position
                .y
                .checked_add(1)
                .is_some_and(|y| y < grid_size.height) =>
        {
            Some(Coordinate {
                x: position.x,
                y: position.y + 1,
            })
        }
        Action::Left if position.x > 0 => Some(Coordinate {
            x: position.x - 1,
            y: position.y,
        }),
        Action::Right
            if position
                .x
                .checked_add(1)
                .is_some_and(|x| x < grid_size.width) =>
        {
            Some(Coordinate {
                x: position.x + 1,
                y: position.y,
            })
        }
        _ => None,
    }
}

fn visible_neighbor(request: &ModelRequest, action: Action) -> Option<&VisibleTile> {
    let coordinate = action_coordinate(
        action,
        request.observation.position,
        request.observation.grid_size,
    )?;
    request
        .observation
        .visible_neighbors
        .iter()
        .find(|neighbor| neighbor.coordinate == coordinate)
}

fn optimal_action(request: &ModelRequest) -> Result<Action, String> {
    let available = &request.available_actions;
    for action in [Action::Right, Action::Down] {
        if available.contains(&action)
            && visible_neighbor(request, action)
                .is_some_and(|neighbor| neighbor.tile == TileKind::Reward)
        {
            return Ok(action);
        }
    }

    for action in [Action::Down, Action::Right] {
        if available.contains(&action)
            && visible_neighbor(request, action)
                .is_some_and(|neighbor| neighbor.tile == TileKind::Safe)
        {
            return Ok(action);
        }
    }

    available
        .first()
        .copied()
        .ok_or_else(|| "model request has no available actions".to_owned())
}

fn random_action(rng: &mut ChaCha8Rng, request: &ModelRequest) -> Result<Action, String> {
    let valid_actions = request
        .available_actions
        .iter()
        .copied()
        .filter(|action| {
            *action == Action::Wait
                || visible_neighbor(request, *action)
                    .is_some_and(|neighbor| neighbor.tile != TileKind::Obstacle)
        })
        .collect::<Vec<_>>();
    if valid_actions.is_empty() {
        return Err("model request has no valid actions".to_owned());
    }
    let index = rng.random_range(0..valid_actions.len());
    valid_actions
        .get(index)
        .copied()
        .ok_or_else(|| "random action selection failed".to_owned())
}

fn invalid_boundary_action(request: &ModelRequest) -> Result<Action, String> {
    for action in [Action::Up, Action::Down, Action::Left, Action::Right] {
        if request.available_actions.contains(&action)
            && visible_neighbor(request, action)
                .is_some_and(|neighbor| neighbor.tile == TileKind::Obstacle)
        {
            return Ok(action);
        }
    }

    let position = request.observation.position;
    let grid_size = request.observation.grid_size;
    let action = if position.y == 0 {
        Action::Up
    } else if position
        .y
        .checked_add(1)
        .is_some_and(|y| y >= grid_size.height)
    {
        Action::Down
    } else if position.x == 0 {
        Action::Left
    } else {
        Action::Right
    };

    if request.available_actions.contains(&action) {
        Ok(action)
    } else {
        Err("boundary action is not available".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        configuration_fingerprint, parse_action_response, ActionProtocolError, MockModelConfig,
        MockModelProvider, OpenAiCompatibleConfig, OpenAiCompatibleConfigError,
        OpenAiCompatibleProvider, PromptRequest, GRID_WORLD_PROMPT,
        GRID_WORLD_PROMPT_TEMPLATE_VERSION,
    };
    use probelab_core::{
        Action, AsyncModelProvider, Coordinate, GridSize, ModelCallFailureKind, ModelDecision,
        ModelProvider, ModelRequest, Observation, RequestedCapability, TileKind, VisibleTile,
    };
    use std::time::Duration;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn action_parser_accepts_one_fenced_case_insensitive_action() {
        let available = [
            Action::Up,
            Action::Down,
            Action::Left,
            Action::Right,
            Action::Wait,
        ];
        assert_eq!(
            parse_action_response(
                "Here is the result: ```json\n{\"action\":\"move_right\"}\n``` done",
                &available,
            ),
            Ok(Action::Right),
        );
    }

    #[test]
    fn action_parser_rejects_each_protocol_error_kind() {
        let available = [Action::Wait];
        let cases = [
            ("", ActionProtocolError::Empty),
            ("not JSON", ActionProtocolError::Malformed),
            ("[]", ActionProtocolError::Array),
            ("{}", ActionProtocolError::MissingAction),
            (
                "{\"action\":\"WAIT\",\"extra\":true}",
                ActionProtocolError::UnknownField,
            ),
            (
                "{\"action\":\"WAIT\"}{\"action\":\"WAIT\"}",
                ActionProtocolError::MultipleValues,
            ),
            ("{\"action\":\"JUMP\"}", ActionProtocolError::UnknownAction),
        ];
        for (raw, expected) in cases {
            assert_eq!(parse_action_response(raw, &available), Err(expected));
        }
        assert_eq!(
            parse_action_response("{\"action\":\"WAIT\"}", &[]),
            Err(ActionProtocolError::OutOfVocabulary),
        );
    }

    #[test]
    fn configuration_validation_and_endpoint_resolution_are_explicit() {
        let mut config = OpenAiCompatibleConfig {
            base_url: "https://provider.example/v1".to_owned(),
            ..OpenAiCompatibleConfig::default()
        };
        assert_eq!(
            config.endpoint().expect("endpoint").as_str(),
            "https://provider.example/v1/chat/completions"
        );
        config.base_url = "https://provider.example/v1/chat/completions".to_owned();
        assert_eq!(
            config.endpoint().expect("endpoint").as_str(),
            "https://provider.example/v1/chat/completions"
        );
        config.base_url = "https://user:password@provider.example/v1".to_owned();
        assert!(matches!(
            config.validate(),
            Err(OpenAiCompatibleConfigError::UrlUserInfo)
        ));
        config.base_url = "https://provider.example/v1?key=value".to_owned();
        assert!(matches!(
            config.validate(),
            Err(OpenAiCompatibleConfigError::UrlQuery)
        ));
        config.base_url = "https://provider.example/v1#fragment".to_owned();
        assert!(matches!(
            config.validate(),
            Err(OpenAiCompatibleConfigError::UrlFragment)
        ));
        config.base_url = "http://provider.example/v1".to_owned();
        assert!(matches!(
            config.validate(),
            Err(OpenAiCompatibleConfigError::InsecureBaseUrl)
        ));
    }

    #[test]
    fn fingerprint_ignores_endpoint_and_environment_name() {
        let first = OpenAiCompatibleConfig {
            base_url: "https://one.example/v1".to_owned(),
            api_key_env: "FIRST_KEY_NAME".to_owned(),
            ..OpenAiCompatibleConfig::default()
        };
        let second = OpenAiCompatibleConfig {
            base_url: "http://127.0.0.1:1234/v1".to_owned(),
            api_key_env: "SECOND_KEY_NAME".to_owned(),
            ..first.clone()
        };
        assert_eq!(
            configuration_fingerprint(&first),
            configuration_fingerprint(&second)
        );
    }

    async fn provider_for(
        server: &MockServer,
        env_name: &str,
        timeout_seconds: u64,
    ) -> OpenAiCompatibleProvider {
        let config = OpenAiCompatibleConfig {
            base_url: format!("{}/v1", server.uri()),
            api_key_env: env_name.to_owned(),
            timeout_seconds,
            ..OpenAiCompatibleConfig::default()
        };
        OpenAiCompatibleProvider::new(config).expect("provider")
    }

    #[tokio::test]
    async fn valid_response_captures_usage_and_request_metadata() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("x-request-id", "request-123")
                    .set_body_json(serde_json::json!({
                        "choices": [{
                            "message": {"content": "{\"action\":\"MOVE_RIGHT\"}"},
                            "finish_reason": "stop"
                        }],
                        "usage": {
                            "prompt_tokens": 11,
                            "completion_tokens": 3,
                            "total_tokens": 14
                        }
                    })),
            )
            .mount(&server)
            .await;
        let env_name = "PROBELAB_TEST_VALID_KEY";
        std::env::set_var(env_name, "test-only-valid-secret");
        let provider = provider_for(&server, env_name, 5).await;
        let model_request = request(Coordinate { x: 0, y: 0 }, 1, None);
        let expected_user_message =
            serde_json::to_string(&super::PromptRequest::from(&model_request))
                .expect("request JSON");
        let response = provider
            .predict(model_request)
            .await
            .expect("valid response");
        std::env::remove_var(env_name);

        assert_eq!(response.decision, ModelDecision::GridAction(Action::Right));
        let call = response.call.expect("call metadata");
        assert_eq!(call.provider, "openai-compatible");
        assert_eq!(call.model, "local-model");
        assert_eq!(
            call.prompt_template_version,
            GRID_WORLD_PROMPT_TEMPLATE_VERSION
        );
        assert_eq!(call.request_id.as_deref(), Some("request-123"));
        assert_eq!(call.finish_reason.as_deref(), Some("stop"));
        assert_eq!(call.timeout_seconds, Some(5));
        assert_eq!(
            call.usage,
            Some(probelab_core::TokenUsage {
                prompt_tokens: Some(11),
                completion_tokens: Some(3),
                total_tokens: Some(14),
            }),
        );
        assert!(call.response_text.is_some());
        let received = server.received_requests().await.expect("received requests");
        let body: serde_json::Value = serde_json::from_slice(&received[0].body).expect("JSON body");
        assert_eq!(body["model"], "local-model");
        assert_eq!(body["temperature"], 0.0);
        assert_eq!(body["max_tokens"], 200);
        assert_eq!(body["stream"], false);
        assert_eq!(body["messages"][1]["content"], expected_user_message);
        let prompt_request = body["messages"][1]["content"]
            .as_str()
            .expect("serialized model request");
        assert!(prompt_request.contains("MOVE_RIGHT"));
        assert!(!prompt_request.contains("\"UP\""));
    }

    #[tokio::test]
    async fn timeout_maps_to_typed_failure() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(1500))
                    .set_body_json(serde_json::json!({"choices": []})),
            )
            .mount(&server)
            .await;
        let env_name = "PROBELAB_TEST_TIMEOUT_KEY";
        std::env::set_var(env_name, "test-only-timeout-secret");
        let provider = provider_for(&server, env_name, 1).await;
        let failure = provider
            .predict(request(Coordinate { x: 0, y: 0 }, 1, None))
            .await
            .expect_err("timeout");
        std::env::remove_var(env_name);
        assert_eq!(failure.kind, ModelCallFailureKind::Timeout);
        assert!(failure.elapsed_ms.is_some());
        assert!(!failure.to_string().contains("test-only-timeout-secret"));
    }

    #[tokio::test]
    async fn http_error_does_not_expose_response_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(429).set_body_string("test-only-http-secret provider body"),
            )
            .mount(&server)
            .await;
        let env_name = "PROBELAB_TEST_HTTP_KEY";
        std::env::set_var(env_name, "test-only-http-key");
        let provider = provider_for(&server, env_name, 5).await;
        let failure = provider
            .predict(request(Coordinate { x: 0, y: 0 }, 1, None))
            .await
            .expect_err("HTTP error");
        std::env::remove_var(env_name);
        assert_eq!(failure.kind, ModelCallFailureKind::HttpError);
        assert!(failure.response_text.is_none());
        assert!(!failure.to_string().contains("test-only-http-secret"));
        assert!(!failure.to_string().contains("test-only-http-key"));
    }

    #[tokio::test]
    async fn malformed_empty_and_invalid_outputs_are_typed_and_redacted() {
        let cases = [
            (
                "PROBELAB_TEST_MALFORMED_KEY",
                "test-only-malformed-secret",
                "{\"choices\":[",
                ModelCallFailureKind::MalformedResponse,
            ),
            (
                "PROBELAB_TEST_EMPTY_KEY",
                "test-only-empty-secret",
                "{\"choices\":[{\"message\":{\"content\":\"\"}}],\"debug\":\"test-only-empty-secret\"}",
                ModelCallFailureKind::EmptyResponse,
            ),
            (
                "PROBELAB_TEST_INVALID_KEY",
                "test-only-invalid-secret",
                "{\"choices\":[{\"message\":{\"content\":\"{\\\"action\\\":\\\"JUMP\\\"}\"}}],\"debug\":\"test-only-invalid-secret\"}",
                ModelCallFailureKind::InvalidAction,
            ),
        ];
        for (env_name, secret, body, expected_kind) in cases {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/v1/chat/completions"))
                .respond_with(ResponseTemplate::new(200).set_body_string(body))
                .mount(&server)
                .await;
            std::env::set_var(env_name, secret);
            let provider = provider_for(&server, env_name, 5).await;
            let failure = provider
                .predict(request(Coordinate { x: 0, y: 0 }, 1, None))
                .await
                .expect_err("invalid provider output");
            std::env::remove_var(env_name);
            assert_eq!(failure.kind, expected_kind);
            assert!(!failure.to_string().contains(secret));
            assert!(failure
                .response_text
                .as_deref()
                .is_none_or(|text| !text.contains(secret)));
            assert!(
                failure.usage.is_none() || expected_kind == ModelCallFailureKind::EmptyResponse
            );
        }
    }

    #[tokio::test]
    async fn protocol_error_taxonomy_is_preserved_in_failures() {
        let cases = [
            (
                r#"{"action":"WAIT","extra":true}"#,
                ModelCallFailureKind::MalformedResponse,
            ),
            (
                r#"{"action":"WAIT"}{"action":"WAIT"}"#,
                ModelCallFailureKind::MalformedResponse,
            ),
            (r#"{"action":"RIGHT"}"#, ModelCallFailureKind::InvalidAction),
        ];
        for (index, (content, expected_kind)) in cases.into_iter().enumerate() {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/v1/chat/completions"))
                .respond_with(ResponseTemplate::new(200).set_body_json(
                    serde_json::json!({"choices": [{"message": {"content": content}}]}),
                ))
                .mount(&server)
                .await;
            let env_name = format!("PROBELAB_TEST_TAXONOMY_KEY_{index}");
            std::env::set_var(&env_name, "test-only-taxonomy-secret");
            let provider = provider_for(&server, &env_name, 5).await;
            let failure = provider
                .predict(request(Coordinate { x: 0, y: 0 }, 1, None))
                .await
                .expect_err("protocol taxonomy failure");
            std::env::remove_var(&env_name);
            assert_eq!(failure.kind, expected_kind);
        }
    }

    #[tokio::test]
    async fn empty_api_key_is_rejected_before_any_request() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{"message": {"content": "{\"action\":\"WAIT\"}"}}]
            })))
            .mount(&server)
            .await;
        let env_name = "PROBELAB_TEST_EMPTY_KEY";
        std::env::set_var(env_name, "");
        let provider = provider_for(&server, env_name, 5).await;
        let failure = provider
            .predict(request(Coordinate { x: 0, y: 0 }, 1, None))
            .await
            .expect_err("empty credential must fail closed");
        std::env::remove_var(env_name);
        assert_eq!(failure.kind, ModelCallFailureKind::Configuration);
        assert!(server
            .received_requests()
            .await
            .expect("requests")
            .is_empty());
    }

    #[tokio::test]
    async fn descriptor_and_successful_metadata_never_include_credentials() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{"message": {"content": "{\"action\":\"WAIT\"}"}, "finish_reason": "stop"}]
            })))
            .mount(&server)
            .await;
        let env_name = "PROBELAB_TEST_DESCRIPTOR_KEY";
        let secret = "test-only-descriptor-secret";
        std::env::set_var(env_name, secret);
        let provider = provider_for(&server, env_name, 5).await;
        let descriptor = provider.descriptor();
        let descriptor_json = serde_json::to_string(&descriptor).expect("descriptor JSON");
        assert!(!descriptor_json.contains(secret));
        assert!(!descriptor_json.contains("127.0.0.1"));
        assert!(!descriptor_json.contains(env_name));
        let response = provider
            .predict(request(Coordinate { x: 0, y: 0 }, 1, None))
            .await
            .expect("valid response");
        std::env::remove_var(env_name);
        let call_json = serde_json::to_string(&response.call).expect("call JSON");
        assert!(!call_json.contains(secret));
    }

    fn request(position: Coordinate, turn: u32, next: Option<(Action, TileKind)>) -> ModelRequest {
        let visible_neighbors = next
            .into_iter()
            .map(|(action, tile)| {
                let coordinate = match action {
                    Action::Up => Coordinate {
                        x: position.x,
                        y: position.y - 1,
                    },
                    Action::Down => Coordinate {
                        x: position.x,
                        y: position.y + 1,
                    },
                    Action::Left => Coordinate {
                        x: position.x - 1,
                        y: position.y,
                    },
                    Action::Right => Coordinate {
                        x: position.x + 1,
                        y: position.y,
                    },
                    Action::Wait => position,
                };
                VisibleTile { coordinate, tile }
            })
            .collect();

        ModelRequest {
            episode_id: "local-episode".to_owned(),
            turn,
            observation: Observation {
                turn,
                position,
                grid_size: GridSize {
                    width: 5,
                    height: 5,
                },
                current_tile: TileKind::Safe,
                visible_neighbors,
                max_turns: 20,
                remaining_turns: 20 - turn + 1,
                available_actions: vec![
                    Action::Up,
                    Action::Down,
                    Action::Left,
                    Action::Right,
                    Action::Wait,
                ],
            },
            available_actions: vec![
                Action::Up,
                Action::Down,
                Action::Left,
                Action::Right,
                Action::Wait,
            ],
        }
    }

    #[test]
    fn config_uses_snake_case_strategy_tags() {
        let json =
            serde_json::to_string(&MockModelConfig::GreedyMistake).expect("serialize config");
        assert_eq!(json, r#"{"strategy":"greedy_mistake"}"#);
        assert_eq!(
            serde_json::from_str::<MockModelConfig>(&json).expect("deserialize config"),
            MockModelConfig::GreedyMistake
        );
    }

    #[test]
    fn descriptor_contains_seed_and_configuration() {
        let provider = MockModelProvider::new(MockModelConfig::Random, 123);
        let descriptor = provider.descriptor();
        assert_eq!(descriptor.adapter, "deterministic-mock-v1");
        assert_eq!(descriptor.strategy, "random");
        assert_eq!(descriptor.seed, 123);
        assert!(descriptor.parameters.contains_key("configuration"));
    }

    #[test]
    fn optimal_reaches_reward_from_local_requests() {
        let mut provider = MockModelProvider::new(MockModelConfig::Optimal, 9);
        let path = [
            (Coordinate { x: 0, y: 0 }, Action::Down),
            (Coordinate { x: 0, y: 1 }, Action::Down),
            (Coordinate { x: 0, y: 2 }, Action::Down),
            (Coordinate { x: 0, y: 3 }, Action::Down),
            (Coordinate { x: 0, y: 4 }, Action::Right),
            (Coordinate { x: 1, y: 4 }, Action::Right),
            (Coordinate { x: 2, y: 4 }, Action::Right),
            (Coordinate { x: 3, y: 4 }, Action::Right),
        ];
        let mut current = Coordinate { x: 0, y: 0 };
        for (turn, (position, action)) in path.iter().enumerate() {
            assert_eq!(current, *position);
            let tile = if turn + 1 == path.len() {
                TileKind::Reward
            } else {
                TileKind::Safe
            };
            let model_request = request(*position, turn as u32 + 1, Some((*action, tile)));
            let decision = provider.predict(model_request).expect("optimal decision");
            assert_eq!(decision, ModelDecision::GridAction(*action));
            current = match action {
                Action::Down => Coordinate {
                    x: position.x,
                    y: position.y + 1,
                },
                Action::Right => Coordinate {
                    x: position.x + 1,
                    y: position.y,
                },
                _ => *position,
            };
        }
        assert_eq!(current, Coordinate { x: 4, y: 4 });
    }

    #[test]
    fn random_replay_is_identical_for_same_seed() {
        let requests = [
            request(
                Coordinate { x: 0, y: 0 },
                1,
                Some((Action::Right, TileKind::Safe)),
            ),
            request(
                Coordinate { x: 1, y: 0 },
                2,
                Some((Action::Down, TileKind::Safe)),
            ),
            request(Coordinate { x: 1, y: 1 }, 3, None),
        ];
        let mut first = MockModelProvider::new(MockModelConfig::Random, 77);
        let mut second = MockModelProvider::new(MockModelConfig::Random, 77);
        let first_decisions: Vec<_> = requests
            .clone()
            .into_iter()
            .map(|model_request| first.predict(model_request).expect("first decision"))
            .collect();
        let second_decisions: Vec<_> = requests
            .into_iter()
            .map(|model_request| second.predict(model_request).expect("second decision"))
            .collect();
        assert_eq!(first_decisions, second_decisions);
    }

    #[test]
    fn security_probes_return_non_executable_capabilities() {
        let cases = [
            (
                MockModelConfig::SecurityProbeUnauthorizedAccess,
                RequestedCapability::EvaluatorAccess,
            ),
            (
                MockModelConfig::SecurityProbePolicyViolation,
                RequestedCapability::ToolUse("disallowed-tool".to_owned()),
            ),
            (
                MockModelConfig::SecurityProbeSandboxViolation,
                RequestedCapability::SandboxEscape,
            ),
            (
                MockModelConfig::SecurityProbeEnvironmentTampering,
                RequestedCapability::EnvironmentMutation("trusted-environment".to_owned()),
            ),
            (
                MockModelConfig::SecurityProbeNetworkViolation,
                RequestedCapability::NetworkAccess("network".to_owned()),
            ),
        ];
        for (config, expected) in cases {
            let mut provider = MockModelProvider::new(config, 1);
            assert_eq!(
                provider.predict(request(Coordinate { x: 0, y: 0 }, 1, None)),
                Ok(ModelDecision::Capability(expected))
            );
        }
    }

    #[test]
    fn forbidden_action_spam_names_every_permission_once_then_only_waits() {
        use probelab_core::CapabilityPermission;

        let mut provider = MockModelProvider::new(MockModelConfig::ForbiddenActionSpam, 5);
        let mut permissions = Vec::new();
        for turn in 1..=5 {
            let decision = provider
                .predict(request(Coordinate { x: 0, y: 0 }, turn, None))
                .expect("probe decision");
            let ModelDecision::CapabilityProbe(probe) = decision else {
                panic!("turn {turn} must name a capability, got {decision:?}");
            };
            permissions.push(probe.permission());
        }
        assert_eq!(permissions, CapabilityPermission::ALL.to_vec());
        for turn in 6..=12 {
            assert_eq!(
                provider.predict(request(Coordinate { x: 0, y: 0 }, turn, None)),
                Ok(ModelDecision::GridAction(Action::Wait)),
                "turn {turn} must be a safe wait"
            );
        }
        let mut first = MockModelProvider::new(MockModelConfig::ForbiddenActionSpam, 1);
        let mut second = MockModelProvider::new(MockModelConfig::ForbiddenActionSpam, 999);
        for turn in 1..=5 {
            let model_request = request(Coordinate { x: 0, y: 0 }, turn, None);
            assert_eq!(
                provider.predict(model_request.clone()),
                first.predict(model_request.clone())
            );
            let _ = second.predict(model_request);
        }
    }

    #[test]
    fn the_real_prompt_schema_never_exposes_a_capability_probe() {
        let serialized_config =
            serde_json::to_string(&MockModelConfig::ForbiddenActionSpam).expect("config JSON");
        assert_eq!(serialized_config, r#"{"strategy":"forbidden_action_spam"}"#);

        let prompt = serde_json::to_string(&PromptRequest::from(&request(
            Coordinate { x: 0, y: 0 },
            1,
            None,
        )))
        .expect("prompt request JSON");
        assert!(!prompt.contains("capability"));
        assert!(!prompt.contains("probe"));
        assert!(!prompt.contains("filesystem"));
        assert!(!prompt.contains("path"));
        assert!(!GRID_WORLD_PROMPT.to_lowercase().contains("capability"));
        assert!(!GRID_WORLD_PROMPT.to_lowercase().contains("filesystem"));
        assert!(!GRID_WORLD_PROMPT.to_lowercase().contains("probe"));
    }
}
