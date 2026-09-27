//! Arena tests.
//!
//! The arena is a projection over an engine that already worked, so most of what these cover is
//! whether the *projection* is honest: that a board shown is the engine's board, that a failure is not
//! reported as a loss, and that an agent the registry says cannot play a game is refused rather than
//! offered.

use std::sync::{Arc, Mutex};

use probelab_core::{GameBoard, GameKind, GameSide, TelemetryEvent, TelemetryRecorder};
use probelab_lab::{
    available_games, AgentRegistry, ArenaMatchSpec, ArenaOutcome, ArenaRunner, IllegalMoveView,
    PlyView,
};

/// The default pairing: a policy under test against the perfect opponent.
const PERFECT: &str = "perfect_tictactoe";
const FIRST_LEGAL: &str = "first_legal_tictactoe";

#[derive(Default)]
struct CountingRecorder {
    events: Mutex<usize>,
}

impl TelemetryRecorder for CountingRecorder {
    fn record(&self, _event: &TelemetryEvent) -> Result<(), String> {
        *self.events.lock().expect("recorder lock") += 1;
        Ok(())
    }
}

fn play(tested: &str, opponent: &str, tested_side: GameSide) -> probelab_lab::ArenaMatch {
    ArenaRunner::new()
        .play(
            &ArenaMatchSpec::new(
                GameKind::TicTacToe,
                tested,
                opponent,
                tested_side,
                7,
                "match-1",
            ),
            None,
        )
        .unwrap_or_else(|error| panic!("{tested} vs {opponent} should play: {error}"))
}

fn cells(board: &GameBoard) -> Vec<String> {
    match board {
        GameBoard::TicTacToe { cells } => cells.iter().map(|cell| format!("{cell:?}")).collect(),
        other => panic!("expected a tic-tac-toe board, got {other:?}"),
    }
}

/// The catalog offers Tic-Tac-Toe and nothing else.
///
/// Asserting the length is the point: connect four, rock-paper-scissors, and battleship have types in
/// the schema but no implementation behind this command, and listing them would offer a match that
/// fails on the first ply.
#[test]
fn only_tic_tac_toe_is_offered() {
    let games = available_games();
    assert_eq!(games.len(), 1);
    assert_eq!(games[0].id, "tic_tac_toe");
    assert_eq!(games[0].width, 3);
    assert_eq!(games[0].height, 3);
    assert_eq!(games[0].max_plies, 9);
}

/// A real match runs to a real terminal state.
#[test]
fn a_match_runs_to_a_terminal_state() {
    let played = play(FIRST_LEGAL, PERFECT, GameSide::First);
    let view = played.view;

    assert_eq!(view.game, "tic_tac_toe");
    assert_eq!(view.match_id, "match-1");
    assert_eq!(view.tested_agent, FIRST_LEGAL);
    assert_eq!(view.opponent_agent, PERFECT);
    assert_eq!(view.tested_side, GameSide::First);
    assert_eq!(view.seed, 7);
    assert!(view.plies > 0, "a real match plays plies");
    assert!(view.plies <= 9, "tic-tac-toe cannot exceed nine plies");
    assert!(!view.history.is_empty(), "every ply is recorded");
    assert!(view.failure.is_none());
}

/// Perfect against perfect is a draw, from either seat.
///
/// This is the arena's own self-check. Tic-tac-toe is exhaustively solvable, so if a future change to
/// either policy made it stronger than perfect, the draw would become a win and this would fail.
#[test]
fn perfect_against_perfect_is_a_draw_from_either_seat() {
    for side in [GameSide::First, GameSide::Second] {
        let played = play(PERFECT, PERFECT, side);
        assert_eq!(
            played.view.outcome,
            ArenaOutcome::Draw,
            "perfect play against perfect play must draw, playing {side:?}"
        );
        assert!(played.view.winner.is_none(), "a draw has no winner");
    }
}

/// The weak policy loses to perfect, and the panel says which side won.
#[test]
fn a_weak_policy_loses_and_the_winner_is_named() {
    let played = play(FIRST_LEGAL, PERFECT, GameSide::First);
    assert_eq!(played.view.outcome, ArenaOutcome::Loss);
    // The tested side lost, so the *other* side won. Getting this backwards would tell a user its own
    // policy won.
    assert_eq!(played.view.winner, Some(GameSide::Second));
    assert!(played.result.expect("a result").lost());
}

/// A weak policy playing second also loses, which checks the seat handling.
#[test]
fn a_weak_policy_also_loses_from_the_second_seat() {
    let played = play(FIRST_LEGAL, PERFECT, GameSide::Second);
    assert_eq!(played.view.outcome, ArenaOutcome::Loss);
    assert_eq!(played.view.winner, Some(GameSide::First));
}

/// The final board is full on a completed nine-ply game, and it is the engine's own board.
#[test]
fn a_completed_game_ends_with_a_full_board() {
    let played = play(PERFECT, PERFECT, GameSide::First);
    let board = cells(&played.view.final_board);
    assert_eq!(board.len(), 9);
    assert!(
        board.iter().all(|cell| cell != "\"Empty\""),
        "a drawn game fills every cell, got {board:?}"
    );
    // And the last recorded ply shows the same board, so the projection and the final position agree.
    let last: &PlyView = played.view.history.last().expect("a last ply");
    assert_eq!(last.board, played.view.final_board);
}

/// The recorded history is in order, one board per ply, each the position *after* its move.
#[test]
fn the_history_is_ordered_and_one_ply_per_entry() {
    let played = play(FIRST_LEGAL, PERFECT, GameSide::First);
    let history = &played.view.history;

    let plies: Vec<u32> = history.iter().map(|ply| ply.ply).collect();
    let expected: Vec<u32> = (1..=history.len() as u32).collect();
    assert_eq!(plies, expected, "plies must be one-based and in order");

    for ply in history {
        let board = cells(&ply.board);
        assert_eq!(
            board.len(),
            9,
            "every recorded board is a full nine-cell board"
        );
    }
}

/// A legal policy produces no illegal moves, and the projection reports none.
#[test]
fn a_legal_policy_produces_no_illegal_moves() {
    let played = play(PERFECT, PERFECT, GameSide::First);
    assert!(
        played.view.illegal_moves.is_empty(),
        "a perfect policy should never be refused, got {:?}",
        played.view.illegal_moves
    );
    assert_eq!(played.view.illegal_move_count(), 0);
}

/// A refused move is recorded with its reason, not reduced to a count.
#[test]
fn a_refusal_records_the_ply_and_the_reason() {
    // The engine's refusal names a ply and a reason but not the move, so the projection must leave the
    // move absent rather than inventing one to fill the field.
    let view = IllegalMoveView {
        ply: 3,
        attempted: None,
        reason: "that cell is already taken".to_owned(),
    };
    let json = serde_json::to_string(&view).expect("serialize");
    assert!(json.contains("\"ply\":3"));
    assert!(
        json.contains("\"attempted\":null"),
        "an unidentified move must serialize as null, not as a guess"
    );
}

/// Agent compatibility is decided by the registry, not the caller.
#[test]
fn only_compatible_agents_are_offered() {
    let runner = ArenaRunner::new();
    let offered = runner.agents_for(GameKind::TicTacToe);
    let ids: Vec<&str> = offered.iter().map(|spec| spec.id.as_str()).collect();
    assert!(ids.contains(&PERFECT));
    assert!(ids.contains(&FIRST_LEGAL));
    // And a grid-world agent must not appear, because it has no idea what tic-tac-toe is.
    assert!(
        !ids.contains(&"optimal"),
        "a grid policy was offered a board"
    );
    assert!(!ids.contains(&"grid_world_reference"));
}

/// An incompatible pairing is refused before anything runs.
#[test]
fn an_incompatible_agent_is_refused() {
    let outcome = ArenaRunner::new().play(
        &ArenaMatchSpec::new(
            GameKind::TicTacToe,
            "optimal",
            PERFECT,
            GameSide::First,
            1,
            "match-x",
        ),
        None,
    );
    assert!(
        matches!(
            outcome,
            Err(probelab_lab::ArenaRunError::IncompatibleAgent { .. })
        ),
        "a grid policy must be refused for a board game, got {outcome:?}"
    );
}

/// An unimplemented game is refused, not half-played.
#[test]
fn an_unimplemented_game_is_refused() {
    for game in [GameKind::ConnectFour, GameKind::Rps, GameKind::Battleship] {
        let outcome = ArenaRunner::new().play(
            &ArenaMatchSpec::new(game, PERFECT, PERFECT, GameSide::First, 1, "m"),
            None,
        );
        assert!(
            matches!(outcome, Err(probelab_lab::ArenaRunError::UnknownGame(_))),
            "{game:?} has no implementation and must be refused, got {outcome:?}"
        );
    }
}

/// An unknown agent is refused in either seat.
#[test]
fn an_unknown_agent_is_refused() {
    for (tested, opponent) in [("nope", PERFECT), (PERFECT, "nope")] {
        let outcome = ArenaRunner::new().play(
            &ArenaMatchSpec::new(
                GameKind::TicTacToe,
                tested,
                opponent,
                GameSide::First,
                1,
                "m",
            ),
            None,
        );
        assert!(
            matches!(outcome, Err(probelab_lab::ArenaRunError::UnknownAgent(_))),
            "{tested} vs {opponent} should be refused, got {outcome:?}"
        );
    }
}

/// A grid-world agent has no reference-policy form, and says so distinctly.
#[test]
fn a_non_policy_agent_has_no_reference_form() {
    let registry = AgentRegistry::built_in();
    assert!(
        registry.build(PERFECT, 0).is_ok(),
        "a policy builds as an agent"
    );
    // Matched rather than `expect_err`, because the success type is a `Box<dyn ReferenceStrategy>`
    // which is not `Debug` and so cannot be printed by the assertion macros.
    match registry.build_reference("optimal", 0) {
        Err(probelab_lab::AgentBuildError::NotAReferencePolicy { .. }) => {}
        Err(other) => panic!("expected a not-a-policy error, got {other:?}"),
        Ok(_) => panic!("a grid policy must not have a reference form"),
    }
}

/// The same seed replays the same match.
#[test]
fn a_seed_replays_the_same_match() {
    let first = play(PERFECT, PERFECT, GameSide::First);
    let second = play(PERFECT, PERFECT, GameSide::First);
    assert_eq!(first.view.final_board, second.view.final_board);
    assert_eq!(first.view.plies, second.view.plies);
    let first_boards: Vec<GameBoard> = first.view.history.iter().map(|p| p.board.clone()).collect();
    let second_boards: Vec<GameBoard> = second
        .view
        .history
        .iter()
        .map(|p| p.board.clone())
        .collect();
    assert_eq!(
        first_boards, second_boards,
        "the same seed must replay exactly"
    );
}

/// The outcome vocabulary distinguishes a draw, a loss, and no result.
#[test]
fn the_outcome_vocabulary_is_four_distinct_facts() {
    // A harness failure must never be reportable as a loss, so `NoResult` exists and is not an alias.
    assert_ne!(ArenaOutcome::NoResult, ArenaOutcome::Loss);
    assert_ne!(ArenaOutcome::NoResult, ArenaOutcome::Draw);
    assert_eq!(ArenaOutcome::NoResult.label(), "NO RESULT");
    assert_eq!(ArenaOutcome::Draw.label(), "DRAW");
}

/// The match timeline uses the canonical event vocabulary.
#[test]
fn the_timeline_uses_the_canonical_event_vocabulary() {
    let played = play(PERFECT, PERFECT, GameSide::First);
    let codes: Vec<&str> = played
        .view
        .timeline
        .iter()
        .map(|event| event.code.as_str())
        .collect();
    assert!(codes.contains(&"arena.ply_recorded"));
    assert!(codes.contains(&"arena.match_completed"));
    // And no event in an arena match may claim to be a security finding: a clean game has none.
    for event in &played.view.timeline {
        assert!(
            !event.is_security_finding(),
            "{} claimed a security finding in a clean match",
            event.code
        );
    }
}

/// A durable recorder receives the same stream the panel holds.
#[test]
fn a_durable_recorder_receives_the_match() {
    let recorder = Arc::new(CountingRecorder::default());
    let sink: Arc<dyn TelemetryRecorder> = recorder.clone();
    ArenaRunner::new()
        .play(
            &ArenaMatchSpec::new(
                GameKind::TicTacToe,
                PERFECT,
                PERFECT,
                GameSide::First,
                7,
                "match-1",
            ),
            Some(sink),
        )
        .expect("play");
    assert!(
        *recorder.events.lock().expect("recorder lock") > 0,
        "the tee must forward from the first ply, not the last"
    );
}

/// The match view round-trips through the IPC encoding.
#[test]
fn the_match_view_round_trips() {
    let played = play(PERFECT, PERFECT, GameSide::First);
    let json = serde_json::to_string(&played.view).expect("serialize");
    let restored: probelab_lab::ArenaMatchView = serde_json::from_str(&json).expect("parse");
    assert_eq!(restored, played.view);
    assert!(
        json.contains("\"testedAgent\""),
        "the wire format is camelCase"
    );
    assert!(json.contains("\"finalBoard\""));
}

/// No credential can reach an arena result.
#[test]
fn an_arena_result_never_carries_a_credential() {
    let played = play(PERFECT, PERFECT, GameSide::First);
    let json = serde_json::to_string(&played.view).expect("serialize");
    for marker in [
        "sk-",
        "sk-ant",
        "AIza",
        "api_key\":",
        "apiKey",
        "password",
        "token\":",
    ] {
        assert!(
            !json.contains(marker),
            "an arena result leaked something credential-shaped: {marker}"
        );
    }
}

/// Structural test: the arena cannot become a capability bypass.
///
/// Same spirit as the runner's. A game environment has no business constructing a capability request,
/// consulting a broker, or starting a process — and a structural assertion is checked on every build
/// rather than in a review comment. The arena's authority is the board, and nothing else.
#[test]
fn the_arena_is_not_a_capability_authority() {
    let source = include_str!("../src/arena.rs");
    for forbidden in [
        "CapabilityBroker",
        "PolicyDecision",
        "RuntimeAuthorization",
        "CapabilityRequest",
        "RequestedCapability",
        "Command::new",
        "std::process",
    ] {
        assert!(
            !source.contains(forbidden),
            "the arena contains {forbidden:?}; a game environment must not be able to reach the \
             enforcement layers"
        );
    }
    // It must also not be able to score anything. Scoring belongs to the evaluator, and an arena that
    // evaluated itself would be a second evaluator.
    assert!(
        !source.contains("fn evaluate"),
        "the arena must not be able to evaluate anything"
    );
}
