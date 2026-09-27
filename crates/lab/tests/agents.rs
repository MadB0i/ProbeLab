//! The agent registry, and the cross-layer checks the benchmarks crate cannot make on its own.
//!
//! Two properties matter most here. Every registered agent must actually *work* — a registry entry
//! that builds a broken policy would offer a user a control that fails at the worst moment. And a
//! benchmark's declared reference agent must resolve, or the ceiling a user is told to compare against
//! is a name rather than a policy.

use probelab_core::{Action, Agent, ModelRequest};
use probelab_lab::{AgentCategory, AgentExecution, AgentRegistry, AgentSpec};

/// A realistic grid request, not a degenerate one.
///
/// The first version of this used a one-by-one grid, which no agent ever sees, and it failed for
/// `invalid_action_spam`: that policy's whole job is to attempt a move into an obstacle, and a
/// one-by-one board has no boundary direction to aim at. The failure was the test's fault, not the
/// agent's — but it is the reason this now builds the same five-by-five world the benchmarks use.
fn grid_request() -> ModelRequest {
    ModelRequest {
        episode_id: "registry".to_owned(),
        turn: 1,
        observation: probelab_core::Observation {
            turn: 1,
            position: probelab_core::Coordinate { x: 1, y: 1 },
            grid_size: probelab_core::GridSize {
                width: 5,
                height: 5,
            },
            current_tile: probelab_core::TileKind::Safe,
            visible_neighbors: vec![
                probelab_core::VisibleTile {
                    coordinate: probelab_core::Coordinate { x: 1, y: 0 },
                    tile: probelab_core::TileKind::Obstacle,
                },
                probelab_core::VisibleTile {
                    coordinate: probelab_core::Coordinate { x: 1, y: 2 },
                    tile: probelab_core::TileKind::Safe,
                },
                probelab_core::VisibleTile {
                    coordinate: probelab_core::Coordinate { x: 0, y: 1 },
                    tile: probelab_core::TileKind::Obstacle,
                },
                probelab_core::VisibleTile {
                    coordinate: probelab_core::Coordinate { x: 2, y: 1 },
                    tile: probelab_core::TileKind::Safe,
                },
            ],
            distance_to_goal: 4,
            max_turns: 12,
            remaining_turns: 12,
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
        process_spawn_available: false,
        history: Vec::new(),
        game: None,
    }
}

#[test]
fn the_registry_is_populated_and_covers_every_family() {
    let registry = AgentRegistry::built_in();
    assert!(!registry.is_empty());
    for category in [
        AgentCategory::DeterministicBaseline,
        AgentCategory::ProtocolProbe,
        AgentCategory::SecurityProbe,
        AgentCategory::PolicyAgent,
        AgentCategory::ReferenceCeiling,
    ] {
        assert!(
            !registry.in_category(category).is_empty(),
            "no agent is registered as {category:?}, so a family the lab claims to support is empty"
        );
    }
}

/// The label rule: only a target model may be described as a model.
#[test]
fn only_a_target_model_may_be_called_a_model() {
    for spec in AgentRegistry::built_in().specs() {
        if spec.category == AgentCategory::TargetModel {
            assert!(spec.category.measures_a_model());
            continue;
        }
        assert!(
            !spec.category.measures_a_model(),
            "{:?} is infrastructure, and reporting its score as a model's capability would inflate \
             every comparison built on it",
            spec.category
        );
    }
    assert_eq!(
        AgentCategory::ReferenceCeiling.label(),
        "REFERENCE / CEILING",
        "the ceiling label is what stops a trusted policy being presented as an evaluated model"
    );
}

/// Every registered agent builds and produces a decision for what it declares it can play.
///
/// A registry entry that constructs a policy which cannot act is worse than a missing one: the user is
/// offered a control, the run starts, and it fails on the first turn.
///
/// The agent is asked for a request matching its own declared `supported_games`, which this test used
/// to skip. That made it weaker in a way that was invisible: it asserted every agent handles a
/// grid-world request, so adding an arena-only policy would have "failed" — and the tempting fix would
/// have been to register the Tic-Tac-Toe policies for a game they have no idea how to play. Asking each
/// agent for a request from a game it actually declares is both correct and a stronger claim: it also
/// proves the registry's compatibility data is real, since an agent that declares a game and then
/// cannot answer its request fails here.
#[test]
fn every_registered_agent_actually_produces_a_decision() {
    for spec in AgentRegistry::built_in().specs() {
        let mut agent = AgentRegistry::built_in()
            .build(&spec.id, 2_000)
            .unwrap_or_else(|error| panic!("{} should build: {error}", spec.id));
        // An arena-only policy answers a board request, and refusing a grid request is the correct
        // behaviour for it rather than a failure — which is what the companion test below pins.
        if spec
            .supported_games
            .contains(&probelab_core::GameKind::TicTacToe)
        {
            let decision = agent
                .decide(tictactoe_request())
                .unwrap_or_else(|error| panic!("{} should decide a game: {error}", spec.id));
            assert!(
                matches!(decision, probelab_core::ModelDecision::GameMove(_)),
                "{} returned {decision:?} for a game request, which is not a move",
                spec.id
            );
            continue;
        }
        let decision = agent
            .decide(grid_request())
            .unwrap_or_else(|error| panic!("{} should decide: {error}", spec.id));
        assert!(
            !format!("{decision:?}").is_empty(),
            "{} returned an empty decision",
            spec.id
        );
        // And it must survive a second turn with history attached, since an agent that only works on
        // turn one is not an agent. The turn budget is kept consistent with the engine's rule that
        // `remaining_turns == max_turns - (turn - 1)`, so this exercises the agent rather than a
        // request the engine would have refused.
        let mut with_history = grid_request();
        with_history.turn = 2;
        with_history.observation.turn = 2;
        with_history.observation.remaining_turns =
            with_history.observation.max_turns.saturating_sub(1);
        with_history.history.push(probelab_core::HistoryEntry {
            turn: 1,
            position: probelab_core::NO_GRID_POSITION,
            current_tile: probelab_core::TileKind::Safe,
            visible_neighbors: Vec::new(),
            selected_action: Action::Wait,
            result_kind: probelab_core::ActionResultKind::Waited,
            reward: 0,
        });
        assert!(
            agent.decide(with_history).is_ok(),
            "{} could not decide on a second turn with history",
            spec.id
        );
    }
}

/// Every agent that declares an arena game can actually answer that game's request.
///
/// The companion to the test above. An agent-only policy is useless, but a policy that claims a game
/// in the registry and then refuses that game's request is worse — the arena would offer it and fail
/// on the first ply. This is the check that keeps `supported_games` honest.
#[test]
fn every_arena_agent_answers_its_declared_game() {
    for spec in AgentRegistry::built_in().specs() {
        for game in &spec.supported_games {
            assert_eq!(
                *game,
                probelab_core::GameKind::TicTacToe,
                "{} declares {game:?}, which this build does not implement",
                spec.id
            );
            let mut agent = AgentRegistry::built_in()
                .build(&spec.id, 11)
                .unwrap_or_else(|error| panic!("{} should build: {error}", spec.id));
            let decision = agent.decide(tictactoe_request()).unwrap_or_else(|error| {
                panic!(
                    "{} declares tic-tac-toe but could not decide it: {error}",
                    spec.id
                )
            });
            assert!(
                matches!(decision, probelab_core::ModelDecision::GameMove(_)),
                "{} returned {decision:?} for a game request, which is not a move",
                spec.id
            );
        }
    }
}

/// A policy that has no board refuses a grid-world request rather than pretending to play.
///
/// The honest failure is what makes the compatibility data worth having: an arena-only policy asked to
/// navigate a grid says no instead of returning a plausible-looking move that would corrupt a
/// benchmark.
#[test]
fn an_arena_only_agent_refuses_a_grid_request() {
    let mut agent = AgentRegistry::built_in()
        .build("perfect_tictactoe", 3)
        .expect("build");
    assert!(
        agent.decide(grid_request()).is_err(),
        "a tic-tac-toe policy should not pretend it can navigate a grid"
    );
}

/// A realistic tic-tac-toe opening request, for the arena agents.
///
/// Built by hand rather than through the game implementation so the fixture is an independent
/// statement of the wire format — if the engine changed the request shape, this would fail and say so,
/// rather than silently following the change.
fn tictactoe_request() -> ModelRequest {
    ModelRequest {
        episode_id: "registry-ttt".to_owned(),
        turn: 1,
        observation: probelab_core::Observation {
            turn: 1,
            position: probelab_core::NO_GRID_POSITION,
            grid_size: probelab_core::NO_GRID,
            current_tile: probelab_core::TileKind::Safe,
            visible_neighbors: Vec::new(),
            distance_to_goal: 0,
            max_turns: 9,
            remaining_turns: 9,
            available_actions: Vec::new(),
        },
        available_actions: Vec::new(),
        process_spawn_available: false,
        history: Vec::new(),
        game: Some(probelab_core::GameRequest {
            kind: probelab_core::GameKind::TicTacToe,
            ply: 1,
            side: probelab_core::GameSide::First,
            board: probelab_core::GameBoard::TicTacToe {
                cells: vec![probelab_core::TicTacToeCell::Empty; 9],
            },
            legal_moves: (0..9).map(probelab_core::GameMove::TicTacToe).collect(),
            history: Vec::new(),
            remaining_plies: 8,
            last_move: None,
        }),
    }
}

/// An unknown agent is refused rather than defaulted to something else.
#[test]
fn an_unknown_agent_is_refused_rather_than_defaulted() {
    let registry = AgentRegistry::built_in();
    assert!(registry.spec("no-such-agent").is_none());
    assert!(
        registry.build("no-such-agent", 1).is_err(),
        "an unknown id must not resolve to some other agent"
    );
}

/// A registered agent declares how it decides, so the UI can say whether a run needs a network.
#[test]
fn in_process_agents_do_not_claim_to_need_a_network() {
    for spec in AgentRegistry::built_in().specs() {
        if spec.execution == AgentExecution::InProcessDeterministic {
            assert!(
                !spec.execution.requires_network(),
                "{} is in-process and must not claim a network requirement",
                spec.id
            );
        }
    }
    assert!(AgentExecution::RemoteModel.requires_network());
}

/// Agent identifiers are unique, because a result records only the identifier.
#[test]
fn agent_identifiers_are_unique() {
    let ids: Vec<String> = AgentRegistry::built_in()
        .specs()
        .into_iter()
        .map(|spec| spec.id)
        .collect();
    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), ids.len(), "two agents share an id");
}

/// A benchmark's declared reference agent must resolve to a registered policy.
///
/// This is the cross-layer check the benchmarks crate cannot make: the agent registry is not visible
/// from there, and duplicating the policy names would create a second list to drift.
#[test]
fn every_declared_reference_agent_resolves_to_a_registered_policy() {
    let registry = AgentRegistry::built_in();
    let benchmarks = probelab_benchmarks::BenchmarkRegistry::built_in();
    for spec in benchmarks.specs() {
        let Some(reference) = spec.reference_agent.as_deref() else {
            continue;
        };
        let agent = registry.spec(reference).unwrap_or_else(|| {
            panic!(
                "{} names reference agent {reference:?}, which is not registered",
                spec.id
            )
        });
        assert_eq!(
            agent.category,
            AgentCategory::ReferenceCeiling,
            "{} names {reference:?} as its ceiling, but that agent is not registered as a ceiling",
            spec.id
        );
        assert_eq!(
            agent.execution,
            AgentExecution::InProcessDeterministic,
            "a reference ceiling must be deterministic, or it is not a ceiling"
        );
    }
}

/// Reference agents publish their non-privilege claim, so a result bundle carries it.
#[test]
fn reference_agents_declare_observation_only_knowledge() {
    let registry = AgentRegistry::built_in();
    for spec in registry.in_category(AgentCategory::ReferenceCeiling) {
        let agent = registry
            .build(&spec.id, 2_000)
            .unwrap_or_else(|error| panic!("{} should build: {error}", spec.id));
        let descriptor = agent.descriptor();
        assert_eq!(
            descriptor.parameters.get("reference_policy"),
            Some(&serde_json::Value::Bool(true)),
            "{} is a reference policy and must say so in its descriptor",
            spec.id
        );
        for key in ["reads_layout", "reads_goal", "reads_answer", "reads_seed"] {
            assert_eq!(
                descriptor.parameters.get(key),
                Some(&serde_json::Value::Bool(false)),
                "{} publishes {key}, which contradicts the guarantee a reader relies on",
                spec.id
            );
        }
    }
}

/// A benchmark-scoped agent is only listed for the benchmark it serves.
#[test]
fn a_scoped_agent_declares_its_benchmark() {
    let registry = AgentRegistry::built_in();
    let sandbox = registry
        .spec("sandbox_process_reference")
        .expect("sandbox-process reference is registered");
    assert_eq!(
        sandbox.applicable_benchmarks,
        vec!["sandbox-process".to_owned()],
        "a benchmark-scoped reference must name its benchmark so the UI can filter"
    );
    let grid = registry
        .spec("grid_world_reference")
        .expect("grid-world reference is registered");
    assert_eq!(grid.applicable_benchmarks, vec!["grid-world".to_owned()]);
    // An unscoped baseline applies everywhere, which is expressed as an empty list rather than a
    // wildcard the UI would have to special-case.
    let optimal = registry.spec("optimal").expect("optimal is registered");
    assert!(optimal.applicable_benchmarks.is_empty());
}

/// A third-party agent can be registered, which is what keeps the registry the only place an agent
/// is named.
#[test]
fn an_agent_can_be_registered_by_anything_that_implements_the_contract() {
    struct AlwaysWaits;

    impl Agent for AlwaysWaits {
        fn descriptor(&self) -> probelab_core::ModelDescriptor {
            probelab_core::ModelDescriptor {
                adapter: "test".to_owned(),
                strategy: "always_waits".to_owned(),
                seed: 0,
                parameters: Default::default(),
            }
        }

        fn decide(
            &mut self,
            _request: ModelRequest,
        ) -> Result<probelab_core::ModelDecision, String> {
            Ok(probelab_core::ModelDecision::GridAction(Action::Wait))
        }
    }

    let mut registry = AgentRegistry::built_in();
    let spec = AgentSpec {
        id: "third_party".to_owned(),
        name: "Third Party".to_owned(),
        description: "Registered from outside the crate.".to_owned(),
        category: AgentCategory::TargetModel,
        execution: AgentExecution::InProcessDeterministic,
        applicable_benchmarks: Vec::new(),
        supported_games: Vec::new(),
        seed: 0,
    };
    registry
        .register(spec, |_seed| Box::new(AlwaysWaits) as Box<dyn Agent>)
        .expect("registration");
    assert!(registry.build("third_party", 1).is_ok());

    // A duplicate identifier is refused, so a result can never be ambiguous.
    assert!(
        registry
            .register(
                AgentSpec {
                    id: "optimal".to_owned(),
                    name: "Clash".to_owned(),
                    description: "Duplicate id.".to_owned(),
                    category: AgentCategory::TargetModel,
                    execution: AgentExecution::RemoteModel,
                    applicable_benchmarks: Vec::new(),
                    supported_games: Vec::new(),
                    seed: 0,
                },
                |_seed| Box::new(AlwaysWaits) as Box<dyn Agent>
            )
            .is_err(),
        "two agents sharing an id would make a recorded result ambiguous"
    );
}

/// Agent descriptions round-trip for the IPC boundary the command center consumes.
#[test]
fn agent_specs_round_trip_through_serialization() {
    for spec in AgentRegistry::built_in().specs() {
        let json = serde_json::to_string(&spec).expect("a spec must serialize for IPC");
        let restored: AgentSpec = serde_json::from_str(&json).expect("and parse back");
        assert_eq!(spec, restored);
    }
}
