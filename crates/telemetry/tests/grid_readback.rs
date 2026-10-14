//! Reading a run's telemetry back out of the append-only store.
//!
//! # Why this file exists
//!
//! A grid episode is recorded in full: every turn carries the observation the agent was handed *and*
//! the outcome of the transition it chose. That is enough to reconstruct the episode afterwards — but
//! only if the committed events can be read back, and until now they could not: the only reader was an
//! in-memory buffer scoped to the current session.
//!
//! So this file is about the read, not about the recording. The recording's own properties are covered
//! by the engine tests, and what matters here is narrower:
//!
//! * a run's events come back **in the order they were executed**,
//! * they come back **equal to what was recorded**, with nothing dropped by serialization,
//! * a run with no recording reads as **empty rather than as an error**,
//! * and reading one **does not weaken the append-only guarantee** the table is built on.
//!
//! The last one is easy to get wrong. A read that opened a write transaction, or that took a path
//! capable of one, would make "the evidence cannot be edited" a property of the code that happened to
//! be on screen rather than of the store.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use probelab_core::{
    Action, ActionOutcome, ActionResultKind, Coordinate, GridSize, Observation, ResourceInfo,
    TileKind, VisibleTile,
};
use probelab_telemetry::{
    NewTelemetryEvent, SqliteResultRepository, SqliteTelemetryRecorder, TelemetryObserver,
};

/// Names scratch files uniquely, so two test binaries cannot race on one database.
static SCRATCH: AtomicU64 = AtomicU64::new(0);

/// A temporary database file, removed when the returned guard drops.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "probelab-readback-{label}-{}-{}",
            std::process::id(),
            SCRATCH.fetch_add(1, Ordering::Relaxed)
        ));
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
        Self { path }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.path.display()));
        }
    }
}

/// A five-by-five observation the agent could actually have been handed.
///
/// Shaped like a real grid turn rather than a minimal one: neighbours are present, the distance
/// reading is a gradient, and the action vocabulary is the full set. A read-back test over a
/// stripped-down event would pass while the fields the visualiser actually depends on were being lost.
fn observation(turn: u32, x: u8, y: u8) -> Observation {
    Observation {
        turn,
        position: Coordinate { x, y },
        grid_size: GridSize {
            width: 5,
            height: 5,
        },
        current_tile: TileKind::Safe,
        visible_neighbors: vec![
            VisibleTile {
                coordinate: Coordinate { x, y: y + 1 },
                tile: TileKind::Danger,
            },
            VisibleTile {
                coordinate: Coordinate { x: x + 1, y },
                tile: TileKind::Safe,
            },
        ],
        distance_to_goal: 6 - turn.min(6),
        max_turns: 10,
        remaining_turns: 10 - (turn - 1),
        available_actions: vec![
            Action::Up,
            Action::Down,
            Action::Left,
            Action::Right,
            Action::Wait,
        ],
    }
}

/// One authorized move: an observation, the transition it produced, and the observation after it.
///
/// The nested post-transition observation is the load-bearing part. It is what lets a reader place the
/// agent where the agent *ended up* rather than only where it *was*, which is the difference between a
/// recording that can be animated and one that can merely be listed.
fn move_event(turn: u32, from: (u8, u8), to: (u8, u8)) -> NewTelemetryEvent {
    let (fx, fy) = from;
    let (tx, ty) = to;
    let mut after = observation(turn + 1, tx, ty);
    after.max_turns = 10;
    after.remaining_turns = 10 - turn;
    NewTelemetryEvent {
        turn,
        observation: observation(turn, fx, fy),
        selected_action: Some(Action::Right),
        game: None,
        requested_capability: None,
        action_outcome: Some(ActionOutcome {
            action: Some(Action::Right),
            kind: ActionResultKind::Moved,
            position: Coordinate { x: tx, y: ty },
            reward: 0,
            observation: Some(after),
        }),
        reward: 0,
        invalid_action: false,
        timeout: false,
        resource_info: Some(ResourceInfo {
            turns_used: turn,
            rejected_actions: 0,
            timed_out: false,
        }),
        integrity_event: None,
        model_call: None,
        model_failure: None,
        broker_denial: None,
        process_lifecycle: None,
    }
}

/// Records a straight three-turn walk and returns the events as they were committed.
fn record_walk(scratch: &Scratch, run_id: &str) -> Vec<probelab_core::TelemetryEvent> {
    let recorder = SqliteTelemetryRecorder::new(&scratch.path).expect("open recorder");
    let mut observer = TelemetryObserver::from_recorder(recorder, run_id, "episode-1");
    let mut committed = Vec::new();
    for (index, (from, to)) in [((0, 0), (1, 0)), ((1, 0), (2, 0)), ((2, 0), (3, 0))]
        .into_iter()
        .enumerate()
    {
        committed.push(
            observer
                .record(move_event(index as u32 + 1, from, to))
                .expect("record the move"),
        );
    }
    committed
}

fn repository(scratch: &Scratch) -> SqliteResultRepository {
    SqliteResultRepository::new(&scratch.path).expect("open the result repository")
}

/// A run's events come back in the order the run executed them.
///
/// The order is the whole contract. A viewer that read the table in whatever order the database
/// happened to return rows would draw the episode backwards some of the time, which looks like a
/// rendering bug and is actually a data bug.
#[test]
fn a_run_reads_back_in_sequence_order() {
    let scratch = Scratch::new("order");
    let committed = record_walk(&scratch, "run-order");

    let read = repository(&scratch)
        .telemetry_events("run-order")
        .expect("read the run");

    let sequences: Vec<u32> = read.iter().map(|event| event.sequence).collect();
    assert_eq!(sequences, vec![1, 2, 3]);
    assert_eq!(
        read, committed,
        "the read-back run is not the run that was recorded"
    );
}

/// What comes back is the event, not a summary of it.
///
/// This is the assertion that a visualiser can rely on: every field the engine committed survives the
/// round trip, including the neighbour tiles and the post-transition observation nested inside the
/// outcome. A read that kept the sequence numbers and dropped the observations would still be ordered
/// correctly, still be the right length, and would still be useless.
#[test]
fn a_read_back_event_carries_the_observation_the_agent_was_given() {
    let scratch = Scratch::new("fields");
    record_walk(&scratch, "run-fields");

    let read = repository(&scratch)
        .telemetry_events("run-fields")
        .expect("read the run");

    let first = &read[0];
    assert_eq!(first.observation.turn, 1);
    assert_eq!(first.observation.position, Coordinate { x: 0, y: 0 });
    assert_eq!(first.observation.grid_size.width, 5);
    assert_eq!(first.observation.visible_neighbors.len(), 2);
    assert_eq!(
        first.observation.visible_neighbors[0].tile,
        TileKind::Danger,
        "a hazard the agent saw must survive the round trip, or the map drawn from it is a guess"
    );
    assert_eq!(first.selected_action, Some(Action::Right));
    let outcome = first
        .action_outcome
        .as_ref()
        .expect("an authorized move records its outcome");
    assert_eq!(outcome.kind, ActionResultKind::Moved);
    assert_eq!(outcome.position, Coordinate { x: 1, y: 0 });
    let after = outcome
        .observation
        .as_ref()
        .expect("a continuing episode records where the agent ended up");
    assert_eq!(after.position, Coordinate { x: 1, y: 0 });
    assert_eq!(after.turn, 2);
}

/// A run that recorded nothing reads as nothing.
///
/// Not as an error. "No events were committed under this identifier" is the true answer for a run that
/// never started and for a recording that is missing, and the two are indistinguishable here on
/// purpose: the store cannot tell them apart, so a reader must not pretend to.
#[test]
fn an_unrecorded_run_reads_back_empty() {
    let scratch = Scratch::new("empty");
    record_walk(&scratch, "run-present");

    let read = repository(&scratch)
        .telemetry_events("run-absent")
        .expect("an unknown run is not a failure");

    assert!(read.is_empty());
}

/// One run's events are never another run's.
///
/// Both writers share a table, so this is the property that keeps a replay from being a plausible-looking
/// episode that never happened: a reader has to be scoped to the run it was asked about.
#[test]
fn reads_are_scoped_to_the_run_that_was_asked_for() {
    let scratch = Scratch::new("scoped");
    record_walk(&scratch, "run-a");
    record_walk(&scratch, "run-b");

    let store = repository(&scratch);
    let a = store.telemetry_events("run-a").expect("read run a");
    let b = store.telemetry_events("run-b").expect("read run b");

    assert_eq!(a.len(), 3);
    assert_eq!(b.len(), 3);
    assert!(a.iter().all(|event| event.run_id == "run-a"));
    assert!(b.iter().all(|event| event.run_id == "run-b"));
}

/// Reading a recording does not open a way to edit one.
///
/// The table refuses `UPDATE` and `DELETE` through triggers. This asserts they still refuse *after* the
/// read path has run — that the guarantee belongs to the store and not to which code happens to be
/// executing.
#[test]
fn a_read_leaves_the_append_only_guarantee_intact() {
    let scratch = Scratch::new("append-only");
    record_walk(&scratch, "run-immutable");

    let store = repository(&scratch);
    let read = store
        .telemetry_events("run-immutable")
        .expect("read the run");
    assert_eq!(read.len(), 3);

    for statement in [
        "UPDATE telemetry_events SET event_json = '{}'",
        "DELETE FROM telemetry_events",
    ] {
        let outcome = rusqlite::Connection::open(&scratch.path)
            .expect("open the database directly")
            .execute(statement, []);
        assert!(
            outcome.is_err(),
            "`{statement}` succeeded after a read; the append-only guarantee is not a property of the \
             store any more"
        );
    }
}
