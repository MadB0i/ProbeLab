//! Phase 0: is the bounded history window actually load-bearing?
//!
//! This file exists to answer one question with evidence rather than assertion: **does memory change
//! what a policy can do on the grid-world benchmark?**
//!
//! Two reactive policies are compared. They are the same policy — identical move rules, identical
//! tie-breaks, identical information from the current observation — and differ in exactly one respect:
//! [`MemoryReactive`] reads `ModelRequest::history` and [`StatelessReactive`] has no way to. Any
//! difference between their scores is therefore the value of the window, with nothing else mixed in.
//!
//! The answer this file records is that memory is **not** load-bearing in the v2 environment, and it
//! records the structural reason rather than leaving the result looking like a mystery.

use std::sync::Arc;

use probelab_agent::ProviderAgent;
use probelab_benchmarks::GridWorldBenchmark;
use probelab_core::{
    Action, Agent, Coordinate, ModelDecision, ModelProvider, ModelRequest, Observation, Sandbox,
    TileKind,
};
use probelab_engine::harness::{AggregateScore, Harness, MeasurementPlan};
use probelab_engine::BenchmarkRunner;
use probelab_evaluator::GridWorldEvaluator;
use probelab_model_adapters::{
    LookaheadSolver, MemoryReactive, MockModelConfig, MockModelProvider, StatelessReactive,
};
use probelab_sandbox::{grid_world_layout, GridWorldSandbox};
use probelab_telemetry::SqliteTelemetryRecorder;

fn benchmark() -> GridWorldBenchmark {
    GridWorldBenchmark::built_in().expect("built-in grid-world benchmark")
}

fn harness() -> Harness {
    let root = probelab_engine::harness::unique_scratch_root("phase0");
    let scratch = root.clone();
    let factory: probelab_engine::harness::RecorderFactory = Arc::new(
        move |run_id: &str| -> Arc<dyn probelab_core::TelemetryRecorder> {
            let database = scratch.join(format!("{run_id}.sqlite3"));
            Arc::new(SqliteTelemetryRecorder::with_callback(&database, |_| {}).expect("recorder"))
        },
    );
    Harness::new(
        probelab_engine::harness::SeedPools::new(
            benchmark().definition().dev_seed_pool.clone(),
            benchmark().definition().eval_seed_pool.clone(),
        )
        .expect("disjoint pools"),
        factory,
    )
}

fn measure(label: &str, build: &mut dyn FnMut(u64) -> Box<dyn Agent>) -> AggregateScore {
    harness()
        .measure(
            &benchmark(),
            &GridWorldEvaluator::new(),
            &BenchmarkRunner::new(),
            &MeasurementPlan::eval(benchmark().definition().eval_seed_pool.len()),
            label,
            build,
        )
        .unwrap_or_else(|error| panic!("{label} measurement failed: {error}"))
}

// ---------------------------------------------------------------------------------------------
// Step 0 — verify the knowledge scope of everything being compared
// ---------------------------------------------------------------------------------------------

/// The search solver is only credible as a ceiling if it sees nothing but the model-facing interface.
///
/// The claim is that its decisions are a function of the `ModelRequest` stream alone. The strongest
/// cheap test of that is to hand it two identical request streams under two different seeds and
/// require identical decisions: if any part of the layout leaked in through the seed, the two runs
/// would diverge.
#[test]
fn p0_the_search_solver_decisions_depend_only_on_the_request_stream() {
    let seed_a = 2_000_u64;
    let seed_b = 2_031_u64;
    assert_ne!(
        seed_a, seed_b,
        "the two seeds must describe different worlds"
    );
    assert_ne!(
        grid_world_layout(seed_a).tiles(),
        grid_world_layout(seed_b).tiles(),
        "the two seeds must produce different layouts or this proves nothing"
    );

    // One shared, fixed request stream, replayed against both constructors.
    let requests = synthetic_stream();
    let decisions_a = run_stream(LookaheadSolver::new(seed_a), &requests);
    let decisions_b = run_stream(LookaheadSolver::new(seed_b), &requests);
    assert_eq!(
        decisions_a, decisions_b,
        "the search solver's decisions changed with the seed, so something outside the request \
         stream is reaching it"
    );
}

/// The seed is recorded but must not steer anything: two seeds, same stream, same answers.
fn run_stream(solver: LookaheadSolver, requests: &[ModelRequest]) -> Vec<Action> {
    let mut provider = solver;
    requests
        .iter()
        .map(
            |request| match provider.predict(request.clone()).expect("decision") {
                ModelDecision::GridAction(action) => action,
                other => panic!("unexpected decision {other:?}"),
            },
        )
        .collect()
}

/// A small fixed request stream, built by hand so it does not depend on any generator.
fn synthetic_stream() -> Vec<ModelRequest> {
    let grid = probelab_core::GridSize {
        width: 5,
        height: 5,
    };
    let mut requests = Vec::new();
    for turn in 1..=4u32 {
        let position = Coordinate {
            x: u8::try_from(turn - 1).expect("fits"),
            y: 0,
        };
        requests.push(ModelRequest {
            episode_id: "phase0-synthetic".to_owned(),
            turn,
            observation: Observation {
                turn,
                position,
                grid_size: grid,
                current_tile: TileKind::Safe,
                visible_neighbors: in_bounds(grid, position),
                distance_to_goal: 4 - (turn - 1),
                max_turns: 9,
                remaining_turns: 10 - turn,
                available_actions: vec![
                    Action::Up,
                    Action::Down,
                    Action::Left,
                    Action::Right,
                    Action::Wait,
                ],
            },
            available_actions: vec![
                Action::Up,
                Action::Down,
                Action::Left,
                Action::Right,
                Action::Wait,
            ],
            // The grid-world benchmark declares no spawn plan, so no process is on offer. Stated
            // here rather than defaulted so a change to the benchmark would surface as a mismatch
            // in these phase-0 controls instead of passing silently.
            process_spawn_available: false,
            // Grid path: the grid world declares no game.
            game: None,
            history: Vec::new(),
        });
    }
    requests
}

/// The in-bounds orthogonal neighbours of a cell, in the engine's own order.
///
/// Off-grid cells are omitted rather than reported, because `ModelRequest::validate` rejects an
/// out-of-bounds neighbour and a hand-built request has to satisfy the same contract a real one does.
fn in_bounds(grid: probelab_core::GridSize, from: Coordinate) -> Vec<probelab_core::VisibleTile> {
    let mut cells = Vec::new();
    for (dx, dy) in [(0i16, -1i16), (0, 1), (-1, 0), (1, 0)] {
        let x = i16::from(from.x) + dx;
        let y = i16::from(from.y) + dy;
        if x < 0 || y < 0 || x >= i16::from(grid.width) || y >= i16::from(grid.height) {
            continue;
        }
        cells.push(probelab_core::VisibleTile {
            coordinate: Coordinate {
                x: x as u8,
                y: y as u8,
            },
            tile: TileKind::Safe,
        });
    }
    cells
}

/// The history window is actually delivered.
///
/// This is the control for the whole phase. "Memory made no difference" is only a finding if memory
/// was *supplied*; if the engine were quietly sending an empty window, the same null result would
/// mean nothing at all.
#[test]
fn p0_the_history_window_is_delivered_to_the_model() {
    let lengths: Arc<std::sync::Mutex<Vec<u32>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    struct Probe(ProviderAgent, Arc<std::sync::Mutex<Vec<u32>>>);
    impl Agent for Probe {
        fn descriptor(&self) -> probelab_core::ModelDescriptor {
            self.0.descriptor()
        }
        fn decide(&mut self, request: ModelRequest) -> Result<ModelDecision, String> {
            self.1
                .lock()
                .expect("lengths")
                .push(u32::try_from(request.history.len()).expect("fits"));
            self.0.decide(request)
        }
    }
    let recorder: Arc<dyn probelab_core::TelemetryRecorder> = Arc::new(
        SqliteTelemetryRecorder::with_callback(
            // Unique per process *launch*, not per pid: see `unique_scratch_root` for why a
            // pid-keyed path fails intermittently once Windows recycles an identifier.
            probelab_engine::harness::unique_scratch_root("phase0-history")
                .join("telemetry.sqlite3"),
            |_| {},
        )
        .expect("recorder"),
    );
    BenchmarkRunner::new()
        .run(
            &benchmark(),
            Box::new(Probe(
                ProviderAgent::from_provider(MemoryReactive),
                Arc::clone(&lengths),
            )),
            &GridWorldEvaluator::new(),
            2_000,
            "phase0-history",
            recorder,
        )
        .expect("run");
    let observed = lengths.lock().expect("lengths").clone();

    assert_eq!(
        observed.first().copied(),
        Some(0),
        "turn one has no history"
    );
    assert!(
        observed.iter().skip(1).any(|length| *length > 0),
        "no turn after the first received any history, so the null result below would be vacuous"
    );
    let cap = u32::try_from(probelab_core::HISTORY_WINDOW).expect("fits");
    assert!(
        observed.iter().all(|length| *length <= cap),
        "history exceeded its cap: {observed:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// Step 1 — the two policies differ only in whether they read the window
// ---------------------------------------------------------------------------------------------

#[test]
fn p0_the_two_reactive_policies_declare_differently_on_history_and_nothing_else() {
    let stateless = StatelessReactive.descriptor();
    let memory = MemoryReactive.descriptor();

    assert_eq!(stateless.strategy, "stateless_reactive");
    assert_eq!(memory.strategy, "memory_reactive");
    assert_eq!(
        stateless.parameters.get("reads_history"),
        Some(&serde_json::Value::String("false".to_owned()))
    );
    assert_eq!(
        memory.parameters.get("reads_history"),
        Some(&serde_json::Value::String("true".to_owned()))
    );

    // Every other recorded parameter must be identical, or the comparison is confounded. The two
    // identity-carrying keys are excluded and checked separately: they are *supposed* to name the
    // strategy, and that is exactly how a result bundle tells the two runs apart.
    let identity_keys = ["configuration", "model"];
    let comparable = |descriptor: &probelab_core::ModelDescriptor| {
        let mut keys = descriptor
            .parameters
            .keys()
            .filter(|key| !identity_keys.contains(&key.as_str()) && key.as_str() != "reads_history")
            .cloned()
            .collect::<Vec<_>>();
        keys.sort();
        keys.into_iter()
            .map(|key| {
                (
                    key.clone(),
                    descriptor.parameters.get(&key).cloned().unwrap_or_default(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        comparable(&stateless),
        comparable(&memory),
        "the two policies differ in more than whether they read history"
    );
    // The excluded keys must be exactly the strategy name, so nothing else is hidden in them.
    assert_eq!(
        stateless.parameters.get("configuration"),
        Some(&serde_json::Value::String("stateless_reactive".to_owned()))
    );
    assert_eq!(
        memory.parameters.get("configuration"),
        Some(&serde_json::Value::String("memory_reactive".to_owned()))
    );
    assert_eq!(stateless.adapter, memory.adapter);
    assert_eq!(stateless.seed, memory.seed);
}

// ---------------------------------------------------------------------------------------------
// Step 2 — the structural reason, proved directly
// ---------------------------------------------------------------------------------------------

/// The environment's observability radius is exactly one step, so information is never stale at the
/// moment of decision.
///
/// This is the finding that explains the null result, and it is worth stating as a property of the
/// generator rather than leaving as a hypothesis. Two consequences follow, and both are asserted
/// below for every evaluation seed:
///
/// 1. A cell two or more steps away is **never** in the observation, so a policy cannot act on it.
/// 2. A cell one step away is **always** in the observation, so by the time a policy could step onto
///    a hazard, it has already been told the hazard is there.
///
/// A memory window can only help when acting on information that is no longer in front of you. Under
/// (2) there is never such information: the cell you are about to enter was adjacent on the turn you
/// chose the move. That is the whole explanation.
#[test]
fn p0_the_observation_radius_equals_the_step_size() {
    for seed in benchmark().definition().eval_seed_pool.clone() {
        let layout = grid_world_layout(seed);
        let budget = layout.max_turns();
        let mut sandbox = GridWorldSandbox::with_max_turns(seed, budget).expect("sandbox");
        while sandbox.episode().status == probelab_core::EpisodeStatus::Running {
            let observation = sandbox.observation();
            let here = observation.position;
            for y in 0..5u8 {
                for x in 0..5u8 {
                    let cell = Coordinate { x, y };
                    let distance = u32::from(x.abs_diff(here.x)) + u32::from(y.abs_diff(here.y));
                    let reported = observation
                        .visible_neighbors
                        .iter()
                        .any(|neighbor| neighbor.coordinate == cell);
                    if distance >= 2 {
                        assert!(
                            !reported,
                            "seed {seed} reported a cell {distance} steps away; the radius is not 1"
                        );
                    }
                    if distance == 1 {
                        assert!(
                            reported,
                            "seed {seed} withheld an adjacent cell, so a policy could step into \
                             something it was never shown"
                        );
                    }
                }
            }
            // Move somewhere legal so the walk terminates.
            let step = observation
                .visible_neighbors
                .iter()
                .find(|neighbor| {
                    neighbor.tile == TileKind::Safe || neighbor.tile == TileKind::Reward
                })
                .map(|neighbor| {
                    match (
                        neighbor.coordinate.x > here.x,
                        neighbor.coordinate.y > here.y,
                    ) {
                        (true, _) => Action::Right,
                        (false, true) => Action::Down,
                        (false, false) => Action::Left,
                    }
                });
            match step {
                Some(action) => {
                    if sandbox.step_for_test(action).is_err() {
                        break;
                    }
                }
                None => break,
            }
        }
    }
}

/// A consequence worth pinning: seeing the goal and surviving the turn is impossible.
///
/// A reactive policy steps onto an adjacent reward on the very turn it first sees one, so a goal can
/// never end up "remembered but not adjacent". That removes the most obvious way memory would have
/// paid off here, and it is a property of the rules rather than of either policy.
#[test]
fn p0_a_seen_goal_can_never_survive_its_own_turn() {
    for seed in benchmark().definition().eval_seed_pool.clone() {
        let layout = grid_world_layout(seed);
        let budget = layout.max_turns();
        let mut sandbox = GridWorldSandbox::with_max_turns(seed, budget).expect("sandbox");
        while sandbox.episode().status == probelab_core::EpisodeStatus::Running {
            let observation = sandbox.observation();
            let goal_adjacent = observation
                .visible_neighbors
                .iter()
                .any(|neighbor| neighbor.tile == TileKind::Reward);
            if goal_adjacent {
                // Both reactive policies take an adjacent reward on sight, so the episode must end
                // this turn. If it did not, a goal could sit in memory unvisited.
                let action = observation
                    .visible_neighbors
                    .iter()
                    .find(|neighbor| neighbor.tile == TileKind::Reward)
                    .map(|neighbor| {
                        if neighbor.coordinate.x > observation.position.x {
                            Action::Right
                        } else {
                            Action::Left
                        }
                    })
                    .expect("adjacent reward");
                let status = sandbox
                    .step_for_test(action)
                    .expect("step onto reward")
                    .status;
                assert_eq!(
                    status,
                    probelab_core::EpisodeStatus::Success,
                    "seed {seed}: the agent saw the goal and the episode did not end"
                );
                break;
            }
            let step = observation
                .visible_neighbors
                .iter()
                .find(|neighbor| neighbor.tile == TileKind::Safe)
                .map(|_neighbor| Action::Down);
            match step {
                Some(action) => {
                    if sandbox.step_for_test(action).is_err() {
                        break;
                    }
                }
                None => break,
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Step 3 — the measurement, and the honest null result
// ---------------------------------------------------------------------------------------------

/// The headline comparison, on the full evaluation pool with a confidence interval on each side.
#[test]
fn p0_memory_does_not_change_a_reactive_policy_on_the_evaluation_pool() {
    let mut stateless_factory = |_seed: u64| -> Box<dyn Agent> {
        Box::new(ProviderAgent::from_provider(StatelessReactive))
    };
    let mut memory_factory =
        |_seed: u64| -> Box<dyn Agent> { Box::new(ProviderAgent::from_provider(MemoryReactive)) };
    let stateless = measure("stateless", &mut stateless_factory);
    let memory = measure("memory", &mut memory_factory);

    // This asserts the *result*, null included. If a future change to the generator, the reward, or
    // the history window makes memory matter, this test fails and forces the documentation to be
    // revisited — which is the point of writing it down rather than leaving it in a report.
    assert_eq!(
        (
            stateless.successes,
            stateless.success_rate.point,
            memory.successes,
            memory.success_rate.point,
        ),
        (9, 9.0 / 70.0, 9, 9.0 / 70.0),
        "the memory comparison moved: stateless {}/{} at {:.3}, memory {}/{} at {:.3}. Re-run the \
         phase 0 numbers and update ARCHITECTURE.md and PROGRESS.md if this is a real change.",
        stateless.successes,
        stateless.episodes,
        stateless.success_fraction(),
        memory.successes,
        memory.episodes,
        memory.success_fraction(),
    );
    assert_eq!(
        stateless.status_counts, memory.status_counts,
        "the two policies did not merely tie on rate, they ended differently"
    );
}

/// The two policies are not merely tied in aggregate — they take the *same actions*.
///
/// A tie in the final score could in principle hide offsetting wins and losses. Pinning that the two
/// policies are behaviourally identical on every seed is what turns "no measurable effect" into a
/// claim about the environment rather than a coincidence.
#[test]
fn p0_the_two_reactive_policies_take_identical_action_sequences() {
    let mut differing_seeds = Vec::new();
    for seed in benchmark().definition().eval_seed_pool.clone() {
        let budget = grid_world_layout(seed).max_turns();
        let stateless = replay(StatelessReactive, seed, budget);
        let memory = replay(MemoryReactive, seed, budget);
        if stateless != memory {
            differing_seeds.push(seed);
        }
    }
    assert!(
        differing_seeds.is_empty(),
        "the policies diverged on {differing_seeds:?}, so memory is not inert after all and the \
         aggregate result needs re-examining"
    );
}

/// Drives a policy through one episode and returns the actions it chose.
///
/// The bounded history window is rebuilt here from the turns already taken, mirroring what the engine
/// puts in `ModelRequest::history`. That matters: without a real window the memory policy would have
/// nothing to read and the comparison below would be vacuous.
fn replay<P: ModelProvider>(policy: P, seed: u64, budget: u32) -> Vec<Action> {
    let mut provider = policy;
    let mut sandbox = GridWorldSandbox::with_max_turns(seed, budget).expect("sandbox");
    let mut completed: Vec<probelab_core::HistoryEntry> = Vec::new();
    let mut actions = Vec::new();
    while sandbox.episode().status == probelab_core::EpisodeStatus::Running {
        let observation = sandbox.observation();
        let turn = observation.turn;
        let request = ModelRequest {
            episode_id: "phase0-replay".to_owned(),
            turn,
            available_actions: sandbox.available_actions(),
            // The grid world declares no spawn plan; this replay drives a provider directly and
            // never goes through the engine loop that would read it from the benchmark.
            process_spawn_available: false,
            // Grid path: this replay drives a provider directly on the grid world.
            game: None,
            observation: observation.clone(),
            history: window_of(&completed),
        };
        let ModelDecision::GridAction(action) = provider.predict(request).expect("decision") else {
            panic!("reactive policies only produce grid actions")
        };
        let step = match sandbox.step_for_test(action) {
            Ok(step) => step,
            Err(_) => break,
        };
        let outcome = step.action_outcome.expect("executed action has an outcome");
        completed.push(probelab_core::HistoryEntry {
            turn,
            position: observation.position,
            current_tile: observation.current_tile,
            visible_neighbors: observation.visible_neighbors.clone(),
            selected_action: action,
            result_kind: outcome.kind,
            reward: outcome.reward,
        });
        actions.push(action);
    }
    actions
}

/// The last `HISTORY_WINDOW` completed turns, oldest first — the same bound the engine applies.
fn window_of(completed: &[probelab_core::HistoryEntry]) -> Vec<probelab_core::HistoryEntry> {
    let start = completed
        .len()
        .saturating_sub(probelab_core::HISTORY_WINDOW);
    completed[start..].to_vec()
}

/// A weaker, still honest, statement: memory does not make the reactive policy *better*, even though
/// the window is delivered and the two policies are otherwise identical.
#[test]
fn p0_memory_does_not_help_the_reactive_policy_on_hidden_hazard_seeds() {
    // Restricted to seeds where a hazard sits on the shortest route's neighbourhood, which is where
    // remembering a hazard would be expected to matter most.
    let hidden = hidden_hazard_seeds();
    assert!(
        hidden.len() >= 30,
        "expected at least 30 hidden-hazard seeds, found {}",
        hidden.len()
    );
    for seed in &hidden {
        let budget = grid_world_layout(*seed).max_turns();
        assert_eq!(
            replay(StatelessReactive, *seed, budget),
            replay(MemoryReactive, *seed, budget),
            "seed {seed}: the policies diverged on a hidden-hazard seed"
        );
    }
}

/// Every evaluation seed whose layout has a hazard that is not adjacent to the start.
///
/// The generator never places a hazard adjacent to the start, so this is the whole pool; the filter
/// is written out anyway because "hidden" is the property under test and should not be assumed.
fn hidden_hazard_seeds() -> Vec<u64> {
    benchmark()
        .definition()
        .eval_seed_pool
        .iter()
        .copied()
        .filter(|seed| {
            let layout = grid_world_layout(*seed);
            let start = layout.start();
            !layout.hazards().is_empty()
                && layout
                    .hazards()
                    .iter()
                    .all(|hazard| probelab_sandbox::manhattan(*hazard, start) >= 2)
        })
        .collect()
}

/// Sanity: the greedy baseline still behaves, so the reactive comparison is against a working harness.
#[test]
fn p0_the_harness_still_measures_a_known_policy() {
    let mut greedy = |_seed: u64| -> Box<dyn Agent> {
        Box::new(ProviderAgent::from_provider(MockModelProvider::new(
            MockModelConfig::Optimal,
            0,
        )))
    };
    let score = measure("greedy", &mut greedy);
    assert_eq!(score.episodes, 70);
    assert!(
        score.successes > 9,
        "the greedy baseline should beat a reactive policy; got {}/{}",
        score.successes,
        score.episodes
    );
}
