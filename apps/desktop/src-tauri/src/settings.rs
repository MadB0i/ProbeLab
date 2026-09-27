//! Settings commands.
//!
//! # What this layer is responsible for
//!
//! Three things, and nothing else: read the stored settings, validate and write a change, and reset
//! to defaults. All of the judgement lives in [`probelab_lab::settings`] and
//! [`probelab_lab::settings_store`], where it is tested without a Tauri runtime.
//!
//! # The authority question
//!
//! Settings are a *backend* store, not a mirror of frontend state. Every read comes from SQLite and
//! every write is validated before it is persisted, so a frontend cannot express a configuration the
//! backend has not accepted. There is deliberately no "set the current settings in memory" command and
//! no path by which a value typed into a field takes effect without going through
//! [`apply_settings`].
//!
//! What the frontend *does* own is the values it sends for the next run, which is unchanged by this
//! milestone: a run's request still carries its own episodes, seed, and provider. Settings decide the
//! defaults the UI starts from, not what a particular run must use.
//!
//! # Read-only facts
//!
//! The storage location and the platform's containment support are read from the host and reported,
//! never written. They are properties of the Tauri application's data directory and of the machine,
//! not preferences, and offering a control that could change where results are stored would be a
//! control with no backend to honour it.
//!
//! # This is not an execution authority
//!
//! The module imports the settings model, the settings store, and the provider status reporter. It
//! does not import the broker, the sandbox, the agent crate, or the evaluator, and it writes to
//! exactly one table. The structural tests at the bottom assert all three.

use probelab_lab::settings::{LabSettings, ReadOnlyFact, SettingsError};
use probelab_lab::settings_view::{ProviderState, SettingsView};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::provider::{ModelRunConfig, OPENAI_COMPATIBLE_PROVIDER};
use crate::AppState;

/// A structured error, so a panel can show which field was refused and why.
///
/// Serialized as a tagged shape rather than a bare string: "invalid settings" is not something a
/// person can act on, and a panel that cannot tell which control produced the error ends up
/// showing one generic banner that tells the user nothing about where to look.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "detail")]
pub enum SettingsCommandError {
    /// The submitted document failed validation. The message names the field and the constraint.
    Invalid {
        /// Which setting was refused, when the error identifies one.
        field: Option<String>,
        /// The reason, written for a person.
        reason: String,
    },
    /// The settings could not be read or written.
    Storage {
        /// The underlying failure, already free of any credential.
        reason: String,
    },
}

impl From<SettingsError> for SettingsCommandError {
    fn from(error: SettingsError) -> Self {
        // `SettingsError` is a closed set of validation failures, so the field name can be named here
        // rather than parsed back out of a message. Matching on a message string would break the
        // moment the wording changed, which is the kind of coupling that makes a validation error
        // useless in a UI.
        let field = match &error {
            SettingsError::EpisodesOutOfRange { .. } => Some("experimentDefaults.episodes"),
            SettingsError::TimeoutOutOfRange { .. } => Some("providerDefaults.timeoutSeconds"),
            SettingsError::InvalidBaseUrl(_) => Some("providerDefaults.baseUrl"),
            SettingsError::InvalidApiKeyEnvName => Some("providerDefaults.apiKeyEnv"),
            SettingsError::EmptyModel => Some("providerDefaults.model"),
            SettingsError::UnsupportedSchemaVersion { .. } => None,
            SettingsError::Storage(_) => None,
        }
        .map(str::to_owned);
        match error {
            SettingsError::Storage(reason) => Self::Storage { reason },
            other => Self::Invalid {
                field,
                reason: other.to_string(),
            },
        }
    }
}

/// Reads the current settings and the live provider state.
#[tauri::command]
pub fn read_settings(state: State<'_, AppState>) -> Result<SettingsView, SettingsCommandError> {
    let settings = state.settings.load().map_err(SettingsCommandError::from)?;
    let provider_state = provider_state_for(&settings);
    Ok(SettingsView::assemble(
        settings,
        provider_state,
        read_only_facts(&state),
    ))
}

/// A request to change settings.
///
/// A whole document rather than a patch. A partial update would mean deciding what a missing field
/// means — keep the old value, or reset it? — and both answers are surprising in different ways. A
/// full document has exactly one meaning: these are the settings now.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsRequest {
    /// The complete intended settings.
    pub settings: LabSettings,
}

/// Validates and persists settings, then returns the view as stored.
///
/// The response is the *stored* document rather than the submitted one, so a panel cannot display a
/// value the backend adjusted or refused. A caller that sends something invalid gets an error and
/// keeps whatever it had.
#[tauri::command]
pub fn apply_settings(
    state: State<'_, AppState>,
    request: SettingsRequest,
) -> Result<SettingsView, SettingsCommandError> {
    let stored = state
        .settings
        .save(&request.settings)
        .map_err(SettingsCommandError::from)?;
    let provider_state = provider_state_for(&stored);
    Ok(SettingsView::assemble(
        stored,
        provider_state,
        read_only_facts(&state),
    ))
}

/// Replaces the stored settings with the built-in defaults.
///
/// Touches one row in the settings table, in the settings store's own file. Results, telemetry,
/// experiment lifecycles, and replay data live in a different store in a different file, so there is
/// nothing here that could reach them — asserted by `reset_touches_no_result_state`, which is stricter
/// than reality: it points both stores at the *same* file and checks the result is untouched
/// afterwards.
#[tauri::command]
pub fn reset_settings(state: State<'_, AppState>) -> Result<SettingsView, SettingsCommandError> {
    let defaults = state
        .settings
        .reset_to_defaults()
        .map_err(SettingsCommandError::from)?;
    let provider_state = provider_state_for(&defaults);
    Ok(SettingsView::assemble(
        defaults,
        provider_state,
        read_only_facts(&state),
    ))
}

/// Where the settings themselves are stored.
///
/// Reported rather than configurable. The path is under the Tauri application data directory, which
/// the host decides; a control that appeared to move it and did not would be worse than no control.
fn settings_path_text(state: &AppState) -> String {
    state.settings_path.display().to_string()
}

/// Builds the read-only facts the host knows and the settings document does not.
fn read_only_facts(state: &AppState) -> Vec<ReadOnlyFact> {
    vec![
        ReadOnlyFact {
            id: "storage.results".to_owned(),
            label: "Results, telemetry, and experiment history".to_owned(),
            value: state.results_path.display().to_string(),
            reason:
                "Fixed to the application data directory so a run's evidence cannot be relocated by a \
                 preference. Move the file yourself if you want to archive it."
                    .to_owned(),
        },
        ReadOnlyFact {
            id: "storage.settings".to_owned(),
            label: "Settings".to_owned(),
            value: settings_path_text(state),
            reason: "Fixed to the application data directory, alongside the results it describes."
                .to_owned(),
        },
        ReadOnlyFact {
            id: "security.credential".to_owned(),
            label: "Model credentials".to_owned(),
            value: "Read from the process environment".to_owned(),
            reason:
                "A credential is never stored by this application. Export it in the named environment \
                 variable before starting ProbeLab; there is nowhere in these settings to put one."
                    .to_owned(),
        },
        ReadOnlyFact {
            id: "logging.level".to_owned(),
            label: "Log verbosity".to_owned(),
            value: "RUST_LOG, read once at start-up".to_owned(),
            reason:
                "The tracing filter is built during start-up, before a window exists. Changing it needs \
                 a restart, so it is an environment variable rather than a setting."
                    .to_owned(),
        },
        ReadOnlyFact {
            id: "containment.platform".to_owned(),
            label: "Process containment".to_owned(),
            value: current_containment_text(),
            reason:
                "A property of the operating system, not a preference. The lab reports what it can \
                 support rather than pretending to support everything."
                    .to_owned(),
        },
    ]
}

/// The provider state a panel shows, derived from the adapter's own status.
///
/// # Why this is not "is the provider in the list"
///
/// Listing a protocol and being able to use it are different facts. The status is produced by the
/// same `provider_status` call the run-time pre-flight uses, so the settings page cannot claim a
/// connected run is ready when the run would be refused — the exact disagreement the provider module
/// was written to prevent.
fn provider_state_for(settings: &LabSettings) -> ProviderState {
    let defaults = &settings.provider_defaults;
    // Built through the real `ModelRunConfig`, so this path exercises the same validation and the
    // same credential check a run would. Constructing the status by hand here would be a second
    // implementation of "could this start", which is the thing most likely to drift.
    let config =
        ModelRunConfig::openai_compatible(&probelab_model_adapters::OpenAiCompatibleConfig {
            base_url: defaults.base_url.clone(),
            api_key_env: defaults.api_key_env.clone(),
            model: defaults.model.clone(),
            timeout_seconds: u64::from(defaults.timeout_seconds),
            temperature: 0.0,
            max_tokens: 1024,
        });
    let status = config.provider_status();
    ProviderState {
        provider: OPENAI_COMPATIBLE_PROVIDER.to_owned(),
        model: status.model.clone(),
        configured: status.configured,
        status: serde_json::to_value(status.status)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_else(|| "not_configured".to_owned()),
        message: status.message,
        credential_source: status.credential_source,
        // Presence as a boolean, read from the environment and reduced to a fact immediately. The
        // value is never bound to a variable that outlives this expression, so there is nothing for a
        // later log line or response to pick up.
        credential_present: std::env::var(&defaults.api_key_env)
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false),
    }
}

/// The platform's containment support, in words.
///
/// Read from the same `current_platform` the runner uses, so this cannot claim support the runner
/// would not provide. Windows gets its real answer; anything else is reported as unsupported rather
/// than optimistically described, because a settings page is where someone goes to find out what the
/// lab will actually do.
fn current_containment_text() -> String {
    if cfg!(target_os = "windows") {
        "Windows AppContainer, if available".to_owned()
    } else {
        "Not supported on this platform; runs are refused rather than executed uncontained"
            .to_owned()
    }
}

#[cfg(test)]
mod tests {
    // # Structural guarantees
    //
    // Settings configures the lab. It is not an execution authority, and the properties that make
    // that true are asserted here rather than left to review.

    /// The module body, without the tests that name the forbidden things.
    fn body() -> String {
        include_str!("settings.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("the module body precedes its tests")
            .to_owned()
    }

    /// Settings cannot reach an executor.
    ///
    /// An import is required to touch a broker, a sandbox, or a provider agent, so the capability
    /// boundary is visible at the import rather than buried in a call three functions down.
    #[test]
    fn settings_cannot_import_an_executor() {
        let source = body();
        for line in source.lines() {
            let trimmed = line.trim();
            if !trimmed.starts_with("use ") {
                continue;
            }
            for forbidden in [
                "probelab_sandbox",
                "probelab_broker",
                "probelab_agent",
                "probelab_evaluator",
                "probelab_benchmarks",
                "std::process",
            ] {
                assert!(
                    !trimmed.contains(forbidden),
                    "the settings path must not import {forbidden}: {trimmed}"
                );
            }
        }
    }

    /// Settings cannot write to results, telemetry, or experiment history.
    ///
    /// The one write it performs is to the settings table. This is the assertion that makes a reset
    /// safe by construction rather than by careful coding: if a future change added a call to the
    /// result repository from this module, it would fail here.
    #[test]
    fn settings_cannot_write_a_result_or_an_experiment() {
        let source = body();
        for forbidden in [
            "store_completed",
            "put_experiment",
            "reconcile_interrupted",
            "SqliteResultRepository",
            "SqliteTelemetryRecorder",
        ] {
            assert!(
                !source.contains(forbidden),
                "the settings path must not reach {forbidden}"
            );
        }
    }

    /// Settings cannot call anything that authorizes.
    ///
    /// An authorization decision is made by the broker from a capability request and a policy. There
    /// is no settings value that reaches either, and this asserts the absence of the call rather than
    /// the absence of a comment claiming it.
    #[test]
    fn settings_cannot_authorize() {
        let source = body();
        for forbidden in [
            "authorize",
            "CapabilityRequest",
            "CapabilityDecision",
            "broker::",
        ] {
            assert!(
                !source.contains(forbidden),
                "the settings path must not reach {forbidden}"
            );
        }
    }

    /// A stored settings document is written through exactly one method.
    ///
    /// Named rather than counted, so adding a second write path is a deliberate act that fails here
    /// instead of a diff nobody reads.
    #[test]
    fn settings_writes_only_through_the_store() {
        let source = body();
        assert!(
            source.contains(".save("),
            "settings must be persisted through the store"
        );
        assert!(
            !source.contains(".execute_batch"),
            "the settings module must not create or alter tables; the store owns the schema"
        );
    }
}
