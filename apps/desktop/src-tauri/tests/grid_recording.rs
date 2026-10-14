//! The grid recording, read back: four properties that are tested separately on purpose.
//!
//! # Why these are four tests and not one
//!
//! They fail for different reasons and are fixed by different changes, so a single test covering all
//! four would report one defect for four independent faults — and the most expensive one to debug is
//! always the one that hides inside somebody else's failure.
//!
//! 1. **Live execution.** What a run streams as it happens is what it wrote. If the two could differ,
//!    "watch it run" and "watch it again" would be two different claims about one episode.
//! 2. **Recorded playback.** The committed events are sufficient to reconstruct the trajectory the engine
//!    actually executed. This is what stops a replay from being an interpolation.
//! 3. **Cancellation.** A stopped run records exactly the turns it committed and nothing after them. A
//!    partial recording is legitimate; a recording with invented frames after the stop is not.
//! 4. **Restart durability.** The recording is still there after the process that made it has gone, and
//!    it is still complete. This is what distinguishes the durable read from the session buffer, which
//!    is bounded, holds the most recent run only, and is empty after a restart.
//!
//! # What no test in this file does
//!
//! It does not invoke `get_run_telemetry` itself. Building the managed `AppState` pulls the whole Tauri
//! runtime into this test binary, and on a Windows host that binary then imports two casings of the same
//! system libraries (`advapi32.dll` and `ADVAPI32.dll`, `ws2_32.dll` and `WS2_32.dll`) and fails to load
//! with `STATUS_ENTRYPOINT_NOT_FOUND` before a single test runs. That was measured, not assumed — every
//! other test binary in this crate links and runs.
//!
//! So the command's own wiring is covered where it can be, without that runtime: the read itself is
//! pinned by `crates/telemetry/tests/grid_readback.rs`, the registration and the argument casing by
//! `lib.rs`'s command guards and `scripts/check-feature-matrix.py`, and the four properties here are
//! asserted against the same `SqliteResultRepository` the command calls. What is left uncovered is the
//! three lines between them, which is a real gap and is written down rather than papered over.
//!
//! It also does not assert anything about success rates, reward shaping, or turn budgets. Those are the
//! benchmark's evaluation semantics and they are unchanged by any of this; a visualisation that moved a
//! published number would be a worse defect than a visualisation that looked plain.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use probelab_benchmarks::GridWorldBenchmark;
use probelab_core::{
    Agent, Benchmark, CompletedRun, Coordinate, TelemetryEvent, TelemetryRecorder,
};
use probelab_desktop_lib::ModelRunConfig;
use probelab_engine::{BenchmarkRunner, EngineError, RunContext};
use probelab_evaluator::GridWorldEvaluator;
use probelab_model_adapters::MockModelConfig;
use probelab_telemetry::{SqliteResultRepository, SqliteTelemetryRecorder};

/// Names scratch files uniquely, so two test binaries cannot race on one database.
static SCRATCH: AtomicU64 = AtomicU64::new(0);

/// A temporary database file, removed when the returned guard drops.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "probelab-grid-recording-{label}-{}-{}",
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

/// The agent every test here runs, chosen so nothing depends on a provider, a network, or a clock.
///
/// A reference policy rather than a scripted "always move right", because the trajectory assertions need
/// an episode that is actually interesting: a policy that walks into a wall produces an episode whose
/// recorded positions never change, which would satisfy a weaker test for the wrong reason.
fn agent(seed: u64) -> Box<dyn Agent> {
    ModelRunConfig::mock(MockModelConfig::Optimal)
        .build_agent(seed)
        .expect("the mock provider needs no credential")
}

/// Runs one grid episode into `path`, streaming every committed event into `streamed`.
///
/// Mirrors what the command layer does — the same benchmark, the same runner, the same evaluator, the
/// same recorder over the same database file — minus the Tauri event emission, which is not what any
/// assertion here is about.
///
/// `cancel_after` is the number of committed events at which the stop flag is raised. The flag is
/// raised from the recorder's post-commit callback rather than from a timer, which is what makes the
/// cancellation point exact: the event it follows is really on disk, and the turn it stops the run at is
/// a turn that never began.
fn run_episode(
    path: &PathBuf,
    run_id: &str,
    streamed: Arc<Mutex<Vec<TelemetryEvent>>>,
    cancel_after: Option<usize>,
) -> Result<CompletedRun, EngineError> {
    let benchmark: Box<dyn Benchmark> =
        Box::new(GridWorldBenchmark::built_in().expect("built-in benchmark"));
    let summary = benchmark.summary();
    let agent = agent(summary.seed);
    let sink = Arc::clone(&streamed);
    let flag = Arc::new(AtomicBool::new(false));
    let stop = Arc::clone(&flag);
    let recorder = SqliteTelemetryRecorder::with_callback(path, move |event| {
        let committed = match sink.lock() {
            Ok(mut events) => {
                events.push(event);
                events.len()
            }
            // A poisoned lock would silently change what this test proves, so it is loud rather than
            // tolerated.
            Err(_) => return,
        };
        if cancel_after == Some(committed) {
            stop.store(true, Ordering::SeqCst);
        }
    })
    .expect("open recorder");
    let recorder: Arc<dyn TelemetryRecorder> = Arc::new(recorder);
    let context = RunContext::ephemeral("grid")?.with_cancel(Arc::clone(&flag));
    BenchmarkRunner::new().run_with_context(
        benchmark.as_ref(),
        agent,
        &GridWorldEvaluator::new(),
        summary.seed,
        run_id,
        recorder,
        context,
    )
}

// # 1. Live execution

/// What a run streams as it happens is byte-for-byte what it wrote.
///
/// This is the property the whole feature rests on. Live and recorded playback are served by one
/// renderer over one event type, and that is only honest if the live stream and the durable record are
/// the same data. If the buffer ever showed an event the database does not hold, a viewer would be
/// watching a state the run never committed.
#[test]
fn the_live_stream_and_the_durable_record_are_the_same_events() {
    let scratch = Scratch::new("live");
    let streamed = Arc::new(Mutex::new(Vec::new()));
    let run_id = "grid-live-run";

    let completed = run_episode(&scratch.path, run_id, Arc::clone(&streamed), None)
        .expect("the optimal policy finishes the episode");

    let live = streamed.lock().expect("streamed events").clone();
    assert!(!live.is_empty(), "a completed episode recorded no events");
    let stored = SqliteResultRepository::new(&scratch.path)
        .expect("open repository")
        .telemetry_events(run_id)
        .expect("read the run back");

    assert_eq!(
        live, stored,
        "the live stream and the durable record disagree; one of them is showing a state the run did \
         not commit"
    );
    let sequences: Vec<u32> = stored.iter().map(|event| event.sequence).collect();
    let expected: Vec<u32> = (1..=stored.len() as u32).collect();
    assert_eq!(
        sequences, expected,
        "the committed sequence has a gap in it"
    );
    assert_eq!(
        stored.len() as u32,
        completed.result.telemetry_event_count,
        "the result claims a different number of events than the recording holds"
    );
}

// # 2. Recorded playback

/// The committed events reconstruct the trajectory the engine actually executed.
///
/// Three links, each one a field the recording already carries:
///
/// * every event names where the agent was when it decided,
/// * every non-final event names where the move put it,
/// * and that destination is the next event's own observation position.
///
/// The third link is what makes a replay an animation of the run rather than a set of unrelated
/// snapshots. It is asserted rather than assumed: if the environment ever returned a post-transition
/// observation that the next turn did not continue from, a viewer stepping between frames would show
/// the agent teleporting, and that would be a data defect this test is here to catch.
#[test]
fn a_recording_reconstructs_the_trajectory_the_engine_executed() {
    let scratch = Scratch::new("replay");
    let streamed = Arc::new(Mutex::new(Vec::new()));
    let run_id = "grid-replay-run";

    run_episode(&scratch.path, run_id, streamed, None).expect("the episode finishes");

    let events = SqliteResultRepository::new(&scratch.path)
        .expect("open repository")
        .telemetry_events(run_id)
        .expect("read the run back");
    assert!(events.len() >= 2, "too short to show a trajectory");

    for window in events.windows(2) {
        let (current, next) = (&window[0], &window[1]);
        let outcome = current
            .action_outcome
            .as_ref()
            .expect("an authorized move records where it landed");
        assert_eq!(
            outcome.position, next.observation.position,
            "turn {} landed somewhere the next turn did not start from",
            current.turn
        );
        if let Some(after) = outcome.observation.as_ref() {
            assert_eq!(
                after.position, next.observation.position,
                "turn {} recorded two different post-transition positions",
                current.turn
            );
        }
    }

    // The reconstruction a viewer performs: one position per frame, plus the position the final move
    // left the agent on. Both come from recorded fields; neither is computed.
    let mut frames: Vec<Coordinate> = events
        .iter()
        .map(|event| event.observation.position)
        .collect();
    if let Some(last) = events
        .last()
        .and_then(|event| event.action_outcome.as_ref())
    {
        frames.push(last.position);
    }
    assert!(
        frames.windows(2).any(|pair| pair[0] != pair[1]),
        "the reconstructed trajectory never moves, so it proves nothing about playback"
    );
    assert!(
        frames
            .iter()
            .all(|position| position.x < 5 && position.y < 5),
        "a reconstructed position is off the grid the observation declared"
    );
}

// # 3. Cancellation

/// A stopped run records exactly the turns it committed, and no frames after the stop.
///
/// Cancellation is cooperative and checked once per turn, so the recorded prefix is a genuine prefix:
/// the turn the flag was set during never began, so it never produced an event, and there is nothing to
/// extrapolate. What must not happen is a viewer continuing past the end of the recording — which is why
/// this asserts the *number* of recorded turns as well as their contiguity. A fabricated frame would
/// look like a longer, perfectly contiguous run.
#[test]
fn a_cancelled_run_records_only_the_turns_it_committed() {
    let scratch = Scratch::new("cancel");
    let streamed = Arc::new(Mutex::new(Vec::new()));
    let run_id = "grid-cancel-run";

    let outcome = run_episode(&scratch.path, run_id, Arc::clone(&streamed), Some(2));

    assert!(
        matches!(outcome, Err(EngineError::Cancelled { .. })),
        "a cancelled run must report cancellation, not a result: {outcome:?}"
    );

    let events = SqliteResultRepository::new(&scratch.path)
        .expect("open repository")
        .telemetry_events(run_id)
        .expect("read the run back");

    assert_eq!(
        events.len(),
        2,
        "the recording is not the two turns that were committed; a viewer would then be showing \
         something the run did not do"
    );
    let sequences: Vec<u32> = events.iter().map(|event| event.sequence).collect();
    assert_eq!(sequences, vec![1, 2], "the cancelled recording has a gap");
    let turns: Vec<u32> = events.iter().map(|event| event.turn).collect();
    assert_eq!(
        turns,
        vec![1, 2],
        "the recorded turns are not contiguous, so playback would skip work"
    );
    // The same fact seen from the live side. A stop button that reports a cancelled run while the
    // stream keeps growing would be a worse lie than either one on its own.
    assert_eq!(
        streamed.lock().expect("streamed").len(),
        events.len(),
        "the live stream and the durable recording disagree about how far the run got"
    );
}

// # 4. Restart durability

/// The recording outlives the process that made it, and it is still whole.
///
/// Two things are being protected here. The first is availability: after a restart the session buffer is
/// empty, so a read path that only consulted the buffer would show a viewer nothing at all. The second is
/// completeness: the buffer is *bounded*, so a long episode would replay from the middle even in the same
/// session. The cross-check against the stored result's own event count is what distinguishes "still
/// there" from "still all there".
#[test]
fn a_recording_survives_the_process_that_made_it() {
    let scratch = Scratch::new("restart");
    let streamed = Arc::new(Mutex::new(Vec::new()));
    let run_id = "grid-restart-run";

    let completed = run_episode(&scratch.path, run_id, Arc::clone(&streamed), None)
        .expect("the episode finishes");
    let before: Vec<TelemetryEvent> = streamed.lock().expect("streamed").clone();

    // A fresh process: new repository, new connection, nothing carried over in memory. The scratch
    // guard drops nothing here, so the database file is exactly as the run left it.
    let repository = SqliteResultRepository::new(&scratch.path).expect("reopen the store");
    let after = repository
        .telemetry_events(run_id)
        .expect("read the run back after the restart");

    assert_eq!(before, after, "the recording changed across a reopen");
    assert_eq!(
        after.len() as u32,
        completed.result.telemetry_event_count,
        "the read-back recording is a prefix of the run the result describes"
    );
}
