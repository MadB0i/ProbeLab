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
use std::time::Duration;

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

/// The wall-clock budget a benchmark plan is expected to declare.
///
/// This mirrors the value the documentation suggests. It is a suggestion only: every plan in these
/// tests states its own timeout explicitly, because an undeclared timeout is refused rather than
/// defaulted.
const SUGGESTED_TIMEOUT: Duration = Duration::from_secs(30);

/// A short timeout, used where a test needs the host's timer rather than the child's CPU budget.
const SHORT_TIMEOUT: Duration = Duration::from_millis(1_500);

/// How long a terminated child is given to actually stop existing.
///
/// `TerminateJobObject` marks a job's processes for termination and returns; the kernel does the work
/// on its own schedule. Generous enough to absorb a loaded machine, short enough that a child which
/// genuinely survives the kill is still caught.
#[cfg(windows)]
const TERMINATION_GRACE: Duration = Duration::from_secs(10);

/// Whether a terminated process is still present, polled over `grace`.
///
/// Returns true only if the process is *still* alive when the grace period expires, so a caller can
/// assert on the outcome without racing the kernel.
#[cfg(windows)]
fn survives(pid: u32, grace: Duration) -> bool {
    let deadline = std::time::Instant::now() + grace;
    loop {
        match is_process_alive(pid) {
            Ok(false) => return false,
            Ok(true) | Err(_) if std::time::Instant::now() >= deadline => {
                return is_process_alive(pid).unwrap_or(true)
            }
            _ => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

#[cfg(windows)]
use probelab_sandbox::is_process_alive;

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
    /// Grants `program` and declares a plan that runs it with `args` under `timeout`.
    fn granting(
        program: &str,
        args: &[&str],
        protected_paths: Vec<PathBuf>,
        timeout: Duration,
    ) -> Self {
        let mut policy = CapabilityPolicy::deny_all();
        policy.grant_process(program).expect("grant program");
        Self {
            policy,
            plan: Some(
                ProcessSpawnPlan::new(program)
                    .with_args(args.iter().copied())
                    .with_timeout(timeout),
            ),
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
            plan: Some(ProcessSpawnPlan::new(planned).with_timeout(SHORT_TIMEOUT)),
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

/// The model asks for the process permission the way a *real* model can: by naming nothing.
///
/// This is the model-reachable request. The contrast with [`probe_spawn`] is the whole point — a
/// probe carries a program and arguments chosen by whatever produced it, whereas this variant has no
/// fields at all, so there is nothing for a model to put a command line into. That is what makes it
/// safe to document in a prompt.
fn request_process() -> ModelDecision {
    ModelDecision::ProcessSpawnRequest
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
    let benchmark = SpawnBenchmark::granting(
        &comspec(),
        &["/D", "/C", "exit 0"],
        vec![protected],
        SUGGESTED_TIMEOUT,
    );

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
// The model-reachable request. This is the part a real provider can actually produce.
// ---------------------------------------------------------------------------

/// A model that asks for the process permission — naming nothing — reaches containment.
///
/// This is the test the whole phase turns on. Before it, `CapabilityProbe` was the only way to ask
/// for a spawn and it is deliberately unreachable from any real provider, so a benchmark could
/// declare a plan, grant the program, and still never be asked. `ProcessSpawnRequest` carries no
/// fields, so there is nothing in it for a model to abuse, and the request the broker reviewed is
/// rebuilt from the benchmark's own plan.
///
/// The pairing with the probe-based test above is what makes the claim checkable: both produce a
/// spawn, and neither can put a model-chosen program on the command line.
#[test]
fn a_model_can_ask_for_the_process_permission_and_reach_containment() {
    let root = temporary_root("model-request");
    let protected = protected_dir(&root, "evaluator");
    let benchmark = SpawnBenchmark::granting(
        &comspec(),
        &["/D", "/C", "echo requested-by-a-real-model"],
        vec![protected],
        SUGGESTED_TIMEOUT,
    );

    let result = run(
        &benchmark,
        Box::new(ScriptedAgent::new(vec![
            request_process(),
            ModelDecision::GridAction(Action::Wait),
        ])),
        &root,
    );
    let (completed, observed) = match result {
        Ok(value) => value,
        // Without OS containment the authorized spawn fails closed, which is still the correct
        // outcome: the request was understood and the run stopped rather than pretending.
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
        "the permission request did not reach containment exactly once: {lifecycles:?}"
    );
    assert!(
        matches!(lifecycles[0].decision, ProcessAuthorization::Authorized),
        "the benchmark's declared plan was authorized"
    );
    assert_eq!(
        completed.result.integrity.broker_process_spawn_blocks, 0,
        "an authorized request must not also be counted as blocked"
    );
    assert_eq!(
        completed.result.integrity.policy_violations, 0,
        "the request must be consumed by the broker and never reach the deny-all policy gate"
    );
    assert_eq!(
        completed.result.integrity.sandbox_violations, 0,
        "asking for a permission the benchmark offers is not a sandbox violation"
    );
}

/// A request in a benchmark that declares no plan is refused, and starts nothing.
///
/// The model-reachable request must not become a way to probe for what is installed. With no plan
/// there is nothing to rebuild the reviewed request from, so the broker sees an empty program, the
/// allowlist comparison fails, and the refusal is recorded as a broker denial — the same treatment a
/// named probe gets. Nothing is asked of the operating system.
#[test]
fn a_process_request_in_a_benchmark_with_no_plan_is_refused_and_starts_nothing() {
    let root = temporary_root("no-plan");
    let benchmark = SpawnBenchmark::granting_nothing();
    let (completed, observed) = run(
        &benchmark,
        Box::new(ScriptedAgent::new(vec![request_process()])),
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
    assert_eq!(completed.result.integrity.policy_violations, 0);
    assert!(
        observed.iter().all(|event| lifecycle_of(event).is_none()),
        "a request in a benchmark with no plan must not reach the operating system"
    );
}

// ---------------------------------------------------------------------------
// Real containment, Windows only.
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod enforced {
    use super::*;
    use probelab_core::{
        ContainmentStatus, IntegrityLevel, StreamCapture, TerminationReason, STREAM_PREVIEW_LIMIT,
    };
    use probelab_sandbox::ENFORCED_JOB_LIMITS;

    #[test]
    fn an_authorized_spawn_runs_contained_with_a_full_lifecycle_record() {
        let root = temporary_root("authorized");
        let protected = protected_dir(&root, "evaluator");
        let benchmark = SpawnBenchmark::granting(
            &comspec(),
            &["/D", "/C", "exit 0"],
            vec![protected],
            SUGGESTED_TIMEOUT,
        );
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
            SUGGESTED_TIMEOUT,
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
        // A tight CPU budget and a generous wall-clock budget, so the *CPU* limit is what ends this
        // child rather than the host's timer.
        let plan = ProcessSpawnPlan::new(comspec())
            .with_args(["/D", "/C", "for /l %i in (1,1,100000000) do @rem"])
            .with_timeout(Duration::from_secs(60))
            .with_budget(probelab_core::ProcessBudget {
                job_cpu_time_millis: 1_000,
                per_process_cpu_time_millis: 1_000,
                job_memory_bytes: 256 * 1024 * 1024,
                per_process_memory_bytes: 128 * 1024 * 1024,
                active_processes: 1,
            });
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

    #[test]
    fn a_child_that_outlives_its_declared_timeout_is_killed_with_the_job() {
        let root = temporary_root("hang");
        // The child runs far past the declared budget, so only the host's wall-clock timer can end it:
        // a job object does not bound elapsed time on its own.
        //
        // The spin loop is deliberate. Two earlier versions of this test used mechanisms that v0.5
        // closed off. `ping -n 60 127.0.0.1` stopped working the moment contained children were refused
        // outbound network, and `waitfor /T 60` is refused outright inside the AppContainer. A bounded
        // `for /L` loop needs neither a network, a console, nor a named object, so it tests the
        // wall-clock bound on its own terms.
        //
        // It also makes the assertion stronger rather than weaker: the child is actively consuming
        // CPU, and the job's CPU budget here is 60 seconds. If the wall clock did not fire at 1.5
        // seconds, the CPU limit would be the thing that eventually stopped it, so a pass here is
        // specifically evidence about elapsed time.
        let plan = ProcessSpawnPlan::new(comspec())
            .with_args(["/D", "/C", "for /L %i in (1,1,2000000000) do @rem"])
            .with_timeout(SHORT_TIMEOUT)
            .with_budget(probelab_core::ProcessBudget {
                job_cpu_time_millis: 60_000,
                per_process_cpu_time_millis: 60_000,
                job_memory_bytes: 256 * 1024 * 1024,
                per_process_memory_bytes: 128 * 1024 * 1024,
                active_processes: 1,
            });
        let mut policy = CapabilityPolicy::deny_all();
        policy.grant_process(comspec()).expect("grant cmd");
        let protected = protected_dir(&root, "evaluator");
        let benchmark = SpawnBenchmark {
            policy,
            plan: Some(plan),
            protected_paths: vec![protected],
            max_turns: 2,
        };

        let started = std::time::Instant::now();
        let result = run(
            &benchmark,
            Box::new(ScriptedAgent::new(vec![probe_spawn("calc.exe")])),
            &root,
        );
        let elapsed = started.elapsed();
        let (completed, observed) = result.expect("the run completes after the timeout");
        remove_root(&root);

        let execution = observed
            .iter()
            .find_map(lifecycle_of)
            .and_then(|l| l.execution())
            .expect("an execution record for the hung child");
        assert!(
            execution.spawned,
            "the child really did start: {execution:?}"
        );
        assert_eq!(
            execution.termination,
            TerminationReason::Timeout,
            "a hung child must be reported as a timeout, not as a normal exit: {execution:?}"
        );
        assert_eq!(
            execution.exit_code, None,
            "the host stopped this child, so the child never chose an exit code"
        );
        // The timer is a real wall-clock bound, so the run cannot have waited the child's own 60s.
        assert!(
            elapsed < Duration::from_secs(45),
            "the host waited {elapsed:?}, which is not a timeout"
        );
        // Nothing the child started may outlive it: the whole job is terminated, not just the tracked
        // process, and the guard's drop closes the job handle with KILL_ON_JOB_CLOSE as a backstop.
        //
        // Polled rather than asserted once. `TerminateJobObject` is a *request*: it marks every process
        // in the job for termination, and the kernel finishes the work asynchronously. An earlier
        // version of this line asserted the child was already gone on the very next query, which is a
        // stronger claim than the API makes — and it duly failed roughly one run in five under the
        // full suite, where the machine is busy enough that the kill had not landed yet. The property
        // worth holding is that the child does not *survive*, so that is what is bounded here. An
        // instant-death assertion would be a test that fails on load rather than on regressions.
        let pid = execution.pid.expect("a process identifier was recorded");
        assert!(
            !survives(pid, TERMINATION_GRACE),
            "the timed-out child was still running {:?} after the job was terminated",
            TERMINATION_GRACE
        );
        assert_eq!(
            completed.result.integrity.total_events(),
            0,
            "a timeout is not a model violation"
        );
    }

    #[test]
    fn a_child_that_writes_more_than_the_preview_cap_is_byte_counted_not_stored_whole() {
        let root = temporary_root("large-output");
        // Far more output than the 4 KiB preview can carry. `ping` per line keeps the command short.
        let plan = ProcessSpawnPlan::new(comspec())
            .with_args([
                "/D",
                "/C",
                r#"for /l %i in (1,1,400) do @echo PROBELAB-LOUD-MARKER-%i-0123456789abcdefghijklmnopqrstuvwxyz"#,
            ])
            .with_timeout(SUGGESTED_TIMEOUT);
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
        let Ok((_, observed)) = result else {
            return;
        };
        let Some(execution) = observed
            .iter()
            .find_map(lifecycle_of)
            .and_then(|l| l.execution())
        else {
            return;
        };
        let stream = &execution.stdout;
        assert!(
            stream.bytes as usize > STREAM_PREVIEW_LIMIT,
            "the child must have written more than the preview cap: {stream:?}"
        );
        assert!(
            stream.is_truncated(),
            "a stream larger than the cap must be marked truncated"
        );
        assert_eq!(
            stream.preview.chars().count(),
            STREAM_PREVIEW_LIMIT,
            "the preview must be capped, not merely shortened"
        );
        assert!(
            !stream.preview.contains(&"Z".repeat(8)),
            "the preview must not carry the whole stream"
        );

        // The full output stays in the designated directory, which has the run's own cleanup rule.
        let captured = root
            .join("spawn")
            .join(probelab_sandbox::STDOUT_CAPTURE_FILE);
        let on_disk = std::fs::read_to_string(&captured).expect("the full capture file");
        assert_eq!(
            on_disk.len() as u64,
            stream.bytes,
            "the recorded byte count must match the file on disk"
        );
        assert!(
            on_disk.len() > STREAM_PREVIEW_LIMIT,
            "the file must hold more than the preview did"
        );
        assert!(on_disk.contains("PROBELAB-LOUD-MARKER-400"));
        remove_root(&root);
    }

    #[test]
    fn captured_output_never_reaches_a_model_facing_observation() {
        let root = temporary_root("no-leak");
        let marker = "PROBELAB-CHILD-SECRET-MARKER";
        let plan = ProcessSpawnPlan::new(comspec())
            .with_args(["/D", "/C", &format!("echo {marker}")])
            .with_timeout(SUGGESTED_TIMEOUT);
        let mut policy = CapabilityPolicy::deny_all();
        policy.grant_process(comspec()).expect("grant cmd");
        let protected = protected_dir(&root, "evaluator");
        let benchmark = SpawnBenchmark {
            policy,
            plan: Some(plan),
            protected_paths: vec![protected],
            max_turns: 3,
        };

        let Ok((_, observed)) = run(
            &benchmark,
            Box::new(ScriptedAgent::new(vec![
                probe_spawn("calc.exe"),
                ModelDecision::GridAction(Action::Wait),
            ])),
            &root,
        ) else {
            return;
        };
        remove_root(&root);

        // The child's own output is recorded for the operator, but it is never part of what the model
        // is shown. Every observation in the run must be free of it.
        for event in &observed {
            let observation =
                serde_json::to_string(&event.observation).expect("serialize observation");
            assert!(
                !observation.contains(marker),
                "a model-facing observation leaked the child's output: {observation}"
            );
        }
        let leaked_into_telemetry = observed
            .iter()
            .filter_map(lifecycle_of)
            .filter_map(|lifecycle| lifecycle.execution())
            .any(|execution| execution.stdout.preview.contains(marker));
        assert!(
            leaked_into_telemetry,
            "the capture should still be recorded for operator visibility"
        );
    }

    #[test]
    fn a_child_that_exits_normally_is_unchanged_by_capture_and_timeout_plumbing() {
        let root = temporary_root("regression");
        let protected = protected_dir(&root, "evaluator");
        let benchmark = SpawnBenchmark::granting(
            &comspec(),
            &["/D", "/C", "exit 0"],
            vec![protected],
            SUGGESTED_TIMEOUT,
        );
        let (completed, observed) = run(
            &benchmark,
            Box::new(ScriptedAgent::new(vec![probe_spawn("calc.exe")])),
            &root,
        )
        .expect("an authorized spawn runs");
        remove_root(&root);

        let execution = observed
            .iter()
            .find_map(lifecycle_of)
            .and_then(|l| l.execution())
            .expect("an execution record");
        assert_eq!(
            execution.termination,
            TerminationReason::Exited,
            "the success path must be unchanged"
        );
        assert_eq!(execution.exit_code, Some(0));
        assert!(execution
            .verified_integrity()
            .is_some_and(|level| level.is_low()));
        assert_eq!(completed.result.integrity.total_events(), 0);
        assert_eq!(completed.result.capability.total_reward, 0);
        // A silent child produces an empty, consistent capture rather than a missing one.
        assert_eq!(execution.stdout, StreamCapture::empty());
        assert_eq!(execution.stderr, StreamCapture::empty());
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
            SUGGESTED_TIMEOUT,
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
