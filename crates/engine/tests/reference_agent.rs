//! The reference agent's guarantees, proved once for every strategy rather than per benchmark.
//!
//! Phase 0 proved one property about one solver, in a test that named it. That does not scale: the
//! next benchmark adds a second reference policy, and the proof is either copy-pasted (and drifts) or
//! quietly skipped. So it lives here, written generically over the registry, and a new strategy is
//! covered the moment it is registered.
//!
//! What is proved, and why each half is needed:
//!
//! 1. **No privileged input reaches a decision.** Two instances built from different seeds, replayed
//!    against one fixed request stream, must agree on every turn. A strategy that peeked at the
//!    layout, the goal, the answer, or its seed would diverge.
//! 2. **The wrapper resets between episodes.** Replaying the same episode twice through one instance
//!    must give the same answers both times. A strategy that forgot to clear its inferred state would
//!    answer the second pass differently, which is an advantage no honest policy has.
//! 3. **The descriptor states the guarantee.** Every registered strategy must declare observation-only
//!    knowledge, and the descriptor must say so. A strategy that admitted to reading the goal would
//!    fail here rather than quietly invalidate every number compared against it.
//!
//! The two seeds used are checked to describe different worlds first, because a proof over two
//! identical layouts would pass no matter what the strategy did.

use probelab_agent::{ReferenceAgent, ReferenceKnowledge, ReferenceStrategy};
use probelab_core::{Agent, ModelDecision, ModelRequest, Sandbox};
use probelab_model_adapters::{GridWorldReference, SandboxProcessReference};
use probelab_sandbox::{grid_world_layout, GridWorldSandbox};

/// Builds a strategy for a seed.
type StrategyFactory = fn(u64) -> Box<dyn ReferenceStrategy>;

/// Every reference strategy the workspace ships, as a factory over a seed.
///
/// A registry rather than a hand-written list of test bodies, so registering a strategy is the same
/// act as making it subject to the proof. A strategy that is not reachable from here is not covered,
/// and that is worth being explicit about rather than assuming.
fn registry() -> Vec<(&'static str, StrategyFactory)> {
    vec![
        ("grid_world_reference", |seed| {
            Box::new(GridWorldReference::new(seed))
        }),
        ("sandbox_process_reference", |seed| {
            Box::new(SandboxProcessReference::new(seed))
        }),
    ]
}

/// Two seeds that are checked to describe different worlds before any proof runs.
const SEED_A: u64 = 2_000;
const SEED_B: u64 = 2_031;

#[test]
fn ref_the_two_proof_seeds_really_do_describe_different_worlds() {
    assert_ne!(SEED_A, SEED_B);
    assert_ne!(
        grid_world_layout(SEED_A).tiles(),
        grid_world_layout(SEED_B).tiles(),
        "the two proof seeds produce the same layout, so every seed-independence check below would \
         pass no matter what the strategy did"
    );
}

/// A fixed request stream, built by walking a real sandbox so the observations are genuine.
fn request_stream() -> Vec<ModelRequest> {
    let mut sandbox = GridWorldSandbox::with_max_turns(SEED_A, 12).expect("sandbox");
    let mut requests = Vec::new();
    let mut history = Vec::new();
    while sandbox.episode().status == probelab_core::EpisodeStatus::Running {
        let observation = sandbox.observation();
        let turn = observation.turn;
        let request = ModelRequest {
            episode_id: "reference-proof".to_owned(),
            turn,
            observation: observation.clone(),
            available_actions: sandbox.available_actions(),
            process_spawn_available: true,
            // Grid path: the reference proof is about the grid strategies.
            game: None,
            // The same bounded window the engine builds, oldest first. Unbounded history would make
            // this stream something no real run ever produces.
            history: window(&history),
        };
        request.validate().expect("a synthesised request is valid");
        requests.push(request);
        // Any fixed move keeps the stream identical for every strategy under test, which is the point:
        // the stream is an input, and the strategies are compared on how they respond to it.
        let step = sandbox
            .step_for_test(probelab_core::Action::Right)
            .expect("step");
        let outcome = step.action_outcome.expect("a movement has an outcome");
        history.push(probelab_core::HistoryEntry {
            turn,
            position: observation.position,
            current_tile: observation.current_tile,
            visible_neighbors: observation.visible_neighbors.clone(),
            selected_action: probelab_core::Action::Right,
            result_kind: outcome.kind,
            reward: outcome.reward,
        });
    }
    assert!(
        requests.len() >= 2,
        "the stream must be long enough to be evidence"
    );
    requests
}

/// The newest `HISTORY_WINDOW` completed turns, oldest first.
fn window(history: &[probelab_core::HistoryEntry]) -> Vec<probelab_core::HistoryEntry> {
    let start = history.len().saturating_sub(probelab_core::HISTORY_WINDOW);
    history[start..].to_vec()
}

fn replay(strategy: &mut dyn ReferenceStrategy, requests: &[ModelRequest]) -> Vec<ModelDecision> {
    requests
        .iter()
        .map(|request| {
            strategy
                .decide(request)
                .expect("a reference strategy always has a decision")
        })
        .collect()
}

/// 1. No privileged input reaches a decision, for any registered strategy.
#[test]
fn ref_no_registered_strategy_lets_the_seed_or_the_world_reach_a_decision() {
    let requests = request_stream();
    for (label, build) in registry() {
        let mut a = build(SEED_A);
        let mut b = build(SEED_B);
        assert_eq!(
            replay(a.as_mut(), &requests),
            replay(b.as_mut(), &requests),
            "{label}: its decisions changed with the seed, so something outside the request stream \
             is reaching it"
        );
    }
}

/// 2. The wrapper resets between episodes, so a second episode cannot benefit from the first.
#[test]
fn ref_replaying_the_same_episode_twice_gives_identical_answers() {
    let requests = request_stream();
    for (label, build) in registry() {
        let mut agent = ReferenceAgent::new(build(SEED_A));
        let first: Vec<ModelDecision> = requests
            .iter()
            .map(|request| agent.decide(request.clone()).expect("decision"))
            .collect();
        let second: Vec<ModelDecision> = requests
            .iter()
            .map(|request| agent.decide(request.clone()).expect("decision"))
            .collect();
        assert_eq!(
            first, second,
            "{label}: replaying one episode changed its answers, so state leaked between episodes"
        );
    }
}

/// 3. Every strategy declares, and publishes, observation-only knowledge.
#[test]
fn ref_every_strategy_declares_and_publishes_observation_only_knowledge() {
    for (label, build) in registry() {
        let strategy = build(SEED_A);
        assert_eq!(
            strategy.knowledge(),
            ReferenceKnowledge::OBSERVATION_ONLY,
            "{label} declares knowledge beyond the model's own view, so it is an oracle rather than \
             a ceiling"
        );

        let descriptor = ReferenceAgent::new(build(SEED_A)).descriptor();
        assert_eq!(descriptor.adapter, "probelab-reference-v1");
        assert_eq!(
            descriptor.parameters.get("reference_policy"),
            Some(&serde_json::Value::Bool(true)),
            "{label} does not identify itself as a reference policy in the descriptor"
        );
        for (key, value) in [
            ("reads_layout", false),
            ("reads_goal", false),
            ("reads_answer", false),
            ("reads_seed", false),
        ] {
            assert_eq!(
                descriptor.parameters.get(key),
                Some(&serde_json::Value::Bool(value)),
                "{label} publishes {key}={value} in the descriptor, which contradicts the guarantee \
                 a reader of a result bundle is entitled to rely on"
            );
        }
    }
}

/// The wrapper rejects a malformed request before the strategy ever sees it.
///
/// Reference policies are trusted code, so "trusted" must not mean "exempt from validation" — a
/// strategy that assumed a valid request would behave differently on a malformed one, and the
/// guarantee that its decisions depend only on the stream would quietly stop holding.
#[test]
fn ref_a_malformed_request_never_reaches_the_strategy() {
    let mut agent = ReferenceAgent::new(GridWorldReference::new(SEED_A));
    let mut broken = request_stream().remove(0);
    broken.turn = 0;
    assert!(
        agent.decide(broken).is_err(),
        "a reference agent accepted a request with a zero turn"
    );
}

/// The sandbox-process reference asks exactly once per episode, and never names anything.
///
/// The count matters as much as the first turn. A strategy that requested on every turn would still
/// pass a weaker "does it ever request" test while behaving nothing like a policy that spends one
/// turn of its budget on a capability.
#[test]
fn ref_the_sandbox_process_reference_asks_exactly_once_and_never_names_a_program() {
    let requests = request_stream();
    let mut agent = ReferenceAgent::new(SandboxProcessReference::new(SEED_A));
    let decisions: Vec<ModelDecision> = requests
        .iter()
        .map(|request| agent.decide(request.clone()).expect("decision"))
        .collect();

    let requests_made = decisions
        .iter()
        .filter(|decision| matches!(decision, ModelDecision::ProcessSpawnRequest))
        .count();
    assert_eq!(
        requests_made,
        1,
        "expected exactly one capability request across {} turns, got {requests_made}",
        decisions.len()
    );
    assert!(
        matches!(decisions[0], ModelDecision::ProcessSpawnRequest),
        "the request must come on turn one, or it wastes a turn without buying anything"
    );
    assert!(
        decisions[1..]
            .iter()
            .all(|decision| matches!(decision, ModelDecision::GridAction(_))),
        "after the request the reference must navigate, not keep asking"
    );
}

/// After the request, the sandbox-process reference is *the same policy* as the grid-world one.
///
/// The two do **not** produce identical action sequences over one episode, and pretending otherwise
/// would be the easy mistake: the request costs the inner solver its first turn, so it has seen one
/// fewer observation and has recorded one fewer attempted move. That cost is the thing the benchmark
/// measures, so it must not be engineered away.
///
/// What is asserted here is the claim that actually matters for attributing a result — that the
/// capability request is the *only* difference in policy. So the reference is compared against a
/// grid-world reference that was likewise never consulted on turn one. If those agree for the rest of
/// the episode, then any difference in measured outcome comes from having asked, not from a second
/// different navigator hiding inside.
#[test]
fn ref_after_the_request_the_two_strategies_run_the_same_policy() {
    let requests = request_stream();
    let mut grid = GridWorldReference::new(SEED_A);
    let mut process = SandboxProcessReference::new(SEED_A);

    // Turn one, spent by the request.
    assert!(matches!(
        process.decide(&requests[0]).expect("decision"),
        ModelDecision::ProcessSpawnRequest
    ));

    // The comparison baseline: the grid-world policy, also not consulted on turn one.
    for (index, request) in requests.iter().enumerate().skip(1) {
        assert_eq!(
            process.decide(request).expect("decision"),
            grid.decide(request).expect("decision"),
            "the two strategies ran different policies from turn {} onwards, so a sandbox-process \
             result cannot be attributed to the capability request",
            index + 1
        );
    }
}
