//! The arena match archive: durability, append-only behaviour, and forward compatibility.
//!
//! These are the properties the Arena panel depends on. If a match does not survive a restart, the
//! history list is a lie; if a recording can be edited or deleted, it is not evidence.

use probelab_core::{GameBoard, GameSide, TicTacToeCell};
use probelab_lab::arena_store::{ArenaMatchStoreError, ArenaMatchSummary, SqliteArenaMatchStore};
use probelab_lab::{
    ArenaMatchView, ArenaOutcome, EventSource, IllegalMoveView, LabEvent, PlyView, Severity,
};

fn a_match(match_id: &str, outcome: ArenaOutcome) -> ArenaMatchView {
    ArenaMatchView {
        match_id: match_id.to_owned(),
        game: "tic_tac_toe".to_owned(),
        game_name: "Tic-Tac-Toe".to_owned(),
        outcome,
        tested_seat: probelab_engine::arena::SeatKind::ReferencePolicy,
        opponent_seat: probelab_engine::arena::SeatKind::ReferencePolicy,
        tested_agent: "first_legal_tictactoe".to_owned(),
        opponent_agent: "perfect_tictactoe".to_owned(),
        tested_side: GameSide::First,
        seed: 7,
        plies: 5,
        final_board: GameBoard::TicTacToe {
            cells: vec![TicTacToeCell::First; 9],
        },
        winner: Some(GameSide::First),
        winning_line: Some(vec![[0, 1], [1, 1], [2, 1]]),
        illegal_moves: Vec::new(),
        history: vec![PlyView {
            ply: 0,
            side: GameSide::First,
            board: GameBoard::TicTacToe {
                cells: vec![TicTacToeCell::First; 9],
            },
            played: Some(probelab_core::GameMove::TicTacToe(0)),
        }],
        turns: Vec::new(),
        timeline: Vec::new(),
        failure: None,
        cancelled: false,
    }
}

fn a_failed_match(match_id: &str) -> ArenaMatchView {
    ArenaMatchView {
        match_id: match_id.to_owned(),
        game: "tic_tac_toe".to_owned(),
        game_name: "Tic-Tac-Toe".to_owned(),
        outcome: ArenaOutcome::NoResult,
        tested_seat: probelab_engine::arena::SeatKind::ReferencePolicy,
        opponent_seat: probelab_engine::arena::SeatKind::ReferencePolicy,
        tested_agent: "first_legal_tictactoe".to_owned(),
        opponent_agent: "perfect_tictactoe".to_owned(),
        tested_side: GameSide::Second,
        seed: 7,
        plies: 2,
        final_board: GameBoard::TicTacToe {
            cells: vec![TicTacToeCell::Empty; 9],
        },
        winner: None,
        winning_line: None,
        illegal_moves: vec![IllegalMoveView {
            ply: 2,
            attempted: None,
            reason: "cell already taken".to_owned(),
        }],
        history: Vec::new(),
        turns: Vec::new(),
        timeline: vec![LabEvent::new(
            1,
            "arena.match_failed",
            EventSource::Telemetry,
            Severity::Error,
            "the engine refused the move",
        )],
        failure: Some("illegal move".to_owned()),
        cancelled: false,
    }
}

fn a_summary() -> ArenaMatchSummary {
    ArenaMatchSummary {
        match_id: "arena-ttt-7-1700000000000-1".to_owned(),
        game: "tic_tac_toe".to_owned(),
        outcome: ArenaOutcome::Win,
        tested_agent: "first_legal_tictactoe".to_owned(),
        opponent_agent: "perfect_tictactoe".to_owned(),
        tested_side: GameSide::First,
        seed: 7,
        plies: 5,
        recorded_unix_ms: 1_700_000_000_000,
        digest: "c71fe9311f42bdde".to_owned(),
    }
}

fn a_store(tag: &str) -> (SqliteArenaMatchStore, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "probelab-arena-store-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("arena.db");
    (
        SqliteArenaMatchStore::open(&path).expect("store opens"),
        path,
    )
}

#[test]
fn a_played_match_survives_reopening_the_store() {
    let (store, path) = a_store("restart");
    let match_view = a_match("arena-ttt-7-1", ArenaOutcome::Win);
    store.put(&match_view, 1_700_000_000_000).expect("recorded");

    // A second store over the same file is what a restart looks like.
    let reopened = SqliteArenaMatchStore::open(&path).expect("reopens");
    let loaded = reopened
        .get("arena-ttt-7-1")
        .expect("reads")
        .expect("the match is still there after a restart");
    assert_eq!(loaded.match_id, "arena-ttt-7-1");
    assert_eq!(loaded.replay_digest(), match_view.replay_digest());
    assert_eq!(reopened.count().expect("counts"), 1);
}

#[test]
fn history_is_newest_first_and_reports_real_metadata() {
    let (store, _path) = a_store("order");
    store
        .put(&a_match("oldest", ArenaOutcome::Draw), 1_000)
        .expect("first");
    store.put(&a_failed_match("middle"), 2_000).expect("second");
    store
        .put(&a_match("newest", ArenaOutcome::Loss), 3_000)
        .expect("third");

    let summaries = store.list(50).expect("lists");
    let ids: Vec<&str> = summaries.iter().map(|s| s.match_id.as_str()).collect();
    assert_eq!(ids, ["newest", "middle", "oldest"], "newest first");

    let newest = &summaries[0];
    assert_eq!(newest.outcome, ArenaOutcome::Loss);
    assert_eq!(newest.tested_side, GameSide::First);
    assert_eq!(newest.plies, 5);
    assert_eq!(newest.seed, 7);
    assert_eq!(newest.recorded_unix_ms, 3_000);
    assert!(
        !newest.digest.is_empty(),
        "a summary carries the recording digest"
    );

    // A failed match is stored as what it was, not as a loss.
    let failed = &summaries[1];
    assert_eq!(failed.outcome, ArenaOutcome::NoResult);
}

#[test]
fn a_limit_bounds_the_history_without_dropping_older_matches() {
    let (store, _path) = a_store("limit");
    for index in 0..5 {
        store
            .put(
                &a_match(&format!("m-{index}"), ArenaOutcome::Win),
                i64::from(index),
            )
            .expect("recorded");
    }
    let summaries = store.list(2).expect("lists");
    assert_eq!(summaries.len(), 2);
    assert_eq!(summaries[0].match_id, "m-4");
    assert_eq!(
        store.count().expect("counts"),
        5,
        "a limit is not a deletion"
    );
}

#[test]
fn a_recording_cannot_be_edited_or_deleted_through_the_store() {
    let (store, path) = a_store("append-only");
    let match_view = a_match("arena-ttt-7-1", ArenaOutcome::Win);
    store.put(&match_view, 1_000).expect("recorded");

    // Writing the same identifier again is refused rather than overwriting the evidence.
    let mut edited = match_view.clone();
    edited.outcome = ArenaOutcome::Loss;
    store
        .put(&edited, 2_000)
        .expect("the write is accepted or ignored");
    let after = store.get("arena-ttt-7-1").expect("reads").expect("present");
    assert_eq!(
        after.outcome,
        ArenaOutcome::Win,
        "a second write must not overwrite the recording of what happened"
    );

    // Neither does a raw DELETE or UPDATE escape the store's intent.
    let connection = rusqlite::Connection::open(&path).expect("raw connection");
    assert!(
        connection.execute("DELETE FROM arena_matches", []).is_err(),
        "the archive must refuse a delete"
    );
    assert!(
        connection
            .execute("UPDATE arena_matches SET outcome = 'loss'", [])
            .is_err(),
        "the archive must refuse an edit"
    );
}

#[test]
fn a_failed_match_is_archived_with_its_failure_rather_than_dropped() {
    let (store, _path) = a_store("failure");
    let failed = a_failed_match("arena-ttt-7-fail");
    store.put(&failed, 1_000).expect("recorded");
    let loaded = store
        .get("arena-ttt-7-fail")
        .expect("reads")
        .expect("present");
    assert_eq!(loaded.outcome, ArenaOutcome::NoResult);
    assert_eq!(loaded.failure.as_deref(), Some("illegal move"));
    assert_eq!(loaded.winning_line, None, "no winner, no line");
    assert_eq!(loaded.illegal_moves.len(), 1);
}

#[test]
fn an_unknown_identifier_is_absent_rather_than_an_error() {
    let (store, _path) = a_store("missing");
    assert_eq!(store.get("never-played").expect("reads"), None);
    assert_eq!(store.count().expect("counts"), 0);
}

#[test]
fn opening_an_existing_database_adds_nothing_and_keeps_every_row() {
    let (store, path) = a_store("migrate");
    store
        .put(&a_match("kept", ArenaOutcome::Win), 1_000)
        .expect("recorded");

    // Opening repeatedly is what happens across launches; it must be idempotent.
    for _ in 0..3 {
        let reopened = SqliteArenaMatchStore::open(&path).expect("reopens");
        assert_eq!(reopened.count().expect("counts"), 1);
    }
    assert_eq!(
        store.get("kept").expect("reads").expect("present").match_id,
        "kept"
    );
}

#[test]
fn a_store_over_an_empty_file_creates_its_schema() {
    // A fresh install: the file may not exist at all.
    let (store, path) = a_store("fresh");
    assert!(path.exists(), "opening created the database file");
    assert_eq!(store.count().expect("counts"), 0);
    let connection = rusqlite::Connection::open(&path).expect("raw connection");
    let tables: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'arena_matches'",
            [],
            |row| row.get(0),
        )
        .expect("queries");
    assert_eq!(tables, 1);
}

#[test]
fn a_summary_serializes_in_the_camel_case_the_frontend_reads() {
    // The frontend binds `matchId` / `testedAgent`; a snake_case row would make every field undefined
    // and the history list would render blank rows that cannot be opened.
    let encoded = serde_json::to_value(a_summary()).expect("summary serializes");
    assert_eq!(encoded["matchId"], "arena-ttt-7-1700000000000-1");
    assert_eq!(encoded["testedAgent"], "first_legal_tictactoe");
    assert_eq!(encoded["opponentAgent"], "perfect_tictactoe");
    assert_eq!(encoded["testedSide"], "first");
    assert_eq!(encoded["recordedUnixMs"], 1_700_000_000_000i64);
    assert_eq!(encoded["outcome"], "win");
    assert!(
        encoded.get("match_id").is_none(),
        "snake_case keys must not also be present"
    );
}

#[test]
fn a_full_recording_round_trips_through_the_store_with_its_digest() {
    let (store, _path) = a_store("digest");
    let original = a_match("arena-ttt-7-1", ArenaOutcome::Win);
    store.put(&original, 1_000).expect("recorded");
    let loaded = store.get("arena-ttt-7-1").expect("reads").expect("present");
    assert_eq!(
        loaded.replay_digest(),
        original.replay_digest(),
        "a stored recording must be digest-identical, or it is not the recording"
    );
    assert_eq!(loaded.winning_line, Some(vec![[0, 1], [1, 1], [2, 1]]));
    assert_eq!(loaded.history.len(), 1);
    assert!(loaded.history[0].played.is_some());
}

#[test]
fn a_corrupt_recording_is_reported_rather_than_silently_dropped() {
    use rusqlite::params;
    let (store, path) = a_store("corrupt");
    store
        .put(&a_match("good", ArenaOutcome::Win), 1_000)
        .expect("recorded");
    // The append-only triggers block UPDATE, so corruption is injected into a fresh database the way
    // a partially-written file from a crashed process would present itself.
    let connection = rusqlite::Connection::open(&path).expect("raw connection");
    connection
        .execute_batch("DROP TRIGGER IF EXISTS arena_matches_reject_update")
        .expect("drops the guard for this test only");
    connection
        .execute(
            // Valid JSON that is not a recording: the shape a row written by a build with a
            // different view would present as. The `json_valid` constraint passes, so the failure has
            // to surface at decode time rather than at write time.
            "UPDATE arena_matches SET match_json = '{\"match_id\":\"good\"}' WHERE match_id = ?1",
            params!["good"],
        )
        .expect("corrupts the row");
    drop(connection);

    match store.get("good") {
        Err(ArenaMatchStoreError::Corrupt { match_id, .. }) => {
            assert_eq!(match_id, "good", "the failure names the recording");
        }
        other => panic!("expected a corrupt-recording error, got {other:?}"),
    }
}
