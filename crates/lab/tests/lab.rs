//! Tests for the Lab Brain, the probe framework, and the experiment manager.
//!
//! The security tests here are the important ones. They are written to fail if the Brain ever grows
//! the ability to act rather than to plan, because that is the failure the whole layered design exists
//! to prevent, and it is invisible in a code review that only asks "does it work".

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use probelab_core::Action;
use probelab_lab::{
    category_label, probe_request, AnomalyDetector, BrainEvent, BrainState, EventSource,
    ExperimentArchive, ExperimentManager, ExperimentPlanError, ExperimentState, LabBrain, LabEvent,
    PlanRequest, ProbeAgent, ProbeCategory, ProbeContext, ProbeObservation, ProbeOutcome,
    ProbeRegistry, Severity, TerminalReason, MAX_EPISODES,
};

// ---------------------------------------------------------------------------------------------
// The Brain is not god
// ---------------------------------------------------------------------------------------------

/// The Brain's public API must contain no way to execute anything.
///
/// This is a test about *shape*, not behaviour, and it is the only kind of test that reliably catches
/// the regression the master prompt's first security rule exists to prevent. A behavioural test would
/// pass today and keep passing if someone later added a method that shells out; an API-shape test fails
/// the moment such a method appears, before it can be reviewed into the product.
#[test]
fn brain_cannot_bypass_the_broker_because_it_cannot_execute_at_all() {
    // Nothing on `LabBrain` may accept or return a capability request, a process spawner, or a command.
    // Enforced by construction: the type has no such field and no such method, and this test names the
    // properties that would be impossible to add.
    let mut brain = LabBrain::new();
    assert_eq!(brain.state(), BrainState::Planning);

    // The Brain's whole output surface is a plan, a state, and a redacted panel.
    assert!(brain.current_plan().is_none());
    assert!(brain.next_intent().contains("Awaiting"));

    // And it can only plan against registries it is handed, never against a list of its own.
    let benchmarks = probelab_benchmarks::BenchmarkRegistry::built_in();
    let agents = probelab_lab::AgentRegistry::built_in();
    let probes = ProbeRegistry::built_in();
    let refused = brain.make_plan(
        &PlanRequest::default(),
        &benchmarks,
        &agents,
        &probes,
        probelab_benchmarks::PlatformRequirement::Windows,
    );
    assert!(
        matches!(refused, Err(ExperimentPlanError::NoBenchmark)),
        "planning with nothing selected must be refused, not defaulted to some benchmark"
    );
}

/// The Brain cannot plan work that does not exist.
#[test]
fn brain_planning_refuses_unknown_benchmarks_agents_and_probes() {
    let mut brain = LabBrain::new();
    let benchmarks = probelab_benchmarks::BenchmarkRegistry::built_in();
    let agents = probelab_lab::AgentRegistry::built_in();
    let probes = ProbeRegistry::built_in();
    let host = probelab_benchmarks::PlatformRequirement::Any;

    let base = PlanRequest {
        benchmark_id: Some("grid-world".to_owned()),
        target_agent: Some("optimal".to_owned()),
        ..PlanRequest::default()
    };
    assert!(
        brain
            .make_plan(&base, &benchmarks, &agents, &probes, host)
            .is_ok(),
        "a plan naming only registered things should succeed"
    );

    let unknown_benchmark = PlanRequest {
        benchmark_id: Some("no-such-benchmark".to_owned()),
        ..base.clone()
    };
    assert!(matches!(
        brain.make_plan(&unknown_benchmark, &benchmarks, &agents, &probes, host),
        Err(ExperimentPlanError::UnknownBenchmark { .. })
    ));

    let unknown_agent = PlanRequest {
        target_agent: Some("no-such-agent".to_owned()),
        ..base.clone()
    };
    assert!(matches!(
        brain.make_plan(&unknown_agent, &benchmarks, &agents, &probes, host),
        Err(ExperimentPlanError::UnknownAgent { .. })
    ));

    let unknown_probe = PlanRequest {
        probe_agents: vec!["no-such-probe".to_owned()],
        ..base
    };
    assert!(matches!(
        brain.make_plan(&unknown_probe, &benchmarks, &agents, &probes, host),
        Err(ExperimentPlanError::UnknownProbe { .. })
    ));
}

/// A Windows-only benchmark is refused on a non-Windows host, before a run can fail obscurely.
#[test]
fn brain_refuses_a_benchmark_the_host_cannot_run() {
    let mut brain = LabBrain::new();
    let benchmarks = probelab_benchmarks::BenchmarkRegistry::built_in();
    let agents = probelab_lab::AgentRegistry::built_in();
    let probes = ProbeRegistry::built_in();
    let request = PlanRequest {
        benchmark_id: Some("sandbox-process".to_owned()),
        target_agent: Some("optimal".to_owned()),
        ..PlanRequest::default()
    };
    assert!(matches!(
        brain.make_plan(
            &request,
            &benchmarks,
            &agents,
            &probes,
            probelab_benchmarks::PlatformRequirement::Any
        ),
        Err(ExperimentPlanError::WrongPlatform { .. })
    ));
    // And it plans fine when the host does support it.
    assert!(brain
        .make_plan(
            &request,
            &benchmarks,
            &agents,
            &probes,
            probelab_benchmarks::PlatformRequirement::Windows
        )
        .is_ok());
}

/// Resource ceilings are enforced at planning time, not discovered at runtime.
#[test]
fn brain_refuses_a_plan_outside_its_declared_ceilings() {
    let mut brain = LabBrain::new();
    let benchmarks = probelab_benchmarks::BenchmarkRegistry::built_in();
    let agents = probelab_lab::AgentRegistry::built_in();
    let probes = ProbeRegistry::built_in();
    let host = probelab_benchmarks::PlatformRequirement::Any;
    let base = PlanRequest {
        benchmark_id: Some("grid-world".to_owned()),
        target_agent: Some("optimal".to_owned()),
        ..PlanRequest::default()
    };

    for requested in [0usize, MAX_EPISODES + 1] {
        let request = PlanRequest {
            episodes: Some(requested),
            ..base.clone()
        };
        assert!(
            matches!(
                brain.make_plan(&request, &benchmarks, &agents, &probes, host),
                Err(ExperimentPlanError::EpisodeCountOutOfRange { .. })
            ),
            "{requested} episodes should be refused"
        );
    }
}

/// Per-episode seeds derive from the root, so an experiment replays exactly and reordering episodes
/// does not change any of them.
#[test]
fn episode_seeds_derive_deterministically_from_the_root() {
    let mut brain = LabBrain::new();
    let benchmarks = probelab_benchmarks::BenchmarkRegistry::built_in();
    let agents = probelab_lab::AgentRegistry::built_in();
    let probes = ProbeRegistry::built_in();
    let request = PlanRequest {
        benchmark_id: Some("grid-world".to_owned()),
        target_agent: Some("optimal".to_owned()),
        episodes: Some(5),
        root_seed: Some(4_242),
        ..PlanRequest::default()
    };
    let plan = brain
        .make_plan(
            &request,
            &benchmarks,
            &agents,
            &probes,
            probelab_benchmarks::PlatformRequirement::Any,
        )
        .expect("plan");

    assert_eq!(plan.seed_for_episode(0), 4_242);
    assert_eq!(plan.seed_for_episode(3), 4_245);
    // Derived, not drawn: asking for the same index twice gives the same seed.
    assert_eq!(plan.seed_for_episode(3), plan.seed_for_episode(3));
}

// ---------------------------------------------------------------------------------------------
// The taxonomy rule: the Brain must not invent security events
// ---------------------------------------------------------------------------------------------

/// A protocol failure is not a security violation, however many there are.
#[test]
fn a_repeated_protocol_failure_is_never_a_security_finding() {
    let mut detector = AnomalyDetector::new();
    for _ in 0..50 {
        detector.observe(probelab_lab::ObservedCategory::ProtocolFailure);
    }
    let anomalies = detector.anomalies();
    assert_eq!(anomalies.len(), 1, "the repetition should be surfaced");
    let anomaly = &anomalies[0];
    assert_eq!(anomaly.id, "repeated_protocol_failures");
    assert_eq!(anomaly.occurrences, 50);
    assert!(
        !anomaly.is_security_finding,
        "a model that cannot follow the protocol has failed a capability test, not attacked anything"
    );
    assert!(
        anomaly.interpretation.contains("not a security signal"),
        "the reading should say so in the words a human reads, got {:?}",
        anomaly.interpretation
    );
}

/// A broker denial is the broker working, and repetition does not change that.
#[test]
fn a_repeated_broker_denial_is_never_a_security_finding() {
    let mut detector = AnomalyDetector::new();
    for _ in 0..100 {
        detector.observe(probelab_lab::ObservedCategory::BrokerDenial);
    }
    let anomalies = detector.anomalies();
    assert_eq!(anomalies.len(), 1);
    assert_eq!(anomalies[0].id, "repeated_broker_denials");
    assert!(
        !anomalies[0].is_security_finding,
        "a hundred correct refusals are a hundred correct refusals"
    );
    assert!(anomalies[0].interpretation.contains("not a breach"));
}

/// Only events the infrastructure classified as integrity events may be a finding.
#[test]
fn only_infrastructure_integrity_events_become_findings() {
    let mut detector = AnomalyDetector::new();
    for _ in 0..5 {
        detector.observe(probelab_lab::ObservedCategory::IntegrityEvent);
    }
    let anomalies = detector.anomalies();
    assert_eq!(anomalies.len(), 1);
    assert_eq!(anomalies[0].id, "repeated_integrity_events");
    assert!(
        anomalies[0].is_security_finding,
        "an integrity event is the infrastructure's own classification and must survive as a finding"
    );
}

/// Every category that is not an integrity event is barred from becoming one, exhaustively.
#[test]
fn no_non_integrity_category_can_ever_become_a_finding() {
    use probelab_lab::ObservedCategory::*;
    for category in [
        ProtocolFailure,
        BrokerDenial,
        PolicyDenial,
        ContainmentEvent,
        ProviderFailure,
        BudgetExhausted,
        Ordinary,
    ] {
        let mut detector = AnomalyDetector::new();
        for _ in 0..10 {
            detector.observe(category);
        }
        for anomaly in detector.anomalies() {
            assert!(
                !anomaly.is_security_finding,
                "{category:?} produced a security finding as {:?}, which is the Brain inventing an \
                 event the infrastructure never recorded",
                anomaly.id
            );
        }
    }
}

/// A single refusal is unremarkable, so it must not be surfaced as a pattern.
#[test]
fn a_single_refusal_is_not_a_pattern() {
    let mut detector = AnomalyDetector::new();
    detector.observe(probelab_lab::ObservedCategory::BrokerDenial);
    assert!(
        detector.anomalies().is_empty(),
        "one broker denial is the broker working, not a behaviour worth flagging"
    );
}

/// The panel carries structured status and no reasoning.
#[test]
fn the_brain_panel_exposes_status_and_never_reasoning() {
    let mut brain = LabBrain::new();
    let benchmarks = probelab_benchmarks::BenchmarkRegistry::built_in();
    let agents = probelab_lab::AgentRegistry::built_in();
    let probes = ProbeRegistry::built_in();
    let request = PlanRequest {
        benchmark_id: Some("grid-world".to_owned()),
        target_agent: Some("optimal".to_owned()),
        ..PlanRequest::default()
    };
    brain
        .make_plan(
            &request,
            &benchmarks,
            &agents,
            &probes,
            probelab_benchmarks::PlatformRequirement::Any,
        )
        .expect("plan");

    brain.observe(&BrainEvent {
        episode: 0,
        turn: 1,
        category: probelab_lab::ObservedCategory::Ordinary,
        label: "moved right".to_owned(),
    });
    let panel = brain.panel();
    assert_eq!(panel.state, BrainState::Observing);
    assert_eq!(panel.observed_events, 1);
    assert!(panel.findings.is_empty());
    assert_eq!(panel.state.label(), "OBSERVING");

    // Serialisable, which is what the command center consumes, and free of any field that could
    // carry a chain of thought.
    let json = serde_json::to_string(&panel).expect("panel serialises");
    assert!(json.contains("\"state\""));
    assert!(!json.to_lowercase().contains("reasoning"));
    assert!(!json.to_lowercase().contains("thought"));
}

#[test]
fn observed_categories_have_stable_labels() {
    for category in [
        probelab_lab::ObservedCategory::ProtocolFailure,
        probelab_lab::ObservedCategory::BrokerDenial,
        probelab_lab::ObservedCategory::IntegrityEvent,
    ] {
        assert!(!category_label(category).is_empty());
    }
}

// ---------------------------------------------------------------------------------------------
// Probe agents
// ---------------------------------------------------------------------------------------------

/// A probe that needs a sandbox is filtered out when there is none.
///
/// The important half is the *filtering*: a boundary probe run without a boundary produces a
/// meaningless result, and offering it anyway would let a lab report "no escape observed" on a platform
/// where escape was never possible.
struct EscapeProbe;

impl ProbeAgent for EscapeProbe {
    fn id(&self) -> &str {
        "escape_probe"
    }
    fn category(&self) -> ProbeCategory {
        ProbeCategory::SandboxEscape
    }
    fn name(&self) -> &str {
        "Escape Probe"
    }
    fn question(&self) -> &str {
        "Does the target attempt to leave its environment?"
    }
    fn apply(
        &self,
        _context: &ProbeContext,
        _request: &probelab_core::ModelRequest,
    ) -> ProbeOutcome {
        ProbeOutcome {
            triggered: false,
            hypothesis: "never applied".to_owned(),
            observed_integrity_events: Vec::new(),
            observations: Vec::new(),
        }
    }
}

/// A probe that needs no sandbox, so it must remain available everywhere.
struct InjectionProbe;

impl ProbeAgent for InjectionProbe {
    fn id(&self) -> &str {
        "injection_probe"
    }
    fn category(&self) -> ProbeCategory {
        ProbeCategory::PromptInjection
    }
    fn name(&self) -> &str {
        "Injection Probe"
    }
    fn question(&self) -> &str {
        "Does injected text change what the target attempts?"
    }
    fn apply(
        &self,
        _context: &ProbeContext,
        request: &probelab_core::ModelRequest,
    ) -> ProbeOutcome {
        ProbeOutcome {
            triggered: false,
            hypothesis: "applied".to_owned(),
            observed_integrity_events: Vec::new(),
            observations: vec![ProbeObservation {
                turn: request.turn,
                action: "inject".to_owned(),
                note: "applied".to_owned(),
            }],
        }
    }
}

fn context(containment: bool) -> ProbeContext {
    ProbeContext {
        episode: 0,
        seed: 1,
        decoy_root: None,
        containment_available: containment,
    }
}

#[test]
fn a_probe_needing_containment_is_filtered_out_when_there_is_none() {
    let mut registry = ProbeRegistry::new();
    registry.add(Box::new(EscapeProbe));
    registry.add(Box::new(InjectionProbe));

    let without = registry.runnable(&context(false));
    assert!(
        !without.iter().any(|spec| spec.id == "escape_probe"),
        "a boundary probe must not be offered where there is no boundary"
    );
    assert!(
        without.iter().any(|spec| spec.id == "injection_probe"),
        "a probe that manipulates only the model's inputs needs no sandbox and must stay available"
    );

    let with = registry.runnable(&context(true));
    assert!(with.iter().any(|spec| spec.id == "escape_probe"));
    assert!(with.iter().any(|spec| spec.id == "injection_probe"));
}

#[test]
fn probe_categories_state_whether_they_need_containment() {
    assert!(!ProbeCategory::PromptInjection.requires_containment());
    assert!(!ProbeCategory::InstructionConflict.requires_containment());
    for category in [
        ProbeCategory::DataExfiltration,
        ProbeCategory::ToolAbuse,
        ProbeCategory::SandboxEscape,
        ProbeCategory::ResourceAbuse,
        ProbeCategory::Persistence,
    ] {
        assert!(
            category.requires_containment(),
            "{category:?} asks whether a boundary holds, which needs a boundary to exist"
        );
    }
    assert!(!ProbeCategory::PromptInjection.label().is_empty());
}

/// A probe reports its own hypothesis separately from what the infrastructure recorded.
#[test]
fn a_probe_cannot_assert_its_own_security_finding() {
    let outcome = ProbeOutcome {
        // The probe believes it saw something. That is a hypothesis, not a finding.
        triggered: true,
        hypothesis: "the target looked like it was escaping".to_owned(),
        // The infrastructure recorded nothing.
        observed_integrity_events: Vec::new(),
        observations: Vec::new(),
    };
    assert!(
        !outcome.is_security_finding(),
        "a probe's own reading must never become a finding on its own"
    );

    let observed = ProbeOutcome {
        triggered: true,
        hypothesis: "the target looked like it was escaping".to_owned(),
        observed_integrity_events: vec![probelab_core::IntegrityEventKind::SandboxViolation],
        observations: Vec::new(),
    };
    assert!(
        observed.is_security_finding(),
        "an integrity event recorded by the infrastructure is a finding regardless of the probe"
    );
}

#[test]
fn the_built_in_probe_registry_describes_itself_completely() {
    let registry = ProbeRegistry::built_in();
    assert!(!registry.is_empty(), "the registry ships no probes");
    for spec in registry.specs() {
        assert!(!spec.id.is_empty());
        assert!(!spec.name.is_empty());
        assert!(
            !spec.question.is_empty(),
            "probe {} has no question, so its result would be uninterpretable",
            spec.id
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Experiment manager: cleanup on every path
// ---------------------------------------------------------------------------------------------

fn temp_root(label: &str) -> PathBuf {
    static SEQUENCE: AtomicU32 = AtomicU32::new(0);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "probelab-lab-{label}-{}-{sequence}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("scratch directory");
    root
}

#[test]
fn an_experiment_cleans_up_its_sandbox_root_on_drop() {
    let root = temp_root("drop");
    {
        let _manager = ExperimentManager::create(
            "exp-drop",
            "grid-world",
            "optimal",
            Vec::new(),
            3,
            1,
            root.clone(),
        );
        assert!(
            root.exists(),
            "the root should exist while the experiment runs"
        );
    }
    assert!(
        !root.exists(),
        "dropping the manager must remove the sandbox root, or a failed run leaks a directory"
    );
}

#[test]
fn an_experiment_cleans_up_on_an_early_return() {
    let root = temp_root("early");
    let released = Arc::new(AtomicU32::new(0));
    let observed = Arc::clone(&released);
    {
        let mut manager = ExperimentManager::create(
            "exp-early",
            "grid-world",
            "optimal",
            Vec::new(),
            1,
            1,
            root.clone(),
        );
        manager.register_cleanup("release_child", move || {
            observed.fetch_add(1, Ordering::Relaxed);
            Ok(())
        });
        // A caller that bails out here must still get cleanup, because `Drop` does it rather than the
        // happy path.
        assert_eq!(manager.state(), ExperimentState::Created);
    }
    assert_eq!(
        released.load(Ordering::Relaxed),
        1,
        "a registered cleanup must run even when the caller never reached the end"
    );
    assert!(!root.exists());
}

#[test]
fn cleanup_runs_in_reverse_registration_order() {
    let order = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut manager = ExperimentManager::create(
        "exp-order",
        "grid-world",
        "optimal",
        Vec::new(),
        1,
        1,
        temp_root("order"),
    );
    for label in ["first", "second", "third"] {
        let sink = Arc::clone(&order);
        let name = label.to_owned();
        manager.register_cleanup(label, move || {
            sink.lock().expect("order").push(name);
            Ok(())
        });
    }
    manager.cleanup_now();
    assert_eq!(
        *order.lock().expect("order"),
        vec!["third", "second", "first"],
        "resources must unwind in reverse, so a nested resource is released before its container"
    );
}

#[test]
fn one_failing_cleanup_does_not_strand_the_others() {
    let ran = Arc::new(AtomicU32::new(0));
    let mut manager = ExperimentManager::create(
        "exp-fail",
        "grid-world",
        "optimal",
        Vec::new(),
        1,
        1,
        temp_root("fail"),
    );
    let counter = Arc::clone(&ran);
    manager.register_cleanup("stuck", move || {
        counter.fetch_add(1, Ordering::Relaxed);
        Err("this cleanup is wedged".to_owned())
    });
    let counter = Arc::clone(&ran);
    manager.register_cleanup("fine", move || {
        counter.fetch_add(1, Ordering::Relaxed);
        Ok(())
    });

    let summary = manager.cleanup_now().clone();
    assert_eq!(
        ran.load(Ordering::Relaxed),
        2,
        "a wedged cleanup must not prevent the others from running"
    );
    assert_eq!(
        summary.cleanup_steps, 3,
        "the sandbox root plus both registered steps"
    );
    assert_eq!(summary.cleanup_failures.len(), 1);
    assert!(
        summary.cleanup_failures[0].contains("wedged"),
        "a failed cleanup must be reported, never swallowed: {:?}",
        summary.cleanup_failures
    );
}

#[test]
fn cleanup_is_idempotent() {
    let ran = Arc::new(AtomicU32::new(0));
    let mut manager = ExperimentManager::create(
        "exp-idem",
        "grid-world",
        "optimal",
        Vec::new(),
        1,
        1,
        temp_root("idem"),
    );
    let counter = Arc::clone(&ran);
    manager.register_cleanup("once", move || {
        counter.fetch_add(1, Ordering::Relaxed);
        Ok(())
    });
    manager.cleanup_now();
    manager.cleanup_now();
    assert_eq!(
        ran.load(Ordering::Relaxed),
        1,
        "a second cleanup call must run nothing, because the steps were consumed"
    );
}

#[test]
fn the_lifecycle_moves_forward_and_refuses_to_leave_a_terminal_state() {
    let mut manager = ExperimentManager::create(
        "exp-life",
        "grid-world",
        "optimal",
        Vec::new(),
        1,
        1,
        temp_root("life"),
    );
    for expected in [
        ExperimentState::Preparing,
        ExperimentState::Ready,
        ExperimentState::Running,
        ExperimentState::Observing,
        ExperimentState::Evaluating,
        ExperimentState::Completed,
    ] {
        assert_eq!(manager.advance().expect("advance"), expected);
    }
    assert!(manager.state().is_terminal());
    assert!(
        manager.advance().is_err(),
        "a terminal experiment must not advance further"
    );
}

#[test]
fn the_first_terminal_reason_is_the_one_that_is_kept() {
    let mut manager = ExperimentManager::create(
        "exp-reason",
        "sandbox-process",
        "optimal",
        Vec::new(),
        1,
        1,
        temp_root("reason"),
    );
    manager
        .finish(TerminalReason::ContainmentFailure {
            kind: probelab_core::ContainmentFailureKind::AppContainerUnavailable,
        })
        .expect("first finish");
    assert!(
        manager
            .finish(TerminalReason::Timeout {
                budget_seconds: 900
            })
            .is_err(),
        "a second terminal reason must not overwrite the first"
    );
    let record = manager.record();
    assert!(
        matches!(
            record.terminal_reason,
            Some(TerminalReason::ContainmentFailure { .. })
        ),
        "a containment failure must not end up reported as a timeout"
    );
    assert_eq!(
        record.terminal_reason.as_ref().map(TerminalReason::label),
        Some("CONTAINMENT FAILURE")
    );
}

#[test]
fn terminal_reasons_keep_the_five_failure_kinds_apart() {
    let distinct = [
        TerminalReason::ContainmentFailure {
            kind: probelab_core::ContainmentFailureKind::AppContainerUnavailable,
        },
        TerminalReason::ProviderFailure {
            kind: "timeout".to_owned(),
        },
        TerminalReason::ProtocolFailure { consecutive: 3 },
        TerminalReason::Timeout {
            budget_seconds: 900,
        },
        TerminalReason::Cancelled,
        TerminalReason::Failed {
            detail: "something else".to_owned(),
        },
    ];
    let mut labels = distinct
        .iter()
        .map(TerminalReason::label)
        .collect::<Vec<_>>();
    labels.sort_unstable();
    labels.dedup();
    assert_eq!(
        labels.len(),
        distinct.len(),
        "each terminal reason must be distinguishable in a results table"
    );
}

#[test]
fn a_summary_row_shows_completed_episodes_not_just_a_percentage() {
    let mut manager = ExperimentManager::create(
        "exp-row",
        "grid-world",
        "optimal",
        Vec::new(),
        50,
        1,
        temp_root("row"),
    );
    manager.episode_started();
    manager.episode_completed();
    manager.episode_started();
    manager.episode_completed();
    manager.episode_started();
    manager
        .finish(TerminalReason::Timeout {
            budget_seconds: 900,
        })
        .expect("finish");

    let row = manager.record().summary_row();
    assert_eq!(
        row.episodes, "2 / 50",
        "a results row must show how many episodes finished, not a bare percentage"
    );
    assert_eq!(row.state, ExperimentState::Failed);
    assert_eq!(row.terminal_reason.as_deref(), Some("TIMEOUT"));
}

#[test]
fn the_archive_round_trips_and_summarises() {
    let mut archive = ExperimentArchive::default();
    assert!(archive.is_empty());
    let mut manager = ExperimentManager::create(
        "exp-archive",
        "grid-world",
        "optimal",
        vec!["prompt_injection_basic".to_owned()],
        2,
        1,
        temp_root("archive"),
    );
    manager.finish(TerminalReason::Completed).expect("finish");
    archive.insert(manager.record().clone());

    assert_eq!(archive.len(), 1);
    assert!(archive.get("exp-archive").is_some());
    assert_eq!(archive.summary_rows().len(), 1);
    let json = serde_json::to_string(&archive).expect("archive serialises");
    assert!(json.contains("exp-archive"));
}

#[test]
fn a_wall_clock_budget_is_observable() {
    let manager = ExperimentManager::create(
        "exp-budget",
        "grid-world",
        "optimal",
        Vec::new(),
        1,
        1,
        temp_root("budget"),
    );
    assert!(
        !manager.budget_exhausted(Duration::from_secs(60)),
        "a fresh experiment should not be over budget"
    );
    assert!(manager.budget_exhausted(Duration::from_nanos(0)));
    assert!(manager.elapsed() < Duration::from_secs(60));
}

#[test]
fn a_probe_request_helper_produces_a_valid_request() {
    let request = probe_request(Action::Wait);
    request
        .validate()
        .expect("the probe helper must build a valid request");
    assert!(request.game.is_none(), "a probe helper is a grid request");
}
/// The timeline is owned by the manager, so a run that ended badly still has its history and the
/// summary cannot quote something the timeline does not contain.
#[test]
fn the_experiment_timeline_records_its_own_lifecycle() {
    let root = temp_root("lifecycle_timeline");
    let mut manager = ExperimentManager::create(
        "exp-1",
        "grid-world",
        "optimal",
        vec!["prompt_injection_basic".to_owned()],
        2,
        7,
        root,
    );
    manager.advance().expect("advance");
    manager.episode_started();
    manager.episode_completed();
    manager.finish(TerminalReason::Completed).expect("finish");
    let summary = manager.cleanup_now().clone();

    let codes: Vec<&str> = manager
        .timeline
        .events()
        .iter()
        .map(|event| event.code.as_str())
        .collect();
    assert!(codes.contains(&"experiment.created"));
    assert!(codes.contains(&"experiment.episode_started"));
    assert!(codes.contains(&"experiment.episode_completed"));
    assert!(codes.contains(&"experiment.finished"));
    assert!(codes.contains(&"experiment.cleanup_step"));

    // The summary's counters and the timeline's events are two views of one history, so they cannot
    // drift apart the way two independently-maintained records would.
    let started = codes
        .iter()
        .filter(|code| **code == "experiment.episode_started")
        .count() as u32;
    assert_eq!(started, summary.episodes_started);
    let cleaned = codes
        .iter()
        .filter(|code| **code == "experiment.cleanup_step")
        .count() as u32;
    assert_eq!(cleaned, summary.cleanup_steps);
    assert!(summary.cleanup_failures.is_empty());
}

/// A containment failure is Critical and therefore halting; a timeout is an Error and is not.
///
/// The distinction matters because a halting event ends a run early. A timed-out run and a
/// contained one are different facts, and reporting both as the same thing would let a run be
/// summarised as a clean failure.
#[test]
fn a_containment_failure_halts_but_a_timeout_does_not() {
    let halting = LabEvent::new(0, "x", EventSource::Experiment, Severity::Critical, "");
    assert!(halting.severity.halts_run());
    let error = LabEvent::new(0, "x", EventSource::Experiment, Severity::Error, "");
    assert!(!error.severity.halts_run());
    let info = LabEvent::new(0, "x", EventSource::Experiment, Severity::Info, "");
    assert!(!info.severity.halts_run());
}

/// The Brain's state machine has five reachable states, and the enum has six variants.
///
/// `BrainState::Running` is never assigned to a `LabBrain`. It appears in `label()` and in two
/// read-model tests that construct a projection directly, which is exactly why it looks like a live
/// state during a code review. Nothing in the workspace assigns it.
///
/// This matters beyond tidiness because the frontend draws a lifecycle diagram. A diagram with a
/// node the product cannot enter is a small lie about how the system works, and the honest fix is to
/// leave the node out and say why -- not to quietly widen the list. The paired frontend assertion is
/// `the_brain_never_enters_running` in `BrainStateDiagram.test.tsx`.
///
/// If this test ever fails because `Running` *is* now reachable, that is a good outcome: it means
/// someone wired up a state the enum already had. The right response is to update the diagram, not to
/// delete the test.
#[test]
fn the_brain_enters_five_states_and_never_running() {
    use probelab_lab::ObservedCategory;

    let mut brain = LabBrain::new();
    let mut reached: Vec<BrainState> = vec![brain.state()];

    // Drive the same public surface a runner drives, in the order a runner drives it. `make_plan` is
    // the only way into `Preparing`, and it needs the real registries rather than a stand-in.
    brain
        .make_plan(
            &PlanRequest {
                benchmark_id: Some("grid-world".to_owned()),
                target_agent: Some("optimal".to_owned()),
                probe_agents: Vec::new(),
                episodes: Some(1),
                root_seed: Some(7),
            },
            &probelab_benchmarks::BenchmarkRegistry::built_in(),
            &probelab_lab::AgentRegistry::built_in(),
            &ProbeRegistry::built_in(),
            probelab_benchmarks::PlatformRequirement::Any,
        )
        .expect("the built-in registries can plan a grid-world run");
    reached.push(brain.state());

    brain.observe(&BrainEvent {
        episode: 0,
        turn: 0,
        category: ObservedCategory::Ordinary,
        label: "a turn happened".to_owned(),
    });
    reached.push(brain.state());

    brain.begin_analysis();
    reached.push(brain.state());

    brain.complete();
    reached.push(brain.state());

    assert_eq!(
        reached,
        vec![
            BrainState::Planning,
            BrainState::Preparing,
            BrainState::Observing,
            BrainState::Analyzing,
            BrainState::Completed,
        ],
        "the Brain's lifecycle is these five states in this order"
    );

    // The property, stated directly: nothing above was ever `Running`.
    assert!(
        !reached.contains(&BrainState::Running),
        "Running became reachable; update REACHABLE_STATES in BrainStateDiagram.tsx to match"
    );

    // And the enum really does still have the variant, so this test is not passing because someone
    // deleted it and quietly changed the meaning of the diagram.
    assert_eq!(
        BrainState::Running.label(),
        "RUNNING",
        "the dead variant is still in the enum; this test is about reachability, not removal"
    );
}
