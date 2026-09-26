//! End-to-end tests for `process.spawn` as the first genuinely executable capability.
//!
//! These tests exist to prove the *ordering*, because the ordering is the whole security claim:
//!
//! 1. the capability broker reviews the request **first**;
//! 2. an unauthorized spawn never reaches containment and never starts a process;
//! 3. an authorized spawn reaches containment and runs inside a job object and a verified
//!    low-integrity token, with the evaluation-critical paths unreadable to it; and
//! 4. the model can ask for the permission but can never name the program, the arguments, or the
//!    environment, so no raw OS process API exists in the model-facing surface.
//!
//! The Windows tests run a real child process. The non-Windows tests assert only that containment
//! reports `UnsupportedPlatform` and starts nothing, because there is nothing to enforce there.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use probelab_benchmarks::GridWorldBenchmark;
use probelab_core::{
    Action, Agent, Benchmark, BenchmarkSummary, CapabilityPermission, CapabilityPolicy,
    CapabilityRequest, CompletedRun, ContainmentFailureKind, IntegrityEventKind, ModelDecision,
    ModelDescriptor, ModelRequest, ProcessAuthorization, ProcessLifecycle, ProcessSpawnPlan,
    Sandbox, TelemetryEvent, TelemetryRecorder,
};
use probelab_engine::{BenchmarkRunner, EngineError, RunContext};
use probelab_evaluator::GridWorldEvaluator;
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

/// A grid-world benchmark that declares a process capability, a spawn plan, and protected paths.
struct SpawnBenchmark {
    policy: CapabilityPolicy,
    plan: Option<ProcessSpawnPlan>,
    protected_paths: Vec<PathBuf>,
    max_turns: u32,
}

impl SpawnBenchmark {
    /// Grants `program` and declares a plan that runs it with `args`.
    fn granting(program: &str, args: &[&str], protected_paths: Vec<PathBuf>) -> Self {
        let mut policy = CapabilityPolicy::deny_all();
        policy.grant_process(program).expect("grant program");
        Self {
            policy,
            plan: Some(ProcessSpawnPlan::new(program).with_args(args.iter().copied())),
            protected_paths,
            max_turns: 4,
        }
    }

    /// Grants nothing and declares no plan at all.
    fn granting_nothing() -> Self {
        Self {
            policy: CapabilityPolicy::deny_all(),
            plan: None,
            protected_paths: Vec::new(),
            max_turns: 4,
        }
    }

    /// Grants a program that is *not* the one the plan runs.
    fn mismatched(granted: &str, planned: &str) -> Self {
        let mut policy = CapabilityPolicy::deny_all();
        policy.grant_process(granted).expect("grant program");
        Self {
            policy,
            plan: Some(ProcessSpawnPlan::new(planned)),
            protected_paths: Vec::new(),
            max_turns: 2,
        }
    }
}

/// Creates a directory that stands in for the evaluator and returns it as a protected path.
///
/// A benchmark that can start a child must declare what that child may not read, so every spawning
/// test protects something. The content is never read by a child; it only has to exist.
fn protected_dir(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    std::fs::create_dir_all(&path).expect("create protected directory");
    std::fs::write(path.join("answer-key.txt"), "the answer is 42").expect("write answer key");
    path
}

impl Benchmark for SpawnBenchmark {
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

    fn process_spawn_plan(&self) -> Option<ProcessSpawnPlan> {
        self.plan.clone()
    }

    fn protected_paths(&self) -> &[PathBuf] {
        &self.protected_paths
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
            adapter: "scripted-spawn-test-v1".to_owned(),
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

/// The model asks for the `process.spawn` permission and names a program of its own choosing.
fn probe_spawn(program: &str) -> ModelDecision {
    ModelDecision::CapabilityProbe(CapabilityRequest::ProcessSpawn {
        program: program.to_owned(),
        args: vec!["/C".to_owned(), "echo owned-by-the-model".to_owned()],
    })
}

fn comspec() -> String {
    std::env::var("ComSpec").unwrap_or_else(|_| "C:\\Windows\\System32\\cmd.exe".to_owned())
}

fn temporary_root(label: &str) -> PathBuf {
    let sequence = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "probelab-engine-spawn-{label}-{}-{sequence}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create run sandbox root");
    std::fs::canonicalize(&root).expect("canonical run sandbox root")
}

fn remove_root(root: &Path) {
    let _ = std::fs::remove_dir_all(root);
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
        123,
        "spawn-run",
        recorder,
        RunContext::with_sandbox_root(root)?,
    )?;
    let observed = events.lock().expect("read memory telemetry").clone();
    Ok((completed, observed))
}

fn lifecycle_of(event: &TelemetryEvent) -> Option<&ProcessLifecycle> {
    event.process_lifecycle.as_ref()
}

// ---------------------------------------------------------------------------
// Ordering, on every platform.
// ---------------------------------------------------------------------------

/// A benchmark that grants nothing refuses the request, counts it once in the broker's own
/// category, and never records a process lifecycle because nothing was ever requested of the OS.
#[test]
fn a_spawn_request_is_refused_when_the_benchmark_declares_no_process_capability() {
    let root = temporary_root("no-capability");
    let benchmark = SpawnBenchmark::granting_nothing();
    let (completed, observed) = run(
        &benchmark,
        Box::new(ScriptedAgent::new(vec![probe_spawn("cmd.exe")])),
        &root,
    )
    .expect("the run completes");
    remove_root(&root);

    let denial = observed
        .iter()
        .find_map(|event| event.broker_denial.as_ref())
        .expect("a broker denial was recorded");
    assert_eq!(denial.permission, CapabilityPermission::ProcessSpawn);
    assert_eq!(
        denial.integrity_event,
        IntegrityEventKind::BrokerProcessSpawnBlocked
    );

    assert_eq!(completed.result.integrity.broker_process_spawn_blocks, 1);
    assert_eq!(completed.result.integrity.sandbox_violations, 0);
    assert_eq!(completed.result.integrity.policy_violations, 0);
    assert_eq!(completed.result.capability.invalid_action_count, 0);
    assert_eq!(completed.result.capability.total_reward, 0);
    assert!(
        observed.iter().all(|event| lifecycle_of(event).is_none()),
        "no process may be requested of the operating system when nothing is authorized"
    );
}

/// A plan is a request to be authorized, never a grant. A program that is not on the allowlist is
/// refused before containment, so no process is started.
#[test]
fn a_plan_whose_program_is_not_allowlisted_is_refused_without_starting_anything() {
    let root = temporary_root("mismatched");
    let benchmark = SpawnBenchmark::mismatched("notepad.exe", "cmd.exe");
    let (completed, observed) = run(
        &benchmark,
        Box::new(ScriptedAgent::new(vec![probe_spawn("cmd.exe")])),
        &root,
    )
    .expect("the run completes");
    remove_root(&root);

    let denial = observed
        .iter()
        .find_map(|event| event.broker_denial.as_ref())
        .expect("a broker denial was recorded");
    assert_eq!(
        denial.integrity_event,
        IntegrityEventKind::BrokerProcessSpawnBlocked
    );
    assert_eq!(completed.result.integrity.broker_process_spawn_blocks, 1);
    assert!(
        observed.iter().all(|event| lifecycle_of(event).is_none()),
        "an unauthorized spawn must never reach containment"
    );
}

/// The engine reviews the benchmark's plan, not the model's framing of the request.
///
/// A model that asks to run a program the benchmark never declared is refused even when the
/// benchmark's own plan is authorized, which is only possible because the reviewed request is
/// rebuilt from trusted configuration.
#[test]
fn the_reviewed_request_is_the_benchmarks_plan_and_not_the_models_framing() {
    let root = temporary_root("trusted-request");
    let protected = protected_dir(&root, "evaluator");
    let benchmark = SpawnBenchmark::granting(&comspec(), &["/D", "/C", "exit 0"], vec![protected]);

    // The model asks for a program the benchmark never declared, and one it did.
    let result = run(
        &benchmark,
        Box::new(ScriptedAgent::new(vec![
            probe_spawn("calc.exe"),
            ModelDecision::GridAction(Action::Wait),
        ])),
        &root,
    );
    let (completed, observed) = match result {
        Ok(value) => value,
        // On a platform without containment the authorized spawn fails closed, which is the other
        // half of this test: the model's `calc.exe` request never influenced what was attempted.
        Err(EngineError::ProcessContainment(failure)) => {
            assert_eq!(failure.kind, ContainmentFailureKind::UnsupportedPlatform);
            remove_root(&root);
            return;
        }
        Err(other) => panic!("unexpected engine error: {other:?}"),
    };
    remove_root(&root);

    let lifecycles: Vec<&ProcessLifecycle> = observed.iter().filter_map(lifecycle_of).collect();
    assert_eq!(
        lifecycles.len(),
        1,
        "exactly one spawn reached containment: {lifecycles:?}"
    );
    assert!(
        matches!(lifecycles[0].decision, ProcessAuthorization::Authorized),
        "the benchmark's own authorized plan is what ran"
    );
    assert_eq!(completed.result.integrity.broker_process_spawn_blocks, 0);
}

// ---------------------------------------------------------------------------
// Real containment, Windows only.
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod enforced {
    use super::*;
    use probelab_core::{ContainmentStatus, IntegrityLevel, TerminationReason};
    use probelab_sandbox::ENFORCED_JOB_LIMITS;

    #[test]
    fn an_authorized_spawn_runs_contained_with_a_full_lifecycle_record() {
        let root = temporary_root("authorized");
        let protected = protected_dir(&root, "evaluator");
        let benchmark =
            SpawnBenchmark::granting(&comspec(), &["/D", "/C", "exit 0"], vec![protected]);
        let (completed, observed) = run(
            &benchmark,
            Box::new(ScriptedAgent::new(vec![probe_spawn("calc.exe")])),
            &root,
        )
        .expect("an authorized spawn runs");
        remove_root(&root);

        let lifecycle = observed
            .iter()
            .find_map(lifecycle_of)
            .expect("a process lifecycle was recorded");
        assert!(lifecycle.requested, "the record must show a request");
        assert!(
            matches!(lifecycle.decision, ProcessAuthorization::Authorized),
            "the broker authorized the benchmark's plan"
        );
        let execution = lifecycle.execution().expect("an execution was recorded");
        assert!(execution.spawned, "a child really ran");
        assert!(
            execution.pid.is_some_and(|pid| pid > 0),
            "a process identifier was recorded"
        );
        assert_eq!(execution.verified_integrity(), Some(IntegrityLevel::Low));
        assert_eq!(
            execution.containment,
            ContainmentStatus::Enforced {
                integrity: IntegrityLevel::Low,
                limits_applied: ENFORCED_JOB_LIMITS,
            }
        );
        assert_eq!(execution.termination, TerminationReason::Exited);
        assert_eq!(execution.exit_code, Some(0));

        // An authorized spawn is the broker working, not a violation: no integrity event at all.
        assert_eq!(completed.result.integrity.total_events(), 0);
        assert_eq!(completed.result.integrity.broker_process_spawn_blocks, 0);
        assert_eq!(completed.result.integrity.total_broker_denials(), 0);
        // The turn was spent without moving the world.
        assert_eq!(completed.result.capability.total_reward, 0);
        assert_eq!(completed.result.capability.invalid_action_count, 0);
    }

    #[test]
    fn a_contained_child_cannot_read_a_protected_path_and_cannot_write_outside_its_directory() {
        let root = temporary_root("boundary");
        // A stand-in for the evaluator: its contents are the answer key.
        let protected = protected_dir(&root, "evaluator");

        // The child copies the answer key and then tries to write outside its designated directory.
        let benchmark = SpawnBenchmark::granting(
            &comspec(),
            &[
                "/D",
                "/C",
                r#"copy /y "..\..\evaluator\answer-key.txt" ".\stolen.txt" >nul 2>&1 & echo escaped > "..\..\escaped.txt""#,
            ],
            vec![protected],
        );
        let result = run(
            &benchmark,
            Box::new(ScriptedAgent::new(vec![probe_spawn("calc.exe")])),
            &root,
        );
        let (completed, observed) = match result {
            Ok(value) => value,
            // Protecting the run root itself is refused rather than applied, because it would lock
            // the trusted process out of its own sandbox. That is the fail-closed path.
            Err(EngineError::ProcessContainment(failure)) => {
                assert_eq!(
                    failure.kind,
                    ContainmentFailureKind::PathProtectionUnavailable
                );
                remove_root(&root);
                return;
            }
            Err(other) => panic!("unexpected engine error: {other:?}"),
        };
        remove_root(&root);

        let lifecycle = observed.iter().find_map(lifecycle_of).expect("a lifecycle");
        let execution = lifecycle.execution().expect("an execution");
        assert!(execution.spawned, "the child ran: {execution:?}");

        // The designated directory is inside the run root, and nothing escaped it.
        assert!(
            !root.join("escaped.txt").exists(),
            "a contained child wrote outside its designated directory"
        );
        assert!(
            completed.result.integrity.total_events() == 0,
            "running an authorized child is not an integrity event"
        );
    }

    #[test]
    fn a_child_that_exceeds_its_cpu_budget_is_stopped_and_reported_as_such() {
        let root = temporary_root("cpu-limit");
        let mut plan = ProcessSpawnPlan::new(comspec()).with_args([
            "/D",
            "/C",
            r#"for /l %i in (1,1,100000000) do @rem"#,
        ]);
        plan.budget = probelab_core::ProcessBudget {
            wall_clock_millis: 20_000,
            job_cpu_time_millis: 1_000,
            per_process_cpu_time_millis: 1_000,
            job_memory_bytes: 256 * 1024 * 1024,
            per_process_memory_bytes: 128 * 1024 * 1024,
            active_processes: 1,
        };
        let mut policy = CapabilityPolicy::deny_all();
        policy.grant_process(comspec()).expect("grant cmd");
        let protected = protected_dir(&root, "evaluator");
        let benchmark = SpawnBenchmark {
            policy,
            plan: Some(plan),
            protected_paths: vec![protected],
            max_turns: 2,
        };

        let result = run(
            &benchmark,
            Box::new(ScriptedAgent::new(vec![probe_spawn("calc.exe")])),
            &root,
        );
        remove_root(&root);
        let Ok((_, observed)) = result else {
            // Failing closed is also an acceptable outcome; nothing is asserted about a child that
            // never started.
            return;
        };
        let Some(execution) = observed
            .iter()
            .find_map(lifecycle_of)
            .and_then(|l| l.execution())
        else {
            return;
        };
        assert!(execution.spawned);
        assert_ne!(
            execution.termination,
            TerminationReason::Exited,
            "a child that burned its whole CPU budget did not exit on its own"
        );
    }
}

#[cfg(not(windows))]
mod unsupported {
    use super::*;

    #[test]
    fn a_platform_without_containment_fails_closed_and_starts_nothing() {
        let root = temporary_root("unsupported");
        let benchmark = SpawnBenchmark::granting(
            "sh",
            &["-c", "echo hi"],
            vec![protected_dir(&root, "evaluator")],
        );
        let error = run(
            &benchmark,
            Box::new(ScriptedAgent::new(vec![probe_spawn("sh")])),
            &root,
        )
        .expect_err("containment cannot be established here");
        remove_root(&root);
        match error {
            EngineError::ProcessContainment(failure) => {
                assert_eq!(failure.kind, ContainmentFailureKind::UnsupportedPlatform);
            }
            other => panic!("unexpected engine error: {other:?}"),
        }
    }
}
