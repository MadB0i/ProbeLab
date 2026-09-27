//! The read model a settings panel renders.
//!
//! # Why this is a projection and not the settings themselves
//!
//! A panel needs three things the bare settings cannot give it: which values are editable, what to
//! show about the parts that are fixed, and whether a provider is actually usable rather than merely
//! listed. Assembling that here — rather than in the panel — means the answers come from the same
//! place the validation does, so a panel cannot decide on its own that something is editable.
//!
//! # The credential question, again
//!
//! [`ProviderState`] reports `configured: true` or `false` and a human-readable message. It has no
//! field for a key, an environment-variable *value*, an authorization header, or a response body. The
//! only thing it carries from the environment is the *name* of the variable it looked in, which is a
//! label an operator needs in order to fix the problem. A settings page that could show whether a
//! secret is set is useful; one that could show the secret is a liability, and this type is built so
//! the second is not expressible.

use crate::settings::{
    ConnectedProvider, DefaultRunMode, ExperimentDefaults, LabSettings, ProviderDefaults,
    ReadOnlyFact, SeedBehaviour, SettingScope,
};
use serde::{Deserialize, Serialize};

/// Whether a provider could actually start a run, and why not if it could not.
///
/// Derived from the adapter's own `provider_status`, not from "is this provider in the list". Listing
/// a protocol and being able to use it are different facts, and a settings page that implied the
/// first meant the second would be the exact claim this product is careful not to make elsewhere.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderState {
    /// Stable provider identifier.
    pub provider: String,
    /// The model identifier that would be requested.
    pub model: String,
    /// Whether a run with this configuration could start.
    pub configured: bool,
    /// Coarse state: `connected` or `not_configured`.
    pub status: String,
    /// A human-readable explanation, already free of credentials.
    pub message: String,
    /// A description of where a credential would come from, such as `environment`.
    ///
    /// Never the value of one. A name is a label; a value is a secret.
    pub credential_source: String,
    /// Whether a credential is present in the environment, reported as a fact and nothing more.
    pub credential_present: bool,
}

/// One editable setting, with the scope the backend decided rather than the panel guessing.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingEntry<T> {
    /// Stable identifier for the setting.
    pub id: String,
    /// The current value.
    pub value: T,
    /// Whether the backend will accept a change to it.
    pub scope: SettingScope,
    /// Why it is read-only, when it is.
    pub read_only_reason: Option<String>,
}

/// The experiment-defaults section, as a panel sees it.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExperimentDefaultsView {
    /// How many episodes a new experiment runs.
    pub episodes: SettingEntry<u32>,
    /// How the seed is chosen.
    pub seed_behaviour: SettingEntry<SeedBehaviour>,
    /// The seed used when the behaviour is fixed.
    pub root_seed: SettingEntry<u64>,
    /// The planner's own ceiling, so the panel can explain the limit rather than enforce it silently.
    pub max_episodes: u32,
    /// Whether the chosen seed is actually the one that will be sent.
    pub effective_root_seed: Option<u64>,
}

/// The provider section, as a panel sees it.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderDefaultsView {
    /// Which protocol a connected run would speak.
    pub provider: SettingEntry<ConnectedProvider>,
    /// The base URL that would be used.
    pub base_url: SettingEntry<String>,
    /// The *name* of the environment variable holding the key.
    pub api_key_env: SettingEntry<String>,
    /// The model that would be requested.
    pub model: SettingEntry<String>,
    /// The request timeout.
    pub timeout_seconds: SettingEntry<u32>,
    /// What is actually known about the provider right now.
    pub state: ProviderState,
    /// A statement that the value is never transferred here, in the panel's own voice.
    pub secret_policy: String,
}

/// Everything a settings panel renders.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    /// The stored settings these values came from.
    pub settings: LabSettings,
    /// The experiment defaults.
    pub experiment_defaults: ExperimentDefaultsView,
    /// The provider defaults.
    pub provider_defaults: ProviderDefaultsView,
    /// Which provider the UI offers first.
    pub default_run_mode: SettingEntry<DefaultRunMode>,
    /// Facts that exist but are fixed by the architecture, shown read-only.
    pub read_only_facts: Vec<ReadOnlyFact>,
    /// Whether these values are the built-in defaults.
    pub is_default: bool,
}

impl SettingsView {
    /// Assembles the view from stored settings and a live provider status.
    ///
    /// `read_only_facts` are supplied by the caller because they are properties of the *host* — the
    /// application's data directory, the platform's containment support — rather than of the
    /// settings document. The lab cannot know where the desktop application stores its database.
    pub fn assemble(
        settings: LabSettings,
        state: ProviderState,
        read_only_facts: Vec<ReadOnlyFact>,
    ) -> Self {
        // Everything the view needs is read from `settings` before it is destructured below, because
        // the struct is moved into the view at the end and a later borrow would be a use of a moved
        // value. Capturing first keeps the ownership story in one place.
        let is_default = settings == LabSettings::default();
        let default_run_mode = settings.default_run_mode;
        let effective_root_seed = settings.default_root_seed();
        // Destructured field by field rather than by moving the sections out. The view carries the
        // whole document as well as the flattened entries, so the struct itself is still needed at
        // the end; taking the pieces individually keeps that borrow honest instead of cloning a
        // section just to move it.
        let ExperimentDefaults {
            episodes,
            seed_behaviour,
            root_seed,
        } = settings.experiment_defaults;
        let ProviderDefaults {
            provider,
            base_url,
            api_key_env,
            model,
            timeout_seconds,
        } = settings.provider_defaults.clone();
        fn editable<T>(id: &str, value: T) -> SettingEntry<T> {
            SettingEntry {
                id: id.to_owned(),
                value,
                scope: SettingScope::Editable,
                read_only_reason: None,
            }
        }

        Self {
            experiment_defaults: ExperimentDefaultsView {
                episodes: editable("experiment_defaults.episodes", episodes),
                seed_behaviour: editable("experiment_defaults.seed_behaviour", seed_behaviour),
                root_seed: editable("experiment_defaults.root_seed", root_seed),
                max_episodes: crate::settings::MAX_DEFAULT_EPISODES,
                effective_root_seed,
            },
            provider_defaults: ProviderDefaultsView {
                provider: editable("provider_defaults.provider", provider),
                base_url: editable("provider_defaults.base_url", base_url),
                api_key_env: editable("provider_defaults.api_key_env", api_key_env),
                model: editable("provider_defaults.model", model),
                timeout_seconds: editable("provider_defaults.timeout_seconds", timeout_seconds),
                state,
                secret_policy:
                    "The key is read from the process environment by the backend at run time. This page \
                     never receives it, cannot store it, and will not display it."
                        .to_owned(),
            },
            default_run_mode: editable("default_run_mode", default_run_mode),
            read_only_facts,
            is_default,
            settings,
        }
    }
}
