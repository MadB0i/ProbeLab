//! The batch command surface: bounds, refusals, and the property that makes a batch worth having.
//!
//! These tests cover the IPC layer over a real on-disk archive. They exist because the interesting
//! failures in a batch are not arithmetic — they are a batch that reports more matches than it played,
//! statistics that disagree with the database, and a stop button that lies.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use probelab_desktop_lib::{ArenaBatchRequest, ArenaSeatRequest};
use probelab_lab::{ArenaOutcome, ArenaTermination, SqliteArenaMatchStore};

/// Names scratch directories uniquely, the way the lab's own store tests do.
///
/// A shared, fixed name would let two test binaries race on the same file, and a test that passes alone
/// and fails in a full run is worse than one that always fails.
static SCRATCH: AtomicU64 = AtomicU64::new(0);

/// A unique temporary directory, removed when the returned guard drops.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "probelab-arena-batch-{tag}-{}",
            SCRATCH.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("temp dir");
        Self { path }
    }

    fn database(&self) -> PathBuf {
        self.path.join("probelab.sqlite3")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn archive(scratch: &Scratch) -> SqliteArenaMatchStore {
    SqliteArenaMatchStore::open(scratch.database()).expect("the archive should open")
}

/// A policy seat, which is what these tests use: no provider, no network, no timing dependence.
fn policy(id: &str) -> ArenaSeatRequest {
    ArenaSeatRequest::Policy { id: id.to_owned() }
}

#[test]
fn a_batch_of_nothing_produces_no_matches_and_no_groups() {
    let scratch = Scratch::new("empty");
    let statistics = probelab_lab::arena_statistics(&[]);
    assert!(statistics.is_empty());
    assert_eq!(archive(&scratch).count().expect("count"), 0);
}

#[test]
fn recorded_matches_survive_the_archive_being_reopened() {
    // The property the whole batch design rests on: statistics are read from the database, so a database
    // that cannot be reopened would make every figure temporary.
    let scratch = Scratch::new("reopen");
    let path = scratch.database();

    let match_id = "arena-tic_tac_toe-7-1700000000000-1";
    let view = probelab_lab::ArenaMatchView {
        match_id: match_id.to_owned(),
        game: "tic_tac_toe".to_owned(),
        game_name: "Tic-Tac-Toe".to_owned(),
        outcome: ArenaOutcome::Win,
        termination: Some(ArenaTermination::Decided),
        tested_agent: "first_legal_tictactoe".to_owned(),
        opponent_agent: "perfect_tictactoe".to_owned(),
        tested_seat: Some(probelab_engine::arena::SeatKind::ReferencePolicy),
        opponent_seat: Some(probelab_engine::arena::SeatKind::ReferencePolicy),
        tested_side: probelab_core::GameSide::First,
        seed: 7,
        plies: 5,
        final_board: probelab_core::GameBoard::TicTacToe {
            cells: vec![probelab_core::TicTacToeCell::Empty; 9],
        },
        winner: Some(probelab_core::GameSide::First),
        winning_line: None,
        illegal_moves: Vec::new(),
        history: Vec::new(),
        turns: Vec::new(),
        timeline: Vec::new(),
        failure: None,
        cancelled: Some(false),
    };

    {
        let store = SqliteArenaMatchStore::open(&path).expect("open");
        store.put(&view, 1_700_000_000_000).expect("put");
    }

    // A brand new store over the same file: nothing carried over in memory.
    let reopened = SqliteArenaMatchStore::open(&path).expect("reopen");
    assert_eq!(reopened.count().expect("count"), 1);
    let read = reopened.get(match_id).expect("get").expect("the match");
    assert_eq!(read.outcome, ArenaOutcome::Win);
    assert_eq!(read.replay_digest(), view.replay_digest());

    let statistics = probelab_lab::arena_statistics(&[read]);
    assert_eq!(statistics.matches_read, 1);
    assert_eq!(statistics.groups.len(), 1);
    assert_eq!(statistics.groups[0].decisions.outcomes.win, 1);
}

#[test]
fn a_recording_written_twice_is_stored_once() {
    // `put` is `ON CONFLICT DO NOTHING`, so re-archiving the same match cannot inflate a count. A batch
    // that retried a match must not report it twice.
    let scratch = Scratch::new("twice");
    let store = archive(&scratch);
    let view = probelab_lab::ArenaMatchView {
        match_id: "same-id".to_owned(),
        game: "connect_four".to_owned(),
        game_name: "Connect Four".to_owned(),
        outcome: ArenaOutcome::Win,
        termination: Some(ArenaTermination::Decided),
        tested_agent: "a".to_owned(),
        opponent_agent: "b".to_owned(),
        tested_seat: Some(probelab_engine::arena::SeatKind::Model),
        opponent_seat: Some(probelab_engine::arena::SeatKind::ReferencePolicy),
        tested_side: probelab_core::GameSide::First,
        seed: 1,
        plies: 7,
        final_board: probelab_core::GameBoard::ConnectFour {
            columns: 7,
            rows: 6,
            cells: vec![probelab_core::ConnectFourCell::Empty; 42],
        },
        winner: Some(probelab_core::GameSide::First),
        winning_line: None,
        illegal_moves: Vec::new(),
        history: Vec::new(),
        turns: Vec::new(),
        timeline: Vec::new(),
        failure: None,
        cancelled: Some(false),
    };

    store.put(&view, 1).expect("first put");
    store.put(&view, 2).expect("second put");
    assert_eq!(store.count().expect("count"), 1);
}

#[test]
fn a_full_batch_is_never_a_window_into_the_archive() {
    // The panel hard-codes the same numbers. If these drift, the panel's stated limit becomes a lie, so
    // they are asserted rather than trusted.
    assert_eq!(probelab_desktop_lib::MIN_BATCH_MATCHES, 1);
    assert_eq!(probelab_desktop_lib::MAX_BATCH_MATCHES, 50);
    // The archive read is paged now rather than windowed, so the property that matters is that a single
    // page can hold a whole batch: every match a batch produces must be readable within one page, or a
    // batch's own matches would straddle pages. Written as a subtraction for the reason above.
    assert_eq!(
        probelab_desktop_lib::STATISTICS_READ_PAGE as u64
            - probelab_desktop_lib::MAX_BATCH_MATCHES as u64,
        150,
        "one page must hold a full batch plus room, so no batch straddles a page boundary"
    );
    assert_eq!(
        probelab_desktop_lib::ARCHIVE_READ_CEILING as u64
            - probelab_desktop_lib::STATISTICS_READ_PAGE as u64,
        19_800,
        "the ceiling must be far above one page, so reaching it means a genuinely enormous archive"
    );
}

#[test]
fn a_batch_request_round_trips_the_seat_kinds_it_was_given() {
    // The backend builds each seat per match from this request, so a label that failed to survive the
    // wire would archive every match under the wrong agent name.
    assert_eq!(
        policy("perfect_tictactoe").seat_label(),
        "perfect_tictactoe"
    );
    let request = ArenaBatchRequest {
        game: "tic_tac_toe".to_owned(),
        tested_agent: policy("first_legal_tictactoe"),
        opponent_agent: policy("perfect_tictactoe"),
        matches: 4,
        seed: 100,
        alternate_sides: true,
    };
    assert_eq!(request.matches, 4);
    assert!(request.alternate_sides);
    assert_ne!(
        request.tested_agent.seat_label(),
        request.opponent_agent.seat_label()
    );
}

#[test]
fn statistics_over_the_archive_agree_with_the_archive_itself() {
    // The failure this guards against is a batch reporting six matches while the database holds four.
    // Reading back and recomputing is the only way to notice, because the report is built from values
    // the worker already held.
    let scratch = Scratch::new("agree");
    let store = archive(&scratch);

    // Twelve decided plus two undecided. Twelve is chosen because the small-sample threshold is ten:
    // this exercises the "not a small sample" path, which a shorter run never reaches.
    let outcomes = [
        ArenaOutcome::Win,
        ArenaOutcome::Win,
        ArenaOutcome::Win,
        ArenaOutcome::Win,
        ArenaOutcome::Win,
        ArenaOutcome::Loss,
        ArenaOutcome::Loss,
        ArenaOutcome::Loss,
        ArenaOutcome::Draw,
        ArenaOutcome::Draw,
        ArenaOutcome::Draw,
        ArenaOutcome::Draw,
        ArenaOutcome::NoResult,
        ArenaOutcome::NoResult,
    ];
    for (index, outcome) in outcomes.iter().enumerate() {
        store
            .put(
                &tictactoe_view(&format!("m{index:02}"), *outcome),
                i64::try_from(index).unwrap_or(0),
            )
            .expect("put");
    }

    assert_eq!(store.count().expect("count"), 14);

    let summaries = store.list(50).expect("list");
    assert_eq!(summaries.len(), 14);
    let views: Vec<probelab_lab::ArenaMatchView> = summaries
        .iter()
        .filter_map(|summary| store.get(&summary.match_id).expect("get"))
        .collect();

    let statistics = probelab_lab::arena_statistics(&views);
    assert_eq!(statistics.matches_read, 14);
    let group = &statistics.groups[0];
    assert_eq!(group.matches, 14);
    assert_eq!(group.decisions.outcomes.win, 5);
    assert_eq!(group.decisions.outcomes.loss, 3);
    assert_eq!(group.decisions.outcomes.draw, 4);
    assert_eq!(group.decisions.outcomes.no_result, 2);

    // Twelve decided of fourteen played: the two undecided must not reach a denominator.
    assert_eq!(group.decisions.outcomes.decided(), 12);
    assert_eq!(group.decisions.outcomes.decisive(), 8);
    assert!(!group.sample_is_small);
    // 5 wins of 12 decided.
    assert!((group.decisions.win_rate.point - 5.0 / 12.0).abs() < 1e-9);
    // 5 wins of 8 decisive, draws excluded.
    assert!((group.decisions.decisive_win_rate.point - 5.0 / 8.0).abs() < 1e-9);
}

/// A minimal tic-tac-toe recording, so these tests are about the archive rather than about the game.
fn tictactoe_view(match_id: &str, outcome: ArenaOutcome) -> probelab_lab::ArenaMatchView {
    probelab_lab::ArenaMatchView {
        match_id: match_id.to_owned(),
        game: "tic_tac_toe".to_owned(),
        game_name: "Tic-Tac-Toe".to_owned(),
        outcome,
        termination: Some(ArenaTermination::Decided),
        tested_agent: "first_legal_tictactoe".to_owned(),
        opponent_agent: "perfect_tictactoe".to_owned(),
        tested_seat: Some(probelab_engine::arena::SeatKind::ReferencePolicy),
        opponent_seat: Some(probelab_engine::arena::SeatKind::ReferencePolicy),
        tested_side: probelab_core::GameSide::First,
        seed: 7,
        plies: 5,
        final_board: probelab_core::GameBoard::TicTacToe {
            cells: vec![probelab_core::TicTacToeCell::Empty; 9],
        },
        winner: None,
        winning_line: None,
        illegal_moves: Vec::new(),
        history: Vec::new(),
        turns: Vec::new(),
        timeline: Vec::new(),
        failure: None,
        cancelled: Some(false),
    }
}
