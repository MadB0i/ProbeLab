//! Transport rules shared by every provider adapter.
//!
//! # Why this is shared rather than per-adapter
//!
//! Four adapters that each validated their own URL would drift. The rules that matter here are
//! security rules — no redirects, no retries, HTTPS unless loopback, redaction of the credential from
//! anything that might be logged — and a rule that is implemented four times is a rule that is
//! enforced four times minus whatever the last adapter forgot.
//!
//! So the OpenAI-compatible adapter's existing, tested validation is lifted out here unchanged, and
//! every vendor goes through the same functions. Its own tests still pass, which is the evidence that
//! this is a move rather than a change.
//!
//! # What is deliberately not configurable
//!
//! Redirects and retries. A provider that answers with a 307 to somewhere else is either misconfigured
//! or hostile, and following it would send the credential somewhere the user never named. One attempt
//! per call, and the failure is reported rather than papered over with a retry.

use std::time::Duration;

use url::{Host, Url};

use serde::{Deserialize, Serialize};

/// A failure in a provider's transport configuration.
#[derive(Clone, Debug, thiserror::Error, Eq, PartialEq, Serialize, Deserialize)]
pub enum TransportConfigError {
    /// The base URL was empty.
    #[error("the provider base URL must not be empty")]
    EmptyBaseUrl,
    /// The base URL did not parse, or had no host.
    #[error("the provider base URL is not a valid absolute URL")]
    InvalidBaseUrl,
    /// The URL embedded credentials, which would put a secret in a log line.
    #[error("the provider base URL must not embed credentials")]
    UrlUserInfo,
    /// The URL carried a query string.
    ///
    /// Refused rather than stripped, because some vendors legitimately require one and silently
    /// dropping it would produce a confusing 404 instead of a clear configuration error.
    #[error("the provider base URL must not carry a query string")]
    UrlQuery,
    /// The URL carried a fragment.
    #[error("the provider base URL must not carry a fragment")]
    UrlFragment,
    /// Plain HTTP to a host that is not loopback.
    #[error("plain HTTP is only allowed to a loopback address")]
    InsecureBaseUrl,
    /// A credential variable name was empty.
    #[error("the API key environment variable name must not be empty")]
    EmptyApiKeyEnv,
    /// A credential variable name was not a plain identifier.
    #[error("the API key environment variable name must be a plain identifier")]
    InvalidApiKeyEnv,
    /// A timeout was zero.
    #[error("the provider timeout must be greater than zero")]
    InvalidTimeout,
    /// A temperature was out of range or not finite.
    #[error("the temperature must be a finite value between 0 and 2")]
    InvalidTemperature,
    /// A token limit was zero.
    #[error("the token limit must be greater than zero")]
    InvalidMaxTokens,
    /// A model name was blank.
    #[error("the model name must not be blank")]
    EmptyModel,
}

impl TransportConfigError {
    /// A short, stable code for a UI to branch on without matching on prose.
    pub fn code(&self) -> &'static str {
        match self {
            TransportConfigError::EmptyBaseUrl => "empty_base_url",
            TransportConfigError::InvalidBaseUrl => "invalid_base_url",
            TransportConfigError::UrlUserInfo => "url_user_info",
            TransportConfigError::UrlQuery => "url_query",
            TransportConfigError::UrlFragment => "url_fragment",
            TransportConfigError::InsecureBaseUrl => "insecure_base_url",
            TransportConfigError::EmptyApiKeyEnv => "empty_api_key_env",
            TransportConfigError::InvalidApiKeyEnv => "invalid_api_key_env",
            TransportConfigError::InvalidTimeout => "invalid_timeout",
            TransportConfigError::InvalidTemperature => "invalid_temperature",
            TransportConfigError::InvalidMaxTokens => "invalid_max_tokens",
            TransportConfigError::EmptyModel => "empty_model",
        }
    }
}

/// Validates a base URL under the shared transport rules.
///
/// HTTPS is required unless the host is loopback, which is what makes a local Ollama or llama.cpp
/// server usable without opening the door to plain HTTP anywhere else.
pub fn validate_base_url(base_url: &str) -> Result<Url, TransportConfigError> {
    let trimmed = base_url.trim();
    if trimmed.is_empty() {
        return Err(TransportConfigError::EmptyBaseUrl);
    }
    let url = Url::parse(trimmed).map_err(|_| TransportConfigError::InvalidBaseUrl)?;
    if url.host().is_none() {
        return Err(TransportConfigError::InvalidBaseUrl);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(TransportConfigError::UrlUserInfo);
    }
    if url.query().is_some() {
        return Err(TransportConfigError::UrlQuery);
    }
    if url.fragment().is_some() {
        return Err(TransportConfigError::UrlFragment);
    }
    let secure = url.scheme() == "https";
    let loopback = is_loopback_url(&url);
    if !secure && !loopback {
        return Err(TransportConfigError::InsecureBaseUrl);
    }
    Ok(url)
}

/// Whether a URL points at the local machine.
pub fn is_loopback_url(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(address)) => std::net::IpAddr::from(address).is_loopback(),
        Some(Host::Ipv6(address)) => std::net::IpAddr::from(address).is_loopback(),
        None => false,
    }
}

/// Validates a credential environment variable *name*.
///
/// The name is validated because it is interpolated into the environment; the value never is. This is
/// the only credential storage the lab has, by design — no keyring, no config file, nothing that could
/// end up in a result bundle.
pub fn validate_api_key_env(name: &str) -> Result<(), TransportConfigError> {
    if name.is_empty() {
        return Err(TransportConfigError::EmptyApiKeyEnv);
    }
    let mut characters = name.chars();
    let first = characters.next().expect("checked non-empty");
    if first.is_ascii_digit() {
        return Err(TransportConfigError::InvalidApiKeyEnv);
    }
    if !first.is_ascii_alphabetic() && first != '_' {
        return Err(TransportConfigError::InvalidApiKeyEnv);
    }
    if !characters.all(|character| character.is_ascii_alphanumeric() || character == '_') {
        return Err(TransportConfigError::InvalidApiKeyEnv);
    }
    Ok(())
}

/// Validates the sampling parameters every vendor shares.
pub fn validate_sampling(temperature: f64, max_tokens: u32) -> Result<(), TransportConfigError> {
    if !temperature.is_finite() || !(0.0..=2.0).contains(&temperature) {
        return Err(TransportConfigError::InvalidTemperature);
    }
    if max_tokens == 0 {
        return Err(TransportConfigError::InvalidMaxTokens);
    }
    Ok(())
}

/// Validates a model name.
pub fn validate_model(model: &str) -> Result<(), TransportConfigError> {
    if model.trim().is_empty() {
        return Err(TransportConfigError::EmptyModel);
    }
    Ok(())
}

/// Validates a model identifier for vendors that put it in the request *path*.
///
/// # Why this is stricter than [`validate_model`]
///
/// Gemini's endpoint is `…/models/{model}:generateContent`, so the model name is spliced into a URL.
/// A name containing `../` does not merely produce a bad request: URL resolution collapses it, so
/// `gemini/../../admin` resolves to `…/v1beta/admin:generateContent` and the adapter silently calls a
/// different endpoint than the configured model names. Since the model name is user input, that is a
/// way to aim a request at an arbitrary path on the host.
///
/// Rather than escape the value and hope, the identifier is constrained to what a real model id looks
/// like — letters, digits, dots, dashes, underscores, and colons, with an optional `models/` prefix.
/// A name that fails this was either a typo or an attempt, and in both cases refusing beats guessing.
pub fn validate_model_identifier(model: &str) -> Result<(), TransportConfigError> {
    let trimmed = model.trim();
    if trimmed.is_empty() {
        return Err(TransportConfigError::EmptyModel);
    }
    let bare = trimmed.strip_prefix("models/").unwrap_or(trimmed);
    if bare.is_empty() || bare.starts_with('.') || bare.ends_with('.') {
        return Err(TransportConfigError::EmptyModel);
    }
    if !bare
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || "._-:".contains(character))
    {
        return Err(TransportConfigError::EmptyModel);
    }
    Ok(())
}

/// Reads the credential named by `api_key_env`, failing closed when it is missing or empty.
pub fn read_credential(api_key_env: &str) -> Result<String, CredentialError> {
    validate_api_key_env(api_key_env).map_err(|_| CredentialError::InvalidName)?;
    let value = std::env::var(api_key_env).map_err(|error| match error {
        std::env::VarError::NotPresent => CredentialError::Missing,
        std::env::VarError::NotUnicode(_) => CredentialError::NotUnicode,
    })?;
    if value.is_empty() {
        return Err(CredentialError::Empty);
    }
    Ok(value)
}

/// A credential could not be obtained.
#[derive(Clone, Copy, Debug, thiserror::Error, Eq, PartialEq)]
pub enum CredentialError {
    /// The variable name was not a plain identifier.
    #[error("the API key environment variable name is not a plain identifier")]
    InvalidName,
    /// The variable is not set.
    #[error("the provider API key environment variable is not set")]
    Missing,
    /// The variable holds something that is not text.
    #[error("the provider API key environment variable is not valid unicode")]
    NotUnicode,
    /// The variable is set to nothing.
    #[error("the provider API key environment variable is empty")]
    Empty,
}

/// The character budget for a redacted fragment of provider output.
///
/// Bounded because this text reaches a result bundle and a UI, and a chatty provider must not be able
/// to inflate either.
pub const SANITIZED_LIMIT: usize = 2_048;

/// Removes a credential from text destined for a log, a result, or a UI.
pub fn sanitize_response_text(text: &str, secret: &str) -> String {
    let mut bounded: String = text.chars().take(SANITIZED_LIMIT).collect();
    if !secret.is_empty() {
        bounded = bounded.replace(secret, "[REDACTED]");
    }
    bounded
}

/// A request timeout, validated.
pub fn validate_timeout(seconds: u64) -> Result<Duration, TransportConfigError> {
    if seconds == 0 {
        return Err(TransportConfigError::InvalidTimeout);
    }
    Ok(Duration::from_secs(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_is_accepted_and_plain_http_is_not() {
        assert!(validate_base_url("https://api.example/v1").is_ok());
        assert_eq!(
            validate_base_url("http://api.example/v1"),
            Err(TransportConfigError::InsecureBaseUrl)
        );
    }

    #[test]
    fn loopback_may_serve_plain_http() {
        for url in [
            "http://127.0.0.1:11434/v1",
            "http://localhost:11434/v1",
            "http://[::1]:11434/v1",
        ] {
            assert!(
                validate_base_url(url).is_ok(),
                "{url} is loopback and should be allowed to serve plain HTTP"
            );
        }
    }

    #[test]
    fn a_credential_in_a_url_is_refused() {
        assert_eq!(
            validate_base_url("https://user:secret@api.example/v1"),
            Err(TransportConfigError::UrlUserInfo)
        );
    }

    #[test]
    fn a_query_string_is_refused_rather_than_silently_dropped() {
        assert_eq!(
            validate_base_url("https://api.example/v1?key=secret"),
            Err(TransportConfigError::UrlQuery)
        );
    }

    #[test]
    fn credential_names_must_be_plain_identifiers() {
        assert!(validate_api_key_env("OPENAI_COMPATIBLE_API_KEY").is_ok());
        for bad in ["", "1BAD", "HAS-DASH", "HAS SPACE", "HAS.DOT"] {
            assert!(
                validate_api_key_env(bad).is_err(),
                "{bad:?} should not be accepted as a variable name"
            );
        }
    }

    #[test]
    fn sampling_bounds_are_enforced() {
        assert!(validate_sampling(0.0, 1).is_ok());
        assert!(validate_sampling(2.0, 1).is_ok());
        assert!(validate_sampling(2.1, 1).is_err());
        assert!(validate_sampling(f64::NAN, 1).is_err());
        assert!(validate_sampling(0.0, 0).is_err());
    }

    #[test]
    fn redaction_removes_the_credential_and_bounds_the_text() {
        let text = "the key is sk-secret-value and then some";
        let redacted = sanitize_response_text(text, "sk-secret-value");
        assert!(
            !redacted.contains("sk-secret-value"),
            "the credential survived redaction: {redacted}"
        );
        assert!(redacted.contains("[REDACTED]"));

        let long = "x".repeat(SANITIZED_LIMIT * 2);
        assert_eq!(
            sanitize_response_text(&long, "y").chars().count(),
            SANITIZED_LIMIT
        );
    }

    #[test]
    fn an_empty_secret_does_not_break_redaction() {
        let redacted = sanitize_response_text("ordinary output", "");
        assert_eq!(redacted, "ordinary output");
    }
}
