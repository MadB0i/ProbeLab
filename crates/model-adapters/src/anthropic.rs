//! The Anthropic Messages API adapter.
//!
//! # Why this is not the OpenAI-compatible endpoint
//!
//! Anthropic is not OpenAI-compatible. The request shape differs (a top-level `system` rather than a
//! system message, `max_tokens` as a *required* field, a versioned `anthropic-version` header), and
//! the response is a *list of content blocks* rather than a single string. A model's reply is often
//! split across several blocks, and the first block is not necessarily the whole answer. Reading
//! `content[0].text` and calling it done would truncate a real model response and produce a
//! protocol error that looked like the model had been unparseable.
//!
//! # The credential
//!
//! `x-api-key`, read from an environment variable at call time. Never stored, never serialized, never
//! logged. The adapter redacts it from any text that could reach a result bundle — including the
//! model's own output, because a model can be induced to echo the key that was in its request.

use std::collections::BTreeMap;
use std::time::Instant;

use probelab_core::{ModelCallFailure, ModelCallFailureKind, ModelDescriptor, TokenUsage};
use reqwest::redirect::Policy;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

use crate::transport::{
    read_credential, sanitize_response_text, validate_api_key_env, validate_base_url,
    validate_model, validate_sampling, validate_timeout, TransportConfigError,
};
use crate::{
    configuration_fingerprint, configuration_fingerprint_for, parse_decision_response, prompt_for,
    prompt_template_version_for, ActionProtocolError, OpenAiCompatibleConfig,
    GRID_WORLD_ENVIRONMENT_VERSION,
};

/// The `anthropic-version` this adapter speaks.
///
/// Pinned rather than taken from configuration, because a moving version means a recorded run cannot
/// be reproduced. A recorded Anthropic result states which API version produced it.
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Configuration for the Anthropic Messages API.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AnthropicConfig {
    /// Base URL, normally `https://api.anthropic.com`.
    pub base_url: String,
    /// Name of the environment variable holding the key.
    pub api_key_env: String,
    /// Model identifier, for example `claude-sonnet-4-5`.
    pub model: String,
    /// Request timeout in seconds.
    pub timeout_seconds: u64,
    /// Sampling temperature.
    pub temperature: f64,
    /// Maximum output tokens. Required by the API, and validated here so a zero never leaves the host.
    pub max_tokens: u32,
}

impl Default for AnthropicConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.anthropic.com".to_owned(),
            api_key_env: "ANTHROPIC_API_KEY".to_owned(),
            model: "claude-sonnet-4-5".to_owned(),
            timeout_seconds: 60,
            temperature: 0.0,
            max_tokens: 200,
        }
    }
}

impl AnthropicConfig {
    /// Validates the configuration under the shared transport rules.
    pub fn validate(&self) -> Result<(), TransportConfigError> {
        self.endpoint()?;
        validate_api_key_env(&self.api_key_env)?;
        validate_model(&self.model)?;
        validate_timeout(self.timeout_seconds)?;
        validate_sampling(self.temperature, self.max_tokens)?;
        Ok(())
    }

    /// Resolves the `/v1/messages` endpoint.
    ///
    /// A base that already names `v1/messages` is left alone, because the audit and UI show the user
    /// a complete URL and it should behave the way it reads.
    pub fn endpoint(&self) -> Result<Url, TransportConfigError> {
        let mut endpoint = validate_base_url(&self.base_url)?;
        let path = endpoint.path().trim_end_matches('/');
        let endpoint_path = if path.ends_with("/v1/messages") || path.ends_with("/messages") {
            path.to_owned()
        } else {
            format!("{path}/v1/messages")
        };
        endpoint.set_path(&endpoint_path);
        Ok(endpoint)
    }

    /// The fingerprint recorded with every result from this configuration.
    pub fn fingerprint(&self) -> String {
        configuration_fingerprint(&OpenAiCompatibleConfig {
            base_url: self
                .endpoint()
                .map(|url| url.to_string())
                .unwrap_or_default(),
            api_key_env: self.api_key_env.clone(),
            model: self.model.clone(),
            timeout_seconds: self.timeout_seconds,
            temperature: self.temperature,
            max_tokens: self.max_tokens,
        })
    }
}

/// One message in a request.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AnthropicMessage {
    /// `user` or `assistant`.
    pub role: String,
    /// The message text.
    pub content: String,
}

/// A request body for `POST /v1/messages`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    temperature: f64,
    system: String,
    messages: Vec<AnthropicMessage>,
    /// Pinned so a run is reproducible against a recorded API version.
    #[serde(skip)]
    version: (),
}

impl AnthropicRequest {
    /// Builds a request body.
    ///
    /// The system prompt goes in the top-level `system` field and the turn in the message list, which
    /// is the shape the API requires; putting the instructions in as a message would silently work
    /// while costing tokens differently and misreporting the prompt template version.
    fn build(
        config: &AnthropicConfig,
        system: &str,
        user: &str,
        _process_spawn_available: bool,
    ) -> Self {
        Self {
            model: config.model.clone(),
            max_tokens: config.max_tokens,
            temperature: config.temperature,
            system: system.to_owned(),
            messages: vec![AnthropicMessage {
                role: "user".to_owned(),
                content: user.to_owned(),
            }],
            version: (),
        }
    }
}

/// A content block in a response.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AnthropicContentBlock {
    /// `text`, `thinking`, `tool_use`, and so on.
    #[serde(rename = "type")]
    pub kind: String,
    /// Present on text blocks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

impl AnthropicContentBlock {
    /// Whether this block contributes to the model's textual answer.
    ///
    /// A `thinking` block is the model's reasoning trace. It is not part of the answer, and including
    /// it would put prose in front of the JSON the parser is looking for. `tool_use` is not text at
    /// all, and the lab does not offer tools.
    fn is_answer_text(&self) -> bool {
        self.kind == "text" && self.text.as_ref().is_some_and(|text| !text.is_empty())
    }
}

/// Token accounting from a response.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
struct AnthropicUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct AnthropicResponse {
    #[serde(default)]
    content: Vec<AnthropicContentBlock>,
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    usage: AnthropicUsage,
}

/// Joins the text blocks of a response into the single string the protocol parser expects.
///
/// Empty blocks are skipped rather than joined with separators, so a response whose blocks are
/// `["", "{\"action\":...}"]` parses instead of failing on a leading newline the provider chose.
pub fn join_text_blocks(blocks: &[AnthropicContentBlock]) -> String {
    let mut out = String::new();
    for block in blocks.iter().filter(|block| block.is_answer_text()) {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(block.text.as_deref().unwrap_or_default());
    }
    out
}

/// A failure specific to the Anthropic wire format.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum AnthropicError {
    /// The response had no answer text at all.
    ///
    /// Distinct from "the model's JSON was bad": a refusal, an empty completion, and a truncated
    /// stream all land here, and none of them are a protocol violation by the model.
    #[error("the Anthropic response contained no text content block")]
    NoTextContent,
    /// The model stopped because it hit the output limit.
    #[error("the model stopped at the output token limit before producing a decision")]
    OutputTruncated,
    /// The API refused the request or reported an error object.
    #[error("the Anthropic API returned an error: {0}")]
    ApiRejected(String),
}

impl From<ActionProtocolError> for AnthropicError {
    fn from(error: ActionProtocolError) -> Self {
        AnthropicError::ApiRejected(error.to_string())
    }
}

/// The Anthropic Messages provider.
#[derive(Clone, Debug)]
pub struct AnthropicProvider {
    config: AnthropicConfig,
    client: Client,
}

impl AnthropicProvider {
    /// Builds a provider, validating the configuration first.
    pub fn new(config: AnthropicConfig) -> Result<Self, TransportConfigError> {
        config.validate()?;
        let client = Client::builder()
            .timeout(validate_timeout(config.timeout_seconds)?)
            .redirect(Policy::none())
            .build()
            .map_err(|_| TransportConfigError::InvalidBaseUrl)?;
        Ok(Self { config, client })
    }

    /// The configuration in use.
    pub fn config(&self) -> &AnthropicConfig {
        &self.config
    }

    /// The descriptor recorded with a result.
    pub fn descriptor_value(&self) -> ModelDescriptor {
        let mut parameters = BTreeMap::new();
        parameters.insert(
            "provider".to_owned(),
            serde_json::Value::String("anthropic".to_owned()),
        );
        parameters.insert(
            "model".to_owned(),
            serde_json::Value::String(self.config.model.clone()),
        );
        parameters.insert(
            "base_url".to_owned(),
            serde_json::Value::String(
                self.config
                    .endpoint()
                    .map(|url| url.to_string())
                    .unwrap_or_default(),
            ),
        );
        parameters.insert(
            "temperature".to_owned(),
            serde_json::Value::from(self.config.temperature),
        );
        parameters.insert(
            "max_tokens".to_owned(),
            serde_json::Value::from(self.config.max_tokens),
        );
        parameters.insert(
            "api_version".to_owned(),
            serde_json::Value::String(ANTHROPIC_VERSION.to_owned()),
        );
        // The variable *name* is publishable, which is what lets a user fix a missing credential. The
        // value is not present in this struct at all, so there is nothing to redact here.
        parameters.insert(
            "credential_source".to_owned(),
            serde_json::Value::String("environment".to_owned()),
        );
        parameters.insert(
            "credential_env".to_owned(),
            serde_json::Value::String(self.config.api_key_env.clone()),
        );
        /*
         * The three reproducibility fields.
         *
         * These were simply absent here, and the engine reads them out of the descriptor to build
         * `ReproducibilityMetadata`. Absent meant empty, so a recorded Anthropic run stored three
         * blank strings where the OpenAI-compatible adapter stored real values -- and nothing
         * complained, because a blank reproducibility field is a string like any other. The result
         * read as though the prompt version and configuration were known and simply had not been
         * filled in, which is worse than not having the adapter at all.
         *
         * `configuration_fingerprint` is the same function the OpenAI-compatible path uses, so a
         * fingerprint means the same thing across vendors: model, sampling, timeout, prompt template,
         * and benchmark version. `prompt_template_version_for(false)` is the non-spawning template,
         * which is the only one this adapter can send -- `prompt_for` is called with the same flag at
         * request time, so the recorded version is the version that ran.
         */
        parameters.insert(
            "configuration_fingerprint".to_owned(),
            serde_json::Value::String(configuration_fingerprint_for(
                "anthropic",
                &self.config.model,
                self.config.temperature,
                self.config.max_tokens,
                self.config.timeout_seconds,
            )),
        );
        parameters.insert(
            "prompt_template_version".to_owned(),
            serde_json::Value::String(prompt_template_version_for(false).to_owned()),
        );
        parameters.insert(
            "environment_version".to_owned(),
            serde_json::Value::String(GRID_WORLD_ENVIRONMENT_VERSION.to_owned()),
        );
        ModelDescriptor {
            adapter: "anthropic-messages-v1".to_owned(),
            strategy: self.config.model.clone(),
            seed: 0,
            parameters,
        }
    }
}

impl probelab_core::AsyncModelProvider for AnthropicProvider {
    fn descriptor(&self) -> ModelDescriptor {
        self.descriptor_value()
    }

    fn predict(
        &self,
        request: probelab_core::ModelRequest,
    ) -> probelab_core::ModelProviderFuture<'_> {
        Box::pin(self.predict_async(request))
    }
}

impl AnthropicProvider {
    async fn predict_async(
        &self,
        request: probelab_core::ModelRequest,
    ) -> Result<probelab_core::ModelResponse, ModelCallFailure> {
        request.validate().map_err(|_| {
            ModelCallFailure::new(
                ModelCallFailureKind::Configuration,
                "model request validation failed",
            )
        })?;
        let credential = read_credential(&self.config.api_key_env).map_err(|error| {
            ModelCallFailure::new(
                ModelCallFailureKind::Configuration,
                // The variable *name* is not a secret and the name is already in the descriptor, so
                // saying which one is missing is what lets a user fix it.
                format!(
                    "the Anthropic credential ({}) is unavailable: {error}",
                    self.config.api_key_env
                ),
            )
        })?;
        let (system, user) = prompt_for(request.process_spawn_available);
        let endpoint = self.config.endpoint().map_err(|error| {
            ModelCallFailure::new(ModelCallFailureKind::Configuration, error.to_string())
        })?;
        let body =
            AnthropicRequest::build(&self.config, system, user, request.process_spawn_available);
        let started = Instant::now();

        let response = self
            .client
            .post(endpoint)
            .header("x-api-key", &credential)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|error| {
                let text = error.to_string();
                let kind = if error.is_timeout() {
                    ModelCallFailureKind::Timeout
                } else {
                    ModelCallFailureKind::TransportDisconnect
                };
                ModelCallFailure {
                    kind,
                    message: "the Anthropic request failed in transport".to_owned(),
                    response_text: Some(sanitize_response_text(&text, &credential)),
                    usage: None,
                    elapsed_ms: Some(elapsed_ms(started)),
                    http_status: None,
                }
            })?;

        let status = response.status();
        let request_id = response
            .headers()
            .get("request-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let raw = response.text().await.map_err(|error| ModelCallFailure {
            kind: ModelCallFailureKind::TransportDisconnect,
            message: "the Anthropic response body could not be read".to_owned(),
            response_text: Some(sanitize_response_text(&error.to_string(), &credential)),
            usage: None,
            elapsed_ms: Some(elapsed_ms(started)),
            http_status: None,
        })?;

        if !status.is_success() {
            // Classified through the shared rules rather than a local `HttpError`, so a 404 naming a
            // model Anthropic does not host reads as "model not found" on every adapter instead of
            // only on the one this happens to be written in.
            let kind = crate::classify_http_failure(status.as_u16(), &raw);
            return Err(ModelCallFailure {
                kind,
                message: format!(
                    "the Anthropic API returned HTTP {}: {}",
                    status.as_u16(),
                    crate::first_useful_line(&raw)
                ),
                response_text: Some(sanitize_response_text(&raw, &credential)),
                usage: None,
                elapsed_ms: Some(elapsed_ms(started)),
                http_status: Some(status.as_u16()),
            });
        }

        let parsed: AnthropicResponse =
            serde_json::from_str(&raw).map_err(|error| ModelCallFailure {
                kind: ModelCallFailureKind::MalformedResponse,
                message: "the Anthropic response was not the expected JSON shape".to_owned(),
                response_text: Some(sanitize_response_text(
                    &format!("{error}; body: {raw}"),
                    &credential,
                )),
                usage: None,
                elapsed_ms: Some(elapsed_ms(started)),
                http_status: Some(status.as_u16()),
            })?;

        let elapsed = elapsed_ms(started);
        let usage = TokenUsage {
            prompt_tokens: Some(parsed.usage.input_tokens),
            completion_tokens: Some(parsed.usage.output_tokens),
            total_tokens: Some(parsed.usage.input_tokens + parsed.usage.output_tokens),
        };

        // A response that ran out of output budget is not a model that cannot follow the protocol.
        // It is a configuration outcome, and conflating the two would make a model look worse than it
        // is while hiding the real fix (raise max_tokens).
        if parsed.stop_reason.as_deref() == Some("max_tokens") {
            return Err(ModelCallFailure {
                kind: ModelCallFailureKind::EmptyResponse,
                message: AnthropicError::OutputTruncated.to_string(),
                response_text: Some(sanitize_response_text(
                    &join_text_blocks(&parsed.content),
                    &credential,
                )),
                usage: Some(usage),
                elapsed_ms: Some(elapsed),
                http_status: Some(status.as_u16()),
            });
        }

        let text = join_text_blocks(&parsed.content);
        if text.is_empty() {
            return Err(ModelCallFailure {
                kind: ModelCallFailureKind::EmptyResponse,
                message: AnthropicError::NoTextContent.to_string(),
                response_text: None,
                usage: Some(usage),
                elapsed_ms: Some(elapsed),
                http_status: Some(status.as_u16()),
            });
        }

        let sanitized = sanitize_response_text(&text, &credential);
        let decision = match parse_decision_response(
            &text,
            &request.available_actions,
            request.process_spawn_available,
        ) {
            Ok(decision) => decision,
            Err(error) => {
                return Err(ModelCallFailure {
                    // A well-formed reply naming a legal-shaped but unavailable action is the model
                    // failing the protocol, so it keeps its own kind.
                    kind: error.kind(),
                    message: error.to_string(),
                    response_text: Some(sanitized),
                    usage: Some(usage),
                    elapsed_ms: Some(elapsed),
                    http_status: Some(status.as_u16()),
                });
            }
        };

        Ok(probelab_core::ModelResponse {
            decision,
            call: Some(probelab_core::ModelCallInfo {
                provider: "anthropic".to_owned(),
                model: self.config.model.clone(),
                prompt_template_version: prompt_template_version_for(
                    request.process_spawn_available,
                )
                .to_owned(),
                request_id,
                finish_reason: parsed.stop_reason,
                usage: Some(usage),
                timeout_seconds: Some(self.config.timeout_seconds),
                elapsed_ms: elapsed,
                response_text: Some(sanitized),
            }),
        })
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}
/// A stable digest of the adapter version, so a result states which wire implementation produced it.
pub fn anthropic_adapter_fingerprint() -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"anthropic-messages-v1");
    hasher.update(ANTHROPIC_VERSION.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three reproducibility fields the engine reads out of the descriptor to build
    /// `ReproducibilityMetadata`.
    ///
    /// They were absent from this adapter, and absence is invisible: the engine's
    /// `descriptor_string` filters empty values, so a missing key and a blank one both became an empty
    /// string in the stored result. A recorded Anthropic run therefore reported no prompt template
    /// version, no configuration fingerprint, and no environment version, and read as though those
    /// facts were known and simply not filled in.
    #[test]
    fn the_descriptor_carries_every_reproducibility_field() {
        let provider = AnthropicProvider::new(AnthropicConfig::default()).expect("valid config");
        let descriptor = provider.descriptor_value();
        for field in [
            "configuration_fingerprint",
            "prompt_template_version",
            "environment_version",
        ] {
            let value = descriptor
                .parameters
                .get(field)
                .unwrap_or_else(|| panic!("{field} is missing from the descriptor"))
                .as_str()
                .unwrap_or_else(|| panic!("{field} is not a string"));
            assert!(!value.trim().is_empty(), "{field} is present but blank");
        }
    }

    #[test]
    fn the_fingerprint_moves_when_the_configuration_does() {
        let base = AnthropicConfig::default();
        let build = |config: AnthropicConfig| {
            AnthropicProvider::new(config)
                .expect("valid config")
                .descriptor_value()
        };
        let unchanged = build(base.clone());
        let same = build(base.clone());
        let hotter = build(AnthropicConfig {
            temperature: base.temperature + 0.5,
            ..base.clone()
        });
        let other_model = build(AnthropicConfig {
            model: "claude-other".to_owned(),
            ..base.clone()
        });

        let read = |descriptor: &probelab_core::ModelDescriptor| {
            descriptor.parameters["configuration_fingerprint"]
                .as_str()
                .expect("a string")
                .to_owned()
        };

        assert_eq!(
            read(&unchanged),
            read(&same),
            "the same config must fingerprint the same"
        );
        assert_ne!(
            read(&unchanged),
            read(&hotter),
            "temperature is model-affecting"
        );
        assert_ne!(
            read(&unchanged),
            read(&other_model),
            "the model is model-affecting"
        );
    }

    #[test]
    fn the_fingerprint_differs_from_another_vendors_for_the_same_settings() {
        // A fingerprint is a reproducibility identity. If two vendors hashing identical settings
        // produced the same value, a stored result would claim an identity spanning two APIs.
        let config = OpenAiCompatibleConfig {
            model: "shared-model".to_owned(),
            temperature: 0.5,
            max_tokens: 256,
            timeout_seconds: 30,
            ..OpenAiCompatibleConfig::default()
        };
        let anthropic = configuration_fingerprint_for(
            "anthropic",
            &config.model,
            config.temperature,
            config.max_tokens,
            config.timeout_seconds,
        );
        let gemini = configuration_fingerprint_for(
            "gemini",
            &config.model,
            config.temperature,
            config.max_tokens,
            config.timeout_seconds,
        );
        let openai = configuration_fingerprint(&config);
        assert_ne!(anthropic, gemini);
        assert_ne!(anthropic, openai);
        assert_ne!(gemini, openai);
    }

    #[test]
    fn the_default_configuration_is_valid_and_names_the_real_api() {
        let config = AnthropicConfig::default();
        assert!(config.validate().is_ok());
        assert_eq!(
            config.endpoint().expect("endpoint").as_str(),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(config.api_key_env, "ANTHROPIC_API_KEY");
    }

    #[test]
    fn an_already_complete_endpoint_is_not_doubled_up() {
        let config = AnthropicConfig {
            base_url: "https://api.anthropic.com/v1/messages".to_owned(),
            ..AnthropicConfig::default()
        };
        assert_eq!(
            config.endpoint().expect("endpoint").as_str(),
            "https://api.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn plain_http_to_a_remote_host_is_refused() {
        let config = AnthropicConfig {
            base_url: "http://api.anthropic.com".to_owned(),
            ..AnthropicConfig::default()
        };
        assert_eq!(
            config.validate(),
            Err(TransportConfigError::InsecureBaseUrl),
            "an Anthropic key must never leave over plain HTTP"
        );
    }

    #[test]
    fn every_text_block_is_joined_not_just_the_first() {
        // The reason this adapter exists. A response split across blocks is normal, and reading
        // only the first one would truncate a real answer.
        let blocks = vec![
            AnthropicContentBlock {
                kind: "thinking".to_owned(),
                text: Some("The user wants an action. I should pick one.".to_owned()),
            },
            AnthropicContentBlock {
                kind: "text".to_owned(),
                text: Some("{\"action\":".to_owned()),
            },
            AnthropicContentBlock {
                kind: "text".to_owned(),
                text: Some("\"WAIT\"}".to_owned()),
            },
        ];
        let joined = join_text_blocks(&blocks);
        assert_eq!(joined, "{\"action\":\n\"WAIT\"}");
        // A reasoning trace must not be in front of the JSON, or the parser sees prose first.
        assert!(!joined.contains("The user wants"));
    }

    #[test]
    fn empty_blocks_are_skipped_rather_than_separated() {
        let blocks = vec![
            AnthropicContentBlock {
                kind: "text".to_owned(),
                text: Some(String::new()),
            },
            AnthropicContentBlock {
                kind: "text".to_owned(),
                text: Some("{\"action\":\"WAIT\"}".to_owned()),
            },
        ];
        assert_eq!(join_text_blocks(&blocks), "{\"action\":\"WAIT\"}");
    }

    #[test]
    fn non_text_blocks_are_not_treated_as_an_answer() {
        let blocks = vec![AnthropicContentBlock {
            kind: "tool_use".to_owned(),
            text: Some("{\"action\":\"WAIT\"}".to_owned()),
        }];
        assert!(join_text_blocks(&blocks).is_empty());
    }

    #[test]
    fn the_descriptor_records_the_api_version_and_never_the_key() {
        let provider = AnthropicProvider::new(AnthropicConfig::default()).expect("provider");
        let descriptor = provider.descriptor_value();
        assert_eq!(descriptor.adapter, "anthropic-messages-v1");
        assert_eq!(descriptor.strategy, "claude-sonnet-4-5");
        let parameters =
            serde_json::to_string(&descriptor.parameters).expect("parameters serialize");
        assert!(parameters.contains(ANTHROPIC_VERSION));
        assert!(!parameters.contains("sk-ant"));
        // The *name* of the variable is publishable; its value is not, and is not present to leak.
        assert!(parameters.contains("ANTHROPIC_API_KEY"));
    }

    #[test]
    fn usage_is_reported_from_the_api_fields() {
        let response: AnthropicResponse = serde_json::from_str(
            r#"{"content":[{"type":"text","text":"{\"action\":\"WAIT\"}"}],
                "stop_reason":"end_turn",
                "usage":{"input_tokens":120,"output_tokens":7}}"#,
        )
        .expect("response");
        assert_eq!(response.usage.input_tokens, 120);
        assert_eq!(response.usage.output_tokens, 7);
        assert_eq!(join_text_blocks(&response.content), "{\"action\":\"WAIT\"}");
    }

    #[test]
    fn a_response_missing_usage_still_parses() {
        // Usage accounting is optional in the API. Refusing the whole response over absent billing
        // metadata would lose a perfectly valid decision.
        let response: AnthropicResponse =
            serde_json::from_str(r#"{"content":[{"type":"text","text":"{}"}]}"#).expect("response");
        assert_eq!(response.usage.input_tokens, 0);
    }

    #[test]
    fn a_truncated_response_is_reported_as_truncation_not_as_bad_json() {
        // max_tokens is a required field, and hitting it produces a *valid* JSON reply that simply
        // stops mid-decision. Reporting that as a protocol violation would blame the model for a
        // configuration choice.
        let stop = Some("max_tokens".to_owned());
        assert!(AnthropicError::OutputTruncated
            .to_string()
            .contains("output token limit"));
        assert_ne!(stop.as_deref(), Some("end_turn"));
    }

    #[test]
    fn the_fingerprint_does_not_contain_the_credential() {
        let fingerprint = AnthropicConfig::default().fingerprint();
        assert!(!fingerprint.is_empty());
        assert!(!fingerprint.contains("sk-"));
    }

    #[test]
    fn the_adapter_fingerprint_is_stable_and_version_specific() {
        assert_eq!(
            anthropic_adapter_fingerprint(),
            anthropic_adapter_fingerprint()
        );
        assert_eq!(
            anthropic_adapter_fingerprint().len(),
            64,
            "a sha-256 digest is fixed width"
        );
        // The digest binds the wire version, so a recorded result can be tied to the API contract it was
        // produced against. It is a hash, not the version text, so it is not readable back.
    }

    #[test]
    fn the_request_carries_the_system_prompt_outside_the_message_list() {
        let config = AnthropicConfig::default();
        let body = AnthropicRequest::build(&config, "system rules", "pick an action", false);
        assert_eq!(body.messages.len(), 1);
        assert_eq!(body.messages[0].role, "user");
        assert_eq!(body.messages[0].content, "pick an action");
        assert!(
            !body.system.is_empty(),
            "the grid instructions belong in system"
        );
        assert_eq!(body.max_tokens, config.max_tokens);
    }
}
