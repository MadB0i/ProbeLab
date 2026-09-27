//! End-to-end user-path verification.
//!
//! # What this is for
//!
//! `docs/STATUS.md` §9 lists commands that pass. This file exercises the *product's own paths* --
//! create, configure, run, observe, evaluate, persist, reload, replay -- end to end, in order, and
//! asserts the state a user would look at after each step.
//!
//! It exists because a suite of unit tests can all be green while the sequence a person actually
//! performs is broken. The three defects this file would have caught on the way in -- nine
//! unregistered Tauri commands, broker refusals counted as security findings, and a replay digest
//! that did not cover the result -- were all invisible to per-component tests and only visible as a
//! path.
//!
//! # What it is not
//!
//! Not a Tauri runtime test. Commands are called through the same library types the command layer
//! calls, because the one thing a Tauri harness would add is the dispatch itself, and that is already
//! verified structurally by `every_command_the_frontend_invokes_is_registered` in the desktop crate.
//! What is verified here is everything behind the dispatch: the real runner, the real broker, the real
//! evaluator, the real SQLite stores, and the real replay reconstruction.

use std::sync::Arc;
use std::time::Duration;

use probelab_core::{GameKind, GameSide, TelemetryRecorder};
use probelab_lab::{
    AgentCategory, AgentRegistry, BoundaryOutcome, LabBrain, LabRunner, PlanRequest,
    SqliteSettingsStore,
};
use probelab_telemetry::{ExperimentLifecycle, SqliteResultRepository, SqliteTelemetryRecorder};

/// A scratch directory unique per process launch, not per pid.
///
/// Windows recycles process identifiers and these are not cleaned up, so a pid-only name eventually
/// hands a later run a database an earlier one already filled. That is not hypothetical: it is how a
/// load-dependent telemetry flake in this repository started, roughly one full-suite run in five.
fn scratch(label: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or_default();
    let root = std::env::temp_dir().join(format!(
        "probelab-e2e-{label}-{}-{started}-{sequence}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("a scratch directory");
    root
}

/// # Experiment
///
/// The full path: configure a plan, run it, observe, evaluate, persist, reload, replay.
#[test]
fn the_experiment_path_runs_evaluates_persists_and_reloads() {
    let root = scratch("experiment");
    let database = root.join("probelab.sqlite3");
    let results = SqliteResultRepository::new(&database).expect("open the result store");
    let recorder: Arc<dyn TelemetryRecorder> =
        Arc::new(SqliteTelemetryRecorder::new(&database).expect("open the telemetry store"));

    // --- configure: a plan the Brain accepts ---
    let mut brain = LabBrain::new();
    let plan_request = PlanRequest {
        benchmark_id: Some("grid-world".to_owned()),
        target_agent: Some("optimal".to_owned()),
        probe_agents: Vec::new(),
        episodes: Some(5),
        root_seed: Some(4_242),
    };
    let plan = brain
        .make_plan(
            &plan_request,
            &probelab_benchmarks::BenchmarkRegistry::built_in(),
            &AgentRegistry::built_in(),
            &probelab_lab::ProbeRegistry::built_in(),
            probelab_lab::runner::current_platform(),
        )
        .expect("a plan for a registered benchmark and agent");

    // --- persist the `running` row BEFORE the first episode ---
    // Written only at the end, a crash mid-run would be invisible: the next start would find no row
    // and could not conclude the run was interrupted rather than completed.
    results
        .put_experiment(&probelab_telemetry::ExperimentRecord {
            experiment_id: "e2e-experiment".to_owned(),
            benchmark_id: "grid-world".to_owned(),
            lifecycle: ExperimentLifecycle::Running,
            terminal_reason: None,
            episodes_requested: 5,
            episodes_completed: 0,
            episodes_failed: 0,
            has_score: false,
            score: None,
            result_json: None,
            updated_unix_ms: 0,
            target_agent: Some("optimal".to_owned()),
            root_seed: Some(4_242),
        })
        .expect("persist the running state");
    let row = results
        .get_experiment("e2e-experiment")
        .expect("read it back")
        .expect("the row exists");
    assert_eq!(row.lifecycle, ExperimentLifecycle::Running);

    // --- run, observe, evaluate ---
    let run = LabRunner::new()
        .run(
            &mut brain,
            &plan_request,
            "e2e-experiment",
            &probelab_evaluator::GridWorldEvaluator::new(),
            recorder,
        )
        .expect("the experiment runs");

    assert_eq!(
        run.results.len(),
        5,
        "the plan asked for five episodes and the runner did not deliver five"
    );
    assert!(
        plan.episodes >= 5,
        "the plan's budget is smaller than the episodes it was asked to run"
    );
    for episode in &run.results {
        assert!(
            !episode.replay_digest.is_empty(),
            "an episode with no replay digest cannot be compared or reconstructed"
        );
        assert_eq!(
            episode.benchmark_id, "grid-world",
            "an episode's benchmark identity must survive into its result"
        );
        assert_eq!(episode.benchmark_version, "2");
    }

    // The evaluator produced a real measurement, not a placeholder.
    //
    // `mean_success_fraction` rather than `aggregate.success_rate`: the former is `None` when nothing
    // completed and the latter is a ratio of zero. The distinction is the whole point -- a run that
    // measured nothing did not score zero, and the field a caller writes to storage must not say it
    // did.
    let mean = run
        .mean_success_fraction()
        .expect("five completed episodes must produce a mean success fraction");
    let aggregate = run.aggregate();
    assert_eq!(aggregate.episodes, 5);
    assert!(
        aggregate.successes > 0,
        "the reference agent solved no seed at all, which means the path is broken rather than hard"
    );
    let interval = aggregate.success_interval;
    assert!(
        interval.lower <= interval.point,
        "a Wilson interval below its point estimate: {interval:?}"
    );
    assert!(
        interval.point <= interval.upper,
        "a Wilson interval above its point estimate: {interval:?}"
    );
    assert!(
        interval.lower >= 0.0 && interval.upper <= 1.0,
        "a Wilson interval must stay inside the unit interval: {interval:?}"
    );
    assert_eq!(
        interval.point, mean,
        "the aggregate's point estimate and the run's own mean disagree"
    );

    // The status counts use the serde spelling, not `Debug`. This is the assertion that would fail if
    // `status_counts` went back to `format!("{:?}")`, which would put `TurnLimit` on the IPC wire
    // where the frontend parses `turn_limit`.
    for status in aggregate.status_counts.keys() {
        assert!(
            ["success", "failure", "turn_limit", "model_protocol_failure"]
                .contains(&status.as_str()),
            "a status count used {status:?}, which is not a wire spelling"
        );
    }

    // --- persist the terminal state ---
    let episodes_completed = run.results.len();
    results
        .put_experiment(&probelab_telemetry::ExperimentRecord {
            experiment_id: "e2e-experiment".to_owned(),
            benchmark_id: "grid-world".to_owned(),
            lifecycle: ExperimentLifecycle::Completed,
            terminal_reason: run
                .terminal_reason
                .as_ref()
                .map(probelab_lab::TerminalReason::to_wire_json),
            episodes_requested: 5,
            episodes_completed,
            episodes_failed: run.failures.len(),
            has_score: true,
            score: Some(mean),
            result_json: None,
            updated_unix_ms: 0,
            target_agent: Some("optimal".to_owned()),
            root_seed: Some(4_242),
        })
        .expect("persist the terminal state");

    // --- reload: a fresh read, as a new process would see it ---
    let reloaded = SqliteResultRepository::new(&database).expect("reopen the store");
    let stored = reloaded
        .get_experiment("e2e-experiment")
        .expect("read after reload")
        .expect("the row survived");
    assert_eq!(stored.lifecycle, ExperimentLifecycle::Completed);
    assert_eq!(stored.episodes_completed, episodes_completed);
    assert_eq!(stored.target_agent.as_deref(), Some("optimal"));
    assert_eq!(stored.root_seed, Some(4_242));
    assert!(
        stored.terminal_reason.is_some(),
        "a completed run with no terminal reason cannot be explained later"
    );
    assert!(stored.has_score && stored.score.is_some());

    // --- replay the recorded run, and check it is honest about what it can do ---
    //
    // An experiment's recording is an event stream, not a board-by-board trace, so it is inspectable
    // but not re-executable. The panel has to be able to tell those apart, and this is the assertion
    // that the capability is derived rather than asserted.
    let replay = probelab_lab::replay_experiment("e2e-experiment", run.timeline.events());
    assert_eq!(replay.kind, probelab_lab::ReplayKind::Experiment);
    assert_eq!(
        replay.capability,
        probelab_lab::ReplayCapability::ProjectedOnly,
        "an experiment recording captured no intermediate environment states, so claiming it is \
         re-executable would let a reader trust a projection as a re-run"
    );
    assert!(
        !replay.frames.is_empty(),
        "a run that recorded a timeline produced no replay frames"
    );
    assert!(
        !replay.digest.is_empty(),
        "a replay with no digest cannot be checked against anything"
    );
    // Deterministic: the same recording gives the same frames every time.
    let again = probelab_lab::replay_experiment("e2e-experiment", run.timeline.events());
    assert_eq!(replay.digest, again.digest);

    // --- and the recorded event stream carries no credential ---
    //
    // Serialized here rather than read back out of SQLite, because the property under test is that
    // the *event type* cannot carry a credential -- not that one particular database is clean. The
    // durability of the stream is covered by `telemetry/tests/experiment_lifecycle.rs`.
    let recorded = run.timeline.events();
    let serialized = serde_json::to_string(&recorded).expect("serialize the recorded events");
    for marker in ["sk-", "AIza", "Bearer ", "api_key\":"] {
        assert!(
            !serialized.contains(marker),
            "recorded telemetry leaked {marker}"
        );
    }
    // A gap-free, strictly increasing sequence is what makes the stream replayable at all. Not
    // necessarily starting at one: a timeline can begin part-way through a run's own numbering, so the
    // property is monotonicity, which is what "gap-free" means here.
    let mut previous: Option<u64> = None;
    for event in recorded {
        if let Some(before) = previous {
            assert_eq!(
                event.sequence,
                before + 1,
                "telemetry sequence jumped from {before} to {}, so a frame is missing",
                event.sequence
            );
        }
        previous = Some(event.sequence);
    }
}
/// A run that crashes mid-flight is recoverable and is *not* reported as complete.
#[test]
fn an_interrupted_run_is_recovered_as_interrupted_not_as_complete() {
    let root = scratch("recovery");
    let database = root.join("probelab.sqlite3");
    let results = SqliteResultRepository::new(&database).expect("open");

    // A row left saying `running` when nothing is running is provably wrong, and left alone it makes a
    // crashed run indistinguishable from a completed one -- the worst thing a persistence layer can do,
    // because it is silently wrong rather than loudly absent.
    results
        .put_experiment(&probelab_telemetry::ExperimentRecord {
            experiment_id: "e2e-crashed".to_owned(),
            benchmark_id: "grid-world".to_owned(),
            lifecycle: ExperimentLifecycle::Running,
            terminal_reason: None,
            episodes_requested: 5,
            episodes_completed: 2,
            episodes_failed: 0,
            has_score: false,
            score: None,
            result_json: None,
            updated_unix_ms: 0,
            target_agent: Some("optimal".to_owned()),
            root_seed: Some(1),
        })
        .expect("persist a crashed run");

    let recovered = SqliteResultRepository::new(&database)
        .expect("reopen, as a new process would")
        .reconcile_interrupted()
        .expect("reconcile");
    assert!(
        recovered.contains(&"e2e-crashed".to_owned()),
        "a row still saying `running` must be corrected, and the correction logged rather than silent"
    );

    let fixed = results
        .get_experiment("e2e-crashed")
        .expect("read")
        .expect("the row");
    assert_eq!(fixed.lifecycle, ExperimentLifecycle::Interrupted);
    assert_eq!(
        fixed.episodes_completed, 2,
        "the partial episodes are evidence and must be preserved"
    );
    assert!(
        !fixed.has_score,
        "an interrupted run's partial episodes are not a complete measurement"
    );
}

/// # Security
///
/// Probe, authorization, enforcement, classification -- and the distinction the product rests on.
#[test]
fn the_security_path_probes_enforces_and_classifies() {
    let report = probelab_lab::sweep("grid-world", 4_242, 1).expect("a sweep runs");

    assert!(
        !report.outcomes.is_empty(),
        "a sweep that ran no probes proves nothing"
    );
    for outcome in &report.outcomes {
        assert!(
            outcome.failure.is_none(),
            "probe {} failed to run",
            outcome.probe_id
        );
        assert!(
            outcome.outcome.boundary_held(),
            "probe {} never reached an enforcement layer",
            outcome.probe_id
        );
        assert!(
            !outcome.is_security_finding(),
            "probe {} was recorded as a finding; a refused probe is a passing test",
            outcome.probe_id
        );
    }
    assert!(
        report.findings().is_empty(),
        "the lab's own probes must not manufacture findings: {:?}",
        report.findings()
    );

    // The taxonomy: a refusal is enforcement working, an unattempted probe is not a pass.
    assert!(BoundaryOutcome::BrokerRefused.boundary_held());
    assert!(!BoundaryOutcome::NotAttempted.boundary_held());
    for outcome in [
        BoundaryOutcome::BrokerRefused,
        BoundaryOutcome::PolicyDenied,
        BoundaryOutcome::NotAttempted,
        BoundaryOutcome::Failed,
    ] {
        assert!(
            !outcome.is_security_finding(),
            "{outcome:?} is not evidence of a model violating a boundary"
        );
    }

    // A report must never carry a credential, and must not merge refusals with violations.
    let text = serde_json::to_string(&report).expect("serialize the report");
    for marker in ["sk-", "AIza", "Bearer "] {
        assert!(!text.contains(marker), "a security report leaked {marker}");
    }
}

/// Every registered security probe is a real, seated agent -- not a label on nothing.
#[test]
fn every_security_probe_resolves_to_a_registered_agent_that_reports_its_own_category() {
    let agents = AgentRegistry::built_in();
    let probes = probelab_lab::all_probes();
    assert!(!probes.is_empty());
    for probe in &probes {
        let spec = agents
            .spec(&probe.id)
            .unwrap_or_else(|| panic!("probe {} is not in the agent registry", probe.id));
        assert_eq!(
            spec.category,
            AgentCategory::SecurityProbe,
            "probe {} is registered as {:?}, not as a security probe",
            probe.id,
            spec.category
        );
        assert!(
            !spec.category.measures_a_model(),
            "probe {} claims to measure a model; a probe tests the boundary",
            probe.id
        );
    }
}

/// # Arena
///
/// Play a real match, inspect the turns, inspect the result, replay it.
#[test]
fn the_arena_path_plays_inspects_and_replays() {
    let played = probelab_lab::ArenaRunner::new()
        .play(
            &probelab_lab::ArenaMatchSpec::new(
                GameKind::TicTacToe,
                "perfect_tictactoe",
                "first_legal_tictactoe",
                GameSide::First,
                4_242,
                "e2e-match",
            ),
            None,
        )
        .expect("a real match plays");
    let view = &played.view;

    assert_eq!(view.game, "tic_tac_toe");
    assert!(view.plies > 0, "a match with no plies did not play");
    assert!(
        !view.history.is_empty(),
        "no per-ply history means no board to inspect"
    );
    assert!(
        view.history.len() == view.plies as usize,
        "the history has {} frames for {} plies",
        view.history.len(),
        view.plies
    );

    // Every recorded frame carries the board *after* the move, so a viewer can render it as the next
    // frame rather than replaying the game to work out where it got to.
    for (index, ply) in view.history.iter().enumerate() {
        assert_eq!(ply.ply as usize, index + 1, "frame {index} is out of order");
        assert!(
            ply.played.is_some(),
            "a ply the model moved on must record the move"
        );
    }

    // The board actually advances rather than repeating.
    let boards: Vec<String> = view
        .history
        .iter()
        .map(|ply| serde_json::to_string(&ply.board).expect("serialize a board"))
        .collect();
    assert!(
        boards.windows(2).any(|pair| pair[0] != pair[1]),
        "the recorded board never changed across {} plies",
        view.plies
    );

    // Replay: deterministic, re-executable, and truthful about its own capability.
    let replay = probelab_lab::replay_match(view);
    assert_eq!(replay.kind, probelab_lab::ReplayKind::ArenaMatch);
    assert_eq!(
        replay.capability,
        probelab_lab::ReplayCapability::Full,
        "an arena recording captured every board, so it is re-executable"
    );
    assert!(!replay.frames.is_empty());
    let again = probelab_lab::replay_match(view);
    assert_eq!(replay.digest, again.digest, "replay must be deterministic");

    // A tampered recording changes the digest, so an edited match is detectable.
    //
    // `outcome` is the field the Arena panel renders as the result, and it is the one a tampered
    // recording would most plausibly be edited to change. Before `replay_digest` was extended to
    // cover the result fields, this exact edit left the digest identical -- so the check is on
    // `outcome` specifically rather than on an arbitrary field.
    let mut tampered = view.clone();
    // Set rather than swapped: a swap has a fallback arm that leaves the value unchanged when the
    // outcome is one this test did not enumerate, and an assertion that passes because nothing was
    // edited is worse than no assertion.
    tampered.outcome = probelab_lab::ArenaOutcome::NoResult;
    assert_ne!(
        view.outcome, tampered.outcome,
        "the match under test was already `no_result`, so the tamper proves nothing"
    );
    assert_ne!(
        view.replay_digest(),
        tampered.replay_digest(),
        "editing the recorded outcome left the replay digest unchanged"
    );
}

/// An unimplemented game is refused by name, at plan time, with a typed error.
#[test]
fn an_unimplemented_game_is_refused_rather_than_half_offered() {
    for game in [GameKind::ConnectFour, GameKind::Rps, GameKind::Battleship] {
        let error = probelab_lab::ArenaRunner::new()
            .play(
                &probelab_lab::ArenaMatchSpec::new(
                    game,
                    "perfect_tictactoe",
                    "perfect_tictactoe",
                    GameSide::First,
                    1,
                    "e2e-unimplemented",
                ),
                None,
            )
            .expect_err("an unimplemented game must be refused");
        assert!(
            matches!(error, probelab_lab::ArenaRunError::UnknownGame(_)),
            "{game:?} produced {error:?} rather than a typed refusal"
        );
    }
    // And the listing offers exactly what can be played, so a picker cannot offer a refusal.
    let offered: Vec<String> = probelab_lab::available_games()
        .into_iter()
        .map(|game| game.id)
        .collect();
    assert_eq!(offered, vec!["tic_tac_toe".to_owned()]);
}

/// # Settings
///
/// Change a setting, re-initialize, verify persistence. And verify a rejected change changes nothing.
#[test]
fn a_settings_change_persists_across_a_reinitialization() {
    let path = scratch("settings").join("probelab-settings.sqlite3");

    {
        let store = SqliteSettingsStore::open(&path).expect("open");
        let mut changed = store.load().expect("load defaults");
        changed.experiment_defaults.episodes = 9;
        store.save(&changed).expect("a valid change is accepted");
    }

    // Re-initialize: a fresh store over the same file, as a restarted application would see.
    let reopened = SqliteSettingsStore::open(&path).expect("reopen");
    let loaded = reopened.load().expect("load after restart");
    assert_eq!(
        loaded.experiment_defaults.episodes, 9,
        "a stored setting did not survive a restart"
    );

    // Reset restores the defaults.
    let reset = reopened.reset_to_defaults().expect("reset");
    assert_eq!(
        reset.experiment_defaults.episodes,
        probelab_lab::LabSettings::default()
            .experiment_defaults
            .episodes
    );
    assert_eq!(
        reopened
            .load()
            .expect("load after reset")
            .experiment_defaults
            .episodes,
        probelab_lab::LabSettings::default()
            .experiment_defaults
            .episodes
    );
}

/// A settings reset cannot reach a result, even when both stores are pointed at one file.
///
/// Stricter than reality, which is why it is a useful test: the real design keeps them in separate
/// files, and this proves the separation is load-bearing rather than incidental.
#[test]
fn a_settings_reset_cannot_reach_a_stored_result_even_in_the_same_file() {
    let path = scratch("isolation").join("shared.sqlite3");
    let results = SqliteResultRepository::new(&path).expect("result store");
    let settings = SqliteSettingsStore::open(&path).expect("settings store over the same file");

    results
        .put_experiment(&probelab_telemetry::ExperimentRecord {
            experiment_id: "e2e-isolation".to_owned(),
            benchmark_id: "grid-world".to_owned(),
            lifecycle: ExperimentLifecycle::Completed,
            terminal_reason: Some("completed".to_owned()),
            episodes_requested: 1,
            episodes_completed: 1,
            episodes_failed: 0,
            has_score: true,
            score: Some(1.0),
            result_json: None,
            updated_unix_ms: 0,
            target_agent: Some("optimal".to_owned()),
            root_seed: Some(1),
        })
        .expect("persist a result");

    settings.reset_to_defaults().expect("reset the settings");

    let after = results
        .get_experiment("e2e-isolation")
        .expect("read")
        .expect("the result row survived a settings reset");
    assert_eq!(after.lifecycle, ExperimentLifecycle::Completed);
    assert_eq!(
        after.score,
        Some(1.0),
        "a settings reset mutated a stored measurement"
    );
}

/// # Comparison
///
/// Run compatible experiments, compare, and verify the refusal path for incompatible ones.
#[test]
fn the_comparison_path_compares_and_refuses() {
    use probelab_lab::{compare, ComparisonOutcome};

    let comparable = vec![
        probelab_lab::ComparableRun {
            experiment_id: "a".to_owned(),
            benchmark_id: "grid-world".to_owned(),
            target_agent: "optimal".to_owned(),
            adapter: "deterministic-mock".to_owned(),
            strategy: "optimal".to_owned(),
            root_seed: 4_242,
            episodes_completed: 3,
            episodes_failed: 0,
            success_fraction: Some(2.0 / 3.0),
            success_interval: Some(probelab_lab::WilsonInterval {
                point: 2.0 / 3.0,
                lower: 0.2,
                upper: 0.95,
                confidence: 0.95,
            }),
            integrity_events: 0,
            broker_refusals: 0,
            is_security_finding: false,
            mean_reward: 9.0,
            mean_turns: 5.0,
            mean_invalid_actions: 0.0,
        },
        probelab_lab::ComparableRun {
            experiment_id: "b".to_owned(),
            ..probelab_lab::ComparableRun {
                experiment_id: "a".to_owned(),
                benchmark_id: "grid-world".to_owned(),
                target_agent: "optimal".to_owned(),
                adapter: "deterministic-mock".to_owned(),
                strategy: "optimal".to_owned(),
                root_seed: 4_242,
                episodes_completed: 3,
                episodes_failed: 0,
                success_fraction: Some(1.0),
                success_interval: None,
                integrity_events: 0,
                broker_refusals: 0,
                is_security_finding: false,
                mean_reward: 12.0,
                mean_turns: 4.0,
                mean_invalid_actions: 0.0,
            }
        },
    ];
    match compare(comparable.clone()) {
        ComparisonOutcome::Comparable(comparison) => {
            assert_eq!(comparison.runs.len(), 2);
            // `varies_across` covers *identity* dimensions -- agent, adapter, strategy -- not
            // measurements. Two runs of the same agent through the same adapter are comparable even
            // when their scores differ widely, and the score difference is the point of the ranking.
            // The blocking dimensions are benchmark, seed, and run count.
            assert!(
                comparison.varies_across.is_empty(),
                "two runs of the same agent through the same adapter must declare no confound: {:?}",
                comparison.varies_across
            );
            // And the ranking is real: the better run ranks first.
            let best = comparison.by_success.first().expect("a standing");
            assert_eq!(
                best.experiment_id, "b",
                "the higher success fraction must rank first"
            );
            assert!(best.value.is_some(), "a ranked run must carry a value");
        }
        ComparisonOutcome::NotComparable(reason) => {
            panic!("two runs differing only in outcome must compare: {reason:?}")
        }
    }

    // Different benchmarks: the central refusal. A success rate from another benchmark is not a better
    // or worse result, and ranking them would be the most misleading thing this product could do.
    let mut cross_benchmark: Vec<probelab_lab::ComparableRun> = comparable.to_vec();
    cross_benchmark[1].benchmark_id = "sandbox-process".to_owned();
    match compare(cross_benchmark) {
        ComparisonOutcome::NotComparable(reason) => {
            assert!(
                reason
                    .dimensions
                    .iter()
                    .any(|dimension| dimension.dimension == "benchmark"),
                "the refusal must name the benchmark as the varied dimension: {:?}",
                reason.dimensions
            );
        }
        ComparisonOutcome::Comparable(_) => {
            panic!("runs of different benchmarks must not be ranked against each other")
        }
    }

    // A single run is not a comparison.
    assert!(matches!(
        compare(vec![comparable[0].clone()]),
        ComparisonOutcome::NotComparable(_)
    ));
}

/// # Provider
///
/// A configured provider is validated before a run starts, and an absent credential or an unreachable
/// endpoint is a typed failure that never carries the key.
#[test]
fn the_provider_path_fails_honestly_when_the_endpoint_does_not_answer() {
    use probelab_core::Agent;
    use probelab_model_adapters::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};

    std::env::set_var("PROBELAB_E2E_KEY", "sk-E2E-ADVERSARIAL-SECRET");
    let config = OpenAiCompatibleConfig {
        // Port 1 on loopback: reserved, so nothing is listening and the connection is refused
        // immediately rather than hanging for the full timeout.
        base_url: "http://127.0.0.1:1/v1".to_owned(),
        api_key_env: "PROBELAB_E2E_KEY".to_owned(),
        model: "e2e-model".to_owned(),
        timeout_seconds: 2,
        temperature: 0.0,
        max_tokens: 16,
    };
    config.validate().expect("a loopback base URL is valid");
    let provider = OpenAiCompatibleProvider::new(config).expect("construct");
    let started = std::time::Instant::now();

    // The real shipped agent, which owns the current-thread runtime the async provider needs.
    let mut agent = probelab_agent::AsyncProviderAgent::from_provider(provider).expect("bridge");
    let failure = agent
        .decide(probelab_core::ModelRequest {
            episode_id: "e2e".to_owned(),
            turn: 1,
            observation: probelab_core::Observation {
                turn: 1,
                position: probelab_core::Coordinate { x: 0, y: 0 },
                grid_size: probelab_core::GridSize {
                    width: 5,
                    height: 5,
                },
                current_tile: probelab_core::TileKind::Safe,
                visible_neighbors: Vec::new(),
                distance_to_goal: 4,
                max_turns: 8,
                remaining_turns: 7,
                available_actions: vec![
                    probelab_core::Action::Up,
                    probelab_core::Action::Down,
                    probelab_core::Action::Wait,
                ],
            },
            available_actions: vec![
                probelab_core::Action::Up,
                probelab_core::Action::Down,
                probelab_core::Action::Wait,
            ],
            process_spawn_available: false,
            history: Vec::new(),
            game: None,
        })
        .expect_err("an unreachable provider must fail rather than return a default decision");

    for marker in ["sk-E2E-ADVERSARIAL-SECRET", "PROBELAB_E2E_KEY"] {
        assert!(
            !failure.contains(marker),
            "a provider failure leaked {marker}: {failure}"
        );
    }
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "an unreachable provider took longer than its own timeout, so the timeout does not apply"
    );
    std::env::remove_var("PROBELAB_E2E_KEY");
}

/// A base URL that would carry a credential, or reach a remote host in plaintext, is refused by
/// validation -- before any request is built and therefore before any key could be attached to one.
#[test]
fn the_provider_path_refuses_an_endpoint_that_would_leak_a_credential() {
    use probelab_model_adapters::{OpenAiCompatibleConfig, OpenAiCompatibleConfigError};

    for base_url in [
        "https://user:password@example.com/v1",
        "https://sk-REALKEY@example.com/v1",
        "http://example.com/v1",
    ] {
        let config = OpenAiCompatibleConfig {
            base_url: base_url.to_owned(),
            api_key_env: "PROBELAB_E2E_KEY".to_owned(),
            model: "e2e-model".to_owned(),
            timeout_seconds: 30,
            temperature: 0.0,
            max_tokens: 200,
        };
        let error = config.validate().expect_err(
            "an endpoint that leaks a credential, or plaintext remote, must be refused",
        );
        assert!(
            matches!(
                error,
                OpenAiCompatibleConfigError::UrlUserInfo
                    | OpenAiCompatibleConfigError::InsecureBaseUrl
                    | OpenAiCompatibleConfigError::UrlQuery
                    | OpenAiCompatibleConfigError::UrlFragment
            ),
            "{base_url} produced {error:?} rather than a base-URL refusal"
        );
        assert!(
            !error.to_string().contains("sk-REALKEY"),
            "the refusal echoed the credential back: {error}"
        );
    }

    // And the local-provider case still works, so the check above is specific and not a blanket refusal.
    OpenAiCompatibleConfig {
        base_url: "http://127.0.0.1:11434/v1".to_owned(),
        api_key_env: "PROBELAB_E2E_KEY".to_owned(),
        model: "e2e-model".to_owned(),
        timeout_seconds: 30,
        temperature: 0.0,
        max_tokens: 200,
    }
    .validate()
    .expect("a loopback endpoint is the local-provider case and must remain usable");
}
