//! Proof that the sandbox-process benchmark measures something, measured the same way as the grid
//! world.
//!
//! This benchmark exists because the containment stack was unreachable from any real model. Its whole
//! value is that a policy which never asks for the process and a policy which does must score
//! differently, with a gap wide enough to be a measurement rather than noise. These tests are written
//! to fail if that stops being true.
//!
//! Two honest limits, stated here because they bound what the numbers mean:
//!
//! * **The child's work is fixed, not seed-varying.** The `Benchmark` trait's spawn-plan accessor takes
//!   no seed, so a per-seed command line is not expressible yet. This benchmark therefore measures
//!   *whether a policy uses the capability it is offered, and still finishes the task under a budget
//!   that charges it a turn* — not coding ability. A seed-aware plan accessor is the prerequisite for
//!   the latter.
//! * **Every episode runs a real contained child**, so these tests spawn real processes. They are
//!   Windows-only for that reason, and they are slower than the grid-world suites as a result.

#![cfg(windows)]

use std::path::PathBuf;
use std::sync::Arc;

use probelab_agent::{ProviderAgent, ReferenceAgent};
use probelab_benchmarks::{SandboxProcessBenchmark, MINIMUM_EVAL_SEEDS};
use probelab_core::{
    Action, Agent, Benchmark, ModelDecision, ModelDescriptor, ModelRequest, TelemetryRecorder,
};
use probelab_engine::harness::{AggregateScore, Harness, MeasurementPlan, SeedPool, SeedPools};
use probelab_engine::BenchmarkRunner;
use probelab_evaluator::SandboxProcessEvaluator;
use probelab_model_adapters::{GridWorldReference, SandboxProcessReference};
use probelab_telemetry::SqliteTelemetryRecorder;

fn benchmark() -> SandboxProcessBenchmark {
    SandboxProcessBenchmark::built_in().expect("built-in sandbox-process benchmark")
}

fn pools() -> SeedPools {
    let catalog = benchmark();
    let definition = catalog.definition();
    SeedPools::new(
        definition.dev_seed_pool.clone(),
        definition.eval_seed_pool.clone(),
    )
    .expect("declared pools are disjoint")
}

static HARNESS_SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static SCRATCH_DIRECTORIES: std::sync::OnceLock<std::sync::Mutex<Vec<PathBuf>>> =
    std::sync::OnceLock::new();

fn scratch_directories() -> &'static std::sync::Mutex<Vec<PathBuf>> {
    SCRATCH_DIRECTORIES.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

fn harness() -> Harness {
    let serial = HARNESS_SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "probelab-sandbox-process-{}-{serial}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).expect("scratch directory");
    let scratch: PathBuf = root.clone();
    let factory: probelab_engine::harness::RecorderFactory =
        Arc::new(move |run_id: &str| -> Arc<dyn TelemetryRecorder> {
            let database = scratch.join(format!("{run_id}.sqlite3"));
            Arc::new(SqliteTelemetryRecorder::with_callback(&database, |_| {}).expect("recorder"))
        });
    let harness = Harness::new(pools(), factory);
    scratch_directories()
        .lock()
        .expect("scratch list")
        .push(root);
    harness
}

/// A policy that asks for the process on its first turn and then behaves like the search reference.
///
/// This is the ceiling for "uses the capability and finishes the task". It exists to show the
/// benchmark is satisfiable, not to be a claim about any model.
fn requests_then_searches(seed: u64) -> Box<dyn Agent> {
    Box::new(ReferenceAgent::new(SandboxProcessReference::new(seed)))
}

/// The same search policy, but it never asks. The only difference from the above is the request, so
/// any gap between them is attributable to the capability and to nothing else.
fn searches_only(seed: u64) -> Box<dyn Agent> {
    Box::new(ReferenceAgent::new(GridWorldReference::new(seed)))
}

/// The floor: a policy that only ever waits, and never asks for anything.
///
/// A real local provider rather than a mock strategy, because every `MockModelConfig` that looks
/// passive still navigates — `GreedyMistake` waits one turn and then plays optimally, which makes it a
/// weak ceiling, not a floor. A benchmark with no genuine floor cannot tell "hard" from "impossible".
struct WaitForever;

impl probelab_core::ModelProvider for WaitForever {
    fn descriptor(&self) -> ModelDescriptor {
        ModelDescriptor {
            adapter: "sandbox-process-test-v1".to_owned(),
            strategy: "wait_forever".to_owned(),
            seed: 0,
            parameters: Default::default(),
        }
    }

    fn predict(&mut self, _request: ModelRequest) -> Result<ModelDecision, String> {
        Ok(ModelDecision::GridAction(Action::Wait))
    }
}

fn waits_only(_seed: u64) -> Box<dyn Agent> {
    Box::new(ProviderAgent::from_provider(WaitForever))
}

fn measure(label: &str, build: &mut dyn FnMut(u64) -> Box<dyn Agent>) -> AggregateScore {
    harness()
        .measure(
            &benchmark(),
            &SandboxProcessEvaluator::new(),
            &BenchmarkRunner::new(),
            &MeasurementPlan::eval(benchmark().definition().eval_seed_pool.len()),
            label,
            build,
        )
        .unwrap_or_else(|error| panic!("{label} measurement failed: {error}"))
}

/// The benchmark's own contract: enough evaluation seeds, disjoint pools, and a declared program.
#[test]
fn sp_the_definition_declares_disjoint_pools_and_one_declared_program() {
    let catalog = benchmark();
    let definition = catalog.definition();
    assert_eq!(definition.id, "sandbox-process");
    assert!(
        definition.eval_seed_pool.len() >= MINIMUM_EVAL_SEEDS,
        "the evaluation pool must be at least {MINIMUM_EVAL_SEEDS} seeds, got {}",
        definition.eval_seed_pool.len()
    );
    for seed in &definition.eval_seed_pool {
        assert!(
            !definition.dev_seed_pool.contains(seed),
            "seed {seed} appears in both pools, so every held-out number would be tuned against"
        );
    }

    let program = catalog.program();
    assert!(
        std::path::Path::new(program).is_absolute(),
        "a spawn plan must name an absolute program, got {program:?}"
    );

    // The grant and the plan must name the same program, or every request is refused and the
    // benchmark silently measures a policy that cannot use its own capability.
    let policy = benchmark().capability_policy();
    assert!(
        policy.allows_program(program),
        "the declared program is not on the policy allowlist"
    );
    assert_eq!(
        policy.filesystem_read_roots(),
        &[] as &[PathBuf],
        "this benchmark grants no filesystem access"
    );
    assert_eq!(
        policy.allowed_endpoints(),
        &[] as &[String],
        "this benchmark grants no network access"
    );

    // A spawning benchmark has to protect its answer key: containment refuses an empty set.
    assert!(
        !catalog.protected_paths().is_empty(),
        "a benchmark that runs a real child must declare protected paths"
    );
    assert_eq!(
        catalog.protected_paths(),
        std::slice::from_ref(&catalog.answer_key_path().to_path_buf()),
        "the protected path and the answer key must be the same directory; if they drift, the child \
         is protected from nothing"
    );
    assert!(
        catalog
            .answer_key_path()
            .join("expected-output.txt")
            .exists(),
        "the answer key must exist, or protecting it protects nothing"
    );
}

/// The headline property: asking for the process is worth something.
///
/// Both policies navigate with the same solver and the same budget. The only difference is the one
/// request. If this ever stops holding, the benchmark has stopped measuring its own capability and
/// has become the grid world with extra steps.
#[test]
fn sp_asking_for_the_process_beats_never_asking_and_the_intervals_are_disjoint() {
    let never = measure("search only (no request)", &mut searches_only);
    let requests = measure("search + process request", &mut requests_then_searches);

    assert_eq!(requests.episodes, never.episodes);
    assert!(
        requests.episodes >= MINIMUM_EVAL_SEEDS,
        "too few episodes for the comparison to mean anything: {}",
        requests.episodes
    );

    assert!(
        requests.successes > never.successes,
        "a policy that uses the capability the benchmark offers did no better than one that never \
         uses it: {} vs {} successes",
        requests.successes,
        never.successes
    );
    assert!(
        never.success_rate.upper < requests.success_rate.lower,
        "the intervals overlap, so the gap is not separable from noise: never-asks \
         [{:.3}, {:.3}] vs asks [{:.3}, {:.3}]",
        never.success_rate.lower,
        never.success_rate.upper,
        requests.success_rate.lower,
        requests.success_rate.upper,
    );
}

/// The floor exists, and the ceiling is real.
///
/// Without a policy that scores zero the benchmark could be trivially satisfiable; without a ceiling
/// it could be unsatisfiable, and an unsatisfiable benchmark reports a confident zero for everything.
#[test]
fn sp_there_is_a_floor_and_a_ceiling() {
    let floor = measure("wait only", &mut waits_only);
    let ceiling = measure("search + process request", &mut requests_then_searches);

    assert_eq!(
        floor.successes, 0,
        "a policy that never asks and never moves should score nothing, got {}",
        floor.successes
    );
    assert!(
        ceiling.successes > 0,
        "no policy can complete this benchmark, so it measures nothing"
    );
}

/// Every policy travels the identical path: same harness, same evaluator, same telemetry sink.
///
/// The requesting policy runs a real child; the others do not. That difference is the subject of the
/// benchmark, not a special case in how it is measured, so it must not come with a different
/// measurement route.
#[test]
fn sp_every_policy_is_measured_through_the_same_harness() {
    let never = measure("search only (no request)", &mut searches_only);
    let requests = measure("search + process request", &mut requests_then_searches);

    assert_eq!(never.episodes, requests.episodes);
    assert_eq!(
        never.success_rate.confidence,
        requests.success_rate.confidence
    );
    assert_eq!(
        never.status_counts.len(),
        requests.status_counts.len(),
        "the two policies terminated in a different number of distinct ways, which suggests they \
         were not measured the same way"
    );
}

/// The gap must also be visible on the dev pool, or it is a property of the held-out seeds.
#[test]
fn sp_the_gap_is_also_present_on_the_dev_pool() {
    let never = harness()
        .measure(
            &benchmark(),
            &SandboxProcessEvaluator::new(),
            &BenchmarkRunner::new(),
            &MeasurementPlan {
                pool: SeedPool::Dev,
                episodes: benchmark().definition().dev_seed_pool.len(),
                ..MeasurementPlan::default()
            },
            "search only (dev)",
            &mut searches_only,
        )
        .expect("dev measurement");
    let requests = harness()
        .measure(
            &benchmark(),
            &SandboxProcessEvaluator::new(),
            &BenchmarkRunner::new(),
            &MeasurementPlan {
                pool: SeedPool::Dev,
                episodes: benchmark().definition().dev_seed_pool.len(),
                ..MeasurementPlan::default()
            },
            "search + request (dev)",
            &mut requests_then_searches,
        )
        .expect("dev measurement");

    assert!(
        requests.successes > never.successes,
        "the gap only appears on the evaluation pool, so it is a property of those seeds rather \
         than of the capability: dev {} vs {}",
        requests.successes,
        never.successes
    );
}
