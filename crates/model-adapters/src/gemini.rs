//! The Google Gemini `generateContent` adapter.
//!
//! # What differs from the OpenAI shape
//!
//! The model name is part of the *path*, not the body, so it has to be escaped before it goes into a
//! URL. The system prompt is `systemInstruction`, not a message with `role: "system"`. The assistant
//! role is called `model`, not `assistant` — a detail worth naming, because sending `assistant`
//! produces a server-side 400 that looks like a malformed request rather than a wrong role.
//!
//! # Why the key goes in a header
//!
//! Gemini accepts the key either as an `x-goog-api-key` header or as a `?key=` query parameter. The
//! query form is refused here for the same reason it is refused in the base URL generally: a
//! credential in a URL leaks into proxy logs, browser history, and error messages that quote the URL.
//! The header form is the safer of the two, so it is the only one implemented.

use std::collections::BTreeMap;
use std::time::Instant;

use probelab_core::{ModelCallFailure, ModelCallFailureKind, ModelDescriptor, TokenUsage};
use reqwest::redirect::Policy;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::transport::{
    read_credential, sanitize_response_text, validate_api_key_env, validate_base_url,
    validate_model, validate_model_identifier, validate_sampling, validate_timeout,
    TransportConfigError,
};
use crate::{
    configuration_fingerprint, configuration_fingerprint_for, parse_decision_response, prompt_for,
    prompt_template_version_for, OpenAiCompatibleConfig, GRID_WORLD_ENVIRONMENT_VERSION,
};

/// The API version this adapter speaks.
pub const GEMINI_API_VERSION: &str = "v1beta";

/// Configuration for the Gemini `generateContent` API.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GeminiConfig {
    /// Base URL, normally `https://generativelanguage.googleapis.com`.
    pub base_url: String,
    /// Name of the environment variable holding the key.
    pub api_key_env: String,
    /// Model identifier, for example `gemini-2.5-pro`.
    pub model: String,
    /// Request timeout in seconds.
    pub timeout_seconds: u64,
    /// Sampling temperature.
    pub temperature: f64,
    /// Maximum output tokens.
    pub max_tokens: u32,
}

impl Default for GeminiConfig {
    fn default() -> Self {
        Self {
            base_url: "https://generativelanguage.googleapis.com".to_owned(),
            api_key_env: "GEMINI_API_KEY".to_owned(),
            model: "gemini-2.5-pro".to_owned(),
            timeout_seconds: 60,
            temperature: 0.0,
            max_tokens: 200,
        }
    }
}

impl GeminiConfig {
    /// Validates the configuration under the shared transport rules.
    pub fn validate(&self) -> Result<(), TransportConfigError> {
        self.endpoint()?;
        validate_api_key_env(&self.api_key_env)?;
        validate_model(&self.model)?;
        validate_timeout(self.timeout_seconds)?;
        validate_sampling(self.temperature, self.max_tokens)?;
        Ok(())
    }

    /// Resolves the `…/models/{model}:generateContent` endpoint.
    ///
    /// The model identifier is percent-encoded rather than pasted in. A model name is user-supplied
    /// text that ends up in a URL, and an unescaped `/` or `?` there would silently change which
    /// endpoint is called.
    pub fn endpoint(&self) -> Result<url::Url, TransportConfigError> {
        let base = validate_base_url(&self.base_url)?;
        validate_model_identifier(&self.model)?;
        let bare = self
            .model
            .trim()
            .strip_prefix("models/")
            .unwrap_or(self.model.trim());
        let path = base.path().trim_end_matches('/');
        let endpoint_path = if path.contains(":generateContent") {
            path.to_owned()
        } else {
            format!("{path}/{GEMINI_API_VERSION}/models/{bare}:generateContent")
        };
        let mut endpoint = base;
        // `set_path` on a `String` path would re-encode the encoded model, so the encoded form is
        // built by hand and assigned to the already-validated base.
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

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct GeminiPart {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    text: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct GeminiContent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    #[serde(default)]
    parts: Vec<GeminiPart>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
struct GeminiGenerationConfig {
    temperature: f64,
    max_output_tokens: u32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct GeminiRequest {
    contents: Vec<GeminiContent>,
    #[serde(rename = "systemInstruction", skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GeminiContent>,
    #[serde(rename = "generationConfig")]
    generation_config: GeminiGenerationConfig,
}

impl GeminiRequest {
    fn build(config: &GeminiConfig, system: &str, user: &str) -> Self {
        let system_instruction = GeminiContent {
            role: None,
            parts: vec![GeminiPart {
                text: Some(system.to_owned()),
            }],
        };
        Self {
            contents: vec![GeminiContent {
                // Gemini calls the assistant role "model". Sending "assistant" is a 400.
                role: Some("user".to_owned()),
                parts: vec![GeminiPart {
                    text: Some(user.to_owned()),
                }],
            }],
            system_instruction: Some(system_instruction),
            generation_config: GeminiGenerationConfig {
                temperature: config.temperature,
                max_output_tokens: config.max_tokens,
            },
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct GeminiCandidate {
    #[serde(default)]
    content: Option<GeminiContent>,
    #[serde(default, rename = "finishReason")]
    finish_reason: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
struct GeminiUsage {
    #[serde(default, rename = "promptTokenCount")]
    prompt_token_count: u64,
    #[serde(default, rename = "candidatesTokenCount")]
    candidates_token_count: u64,
    #[serde(default, rename = "totalTokenCount")]
    total_token_count: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct GeminiResponse {
    #[serde(default)]
    candidates: Vec<GeminiCandidate>,
    #[serde(default, rename = "usageMetadata")]
    usage_metadata: GeminiUsage,
    #[serde(default)]
    error: Option<GeminiErrorBody>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct GeminiErrorBody {
    #[serde(default)]
    message: String,
}

/// A failure specific to the Gemini wire format.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum GeminiError {
    /// The response carried no candidates at all.
    #[error("the Gemini response contained no candidates")]
    NoCandidates,
    /// The single candidate carried no text.
    #[error("the Gemini response contained no text part")]
    NoText,
    /// The candidate was cut off at the output limit.
    #[error("the model stopped at the output token limit before producing a decision")]
    OutputTruncated,
    /// The candidate was blocked by a safety filter.
    #[error("the response was blocked by a provider safety filter: {0}")]
    Blocked(String),
    /// The API reported an error object.
    #[error("the Gemini API returned an error: {0}")]
    ApiRejected(String),
}

/// Joins the text parts of a candidate into the single string the protocol parser expects.
fn join_text_parts(candidate: &GeminiCandidate) -> String {
    let Some(content) = candidate.content.as_ref() else {
        return String::new();
    };
    let mut out = String::new();
    for part in content.parts.iter().filter_map(|part| part.text.as_deref()) {
        if part.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(part);
    }
    out
}

/// Classifies a response into a decision or a typed failure.
///
/// Kept separate from the HTTP call so the classification is testable without a network, which is the
/// only way these rules get exercised at all in CI.
fn classify(response: &GeminiResponse) -> Result<String, GeminiError> {
    if let Some(error) = response.error.as_ref() {
        return Err(GeminiError::ApiRejected(error.message.clone()));
    }
    let Some(candidate) = response.candidates.first() else {
        return Err(GeminiError::NoCandidates);
    };
    if let Some(reason) = candidate.finish_reason.as_deref() {
        match reason {
            "MAX_TOKENS" => return Err(GeminiError::OutputTruncated),
            // A safety block is the provider refusing, not the model failing a protocol test. The
            // distinction matters: a blocked response must not be scored as an unparseable model.
            other if other.starts_with("SAFETY") || other.starts_with("PROHIBITED_CONTENT") => {
                return Err(GeminiError::Blocked(other.to_owned()));
            }
            _ => {}
        }
    }
    let text = join_text_parts(candidate);
    if text.is_empty() {
        return Err(GeminiError::NoText);
    }
    Ok(text)
}

/// The Gemini `generateContent` provider.
#[derive(Clone, Debug)]
pub struct GeminiProvider {
    config: GeminiConfig,
    client: Client,
}

impl GeminiProvider {
    /// Builds a provider, validating the configuration first.
    pub fn new(config: GeminiConfig) -> Result<Self, TransportConfigError> {
        config.validate()?;
        let client = Client::builder()
            .timeout(validate_timeout(config.timeout_seconds)?)
            .redirect(Policy::none())
            .build()
            .map_err(|_| TransportConfigError::InvalidBaseUrl)?;
        Ok(Self { config, client })
    }

    /// The configuration in use.
    pub fn config(&self) -> &GeminiConfig {
        &self.config
    }

    /// The descriptor recorded with a result.
    pub fn descriptor_value(&self) -> ModelDescriptor {
        let mut parameters = BTreeMap::new();
        parameters.insert(
            "provider".to_owned(),
            serde_json::Value::String("gemini".to_owned()),
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
            serde_json::Value::String(GEMINI_API_VERSION.to_owned()),
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
         * The three reproducibility fields, which were absent here for the same reason they were absent
         * in the Anthropic adapter: the engine reads them out of the descriptor to build
         * `ReproducibilityMetadata`, and a key that is not inserted arrives as an empty string rather
         * than as an error. A recorded Gemini run therefore stored three blanks where the
         * OpenAI-compatible adapter stored real values, and read as though the prompt version and
         * configuration were known but unfilled.
         *
         * The same shared `configuration_fingerprint` is used, so a fingerprint means the same thing
         * across vendors. `prompt_template_version_for(false)` is the non-spawning template, which is
         * the only one this adapter sends.
         */
        parameters.insert(
            "configuration_fingerprint".to_owned(),
            serde_json::Value::String(configuration_fingerprint_for(
                "gemini",
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
            adapter: "gemini-generate-content-v1".to_owned(),
            strategy: self.config.model.clone(),
            seed: 0,
            parameters,
        }
    }
}

impl probelab_core::AsyncModelProvider for GeminiProvider {
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

impl GeminiProvider {
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
                format!(
                    "the Gemini credential ({}) is unavailable: {error}",
                    self.config.api_key_env
                ),
            )
        })?;
        let (system, user) = prompt_for(request.process_spawn_available);
        let endpoint = self.config.endpoint().map_err(|error| {
            ModelCallFailure::new(ModelCallFailureKind::Configuration, error.to_string())
        })?;
        let body = GeminiRequest::build(&self.config, system, user);
        let started = Instant::now();

        let response = self
            .client
            .post(endpoint)
            .header("x-goog-api-key", &credential)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|error| {
                let kind = if error.is_timeout() {
                    ModelCallFailureKind::Timeout
                } else {
                    ModelCallFailureKind::TransportDisconnect
                };
                ModelCallFailure {
                    kind,
                    message: "the Gemini request failed in transport".to_owned(),
                    response_text: Some(sanitize_response_text(&error.to_string(), &credential)),
                    usage: None,
                    elapsed_ms: Some(elapsed_ms(started)),
                    http_status: None,
                }
            })?;

        let status = response.status();
        let raw = response.text().await.map_err(|error| ModelCallFailure {
            kind: ModelCallFailureKind::TransportDisconnect,
            message: "the Gemini response body could not be read".to_owned(),
            response_text: Some(sanitize_response_text(&error.to_string(), &credential)),
            usage: None,
            elapsed_ms: Some(elapsed_ms(started)),
            http_status: None,
        })?;

        if !status.is_success() {
            // Classified through the shared rules, for the same reason the Anthropic adapter does:
            // one classification for every adapter, so a UI branch written once works on all three.
            let kind = crate::classify_http_failure(status.as_u16(), &raw);
            return Err(ModelCallFailure {
                kind,
                message: format!(
                    "the Gemini API returned HTTP {}: {}",
                    status.as_u16(),
                    crate::first_useful_line(&raw)
                ),
                response_text: Some(sanitize_response_text(&raw, &credential)),
                usage: None,
                elapsed_ms: Some(elapsed_ms(started)),
                http_status: Some(status.as_u16()),
            });
        }

        let parsed: GeminiResponse =
            serde_json::from_str(&raw).map_err(|error| ModelCallFailure {
                kind: ModelCallFailureKind::MalformedResponse,
                message: "the Gemini response was not the expected JSON shape".to_owned(),
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
            prompt_tokens: Some(parsed.usage_metadata.prompt_token_count),
            completion_tokens: Some(parsed.usage_metadata.candidates_token_count),
            total_tokens: Some(parsed.usage_metadata.total_token_count),
        };

        // A safety block and a truncated completion are both the provider declining to finish, not the
        // model failing the protocol. `ProviderError` says exactly that, so neither is scored as a
        // model that cannot follow instructions.
        let text = match classify(&parsed) {
            Ok(text) => text,
            Err(GeminiError::OutputTruncated) => {
                return Err(ModelCallFailure {
                    kind: ModelCallFailureKind::EmptyResponse,
                    message: GeminiError::OutputTruncated.to_string(),
                    response_text: Some(sanitize_response_text(&raw, &credential)),
                    usage: Some(usage),
                    elapsed_ms: Some(elapsed),
                    http_status: Some(status.as_u16()),
                })
            }
            Err(error) => {
                return Err(ModelCallFailure {
                    kind: ModelCallFailureKind::ProviderError,
                    message: error.to_string(),
                    response_text: Some(sanitize_response_text(&raw, &credential)),
                    usage: Some(usage),
                    elapsed_ms: Some(elapsed),
                    http_status: Some(status.as_u16()),
                })
            }
        };

        let sanitized = sanitize_response_text(&text, &credential);
        let decision = match parse_decision_response(
            &text,
            &request.available_actions,
            request.process_spawn_available,
        ) {
            Ok(decision) => decision,
            Err(error) => {
                return Err(ModelCallFailure {
                    kind: error.kind(),
                    message: error.to_string(),
                    response_text: Some(sanitized),
                    usage: Some(usage),
                    elapsed_ms: Some(elapsed),
                    http_status: Some(status.as_u16()),
                })
            }
        };

        Ok(probelab_core::ModelResponse {
            decision,
            call: Some(probelab_core::ModelCallInfo {
                provider: "gemini".to_owned(),
                model: self.config.model.clone(),
                prompt_template_version: prompt_template_version_for(
                    request.process_spawn_available,
                )
                .to_owned(),
                request_id: None,
                finish_reason: parsed
                    .candidates
                    .first()
                    .and_then(|c| c.finish_reason.clone()),
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
#[cfg(test)]
mod tests {
    use super::*;

    /// The three reproducibility fields the engine reads out of the descriptor.
    ///
    /// Absent here for the same reason they were absent in the Anthropic adapter, and absence is
    /// invisible: the engine's `descriptor_string` filters empty values, so a missing key and a blank
    /// one both became an empty string in the stored result. A recorded Gemini run reported no prompt
    /// template version, no configuration fingerprint, and no environment version.
    #[test]
    fn the_descriptor_carries_every_reproducibility_field() {
        let provider = GeminiProvider::new(GeminiConfig::default()).expect("valid config");
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
        let base = GeminiConfig::default();
        let build = |config: GeminiConfig| {
            GeminiProvider::new(config)
                .expect("valid config")
                .descriptor_value()
        };
        let read = |descriptor: &probelab_core::ModelDescriptor| {
            descriptor.parameters["configuration_fingerprint"]
                .as_str()
                .expect("a string")
                .to_owned()
        };

        assert_eq!(read(&build(base.clone())), read(&build(base.clone())));
        assert_ne!(
            read(&build(base.clone())),
            read(&build(GeminiConfig {
                model: "gemini-other".to_owned(),
                ..base.clone()
            })),
            "the model is model-affecting"
        );
        assert_ne!(
            read(&build(base.clone())),
            read(&build(GeminiConfig {
                max_tokens: base.max_tokens + 64,
                ..base.clone()
            })),
            "the token budget is model-affecting"
        );
    }

    #[test]
    fn the_default_configuration_is_valid_and_names_the_real_api() {
        let config = GeminiConfig::default();
        assert!(config.validate().is_ok());
        assert_eq!(
            config.endpoint().expect("endpoint").as_str(),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-pro:generateContent"
        );
        assert_eq!(config.api_key_env, "GEMINI_API_KEY");
    }

    #[test]
    fn a_models_prefix_is_not_doubled() {
        let config = GeminiConfig {
            model: "models/gemini-2.5-flash".to_owned(),
            ..GeminiConfig::default()
        };
        let endpoint = config.endpoint().expect("endpoint");
        assert!(endpoint
            .path()
            .ends_with("/models/gemini-2.5-flash:generateContent"));
        assert!(!endpoint.path().contains("models/models"));
    }

    #[test]
    fn a_model_name_cannot_smuggle_a_different_endpoint() {
        // The model identifier goes into a URL. If it were pasted in unescaped, a name containing a
        // slash or a query would change which endpoint is called.
        let config = GeminiConfig {
            model: "gemini/../../admin?x=1".to_owned(),
            ..GeminiConfig::default()
        };
        assert!(config.validate().is_err(), "this name must not be accepted");
    }

    #[test]
    fn plain_http_to_a_remote_host_is_refused() {
        let config = GeminiConfig {
            base_url: "http://generativelanguage.googleapis.com".to_owned(),
            ..GeminiConfig::default()
        };
        assert_eq!(
            config.validate(),
            Err(TransportConfigError::InsecureBaseUrl)
        );
    }

    #[test]
    fn a_credential_in_a_query_string_is_refused() {
        // The ?key= form works but leaks into proxy logs, so the base URL refuses it outright.
        let config = GeminiConfig {
            base_url: "https://generativelanguage.googleapis.com?key=secret".to_owned(),
            ..GeminiConfig::default()
        };
        assert_eq!(config.validate(), Err(TransportConfigError::UrlQuery));
    }

    #[test]
    fn the_request_uses_the_model_role_not_assistant() {
        let config = GeminiConfig::default();
        let body = GeminiRequest::build(&config, "system rules", "pick an action");
        assert_eq!(body.contents[0].role.as_deref(), Some("user"));
        let system = body
            .system_instruction
            .expect("grid instructions belong in systemInstruction");
        assert!(!system.parts[0]
            .text
            .as_deref()
            .unwrap_or_default()
            .is_empty());
        assert_eq!(body.generation_config.max_output_tokens, config.max_tokens);
    }

    #[test]
    fn a_normal_response_yields_the_model_text() {
        let response: GeminiResponse = serde_json::from_str(
            r#"{"candidates":[{"content":{"role":"model",
                "parts":[{"text":"{\"action\":\"WAIT\"}"}]},"finishReason":"STOP"}],
                "usageMetadata":{"promptTokenCount":90,"candidatesTokenCount":6,"totalTokenCount":96}}"#,
        )
        .expect("response");
        assert_eq!(classify(&response).expect("text"), "{\"action\":\"WAIT\"}");
        assert_eq!(response.usage_metadata.total_token_count, 96);
    }

    #[test]
    fn a_truncated_candidate_is_truncation_not_a_protocol_failure() {
        let response: GeminiResponse = serde_json::from_str(
            r#"{"candidates":[{"content":{"parts":[{"text":"{\"action\":"}]},"finishReason":"MAX_TOKENS"}]}"#,
        )
        .expect("response");
        assert_eq!(
            classify(&response),
            Err(GeminiError::OutputTruncated),
            "a cut-off completion is a token-budget outcome, not a model that cannot follow the protocol"
        );
    }

    #[test]
    fn a_safety_block_is_reported_as_a_block_not_as_bad_json() {
        let response: GeminiResponse = serde_json::from_str(
            r#"{"candidates":[{"content":{"parts":[{"text":"I can't help with that"}]},
                "finishReason":"SAFETY"}]}"#,
        )
        .expect("response");
        assert!(
            matches!(classify(&response), Err(GeminiError::Blocked(_))),
            "a provider refusal must not be scored as an unparseable model response"
        );
    }

    #[test]
    fn an_api_error_body_is_reported_rather_than_parsed_as_a_candidate() {
        let response: GeminiResponse =
            serde_json::from_str(r#"{"error":{"message":"API key not valid"}}"#).expect("response");
        assert!(matches!(
            classify(&response),
            Err(GeminiError::ApiRejected(_))
        ));
    }

    #[test]
    fn an_empty_response_is_distinguished_from_a_missing_one() {
        let empty: GeminiResponse = serde_json::from_str(r#"{"candidates":[]}"#).expect("response");
        assert_eq!(classify(&empty), Err(GeminiError::NoCandidates));
        let no_text: GeminiResponse =
            serde_json::from_str(r#"{"candidates":[{"content":{"parts":[]}}]}"#).expect("response");
        assert_eq!(classify(&no_text), Err(GeminiError::NoText));
    }

    #[test]
    fn every_text_part_is_joined() {
        let response: GeminiResponse = serde_json::from_str(
            r#"{"candidates":[{"content":{"parts":[{"text":"{\"action\":"},{"text":"\"WAIT\"}"}]}}]}"#,
        )
        .expect("response");
        assert_eq!(
            classify(&response).expect("text"),
            "{\"action\":\n\"WAIT\"}"
        );
    }

    #[test]
    fn the_descriptor_records_the_version_and_never_the_key() {
        let provider = GeminiProvider::new(GeminiConfig::default()).expect("provider");
        let descriptor = provider.descriptor_value();
        let parameters =
            serde_json::to_string(&descriptor.parameters).expect("parameters serialize");
        assert!(parameters.contains(GEMINI_API_VERSION));
        assert!(parameters.contains("GEMINI_API_KEY"));
        assert!(!parameters.contains("AIza"));
    }
}
