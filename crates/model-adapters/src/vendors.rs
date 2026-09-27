//! One place to name, configure, and build every supported provider vendor.
//!
//! # Why this exists
//!
//! Before this module, a user who wanted to compare two vendors needed a different configuration struct,
//! a different constructor, and a different error type for each. The desktop IPC layer carried one
//! config per vendor and the UI grew a branch per vendor, so "connect a provider" was three edits in
//! three places and the third was always forgotten.
//!
//! [`ProviderKind`] and [`AnyProvider`] collapse that. The UI lists `ProviderKind::all()`, builds one
//! [`AnyProvider`], and the experiment manager is handed the same object regardless of which model is
//! actually being evaluated.
//!
//! # Why there is no Ollama-native variant
//!
//! Ollama is reached through the OpenAI-compatible endpoint, and that path is verified working. A
//! native `/api/chat` adapter would send a different request body to reach a model the existing path
//! already reaches correctly, so it would add a second thing to test without adding a capability. It
//! is deliberately absent rather than stubbed, and this note exists so its absence reads as a decision
//! rather than an oversight.
//!
//! "OpenCode" is not here either, and for a different reason: it is a coding agent, not a model
//! endpoint. There is no API to point at, so there is nothing to configure honestly.

use std::collections::BTreeMap;

use probelab_core::{AsyncModelProvider, ModelDescriptor};
use serde::{Deserialize, Serialize};

use crate::anthropic::{AnthropicConfig, AnthropicProvider};
use crate::gemini::{GeminiConfig, GeminiProvider};
use crate::transport::{validate_base_url, TransportConfigError};
use crate::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};

/// Narrows the established adapter's error to a shared transport error, preserving which rule failed.
fn narrow_openai_error(error: crate::OpenAiCompatibleConfigError) -> TransportConfigError {
    use crate::OpenAiCompatibleConfigError as Existing;
    use TransportConfigError as Transport;
    match error {
        Existing::EmptyBaseUrl => Transport::EmptyBaseUrl,
        Existing::InvalidBaseUrl => Transport::InvalidBaseUrl,
        Existing::UrlUserInfo => Transport::UrlUserInfo,
        Existing::UrlQuery => Transport::UrlQuery,
        Existing::UrlFragment => Transport::UrlFragment,
        Existing::InsecureBaseUrl => Transport::InsecureBaseUrl,
        Existing::EmptyApiKeyEnv => Transport::EmptyApiKeyEnv,
        Existing::InvalidApiKeyEnv => Transport::InvalidApiKeyEnv,
        Existing::EmptyModel => Transport::EmptyModel,
        Existing::InvalidTimeout => Transport::InvalidTimeout,
        Existing::InvalidTemperature => Transport::InvalidTemperature,
        Existing::InvalidMaxTokens => Transport::InvalidMaxTokens,
        // The only variant with no transport equivalent: the HTTP client could not be built, which is
        // a local build failure rather than a rule about the configuration.
        Existing::ClientConstruction => Transport::InvalidBaseUrl,
    }
}

/// A provider vendor the lab can talk to.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderKind {
    /// Any server speaking the OpenAI chat-completions API, including a local Ollama or llama.cpp.
    OpenAiCompatible,
    /// Anthropic's Messages API.
    Anthropic,
    /// Google's Gemini `generateContent` API.
    Gemini,
}

impl ProviderKind {
    /// Every vendor, in the order a UI should list them.
    pub fn all() -> Vec<ProviderKind> {
        vec![
            ProviderKind::OpenAiCompatible,
            ProviderKind::Anthropic,
            ProviderKind::Gemini,
        ]
    }

    /// The stable identifier recorded in a result.
    pub fn id(&self) -> &'static str {
        match self {
            ProviderKind::OpenAiCompatible => "openai-compatible",
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::Gemini => "gemini",
        }
    }

    /// The name shown to a user.
    pub fn display_name(&self) -> &'static str {
        match self {
            ProviderKind::OpenAiCompatible => {
                "OpenAI-compatible (ChatGPT, Ollama, llama.cpp, vLLM)"
            }
            ProviderKind::Anthropic => "Anthropic (Claude)",
            ProviderKind::Gemini => "Google Gemini",
        }
    }

    /// The default endpoint, which is also what the UI pre-fills.
    pub fn default_base_url(&self) -> &'static str {
        match self {
            ProviderKind::OpenAiCompatible => "https://api.openai.com/v1",
            ProviderKind::Anthropic => "https://api.anthropic.com",
            ProviderKind::Gemini => "https://generativelanguage.googleapis.com",
        }
    }

    /// The environment variable the vendor's key is read from.
    pub fn default_api_key_env(&self) -> &'static str {
        match self {
            ProviderKind::OpenAiCompatible => "OPENAI_COMPATIBLE_API_KEY",
            ProviderKind::Anthropic => "ANTHROPIC_API_KEY",
            ProviderKind::Gemini => "GEMINI_API_KEY",
        }
    }

    /// Whether this vendor's credential is required for a call to be attempted.
    ///
    /// True for all three. A local server that ignores the header still gets a value, and requiring one
    /// keeps the rule uniform: the lab never invents a placeholder credential to satisfy a vendor that
    /// does not check.
    pub fn requires_credential(&self) -> bool {
        true
    }

    /// Whether the default endpoint is reached over the public internet.
    pub fn is_remote(&self) -> bool {
        true
    }
}

/// A vendor-agnostic provider configuration.
///
/// The three vendor configs are kept as variants rather than flattened into one struct of optional
/// fields. A flat struct would let a user build `{anthropic_key: Some(k), gemini_model: None}` — a
/// configuration that is neither vendor's — and validate it successfully, which is exactly the class
/// of quietly-wrong setup this lab exists to avoid.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "vendor", rename_all = "kebab-case")]
pub enum AnyProviderConfig {
    /// An OpenAI-compatible endpoint.
    OpenAiCompatible(OpenAiCompatibleConfig),
    /// An Anthropic Messages endpoint.
    Anthropic(AnthropicConfig),
    /// A Gemini `generateContent` endpoint.
    Gemini(GeminiConfig),
}

impl AnyProviderConfig {
    /// The vendor this configuration is for.
    pub fn kind(&self) -> ProviderKind {
        match self {
            AnyProviderConfig::OpenAiCompatible(_) => ProviderKind::OpenAiCompatible,
            AnyProviderConfig::Anthropic(_) => ProviderKind::Anthropic,
            AnyProviderConfig::Gemini(_) => ProviderKind::Gemini,
        }
    }

    /// The model identifier requested.
    pub fn model(&self) -> &str {
        match self {
            AnyProviderConfig::OpenAiCompatible(config) => &config.model,
            AnyProviderConfig::Anthropic(config) => &config.model,
            AnyProviderConfig::Gemini(config) => &config.model,
        }
    }

    /// The configured base URL.
    pub fn base_url(&self) -> &str {
        match self {
            AnyProviderConfig::OpenAiCompatible(config) => &config.base_url,
            AnyProviderConfig::Anthropic(config) => &config.base_url,
            AnyProviderConfig::Gemini(config) => &config.base_url,
        }
    }

    /// Validates the configuration under this vendor's rules.
    pub fn validate(&self) -> Result<(), TransportConfigError> {
        match self {
            // The established adapter has its own error type because its messages are part of the
            // desktop IPC contract, so it is narrowed rather than replaced. Flattening every case to
            // one variant would be wrong in a way a user would hit: a plain-HTTP base URL would be
            // reported as a malformed one, sending them to fix a URL that is perfectly well formed.
            AnyProviderConfig::OpenAiCompatible(config) => {
                config.validate().map_err(narrow_openai_error)
            }
            AnyProviderConfig::Anthropic(config) => config.validate(),
            AnyProviderConfig::Gemini(config) => config.validate(),
        }
    }

    /// A starting configuration for a vendor, pointing at that vendor's real endpoint.
    pub fn default_for(kind: ProviderKind) -> Self {
        match kind {
            ProviderKind::OpenAiCompatible => {
                AnyProviderConfig::OpenAiCompatible(OpenAiCompatibleConfig {
                    base_url: kind.default_base_url().to_owned(),
                    ..OpenAiCompatibleConfig::default()
                })
            }
            ProviderKind::Anthropic => AnyProviderConfig::Anthropic(AnthropicConfig::default()),
            ProviderKind::Gemini => AnyProviderConfig::Gemini(GeminiConfig::default()),
        }
    }
}

/// A provider of any supported vendor, behind one trait object.
#[derive(Clone, Debug)]
pub enum AnyProvider {
    /// An OpenAI-compatible provider.
    OpenAiCompatible(Box<OpenAiCompatibleProvider>),
    /// An Anthropic Messages provider.
    Anthropic(Box<AnthropicProvider>),
    /// A Gemini `generateContent` provider.
    Gemini(Box<GeminiProvider>),
}

impl AnyProvider {
    /// Builds a provider from a configuration, validating first.
    pub fn build(config: AnyProviderConfig) -> Result<Self, TransportConfigError> {
        match config {
            AnyProviderConfig::OpenAiCompatible(config) => OpenAiCompatibleProvider::new(config)
                .map(|provider| AnyProvider::OpenAiCompatible(Box::new(provider)))
                .map_err(narrow_openai_error),
            AnyProviderConfig::Anthropic(config) => Ok(AnyProvider::Anthropic(Box::new(
                AnthropicProvider::new(config)?,
            ))),
            AnyProviderConfig::Gemini(config) => {
                Ok(AnyProvider::Gemini(Box::new(GeminiProvider::new(config)?)))
            }
        }
    }

    /// The vendor behind this provider.
    pub fn kind(&self) -> ProviderKind {
        match self {
            AnyProvider::OpenAiCompatible(_) => ProviderKind::OpenAiCompatible,
            AnyProvider::Anthropic(_) => ProviderKind::Anthropic,
            AnyProvider::Gemini(_) => ProviderKind::Gemini,
        }
    }
}

impl AsyncModelProvider for AnyProvider {
    fn descriptor(&self) -> ModelDescriptor {
        match self {
            AnyProvider::OpenAiCompatible(provider) => provider.descriptor(),
            AnyProvider::Anthropic(provider) => provider.descriptor(),
            AnyProvider::Gemini(provider) => provider.descriptor(),
        }
    }

    fn predict(
        &self,
        request: probelab_core::ModelRequest,
    ) -> probelab_core::ModelProviderFuture<'_> {
        match self {
            AnyProvider::OpenAiCompatible(provider) => provider.predict(request),
            AnyProvider::Anthropic(provider) => provider.predict(request),
            AnyProvider::Gemini(provider) => provider.predict(request),
        }
    }
}

/// Which vendors would send a key over the network, and which would not.
///
/// Exposed as a table so the security review can read the whole surface at once instead of inferring
/// it from three constructors.
pub fn credential_posture() -> Vec<(ProviderKind, &'static str, &'static str)> {
    ProviderKind::all()
        .into_iter()
        .map(|kind| {
            let transport = match kind {
                ProviderKind::OpenAiCompatible => "HTTPS, or HTTP to loopback",
                ProviderKind::Anthropic => "HTTPS only",
                ProviderKind::Gemini => "HTTPS only",
            };
            (kind, kind.default_api_key_env(), transport)
        })
        .collect()
}

/// A description of one vendor for a settings screen, with no credential material in it.
pub fn provider_catalog() -> Vec<BTreeMap<String, serde_json::Value>> {
    ProviderKind::all()
        .into_iter()
        .map(|kind| {
            let mut entry = BTreeMap::new();
            entry.insert(
                "id".to_owned(),
                serde_json::Value::String(kind.id().to_owned()),
            );
            entry.insert(
                "display_name".to_owned(),
                serde_json::Value::String(kind.display_name().to_owned()),
            );
            entry.insert(
                "default_base_url".to_owned(),
                serde_json::Value::String(kind.default_base_url().to_owned()),
            );
            entry.insert(
                "api_key_env".to_owned(),
                serde_json::Value::String(kind.default_api_key_env().to_owned()),
            );
            entry.insert(
                "requires_credential".to_owned(),
                serde_json::Value::Bool(kind.requires_credential()),
            );
            entry
        })
        .collect()
}

/// Whether a URL is safe to use as a base for any vendor.
pub fn base_url_is_transportable(base_url: &str) -> bool {
    validate_base_url(base_url).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_vendor_has_a_default_that_validates() {
        for kind in ProviderKind::all() {
            let config = AnyProviderConfig::default_for(kind);
            assert!(
                config.validate().is_ok(),
                "{kind:?} does not have a working default configuration"
            );
            assert_eq!(config.kind(), kind);
        }
    }

    #[test]
    fn every_vendor_builds_a_provider_that_describes_itself() {
        for kind in ProviderKind::all() {
            let provider = AnyProvider::build(AnyProviderConfig::default_for(kind))
                .unwrap_or_else(|error| panic!("{kind:?} should build: {error}"));
            assert_eq!(provider.kind(), kind);
            let descriptor = provider.descriptor();
            assert!(
                !descriptor.adapter.is_empty(),
                "{kind:?} produced a descriptor with no adapter name"
            );
            assert_eq!(
                descriptor.strategy,
                AnyProviderConfig::default_for(kind).model(),
                "{kind:?} descriptor disagrees with its configured model"
            );
        }
    }

    #[test]
    fn adapter_names_are_distinct_so_a_result_says_which_wire_protocol_ran() {
        let mut adapters: Vec<String> = ProviderKind::all()
            .into_iter()
            .map(|kind| {
                AnyProvider::build(AnyProviderConfig::default_for(kind))
                    .expect("provider")
                    .descriptor()
                    .adapter
            })
            .collect();
        let count = adapters.len();
        adapters.sort();
        adapters.dedup();
        assert_eq!(adapters.len(), count, "two vendors share an adapter name");
    }

    #[test]
    fn no_default_credential_value_appears_in_the_catalog() {
        // The catalog is what a settings screen renders and what a user may paste into an issue.
        for entry in provider_catalog() {
            for value in entry.values() {
                if let Some(text) = value.as_str() {
                    assert!(
                        !text.starts_with("sk-") && !text.starts_with("AIza"),
                        "a credential-shaped value reached the provider catalog: {text}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_catalog_covers_every_vendor_exactly_once() {
        let catalog = provider_catalog();
        assert_eq!(catalog.len(), ProviderKind::all().len());
        let ids: Vec<&str> = catalog
            .iter()
            .filter_map(|entry| entry.get("id"))
            .filter_map(serde_json::Value::as_str)
            .collect();
        assert!(ids.contains(&"anthropic"));
        assert!(ids.contains(&"gemini"));
        assert!(ids.contains(&"openai-compatible"));
    }

    #[test]
    fn a_configuration_cannot_be_mistaken_for_the_wrong_vendor() {
        // The tagged enum is the reason this is a compile error rather than a runtime surprise: there
        // is no shape in which an Anthropic config carries a Gemini base URL.
        let json = r#"{"vendor":"anthropic","base_url":"https://api.anthropic.com",
            "api_key_env":"ANTHROPIC_API_KEY","model":"claude-sonnet-4-5",
            "timeout_seconds":60,"temperature":0.0,"max_tokens":200}"#;
        let config: AnyProviderConfig = serde_json::from_str(json).expect("config");
        assert_eq!(config.kind(), ProviderKind::Anthropic);
        assert!(config.validate().is_ok());

        assert!(
            serde_json::from_str::<AnyProviderConfig>(r#"{"vendor":"opencode"}"#).is_err(),
            "an unknown vendor must not deserialize into something that looks configured"
        );
    }

    #[test]
    fn an_insecure_base_url_is_refused_for_every_vendor() {
        for kind in ProviderKind::all() {
            let mut config = AnyProviderConfig::default_for(kind);
            match &mut config {
                AnyProviderConfig::OpenAiCompatible(config) => {
                    config.base_url = "http://api.example.com/v1".to_owned()
                }
                AnyProviderConfig::Anthropic(config) => {
                    config.base_url = "http://api.anthropic.com".to_owned()
                }
                AnyProviderConfig::Gemini(config) => {
                    config.base_url = "http://generativelanguage.googleapis.com".to_owned()
                }
            }
            assert_eq!(
                config.validate(),
                Err(TransportConfigError::InsecureBaseUrl),
                "{kind:?} would have sent a key over plain HTTP"
            );
        }
    }

    #[test]
    fn the_credential_posture_names_every_vendors_variable() {
        let posture = credential_posture();
        assert_eq!(posture.len(), 3);
        for (kind, variable, transport) in posture {
            assert_eq!(variable, kind.default_api_key_env());
            assert!(!transport.is_empty());
        }
    }

    #[test]
    fn a_local_endpoint_is_reachable_over_plain_http() {
        // The one legitimate exception to HTTPS, and it is checked rather than assumed.
        assert!(base_url_is_transportable("http://127.0.0.1:11434/v1"));
        assert!(base_url_is_transportable("http://localhost:8080/v1"));
        assert!(!base_url_is_transportable("http://192.168.1.10:11434/v1"));
    }
}
