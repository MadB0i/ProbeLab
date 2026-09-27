//! Application settings: what the lab is configured to do by default.
//!
//! # What this is
//!
//! A small, typed, validated description of the defaults the lab will use for future work. It exists
//! because those defaults were previously hard-coded in the React layer, which meant they were lost
//! on every restart, could drift between two call sites that each had their own idea of the right
//! number, and were never validated by anything.
//!
//! # What this is deliberately not
//!
//! * **Not an execution authority.** Settings choose defaults. They do not authorize a capability,
//!   run a benchmark, or influence an outcome that has already been recorded. A recorded result is
//!   immutable and a setting cannot reach it.
//! * **Not a place to put a secret.** See the type design below.
//! * **Not a way to move the database.** The storage location is fixed by the Tauri application
//!   data directory, and pretending otherwise would be a control that appears to work.
//!
//! # Why this type cannot hold a credential
//!
//! The single most important property here is a structural one: [`LabSettings`] has no field of any
//! type that could contain a key. There is no `api_key: String`, no `secret: Option<String>`, no
//! generic `Map<String, String>` escape hatch. The connected-provider settings carry the *name* of
//! an environment variable — `OPENAI_COMPATIBLE_API_KEY` — which is a label, not a value, and the
//! only place a key is ever read is the transport layer reading the process environment.
//!
//! That means a secret cannot reach this struct, so it cannot be persisted with it, logged with it,
//! serialized into a result, or returned to a frontend. The property is enforced by the type rather
//! than by remembering to redact a field, and `secret_never_enters_settings` in the tests asserts it
//! by construction rather than by convention.
//!
//! # Validation is not optional
//!
//! Every mutable value is validated here, in the lab crate, before it is ever stored — not in the
//! UI, and not on the way out through IPC. A frontend that sends an episode count of zero is refused
//! the same way a caller in Rust would be. Every failure is a typed [`SettingsError`] naming the
//! field and the constraint, so a panel can show what is wrong rather than "invalid settings".
//!
//! The URL checks are *delegated* to the transport layer's own `validate_base_url` rather than
//! reimplemented. Those rules decide whether a URL may carry a credential, whether it is HTTPS or
//! loopback, and whether it smuggles a query string — they are security rules, and a second copy of
//! them in a settings module is a second thing to get wrong and a second place for a reviewer to
//! miss. This crate is in `probelab-lab` rather than `probelab-core` for exactly that reason: it
//! already depends on the adapter crate, and the dependency runs in the direction that keeps one
//! copy of those rules.

use probelab_model_adapters::transport::{
    validate_api_key_env, validate_base_url, validate_model, TransportConfigError,
};
use serde::{Deserialize, Serialize};

/// The version of the settings contract.
///
/// Bumped when a stored document cannot be read by the current build. An unknown version is
/// refused rather than interpreted: guessing at a settings layout from an older or newer build is
/// how a lab ends up running with a timeout of zero because a field moved.
pub const SETTINGS_SCHEMA_VERSION: u32 = 1;

/// The most episodes a default may request.
///
/// Matches the Brain's own ceiling, so a default can never plan something the planner would refuse.
/// The constraint lives in the planner too; duplicating the number here would be a second source of
/// truth that could drift.
pub const MAX_DEFAULT_EPISODES: u32 = 1_000;

/// The shortest and longest request timeout, in seconds.
///
/// The lower bound exists because a zero timeout is not "instant" — it is a request that has already
/// failed. The upper bound is a resource limit, since a timeout is how long a provider call can hold
/// a worker.
pub const MIN_TIMEOUT_SECONDS: u32 = 1;
/// See [`MIN_TIMEOUT_SECONDS`].
pub const MAX_TIMEOUT_SECONDS: u32 = 600;

/// The provider a future connected run should use.
///
/// A closed enum rather than a free string. Every arm maps to a provider protocol the lab genuinely
/// speaks, so this cannot be used to name an endpoint the adapters would refuse at run time anyway.
/// The model name stays a string because model identifiers are provider-specific and open-ended;
/// it is validated for emptiness and shape, not against a list.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectedProvider {
    /// A base URL plus model, spoken over the OpenAI chat-completions protocol.
    ///
    /// Named explicitly rather than left to `rename_all`, because the derived form of `OpenAi` is
    /// `open_ai_compatible` and the provider registry calls this `openai_compatible`. A default
    /// rename here would have written a document the rest of the system could not match, which is
    /// exactly the class of defect the wire-format sweep exists to catch — and it is worth noting
    /// that this one was caught by a test asserting a *field list*, not by a round trip, since both
    /// ends would have agreed on the wrong spelling.
    #[serde(rename = "openai_compatible")]
    OpenAiCompatible,
}

impl ConnectedProvider {
    /// The stable wire name, which is also what the provider registry uses.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAiCompatible => "openai_compatible",
        }
    }
}

/// How the seed for a future experiment is chosen.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SeedBehaviour {
    /// Let the planner choose, which is what `root_seed: None` means.
    PlannerChosen,
    /// Always use the seed stored in these settings.
    Fixed,
}

impl SeedBehaviour {
    /// Whether this behaviour uses the configured seed.
    pub fn uses_configured_seed(self) -> bool {
        self == Self::Fixed
    }
}

/// The defaults a future experiment will run with.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExperimentDefaults {
    /// How many episodes to run.
    pub episodes: u32,
    /// How the root seed is chosen.
    pub seed_behaviour: SeedBehaviour,
    /// The seed used when [`Self::seed_behaviour`] is [`SeedBehaviour::Fixed`].
    pub root_seed: u64,
}

impl Default for ExperimentDefaults {
    fn default() -> Self {
        Self {
            // Five rather than the planner's own fallback of ten: it is a default a person is
            // unlikely to change, and a small enough number that a first run finishes while someone
            // is still watching.
            episodes: 5,
            seed_behaviour: SeedBehaviour::PlannerChosen,
            root_seed: 0,
        }
    }
}

/// The defaults a future connected run will use.
///
/// No field here can hold a credential. [`Self::api_key_env`] is the *name* of an environment
/// variable — a label such as `OPENAI_COMPATIBLE_API_KEY` — and the transport layer is the only
/// place that reads the variable's value. This is the existing design, preserved: the key is never
/// accepted, stored, or displayed by this application.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderDefaults {
    /// Which provider protocol to speak.
    pub provider: ConnectedProvider,
    /// Base URL or complete chat-completions endpoint. Credentials are rejected in it.
    pub base_url: String,
    /// The environment variable whose *name* is recorded here. Never its value.
    pub api_key_env: String,
    /// The model identifier to request.
    pub model: String,
    /// Request timeout in seconds.
    pub timeout_seconds: u32,
}

impl Default for ProviderDefaults {
    fn default() -> Self {
        Self {
            provider: ConnectedProvider::OpenAiCompatible,
            // Loopback, matching what the providers page already documents as the verified local
            // path. Not a placeholder that fails: this is a working default for a local
            // OpenAI-compatible server.
            base_url: "http://127.0.0.1:11434/v1".to_owned(),
            api_key_env: "OPENAI_COMPATIBLE_API_KEY".to_owned(),
            model: "local-model".to_owned(),
            timeout_seconds: 30,
        }
    }
}

/// Whether a future run should use the deterministic mock or the connected provider.
///
/// This is a *default*, not a switch. A run still has its own provider in its request, and this only
/// decides what the UI offers first. It cannot make a connected run happen without a credential
/// present, because the transport layer reads the environment regardless of what this says.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DefaultRunMode {
    /// Offer the deterministic mock first.
    Mock,
    /// Offer the connected provider first.
    Connected,
}

/// Which settings a person may change.
///
/// Exposed as a type so the IPC layer can answer "can this be edited" without duplicating the rule
/// somewhere in the frontend. Read-only entries are still shown, with the reason, because hiding a
/// value that exists is how a user concludes it does not.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingScope {
    /// Persisted and editable.
    Editable,
    /// Reported but not editable, because the architecture fixes it.
    ReadOnly,
}

/// One read-only fact about the lab, shown so the user knows where things are.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadOnlyFact {
    /// Stable identifier for the fact.
    pub id: String,
    /// What it is, in one line.
    pub label: String,
    /// The value, already safe to display.
    pub value: String,
    /// Why it cannot be changed here.
    pub reason: String,
}

/// The complete, validated settings for the lab.
///
/// Construct one with [`LabSettings::validated`], which is the only path that guarantees the value is
/// legal. Deserializing one is also validated on read from storage, so a hand-edited file cannot
/// smuggle an out-of-range value past the checks.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LabSettings {
    /// The contract version this document was written against.
    pub schema_version: u32,
    /// Defaults for future experiments.
    pub experiment_defaults: ExperimentDefaults,
    /// Defaults for future connected runs.
    pub provider_defaults: ProviderDefaults,
    /// Which provider the UI should offer first.
    pub default_run_mode: DefaultRunMode,
}

impl Default for LabSettings {
    fn default() -> Self {
        Self {
            schema_version: SETTINGS_SCHEMA_VERSION,
            experiment_defaults: ExperimentDefaults::default(),
            provider_defaults: ProviderDefaults::default(),
            default_run_mode: DefaultRunMode::Mock,
        }
    }
}

/// A rejected setting, naming the field and the constraint it broke.
///
/// Typed rather than a string so a panel can point at the offending control, and so a test can assert
/// the *reason* a value was refused rather than only that something was.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SettingsError {
    /// The document was written against a version this build cannot read.
    #[error(
        "settings schema version {actual} is not supported; this build reads version {expected}"
    )]
    UnsupportedSchemaVersion {
        /// The version this build understands.
        expected: u32,
        /// The version found in the stored document.
        actual: u32,
    },
    /// The episode count was zero or beyond the planner's ceiling.
    #[error("default episodes must be between 1 and {max}, not {actual}")]
    EpisodesOutOfRange {
        /// The ceiling that was applied.
        max: u32,
        /// The value that was refused.
        actual: u32,
    },
    /// The timeout was outside the supported range.
    #[error("timeout must be between {min} and {max} seconds, not {actual}")]
    TimeoutOutOfRange {
        /// The shortest supported timeout.
        min: u32,
        /// The longest supported timeout.
        max: u32,
        /// The value that was refused.
        actual: u32,
    },
    /// The provider base URL failed the adapter's own trusted validation.
    ///
    /// Delegated rather than reimplemented: the rules that decide whether a URL may carry a
    /// credential, use HTTPS, or contain a query string already exist in the transport layer, and a
    /// second copy of them here would be a second thing to get wrong.
    #[error("the provider base URL was refused: {0}")]
    InvalidBaseUrl(#[from] TransportConfigError),
    /// The environment variable *name* was empty or not a simple identifier.
    #[error(
        "the API key environment variable name must contain only letters, digits, and underscores"
    )]
    InvalidApiKeyEnvName,
    /// The model identifier was empty.
    ///
    /// Reported separately from the transport layer's own `EmptyModel` because the two name
    /// different things: that one is about a value being sent, and this one is about a stored
    /// preference being blank. Merging them would leave a panel unable to say which field it is.
    #[error("the model name must not be empty")]
    EmptyModel,
    /// The settings could not be written or read.
    #[error("settings could not be persisted: {0}")]
    Storage(String),
}

impl LabSettings {
    /// Validates these settings, returning a copy stamped with the current schema version.
    ///
    /// The version is stamped here rather than trusted from the caller, so a document that arrives
    /// without one still stores as a readable version.
    pub fn validated(self) -> Result<Self, SettingsError> {
        if self.schema_version != SETTINGS_SCHEMA_VERSION {
            return Err(SettingsError::UnsupportedSchemaVersion {
                expected: SETTINGS_SCHEMA_VERSION,
                actual: self.schema_version,
            });
        }

        if self.experiment_defaults.episodes == 0
            || self.experiment_defaults.episodes > MAX_DEFAULT_EPISODES
        {
            return Err(SettingsError::EpisodesOutOfRange {
                max: MAX_DEFAULT_EPISODES,
                actual: self.experiment_defaults.episodes,
            });
        }

        let timeout = self.provider_defaults.timeout_seconds;
        if !(MIN_TIMEOUT_SECONDS..=MAX_TIMEOUT_SECONDS).contains(&timeout) {
            return Err(SettingsError::TimeoutOutOfRange {
                min: MIN_TIMEOUT_SECONDS,
                max: MAX_TIMEOUT_SECONDS,
                actual: timeout,
            });
        }

        // The base URL, key name, and model all go through the same validation the transport layer
        // applies, so a setting that is accepted here is one a run could actually use. A setting the
        // runtime would refuse is a setting that lies.
        validate_base_url(&self.provider_defaults.base_url)?;
        validate_api_key_env(&self.provider_defaults.api_key_env)
            .map_err(|_| SettingsError::InvalidApiKeyEnvName)?;
        // `validate_model` covers emptiness as well as shape. The transport layer's own error is
        // discarded in favour of the settings-level one, because this is a blank *preference* rather
        // than a malformed value being sent to a provider, and a panel needs to say which.
        if validate_model(&self.provider_defaults.model).is_err() {
            return Err(SettingsError::EmptyModel);
        }

        Ok(LabSettings {
            schema_version: SETTINGS_SCHEMA_VERSION,
            ..self
        })
    }

    /// The episodes a new experiment should request.
    ///
    /// A method rather than a field read, because this is where "the default" becomes "the value to
    /// send", and that translation should be somewhere a test can reach.
    pub fn default_episodes(&self) -> u32 {
        self.experiment_defaults.episodes
    }

    /// The root seed a new experiment should send, or `None` to let the planner choose.
    pub fn default_root_seed(&self) -> Option<u64> {
        self.experiment_defaults
            .seed_behaviour
            .uses_configured_seed()
            .then_some(self.experiment_defaults.root_seed)
    }
}
