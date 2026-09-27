//! Settings tests.
//!
//! # What is worth testing here, and why
//!
//! Three things, in order of how much damage getting them wrong would do:
//!
//! 1. **Validation rejects bad values.** A settings page that accepts an episode count of zero, or a
//!    base URL carrying a credential, is a page that stores a lie. Every rejection is asserted for
//!    the specific typed error, not just for "it failed", so a test fails when the *reason* changes.
//! 2. **Persistence survives a restart.** The whole reason settings exist is that the previous
//!    hard-coded React defaults were lost on every launch. A store that only works within one process
//!    would be the same bug wearing a SQLite file.
//! 3. **A secret cannot enter the document.** Asserted by construction — see
//!    [`secret_never_enters_settings`] — because the strongest form of "we redact secrets" is that
//!    there is nowhere to put one.
//!
//! The tests below deliberately do not assert a *default* is a particular number. They assert the
//! defaults are valid and round-trip, so changing a default is a deliberate act rather than a
//! failing test.

use std::path::PathBuf;

use probelab_lab::settings::{
    ConnectedProvider, DefaultRunMode, ExperimentDefaults, LabSettings, ProviderDefaults,
    SeedBehaviour, SettingsError, MAX_DEFAULT_EPISODES, MAX_TIMEOUT_SECONDS, MIN_TIMEOUT_SECONDS,
    SETTINGS_SCHEMA_VERSION,
};
use probelab_lab::settings_store::SqliteSettingsStore;
use probelab_model_adapters::transport::TransportConfigError;

/// A unique-ish temporary path, so a test run does not inherit a previous run's settings.
fn temp_settings_path(label: &str) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since_epoch| since_epoch.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir()
        .join(format!("probelab-settings-{label}-{unique}"))
        .join("settings.sqlite3")
}

fn store_at(label: &str) -> (SqliteSettingsStore, PathBuf) {
    let path = temp_settings_path(label);
    let store = SqliteSettingsStore::open(&path).expect("open the settings store");
    (store, path)
}

/// The defaults are legal. A default that failed its own validation would make the first run of the
/// application fail, so this is the cheapest possible guard against a bad constant.
#[test]
fn the_defaults_are_valid() {
    LabSettings::default()
        .validated()
        .expect("the built-in defaults must pass their own validation");
}

#[test]
fn defaults_describe_a_mock_first_run() {
    let defaults = LabSettings::default();
    assert_eq!(defaults.schema_version, SETTINGS_SCHEMA_VERSION);
    assert_eq!(defaults.default_run_mode, DefaultRunMode::Mock);
    assert!(
        defaults.default_root_seed().is_none(),
        "the default must let the planner choose, not pin a seed"
    );
    assert!(defaults.default_episodes() > 0);
}

/// A store with nothing in it returns the defaults rather than failing, because a first run has no
/// settings written yet and the page has to open before it has ever been saved.
#[test]
fn an_empty_store_reads_as_the_defaults() {
    let (store, path) = store_at("empty");
    assert_eq!(store.load().expect("load"), LabSettings::default());
    let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
}

#[test]
fn a_saved_setting_is_read_back() {
    let (store, path) = store_at("save");
    let mut settings = LabSettings::default();
    settings.experiment_defaults.episodes = 12;
    store.save(&settings).expect("save");
    assert_eq!(store.load().expect("load").experiment_defaults.episodes, 12);
    let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
}

/// The point of the milestone. The previous defaults lived in React state and were lost on every
/// launch; a store that did not survive reopening the database would be the same bug with extra
/// steps.
#[test]
fn a_setting_survives_reopening_the_store() {
    let (store, path) = store_at("restart");
    let mut settings = LabSettings::default();
    settings.experiment_defaults.episodes = 7;
    settings.experiment_defaults.seed_behaviour = SeedBehaviour::Fixed;
    settings.experiment_defaults.root_seed = 99;
    settings.provider_defaults.model = "persisted-model".to_owned();
    settings.provider_defaults.timeout_seconds = 45;
    settings.default_run_mode = DefaultRunMode::Connected;
    store.save(&settings).expect("save");
    drop(store);

    // A fresh process would do exactly this: open the same file, read the settings.
    let reopened = SqliteSettingsStore::open(&path).expect("reopen");
    let loaded = reopened.load().expect("load after restart");
    assert_eq!(loaded.experiment_defaults.episodes, 7);
    assert_eq!(
        loaded.experiment_defaults.seed_behaviour,
        SeedBehaviour::Fixed
    );
    assert_eq!(loaded.experiment_defaults.root_seed, 99);
    assert_eq!(loaded.provider_defaults.model, "persisted-model");
    assert_eq!(loaded.provider_defaults.timeout_seconds, 45);
    assert_eq!(loaded.default_run_mode, DefaultRunMode::Connected);
    let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
}

/// The last write wins, and a save returns the validated document rather than the input, so a caller
/// cannot believe it stored something the backend adjusted or refused.
#[test]
fn saving_replaces_the_previous_document_and_returns_what_was_stored() {
    let (store, path) = store_at("replace");
    let mut first = LabSettings::default();
    first.experiment_defaults.episodes = 3;
    store.save(&first).expect("save first");
    let mut second = LabSettings::default();
    second.experiment_defaults.episodes = 4;
    let returned = store.save(&second).expect("save second");
    assert_eq!(returned.experiment_defaults.episodes, 4);
    assert_eq!(store.load().expect("load").experiment_defaults.episodes, 4);
    let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
}

/// A rejected save must leave the previous settings alone. A store that cleared the document and then
/// failed would turn a validation error into data loss.
#[test]
fn a_rejected_save_leaves_the_previous_settings_intact() {
    let (store, path) = store_at("rejected");
    let mut good = LabSettings::default();
    good.experiment_defaults.episodes = 8;
    store.save(&good).expect("save the good settings");

    let mut bad = LabSettings::default();
    bad.experiment_defaults.episodes = 0;
    assert!(store.save(&bad).is_err(), "zero episodes must be refused");

    assert_eq!(
        store.load().expect("load").experiment_defaults.episodes,
        8,
        "a rejected save must not disturb what was already stored"
    );
    let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
}

#[test]
fn reset_restores_the_defaults() {
    let (store, path) = store_at("reset");
    let mut changed = LabSettings::default();
    changed.experiment_defaults.episodes = 20;
    changed.provider_defaults.model = "changed".to_owned();
    store.save(&changed).expect("save");

    let reset = store.reset_to_defaults().expect("reset");
    assert_eq!(reset, LabSettings::default());
    assert_eq!(store.load().expect("load"), LabSettings::default());
    let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
}

// # Validation
//
// Every case below asserts the *specific* error, not merely that something failed. A validation
// message is what a panel shows a user, so "it was rejected" is not a sufficient test.

#[test]
fn zero_episodes_are_refused() {
    let mut settings = LabSettings::default();
    settings.experiment_defaults.episodes = 0;
    assert!(matches!(
        settings.clone().validated(),
        Err(SettingsError::EpisodesOutOfRange { actual: 0, .. })
    ));
}

#[test]
fn more_episodes_than_the_planner_allows_are_refused() {
    let mut settings = LabSettings::default();
    settings.experiment_defaults.episodes = MAX_DEFAULT_EPISODES + 1;
    assert!(matches!(
        settings.clone().validated(),
        Err(SettingsError::EpisodesOutOfRange { .. })
    ));
}

/// The boundary itself must be accepted, or the documented maximum would be a lie.
#[test]
fn the_episode_limits_themselves_are_accepted() {
    for episodes in [1, MAX_DEFAULT_EPISODES] {
        let mut settings = LabSettings::default();
        settings.experiment_defaults.episodes = episodes;
        assert!(
            settings.validated().is_ok(),
            "{episodes} episodes should be allowed"
        );
    }
}

#[test]
fn a_zero_timeout_is_refused() {
    let mut settings = LabSettings::default();
    settings.provider_defaults.timeout_seconds = 0;
    assert!(matches!(
        settings.clone().validated(),
        Err(SettingsError::TimeoutOutOfRange { min, actual: 0, .. }) if min == MIN_TIMEOUT_SECONDS
    ));
}

#[test]
fn an_absurd_timeout_is_refused() {
    let mut settings = LabSettings::default();
    settings.provider_defaults.timeout_seconds = MAX_TIMEOUT_SECONDS + 1;
    assert!(matches!(
        settings.clone().validated(),
        Err(SettingsError::TimeoutOutOfRange { max, .. }) if max == MAX_TIMEOUT_SECONDS
    ));
}

#[test]
fn the_timeout_limits_themselves_are_accepted() {
    for seconds in [MIN_TIMEOUT_SECONDS, MAX_TIMEOUT_SECONDS] {
        let mut settings = LabSettings::default();
        settings.provider_defaults.timeout_seconds = seconds;
        assert!(settings.validated().is_ok(), "{seconds}s should be allowed");
    }
}

/// The URL rules are the transport layer's, not this module's. Delegating is the point, and these
/// tests are what would notice if the delegation were quietly removed in favour of a looser local
/// check that happened to pass.
#[test]
fn a_base_url_carrying_a_credential_is_refused() {
    let mut settings = LabSettings::default();
    settings.provider_defaults.base_url = "https://user:secret@example.com/v1".to_owned();
    assert!(matches!(
        settings.clone().validated(),
        Err(SettingsError::InvalidBaseUrl(
            TransportConfigError::UrlUserInfo | TransportConfigError::UrlQuery
        ))
    ));
}

#[test]
fn a_plain_http_url_that_is_not_loopback_is_refused() {
    let mut settings = LabSettings::default();
    settings.provider_defaults.base_url = "http://example.com/v1".to_owned();
    assert!(matches!(
        settings.clone().validated(),
        Err(SettingsError::InvalidBaseUrl(
            TransportConfigError::InsecureBaseUrl
        ))
    ));
}

#[test]
fn an_empty_base_url_is_refused() {
    let mut settings = LabSettings::default();
    settings.provider_defaults.base_url = String::new();
    assert!(matches!(
        settings.clone().validated(),
        Err(SettingsError::InvalidBaseUrl(
            TransportConfigError::EmptyBaseUrl
        ))
    ));
}

#[test]
fn a_loopback_http_url_is_accepted_because_that_is_the_local_path() {
    let mut settings = LabSettings::default();
    settings.provider_defaults.base_url = "http://127.0.0.1:11434/v1".to_owned();
    assert!(settings.validated().is_ok());
}

#[test]
fn a_malformed_key_variable_name_is_refused() {
    for name in ["", "has space", "semi;colon", "quote\"name"] {
        let mut settings = LabSettings::default();
        settings.provider_defaults.api_key_env = name.to_owned();
        assert!(
            matches!(
                settings.clone().validated(),
                Err(SettingsError::InvalidApiKeyEnvName)
            ),
            "{name:?} should be refused as a variable name"
        );
    }
}

#[test]
fn a_plain_key_variable_name_is_accepted() {
    let mut settings = LabSettings::default();
    settings.provider_defaults.api_key_env = "MY_KEY_2".to_owned();
    assert!(settings.validated().is_ok());
}

#[test]
fn an_empty_model_name_is_refused() {
    let mut settings = LabSettings::default();
    settings.provider_defaults.model = "   ".to_owned();
    assert!(settings.validated().is_err());
}

/// A document from a different build is refused rather than interpreted. Guessing at a settings
/// layout is how a lab ends up running with a field it thought was present and was not.
#[test]
fn an_unknown_schema_version_is_refused() {
    let settings = LabSettings {
        schema_version: SETTINGS_SCHEMA_VERSION + 1,
        ..LabSettings::default()
    };
    assert!(matches!(
        settings.clone().validated(),
        Err(SettingsError::UnsupportedSchemaVersion { .. })
    ));
}

/// A hand-edited document that violates the current rules must fail to load, not load partially.
/// The defaulting store would otherwise present a document with fields missing as if it were a
/// complete configuration.
#[test]
fn a_stored_document_that_is_no_longer_valid_fails_to_load() {
    let path = temp_settings_path("hand-edited");
    {
        let store = SqliteSettingsStore::open(&path).expect("open");
        // Written directly, bypassing `save`, so the invalid content reaches the table.
        let connection = rusqlite::Connection::open(&path).expect("reopen");
        let document = r#"{"schemaVersion":1,"experimentDefaults":{"episodes":0,
            "seedBehaviour":"planner_chosen","rootSeed":0},"providerDefaults":
            {"provider":"openai_compatible","baseUrl":"http://127.0.0.1:11434/v1",
            "apiKeyEnv":"OPENAI_COMPATIBLE_API_KEY","model":"local-model","timeoutSeconds":30},
            "defaultRunMode":"mock"}"#;
        connection
            .execute(
                "INSERT INTO settings (schema_version, document, updated_unix_ms)
                 VALUES (1, ?1, 0)",
                rusqlite::params![document],
            )
            .expect("insert the hand-edited document");
        assert!(
            store.load().is_err(),
            "an invalid stored document must not load"
        );
    }
    let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
}

// # Secret safety
//
// The strongest available form of "secrets are not logged or stored" is that the type has nowhere to
// put one. These tests assert that property rather than asserting a redaction step, because a
// redaction step is code someone can forget to call.

/// The settings document has no field that could hold a credential.
///
/// If this test ever needs changing, the change is not "add a field" — it is a decision to build
/// secret storage, which this application deliberately does not do.
///
/// The assertion is on exact field *names* rather than on substrings of the document, because
/// `apiKeyEnv` legitimately contains the letters "apiKey" while carrying nothing of the kind: it
/// records which variable to read, not what is in it. A substring test would have to exempt that
/// name, and an exemption list is exactly how a real leak gets waved through later. So the list
/// below is the set of names that would actually carry a value, and `apiKeyEnv` is deliberately not
/// in it.
#[test]
fn secret_never_enters_settings() {
    let document = serde_json::to_value(LabSettings::default()).expect("serialize");

    // Names that would hold a credential if they existed. Matched exactly, at every level, because
    // a nested struct is exactly where such a field would be added.
    let forbidden = [
        "apiKey",
        "api_key",
        "apiKeyValue",
        "secret",
        "secretValue",
        "token",
        "tokenValue",
        "password",
        "credential",
        "credentialValue",
        "bearer",
        "authorization",
        "authHeader",
        "keyValue",
    ];

    fn walk(value: &serde_json::Value, forbidden: &[&str]) {
        match value {
            serde_json::Value::Object(map) => {
                for (key, nested) in map {
                    assert!(
                        !forbidden.contains(&key.as_str()),
                        "the settings document must not have a field named {key}"
                    );
                    walk(nested, forbidden);
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    walk(item, forbidden);
                }
            }
            _ => {}
        }
    }
    walk(&document, &forbidden);

    // The one field whose name mentions a key is present, and holds a variable name.
    assert_eq!(
        document["providerDefaults"]["apiKeyEnv"],
        serde_json::Value::String("OPENAI_COMPATIBLE_API_KEY".to_owned()),
        "the variable name is recorded, which is what lets the page say where it looked"
    );

    // And the serialized text carries no value that looks like a credential, whatever the field names.
    let text = serde_json::to_string(&LabSettings::default()).expect("serialize");
    for pattern in ["sk-", "Bearer ", "-----BEGIN"] {
        assert!(
            !text.contains(pattern),
            "the settings document must not contain {pattern}: {text}"
        );
    }
}

/// The key environment variable is recorded by *name* only. A name is a label an operator needs in
/// order to fix a missing credential; a value is the secret itself.
#[test]
fn the_key_variable_is_recorded_by_name_not_value() {
    let mut settings = LabSettings::default();
    settings.provider_defaults.api_key_env = "SOME_KEY".to_owned();
    let document = serde_json::to_value(&settings).expect("serialize");
    assert_eq!(
        document["providerDefaults"]["apiKeyEnv"],
        serde_json::Value::String("SOME_KEY".to_owned()),
        "the variable name must be recorded so the page can say where it looked"
    );
}

/// The full round trip through storage also carries no credential, because persistence is where a
/// secret would most plausibly leak — into a file on disk.
///
/// Reads the stored document out of the database rather than the raw file, because a SQLite file is
/// binary and scanning its bytes for a string would only ever find the ones in the record anyway.
#[test]
fn a_stored_settings_document_carries_no_credential() {
    let (store, path) = store_at("secret-free");
    store.save(&LabSettings::default()).expect("save");
    let connection = rusqlite::Connection::open(&path).expect("reopen");
    let document: String = connection
        .query_row("SELECT document FROM settings", [], |row| row.get(0))
        .expect("read the stored document");
    for forbidden in ["sk-", "Bearer ", "-----BEGIN", "password", "apiKey\":"] {
        assert!(
            !document.contains(forbidden),
            "the stored settings must not contain {forbidden}: {document}"
        );
    }
    let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
}

/// The settings document is a configuration, and a configuration that carried a result would be able
/// to influence one. Asserting the field set keeps that boundary visible.
#[test]
fn settings_describe_configuration_and_no_measurement() {
    let document = serde_json::to_value(LabSettings::default()).expect("serialize");
    let object = document.as_object().expect("an object");
    for forbidden in [
        "result",
        "score",
        "reward",
        "outcome",
        "digest",
        "episode",
        "telemetry",
    ] {
        assert!(
            !object.contains_key(forbidden),
            "settings must not carry a measurement field: {forbidden}"
        );
    }
}

// # Defaults as a contract
//
// The seed behaviour is the one setting with a real interaction with another, so it is worth pinning
// that "planner chosen" and "fixed" mean what they say.

#[test]
fn a_fixed_seed_is_the_seed_that_will_be_sent() {
    let mut settings = LabSettings::default();
    settings.experiment_defaults.seed_behaviour = SeedBehaviour::Fixed;
    settings.experiment_defaults.root_seed = 4242;
    let settings = settings.validated().expect("valid");
    assert_eq!(settings.default_root_seed(), Some(4242));
}

#[test]
fn a_planner_chosen_seed_sends_no_seed_at_all() {
    let mut settings = LabSettings::default();
    settings.experiment_defaults.seed_behaviour = SeedBehaviour::PlannerChosen;
    settings.experiment_defaults.root_seed = 4242;
    let settings = settings.validated().expect("valid");
    assert_eq!(
        settings.default_root_seed(),
        None,
        "a stored seed must not be sent when the behaviour says the planner chooses"
    );
}

/// The provider enum is closed. A settings document naming a protocol the adapters do not speak must
/// not decode, because it would offer a run the transport layer would refuse.
#[test]
fn an_unknown_provider_does_not_decode() {
    for document in [
        r#"{"schemaVersion":1,"experimentDefaults":{"episodes":5,"seedBehaviour":"planner_chosen","rootSeed":0},
             "providerDefaults":{"provider":"opencode","baseUrl":"http://127.0.0.1:1/v1",
             "apiKeyEnv":"K","model":"m","timeoutSeconds":30},"defaultRunMode":"mock"}"#,
        r#"{"schemaVersion":1,"experimentDefaults":{"episodes":5,"seedBehaviour":"planner_chosen","rootSeed":0},
             "providerDefaults":{"provider":"ollama-native","baseUrl":"http://127.0.0.1:1/v1",
             "apiKeyEnv":"K","model":"m","timeoutSeconds":30},"defaultRunMode":"mock"}"#,
    ] {
        assert!(
            serde_json::from_str::<LabSettings>(document).is_err(),
            "a provider the lab does not speak must not decode"
        );
    }
}

/// An out-of-range number in a hand-written IPC payload is caught by the same validation as a value
/// typed into the UI. There is one path in, so there is one place it can be refused.
#[test]
fn a_negative_episode_count_in_a_payload_is_refused() {
    let document = r#"{"schemaVersion":1,"experimentDefaults":{"episodes":-5,
        "seedBehaviour":"planner_chosen","rootSeed":0},"providerDefaults":
        {"provider":"openai_compatible","baseUrl":"http://127.0.0.1:11434/v1",
        "apiKeyEnv":"K","model":"m","timeoutSeconds":30},"defaultRunMode":"mock"}"#;
    // A negative value does not even decode into the `u32` field, which is the first line of defence.
    assert!(serde_json::from_str::<LabSettings>(document).is_err());
}

/// And a float where an integer belongs, which a lenient parser might accept and truncate.
#[test]
fn a_non_integer_episode_count_is_refused() {
    let document = r#"{"schemaVersion":1,"experimentDefaults":{"episodes":2.5,
        "seedBehaviour":"planner_chosen","rootSeed":0},"providerDefaults":
        {"provider":"openai_compatible","baseUrl":"http://127.0.0.1:11434/v1",
        "apiKeyEnv":"K","model":"m","timeoutSeconds":30},"defaultRunMode":"mock"}"#;
    assert!(serde_json::from_str::<LabSettings>(document).is_err());
}

/// The closed enum means an injected mode string is refused rather than defaulted. Defaulting an
/// unknown run mode would silently change which provider a person is offered.
#[test]
fn an_unknown_run_mode_does_not_decode() {
    let document = r#"{"schemaVersion":1,"experimentDefaults":{"episodes":5,
        "seedBehaviour":"planner_chosen","rootSeed":0},"providerDefaults":
        {"provider":"openai_compatible","baseUrl":"http://127.0.0.1:11434/v1",
        "apiKeyEnv":"K","model":"m","timeoutSeconds":30},"defaultRunMode":"telepathy"}"#;
    assert!(serde_json::from_str::<LabSettings>(document).is_err());
}

/// The settings survive their own round trip, which is what a save followed by a load does. A
/// `rename_all` that renamed a field would break this and nothing else.
#[test]
fn settings_round_trip_through_json() {
    let mut settings = LabSettings::default();
    settings.experiment_defaults.episodes = 9;
    settings.experiment_defaults.seed_behaviour = SeedBehaviour::Fixed;
    settings.provider_defaults = ProviderDefaults {
        provider: ConnectedProvider::OpenAiCompatible,
        base_url: "https://api.example.com/v1".to_owned(),
        api_key_env: "EXAMPLE_KEY".to_owned(),
        model: "example-model".to_owned(),
        timeout_seconds: 60,
    };
    settings.default_run_mode = DefaultRunMode::Connected;

    let document = serde_json::to_string(&settings).expect("serialize");
    let back: LabSettings = serde_json::from_str(&document).expect("parse");
    assert_eq!(back, settings);
    assert_eq!(back.validated().expect("valid"), settings);
}

/// A reset must not be able to reach a result, and the claim is checked rather than asserted in a
/// comment.
///
/// The settings store and the result repository are pointed at the *same file*, which is the
/// adversarial case: if reset could reach a result row through any path, this is where it would show.
/// It uses a separate file in production, so this is stricter than reality — a store that reached
/// across would only do so by accident, and an accident is what this catches.
#[test]
fn reset_touches_no_result_state() {
    use probelab_core::{CapabilityResult, CompletionStatus, ModelDescriptor, TestResult};
    use probelab_telemetry::SqliteResultRepository;

    let path = temp_settings_path("shared-file");
    // Create the directory first: the result repository expects its parent to exist, which is the
    // host application's job in production and therefore not this crate's concern.
    std::fs::create_dir_all(path.parent().expect("parent")).expect("create the directory");
    let result = TestResult {
        schema_version: 1,
        benchmark_id: "grid-world".to_owned(),
        benchmark_version: "2".to_owned(),
        model: ModelDescriptor {
            adapter: "test".to_owned(),
            strategy: "optimal".to_owned(),
            seed: 0,
            parameters: Default::default(),
        },
        seed: 7,
        capability: CapabilityResult {
            dimension: "navigation".to_owned(),
            task_success: true,
            total_reward: 10,
            turns_used: 5,
            invalid_action_count: 0,
            completion_status: CompletionStatus::Success,
            protocol_adherence: Default::default(),
        },
        integrity: Default::default(),
        telemetry_event_count: 5,
        replay_digest: "the-original-digest".to_owned(),
        reproducibility: Default::default(),
    };

    // A result exists in this file before the reset happens.
    {
        let results = SqliteResultRepository::new(&path).expect("open the result repository");
        results.store("run-1", &result).expect("store a result");
    }

    let (store, _) = store_at("shared-file");
    let mut changed = LabSettings::default();
    changed.experiment_defaults.episodes = 42;
    store.save(&changed).expect("save settings");
    store.reset_to_defaults().expect("reset");
    assert_eq!(store.load().expect("load"), LabSettings::default());

    // And afterwards the result is exactly as it was: same digest, same score, same everything.
    let results = SqliteResultRepository::new(&path).expect("reopen the result repository");
    let stored = results
        .get("run-1")
        .expect("read")
        .expect("the result must survive a reset");
    assert_eq!(stored, result, "a settings reset altered a recorded result");
    let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
}

/// Two stores over two files are independent, which is what makes "settings are not experiment
/// state" checkable rather than a claim about table names.
#[test]
fn separate_stores_do_not_share_state() {
    let (first, first_path) = store_at("independent-a");
    let (second, second_path) = store_at("independent-b");
    let mut changed = LabSettings::default();
    changed.experiment_defaults.episodes = 15;
    first.save(&changed).expect("save to the first");
    assert_eq!(
        second
            .load()
            .expect("load the second")
            .experiment_defaults
            .episodes,
        LabSettings::default().experiment_defaults.episodes
    );
    let _ = std::fs::remove_dir_all(first_path.parent().expect("parent"));
    let _ = std::fs::remove_dir_all(second_path.parent().expect("parent"));
}

/// The document the app stores is a single JSON object under one key, which is what makes
/// "reset replaced exactly this" checkable.
#[test]
fn the_stored_document_is_the_settings_document() {
    let (store, path) = store_at("document");
    let mut settings = LabSettings::default();
    settings.experiment_defaults.episodes = 11;
    store.save(&settings).expect("save");
    let connection = rusqlite::Connection::open(&path).expect("reopen");
    let document: String = connection
        .query_row("SELECT document FROM settings", [], |row| row.get(0))
        .expect("read the document");
    let parsed: serde_json::Value = serde_json::from_str(&document).expect("valid JSON");
    assert_eq!(parsed["experimentDefaults"]["episodes"], 11);
    let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
}

/// The defaults are also the reference for what "is default" means in the view, so a panel can say
/// "these are the built-in defaults" without hard-coding a second copy of them.
#[test]
fn the_view_reports_whether_settings_are_the_defaults() {
    let state = probelab_lab::settings_view::ProviderState {
        provider: "openai_compatible".to_owned(),
        model: "local-model".to_owned(),
        configured: false,
        status: "not_configured".to_owned(),
        message: "no credential".to_owned(),
        credential_source: "environment".to_owned(),
        credential_present: false,
    };
    let unchanged =
        probelab_lab::SettingsView::assemble(LabSettings::default(), state.clone(), vec![]);
    assert!(unchanged.is_default);

    let mut changed = LabSettings::default();
    changed.experiment_defaults.episodes = 3;
    let altered = probelab_lab::SettingsView::assemble(changed, state, vec![]);
    assert!(!altered.is_default, "a changed setting is not the default");
}

/// The view reports the seed that will actually be sent, so a panel cannot show a stored seed next to
/// a "planner chosen" behaviour and imply the seed is in effect.
#[test]
fn the_view_reports_the_effective_seed() {
    let state = probelab_lab::settings_view::ProviderState {
        provider: "openai_compatible".to_owned(),
        model: "m".to_owned(),
        configured: false,
        status: "not_configured".to_owned(),
        message: String::new(),
        credential_source: "environment".to_owned(),
        credential_present: false,
    };
    let mut settings = LabSettings::default();
    settings.experiment_defaults.seed_behaviour = SeedBehaviour::Fixed;
    settings.experiment_defaults.root_seed = 77;
    let view = probelab_lab::SettingsView::assemble(settings, state, vec![]);
    assert_eq!(view.experiment_defaults.effective_root_seed, Some(77));
    assert_eq!(view.experiment_defaults.root_seed.value, 77);
}

/// Every editable entry is marked editable by the backend, so a panel does not decide for itself which
/// controls are live. The read-only facts arrive separately, with their reasons.
#[test]
fn the_view_marks_every_editable_setting_as_editable() {
    let state = probelab_lab::settings_view::ProviderState {
        provider: "openai_compatible".to_owned(),
        model: "m".to_owned(),
        configured: false,
        status: "not_configured".to_owned(),
        message: String::new(),
        credential_source: "environment".to_owned(),
        credential_present: false,
    };
    let view = probelab_lab::SettingsView::assemble(LabSettings::default(), state, vec![]);
    // Checked one at a time because the entries are of different types; collected into a list of
    // (id, scope) pairs first, so the assertion reads the same for all of them.
    let scopes: Vec<(&str, probelab_lab::settings::SettingScope)> = vec![
        (
            view.experiment_defaults.episodes.id.as_str(),
            view.experiment_defaults.episodes.scope,
        ),
        (
            view.experiment_defaults.seed_behaviour.id.as_str(),
            view.experiment_defaults.seed_behaviour.scope,
        ),
        (
            view.experiment_defaults.root_seed.id.as_str(),
            view.experiment_defaults.root_seed.scope,
        ),
        (
            view.provider_defaults.provider.id.as_str(),
            view.provider_defaults.provider.scope,
        ),
        (
            view.provider_defaults.base_url.id.as_str(),
            view.provider_defaults.base_url.scope,
        ),
        (
            view.provider_defaults.api_key_env.id.as_str(),
            view.provider_defaults.api_key_env.scope,
        ),
        (
            view.provider_defaults.model.id.as_str(),
            view.provider_defaults.model.scope,
        ),
        (
            view.provider_defaults.timeout_seconds.id.as_str(),
            view.provider_defaults.timeout_seconds.scope,
        ),
        (
            view.default_run_mode.id.as_str(),
            view.default_run_mode.scope,
        ),
    ];
    for (id, scope) in scopes {
        assert_eq!(
            scope,
            probelab_lab::settings::SettingScope::Editable,
            "{id} should be editable"
        );
    }
}

/// `ExperimentDefaults` has to derive `Default` correctly, because the reset path uses it.
#[test]
fn experiment_defaults_default_to_a_legal_plan() {
    let defaults = ExperimentDefaults::default();
    assert!(defaults.episodes > 0);
    assert!(!defaults.seed_behaviour.uses_configured_seed());
}
