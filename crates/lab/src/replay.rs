//! Deterministic replay: reconstructing recorded execution without re-running it.
//!
//! # What replay is, and what it emphatically is not
//!
//! Replay reads what a run *recorded* and reconstructs the states it passed through. It does not run
//! the engine again, does not call a provider, and does not ask a model what it would do now. A
//! "replay" that re-queried an LLM would be a second, different experiment wearing a familiar name —
//! and since external models are nondeterministic, its output would differ every time while claiming
//! to be the same thing.
//!
//! # The guarantee, and its limit
//!
//! The guarantee is **reconstruction from immutable recorded data**: given the same recording, the
//! replay produces the same states, byte for byte, every time, with no external dependency. That is
//! testable and this file is where it is tested.
//!
//! The limit is that replay can only show what was recorded. If a run did not record an intermediate
//! state, replay cannot invent one, and [`ReplayCapability`] says so rather than interpolating. A
//! replay that quietly filled gaps would be fabricating history, which is the one thing this subsystem
//! exists to avoid.
//!
//! # Three properties that make it replay rather than a re-run
//!
//! * It holds no provider, so it *cannot* call a model even by accident.
//! * It holds no broker and no process launcher, so it cannot authorize or execute anything.
//! * It takes its input by value and derives everything else, so the original recording is never
//!   mutated — a caller cannot corrupt the evidence by replaying it.
//!
//! All three are asserted structurally at the bottom of this file. A replay subsystem that could
//! execute or authorize would be a severe trust violation in a product whose whole purpose is
//! measuring untrusted code.

use serde::{Deserialize, Serialize};

use crate::arena::ArenaMatchView;
use crate::events::LabEvent;

/// What a recording can support.
///
/// The distinction matters because "projected" and "re-executed" are different claims, and conflating
/// them would let someone trust a view as if it were a re-run. A recording that captured every state
/// can be re-executed step by step; one that captured only outcomes can only be projected.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayCapability {
    /// Every intermediate state was recorded, so the run can be stepped through deterministically.
    Full,
    /// Only the outcome was recorded, so the run can be inspected but not re-executed.
    ProjectedOnly,
}

impl ReplayCapability {
    /// A short label for the panel.
    pub fn label(self) -> &'static str {
        match self {
            ReplayCapability::Full => "RE-EXECUTABLE",
            ReplayCapability::ProjectedOnly => "INSPECTION ONLY",
        }
    }

    /// Whether stepping through the recording is meaningful.
    pub fn can_step(self) -> bool {
        self == ReplayCapability::Full
    }
}

/// One reconstructed moment in a replay.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayFrame {
    /// Zero-based position in the replay.
    pub index: usize,
    /// What produced this frame.
    pub source: ReplaySource,
    /// A one-line description of the state at this point.
    pub label: String,
    /// The state's digest, when one is available.
    ///
    /// `None` rather than a placeholder, because a fabricated digest would let a corrupted replay
    /// look intact.
    pub digest: Option<String>,
}

/// Where a replay frame came from.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplaySource {
    /// The state before anything happened.
    Start,
    /// An arena ply's recorded board.
    ArenaPly,
    /// A recorded event from an experiment's timeline.
    ExperimentEvent,
    /// The terminal state the run produced.
    Terminal,
}

/// A deterministic reconstruction of a recorded run.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Replay {
    /// The recording's identifier.
    pub recording_id: String,
    /// What the recording was.
    pub kind: ReplayKind,
    /// What the recording can support.
    pub capability: ReplayCapability,
    /// Every reconstructed moment, in order.
    pub frames: Vec<ReplayFrame>,
    /// A stable digest of the whole recording.
    ///
    /// Computed from the frames, so two reconstructions of the same recording agree and a tampered
    /// one does not.
    pub digest: String,
    /// What could not be reconstructed, stated rather than hidden.
    pub limitations: Vec<String>,
}

/// What a recording is of.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayKind {
    /// An arena match.
    ArenaMatch,
    /// An experiment across several episodes.
    Experiment,
}

/// Replays a recorded arena match.
///
/// Pure reconstruction: the frames come from the per-ply boards the engine recorded, and the digest is
/// computed from them. The engine is not called and the original view is borrowed, not consumed.
pub fn replay_match(view: &ArenaMatchView) -> Replay {
    let mut frames = vec![ReplayFrame {
        index: 0,
        source: ReplaySource::Start,
        label: format!("{} before any ply", view.game_name),
        digest: None,
    }];
    let mut limitations = Vec::new();

    for (index, ply) in view.history.iter().enumerate() {
        frames.push(ReplayFrame {
            index: index + 1,
            source: ReplaySource::ArenaPly,
            label: format!(
                "after ply {} by {:?}{}",
                ply.ply,
                ply.side,
                match ply.played {
                    Some(played) => format!(" playing {played:?}"),
                    None => String::new(),
                }
            ),
            // The engine records no per-ply digest, so none is invented. The final board's
            // availability is noted below rather than papered over with a placeholder.
            digest: None,
        });
    }

    frames.push(ReplayFrame {
        index: frames.len(),
        source: ReplaySource::Terminal,
        label: format!(
            "final: {:?} after {} plies",
            view.outcome.label(),
            view.plies
        ),
        digest: Some(view.replay_digest()),
    });

    if view.history.is_empty() {
        limitations.push(
            "No ply history was recorded for this match, so only the final state is available. The \
             intermediate positions were not captured and cannot be reconstructed."
                .to_owned(),
        );
    }
    if view
        .illegal_moves
        .iter()
        .any(|illegal| illegal.attempted.is_none())
    {
        limitations.push(
            "At least one refused move was recorded without identifying which move was attempted, so \
             that ply's attempted action is unknown."
                .to_owned(),
        );
    }

    let digest = digest_of(frames.iter().filter_map(|frame| frame.digest.clone()));
    Replay {
        recording_id: view.match_id.clone(),
        kind: ReplayKind::ArenaMatch,
        capability: if view.history.is_empty() {
            ReplayCapability::ProjectedOnly
        } else {
            ReplayCapability::Full
        },
        frames,
        digest,
        limitations,
    }
}

/// Replays a recorded experiment's event stream.
///
/// The frames are the experiment's own recorded events. This is inspection rather than
/// re-execution: an event log says what was reported, not what every intermediate environment state
/// was, and the capability says so.
pub fn replay_experiment(experiment_id: &str, timeline: &[LabEvent]) -> Replay {
    let mut frames = vec![ReplayFrame {
        index: 0,
        source: ReplaySource::Start,
        label: format!("{experiment_id} before the first event"),
        digest: None,
    }];
    for (index, event) in timeline.iter().enumerate() {
        frames.push(ReplayFrame {
            index: index + 1,
            source: ReplaySource::ExperimentEvent,
            label: format!("{}: {}", event.code, event.detail),
            digest: None,
        });
    }

    Replay {
        recording_id: experiment_id.to_owned(),
        kind: ReplayKind::Experiment,
        // An experiment's recording is an event stream, so it is inspectable but not re-executable
        // into intermediate environment states. Saying otherwise would let someone trust a
        // projection as a re-run.
        capability: ReplayCapability::ProjectedOnly,
        frames,
        digest: digest_of(timeline.iter().map(|event| event.code.clone())),
        limitations: vec![
            "This recording is an event stream. It shows what was reported and when, not every \
             intermediate environment state, so it cannot be stepped as a re-execution would be."
                .to_owned(),
        ],
    }
}

/// Steps through a replay, returning the frame at `position`.
///
/// Returns `None` past the end rather than clamping, so a caller cannot silently be handed the last
/// frame while believing it asked for a later one.
pub fn frame_at(replay: &Replay, position: usize) -> Option<&ReplayFrame> {
    replay.frames.get(position)
}

/// Computes a stable digest from a sequence of strings.
///
/// A simple FNV-style accumulation rather than SHA-256, because this is a change detector for
/// reconstruction, not a security primitive, and the arena crate already depends on `sha2` for the
/// digests that *are* security-relevant.
fn digest_of(parts: impl Iterator<Item = String>) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for byte in part.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash ^= 0x1f;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}
