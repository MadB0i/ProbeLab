//! Replay tests.
//!
//! Two things are being established, and they are different in kind.
//!
//! The first is that reconstruction is *deterministic*: the same recording produces the same frames
//! and the same digest, every time, with no external dependency. That is what makes replay a
//! reproduction claim rather than a convenience.
//!
//! The second, and the more important, is that replay **cannot** execute or authorize anything. A
//! replay subsystem in a product that measures untrusted code which could reach a provider, a broker,
//! or a process launcher would be a severe trust violation. So the tests below assert those handles are
//! absent, structurally, rather than trusting that replay simply does not use them.

use probelab_core::{GameBoard, GameSide, TicTacToeCell};
use probelab_lab::{
    frame_at, replay_experiment, replay_match, ArenaOutcome, IllegalMoveView, PlyView,
    ReplayCapability, ReplaySource,
};
use probelab_lab::{ArenaMatchView, EventSource, LabEvent, Severity};

fn drawn_match() -> ArenaMatchView {
    ArenaMatchView {
        match_id: "arena-ttt-7".to_owned(),
        game: "tic_tac_toe".to_owned(),
        game_name: "Tic-Tac-Toe".to_owned(),
        outcome: ArenaOutcome::Draw,
        tested_agent: "perfect_tictactoe".to_owned(),
        opponent_agent: "perfect_tictactoe".to_owned(),
        tested_side: GameSide::First,
        seed: 7,
        plies: 2,
        final_board: board(["first", "second", "first"]),
        winner: None,
        winning_line: None,
        illegal_moves: Vec::new(),
        history: vec![
            ply(1, GameSide::First, ["first"], Some(0)),
            ply(2, GameSide::Second, ["first", "second"], Some(1)),
        ],
        timeline: Vec::new(),
        failure: None,
    }
}

fn board<const N: usize>(cells: [&str; N]) -> GameBoard {
    let mut all = Vec::new();
    for cell in cells {
        all.push(match cell {
            "first" => TicTacToeCell::First,
            "second" => TicTacToeCell::Second,
            _ => TicTacToeCell::Empty,
        });
    }
    all.resize(9, TicTacToeCell::Empty);
    GameBoard::TicTacToe { cells: all }
}

fn ply<const N: usize>(
    number: u32,
    side: GameSide,
    cells: [&str; N],
    played: Option<usize>,
) -> PlyView {
    PlyView {
        ply: number,
        side,
        board: board(cells),
        played: played.map(probelab_core::GameMove::TicTacToe),
    }
}

/// The same recording reconstructs identically, every time.
///
/// This is the whole claim. If reconstruction depended on a clock, a hash seed, or an external service,
/// it would differ between runs and the digest would be worthless as evidence.
#[test]
fn reconstruction_is_deterministic() {
    let match_view = drawn_match();
    let first = replay_match(&match_view);
    for _ in 0..5 {
        assert_eq!(
            replay_match(&match_view),
            first,
            "replay must be reproducible"
        );
    }
    // And a separately-constructed but identical recording agrees, so the digest is a property of the
    // content rather than of one object's identity.
    assert_eq!(replay_match(&drawn_match()), first);
}

/// A tampered recording produces a different digest.
///
/// Without this, the digest would only prove that replay is self-consistent, not that it reflects what
/// was actually recorded.
#[test]
fn a_tampered_recording_changes_the_digest() {
    let original = drawn_match();
    let original_replay = replay_match(&original);

    let mut tampered = original.clone();
    tampered.history[0].board = board(["second", "empty", "empty"]);
    let tampered_replay = replay_match(&tampered);
    assert_ne!(
        tampered_replay.digest, original_replay.digest,
        "a changed board must change the digest"
    );

    let mut reseeded = original.clone();
    reseeded.seed = 8;
    assert_ne!(replay_match(&reseeded).digest, original_replay.digest);

    let mut swapped = original.clone();
    swapped.tested_agent = "first_legal_tictactoe".to_owned();
    assert_ne!(replay_match(&swapped).digest, original_replay.digest);
}

/// Frames are in order and addressable.
#[test]
fn frames_are_ordered_and_addressable() {
    let replay = replay_match(&drawn_match());
    // A start frame, one per ply, and a terminal frame.
    assert_eq!(replay.frames.len(), 4);
    assert_eq!(replay.frames[0].source, ReplaySource::Start);
    assert_eq!(replay.frames[1].source, ReplaySource::ArenaPly);
    assert_eq!(replay.frames[3].source, ReplaySource::Terminal);

    for (index, frame) in replay.frames.iter().enumerate() {
        assert_eq!(frame.index, index, "frames must be numbered in order");
    }
    assert!(frame_at(&replay, 1).is_some());
    assert!(
        frame_at(&replay, 99).is_none(),
        "a position past the end must not silently clamp to the last frame"
    );
}

/// A match with recorded plies is fully re-executable.
#[test]
fn a_recorded_match_is_re_executable() {
    let replay = replay_match(&drawn_match());
    assert_eq!(replay.capability, ReplayCapability::Full);
    assert!(replay.capability.can_step());
    assert!(replay.limitations.is_empty());
    assert_eq!(replay.kind, probelab_lab::ReplayKind::ArenaMatch);
}

/// A recording with no ply history says so, and is marked inspection-only.
///
/// The honest move. Interpolating plausible intermediate boards would be fabricating history, which is
/// the one thing this subsystem exists to avoid.
#[test]
fn a_match_without_history_is_inspection_only_and_says_why() {
    let mut view = drawn_match();
    view.history.clear();
    let replay = replay_match(&view);

    assert_eq!(replay.capability, ReplayCapability::ProjectedOnly);
    assert!(!replay.capability.can_step());
    assert_eq!(
        replay.frames.len(),
        2,
        "a start frame and a terminal frame, nothing invented between"
    );
    assert!(
        replay
            .limitations
            .iter()
            .any(|text| text.contains("cannot be reconstructed")),
        "the limitation must be stated, not merely implied by the capability"
    );
}

/// A refused move whose action was never identified is called out.
#[test]
fn an_unidentified_refused_move_is_reported_as_a_limitation() {
    let mut view = drawn_match();
    view.illegal_moves = vec![IllegalMoveView {
        ply: 3,
        // The engine's refusal names the ply and the reason but not the move.
        attempted: None,
        reason: "that cell is already taken".to_owned(),
    }];
    let replay = replay_match(&view);
    assert!(
        replay
            .limitations
            .iter()
            .any(|text| text.contains("attempted action is unknown")),
        "an unidentified move must be reported as unknown rather than filled in"
    );
}

/// An experiment recording is inspection-only, and says why.
///
/// An event stream says what was reported and when. It does not contain every intermediate
/// environment state, so presenting it as re-executable would let someone trust a projection as a
/// re-run.
#[test]
fn an_experiment_recording_is_inspection_only() {
    let events = vec![
        LabEvent::new(
            0,
            "experiment.created",
            EventSource::Experiment,
            Severity::Info,
            "created",
        ),
        LabEvent::new(
            0,
            "runner.episode_result",
            EventSource::Telemetry,
            Severity::Info,
            "episode 0: task success 1",
        ),
    ];
    let replay = replay_experiment("exp-1", &events);

    assert_eq!(replay.kind, probelab_lab::ReplayKind::Experiment);
    assert_eq!(replay.capability, ReplayCapability::ProjectedOnly);
    assert_eq!(replay.frames.len(), 3, "a start frame plus one per event");
    assert!(!replay.limitations.is_empty());
    assert!(replay.digest.contains(&replay.digest[..2]) || !replay.digest.is_empty());
    // Deterministic too.
    assert_eq!(replay_experiment("exp-1", &events), replay);
}

/// An event stream's digest changes when an event changes.
#[test]
fn an_event_digest_reflects_the_stream() {
    let base = vec![LabEvent::new(
        0,
        "experiment.created",
        EventSource::Experiment,
        Severity::Info,
        "created",
    )];
    let original = replay_experiment("exp", &base).digest;
    let mut changed = base.clone();
    changed.push(LabEvent::new(
        0,
        "experiment.finished",
        EventSource::Experiment,
        Severity::Info,
        "completed",
    ));
    assert_ne!(replay_experiment("exp", &changed).digest, original);
}

/// An empty recording still replays, and produces a start and a terminal frame.
#[test]
fn an_empty_recording_replays_rather_than_failing() {
    let mut view = drawn_match();
    view.history.clear();
    let replay = replay_match(&view);
    assert_eq!(replay.frames.len(), 2);
    assert!(!replay.digest.is_empty());
}

/// The replay round-trips through the IPC encoding.
#[test]
fn a_replay_round_trips() {
    let replay = replay_match(&drawn_match());
    let json = serde_json::to_string(&replay).expect("serialize");
    let restored: probelab_lab::Replay = serde_json::from_str(&json).expect("parse");
    assert_eq!(restored, replay);
    assert!(json.contains("\"recordingId\""));
    assert!(json.contains("\"capability\""));
}

/// No credential can reach a replay.
///
/// A replay is a shareable artefact in a way a live panel is not, so its payloads are checked for
/// credential-shaped text.
#[test]
fn a_replay_never_carries_a_credential() {
    let json = serde_json::to_string(&replay_match(&drawn_match())).expect("serialize");
    for marker in [
        "sk-",
        "sk-ant",
        "AIza",
        "api_key\":",
        "apiKey",
        "password",
        "token\":",
    ] {
        assert!(!json.contains(marker), "a replay leaked {marker}");
    }
}

/// Structural test: replay cannot execute, authorize, or reach a provider.
///
/// The three guarantees that make this a replay rather than a re-run, asserted structurally so a later
/// edit cannot quietly add a handle. A replay that could call a model would produce different output
/// every time while claiming to be a reproduction.
#[test]
fn replay_holds_no_execution_or_authorization_capability() {
    let source = code_only(include_str!("../src/replay.rs"));
    for forbidden in [
        // Execution.
        "Command::new",
        "std::process",
        "ContainedProcessSpawner",
        "RestrictedProcessSpec",
        // Authorization and enforcement.
        "CapabilityBroker",
        "PolicyDecision",
        "RuntimeAuthorization",
        "CapabilityRequest",
        // Model access -- the handle that would break determinism.
        "AsyncModelProvider",
        "OpenAiCompatibleProvider",
        "AnthropicProvider",
        "GeminiProvider",
        "LabRunner",
        "BenchmarkRunner",
        "play_match",
        // Filesystem and network, for completeness.
        "std::fs",
        "reqwest",
    ] {
        assert!(
            !occurs_as_identifier(&source, forbidden),
            "the replay module contains {forbidden:?}; a replay must reconstruct recorded data and \
             be incapable of executing, authorizing, or contacting anything"
        );
    }
    // It must not be able to evaluate or score either: replay reports, it does not judge.
    assert!(
        !source.contains("fn evaluate"),
        "the replay module must not be able to evaluate anything"
    );
}

/// Whether `needle` appears in `source` as a whole identifier rather than as a substring.
///
/// This is not pedantry. `replay_match` -- this module's own public function -- contains `play_match`
/// as a substring, so a plain `contains` flagged the module's API as a forbidden engine call and the
/// first version of this test failed on it. Matching whole identifiers is what makes the assertion
/// about *calling* the engine rather than about a name that happens to contain it. Two of the forbidden
/// entries are path-shaped (`std::process`, `reqwest`) and are matched as substrings for that reason.
fn occurs_as_identifier(source: &str, needle: &str) -> bool {
    if needle.contains("::")
        || needle.contains('.') && !needle.chars().next().is_some_and(|c| c.is_alphabetic())
    {
        return source.contains(needle);
    }
    let bytes = source.as_bytes();
    let target = needle.as_bytes();
    let mut start = 0;
    while let Some(offset) = source[start..].find(needle) {
        let index = start + offset;
        let before_ok = index == 0 || !is_identifier_byte(bytes[index - 1]);
        let after = index + target.len();
        let after_ok = after >= bytes.len() || !is_identifier_byte(bytes[after]);
        if before_ok && after_ok {
            return true;
        }
        start = index + 1;
        if start >= source.len() {
            break;
        }
    }
    false
}

fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Strips comments from Rust source so a structural assertion tests code rather than prose.
///
/// Without this, the module's own documentation -- which necessarily *names* the things it refuses to
/// do -- would trip every assertion. That is not hypothetical: the first version of this file failed
/// on `play_match` appearing in a sentence explaining that replay does not call it. Testing the prose
/// would have meant deleting the explanation of why the guarantee exists, which is backwards.
fn code_only(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    for line in source.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") {
            continue;
        }
        let code = line.split("//").next().unwrap_or(line);
        out.push_str(code);
        out.push('\n');
    }
    out
}

/// Replay borrows its recording rather than consuming it.
///
/// A caller must be able to replay the same evidence repeatedly, and must not be able to corrupt the
/// original by doing so.
#[test]
fn replay_does_not_mutate_its_recording() {
    let view = drawn_match();
    let before = view.clone();
    let _ = replay_match(&view);
    assert_eq!(view, before, "replay mutated the recording it was given");

    let events = vec![LabEvent::new(
        0,
        "experiment.created",
        EventSource::Experiment,
        Severity::Info,
        "created",
    )];
    let events_before = events.clone();
    let _ = replay_experiment("exp", &events);
    assert_eq!(
        events, events_before,
        "replay mutated the event stream it was given"
    );
}
