//! End-to-end checks for the v0.3 capability broker inside the trusted engine run context.
//!
//! These tests drive the engine directly with a scripted internal agent. No real model provider
//! is involved, and no capability is ever executed: an authorized request must stop the run with
//! an explicit unsupported-execution error instead of touching the host.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use probelab_agent::ProviderAgent;
use probelab_benchmarks::GridWorldBenchmark;
use probelab_core::{
    Action, Agent, Benchmark, BenchmarkSummary, CapabilityPolicy, CapabilityRequest, CompletedRun,
    IntegrityEventKind, ModelDecision, ModelDescriptor, ModelRequest, Sandbox, TelemetryEvent,
    TelemetryRecorder,
};
use probelab_engine::{BenchmarkRunner, EngineError, RunContext};
use probelab_evaluator::GridWorldEvaluator;
use probelab_model_adapters::{MockModelConfig, MockModelProvider};
use probelab_sandbox::GridWorldSandbox;

static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);

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

/// A grid-world benchmark that declares a non-default capability policy.
struct GrantingBenchmark {
    policy: CapabilityPolicy,
    max_turns: u32,
}

impl Benchmark for GrantingBenchmark {
    fn summary(&self) -> BenchmarkSummary {
        BenchmarkSummary {
            max_turns: self.max_turns,
            ..GridWorldBenchmark::built_in()
                .expect("built-in benchmark")
                .summary()
        }
    }

    fn capability_policy(&self) -> CapabilityPolicy {
        self.policy.clone()
    }

    fn create_sandbox(&self, seed: u64) -> Result<Box<dyn Sandbox>, String> {
        Ok(Box::new(
            GridWorldSandbox::with_max_turns(seed, self.max_turns).expect("sandbox"),
        ))
    }
}

/// An agent that replays a fixed decision list and then always waits.
struct ScriptedAgent {
    decisions: VecDeque<ModelDecision>,
}

impl ScriptedAgent {
    fn new(decisions: Vec<ModelDecision>) -> Self {
        Self {
            decisions: decisions.into(),
        }
    }
}

impl Agent for ScriptedAgent {
    fn descriptor(&self) -> ModelDescriptor {
        ModelDescriptor {
            adapter: "scripted-broker-test-v1".to_owned(),
            strategy: "scripted".to_owned(),
            seed: 0,
            parameters: Default::default(),
        }
    }

    fn decide(&mut self, _request: ModelRequest) -> Result<ModelDecision, String> {
        Ok(self
            .decisions
            .pop_front()
            .unwrap_or(ModelDecision::GridAction(Action::Wait)))
    }
}

fn probe_read() -> ModelDecision {
    ModelDecision::CapabilityProbe(CapabilityRequest::FilesystemRead {
        path: PathBuf::from("notes.txt"),
    })
}

fn probe_write() -> ModelDecision {
    ModelDecision::CapabilityProbe(CapabilityRequest::FilesystemWrite {
        path: PathBuf::from("notes.txt"),
    })
}

fn probe_environment() -> ModelDecision {
    ModelDecision::CapabilityProbe(CapabilityRequest::EnvironmentInspect {
        variable: "PROBELAB_BROKER_TEST".to_owned(),
    })
}

fn temporary_root(label: &str) -> PathBuf {
    let sequence = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "probelab-engine-broker-{label}-{}-{sequence}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create run sandbox root");
    std::fs::canonicalize(&root).expect("canonical run sandbox root")
}

fn remove_root(root: &Path) {
    let _ = std::fs::remove_dir_all(root);
}

/// The seed the broker cases run on, and the turn budget its episode therefore gets.
///
/// A security probe refuses every turn, so the episode runs until the budget is spent. Derived from
/// the seed rather than hardcoded, so the assertions below keep meaning.
const PROBE_SEED: u64 = 1_000;

fn probe_budget() -> u32 {
    GridWorldBenchmark::built_in()
        .expect("load built-in benchmark")
        .definition()
        .max_turns_for(PROBE_SEED)
}

fn run(
    benchmark: &dyn Benchmark,
    agent: Box<dyn Agent>,
    root: &Path,
) -> Result<(CompletedRun, Vec<TelemetryEvent>), EngineError> {
    let events = Arc::new(Mutex::new(Vec::new()));
    let recorder: Arc<dyn TelemetryRecorder> = Arc::new(MemoryRecorder {
        events: Arc::clone(&events),
    });
    let completed = BenchmarkRunner::new().run_with_context(
        benchmark,
        agent,
        &GridWorldEvaluator::new(),
        PROBE_SEED,
        "broker-run",
        recorder,
        RunContext::with_sandbox_root(root)?,
    )?;
    let observed = events.lock().expect("read memory telemetry").clone();
    Ok((completed, observed))
}

#[test]
fn an_authorized_capability_stops_the_run_without_executing_anything() {
    let root = temporary_root("authorized");
    let mut policy = CapabilityPolicy::deny_all();
    policy.grant_filesystem_root(&root).expect("grant run root");
    let benchmark = GrantingBenchmark {
        policy,
        max_turns: 4,
    };

    let error = run(
        &benchmark,
        Box::new(ScriptedAgent::new(vec![probe_read()])),
        &root,
    )
    .expect_err("an authorized capability must not silently execute");
    remove_root(&root);
    match error {
        EngineError::CapabilityExecutionUnsupported { permission } => assert_eq!(
            permission,
            probelab_core::CapabilityPermission::FilesystemRead
        ),
        other => panic!("unexpected engine error: {other:?}"),
    }
    // Authorization performed no filesystem work.
    assert!(!root.join("notes.txt").exists());
}

#[test]
fn a_read_only_run_policy_authorizes_reads_but_refuses_writes() {
    let root = temporary_root("read-only-run");
    let mut policy = CapabilityPolicy::deny_all();
    policy
        .grant_filesystem_read_root(&root)
        .expect("grant read root");
    let benchmark = GrantingBenchmark {
        policy,
        max_turns: 2,
    };

    // The read is authorized for this run, so the run stops without executing anything.
    let error = run(
        &benchmark,
        Box::new(ScriptedAgent::new(vec![probe_read()])),
        &root,
    )
    .expect_err("an authorized capability must not silently execute");
    assert!(matches!(
        error,
        EngineError::CapabilityExecutionUnsupported {
            permission: probelab_core::CapabilityPermission::FilesystemRead
        }
    ));
    assert!(!root.join("notes.txt").exists());

    // The same per-run policy refuses the write direction, even for the very root it grants for
    // reading.
    let decisions = vec![probe_write(), ModelDecision::GridAction(Action::Wait)];
    let (completed, events) =
        run(&benchmark, Box::new(ScriptedAgent::new(decisions)), &root).expect("run completes");
    remove_root(&root);
    assert_eq!(events.len(), 2);
    assert_eq!(
        events[0].broker_denial.as_ref().map(|d| d.permission),
        Some(probelab_core::CapabilityPermission::FilesystemWrite)
    );
    assert_eq!(
        events[0].integrity_event,
        Some(IntegrityEventKind::BrokerUnauthorizedWrite)
    );
    assert_eq!(events[1].selected_action, Some(Action::Wait));
    assert_eq!(completed.result.integrity.broker_unauthorized_reads, 0);
    // A broker write refusal must not be counted as a policy-gate sandbox violation.
    assert_eq!(completed.result.integrity.sandbox_violations, 0);
    assert_eq!(completed.result.integrity.broker_unauthorized_writes, 1);
    assert!(!root.join("notes.txt").exists());
}

#[test]
fn a_write_only_run_policy_refuses_reads_and_authorizes_writes() {
    let root = temporary_root("write-only-run");
    let mut policy = CapabilityPolicy::deny_all();
    policy
        .grant_filesystem_write_root(&root)
        .expect("grant write root");
    let benchmark = GrantingBenchmark {
        policy,
        max_turns: 2,
    };

    // The read is refused even though the write grant covers the run root itself.
    let decisions = vec![probe_read(), ModelDecision::GridAction(Action::Wait)];
    let (completed, events) =
        run(&benchmark, Box::new(ScriptedAgent::new(decisions)), &root).expect("run completes");
    assert_eq!(events.len(), 2);
    assert_eq!(
        events[0].broker_denial.as_ref().map(|d| d.permission),
        Some(probelab_core::CapabilityPermission::FilesystemRead)
    );
    assert_eq!(
        events[0].integrity_event,
        Some(IntegrityEventKind::BrokerUnauthorizedRead)
    );
    assert_eq!(events[1].selected_action, Some(Action::Wait));
    assert_eq!(completed.result.integrity.broker_unauthorized_reads, 1);
    assert_eq!(completed.result.integrity.sandbox_violations, 0);

    // A write under the same per-run policy is authorized, and still executes nothing.
    let error = run(
        &benchmark,
        Box::new(ScriptedAgent::new(vec![probe_write()])),
        &root,
    )
    .expect_err("an authorized capability must not silently execute");
    remove_root(&root);
    assert!(matches!(
        error,
        EngineError::CapabilityExecutionUnsupported {
            permission: probelab_core::CapabilityPermission::FilesystemWrite
        }
    ));
    assert!(!root.join("notes.txt").exists());
}

#[test]
fn a_grant_outside_the_run_sandbox_root_fails_run_construction() {
    let root = temporary_root("outside-grant");
    let mut policy = CapabilityPolicy::deny_all();
    policy
        .grant_filesystem_root(std::env::temp_dir())
        .expect("declare wide grant");
    let benchmark = GrantingBenchmark {
        policy,
        max_turns: 2,
    };

    let error = run(
        &benchmark,
        Box::new(ScriptedAgent::new(vec![probe_read()])),
        &root,
    )
    .expect_err("a grant outside the run root must not be accepted");
    remove_root(&root);
    assert!(matches!(error, EngineError::CapabilityBroker(_)));
}

#[test]
fn broker_denials_never_reach_the_policy_gate_or_move_the_grid() {
    let root = temporary_root("deny-turns");
    let benchmark = GrantingBenchmark {
        policy: CapabilityPolicy::deny_all(),
        max_turns: 6,
    };
    let decisions = vec![
        probe_read(),
        ModelDecision::GridAction(Action::Wait),
        probe_environment(),
        ModelDecision::GridAction(Action::Wait),
    ];
    let (completed, events) =
        run(&benchmark, Box::new(ScriptedAgent::new(decisions)), &root).expect("run completes");
    remove_root(&root);

    assert_eq!(events.len(), 6);
    assert_eq!(
        events[0].broker_denial.as_ref().map(|d| d.permission),
        Some(probelab_core::CapabilityPermission::FilesystemRead)
    );
    assert_eq!(
        events[0].integrity_event,
        Some(IntegrityEventKind::BrokerUnauthorizedRead)
    );
    assert_eq!(
        events[1].selected_action,
        Some(Action::Wait),
        "an ordinary action must still execute through the trusted path"
    );
    assert!(events[1].action_outcome.is_some());
    assert_eq!(
        events[2].broker_denial.as_ref().map(|d| d.permission),
        Some(probelab_core::CapabilityPermission::EnvironmentInspect)
    );
    assert_eq!(
        events[2].integrity_event,
        Some(IntegrityEventKind::BrokerEnvironmentInspectDenied)
    );
    assert_eq!(events[3].selected_action, Some(Action::Wait));

    // Denials are integrity events, never protocol failures and never invalid actions.
    assert_eq!(completed.result.capability.invalid_action_count, 0);
    let protocol = &completed.result.capability.protocol_adherence;
    assert_eq!(protocol.total_model_calls, 6);
    assert_eq!(protocol.valid_responses, 6);
    assert_eq!(protocol.invalid_actions, 0);
    assert_eq!(protocol.parse_failures, 0);
    assert_eq!(protocol.valid_response_rate_basis_points, 10_000);
    assert_eq!(completed.result.integrity.broker_unauthorized_reads, 1);
    assert_eq!(
        completed
            .result
            .integrity
            .broker_environment_inspect_denials,
        1
    );
    assert_eq!(completed.result.integrity.total_events(), 2);
    assert!(completed.result.integrity.policy_violations == 0);
    assert!(completed.result.integrity.sandbox_violations == 0);
}

#[test]
fn the_run_context_owns_an_ephemeral_root_and_keeps_it_out_of_the_model() {
    let context = RunContext::ephemeral("ephemeral-root/run id").expect("ephemeral run context");
    let root = context.sandbox_root().to_path_buf();
    assert!(root.is_absolute());
    assert!(root.is_dir());
    assert!(
        !root.to_string_lossy().contains("run id"),
        "the run identifier must not reach the filesystem path"
    );
    drop(context);
    assert!(!root.exists(), "an owned sandbox root is removed on drop");

    assert!(RunContext::with_sandbox_root("relative/root").is_err());
    assert!(RunContext::with_sandbox_root("").is_err());
    let file = temporary_root("not-a-directory").join("file");
    std::fs::write(&file, "file").expect("write file");
    assert!(RunContext::with_sandbox_root(&file).is_err());
    remove_root(file.parent().expect("parent"));
}

#[test]
fn a_v0_1_security_probe_still_produces_the_same_policy_and_integrity_behavior() {
    let root = temporary_root("v0-1-probe");
    let benchmark = GridWorldBenchmark::built_in().expect("built-in benchmark");
    let agent: Box<dyn Agent> = Box::new(ProviderAgent::from_provider(MockModelProvider::new(
        MockModelConfig::SecurityProbeSandboxViolation,
        9,
    )));
    let (completed, events) = run(&benchmark, agent, &root).expect("run completes");
    remove_root(&root);

    assert_eq!(events.len(), probe_budget() as usize);
    assert_eq!(
        completed.result.integrity.sandbox_violations,
        probe_budget()
    );
    assert_eq!(completed.result.integrity.total_broker_denials(), 0);
    assert_eq!(completed.result.integrity.policy_violations, 0);
    assert!(events.iter().all(|event| event.broker_denial.is_none()));
    assert!(events.iter().all(|event| {
        event.selected_action.is_none()
            && event.requested_capability.is_some()
            && event.integrity_event == Some(IntegrityEventKind::SandboxViolation)
    }));
}
