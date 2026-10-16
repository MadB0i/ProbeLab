//! Audit finding B4 — historical recording compatibility.
//!
//! The archive contains recordings written in more than one shape. These tests pin which shapes exist,
//! that each is readable, and — the part that matters most — that reading a legacy recording does not
//! change what the archive holds.
//!
//! # What actually went wrong
//!
//! `ArenaMatchView::winning_line` was widened from a flat list of cell indices to a list of coordinate
//! pairs. Recordings written before that widening store `"winningLine":[2,4,6]`; current ones store
//! `"winningLine":[[2,0],[4,0],[6,0]]`. A decoder accepting only the current shape rejected every legacy
//! row with `invalid type: integer 2, expected an array of length 2` — 24 of the 59 recordings in the
//! archive this was found in, all listed in the index and none openable.
//!
//! The rule is exact rather than approximate: the second component of a coordinate is `0` for every game
//! this build plays, so a legacy index `i` maps onto the pair `[i, 0]` and nothing is lost. Genuinely
//! corrupt input is still refused.
//!
//! Every test here deserialises a **real `ArenaMatchView`**, not a local stand-in. An earlier version of
//! this file parsed a local struct with a plain `Vec<[u8;2]>`, which therefore had none of the
//! compatibility behaviour and failed five tests while appearing to test the right thing.

use probelab_lab::ArenaMatchView;

/// A complete, minimal, valid recording carrying the given `winningLine` JSON.
fn recording(winning_line_json: &str) -> serde_json::Value {
    serde_json::json!({
        "matchId": "arena-ttt-7-fixture-1",
        "game": "tic_tac_toe",
        "gameName": "Tic-Tac-Toe",
        "outcome": "loss",
        "testedAgent": "first_legal_tictactoe",
        "opponentAgent": "perfect_tictactoe",
        "testedSide": "second",
        "seed": 7,
        "plies": 7,
        "testedSeat": "reference_policy",
        "opponentSeat": "reference_policy",
        "finalBoard": {"game": "tic_tac_toe", "state": {"cells": [
            "first", "second", "first", "second", "first", "second", "first", "empty", "empty"
        ]}},
        "winner": "first",
        "illegalMoves": [],
        "history": [],
        "turns": [],
        "timeline": [],
        "cancelled": false,
        "winningLine": serde_json::from_str::<serde_json::Value>(winning_line_json).expect("fixture JSON"),
    })
}

fn read(winning_line_json: &str) -> Result<ArenaMatchView, serde_json::Error> {
    serde_json::from_value(recording(winning_line_json))
}

#[test]
fn a_legacy_flat_winning_line_is_readable() {
    // Exactly the bytes from a real legacy row in the archive this was found in.
    let view = read("[2,4,6]").expect("a legacy recording must be readable");
    assert_eq!(
        view.winning_line,
        Some(vec![[2, 0], [4, 0], [6, 0]]),
        "a flat index widens to the pair the current writer emits for the same line"
    );
}

#[test]
fn a_current_coordinate_winning_line_is_unchanged() {
    let view = read("[[2,0],[4,0],[6,0]]").expect("current shape reads");
    assert_eq!(view.winning_line, Some(vec![[2, 0], [4, 0], [6, 0]]));
}

#[test]
fn the_two_shapes_denote_the_same_line() {
    let legacy = read("[2,4,6]").expect("legacy reads").winning_line;
    let current = read("[[2,0],[4,0],[6,0]]")
        .expect("current reads")
        .winning_line;
    assert_eq!(
        legacy, current,
        "if the shapes denoted different lines the compatibility rule would be inventing something"
    );
}

#[test]
fn a_four_in_a_row_survives_in_both_shapes() {
    let legacy = read("[0,1,2,3]").expect("legacy four reads").winning_line;
    let current = read("[[0,0],[1,0],[2,0],[3,0]]")
        .expect("current four reads")
        .winning_line;
    assert_eq!(legacy, current);
    assert_eq!(legacy, Some(vec![[0, 0], [1, 0], [2, 0], [3, 0]]));
}

#[test]
fn an_absent_winning_line_is_absent_rather_than_empty() {
    let view: ArenaMatchView = serde_json::from_value(serde_json::json!({
        "matchId": "arena-ttt-7-no-line-1",
        "game": "tic_tac_toe",
        "gameName": "Tic-Tac-Toe",
        "outcome": "draw",
        "testedAgent": "first_legal_tictactoe",
        "opponentAgent": "perfect_tictactoe",
        "testedSide": "first",
        "seed": 7,
        "plies": 9,
        "testedSeat": "reference_policy",
        "opponentSeat": "reference_policy",
        "finalBoard": {"game": "tic_tac_toe", "state": {"cells": [
            "first", "empty", "second", "empty", "first", "second", "empty", "empty", "empty"
        ]}},
        "winner": null,
        "illegalMoves": [],
        "history": [],
        "turns": [],
        "timeline": [],
        "cancelled": false,
    }))
    .expect("a record without the field reads");

    assert_eq!(
        view.winning_line, None,
        "absent is not the same as an empty line"
    );

    let explicit_null = read("null").expect("an explicit null reads");
    assert_eq!(explicit_null.winning_line, None);
}

/// The compatibility layer is a version rule, not a repair rule.
///
/// A line that is neither shape is corruption. If it were accepted, a damaged recording would be
/// reported as a readable one and the archive would look healthier than it is.
#[test]
fn corruption_is_still_refused() {
    for corrupt in [
        "[999]",         // not a cell index
        "[[0],4]",       // mixed shapes
        "[[0,0],\"x\"]", // wrong element type
        "\"2,4,6\"",     // not a list at all
    ] {
        assert!(
            read(corrupt).is_err(),
            "{corrupt} should not be silently accepted as a winning line"
        );
    }
}

/// Reading must not rewrite.
///
/// The reader widens the line for the consumer; it does not write back. An earlier test in this file
/// asserted that the stored string equalled itself, which is true of any input and so proved nothing —
/// the meaningful half is that the *view handed to a caller* is the widened form while the bytes on
/// disk are the harness's own untouched fixture.
#[test]
fn reading_widens_for_the_caller_without_rewriting_the_source() {
    let stored = recording("[2,4,6]");
    let stored_winning_line = stored["winningLine"].clone();
    assert_eq!(
        stored_winning_line,
        serde_json::json!([2, 4, 6]),
        "the fixture still holds the legacy shape"
    );

    let view: ArenaMatchView =
        serde_json::from_value(stored.clone()).expect("the legacy view reads");

    assert_eq!(view.winning_line, Some(vec![[2, 0], [4, 0], [6, 0]]));
    assert_eq!(
        stored["winningLine"],
        serde_json::json!([2, 4, 6]),
        "reading does not modify the value that was read"
    );
}

/// A current recording must round-trip unchanged, or the compatibility layer has altered live evidence.
#[test]
fn a_current_recording_round_trips_unchanged() {
    let stored = recording("[[0,0],[1,0],[2,0],[3,0]]");
    let view: ArenaMatchView = serde_json::from_value(stored.clone()).expect("current reads");
    let re_encoded = serde_json::to_value(&view).expect("re-encodes");

    assert_eq!(re_encoded["winningLine"], stored["winningLine"]);
    assert_eq!(re_encoded["matchId"], stored["matchId"]);
}
