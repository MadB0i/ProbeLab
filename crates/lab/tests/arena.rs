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
        .play_simple(
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

/// The catalog offers exactly the games this build can actually play.
///
/// Asserting the *set* is the point, in both directions. Listing a game with types in the schema but
/// no implementation behind this command would offer a match that fails on the first ply, and the
/// set assertion is what stops rock-paper-scissors or battleship from quietly appearing in a dropdown
/// because somebody added a type. The geometry is pinned too, because a renderer lays the board out
/// from these numbers and a wrong row count draws a game nobody played.
#[test]
fn only_implemented_games_are_offered() {
    let games = available_games();
    let ids: Vec<&str> = games.iter().map(|game| game.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["tic_tac_toe", "connect_four"],
        "the arena offers exactly the games it implements, in a stable order"
    );
    assert_eq!(
        (games[0].width, games[0].height, games[0].max_plies),
        (3, 3, 9)
    );
    assert_eq!(
        (games[1].width, games[1].height, games[1].max_plies),
        (7, 6, 42),
        "connect four is a seven-by-six board that fills in forty-two plies"
    );
}

/// Every offered game is playable end to end by the agents the registry seats for it.
///
/// The catalog and the runner are separate lists that have to agree, and a test that only checks the
/// catalog's length would pass while `play` refused a listed game. This closes that gap by actually
/// playing each one.
#[test]
fn every_offered_game_is_playable_by_its_registered_agents() {
    for game in available_games() {
        let kind = match game.id.as_str() {
            "tic_tac_toe" => GameKind::TicTacToe,
            "connect_four" => GameKind::ConnectFour,
            other => panic!("{other} is offered but this test does not know its kind"),
        };
        let runner = ArenaRunner::new();
        let agents = runner.agents_for(kind);
        assert!(
            agents.len() >= 2,
            "{} needs at least two agents to form a match, has {}",
            game.id,
            agents.len()
        );
        let played = runner.play_simple(
            &ArenaMatchSpec::new(
                kind,
                &agents[0].id,
                &agents[1].id,
                GameSide::First,
                7,
                "catalog-check",
            ),
            None,
        );
        let match_view = match played {
            Ok(played) => played.view,
            Err(error) => panic!("{} is offered but could not be played: {error}", game.id),
        };
        assert!(
            match_view.plies > 0,
            "{} produced no plies, so its outcome would be vacuous",
            game.id
        );
        assert_eq!(
            match_view.plies,
            u32::try_from(match_view.history.len())
                .expect("a board cannot exceed four billion plies"),
            "the recorded ply count and the recorded history must be the same fact"
        );
        assert!(!match_view.turns.is_empty(), "every ply reports its cost");
        assert!(
            match_view.failure.is_none(),
            "{} failed: {:?}",
            game.id,
            match_view.failure
        );
    }
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

/// The recorded winning line is the engine's own, and it agrees with the terminal board.
///
/// The panel highlights these cells, so they have to come from the engine rather than be re-derived
/// in the renderer. This asserts the line belongs to the winner, is complete on the final board, and
/// is the cells the winner actually completed.
#[test]
fn a_win_records_the_winning_line_from_the_engine() {
    let played = play(FIRST_LEGAL, PERFECT, GameSide::First);
    assert_eq!(played.view.outcome, ArenaOutcome::Loss);
    let line = played
        .view
        .winning_line
        .expect("a decided match records the line the winner completed");
    assert_eq!(line.len(), 3);
    let cells = cells(&played.view.final_board);
    let winner_mark = match played.view.winner.expect("a winner") {
        GameSide::First => "First",
        GameSide::Second => "Second",
    };
    // The line is a list of `(column, row)` coordinates. Tic-tac-toe reports the second component as
    // zero, so the row is derived from the index rather than read from the pair — and the assertion
    // is that the *index* the engine named holds the winner's mark.
    for [index, row] in &line {
        assert_eq!(
            *row, 0,
            "tic-tac-toe reports a flat cell index, not a two-dimensional coordinate"
        );
        assert_eq!(
            cells[usize::from(*index)],
            winner_mark,
            "cell {index} is not the winner's mark, so this is not a line the winner completed"
        );
    }
    assert_ne!(
        line[0], line[1],
        "a line cannot complete on the same cell twice"
    );
}

/// A draw has no winning line. Nobody won, so there is nothing to highlight.
#[test]
fn a_draw_records_no_winning_line() {
    let played = play(PERFECT, PERFECT, GameSide::First);
    assert_eq!(played.view.outcome, ArenaOutcome::Draw);
    assert_eq!(played.view.winner, None);
    assert_eq!(
        played.view.winning_line, None,
        "a draw must not report a winning line; inventing one would claim a winner nobody had"
    );
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
    let outcome = ArenaRunner::new().play_simple(
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

/// A game with a schema type but no implementation is refused, not half-played.
///
/// Connect four used to be in this list; it is not any more, and the list is derived from the catalog
/// rather than written out, so a game shipping later removes itself from the assertion instead of
/// leaving a test that quietly contradicts the product.
#[test]
fn an_unimplemented_game_is_refused() {
    let offered: Vec<String> = available_games().into_iter().map(|game| game.id).collect();
    let all = [
        GameKind::TicTacToe,
        GameKind::ConnectFour,
        GameKind::Rps,
        GameKind::Battleship,
    ];
    for game in all {
        let outcome = ArenaRunner::new().play_simple(
            &ArenaMatchSpec::new(game, PERFECT, PERFECT, GameSide::First, 1, "m"),
            None,
        );
        if !offered.contains(&game.as_str().to_owned()) {
            assert!(
                matches!(outcome, Err(probelab_lab::ArenaRunError::UnknownGame(_))),
                "{game:?} has no implementation and must be refused, got {outcome:?}"
            );
            continue;
        }
        // The game is listed, so the refusal must be about the *agents*, never about the game. A
        // listed game that reports `UnknownGame` would be a catalog that lies, and one that accepts
        // a tic-tac-toe policy would be a compatibility check that does not check.
        match outcome {
            Ok(played) => {
                assert_eq!(
                    played.view.game,
                    game.as_str(),
                    "a listed game must report its own identifier"
                );
            }
            Err(probelab_lab::ArenaRunError::IncompatibleAgent { .. }) => {}
            Err(other) => panic!(
                "{game:?} is listed, so seating the wrong policy must be an incompatibility, got {other:?}"
            ),
        }
    }
}

/// An unknown agent is refused in either seat.
#[test]
fn an_unknown_agent_is_refused() {
    for (tested, opponent) in [("nope", PERFECT), (PERFECT, "nope")] {
        let outcome = ArenaRunner::new().play_simple(
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
        .play_simple(
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
