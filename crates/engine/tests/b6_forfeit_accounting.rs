//! Audit finding B6 — failure accounting at the engine boundary.
//!
//! The defect these cover: a tested seat that was reached, answered, and returned something no legal
//! move could be read from ended the match with an *error*. The lab layer turned every error into
//! `NoResult`, which is excluded from every rate denominator. A model that forfeited every single
//! match therefore reported *no results at all* rather than *a loss in every match*.
//!
//! These sit at the engine boundary because that is where the distinction is made. The engine knows
//! whether a seat was reached and answered unusably; the statistics layer can only report what it is
//! told, and it is told the wrong thing if this returns an error.

use probelab_core::{GameSide, ModelCallFailure, ModelCallFailureKind, ModelRequest};
use probelab_engine::arena::tictactoe::{PerfectTicTacToe, TicTacToe};
use probelab_engine::arena::{
    play_match, ArenaError, ArenaSeat, MatchConfig, MatchOutcome, MatchTermination, NullRecorder,
    PlyOutcome, ReferenceSeat, SeatDecision, SeatKind,
};

/// A seat that answers every request with something no legal move can be read from.
///
/// This is the ordinary failure of a small model asked for a move: it replies in prose, or invents a
/// cell that is not on the board. The provider answered, so this is the model's failure and not the
/// harness's — which is exactly why it must not be reported as a harness fault.
struct UnusableSeat {
    label: &'static str,
}

impl ArenaSeat for UnusableSeat {
    fn label(&self) -> &str {
        self.label
    }

    fn kind(&self) -> SeatKind {
        SeatKind::Model
    }

    fn decide(&mut self, _request: ModelRequest) -> Result<SeatDecision, ArenaError> {
        Err(ArenaError::SeatRefused {
            label: self.label.to_owned(),
            failure: Box::new(ModelCallFailure {
                kind: ModelCallFailureKind::InvalidAction,
                message: "the response contained no legal move".to_owned(),
                response_text: Some("I think it should go in the middle somewhere".to_owned()),
                usage: None,
                elapsed_ms: Some(41),
                http_status: Some(200),
            }),
        })
    }
}

/// A seat that cannot reach its provider at all.
///
/// The opposite case: the model was never asked, so nothing about it was measured.
struct UnreachableSeat {
    label: &'static str,
}

impl ArenaSeat for UnreachableSeat {
    fn label(&self) -> &str {
        self.label
    }

    fn kind(&self) -> SeatKind {
        SeatKind::Model
    }

    fn decide(&mut self, _request: ModelRequest) -> Result<SeatDecision, ArenaError> {
        Err(ArenaError::SeatFailed {
            label: self.label.to_owned(),
            failure: Box::new(ModelCallFailure {
                kind: ModelCallFailureKind::ConnectionRefused,
                message: "nothing is listening on that port".to_owned(),
                response_text: None,
                usage: None,
                elapsed_ms: None,
                http_status: None,
            }),
        })
    }
}

fn opponent() -> ReferenceSeat {
    ReferenceSeat::new(Box::new(PerfectTicTacToe::new()))
}

fn config() -> MatchConfig {
    MatchConfig::new("unusable-model", "perfect", 7, "run", "episode")
}

#[test]
fn a_tested_seat_that_answers_unusably_forfeits_and_stays_in_the_result() {
    let game = TicTacToe::default_game();
    let result = play_match(
        &game,
        &mut UnusableSeat {
            label: "unusable-model",
        },
        &mut opponent(),
        GameSide::First,
        &config(),
        &NullRecorder,
        &mut |_outcome: PlyOutcome| true,
    )
    .expect("a refusal from the tested seat is a result, not an engine error");

    assert_eq!(
        result.outcome,
        MatchOutcome::Loss,
        "a seat that names no legal move forfeits — the engine's own illegal-move path has always done \
         exactly this, and the refusal is the same kind of event"
    );
    assert_eq!(
        result.termination,
        MatchTermination::ModelRuleViolation,
        "the seat was reached and answered, so this is a game outcome and not an infrastructure fault"
    );
    assert!(
        result.termination.is_conditional(),
        "a forfeit belongs in the win/draw/loss population"
    );
}

#[test]
fn the_forfeit_records_why_without_inventing_a_move() {
    let game = TicTacToe::default_game();
    let result = play_match(
        &game,
        &mut UnusableSeat {
            label: "unusable-model",
        },
        &mut opponent(),
        GameSide::First,
        &config(),
        &NullRecorder,
        &mut |_outcome: PlyOutcome| true,
    )
    .expect("forfeit");

    assert_eq!(
        result.illegal_moves.len(),
        1,
        "the refusal is recorded as the rule violation it is, not dropped on the floor"
    );
    let refusal = &result.illegal_moves[0];
    assert_eq!(refusal.ply, 1, "it happened on the first ply");
    assert!(
        refusal.reason.contains("unusable-model"),
        "the reason names the seat: {}",
        refusal.reason
    );
    assert!(
        refusal.reason.contains("no legal move"),
        "the reason carries the provider's own message: {}",
        refusal.reason
    );
    assert_eq!(
        refusal.attempted, None,
        "no move can be reported when the response never parsed into one, and a placeholder cell would \
         fabricate the one datum a reader most wants"
    );
}

#[test]
fn an_unreachable_provider_is_not_a_forfeit() {
    // The distinction the whole finding turns on. A refused connection means the model never got to
    // answer, so it must stay an infrastructure failure and must not enter the conditional denominator.
    let game = TicTacToe::default_game();
    let error = play_match(
        &game,
        &mut UnreachableSeat {
            label: "absent-model",
        },
        &mut opponent(),
        GameSide::First,
        &MatchConfig::new("absent-model", "perfect", 7, "run", "episode"),
        &NullRecorder,
        &mut |_outcome: PlyOutcome| true,
    )
    .expect_err("an unreachable provider is an infrastructure failure, not a result");

    assert!(
        matches!(error, ArenaError::SeatFailed { .. }),
        "expected a harness failure, got {error:?}"
    );
    assert!(
        error.is_harness_failure(),
        "the lab layer decides between forfeit and infrastructure from exactly this"
    );
    assert!(
        !error.is_seat_protocol_failure(),
        "a refused connection is not the seat breaking the move protocol"
    );
}

#[test]
fn a_legal_but_refused_move_is_still_recorded_with_the_move_it_played() {
    // The neighbouring case, kept deliberately, because the two are easy to conflate. Here the
    // response *did* parse into a move and the board then refused it, so the move is known and must be
    // reported rather than discarded — and the termination stays an ordinary decision.
    struct IllegalSeat;
    impl ArenaSeat for IllegalSeat {
        fn label(&self) -> &str {
            "illegal-move-model"
        }
        fn kind(&self) -> SeatKind {
            SeatKind::Model
        }
        fn decide(&mut self, _request: ModelRequest) -> Result<SeatDecision, ArenaError> {
            // Always the same cell. It is legal once and refused on every turn after that, which is
            // exactly the "understood nothing and kept trying it" failure the distinction is about.
            Ok(SeatDecision {
                move_: probelab_core::GameMove::TicTacToe(4),
                call: None,
                failure: None,
            })
        }
    }

    let game = TicTacToe::default_game();
    let result = play_match(
        &game,
        &mut IllegalSeat,
        &mut opponent(),
        GameSide::First,
        &MatchConfig::new("illegal-move-model", "perfect", 3, "run", "episode"),
        &NullRecorder,
        &mut |_outcome: PlyOutcome| true,
    )
    .expect("a refused move from the tested seat is a forfeit");

    assert_eq!(result.outcome, MatchOutcome::Loss);
    assert_eq!(
        result.termination,
        MatchTermination::Decided,
        "the rules produced the result; the seat simply picked badly, which is ordinary play"
    );
    assert!(!result.illegal_moves.is_empty());
    assert!(
        result
            .illegal_moves
            .iter()
            .all(|illegal| illegal.attempted.is_some()),
        "the move parsed, so it is known and is reported rather than discarded"
    );
}
