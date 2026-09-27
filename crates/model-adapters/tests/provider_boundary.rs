//! Provider protocol boundary tests.
//!
//! # Why this file exists
//!
//! Gemini's endpoint puts the model name in the URL path, so the model name is user input that ends up
//! in a request target. An earlier audit found that `gemini/../../admin` resolved to
//! `.../v1beta/admin:generateContent` -- URL resolution collapsed the traversal and escaped the `/models`
//! segment, aiming the request at a different endpoint than the configured model named.
//!
//! That was fixed with a strict identifier rule. This file is the permanent regression net for it, and
//! it is deliberately adversarial: every payload here is something that could plausibly be typed or
//! pasted into a model field, and each must be refused or safely escaped.
//!
//! The general rule these tests encode: **untrusted text is never concatenated into a path structure
//! without validation.** Escaping is not enough, because URL resolution happens after escaping.

use probelab_model_adapters::gemini::GeminiConfig;
use probelab_model_adapters::transport::{validate_base_url, TransportConfigError};
use probelab_model_adapters::{AnthropicConfig, ProviderKind};
use url::Url;

/// The endpoint a model name must never be able to redirect a request to.
fn endpoint_for(model: &str) -> Option<Url> {
    let config = GeminiConfig {
        model: model.to_owned(),
        ..GeminiConfig::default()
    };
    config.endpoint().ok()
}

/// Plain traversal must be refused outright.
#[test]
fn a_plain_traversal_model_name_is_refused() {
    for payload in [
        "gemini/../../admin",
        "gemini/../../admin?x=1",
        "../../admin",
        "../v1beta/admin",
        "a/../../b",
    ] {
        let config = GeminiConfig {
            model: payload.to_owned(),
            ..GeminiConfig::default()
        };
        assert!(
            config.validate().is_err(),
            "{payload:?} must be refused; it escapes the /models segment"
        );
    }
}

/// No accepted model name may produce an endpoint outside the intended path.
#[test]
fn no_accepted_model_name_escapes_the_models_path() {
    // Counting the acceptances is what stops this being vacuous: a loop that skipped every payload
    // because all were refused would also "pass", while proving nothing about endpoint construction.
    let mut accepted = 0;
    for payload in [
        "gemini-2.5-pro",
        "models/gemini-2.5-pro",
        "gemini_2.5_flash",
        "gemini-2.5-flash-lite",
    ] {
        let Some(endpoint) = endpoint_for(payload) else {
            // Being refused is acceptable for a hostile name; silently redirecting is not.
            continue;
        };
        accepted += 1;
        let path = endpoint.path();
        assert!(
            path.contains("/v1beta/models/"),
            "{payload:?} produced {path}, which is outside the intended /v1beta/models/ path"
        );
        assert!(
            !path.contains(".."),
            "{payload:?} produced a path containing traversal: {path}"
        );
    }
    assert!(
        accepted > 0,
        "every real model name was refused, so this test proved nothing about endpoint construction"
    );
}

/// Query, fragment, and encoded-traversal payloads must all be refused.
#[test]
fn query_fragment_and_encoded_traversal_are_refused() {
    for payload in [
        "gemini?admin=1",
        "gemini#fragment",
        "gemini%2f..%2f..%2fadmin",
        "gemini%2F..%2Fadmin",
        "gemini%252f..%252fadmin",
        "gemini?x=../../admin",
    ] {
        let config = GeminiConfig {
            model: payload.to_owned(),
            ..GeminiConfig::default()
        };
        assert!(
            config.validate().is_err(),
            "{payload:?} must be refused; it smuggles a separator or an escape into the path"
        );
    }
}

/// An empty or blank model is refused rather than producing a nameless endpoint.
#[test]
fn an_empty_model_is_refused() {
    for payload in ["", "   ", "\t", "\n"] {
        let config = GeminiConfig {
            model: payload.to_owned(),
            ..GeminiConfig::default()
        };
        assert!(
            config.validate().is_err(),
            "{payload:?} must be refused rather than producing a bare models path"
        );
    }
}

/// A leading or trailing dot is refused, because it is how traversal is spelled.
#[test]
fn a_dotted_model_name_is_refused() {
    for payload in [".", "..", ".hidden", "trailing.", "gemini..", "..gemini"] {
        let config = GeminiConfig {
            model: payload.to_owned(),
            ..GeminiConfig::default()
        };
        assert!(
            config.validate().is_err(),
            "{payload:?} must be refused; dots are the alphabet of path traversal"
        );
    }
}

/// The base URL gets the same treatment: no credentials, no query, no fragment.
#[test]
fn a_poisoned_base_url_is_refused() {
    for payload in [
        "https://user:secret@generativelanguage.googleapis.com",
        "https://generativelanguage.googleapis.com?key=leaked",
        "https://generativelanguage.googleapis.com#frag",
        "http://generativelanguage.googleapis.com",
        "not a url",
        "://missing-scheme",
    ] {
        let config = GeminiConfig {
            base_url: payload.to_owned(),
            ..GeminiConfig::default()
        };
        assert!(
            config.validate().is_err(),
            "{payload:?} must be refused as a base URL"
        );
    }
}

/// The credential goes in a header, so a query-string key is never needed.
#[test]
fn a_key_in_the_base_url_is_refused_rather_than_relied_upon() {
    // Gemini accepts `?key=` and the shared transport rules refuse it, because a credential in a URL
    // reaches proxy logs and error messages that quote the URL.
    assert_eq!(
        validate_base_url("https://generativelanguage.googleapis.com?key=secret"),
        Err(TransportConfigError::UrlQuery)
    );
}

/// Anthropic's endpoint is host-fixed, so a model name cannot influence its path at all.
#[test]
fn anthropic_model_names_cannot_influence_the_path() {
    // The contrast with Gemini is the point: Anthropic puts the model in the request *body*, so no
    // model name can redirect the request. A traversal payload is therefore simply an unknown model,
    // not a path escape.
    for payload in ["../../admin", "gemini?x=1", "a#b"] {
        let config = AnthropicConfig {
            model: payload.to_owned(),
            ..AnthropicConfig::default()
        };
        let endpoint = config
            .endpoint()
            .expect("the endpoint does not depend on the model");
        assert_eq!(
            endpoint.as_str(),
            "https://api.anthropic.com/v1/messages",
            "{payload:?} changed the Anthropic endpoint"
        );
    }
}

/// A malformed endpoint is refused, not coerced.
#[test]
fn a_malformed_endpoint_is_refused() {
    let config = AnthropicConfig {
        base_url: "https://api.anthropic.com/v1/messages?leak=1".to_owned(),
        ..AnthropicConfig::default()
    };
    assert!(config.validate().is_err());
}

/// Every vendor keeps the credential out of the endpoint it constructs.
#[test]
fn no_vendor_constructs_an_endpoint_containing_a_credential() {
    for kind in ProviderKind::all() {
        let config = probelab_model_adapters::AnyProviderConfig::default_for(kind);
        let base = config.base_url().to_owned();
        assert!(
            !base.contains('@'),
            "{kind:?} default base URL embeds user info: {base}"
        );
        assert!(
            !base.to_ascii_lowercase().contains("key="),
            "{kind:?} default base URL embeds a credential: {base}"
        );
    }
}
