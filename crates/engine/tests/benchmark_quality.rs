//! Proof that the grid-world benchmark measures something a one-step heuristic cannot.
//!
//! These tests are the deliverable of the benchmark redesign, and they are deliberately written to
//! fail if the benchmark regresses into something trivial. Each one names the property it defends and
//! what the property is worth.
//!
//! The seed pools used here are declared in `benchmarks/games/grid-world.v1.json` and validated when
//! that definition loads. `B5` re-asserts their disjointness here as well, because this is the file
//! that actually draws from them.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use probelab_agent::ProviderAgent;
use probelab_benchmarks::{GridWorldBenchmark, MINIMUM_EVAL_SEEDS};
use probelab_core::{
    Action, Agent, Coordinate, ModelRequest, Observation, Sandbox, TelemetryRecorder, TileKind,
    VisibleTile,
};
use probelab_engine::harness::{
    wilson_interval, AggregateScore, Harness, MeasurementPlan, SeedPool, SeedPools,
};
use probelab_engine::BenchmarkRunner;
use probelab_evaluator::GridWorldEvaluator;
use probelab_model_adapters::{LookaheadSolver, MockModelConfig, MockModelProvider};
use probelab_sandbox::{grid_world_layout, GridWorldError, GridWorldSandbox};
use probelab_telemetry::SqliteTelemetryRecorder;

/// A dev-pool seed, used wherever a test needs one specific world and is not measuring anything.
const DEV_SEED: u64 = 1_000;

fn benchmark() -> GridWorldBenchmark {
    GridWorldBenchmark::built_in().expect("built-in grid-world benchmark")
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

/// A process-wide counter so two harnesses built in the same test process never share a scratch
/// directory. Without it, two tests measuring the same policy label would open the same telemetry
/// database and the second would be rejected as a non-gap-free sequence.
static HARNESS_SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Every scratch directory this test binary created, removed on exit.
static SCRATCH_DIRECTORIES: std::sync::OnceLock<std::sync::Mutex<Vec<PathBuf>>> =
    std::sync::OnceLock::new();

fn scratch_directories() -> &'static std::sync::Mutex<Vec<PathBuf>> {
    SCRATCH_DIRECTORIES.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

fn harness() -> Harness {
    let serial = HARNESS_SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root =
        std::env::temp_dir().join(format!("probelab-quality-{}-{serial}", std::process::id()));
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

fn greedy_factory(_seed: u64) -> Box<dyn Agent> {
    Box::new(ProviderAgent::from_provider(MockModelProvider::new(
        MockModelConfig::Optimal,
        0,
    )))
}

fn random_factory(_seed: u64) -> Box<dyn Agent> {
    Box::new(ProviderAgent::from_provider(MockModelProvider::new(
        MockModelConfig::Random,
        0,
    )))
}

fn lookahead_factory(seed: u64) -> Box<dyn Agent> {
    Box::new(ProviderAgent::from_provider(LookaheadSolver::new(seed)))
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
// B1 — the map and the goal are per seed, and the turn budget is derived from the seed
// ---------------------------------------------------------------------------------------------

#[test]
fn b1_layouts_differ_across_seeds_and_are_identical_for_the_same_seed() {
    let mut goals = std::collections::BTreeSet::new();
    let mut starts = std::collections::BTreeSet::new();
    let mut layouts = std::collections::BTreeSet::new();
    let mut budgets = std::collections::BTreeSet::new();

    for seed in benchmark().definition().eval_seed_pool.clone() {
        let first = grid_world_layout(seed);
        let second = grid_world_layout(seed);
        assert_eq!(first, second, "seed {seed} is not deterministic");

        goals.insert((first.goal().x, first.goal().y));
        starts.insert((first.start().x, first.start().y));
        layouts.insert(encode(first.tiles()));
        budgets.insert(first.max_turns());
    }

    assert!(
        goals.len() > 1,
        "the goal never moves, so a model could hardcode a coordinate"
    );
    assert!(
        starts.len() > 1,
        "the start never moves, so a fixed movement preference stays accidentally correct"
    );
    assert!(
        layouts.len() > 1,
        "every seed produced an identical tile layout"
    );
    assert!(
        budgets.len() > 1,
        "every seed produced the same turn budget, so the budget is a constant in disguise"
    );
}

#[test]
fn b1_the_turn_budget_is_derived_from_the_seeds_own_shortest_safe_route() {
    let catalog = benchmark();
    let definition = catalog.definition();
    for seed in definition.eval_seed_pool.iter().take(25) {
        let layout = grid_world_layout(*seed);
        let budget = definition.max_turns_for(*seed);
        assert_eq!(
            budget,
            probelab_sandbox::turn_budget_for(layout.shortest_safe_path()),
            "seed {seed} budget is not the declared ratio applied to its own route"
        );
        assert!(
            budget >= layout.shortest_safe_path(),
            "seed {seed} cannot finish inside its own budget"
        );
        assert!(
            budget <= definition.max_turns,
            "seed {seed} exceeds the published catalog ceiling"
        );
    }
}

#[test]
fn b1_every_seed_is_solvable_and_offers_more_than_one_safe_neighbour_somewhere() {
    let catalog = benchmark();
    for seed in catalog.definition().eval_seed_pool.clone() {
        let layout = grid_world_layout(seed);
        assert!(
            layout.shortest_safe_path() > 0,
            "seed {seed} has no route to the goal"
        );
        assert!(
            !layout.hazards().is_empty(),
            "seed {seed} has no hazard, so nothing punishes a wrong turn"
        );
        // A branch is a safe cell with three or more safe neighbours. With no branch anywhere the
        // safe region is a path again, and the benchmark is back to being a corridor.
        let branches = layout
            .traversable_tiles()
            .into_iter()
            .filter(|cell| probelab_sandbox::neighbour_count(&layout, *cell) >= 3)
            .count();
        assert!(
            branches > 0,
            "seed {seed} has no branching safe cell, so the safe region is a bare path"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// B2 — hazards are hidden until the agent is adjacent
// ---------------------------------------------------------------------------------------------

#[test]
fn b2_a_hazard_not_adjacent_to_the_start_is_absent_until_the_agent_is_adjacent() {
    let mut checked = 0;
    for seed in benchmark().definition().eval_seed_pool.clone() {
        let layout = grid_world_layout(seed);
        let start = layout.start();
        for hazard in layout.hazards() {
            if probelab_sandbox::manhattan(hazard, start) < 2 {
                continue;
            }
            // Turn one: the hazard is not adjacent, so it must not appear anywhere in the payload.
            let mut sandbox = GridWorldSandbox::new(seed).expect("sandbox");
            let observation = sandbox.observation();
            assert!(
                !observation
                    .visible_neighbors
                    .iter()
                    .any(|neighbor| neighbor.coordinate == hazard),
                "seed {seed} leaked hazard {hazard:?} on turn one"
            );
            assert!(
                !observation
                    .visible_neighbors
                    .iter()
                    .any(|neighbor| neighbor.tile == TileKind::Danger),
                "seed {seed} showed a hazard on turn one despite starting away from all of them"
            );

            // Stand the agent next to it: now, and only now, it must be reported. A hazard walled in
            // on all four sides has no safe approach and cannot be observed from anywhere, so it is
            // not a case this contract can speak to.
            let Some(approach) = probelab_sandbox::orthogonal_coordinates(hazard)
                .into_iter()
                .find(|cell| *cell != start && matches!(layout.tile(*cell), TileKind::Safe))
            else {
                continue;
            };
            let approach_distance = layout.manhattan_to_goal(approach);
            sandbox.teleport_for_test(approach);
            let adjacent = sandbox.observation();
            assert!(
                adjacent
                    .visible_neighbors
                    .iter()
                    .any(|neighbor| neighbor.coordinate == hazard
                        && neighbor.tile == TileKind::Danger),
                "seed {seed} did not reveal hazard {hazard:?} once adjacent to it"
            );
            assert_eq!(
                adjacent.distance_to_goal, approach_distance,
                "the reported goal distance disagrees with the layout"
            );
            checked += 1;
            if checked >= 40 {
                return;
            }
        }
    }
    assert!(checked > 0, "no hidden hazard was found to test");
}

/// A hand-verified transcript for one seed, written out independently of the replay digest.
///
/// The digest fixtures prove a run is *reproducible*. They cannot prove it is *correct*: a
/// deterministically wrong run hashes just as consistently as a right one. So this test pins the
/// literal expected observation content, derived by reading the generator and the world rules, and a
/// change in behaviour has to be justified here rather than absorbed by regenerating a hash.
///
/// Seed 1000's layout, with the goal distance of every tile. The goal is `(4,4)`:
///
/// ```text
///        x=0  1   2   3   4      distance to (4,4)
///  y=0    !   #   !   #   #        8   7   6   5   4
///  y=1    #   S   .   #   #        7   6   5   4   3
///  y=2    .   .   .   #   #        6   5   4   3   2
///  y=3    .   .   .   .   #        5   4   3   2   1
///  y=4    .   .   .   .   R        4   3   2   1   0
/// ```
///
/// `S` is the start at `(1,1)`, `R` the goal, `#` an obstacle, `.` safe, `!` a hazard. Reading the
/// grid: the start is 6 from the goal, a 6-move route exists down the second column and then right
/// along the bottom, so the shortest safe path is 6 and the derived budget is `ceil(3/2 * 6) = 9`.
///
/// The two hazards sit at `(0,0)` and `(2,0)`. Both are 2 tiles from the start, so neither is
/// adjacent and neither may appear on turn one. That is the property under test below.
#[test]
fn b2_the_observation_content_of_a_hidden_hazard_seed_is_exactly_as_hand_computed() {
    let layout = grid_world_layout(DEV_SEED);
    assert_eq!(layout.start(), Coordinate { x: 1, y: 1 });
    assert_eq!(layout.goal(), Coordinate { x: 4, y: 4 });
    assert_eq!(layout.shortest_safe_path(), 6);
    assert_eq!(layout.max_turns(), 9);
    assert_eq!(
        layout.hazards(),
        vec![Coordinate { x: 0, y: 0 }, Coordinate { x: 2, y: 0 }],
        "the hand-written hazard list for this seed no longer matches the generator"
    );

    // Turn 1, standing on the start. Up and left are walls; down and right are safe. Neither hazard
    // appears, because both are two tiles away.
    assert_eq!(
        expected(
            1,
            Coordinate { x: 1, y: 1 },
            6,
            9,
            &[
                (1, 0, TileKind::Obstacle),
                (1, 2, TileKind::Safe),
                (0, 1, TileKind::Obstacle),
                (2, 1, TileKind::Safe),
            ]
        ),
        observed(DEV_SEED, 1),
        "turn 1 does not match the hand-computed observation"
    );

    // Turn 2, after stepping right to (2,1). The hazard at (2,0) is now directly above and is
    // reported. This is the first turn on which either hazard is visible at all.
    assert_eq!(
        expected(
            2,
            Coordinate { x: 2, y: 1 },
            5,
            8,
            &[
                (2, 0, TileKind::Danger),
                (2, 2, TileKind::Safe),
                (1, 1, TileKind::Safe),
                (3, 1, TileKind::Obstacle),
            ]
        ),
        observed(DEV_SEED, 2),
        "turn 2 does not match the hand-computed observation"
    );

    // Turn 3, back on the start tile. The hazard at (2,0) is *still a hazard* and is still two tiles
    // away, so it is correctly gone from the payload again. Nothing in the observation says it is
    // safe — it is simply not reported. Remembering it is the agent's job, and that is exactly what
    // the bounded history window and a remembered map are for.
    assert_eq!(
        expected(
            3,
            Coordinate { x: 1, y: 1 },
            6,
            7,
            &[
                (1, 0, TileKind::Obstacle),
                (1, 2, TileKind::Safe),
                (0, 1, TileKind::Obstacle),
                (2, 1, TileKind::Safe),
            ]
        ),
        observed(DEV_SEED, 3),
        "turn 3 does not match the hand-computed observation"
    );
}

/// The exact observation a scripted walk produces on one turn of seed 1000.
fn observed(seed: u64, target_turn: u32) -> Observation {
    // Right, then left: step under the hazard at (2,0) and come back, which is the smallest walk
    // that makes the reveal-then-hide sequence observable. The trailing wait exists only so the loop
    // below can reach turn 3; it is never executed.
    let script = [Action::Right, Action::Left, Action::Wait];
    let budget = probelab_sandbox::turn_budget_for(grid_world_layout(seed).shortest_safe_path());
    let mut sandbox = GridWorldSandbox::with_max_turns(seed, budget).expect("sandbox");
    for (index, action) in script.iter().enumerate() {
        if index as u32 + 1 == target_turn {
            return sandbox.observation();
        }
        sandbox.step_for_test(*action).expect("scripted movement");
    }
    panic!("turn {target_turn} is past the end of the scripted walk");
}

/// The exact observation a scripted walk produces on one turn of seed 1000.
fn expected(
    turn: u32,
    position: Coordinate,
    distance_to_goal: u32,
    remaining_turns: u32,
    neighbors: &[(u8, u8, TileKind)],
) -> Observation {
    Observation {
        turn,
        position,
        grid_size: probelab_core::GridSize {
            width: 5,
            height: 5,
        },
        current_tile: TileKind::Safe,
        visible_neighbors: neighbors
            .iter()
            .map(|(x, y, tile)| VisibleTile {
                coordinate: Coordinate { x: *x, y: *y },
                tile: *tile,
            })
            .collect(),
        distance_to_goal,
        max_turns: 9,
        remaining_turns,
        available_actions: vec![
            Action::Up,
            Action::Down,
            Action::Left,
            Action::Right,
            Action::Wait,
        ],
    }
}

// ---------------------------------------------------------------------------------------------
// B3 — shaped reward separates failures of different quality
// ---------------------------------------------------------------------------------------------

#[test]
fn b3_two_failing_runs_of_different_quality_score_differently() {
    // Collect the scores of *failing* runs only. A success is not evidence about how the score
    // separates failures, so those seeds are skipped rather than counted.
    let mut scores = std::collections::BTreeSet::new();
    for seed in benchmark().definition().eval_seed_pool.clone() {
        let mut sandbox = GridWorldSandbox::new(seed).expect("sandbox");
        while sandbox.episode().status == probelab_core::EpisodeStatus::Running {
            let observation = sandbox.observation();
            if sandbox.step_for_test(myopic(&observation)).is_err() {
                break;
            }
        }
        let episode = sandbox.episode();
        if episode.status == probelab_core::EpisodeStatus::Success {
            continue;
        }
        scores.insert(episode.total_reward);
        if scores.len() >= 3 {
            break;
        }
    }
    assert!(
        scores.len() >= 2,
        "every failing run scored identically ({scores:?}), so the score still carries one bit"
    );
}

#[test]
fn b3_a_run_that_makes_no_progress_scores_worse_than_one_that_gets_partway() {
    let catalog = benchmark();
    let seed = catalog.definition().eval_seed_pool[0];
    let mut idle = GridWorldSandbox::new(seed).expect("sandbox");
    while idle.episode().status == probelab_core::EpisodeStatus::Running {
        idle.step_for_test(Action::Wait).expect("wait");
    }
    assert_eq!(
        idle.episode().status,
        probelab_core::EpisodeStatus::TurnLimit
    );
    assert_eq!(
        idle.episode().total_reward,
        0,
        "a run that never moved should score zero"
    );

    // Walk one legal step toward the goal and then idle the rest away. Same terminal status, strictly
    // better score: this is exactly the distinction the old terminal-only reward could not express.
    let mut partial = GridWorldSandbox::new(seed).expect("sandbox");
    let start = partial.observation();
    // A neighbour that is safe *and* nearer than here is progress. The observation reports only the
    // agent's own distance, so "nearer" is derived by re-reading the distance after the step rather
    // than by knowing where the goal is.
    let toward = start
        .visible_neighbors
        .iter()
        .find(|neighbor| neighbor.tile == TileKind::Safe)
        .map(|neighbor| {
            (
                neighbor.coordinate,
                action_between(start.position, neighbor.coordinate),
            )
        });
    if let Some((_step, action)) = toward {
        if partial.step_for_test(action).is_ok() {
            let gained = partial.observation().distance_to_goal < start.distance_to_goal;
            while partial.episode().status == probelab_core::EpisodeStatus::Running {
                partial.step_for_test(Action::Wait).expect("wait");
            }
            assert_eq!(
                partial.episode().status,
                probelab_core::EpisodeStatus::TurnLimit
            );
            assert_ne!(
                partial.episode().status,
                probelab_core::EpisodeStatus::Success,
                "this case is meant to be a failure"
            );
            if gained {
                assert!(
                    partial.episode().total_reward > 0,
                    "making progress and then idling should outscore idling"
                );
            }
        }
    }
}

/// The action that moves from one cell to an orthogonally adjacent one.
fn action_between(from: Coordinate, to: Coordinate) -> Action {
    if to.x > from.x {
        Action::Right
    } else if to.x < from.x {
        Action::Left
    } else if to.y > from.y {
        Action::Down
    } else {
        Action::Up
    }
}

// ---------------------------------------------------------------------------------------------
// B4 — the history window is bounded
// ---------------------------------------------------------------------------------------------

#[test]
fn b4_the_history_window_is_bounded_and_drops_the_oldest_entries() {
    use probelab_core::HISTORY_WINDOW;
    assert_eq!(HISTORY_WINDOW, 8, "the documented window size changed");

    // An engine run drives the window; observe its length at each turn through a recording agent.
    let lengths = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorder = Arc::clone(&lengths);
    let agent = WindowProbe {
        inner: ProviderAgent::from_provider(MockModelProvider::new(MockModelConfig::Optimal, 0)),
        lengths: recorder,
    };
    let run = probelab_engine::BenchmarkRunner::new()
        .run(
            &benchmark(),
            Box::new(agent),
            &GridWorldEvaluator::new(),
            DEV_SEED,
            "window-probe",
            Arc::new(
                SqliteTelemetryRecorder::with_callback(
                    std::env::temp_dir()
                        .join(format!("window-probe-{}.sqlite3", std::process::id())),
                    |_| {},
                )
                .expect("recorder"),
            ),
        )
        .expect("window probe run");
    assert_eq!(run.telemetry_event_count, 6);

    let observed = lengths.lock().expect("lengths").clone();
    assert_eq!(
        observed.first().copied(),
        Some(0),
        "turn one has no history"
    );
    assert_eq!(observed.get(1).copied(), Some(1));
    // The window fills, then plateaus. It must never exceed the cap, whatever the episode length.
    for length in &observed {
        assert!(
            (*length as usize) <= HISTORY_WINDOW,
            "history grew to {length} entries, past the {HISTORY_WINDOW} cap"
        );
    }
    assert!(
        observed.windows(2).all(|pair| pair[1] >= pair[0]),
        "history shrank: {observed:?}"
    );
    assert_eq!(
        *observed.last().expect("at least one turn"),
        (HISTORY_WINDOW as u32).min(5),
        "the window did not fill as expected: {observed:?}"
    );
}

struct WindowProbe {
    inner: ProviderAgent,
    lengths: Arc<std::sync::Mutex<Vec<u32>>>,
}

impl Agent for WindowProbe {
    fn descriptor(&self) -> probelab_core::ModelDescriptor {
        self.inner.descriptor()
    }

    fn decide(&mut self, request: ModelRequest) -> Result<probelab_core::ModelDecision, String> {
        self.lengths
            .lock()
            .expect("lengths")
            .push(u32::try_from(request.history.len()).expect("history fits u32"));
        self.inner.decide(request)
    }
}

#[test]
fn b4_a_request_carrying_an_over_long_history_is_refused_rather_than_truncated() {
    use probelab_core::{HistoryEntry, HISTORY_WINDOW};
    let base = observed(DEV_SEED, 1);
    let entry = HistoryEntry {
        turn: 1,
        position: base.position,
        current_tile: base.current_tile,
        visible_neighbors: base.visible_neighbors.clone(),
        selected_action: Action::Wait,
        result_kind: probelab_core::ActionResultKind::Waited,
        reward: 0,
    };
    let mut request = ModelRequest {
        episode_id: "window".to_owned(),
        turn: 20,
        observation: Observation {
            turn: 20,
            remaining_turns: 1,
            max_turns: 20,
            ..base.clone()
        },
        available_actions: base.available_actions.clone(),
        history: Vec::new(),
    };
    request.history = (0..=HISTORY_WINDOW)
        .map(|index| HistoryEntry {
            turn: u32::try_from(index).expect("fits") + 1,
            ..entry.clone()
        })
        .collect();
    assert!(
        request.validate().is_err(),
        "an over-long history was accepted, so a caller could believe it supplied more context \
         than the model received"
    );

    request.history.truncate(HISTORY_WINDOW);
    assert!(
        request.validate().is_ok(),
        "a history at exactly the cap was refused: {:?}",
        request.validate()
    );
}

// ---------------------------------------------------------------------------------------------
// B5 — a statistical harness over disjoint seed pools
// ---------------------------------------------------------------------------------------------

#[test]
fn b5_the_declared_pools_are_disjoint_and_the_eval_pool_is_large_enough() {
    let catalog = benchmark();
    let definition = catalog.definition();
    let declared = pools();
    assert!(declared.are_disjoint());
    for seed in &definition.eval_seed_pool {
        assert!(
            !definition.dev_seed_pool.contains(seed),
            "seed {seed} is in both pools, so no eval number would be held out"
        );
    }
    assert!(definition.eval_seed_pool.len() >= MINIMUM_EVAL_SEEDS);
    assert!(!definition.dev_seed_pool.is_empty());

    // The harness refuses to be constructed from overlapping pools, so a measurement cannot be
    // taken on a contaminated pool even if a caller ignores the definition's own validation.
    assert!(SeedPools::new(vec![7], vec![7, 8]).is_err());
    assert!(SeedPools::new(vec![7], vec![8])
        .expect("disjoint pools")
        .are_disjoint());
}

#[test]
fn b5_a_measurement_reports_a_scalar_with_a_confidence_interval() {
    let score = measure("greedy", &mut greedy_factory);
    assert_eq!(
        score.episodes,
        benchmark().definition().eval_seed_pool.len()
    );
    assert!(
        score.episodes >= 30,
        "a measurement ran fewer than 30 episodes"
    );
    let rate = score.success_rate;
    assert!((0.0..=1.0).contains(&rate.point));
    assert!(rate.lower <= rate.point && rate.point <= rate.upper);
    assert!(
        rate.upper - rate.lower < 0.5,
        "the interval is too wide to distinguish anything: {rate:?}"
    );
    assert!(!score.status_counts.is_empty());
}

#[test]
fn b5_the_wilson_interval_is_well_behaved_at_the_boundaries() {
    // A zero-success and an all-success sample are exactly where a normal approximation collapses.
    let none = wilson_interval(0, 70, 0.95);
    assert_eq!(none.point, 0.0);
    assert!(none.lower >= 0.0 && none.upper > 0.0 && none.upper < 0.1);

    let all = wilson_interval(70, 70, 0.95);
    assert_eq!(all.point, 1.0);
    assert!(all.lower > 0.9 && all.lower < 1.0 && all.upper <= 1.0);

    // It narrows as evidence accumulates, which is the entire point of quoting an interval.
    let small = wilson_interval(1, 10, 0.95);
    let large = wilson_interval(100, 1_000, 0.95);
    assert!(large.upper - large.lower < small.upper - small.lower);
    assert_eq!(wilson_interval(0, 0, 0.95).point, 0.0);
}

#[test]
fn b5_every_policy_is_measured_through_the_same_harness_and_telemetry() {
    let random = measure("random", &mut random_factory);
    let greedy = measure("greedy", &mut greedy_factory);
    let lookahead = measure("lookahead", &mut lookahead_factory);
    for score in [&random, &greedy, &lookahead] {
        assert_eq!(score.episodes, greedy.episodes);
        assert!(score.success_rate.upper <= 1.0);
    }
    // A floor must exist. If uniform-random walking solved the benchmark, nothing is being measured.
    assert!(
        random.success_fraction() < greedy.success_fraction(),
        "random walking ({}) is not worse than the greedy baseline ({})",
        random.success_fraction(),
        greedy.success_fraction()
    );
}

// ---------------------------------------------------------------------------------------------
// B6 — the falsifiable proof
// ---------------------------------------------------------------------------------------------

/// The proof this milestone exists to produce.
///
/// It establishes a **gap** between a one-step-lookahead policy and a search-based one, with
/// confidence intervals that do not overlap. If this test passes, the benchmark is no longer
/// solvable by a trivial heuristic, and any score above the greedy baseline reflects something the
/// greedy baseline does not do.
///
/// Stated plainly, this test does **not** show that any real model reasons well. It shows the
/// instrument has a measurable range. Filling that range is the next step, and it needs actual
/// models run through [`measure`].
#[test]
fn b6_greedy_is_beaten_by_a_search_policy_with_disjoint_intervals() {
    let greedy = measure("greedy", &mut greedy_factory);
    let lookahead = measure("lookahead", &mut lookahead_factory);

    assert!(
        lookahead.successes > greedy.successes,
        "the search policy did not beat the one-step policy ({} vs {} of {})",
        lookahead.successes,
        greedy.successes,
        greedy.episodes
    );
    assert!(
        greedy.success_rate.upper < lookahead.success_rate.lower,
        "the confidence intervals overlap, so the gap is not established: greedy {:?} vs \
         lookahead {:?}",
        greedy.success_rate,
        lookahead.success_rate
    );
    assert!(
        greedy.successes * 2 <= greedy.episodes,
        "the one-step policy solved more than half the pool ({} of {}), so the benchmark is still \
         close to trivial",
        greedy.successes,
        greedy.episodes
    );
    assert!(
        lookahead.successes * 10 >= lookahead.episodes * 9,
        "the search policy solved only {} of {}; the reference solver is not a valid ceiling",
        lookahead.successes,
        lookahead.episodes
    );
}

/// The same gap, asserted on the dev pool.
///
/// The eval-pool test above is the one that counts. This one exists so that a future edit which
/// quietly tunes the generator against the evaluation pool still trips a test, because the dev pool
/// is where tuning is supposed to happen and it is not supposed to be *this* lopsided by accident.
#[test]
fn b6_the_gap_is_also_present_on_the_dev_pool() {
    let harness = harness();
    let plan = MeasurementPlan {
        pool: SeedPool::Dev,
        episodes: benchmark().definition().dev_seed_pool.len(),
        confidence: 0.95,
    };
    let greedy = harness
        .measure(
            &benchmark(),
            &GridWorldEvaluator::new(),
            &BenchmarkRunner::new(),
            &plan,
            "greedy-dev",
            &mut greedy_factory,
        )
        .expect("dev greedy measurement");
    let lookahead = harness
        .measure(
            &benchmark(),
            &GridWorldEvaluator::new(),
            &BenchmarkRunner::new(),
            &plan,
            "lookahead-dev",
            &mut lookahead_factory,
        )
        .expect("dev lookahead measurement");
    assert!(
        lookahead.successes > greedy.successes,
        "dev pool: search {} vs one-step {}",
        lookahead.successes,
        greedy.successes
    );
}

/// A guard on the harness itself: the pools really are different sets of worlds, so a "held out"
/// claim means something.
#[test]
fn b5_the_two_pools_describe_genuinely_different_worlds() {
    let catalog = benchmark();
    let definition = catalog.definition();
    let dev_layouts: BTreeMap<String, u64> = definition
        .dev_seed_pool
        .iter()
        .map(|seed| (encode(grid_world_layout(*seed).tiles()), *seed))
        .collect();
    let shared = definition
        .eval_seed_pool
        .iter()
        .filter(|seed| dev_layouts.contains_key(&encode(grid_world_layout(**seed).tiles())))
        .count();
    assert_eq!(
        shared, 0,
        "an eval seed reproduced a dev layout, so the pools are not independent worlds"
    );
}

/// Sanity check that the sandbox constructor still refuses a nonsense budget.
#[test]
fn b1_a_zero_turn_budget_is_refused() {
    assert!(matches!(
        GridWorldSandbox::with_max_turns(DEV_SEED, 0),
        Err(GridWorldError::ZeroMaxTurns)
    ));
}

/// The myopic one-step policy used by the B3 assertions, kept local so this file does not depend on
/// the sandbox crate's own test helpers.
fn myopic(observation: &Observation) -> Action {
    let here = observation.position;
    let targets = |action: Action, tile: TileKind| {
        observation.visible_neighbors.iter().any(|neighbor| {
            neighbor.tile == tile
                && match action {
                    Action::Up => {
                        neighbor.coordinate.x == here.x && neighbor.coordinate.y + 1 == here.y
                    }
                    Action::Down => {
                        neighbor.coordinate.x == here.x && here.y + 1 == neighbor.coordinate.y
                    }
                    Action::Left => {
                        neighbor.coordinate.y == here.y && neighbor.coordinate.x + 1 == here.x
                    }
                    Action::Right => {
                        neighbor.coordinate.y == here.y && here.x + 1 == neighbor.coordinate.x
                    }
                    Action::Wait => false,
                }
        })
    };
    for action in [Action::Right, Action::Down] {
        if targets(action, TileKind::Reward) {
            return action;
        }
    }
    for action in [Action::Down, Action::Right, Action::Up, Action::Left] {
        if targets(action, TileKind::Safe) {
            return action;
        }
    }
    Action::Wait
}

/// A comparable, orderable encoding of a tile row.
///
/// `TileKind` deliberately does not implement `Ord` — the domain has no use for ordering terrain —
/// so a test that wants to compare two layouts as map keys formats them instead.
fn encode(tiles: &[TileKind; 25]) -> String {
    tiles
        .iter()
        .map(|tile| match tile {
            TileKind::Safe => '.',
            TileKind::Obstacle => '#',
            TileKind::Danger => '!',
            TileKind::Reward => 'R',
        })
        .collect()
}
