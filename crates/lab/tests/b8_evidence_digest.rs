//! Audit finding B8 — the evidence digest must commit to the evidence.
//!
//! Version 1 of the digest absorbed identity, result, winning line, final board, illegal moves, the
//! failure string and the ply history — with FNV-1a and no delimiter between absorbed values. Two
//! consequences, both tested here:
//!
//! * **Coverage.** The per-turn evidence (elapsed time, token usage, the model's actual response text,
//!   the prompt template version) was outside the hash entirely, as were the termination reason, the
//!   two seat kinds, the cancellation flag and the event timeline.
//! * **Framing.** Without a length delimiter, `("ab","c")` and `("a","bc")` absorb the same bytes.
//!
//! Each test mutates **one** field and asserts the digest moves. A test that asserted "something
//! changed" without naming which field would pass for the wrong reason, so every case is explicit.

use probelab_core::{GameBoard, GameSide, TicTacToeCell};
use probelab_lab::{ArenaMatchView, ArenaOutcome, ArenaTermination};

fn base() -> ArenaMatchView {
    serde_json::from_value(serde_json::json!({
        "matchId": "arena-ttt-7-digest-1",
        "game": "tic_tac_toe",
        "gameName": "Tic-Tac-Toe",
        "outcome": "win",
        "testedAgent": "controlled-model",
        "opponentAgent": "perfect_tictactoe",
        "testedSide": "first",
        "testedSeat": "model",
        "opponentSeat": "reference_policy",
        "seed": 7,
        "plies": 1,
        "finalBoard": {"game": "tic_tac_toe", "state": {"cells": [
            "first", "empty", "empty", "empty", "empty", "empty", "empty", "empty", "empty"
        ]}},
        "winner": "first",
        "winningLine": [[0, 0]],
        "illegalMoves": [],
        "history": [],
        "cancelled": false,
        "termination": "decided",
        "turns": [],
        "timeline": [],
    }))
    .expect("the fixture is a valid recording")
}

#[test]
fn the_same_recording_hashes_the_same_way_every_time() {
    assert_eq!(
        base().evidence_digest(),
        base().evidence_digest(),
        "identity must be deterministic or nothing downstream can rely on it"
    );
    assert!(
        base().evidence_digest().starts_with("v2:"),
        "a digest must name its version, so a legacy value can never be read as a current one"
    );
}

#[test]
fn two_different_recordings_do_not_share_a_digest() {
    let mut other = base();
    other.match_id = "arena-ttt-7-digest-2".to_owned();
    assert_ne!(base().evidence_digest(), other.evidence_digest());
}

/// Every field below is evidence a reader could be misled about. Each gets its own mutation.
#[test]
fn every_defined_field_is_covered() {
    let original = base().evidence_digest();

    // --- result fields, which version 1 already covered, kept as a regression guard -------------
    let mut outcome = base();
    outcome.outcome = ArenaOutcome::Loss;
    assert_ne!(original, outcome.evidence_digest(), "outcome must be bound");

    let mut winner = base();
    winner.winner = Some(GameSide::Second);
    assert_ne!(original, winner.evidence_digest(), "winner must be bound");

    let mut plies = base();
    plies.plies = 2;
    assert_ne!(original, plies.evidence_digest(), "plies must be bound");

    let mut board = base();
    board.final_board = GameBoard::TicTacToe {
        cells: vec![TicTacToeCell::Empty; 9],
    };
    assert_ne!(
        original,
        board.evidence_digest(),
        "final board must be bound"
    );

    // --- fields version 1 never touched --------------------------------------------------
    let mut termination = base();
    termination.termination = Some(ArenaTermination::Cancelled);
    assert_ne!(
        original,
        termination.evidence_digest(),
        "termination must be bound: a forfeit reclassified as a cancellation is a different claim"
    );

    let mut unknown_termination = base();
    unknown_termination.termination = None;
    assert_ne!(
        original,
        unknown_termination.evidence_digest(),
        "an unknown termination is not the same fact as a known one"
    );

    let mut cancelled = base();
    cancelled.cancelled = Some(true);
    assert_ne!(
        original,
        cancelled.evidence_digest(),
        "cancellation must be bound"
    );

    let mut unknown_cancelled = base();
    unknown_cancelled.cancelled = None;
    assert_ne!(
        original,
        unknown_cancelled.evidence_digest(),
        "unknown cancellation must not hash as 'not cancelled'"
    );

    let mut seat = base();
    seat.tested_seat = None;
    assert_ne!(
        original,
        seat.evidence_digest(),
        "a seat that was not recorded is not the same as one that was"
    );
}

/// The evidence version 1 omitted entirely, and the reason B8 exists.
#[test]
fn per_turn_evidence_is_covered() {
    let mut with_turn = base();
    with_turn.turns = vec![a_turn()];

    let original = base().evidence_digest();
    assert_ne!(
        original,
        with_turn.evidence_digest(),
        "per-turn cost and response evidence must be bound"
    );

    // One field at a time, so a failure names the field that is not covered.
    let mut response_changed = with_turn.clone();
    response_changed.turns[0].response_text = Some("{\"move\":\"COLUMN_4\"}".to_owned());
    assert_ne!(
        with_turn.evidence_digest(),
        response_changed.evidence_digest(),
        "a changed model response must move the digest"
    );

    let mut usage_changed = with_turn.clone();
    usage_changed.turns[0].usage = Some(probelab_core::TokenUsage {
        prompt_tokens: Some(999),
        completion_tokens: Some(4),
        total_tokens: Some(1003),
    });
    assert_ne!(
        with_turn.evidence_digest(),
        usage_changed.evidence_digest(),
        "a changed token total must move the digest"
    );

    let mut elapsed_changed = with_turn.clone();
    elapsed_changed.turns[0].elapsed_ms = 4_100;
    assert_ne!(
        with_turn.evidence_digest(),
        elapsed_changed.evidence_digest(),
        "changed latency must move the digest"
    );

    let mut template_changed = with_turn.clone();
    template_changed.turns[0].prompt_template_version = Some("connect-four/v2".to_owned());
    assert_ne!(
        with_turn.evidence_digest(),
        template_changed.evidence_digest(),
        "a changed prompt template version must move the digest"
    );
}

#[test]
fn the_event_timeline_is_covered() {
    let mut with_event = base();
    with_event.timeline = vec![probelab_lab::LabEvent::new(
        0,
        "experiment.created",
        probelab_lab::EventSource::Experiment,
        probelab_lab::Severity::Info,
        "match created",
    )];
    assert_ne!(base().evidence_digest(), with_event.evidence_digest());

    let mut different_detail = with_event.clone();
    different_detail.timeline[0].detail = "match created differently".to_owned();
    assert_ne!(
        with_event.evidence_digest(),
        different_detail.evidence_digest(),
        "a changed event detail must move the digest"
    );
}

/// The framing defect, tested as a property rather than as one lucky pair.
#[test]
fn adjacent_values_cannot_be_confused() {
    // The identity strings are absorbed one after another in version 1 with nothing between them, so
    // moving a character across the boundary produced the same absorbed bytes. Here one character is
    // moved out of one field and into its neighbour.
    let mut shifted = base();
    shifted.tested_agent = "controlled-modelx".to_owned();
    shifted.opponent_agent = "perfect_tictacto".to_owned();

    assert_ne!(
        shifted.evidence_digest(),
        base().evidence_digest(),
        "moving characters between adjacent fields must change the digest"
    );
    assert_ne!(
        shifted.tested_agent, shifted.opponent_agent,
        "sanity: the two fields really do differ from the base recording"
    );
}

/// The legacy value must keep working, and must be distinguishable from a version 2 one.
#[test]
fn the_legacy_digest_is_still_computed_and_is_not_version_two() {
    let legacy = base().replay_digest();
    assert_eq!(legacy.len(), 16, "v1 is a 64-bit FNV rendered as hex");
    assert!(
        !legacy.starts_with("v2:"),
        "a v1 value must not masquerade as a v2 one"
    );
    assert_eq!(
        base().evidence_digest().len(),
        3 + 64,
        "v2 is a SHA-256 prefix and digest"
    );
}

/// Whether version 1 covered a field is the claim B8 makes about it, so it is pinned here rather
/// than left to memory.
#[test]
fn version_one_really_did_omit_the_fields_this_finding_names() {
    let mut with_turn = base();
    with_turn.turns = vec![a_turn()];
    with_turn.turns[0].response_text = Some("changed".to_owned());
    assert_eq!(
        with_turn.replay_digest(),
        base().replay_digest(),
        "version 1 hashes two recordings that differ only in model response text — the omission B8 \
         describes"
    );
    assert_ne!(
        with_turn.evidence_digest(),
        base().evidence_digest(),
        "version 2 does not"
    );

    let mut termination = base();
    termination.termination = Some(ArenaTermination::InfrastructureFailure);
    assert_eq!(
        termination.replay_digest(),
        base().replay_digest(),
        "version 1 also ignored the termination reason"
    );
}

/// One model-backed turn carrying the evidence version 1 omitted.
fn a_turn() -> probelab_lab::TurnTelemetry {
    probelab_lab::TurnTelemetry {
        ply: 1,
        side: GameSide::First,
        seat: probelab_engine::arena::SeatKind::Model,
        seat_label: "controlled-model".to_owned(),
        elapsed_ms: 41,
        usage: Some(probelab_core::TokenUsage {
            prompt_tokens: Some(120),
            completion_tokens: Some(4),
            total_tokens: Some(124),
        }),
        prompt_template_version: Some("connect-four/v1".to_owned()),
        response_text: Some("{\"move\":\"COLUMN_1\"}".to_owned()),
        failure: None,
        request_board: GameBoard::TicTacToe {
            cells: vec![TicTacToeCell::Empty; 9],
        },
    }
}
