//! Versioned, credential-free model configuration and provider status for the IPC boundary.
//!
//! The UI may only select *how* a model is reached. It can never supply a secret. This module
//! therefore exposes exactly two things to the frontend:
//!
//! 1. [`ModelRunConfig`], a tagged, `deny_unknown_fields` union with two variants. The
//!    `openai_compatible` variant names an *environment variable* and never its value, and
//!    accepts no API key, no header map, and no free-form endpoint object.
//! 2. [`ProviderStatus`], a human-facing summary that reports only whether a run *could* start.
//!
//! The API key itself is read inside the trusted model adapter
//! ([`probelab_model_adapters::OpenAiCompatibleProvider`]) directly from the process
//! environment, so it never crosses the IPC boundary in either direction.

use probelab_agent::{AsyncProviderAgent, ProviderAgent};
use probelab_core::{Agent, ModelProvider};
use probelab_model_adapters::{
    MockModelConfig, MockModelProvider, OpenAiCompatibleConfig, OpenAiCompatibleConfigError,
    OpenAiCompatibleProvider, GRID_WORLD_PROMPT_TEMPLATE_VERSION, MOCK_PROMPT_TEMPLATE_VERSION,
};
use serde::{Deserialize, Serialize};

/// Version of the tagged [`ModelRunConfig`] IPC contract understood by this build.
///
/// A version mismatch fails closed rather than being silently coerced, so an older or newer
/// frontend cannot smuggle an unrecognised field past `deny_unknown_fields`.
pub const MODEL_RUN_CONFIG_SCHEMA_VERSION: u32 = 1;

/// Version of the [`ProviderStatus`] IPC contract understood by this build.
pub const PROVIDER_STATUS_SCHEMA_VERSION: u32 = 1;

/// Stable provider identifier reported for the deterministic in-process mock.
pub const MOCK_PROVIDER: &str = "deterministic-mock";

/// Stable provider identifier reported for the OpenAI-compatible provider.
pub const OPENAI_COMPATIBLE_PROVIDER: &str = "openai-compatible";

/// Generic, non-identifying description of where a credential is read from.
///
/// The concrete environment-variable name stays with the operator who configured it. The
/// backend reports only this generic label and never echoes a variable name, a value, or
/// anything derived from them in provider status.
pub const CREDENTIAL_SOURCE_ENVIRONMENT: &str = "environment";

/// Credential source label for a provider that requires no credential.
pub const CREDENTIAL_SOURCE_NONE: &str = "none";

/// Serializes [`MockModelConfig`] as a bare strategy name instead of a nested tag object.
///
/// The adapter enum is internally tagged, so reusing it directly would nest a second
/// `"strategy"` object inside the already tagged IPC payload. This keeps the mock variant
/// flat while preserving the existing `strategy` field name and values.
mod flat_strategy {
    use probelab_model_adapters::MockModelConfig;
    use serde::{de::Error as _, Deserialize, Deserializer, Serializer};

    /// Writes the stable strategy name as a plain string.
    pub fn serialize<S>(strategy: &MockModelConfig, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(strategy.strategy_name())
    }

    /// Reads a stable strategy name back into the adapter enum.
    pub fn deserialize<'de, D>(deserializer: D) -> Result<MockModelConfig, D::Error>
    where
        D: Deserializer<'de>,
    {
        let name = String::deserialize(deserializer)?;
        const CANONICAL_STRATEGIES: [&str; 10] = [
            "optimal",
            "random",
            "greedy_mistake",
            "invalid_action_spam",
            "security_probe_unauthorized_access",
            "security_probe_policy_violation",
            "security_probe_sandbox_violation",
            "security_probe_environment_tampering",
            "security_probe_network_violation",
            "forbidden_action_spam",
        ];
        if !CANONICAL_STRATEGIES.contains(&name.as_str()) {
            return Err(D::Error::custom("unknown mock strategy"));
        }
        // `MockModelConfig` is internally tagged, so it needs its tag object re-wrapped.
        serde_json::from_value(serde_json::json!({ "strategy": name }))
            .map_err(|error| D::Error::custom(format!("unknown mock strategy: {error}")))
    }
}

/// A tagged, versioned, credential-free description of the model a run should use.
///
/// The wire shape is internally tagged by the `kind` field:
///
/// ```json
/// { "kind": "mock", "schema_version": 1, "strategy": "optimal" }
/// ```
///
/// ```json
/// {
///   "kind": "openai_compatible",
///   "schema_version": 1,
///   "base_url": "http://127.0.0.1:11434/v1",
///   "api_key_env": "OPENAI_COMPATIBLE_API_KEY",
///   "model": "local-model",
///   "timeout_seconds": 30,
///   "temperature": 0.0,
///   "max_tokens": 200
/// }
/// ```
///
/// Both variants use `deny_unknown_fields`, so an `api_key`, a `headers` map, or any other
/// undeclared field is rejected by deserialization rather than silently ignored. The model
/// itself never sees this structure; it only ever receives a
/// [`probelab_core::ModelRequest`] built from its local observation.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModelRunConfig {
    /// Deterministic in-process mock provider. Requires no credential and no network.
    Mock {
        /// IPC contract version, which must equal [`MODEL_RUN_CONFIG_SCHEMA_VERSION`].
        schema_version: u32,
        /// Deterministic mock strategy, sent as its stable snake_case strategy name.
        #[serde(with = "flat_strategy")]
        strategy: MockModelConfig,
    },
    /// OpenAI-compatible chat-completions provider reached over the network.
    #[serde(rename = "openai_compatible")]
    OpenAiCompatible {
        /// IPC contract version, which must equal [`MODEL_RUN_CONFIG_SCHEMA_VERSION`].
        schema_version: u32,
        /// Base URL or complete chat-completions endpoint. Credentials are not allowed in it.
        base_url: String,
        /// Name of the environment variable that holds the API key, never the key itself.
        api_key_env: String,
        /// Model identifier sent to the provider.
        model: String,
        /// Request timeout in seconds.
        timeout_seconds: u64,
        /// Sampling temperature.
        temperature: f64,
        /// Maximum completion tokens.
        max_tokens: u32,
    },
}

/// A typed failure raised before any benchmark execution begins.
#[derive(Debug, thiserror::Error)]
pub enum ModelRunConfigError {
    /// The frontend and backend disagree about the tagged IPC contract version.
    #[error("unsupported model run config schema version {actual}; this build supports version {expected}")]
    UnsupportedSchemaVersion {
        /// Version this build understands.
        expected: u32,
        /// Version supplied by the caller.
        actual: u32,
    },
    /// The OpenAI-compatible settings failed trusted validation.
    #[error(transparent)]
    InvalidOpenAiCompatibleConfig(#[from] OpenAiCompatibleConfigError),
    /// The asynchronous provider agent could not be constructed.
    #[error("failed to create the async provider agent: {0}")]
    Agent(String),
    /// The configured environment variable does not hold a usable credential.
    #[error(
        "the environment variable {variable} is not set to a non-empty value; \
         export the API key there before running a connected model"
    )]
    MissingCredential {
        /// Name of the configured environment variable, which is not a secret.
        variable: String,
    },
}

/// Coarse connection state shown to the operator.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderConnectionStatus {
    /// A run using this configuration could start.
    Connected,
    /// A run using this configuration would be rejected before it starts.
    NotConfigured,
}

/// Human-facing, credential-free provider status for the operator UI.
///
/// This type is deliberately incapable of carrying a secret: it has no field for an API key,
/// an environment-variable value, an endpoint credential, or a request header.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderStatus {
    /// Version of this status contract.
    pub schema_version: u32,
    /// Stable provider identifier.
    pub provider: String,
    /// Provider model identifier, without endpoint or credential material.
    pub model: String,
    /// Version of the prompt template this provider would receive.
    pub prompt_template_version: String,
    /// Generic description of the credential source, such as `environment`.
    pub credential_source: String,
    /// Whether a run with this configuration could start.
    pub configured: bool,
    /// Coarse connection state.
    pub status: ProviderConnectionStatus,
    /// Human-readable explanation suitable for direct display.
    pub message: String,
}

impl ProviderStatus {
    /// Returns a not-configured status for a configuration that failed validation.
    fn not_configured(
        provider: &str,
        model: String,
        prompt_template_version: &str,
        reason: String,
    ) -> Self {
        Self {
            schema_version: PROVIDER_STATUS_SCHEMA_VERSION,
            provider: provider.to_owned(),
            model,
            prompt_template_version: prompt_template_version.to_owned(),
            credential_source: CREDENTIAL_SOURCE_ENVIRONMENT.to_owned(),
            configured: false,
            status: ProviderConnectionStatus::NotConfigured,
            message: reason,
        }
    }
}

impl ModelRunConfig {
    /// Creates a mock configuration at the current contract version.
    pub fn mock(strategy: MockModelConfig) -> Self {
        Self::Mock {
            schema_version: MODEL_RUN_CONFIG_SCHEMA_VERSION,
            strategy,
        }
    }

    /// Creates an OpenAI-compatible configuration at the current contract version.
    pub fn openai_compatible(config: &OpenAiCompatibleConfig) -> Self {
        Self::OpenAiCompatible {
            schema_version: MODEL_RUN_CONFIG_SCHEMA_VERSION,
            base_url: config.base_url.clone(),
            api_key_env: config.api_key_env.clone(),
            model: config.model.clone(),
            timeout_seconds: config.timeout_seconds,
            temperature: config.temperature,
            max_tokens: config.max_tokens,
        }
    }

    /// Returns a stable, non-secret label for the selected provider kind.
    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::Mock { .. } => MOCK_PROVIDER,
            Self::OpenAiCompatible { .. } => OPENAI_COMPATIBLE_PROVIDER,
        }
    }

    /// Returns the contract version supplied by the caller.
    pub fn schema_version(&self) -> u32 {
        match self {
            Self::Mock { schema_version, .. } | Self::OpenAiCompatible { schema_version, .. } => {
                *schema_version
            }
        }
    }

    /// Converts the connected variant into the adapter's own configuration settings.
    ///
    /// The mock variant has no network settings, so it yields the adapter defaults; callers
    /// must branch on the variant before using the result.
    pub fn openai_config(&self) -> OpenAiCompatibleConfig {
        match self {
            Self::Mock { .. } => OpenAiCompatibleConfig::default(),
            Self::OpenAiCompatible {
                base_url,
                api_key_env,
                model,
                timeout_seconds,
                temperature,
                max_tokens,
                ..
            } => OpenAiCompatibleConfig {
                base_url: base_url.clone(),
                api_key_env: api_key_env.clone(),
                model: model.clone(),
                timeout_seconds: *timeout_seconds,
                temperature: *temperature,
                max_tokens: *max_tokens,
            },
        }
    }

    /// Rejects any configuration whose version or settings this build cannot honour.
    ///
    /// This runs before a run is accepted, so an invalid configuration never reaches the
    /// engine and never produces a partially written result.
    pub fn validate(&self) -> Result<(), ModelRunConfigError> {
        let actual = self.schema_version();
        if actual != MODEL_RUN_CONFIG_SCHEMA_VERSION {
            return Err(ModelRunConfigError::UnsupportedSchemaVersion {
                expected: MODEL_RUN_CONFIG_SCHEMA_VERSION,
                actual,
            });
        }
        match self {
            Self::Mock { .. } => Ok(()),
            Self::OpenAiCompatible { .. } => self.openai_config().validate().map_err(Into::into),
        }
    }

    /// Returns the safe status an operator sees for this configuration.
    ///
    /// The status command and the run-time pre-flight share [`Self::validate`] and the
    /// credential presence check, so the card can never claim `connected` for a run that
    /// would be rejected.
    pub fn provider_status(&self) -> ProviderStatus {
        match self {
            Self::Mock { strategy, .. } => ProviderStatus {
                schema_version: PROVIDER_STATUS_SCHEMA_VERSION,
                provider: MOCK_PROVIDER.to_owned(),
                model: mock_model_label(*strategy),
                prompt_template_version: MOCK_PROMPT_TEMPLATE_VERSION.to_owned(),
                credential_source: CREDENTIAL_SOURCE_NONE.to_owned(),
                configured: true,
                status: ProviderConnectionStatus::Connected,
                message:
                    "Deterministic mock provider is available locally. No credential and no network access are required."
                        .to_owned(),
            },
            Self::OpenAiCompatible { model, .. } => {
                let model = model.clone();
                let connected = || ProviderStatus {
                    schema_version: PROVIDER_STATUS_SCHEMA_VERSION,
                    provider: OPENAI_COMPATIBLE_PROVIDER.to_owned(),
                    model: model.clone(),
                    prompt_template_version: GRID_WORLD_PROMPT_TEMPLATE_VERSION.to_owned(),
                    credential_source: CREDENTIAL_SOURCE_ENVIRONMENT.to_owned(),
                    configured: true,
                    status: ProviderConnectionStatus::Connected,
                    message: "Credential found in the environment. A connected run is ready to start."
                        .to_owned(),
                };
                let not_configured = |reason: ModelRunConfigError| {
                    ProviderStatus::not_configured(
                        OPENAI_COMPATIBLE_PROVIDER,
                        model.clone(),
                        GRID_WORLD_PROMPT_TEMPLATE_VERSION,
                        match reason {
                            ModelRunConfigError::MissingCredential { .. } => {
                                "The API key environment variable is not set to a non-empty value."
                                    .to_owned()
                            }
                            other => other.to_string(),
                        },
                    )
                };
                let credential_variable = match self {
                    Self::OpenAiCompatible { api_key_env, .. } => api_key_env.clone(),
                    Self::Mock { .. } => String::new(),
                };
                self.validate()
                    .and_then(|()| credential_is_present(&credential_variable))
                    .map_or_else(not_configured, |()| connected())
            }
        }
    }

    /// Builds the agent a run should use, after validating the configuration.
    ///
    /// For a connected configuration this fails closed when the credential is missing; it
    /// never fabricates a provider that would fail later inside the sandbox.
    pub fn build_agent(&self, benchmark_seed: u64) -> Result<Box<dyn Agent>, ModelRunConfigError> {
        self.validate()?;
        match self {
            Self::Mock { strategy, .. } => Ok(Box::new(ProviderAgent::from_provider(
                MockModelProvider::new(*strategy, benchmark_seed),
            ))),
            Self::OpenAiCompatible { api_key_env, .. } => {
                credential_is_present(api_key_env)?;
                let provider = OpenAiCompatibleProvider::new(self.openai_config())?;
                let agent = AsyncProviderAgent::from_provider(provider)
                    .map_err(ModelRunConfigError::Agent)?;
                Ok(Box::new(agent))
            }
        }
    }
}

/// Reads the configured environment variable purely for presence and reports no value.
///
/// The value never leaves this function: it is compared for emptiness and dropped.
fn credential_is_present(api_key_env: &str) -> Result<(), ModelRunConfigError> {
    let missing = || ModelRunConfigError::MissingCredential {
        variable: api_key_env.to_owned(),
    };
    match std::env::var(api_key_env) {
        Ok(value) if value.trim().is_empty() => Err(missing()),
        Ok(_) => Ok(()),
        Err(_) => Err(missing()),
    }
}

fn mock_model_label(strategy: MockModelConfig) -> String {
    MockModelProvider::new(strategy, 0)
        .descriptor()
        .parameters
        .get("model")
        .and_then(|value| value.as_str())
        .map_or_else(|| strategy.strategy_name().to_owned(), str::to_owned)
}

#[cfg(test)]
mod tests {
    use probelab_model_adapters::{MockModelConfig, OpenAiCompatibleConfig};

    use super::{
        mock_model_label, ModelRunConfig, ModelRunConfigError, ProviderConnectionStatus,
        MODEL_RUN_CONFIG_SCHEMA_VERSION,
    };

    /// Each test uses a distinct variable name because `cargo test` runs them in parallel
    /// threads that share one process environment.
    const MISSING_KEY_VARIABLE: &str = "PROBELAB_DESKTOP_TEST_MISSING_KEY";
    const PRESENT_KEY_VARIABLE: &str = "PROBELAB_DESKTOP_TEST_PRESENT_KEY";
    const INSECURE_KEY_VARIABLE: &str = "PROBELAB_DESKTOP_TEST_INSECURE_KEY";

    fn connected_config(api_key_env: &str) -> ModelRunConfig {
        ModelRunConfig::openai_compatible(&OpenAiCompatibleConfig {
            base_url: "https://provider.example/v1".to_owned(),
            api_key_env: api_key_env.to_owned(),
            model: "test-model".to_owned(),
            timeout_seconds: 5,
            temperature: 0.0,
            max_tokens: 64,
        })
    }

    #[test]
    fn mock_config_round_trips_through_the_tagged_wire_shape() {
        let config = ModelRunConfig::mock(MockModelConfig::Optimal);
        let json = serde_json::to_string(&config).expect("serialize mock config");
        assert_eq!(
            json,
            r#"{"kind":"mock","schema_version":1,"strategy":"optimal"}"#
        );
        assert_eq!(
            serde_json::from_str::<ModelRunConfig>(&json).expect("deserialize mock config"),
            config
        );
    }

    #[test]
    fn connected_config_round_trips_and_rejects_secrets_and_headers() {
        let config = connected_config(MISSING_KEY_VARIABLE);
        let json = serde_json::to_string(&config).expect("serialize connected config");
        assert!(json.contains(r#""kind":"openai_compatible""#));
        assert!(json.contains(r#""api_key_env":"PROBELAB_DESKTOP_TEST_MISSING_KEY""#));
        assert!(!json.contains("secret"));
        assert_eq!(
            serde_json::from_str::<ModelRunConfig>(&json).expect("deserialize connected config"),
            config
        );

        for forbidden in [
            r#"{"kind":"openai_compatible","schema_version":1,"base_url":"https://p.example/v1","api_key_env":"K","model":"m","timeout_seconds":1,"temperature":0.0,"max_tokens":1,"api_key":"secret"}"#,
            r#"{"kind":"openai_compatible","schema_version":1,"base_url":"https://p.example/v1","api_key_env":"K","model":"m","timeout_seconds":1,"temperature":0.0,"max_tokens":1,"headers":{"Authorization":"Bearer secret"}}"#,
            r#"{"kind":"mock","schema_version":1,"strategy":"optimal","endpoint":"https://evil.example"}"#,
            r#"{"kind":"mock","schema_version":1,"strategy":"security_probe_network"}"#,
            r#"{"kind":"unknown_provider","schema_version":1}"#,
        ] {
            assert!(
                serde_json::from_str::<ModelRunConfig>(forbidden).is_err(),
                "must reject {forbidden}"
            );
        }
    }

    #[test]
    fn unsupported_schema_version_fails_closed() {
        let error = ModelRunConfig::Mock {
            schema_version: MODEL_RUN_CONFIG_SCHEMA_VERSION + 1,
            strategy: MockModelConfig::Random,
        }
        .validate()
        .expect_err("future version must be rejected");
        assert!(matches!(
            error,
            ModelRunConfigError::UnsupportedSchemaVersion { .. }
        ));
    }

    #[test]
    fn mock_status_is_always_connected() {
        let status = ModelRunConfig::mock(MockModelConfig::Optimal).provider_status();
        assert!(status.configured);
        assert_eq!(status.status, ProviderConnectionStatus::Connected);
        assert_eq!(status.provider, "deterministic-mock");
        assert_eq!(status.prompt_template_version, "deterministic_mock_v1");
        assert_eq!(status.credential_source, "none");
        assert_eq!(
            mock_model_label(MockModelConfig::Optimal),
            "native-mock-optimal"
        );
    }

    #[test]
    fn connected_status_is_not_configured_without_a_credential() {
        let status = connected_config(MISSING_KEY_VARIABLE).provider_status();
        assert!(!status.configured);
        assert_eq!(status.status, ProviderConnectionStatus::NotConfigured);
        assert!(status.message.contains("environment variable"));
        assert!(!status.message.contains(MISSING_KEY_VARIABLE));
        assert_eq!(status.provider, "openai-compatible");
        assert_eq!(status.model, "test-model");
        assert_eq!(status.prompt_template_version, "grid_world_v2");
        assert_eq!(status.credential_source, "environment");
    }

    #[test]
    fn connected_status_is_connected_only_with_a_credential_present() {
        std::env::set_var(PRESENT_KEY_VARIABLE, "test-only-secret");
        let config = connected_config(PRESENT_KEY_VARIABLE);
        let status = config.provider_status();
        let built = config.build_agent(123);
        std::env::remove_var(PRESENT_KEY_VARIABLE);

        assert!(status.configured);
        assert_eq!(status.status, ProviderConnectionStatus::Connected);
        assert_eq!(
            built
                .expect("credential present builds a connected agent")
                .descriptor()
                .adapter,
            "openai-compatible-v1"
        );
        let serialized = serde_json::to_string(&status).expect("serialize status");
        assert!(!serialized.contains("test-only-secret"));
    }

    #[test]
    fn connected_build_agent_refuses_to_fabricate_a_provider() {
        let Err(error) = connected_config(MISSING_KEY_VARIABLE).build_agent(123) else {
            panic!("missing credential must fail before the run starts");
        };
        assert!(matches!(
            error,
            ModelRunConfigError::MissingCredential { .. }
        ));
        assert!(error.to_string().contains("export the API key"));
    }

    #[test]
    fn invalid_connected_settings_are_rejected_before_a_run_starts() {
        let config = ModelRunConfig::openai_compatible(&OpenAiCompatibleConfig {
            base_url: "http://provider.example/v1".to_owned(),
            api_key_env: INSECURE_KEY_VARIABLE.to_owned(),
            ..OpenAiCompatibleConfig::default()
        });
        std::env::set_var(INSECURE_KEY_VARIABLE, "test-only-secret");
        let built = config.build_agent(123);
        std::env::remove_var(INSECURE_KEY_VARIABLE);
        let Err(error) = built else {
            panic!("an insecure base URL must fail before a run starts");
        };
        assert_eq!(
            config.provider_status().status,
            ProviderConnectionStatus::NotConfigured
        );
        assert!(matches!(
            error,
            ModelRunConfigError::InvalidOpenAiCompatibleConfig(_)
        ));
    }
}
