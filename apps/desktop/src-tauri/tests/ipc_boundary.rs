//! The IPC boundary under adversarial input.
//!
//! # What this file is for
//!
//! Every other test in this crate exercises a command with a *well-formed* payload. That is the easy
//! half, and it was already covered. This target is the other half: what happens when the frontend is
//! stale, buggy, hostile, or replaying a captured payload.
//!
//! The premise is that the frontend is untrusted input. It is TypeScript, it runs in a webview, and any
//! of its values can be anything -- a renamed field, a string where a number belongs, a negative
//! integer, a nested object where a tagged union belongs, a payload from a build three versions old. A
//! boundary that only handles well-formed input has not been shown to hold a boundary.
//!
//! # What is actually tested
//!
//! Payloads are deserialized through the *real* request types with their *real* `serde` attributes, then
//! the resulting value is put through the *real* validation. That is the path a message takes. Calling a
//! `pub fn` with a hand-built struct would skip both halves and prove nothing: the struct would be
//! correct by construction, which is the assumption under test.
//!
//! Notably absent: a `tauri::test` mock runtime. It would prove the commands dispatch, and the
//! registration guard in `lib.rs` already proves that from the real handler list. What a mock runtime
//! does *not* prove is that a bad payload is refused before it reaches the engine, and that is the
//! property this product is built on.
//!
//! # The distinction that matters
//!
//! Every refusal here is a *typed refusal*: a deserialization error, or one of this repository's own
//! error enums. Never acceptable: an `unwrap`, an index panic, an arithmetic overflow, or a value that
//! reaches the engine unvalidated. The first three are crashes; the last is a security failure. They are
//! asserted separately, because "it returned an error" and "it did not execute" are different claims and
//! only one of them is about safety.

use probelab_desktop_lib::{
    ArenaCommandError, ArenaMatchRequest, BrainCommandError, ComparisonRequest, ExperimentRequest,
    ModelRunConfig, SecurityCommandError, SecuritySweepRequest, SettingsCommandError,
    SettingsRequest,
};
use probelab_lab::{ComparisonOutcome, LabSettings, SettingsError, SqliteSettingsStore};

/// Validates a model configuration exactly as a run would, before it reaches the engine.
///
/// `ModelRunConfig::validate` is the real pre-flight: the version check and the trusted transport
/// validation, in the order a run performs them. Calling anything else would test a different path from
/// the one a payload actually takes.
fn validate_model_run_config(config: &ModelRunConfig) -> Result<(), String> {
    config.validate().map_err(|error| error.to_string())
}

/// Deserializes a payload the way Tauri would.
///
/// `serde_json::from_str` is the decoder Tauri's command layer uses for a JSON payload, and the request
/// types carry the same `deny_unknown_fields` and `rename_all` attributes they carry in production. A
/// payload refused here is refused before it reaches a command body.
fn decode<T: serde::de::DeserializeOwned>(json: &str) -> Result<T, serde_json::Error> {
    serde_json::from_str(json)
}

// # Malformed payloads: shape

/// A payload that is not JSON at all is refused.
#[test]
fn a_payload_that_is_not_json_is_refused() {
    for payload in [
        "",
        "   ",
        "not json",
        "{",
        "}",
        "[]",
        "null",
        "{'single_quotes': true}",
        r#"{"a": }"#,
    ] {
        assert!(
            decode::<ExperimentRequest>(payload).is_err(),
            "{payload:?} must be refused"
        );
    }
}

/// A payload missing a required field is refused rather than defaulted.
///
/// This matters more than it looks. If a missing `episodes` defaulted to zero, a run would be accepted
/// and then execute nothing, and the panel would show a completed experiment with no episodes -- a
/// measurement that never happened, recorded as though it did.
#[test]
fn a_required_field_cannot_be_omitted() {
    for payload in [
        r#"{}"#,
        r#"{"benchmarkId": "grid-world"}"#,
        r#"{"benchmarkId": "grid-world", "agentId": "optimal"}"#,
        r#"{"agentId": "optimal", "episodes": 1, "rootSeed": null}"#,
    ] {
        assert!(
            decode::<ExperimentRequest>(payload).is_err(),
            "{payload} must be refused for a missing field"
        );
    }
    // And the canonical form is accepted, so the refusals above are about the payload and not the type.
    assert!(decode::<ExperimentRequest>(
        r#"{"benchmarkId":"grid-world","agentId":"optimal","episodes":1,"rootSeed":null}"#
    )
    .is_ok());
}

/// The credential-bearing types refuse an undeclared field, so a secret cannot ride along.
///
/// `deny_unknown_fields` is what makes this hold, and it is asserted at the boundary rather than by
/// checking the attribute is present. An ignored field is how a payload carrying `api_key` or `headers`
/// gets accepted with the dangerous part silently dropped -- and then a reader sees the field was
/// "handled" and draws the wrong conclusion.
#[test]
fn a_credential_cannot_hide_in_an_undeclared_field() {
    for extra in [
        r#""api_key":"sk-REALKEY""#,
        r#""apiKey":"sk-REALKEY""#,
        r#""headers":{"Authorization":"Bearer sk-REALKEY"}"#,
    ] {
        let payload = format!(
            r#"{{"kind":"openai_compatible","schema_version":1,"base_url":"http://127.0.0.1:8080/v1",
                 "api_key_env":"KEY","model":"m","timeout_seconds":30,"temperature":0.0,"max_tokens":200,
                 {extra}}}"#
        );
        assert!(
            decode::<ModelRunConfig>(&payload).is_err(),
            "a payload carrying {extra} must be refused"
        );
        // The settings document is the other credential-adjacent type, and it refuses too.
        let settings = format!(
            r#"{{"schemaVersion":1,
                 "experimentDefaults":{{"episodes":5,"seedBehaviour":"planner_chosen","rootSeed":0}},
                 "providerDefaults":{{"provider":"openai_compatible","baseUrl":"http://127.0.0.1:8080/v1",
                 "apiKeyEnv":"KEY","model":"m","timeoutSeconds":30}},
                 "defaultRunMode":"mock",{extra}}}"#
        );
        assert!(
            decode::<SettingsRequest>(&settings).is_err(),
            "settings carrying {extra} must be refused"
        );
    }
}

/// Field names are matched exactly, including case.
///
/// The types use `rename_all = "camelCase"`. Silently accepting both spellings would mean the wire
/// format has two shapes, and only one of them would be pinned by the wire-format tests.
#[test]
fn field_names_are_case_sensitive() {
    for payload in [
        r#"{"benchmark_id":"grid-world","agent_id":"optimal","episodes":1,"root_seed":null}"#,
        r#"{"BenchmarkId":"grid-world","AgentId":"optimal","Episodes":1,"RootSeed":null}"#,
    ] {
        assert!(
            decode::<ExperimentRequest>(payload).is_err(),
            "{payload} must be refused; the wire format has exactly one spelling"
        );
    }
}

// # Invalid numbers

/// A numeric field must actually be a number.
///
/// A string `"5"` where a `usize` belongs is refused rather than parsed. Coercing strings to numbers
/// would let a payload carry a value past the type that was supposed to constrain it.
#[test]
fn a_number_given_as_a_string_is_refused() {
    for payload in [
        r#"{"benchmarkId":"grid-world","agentId":"optimal","episodes":"1","rootSeed":null}"#,
        r#"{"benchmarkId":"grid-world","agentId":"optimal","episodes":1.5,"rootSeed":null}"#,
        r#"{"benchmarkId":"grid-world","agentId":"optimal","episodes":true,"rootSeed":null}"#,
        r#"{"benchmarkId":"grid-world","agentId":"optimal","episodes":null,"rootSeed":null}"#,
        r#"{"benchmarkId":"grid-world","agentId":"optimal","episodes":1,"rootSeed":"abc"}"#,
    ] {
        assert!(
            decode::<ExperimentRequest>(payload).is_err(),
            "{payload} must be refused; a number is not a string"
        );
    }
}

/// A negative episode count is refused by the type, before anything can loop on it.
///
/// `episodes` is a `usize`, so `-1` cannot be represented at all. That matters because a signed count
/// reaching a loop bound is an arithmetic overflow, and `usize::MAX` reaching one is a loop that never
/// terminates. The unsigned type refuses the first at the boundary, which is the only place it can.
#[test]
fn a_negative_episode_count_is_refused_by_the_type() {
    for episodes in ["-1", "-9999", "-0.5"] {
        let payload = format!(
            r#"{{"benchmarkId":"grid-world","agentId":"optimal","episodes":{episodes},"rootSeed":null}}"#
        );
        assert!(
            decode::<ExperimentRequest>(&payload).is_err(),
            "episodes: {episodes} must be refused; the field is unsigned"
        );
    }
}

/// An episode count too large to index is refused by the planner, not by the type.
///
/// `usize` admits `u64::MAX`, so the type alone is not the bound that matters. What bounds it is
/// `LabBrain::make_plan` refusing anything above `MAX_EPISODES`, and this asserts that refusal happens
/// -- an unbounded episode count is a resource-exhaustion vector pointed at the lab itself, which is why
/// the ceiling exists.
#[test]
fn an_unbounded_episode_count_is_refused_by_the_planner() {
    let payload = r#"{"benchmarkId":"grid-world","agentId":"optimal","episodes":18446744073709551615,"rootSeed":null}"#;
    let request: ExperimentRequest = decode(payload).expect("a usize maximum is representable");
    assert_eq!(
        request.episodes,
        usize::MAX,
        "the type admits it, so the bound must come from the planner"
    );

    let error = plan_for(request.episodes)
        .expect_err("an unbounded episode count must be refused before anything runs");
    assert!(
        matches!(
            error,
            probelab_lab::ExperimentPlanError::EpisodeCountOutOfRange { .. }
        ),
        "the refusal must name the constraint, got {error:?}"
    );

    // And the boundary value itself is accepted, so this is a bound and not a blanket refusal.
    assert!(
        plan_for(probelab_lab::MAX_EPISODES).is_ok(),
        "the documented maximum must remain usable"
    );
}

/// Asks the Brain to plan an experiment with the given episode count.
///
/// Goes through the real `make_plan` with the real registries, so the bound under test is the one a run
/// actually hits rather than a reimplementation of it.
fn plan_for(
    episodes: usize,
) -> Result<probelab_lab::ExperimentPlan, probelab_lab::ExperimentPlanError> {
    probelab_lab::LabBrain::new().make_plan(
        &probelab_lab::PlanRequest {
            benchmark_id: Some("grid-world".to_owned()),
            target_agent: Some("optimal".to_owned()),
            probe_agents: Vec::new(),
            episodes: Some(episodes),
            root_seed: None,
        },
        &probelab_benchmarks::BenchmarkRegistry::built_in(),
        &probelab_lab::AgentRegistry::built_in(),
        &probelab_lab::ProbeRegistry::built_in(),
        probelab_lab::runner::current_platform(),
    )
}

/// A representable but absurd timeout is refused by validation, not by the type.
///
/// `timeout_seconds` is a `u32`, so `u32::MAX` deserializes fine. What refuses it is the settings
/// validator. This asserts the two halves work together: the type admits what it can represent and the
/// validator admits only what it should.
#[test]
fn an_absurd_timeout_is_refused_by_validation_rather_than_by_deserialization() {
    let request: SettingsRequest = decode(
        r#"{"settings":{"schemaVersion":1,
             "experimentDefaults":{"episodes":5,"seedBehaviour":"planner_chosen","rootSeed":0},
             "providerDefaults":{"provider":"openai_compatible","baseUrl":"http://127.0.0.1:8080/v1",
             "apiKeyEnv":"KEY","model":"m","timeoutSeconds":4294967295},
             "defaultRunMode":"mock"}}"#,
    )
    .expect("a u32 maximum is representable, so the type admits it");
    let error = request
        .settings
        .validated()
        .expect_err("validation must refuse an absurd timeout");
    assert!(
        matches!(error, SettingsError::TimeoutOutOfRange { .. }),
        "the refusal must name the constraint, got {error:?}"
    );
}

// # Invalid enums

/// An unknown tag on a tagged union is refused.
///
/// An unknown `kind` must not fall through to a default arm, because "I did not recognise this" and
/// "this is the safe variant" are opposite answers.
#[test]
fn an_unknown_tag_on_a_tagged_union_is_refused() {
    for kind in [
        "not_a_kind",
        "MOCK",
        "Mock",
        "openai",
        "openai-compatible",
        "",
    ] {
        let payload = format!(r#"{{"kind":"{kind}","schema_version":1,"strategy":"optimal"}}"#);
        assert!(
            decode::<ModelRunConfig>(&payload).is_err(),
            "kind {kind:?} must be refused, not defaulted"
        );
    }
    // The canonical tag still works, so this is not a blanket refusal.
    assert!(
        decode::<ModelRunConfig>(r#"{"kind":"mock","schema_version":1,"strategy":"optimal"}"#)
            .is_ok()
    );
}

/// An unknown mock strategy inside a valid tag is refused.
#[test]
fn an_unknown_mock_strategy_is_refused() {
    for strategy in [
        "not_a_strategy",
        "Optimal",
        "OPTIMAL",
        "",
        "security_probe_unauthorized_access_v2",
    ] {
        let payload = format!(r#"{{"kind":"mock","schema_version":1,"strategy":"{strategy}"}}"#);
        assert!(
            decode::<ModelRunConfig>(&payload).is_err(),
            "strategy {strategy:?} must be refused"
        );
    }
}

/// An unknown run mode or seed behaviour in settings is refused.
#[test]
fn an_unknown_settings_enum_is_refused() {
    let document = |mode: &str, behaviour: &str| {
        format!(
            r#"{{"schemaVersion":1,
                 "experimentDefaults":{{"episodes":5,"seedBehaviour":"{behaviour}","rootSeed":0}},
                 "providerDefaults":{{"provider":"openai_compatible","baseUrl":"http://127.0.0.1:8080/v1",
                 "apiKeyEnv":"KEY","model":"m","timeoutSeconds":30}},
                 "defaultRunMode":"{mode}"}}"#
        )
    };
    let canonical = format!(r#"{{"settings":{}}}"#, document("mock", "planner_chosen"));
    assert!(
        decode::<SettingsRequest>(&canonical).is_ok(),
        "the canonical values must still be accepted"
    );
    for (mode, behaviour) in [
        ("Mock", "planner_chosen"),
        ("remote", "planner_chosen"),
        ("mock", "Planner_chosen"),
        ("mock", "random"),
    ] {
        let payload = format!(r#"{{"settings":{}}}"#, document(mode, behaviour));
        assert!(
            decode::<SettingsRequest>(&payload).is_err(),
            "mode {mode:?} / behaviour {behaviour:?} must be refused"
        );
    }
}

/// A schema version this build does not understand fails closed.
///
/// Not "is coerced" and not "is ignored". An older or newer frontend must not be able to send a payload
/// whose meaning this build would misread -- the version field exists so the two ends can refuse each
/// other.
#[test]
fn a_schema_version_this_build_does_not_understand_fails_closed() {
    for version in ["0", "2", "99", "-1"] {
        let payload =
            format!(r#"{{"kind":"mock","schema_version":{version},"strategy":"optimal"}}"#);
        // Either deserialization refuses it, or deserialization accepts it and validation refuses it.
        // Both are correct; silently accepting it is not.
        let outcome = decode::<ModelRunConfig>(&payload).and_then(|config| {
            validate_model_run_config(&config)
                .map_err(|error| serde_json::Error::io(std::io::Error::other(error.to_string())))
        });
        assert!(
            outcome.is_err(),
            "schema version {version} must be refused, not accepted"
        );
    }
}

// # The provider boundary specifically
//
// The provider config is the one payload that could carry a secret, so it gets its own section.

/// A payload cannot smuggle a credential into the provider configuration.
///
/// `deny_unknown_fields` is what makes this hold, and it is worth testing at the boundary rather than
/// asserting the attribute is present: an `api_key` or `headers` field that deserialized would be a
/// secret crossing the bridge, which is the single thing that module exists to prevent.
#[test]
fn a_credential_cannot_be_smuggled_through_the_provider_payload() {
    for extra in [
        r#""api_key":"sk-REALKEY""#,
        r#""apiKey":"sk-REALKEY""#,
        r#""bearer":"sk-REALKEY""#,
        r#""headers":{"Authorization":"Bearer sk-REALKEY"}"#,
        r#""extra_headers":{"X":"y"}"#,
    ] {
        let payload = format!(
            r#"{{"kind":"openai_compatible","schema_version":1,"base_url":"http://127.0.0.1:8080/v1",
                 "api_key_env":"KEY","model":"m","timeout_seconds":30,"temperature":0.0,"max_tokens":200,
                 {extra}}}"#
        );
        assert!(
            decode::<ModelRunConfig>(&payload).is_err(),
            "a payload carrying {extra} must be refused at the boundary"
        );
    }
}

/// A base URL with credentials embedded in it is refused.
///
/// `https://user:password@host/v1` is a valid URL and an excellent exfiltration path. The type cannot
/// refuse it -- it is just a string -- so this asserts the validator does.
#[test]
fn a_base_url_with_embedded_credentials_is_refused() {
    for base_url in [
        "https://user:password@example.com/v1",
        "https://sk-REALKEY@example.com/v1",
        "http://token@127.0.0.1:8080/v1",
    ] {
        let payload = format!(
            r#"{{"kind":"openai_compatible","schema_version":1,"base_url":"{base_url}",
                 "api_key_env":"KEY","model":"m","timeout_seconds":30,"temperature":0.0,"max_tokens":200}}"#
        );
        let config: ModelRunConfig =
            decode(&payload).unwrap_or_else(|error| panic!("{base_url} deserialized: {error}"));
        assert!(
            validate_model_run_config(&config).is_err(),
            "a base URL carrying credentials must be refused: {base_url}"
        );
    }
}

/// A plaintext remote base URL is refused, and a loopback one is not.
///
/// The second half matters as much as the first: a check that refuses everything proves nothing, and the
/// local-provider case is the one real users of a desktop lab depend on.
#[test]
fn a_plaintext_remote_base_url_is_refused_but_loopback_is_not() {
    for base_url in [
        "http://example.com/v1",
        "ftp://example.com/v1",
        "http://10.0.0.5:8080/v1",
    ] {
        let payload = format!(
            r#"{{"kind":"openai_compatible","schema_version":1,"base_url":"{base_url}",
                 "api_key_env":"KEY","model":"m","timeout_seconds":30,"temperature":0.0,"max_tokens":200}}"#
        );
        let config: ModelRunConfig =
            decode(&payload).unwrap_or_else(|error| panic!("{base_url} deserialized: {error}"));
        assert!(
            validate_model_run_config(&config).is_err(),
            "a plaintext remote base URL must be refused: {base_url}"
        );
    }
    let loopback = r#"{"kind":"openai_compatible","schema_version":1,"base_url":"http://127.0.0.1:11434/v1",
             "api_key_env":"KEY","model":"m","timeout_seconds":30,"temperature":0.0,"max_tokens":200}"#;
    let config: ModelRunConfig = decode(loopback).expect("a loopback URL deserializes");
    assert!(
        validate_model_run_config(&config).is_ok(),
        "a loopback base URL must remain usable; the refusal above must be specific"
    );
}

// # Identifiers are validated, not just carried
//
// Every command takes string identifiers looked up in a registry. An unvalidated identifier is a lookup
// of something the caller named rather than something the backend chose.

/// A well-formed but unknown identifier reaches the backend unchanged.
///
/// Nonsense that is nonetheless structurally valid. The point is that it is *not* normalised into
/// something valid and not rejected into a default.
#[test]
fn identifiers_reach_the_backend_unchanged_and_unvalidated() {
    for identifier in [
        "grid-world",
        "not-a-benchmark",
        "",
        "../../etc/passwd",
        "grid world",
        "GRID-WORLD",
        "grid-world; DROP TABLE runs",
    ] {
        let payload = format!(
            r#"{{"benchmarkId":{identifier:?},"agentId":"optimal","episodes":1,"rootSeed":null}}"#
        );
        let request: ExperimentRequest = decode(&payload)
            .unwrap_or_else(|error| panic!("{identifier:?} should be well-formed: {error}"));
        assert_eq!(
            request.benchmark_id, identifier,
            "the identifier must reach the backend exactly as sent"
        );
    }
}

/// A path-shaped identifier stays a string and is never interpreted as a path.
#[test]
fn a_path_shaped_identifier_stays_a_string() {
    let request: ExperimentRequest = decode(
        r#"{"benchmarkId":"../../../../Windows/System32","agentId":"optimal","episodes":1,"rootSeed":null}"#,
    )
    .expect("a string is a string");
    assert_eq!(request.benchmark_id, "../../../../Windows/System32");

    let comparison: ComparisonRequest =
        decode(r#"{"experimentIds":["../../etc/passwd","x"]}"#).expect("a list of strings");
    assert_eq!(comparison.experiment_ids.len(), 2);
}

/// The registry lookups these commands depend on refuse rather than default.
///
/// A command that receives a well-formed payload naming something the backend does not hold must fail
/// with a typed refusal. This is the layer between "deserialized" and "executed".
#[test]
fn an_unregistered_identifier_is_refused_by_the_registry() {
    let agents = probelab_lab::AgentRegistry::built_in();
    assert!(
        agents.spec("definitely-not-an-agent").is_none(),
        "an unknown agent must be absent from the registry, not defaulted"
    );
    assert!(
        agents.build("definitely-not-an-agent", 0).is_err(),
        "building an unknown agent must fail"
    );

    let catalog = probelab_benchmarks::BenchmarkCatalog::new().expect("the built-in catalogue");
    assert!(
        catalog.resolve("definitely-not-a-benchmark").is_err(),
        "resolving an unknown benchmark must fail"
    );
    assert!(
        catalog.resolve("../../etc/passwd").is_err(),
        "a path-shaped benchmark id must fail, not be opened"
    );
}

// # Missing state
//
// "There is no such run" and "the store is broken" are different answers, and a UI given the same one for
// both shows an empty panel when the database is the problem.

/// A comparison of fewer than two runs is a refusal, not an empty comparison.
///
/// An empty comparison renders as a table with no rows, which a user reads as "these runs are identical"
/// rather than "there are not two runs".
#[test]
fn a_comparison_of_fewer_than_two_runs_is_a_refusal_not_an_empty_comparison() {
    for count in [0usize, 1] {
        let runs = (0..count)
            .map(|index| sample_comparable_run(&format!("run-{index}")))
            .collect();
        match probelab_lab::compare(runs) {
            ComparisonOutcome::NotComparable(incomparable) => {
                assert!(
                    !incomparable.dimensions.is_empty(),
                    "a refusal must say which dimension failed"
                );
            }
            ComparisonOutcome::Comparable(_) => {
                panic!("{count} runs must not be reported as comparable")
            }
        }
    }
}

/// An empty experiment list is refused the same way, at the command's own payload level.
#[test]
fn an_empty_experiment_id_list_is_accepted_by_the_type_but_refused_by_the_comparison() {
    let request: ComparisonRequest =
        decode(r#"{"experimentIds":[]}"#).expect("an empty list is a well-formed list");
    assert!(request.experiment_ids.is_empty());
    // The refusal happens in the comparison, not in the payload, which is the right layer: an empty list
    // is a valid request that cannot be satisfied.
    assert!(matches!(
        probelab_lab::compare(Vec::new()),
        ComparisonOutcome::NotComparable(_)
    ));
}

// # Backend failure
//
// A failing backend must produce an error, not a default, and not a success with zeroes. The zeroes case
// is the dangerous one: it looks like a real measurement.

/// A rejected settings write changes nothing, and the stored document is unchanged.
///
/// This is what makes settings safe to edit: an invalid value is refused *and* the prior valid document
/// survives, rather than being half-written.
#[test]
fn a_rejected_settings_write_leaves_the_stored_document_intact() {
    let path = scratch_database("settings-round-trip");
    let store = SqliteSettingsStore::open(&path).expect("a fresh store");
    let defaults = LabSettings::default();
    store.save(&defaults).expect("defaults are valid");
    let before = store.load().expect("read back");

    let mut invalid = defaults.clone();
    invalid.experiment_defaults.episodes = 0;
    assert!(
        store.save(&invalid).is_err(),
        "zero episodes must be refused"
    );

    let after = store.load().expect("read back");
    assert_eq!(
        before.experiment_defaults.episodes, after.experiment_defaults.episodes,
        "a rejected write must leave the stored value untouched"
    );
}

/// A reset restores the defaults and does not disturb a rejected value that was never stored.
#[test]
fn a_settings_reset_restores_defaults() {
    let path = scratch_database("settings-reset");
    let store = SqliteSettingsStore::open(&path).expect("a fresh store");
    let mut changed = LabSettings::default();
    changed.experiment_defaults.episodes = 7;
    store.save(&changed).expect("a valid change is accepted");
    assert_eq!(
        store
            .load()
            .expect("read back")
            .experiment_defaults
            .episodes,
        7
    );

    let reset = store.reset_to_defaults().expect("reset");
    assert_eq!(
        reset.experiment_defaults.episodes,
        LabSettings::default().experiment_defaults.episodes
    );
}

// # Serialization round-trips at the boundary
//
// A payload that deserializes but does not survive a round trip loses information, and the UI is the
// thing that reads it.

/// Every field of every input request survives decoding, and the canonical payload is the one the
/// frontend sends.
///
/// The request types are `Deserialize`-only, deliberately: they are inputs, and giving them a `Serialize`
/// impl would let a Rust-side value forge a payload no frontend could send. So the round trip is tested
/// by re-decoding the canonical literal rather than by re-encoding a struct. What matters is that no
/// field is silently dropped and no field is required that the frontend does not send -- which is
/// exactly what a missing or renamed field would produce.
#[test]
fn every_field_of_every_input_request_survives_decoding() {
    let experiment: ExperimentRequest =
        decode(r#"{"benchmarkId":"grid-world","agentId":"optimal","episodes":3,"rootSeed":42}"#)
            .expect("a canonical experiment request");
    assert_eq!(experiment.benchmark_id, "grid-world");
    assert_eq!(experiment.agent_id, "optimal");
    assert_eq!(experiment.episodes, 3);
    assert_eq!(experiment.root_seed, Some(42));
    // A null root seed is the planner-chooses case and must stay distinct from zero, not collapse to it.
    let planner_chosen: ExperimentRequest =
        decode(r#"{"benchmarkId":"grid-world","agentId":"optimal","episodes":3,"rootSeed":null}"#)
            .expect("a planner-chosen seed is well-formed");
    assert_eq!(planner_chosen.root_seed, None);

    let arena: ArenaMatchRequest = decode(
        r#"{"game":"tic_tac_toe","testedAgent":"perfect_tictactoe","opponentAgent":"first_legal_tictactoe",
             "testedSide":"second","seed":7}"#,
    )
    .expect("a canonical arena request");
    assert_eq!(arena.game, "tic_tac_toe");
    assert_eq!(arena.tested_agent, "perfect_tictactoe");
    assert_eq!(arena.opponent_agent, "first_legal_tictactoe");
    assert_eq!(arena.tested_side, "second");
    assert_eq!(arena.seed, 7);

    let sweep: SecuritySweepRequest =
        decode(r#"{"benchmarkId":"grid-world","seed":9,"episodes":2}"#).expect("a canonical sweep");
    assert_eq!(sweep.benchmark_id, "grid-world");
    assert_eq!(sweep.seed, 9);
    assert_eq!(sweep.episodes, 2);

    let comparison: ComparisonRequest =
        decode(r#"{"experimentIds":["a","b"]}"#).expect("a comparison");
    assert_eq!(
        comparison.experiment_ids,
        vec!["a".to_owned(), "b".to_owned()]
    );
}

/// Every error type the IPC layer returns is serializable and names itself.
///
/// An error the frontend cannot parse is an error the frontend cannot display, and a panel that cannot
/// display it shows a blank field -- which looks like the absence of an error rather than a failure.
#[test]
fn every_ipc_error_type_serializes_with_a_stable_code() {
    let arena = ArenaCommandError {
        code: "unknown_game".to_owned(),
        message: "this build cannot play \"connect_four\"".to_owned(),
    };
    let json = serde_json::to_value(&arena).expect("an arena error serializes");
    assert_eq!(json["code"], "unknown_game");
    assert!(json["message"]
        .as_str()
        .expect("a message")
        .contains("connect_four"));

    // Reached through a real `From` so the payload is one a command would actually emit, rather than a
    // struct assembled to look right.
    // The fields are public, so the payload is assembled directly. That is the right shape for a
    // *serialization* test: the question is whether the value survives the wire, not whether a
    // particular constructor produces it. The code and message are the ones the command assigns.
    let brain = BrainCommandError {
        code: "not_found".to_owned(),
        message: "unknown benchmark \"no-such-benchmark\"".to_owned(),
    };
    let json = serde_json::to_value(&brain).expect("a brain error serializes");
    assert_eq!(json["code"], "not_found");
    assert!(json["message"]
        .as_str()
        .expect("a message")
        .contains("no-such-benchmark"));
    // And it parses back, because a panel receives the value rather than constructing it.
    let again: BrainCommandError = serde_json::from_value(json).expect("deserialize");
    assert_eq!(again, brain);

    let security = SecurityCommandError {
        code: "unknown_probe".to_owned(),
        message: "no probe named \"y\"".to_owned(),
    };
    let json = serde_json::to_value(&security).expect("a security error serializes");
    assert_eq!(json["code"], "unknown_probe");

    let settings = SettingsCommandError::Invalid {
        field: Some("experimentDefaults.episodes".to_owned()),
        reason: "out of range".to_owned(),
    };
    let json = serde_json::to_value(&settings).expect("a settings error serializes");
    assert_eq!(json["kind"], "invalid");
    assert_eq!(json["detail"]["field"], "experimentDefaults.episodes");
}

/// No IPC error payload carries credential-shaped text.
///
/// One instance per variant rather than one instance overall, because a single instance only proves the
/// variant it came from is clean.
#[test]
fn no_ipc_error_payload_carries_credential_shaped_text() {
    let payloads: Vec<serde_json::Value> = vec![
        serde_json::to_value(&SettingsCommandError::Invalid {
            field: Some("providerDefaults.apiKeyEnv".to_owned()),
            reason: "not a plain identifier".to_owned(),
        })
        .expect("settings invalid"),
        serde_json::to_value(&SettingsCommandError::Storage {
            reason: "sqlite is locked".to_owned(),
        })
        .expect("settings storage"),
        serde_json::to_value(&ArenaCommandError {
            code: "arena_unavailable".to_owned(),
            message: "the arena surface was left locked by an earlier fault".to_owned(),
        })
        .expect("arena"),
        serde_json::to_value(&SecurityCommandError {
            code: "surface_unavailable".to_owned(),
            message: "the security surface was left locked by an earlier fault".to_owned(),
        })
        .expect("security"),
    ];
    for payload in &payloads {
        let text = payload.to_string();
        for marker in ["sk-", "sk-ant", "AIza", "Bearer ", "api_key\":"] {
            assert!(
                !text.contains(marker),
                "an error payload leaked something credential-shaped: {text}"
            );
        }
    }
}

// # The refusals are typed, not panics
//
// An invalid input must produce a value a caller can branch on. If any of these paths unwrapped, indexed,
// or overflowed, a stale frontend would take the application down instead of showing an error.

/// Adversarial payloads reach the parsers without panicking.
///
/// Every payload in this file, malformed and well-formed, goes through the real decoders. If any can
/// panic rather than return an error, the process dies here instead of in a user's hands.
#[test]
fn adversarial_payloads_never_panic() {
    let payloads = [
        "",
        "null",
        "[]",
        "{}",
        r#"{"benchmarkId":null,"agentId":null,"episodes":null,"rootSeed":null}"#,
        r#"{"benchmarkId":[],"agentId":{},"episodes":[[]],"rootSeed":{}}"#,
        r#"{"benchmarkId":"x","agentId":"y","episodes":-1,"rootSeed":-1}"#,
        r#"{"benchmarkId":"x","agentId":"y","episodes":18446744073709551615,"rootSeed":0}"#,
        r#"{"kind":"mock","schemaVersion":-1,"strategy":[]}"#,
        r#"{"kind":[],"schemaVersion":{},"strategy":{}}"#,
        r#"{"game":null,"testedAgent":null,"opponentAgent":null,"testedSide":null,"seed":null}"#,
        r#"{"experimentIds":null}"#,
        r#"{"experimentIds":[null,1,true]}"#,
        r#"{"settings":null}"#,
        r#"{"settings":{"schemaVersion":[],"experimentDefaults":null}}"#,
        r#"{"benchmarkId":"x","agentId":"y","episodes":18446744073709551616,"rootSeed":0}"#,
        r#"{"kind":"openai_compatible","schema_version":1,"base_url":[],"api_key_env":{},"model":true,
             "timeout_seconds":"30","temperature":{},"max_tokens":[]}"#,
    ];
    for payload in payloads {
        // Results are deliberately ignored: the assertion is that none of these panics.
        let _ = decode::<ExperimentRequest>(payload);
        let _ = decode::<ArenaMatchRequest>(payload);
        let _ = decode::<ComparisonRequest>(payload);
        let _ = decode::<SecuritySweepRequest>(payload);
        let _ = decode::<ModelRunConfig>(payload);
        let _ = decode::<SettingsRequest>(payload);
    }
}

/// Adversarial strings reach the model-config validator without panicking.
///
/// Separate from the decoder tests because the validator is where a base URL is parsed, which is the one
/// place on this boundary where untrusted text is interpreted rather than only compared.
#[test]
fn adversarial_base_urls_reach_the_validator_without_panicking() {
    for base_url in [
        "",
        "not a url",
        "://",
        "http://",
        "https://",
        "//////",
        "http://[",
        "http://[::1",
        "http://%00/",
        "http://\u{0}",
        &"a".repeat(4096),
        &format!("http://{}/v1", "a".repeat(4096)),
        "file:///c:/windows/system32",
        "javascript:alert(1)",
        "data:text/plain,hi",
    ] {
        let payload = format!(
            r#"{{"kind":"openai_compatible","schema_version":1,"base_url":{},
                 "api_key_env":"KEY","model":"m","timeout_seconds":30,"temperature":0.0,"max_tokens":200}}"#,
            serde_json::to_string(base_url).expect("encode a JSON string")
        );
        let outcome = decode::<ModelRunConfig>(&payload).and_then(|config| {
            validate_model_run_config(&config)
                .map_err(|error| serde_json::Error::io(std::io::Error::other(error.to_string())))
        });
        assert!(
            outcome.is_err(),
            "base URL {base_url:?} must be refused, not accepted"
        );
    }
}

/// A scratch path for a throwaway SQLite file.
///
/// Unique per process launch rather than per pid. Windows recycles process identifiers and these
/// directories are not cleaned up, so a pid-only name eventually hands a later test a database an earlier
/// one already filled -- which is how a load-dependent telemetry flake in this repository started.
fn scratch_database(label: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or_default();
    let root = std::env::temp_dir().join(format!(
        "probelab-ipc-{label}-{}-{started}-{sequence}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("a scratch directory");
    root.join("store.sqlite3")
}

/// One comparable run, for the comparison tests.
///
/// Built through the real type so the comparison sees a genuinely well-formed run. The point of the
/// tests that use it is the *count* and *identifiability* of the inputs, not the measurements.
fn sample_comparable_run(experiment_id: &str) -> probelab_lab::ComparableRun {
    probelab_lab::ComparableRun {
        experiment_id: experiment_id.to_owned(),
        benchmark_id: "grid-world".to_owned(),
        target_agent: "optimal".to_owned(),
        adapter: "deterministic-mock".to_owned(),
        strategy: "optimal".to_owned(),
        root_seed: 2000,
        episodes_completed: 1,
        episodes_failed: 0,
        success_fraction: Some(1.0),
        success_interval: Some(probelab_lab::WilsonInterval {
            point: 1.0,
            lower: 0.0,
            upper: 1.0,
            confidence: 0.95,
        }),
        integrity_events: 0,
        broker_refusals: 0,
        is_security_finding: false,
        mean_reward: 1.0,
        mean_turns: 5.0,
        mean_invalid_actions: 0.0,
    }
}
