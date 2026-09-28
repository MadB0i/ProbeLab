use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use probelab_agent::AsyncProviderAgent;
use probelab_benchmarks::GridWorldBenchmark;
use probelab_core::{
    Action, Agent, Benchmark, BenchmarkSummary, CompletedRun, CompletionStatus, Episode,
    EpisodeStatus, EvaluationInput, ModelCallFailure, ModelCallFailureKind, ModelCallInfo,
    ModelDecision, ModelDescriptor, ModelRequest, ModelResponse, Observation, ProtocolAdherence,
    RuntimeAuthorization, Sandbox, StepOutcome, TelemetryEvent, TelemetryRecorder, TokenUsage,
};
use probelab_engine::BenchmarkRunner;
use probelab_evaluator::GridWorldEvaluator;
use probelab_model_adapters::{MockModelConfig, OpenAiCompatibleConfig, OpenAiCompatibleProvider};
use probelab_sandbox::GridWorldSandbox;
use probelab_telemetry::SqliteResultRepository;
use rusqlite::Connection;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct MemoryRecorder {
    events: Arc<Mutex<Vec<TelemetryEvent>>>,
}

impl TelemetryRecorder for MemoryRecorder {
    fn record(&self, event: &TelemetryEvent) -> Result<(), String> {
        self.events
            .lock()
            .expect("lock memory telemetry")
            .push(event.clone());
        Ok(())
    }
}

struct ScriptedAgent {
    responses: VecDeque<Result<ModelResponse, ModelCallFailure>>,
    fallback: ModelResponse,
}

impl ScriptedAgent {
    fn new(responses: Vec<Result<ModelResponse, ModelCallFailure>>) -> Self {
        Self {
            responses: responses.into(),
            fallback: valid_response(None, None),
        }
    }
}

impl Agent for ScriptedAgent {
    fn descriptor(&self) -> ModelDescriptor {
        let mut parameters = std::collections::BTreeMap::new();
        parameters.insert(
            "provider".to_owned(),
            serde_json::Value::String("scripted".to_owned()),
        );
        parameters.insert(
            "model".to_owned(),
            serde_json::Value::String("scripted-model".to_owned()),
        );
        parameters.insert(
            "prompt_template_version".to_owned(),
            serde_json::Value::String("scripted-v1".to_owned()),
        );
        ModelDescriptor {
            adapter: "scripted-test-v1".to_owned(),
            strategy: "scripted".to_owned(),
            seed: 7,
            parameters,
        }
    }

    fn decide(&mut self, _request: ModelRequest) -> Result<ModelDecision, String> {
        Err("scripted agents must use decide_with_metadata".to_owned())
    }

    fn decide_with_metadata(
        &mut self,
        _request: ModelRequest,
    ) -> Result<ModelResponse, ModelCallFailure> {
        self.responses
            .pop_front()
            .unwrap_or_else(|| Ok(self.fallback.clone()))
    }
}

fn valid_response(usage: Option<TokenUsage>, response_text: Option<String>) -> ModelResponse {
    ModelResponse {
        decision: ModelDecision::GridAction(Action::Wait),
        call: Some(ModelCallInfo {
            provider: "scripted".to_owned(),
            model: "scripted-model".to_owned(),
            prompt_template_version: "scripted-v1".to_owned(),
            request_id: None,
            finish_reason: Some("stop".to_owned()),
            usage,
            timeout_seconds: Some(5),
            elapsed_ms: 1,
            response_text,
        }),
    }
}

fn failure(kind: ModelCallFailureKind, usage: Option<TokenUsage>) -> ModelCallFailure {
    ModelCallFailure {
        kind,
        message: "scripted failure".to_owned(),
        response_text: None,
        usage,
        elapsed_ms: Some(1),
    }
}

fn usage(prompt: u64, completion: u64, total: u64) -> TokenUsage {
    TokenUsage {
        prompt_tokens: Some(prompt),
        completion_tokens: Some(completion),
        total_tokens: Some(total),
    }
}

fn run_scripted(
    responses: Vec<Result<ModelResponse, ModelCallFailure>>,
    benchmark: &dyn Benchmark,
) -> (CompletedRun, Vec<TelemetryEvent>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let recorder: Arc<dyn TelemetryRecorder> = Arc::new(MemoryRecorder {
        events: Arc::clone(&events),
    });
    let completed = BenchmarkRunner::new()
        .run_with_metadata(
            benchmark,
            Box::new(ScriptedAgent::new(responses)),
            &GridWorldEvaluator::new(),
            SCRIPT_SEED,
            "scripted-run",
            recorder,
        )
        .expect("scripted run");
    let observed = events.lock().expect("read memory telemetry").clone();
    (completed, observed)
}

/// The seed every scripted protocol case runs on.
///
/// A scripted agent that always answers `WAIT` runs until the episode's budget is spent, so these
/// tests are really about "the episode lasted exactly as long as the seed's budget says". Pinned to a
/// dev-pool seed and derived, never hardcoded, so the assertions keep meaning if the budget policy
/// changes.
const SCRIPT_SEED: u64 = 1_000;

/// The turn budget `SCRIPT_SEED`'s episode runs with.
fn script_budget() -> u32 {
    GridWorldBenchmark::built_in()
        .expect("load built-in benchmark")
        .definition()
        .max_turns_for(SCRIPT_SEED)
}

fn built_in_benchmark() -> GridWorldBenchmark {
    GridWorldBenchmark::built_in().expect("load built-in benchmark")
}

#[test]
fn malformed_responses_recover_before_the_three_strike_threshold() {
    let benchmark = built_in_benchmark();
    let (completed, events) = run_scripted(
        vec![
            Err(failure(ModelCallFailureKind::MalformedResponse, None)),
            Err(failure(ModelCallFailureKind::EmptyResponse, None)),
            Ok(valid_response(None, None)),
        ],
        &benchmark,
    );

    assert_ne!(
        completed.result.capability.completion_status,
        CompletionStatus::ModelProtocolFailure
    );
    assert_eq!(
        completed
            .result
            .capability
            .protocol_adherence
            .parse_failures,
        2
    );
    assert_eq!(
        completed
            .result
            .capability
            .protocol_adherence
            .valid_responses,
        u64::from(script_budget()) - 2
    );
    assert_eq!(
        completed
            .result
            .capability
            .protocol_adherence
            .total_model_calls,
        u64::from(script_budget())
    );
    assert_eq!(events.len(), script_budget() as usize);
    assert_eq!(
        events[0].model_failure.as_ref().map(|failure| failure.kind),
        Some(ModelCallFailureKind::MalformedResponse)
    );
    assert_eq!(events[2].selected_action, Some(Action::Wait));
    assert!(completed.metadata.action_trace[0]
        .model_failure_kind
        .is_some());
    assert!(completed.metadata.action_trace[2]
        .model_failure_kind
        .is_none());
}

#[test]
fn three_consecutive_typed_failures_terminate_without_integrity_events() {
    let benchmark = built_in_benchmark();
    let (completed, events) = run_scripted(
        vec![
            Err(failure(ModelCallFailureKind::MalformedResponse, None)),
            Err(failure(ModelCallFailureKind::Timeout, None)),
            Err(failure(ModelCallFailureKind::ProviderError, None)),
        ],
        &benchmark,
    );

    assert_eq!(
        completed.result.capability.completion_status,
        CompletionStatus::ModelProtocolFailure
    );
    assert_eq!(completed.result.capability.turns_used, 3);
    assert_eq!(events.len(), 3);
    assert_eq!(completed.result.integrity.total_events(), 0);
    assert_eq!(
        completed
            .result
            .capability
            .protocol_adherence
            .parse_failures,
        1
    );
    assert_eq!(completed.result.capability.protocol_adherence.timeouts, 1);
    assert_eq!(
        completed
            .result
            .capability
            .protocol_adherence
            .provider_failures,
        1
    );
    assert_eq!(completed.metadata.action_trace.len(), 3);
}

struct RestrictedBenchmark {
    max_turns: u32,
}

impl Benchmark for RestrictedBenchmark {
    fn summary(&self) -> BenchmarkSummary {
        // `..Default::default()` rather than a full literal. This is a test double, not the real
        // grid-world benchmark, so it has no embedded definition to hash and no honest digest to
        // report -- and saying so with an empty field is the point. A fabricated digest here would be
        // a test asserting against an identity this benchmark does not have.
        BenchmarkSummary {
            benchmark_id: "grid-world".to_owned(),
            benchmark_version: probelab_evaluator::GRID_WORLD_BENCHMARK_VERSION.to_owned(),
            title: "test".to_owned(),
            description: "test".to_owned(),
            capability_dimension: "grid-world planning".to_owned(),
            seed: 123,
            max_turns: self.max_turns,
            metadata: Default::default(),
            ..BenchmarkSummary::default()
        }
    }

    fn create_sandbox(&self, seed: u64) -> Result<Box<dyn Sandbox>, String> {
        Ok(Box::new(RestrictedSandbox {
            inner: GridWorldSandbox::with_max_turns(seed, self.max_turns)
                .expect("restricted sandbox"),
        }))
    }
}

struct RestrictedSandbox {
    inner: GridWorldSandbox,
}

impl Sandbox for RestrictedSandbox {
    fn observation(&self) -> Observation {
        let mut observation = self.inner.observation();
        observation.available_actions = vec![Action::Wait];
        observation
    }

    fn available_actions(&self) -> Vec<Action> {
        vec![Action::Wait]
    }

    fn execute(&mut self, authorization: RuntimeAuthorization) -> Result<StepOutcome, String> {
        match authorization {
            RuntimeAuthorization::Execute(Action::Wait) => self.inner.execute(authorization),
            RuntimeAuthorization::Execute(action) => Err(format!("restricted action: {action:?}")),
            RuntimeAuthorization::Reject(event) => {
                Err(format!("restricted authorization: {event:?}"))
            }
        }
    }

    fn consume_capability_turn(&mut self) -> Result<StepOutcome, String> {
        self.consume_denied_turn(probelab_core::IntegrityEventKind::PolicyViolation)
    }

    fn consume_denied_turn(
        &mut self,
        event: probelab_core::IntegrityEventKind,
    ) -> Result<StepOutcome, String> {
        self.inner.consume_denied_turn(event)
    }

    fn consume_protocol_failure_turn(&mut self, terminate: bool) -> Result<StepOutcome, String> {
        self.inner.consume_protocol_failure_turn(terminate)
    }

    fn episode(&self) -> Episode {
        self.inner.episode()
    }
}

#[test]
fn action_vocabulary_rejection_is_protocol_failure_not_environment_invalid_action() {
    let responses = (0..3)
        .map(|_| {
            Ok(ModelResponse {
                decision: ModelDecision::GridAction(Action::Right),
                call: None,
            })
        })
        .collect();
    let (completed, events) = run_scripted(responses, &RestrictedBenchmark { max_turns: 3 });

    assert_eq!(
        completed.result.capability.completion_status,
        CompletionStatus::ModelProtocolFailure
    );
    assert_eq!(completed.result.capability.invalid_action_count, 0);
    assert_eq!(completed.result.integrity.total_events(), 0);
    assert_eq!(
        completed
            .result
            .capability
            .protocol_adherence
            .invalid_actions,
        3
    );
    assert!(events.iter().all(|event| event.model_failure.is_some()));
    assert!(events.iter().all(|event| event.action_outcome.is_none()));
    assert_eq!(
        completed.metadata.action_trace[0].selected_action,
        Some(Action::Right)
    );
    assert_eq!(
        completed.metadata.action_trace[0].parsed_action,
        Some(Action::Right)
    );
    assert_eq!(completed.metadata.action_trace[0].action_result_kind, None);
}

#[test]
fn successful_and_failed_token_usage_is_aggregated() {
    let benchmark = built_in_benchmark();
    let (completed, _) = run_scripted(
        vec![
            Err(failure(
                ModelCallFailureKind::MalformedResponse,
                Some(usage(1, 2, 3)),
            )),
            Ok(valid_response(Some(usage(10, 20, 30)), None)),
            Ok(valid_response(Some(usage(100, 200, 300)), None)),
            Ok(valid_response(None, None)),
        ],
        &benchmark,
    );

    assert_eq!(
        completed.result.reproducibility.token_usage,
        usage(111, 222, 333)
    );
    assert_eq!(completed.metadata.token_usage, usage(111, 222, 333));
    assert_eq!(
        completed
            .result
            .capability
            .protocol_adherence
            .total_model_calls,
        u64::from(script_budget())
    );
}

#[test]
fn completed_metadata_round_trips_and_keeps_secret_fields_out_of_storage() {
    let path = std::env::temp_dir().join(format!(
        "probelab-engine-completed-{}-{}.sqlite",
        std::process::id(),
        completed_metadata_path_id()
    ));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));

    let benchmark = built_in_benchmark();
    let (completed, events) = run_scripted(
        vec![Ok(valid_response(
            None,
            Some("{\"action\":\"WAIT\"}".to_owned()),
        ))],
        &benchmark,
    );
    let repository = SqliteResultRepository::new(&path).expect("open result repository");
    repository
        .store_completed("metadata-run", &completed)
        .expect("store completed run");
    assert_eq!(
        repository
            .get("metadata-run")
            .expect("read result")
            .expect("stored result"),
        completed.result
    );
    assert_eq!(
        repository
            .get_metadata("metadata-run")
            .expect("read metadata")
            .expect("stored metadata"),
        completed.metadata
    );

    let connection = Connection::open(&path).expect("open stored result database");
    let (result_json, metadata_json): (String, Option<String>) = connection
        .query_row(
            "SELECT result_json, metadata_json FROM results WHERE run_id = ?1",
            ["metadata-run"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("read stored JSON");
    let stored = format!("{result_json}{}", metadata_json.unwrap_or_default());
    assert!(!stored.contains("api_key"));
    assert!(!stored.contains("OPENAI"));
    assert!(events.iter().all(|event| {
        !serde_json::to_string(event)
            .expect("serialize event")
            .contains("api_key")
    }));

    drop(connection);
    drop(repository);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
}

static NEXT_METADATA_PATH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn completed_metadata_path_id() -> u64 {
    NEXT_METADATA_PATH.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

#[test]
fn openai_metadata_and_telemetry_redact_provider_credentials() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("create wiremock runtime");
    let server = runtime.block_on(async { MockServer::start().await });
    let secret = "fake-engine-secret";
    runtime.block_on(async {
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{
                    "message": {
                        "content": format!(r#"{{"action":"WAIT"}} {secret}"#)
                    },
                    "finish_reason": "stop"
                }],
                "usage": {
                    "prompt_tokens": 1,
                    "completion_tokens": 1,
                    "total_tokens": 2
                }
            })))
            .mount(&server)
            .await;
    });
    drop(runtime);

    let env_name = "PROBELAB_ENGINE_SECRET_KEY";
    std::env::set_var(env_name, secret);
    let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        base_url: format!("{}/v1", server.uri()),
        api_key_env: env_name.to_owned(),
        ..OpenAiCompatibleConfig::default()
    })
    .expect("create OpenAI-compatible provider");
    let agent = AsyncProviderAgent::from_provider(provider).expect("create async agent");
    let events = Arc::new(Mutex::new(Vec::new()));
    let recorder: Arc<dyn TelemetryRecorder> = Arc::new(MemoryRecorder {
        events: Arc::clone(&events),
    });
    let completed = BenchmarkRunner::new()
        .run_with_metadata(
            &built_in_benchmark(),
            Box::new(agent),
            &GridWorldEvaluator::new(),
            SCRIPT_SEED,
            "openai-secret-run",
            recorder,
        )
        .expect("OpenAI-compatible run");
    std::env::remove_var(env_name);

    let result_json = serde_json::to_string(&completed.result).expect("serialize result");
    let metadata_json = serde_json::to_string(&completed.metadata).expect("serialize metadata");
    let event_json = events
        .lock()
        .expect("read OpenAI telemetry")
        .iter()
        .map(|event| serde_json::to_string(event).expect("serialize telemetry event"))
        .collect::<Vec<_>>()
        .join("");
    assert!(!result_json.contains(secret));
    assert!(!metadata_json.contains(secret));
    assert!(!event_json.contains(secret));
    assert!(!result_json.contains(env_name));
    assert!(!metadata_json.contains(env_name));
    assert!(!event_json.contains(env_name));
    assert_eq!(
        completed.result.reproducibility.provider,
        "openai-compatible"
    );
    assert_eq!(
        completed.result.reproducibility.prompt_template_version,
        "grid_world_v2"
    );
    // The scripted provider reports one prompt and one completion token per call, and the call count
    // is the episode's turn budget, so the aggregate is derived rather than a literal that would rot
    // the moment the budget policy changed.
    let calls = u64::from(script_budget());
    assert_eq!(
        completed.result.reproducibility.token_usage,
        usage(calls, calls, calls * 2)
    );
    assert!(events
        .lock()
        .expect("read OpenAI telemetry")
        .iter()
        .all(|event| event.model_call.is_some()));
}

#[test]
fn mock_results_remain_field_identical_for_the_same_seed() {
    let first = run_with_mock("mock-first");
    let second = run_with_mock("mock-second");
    assert_eq!(first, second);
}

fn run_with_mock(run_id: &str) -> probelab_core::TestResult {
    let events = Arc::new(Mutex::new(Vec::new()));
    let recorder: Arc<dyn TelemetryRecorder> = Arc::new(MemoryRecorder { events });
    let benchmark = built_in_benchmark();
    BenchmarkRunner::new()
        .run(
            &benchmark,
            Box::new(probelab_agent::ProviderAgent::from_provider(
                probelab_model_adapters::MockModelProvider::new(MockModelConfig::Optimal, 9),
            )),
            &GridWorldEvaluator::new(),
            SCRIPT_SEED,
            run_id,
            recorder,
        )
        .expect("mock run")
}

#[test]
fn evaluator_keeps_protocol_failures_outside_integrity() {
    let mut input = EvaluationInput {
        benchmark_id: "grid-world".to_owned(),
        benchmark_version: probelab_evaluator::GRID_WORLD_BENCHMARK_VERSION.to_owned(),
        capability_dimension: "grid-world planning".to_owned(),
        model: ModelDescriptor {
            adapter: "test".to_owned(),
            strategy: "test".to_owned(),
            seed: 0,
            parameters: Default::default(),
        },
        seed: 123,
        episode: Episode {
            episode_id: "episode".to_owned(),
            seed: 123,
            status: EpisodeStatus::ModelProtocolFailure,
            turns_used: 3,
            total_reward: 0,
            invalid_action_count: 0,
        },
        integrity_events: Vec::new(),
        broker_denials: Vec::new(),
        process_lifecycles: Vec::new(),
        telemetry_event_count: 3,
        replay_digest: "a".repeat(64),
        protocol_adherence: ProtocolAdherence::default(),
        reproducibility: Default::default(),
    };
    input
        .protocol_adherence
        .record_failure(ModelCallFailureKind::MalformedResponse);
    input
        .protocol_adherence
        .record_failure(ModelCallFailureKind::MalformedResponse);
    input
        .protocol_adherence
        .record_failure(ModelCallFailureKind::MalformedResponse);
    let result = GridWorldEvaluator::new()
        .evaluate(input)
        .expect("evaluate protocol failure");
    assert_eq!(result.capability.protocol_adherence.parse_failures, 3);
    assert!(result.integrity.is_compliant());
}
