//! The arena: a controlled game laboratory, connected to the product.
//!
//! # What this module does and does not do
//!
//! It runs matches through the engine's existing [`play_match`] and projects what that produced. It
//! does not implement a game, decide a winner, or validate a move. Every rule question is answered by
//! the engine and the `Game` implementation, and every legality question is answered by the
//! environment. This module is a camera, not a referee.
//!
//! # The board in this panel is a recording, not a live state machine
//!
//! `play_match` is atomic: it runs the whole match and returns a `MatchResult`. The board the panel
//! draws comes from `GameTelemetry`, which the engine records per ply *after* the move was applied.
//! So the sequence of boards is the game's own history, and the final one is the game's own final
//! position. Nothing here re-derives a position, and there is no second board the frontend could
//! disagree with.
//!
//! That has a visible consequence worth stating: there is no "your turn" in a completed match, because
//! the match is over. The panel shows the turn that *would* be next rather than inviting a click.
//!
//! # No human seat
//!
//! The arena seats a model in the tested seat and a `ReferenceStrategy` in the opponent seat. There is
//! no human-turn path, and none was invented: a human player would be a parallel execution path with
//! its own state transitions, and the whole reason to trust this panel is that the environment is the
//! only thing that moves the board. A match is agent against agent, and the panel says so.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use probelab_core::{
    GameBoard, GameKind, GameMove, GameSide, GameTelemetry, IntegrityEventKind, ModelCallFailure,
    TelemetryEvent, TelemetryRecorder, TokenUsage,
};
use probelab_engine::arena::{
    play_match, ArenaError, BoxedSeat, IllegalMove, MatchConfig, MatchOutcome, MatchResult,
    PlyOutcome, SeatKind,
};
use serde::{Deserialize, Serialize};

use crate::agents::AgentRegistry;
use crate::events::{EventSource, LabEvent, LabTimeline, Severity};

/// The games the arena can currently be asked to play.
///
/// A closed list rather than a probe of the registry, because it is a statement about what this build
/// supports. Rock-paper-scissors and battleship have types in the schema and no implementation behind
/// this command, and listing them would offer a match that fails.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ArenaGame {
    /// Stable identifier, matching the core game kind.
    pub id: String,
    /// Name for the header.
    pub name: String,
    /// What the game is, in one sentence.
    pub description: String,
    /// Board width in cells, where the game has a fixed grid.
    pub width: usize,
    /// Board height in cells, where the game has a fixed grid.
    pub height: usize,
    /// The most plies the game can take.
    pub max_plies: u32,
}

/// Every game this build can play.
///
/// The two shipped games are chosen because they measure different things, not because they are the
/// only two that exist in the schema. Tic-tac-toe has a *proved* ceiling: against a perfect opponent
/// the best a policy can do is draw, so every non-loss is evidence about not blundering. Connect four
/// has no proved ceiling and a reference opponent that can be beaten, which is what makes it the game
/// a real model is worth running against.
pub fn available_games() -> Vec<ArenaGame> {
    vec![
        ArenaGame {
            id: "tic_tac_toe".to_owned(),
            name: "Tic-Tac-Toe".to_owned(),
            description:
                "Three by three, first side first. Exhaustively solvable, so the ceiling is a \
                          true ceiling rather than a good heuristic."
                    .to_owned(),
            width: 3,
            height: 3,
            max_plies: 9,
        },
        ArenaGame {
            id: "connect_four".to_owned(),
            name: "Connect Four".to_owned(),
            description:
                "Seven columns by six rows, played by dropping discs under gravity. No proved \
                          ceiling: the reference opponent is a depth-bounded search with published \
                          limits, so it can be beaten and its strength is a stated number."
                    .to_owned(),
            width: 7,
            height: 6,
            max_plies: 42,
        },
    ]
}

/// The display name for a game, falling back to its stable identifier.
///
/// One lookup, so a view cannot call the same match "Connect Four" in the header and "connect_four"
/// in the archive.
fn game_name(kind: GameKind) -> String {
    available_games()
        .into_iter()
        .find(|game| game.id == kind.as_str())
        .map(|game| game.name)
        .unwrap_or_else(|| kind.as_str().to_owned())
}

/// Why an arena match could not be started.
#[derive(Clone, Debug, thiserror::Error)]
pub enum ArenaRunError {
    /// The requested game is not implemented in this build.
    #[error("this build cannot play {0:?}")]
    UnknownGame(String),
    /// Neither seat named a registered agent.
    #[error("no agent is registered as {0:?}")]
    UnknownAgent(String),
    /// The named agent cannot legally play the requested game.
    ///
    /// Refused in the backend rather than discovered by the UI, so a frontend cannot offer a pairing
    /// that the environment would not accept.
    #[error("{agent} cannot play {game}")]
    IncompatibleAgent {
        /// The agent that was asked.
        agent: String,
        /// The game it cannot play.
        game: String,
    },
    /// The engine refused the match.
    #[error("the arena could not play the match: {0}")]
    Engine(String),
}

/// One recorded ply, projected for the panel.
///
/// The board is the position *after* the move, taken from the engine's own telemetry. It is included
/// per ply so the panel can step through a real history rather than re-running the game to work out
/// where it got to.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlyView {
    /// One-based ply index.
    pub ply: u32,
    /// Which side was to move.
    pub side: GameSide,
    /// The board after this ply.
    pub board: GameBoard,
    /// The move that was played, absent on a ply the tested side did not get to move on.
    pub played: Option<GameMove>,
}

impl From<&GameTelemetry> for PlyView {
    fn from(telemetry: &GameTelemetry) -> Self {
        Self {
            ply: telemetry.ply,
            side: telemetry.side,
            board: telemetry.board.clone(),
            played: telemetry.played,
        }
    }
}

/// An illegal move, projected with the reason preserved.
///
/// Recorded rather than reduced to a count, because "how often, and what did it try" is the difference
/// between a policy that misunderstood the rules and one that merely plays badly.
///
/// `attempted` is absent when the failure did not come from a refused move — a transport or protocol
/// failure names no move, and inventing one to fill the field would be a fabricated datum in a result
/// bundle.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IllegalMoveView {
    /// The ply it happened on.
    pub ply: u32,
    /// The move the model returned, when the failure identified one.
    pub attempted: Option<GameMove>,
    /// Why the environment refused it.
    pub reason: String,
}

impl From<&IllegalMove> for IllegalMoveView {
    fn from(illegal: &IllegalMove) -> Self {
        Self {
            ply: illegal.ply,
            attempted: Some(illegal.attempted),
            reason: illegal.reason.clone(),
        }
    }
}

/// How a match ended, as the panel states it.
///
/// Deliberately richer than the engine's three-valued [`MatchOutcome`], because that enum is scoped to
/// the tested side and the panel has to say more than that. A draw, a win, a loss, and a match that
/// never produced a result are four different facts, and collapsing the last two would turn a harness
/// failure into a game outcome.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArenaOutcome {
    /// The tested side won.
    Win,
    /// The match was drawn.
    Draw,
    /// The tested side lost.
    Loss,
    /// The match produced no result because execution failed.
    ///
    /// Not a loss. Nothing was won or lost; the arena stopped, and saying "loss" would blame a policy
    /// for a harness problem.
    NoResult,
}

impl ArenaOutcome {
    /// A short label for the panel.
    pub fn label(self) -> &'static str {
        match self {
            ArenaOutcome::Win => "WIN",
            ArenaOutcome::Draw => "DRAW",
            ArenaOutcome::Loss => "LOSS",
            ArenaOutcome::NoResult => "NO RESULT",
        }
    }
}

/// What one turn cost, and who played it.
///
/// Separated from [`PlyView`] on purpose. A `PlyView` is the *board*: what happened. This is the
/// *cost*: how long it took, what it cost in tokens, and under which prompt template. They are kept
/// apart because they answer different questions and have different lifetimes — the board is needed
/// to replay, the cost is needed to explain — and because merging them would make the replay
/// projection carry provider metadata it does not use.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnTelemetry {
    /// One-based ply index.
    pub ply: u32,
    /// Which side moved.
    pub side: GameSide,
    /// What kind of seat moved: a shipped policy or a real model.
    pub seat: SeatKind,
    /// The seat's label, so a transcript reads without a lookup.
    pub seat_label: String,
    /// How long the decision took, in milliseconds.
    pub elapsed_ms: u64,
    /// Provider token usage, when the seat is model-backed and the provider reported it.
    ///
    /// `None` is meaningful, not missing: a policy seat has no tokens, and so does a provider that
    /// does not report them. Reporting zero for either would make "cost nothing" indistinguishable
    /// from "we do not know".
    pub usage: Option<TokenUsage>,
    /// The prompt template that produced the decision.
    pub prompt_template_version: Option<String>,
    /// The model's own reply, sanitized and bounded, when the seat is model-backed.
    pub response_text: Option<String>,
    /// A sanitized failure, when the decision failed.
    pub failure: Option<ModelCallFailure>,
    /// The board this seat was actually shown.
    ///
    /// Carried so "the model was shown the legal state available to its side" is a checkable claim
    /// rather than a promise in a comment. A seat that were ever shown a hidden cell would show up
    /// here, and the assertion in the arena tests is on this field rather than on the outcome.
    pub request_board: GameBoard,
}

impl From<&PlyOutcome> for TurnTelemetry {
    fn from(outcome: &PlyOutcome) -> Self {
        Self {
            ply: outcome.ply,
            side: outcome.side,
            seat: outcome.seat,
            seat_label: outcome.seat_label.clone(),
            elapsed_ms: outcome.elapsed_ms,
            usage: outcome.usage.clone(),
            prompt_template_version: outcome.prompt_template_version.clone(),
            response_text: outcome.response_text.clone(),
            failure: outcome.failure.clone(),
            request_board: outcome.request_board.clone(),
        }
    }
}

/// The full record of one played match.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArenaMatchView {
    /// Stable identifier for this match.
    pub match_id: String,
    /// Which game was played.
    pub game: String,
    /// The game name for the header.
    pub game_name: String,
    /// What the match produced, or that it produced nothing.
    pub outcome: ArenaOutcome,
    /// The tested policy's identifier.
    pub tested_agent: String,
    /// The opponent's identifier.
    pub opponent_agent: String,
    /// What kind of seat the tested side was.
    pub tested_seat: SeatKind,
    /// What kind of seat the opponent was.
    pub opponent_seat: SeatKind,
    /// Which side the tested agent played.
    pub tested_side: GameSide,
    /// The seed the opening was derived from, so the match replays.
    pub seed: u64,
    /// Plies actually played.
    pub plies: u32,
    /// The final board, as the engine recorded it.
    pub final_board: GameBoard,
    /// The side that won, when there is one.
    pub winner: Option<GameSide>,
    /// The cells the winner completed a line on, when the game has lines.
    ///
    /// Produced by the engine's own winning-line rule rather than by the panel. The renderer used to
    /// have to be told nothing about a result and inferred everything from the final board, which
    /// meant a highlight drawn in the UI was a second, unaudited opinion about who won what.
    ///
    /// Four cells for connect four and three for tic-tac-toe, so it is a flat list of coordinates
    /// rather than a fixed triple: a fixed `[u8; 3]` cannot express a four-in-a-row without either
    /// truncating it — which would highlight the wrong cells — or being a different type per game.
    pub winning_line: Option<Vec<[u8; 2]>>,
    /// Illegal moves the tested side returned.
    pub illegal_moves: Vec<IllegalMoveView>,
    /// The recorded ply history, in order.
    pub history: Vec<PlyView>,
    /// Per-turn cost and identity, in order.
    pub turns: Vec<TurnTelemetry>,
    /// The match's own event stream.
    pub timeline: Vec<LabEvent>,
    /// Why the match ended, when it failed.
    pub failure: Option<String>,
    /// Whether the match was stopped by the operator rather than played out.
    ///
    /// Distinct from `failure`: a cancelled match is not broken, and reporting it as a failure would
    /// put a deliberate stop into the same bucket as a provider that refused a connection.
    pub cancelled: bool,
}

impl ArenaMatchView {
    /// How many illegal moves the tested side returned.
    pub fn illegal_move_count(&self) -> usize {
        self.illegal_moves.len()
    }

    /// A stable digest of the recorded match.
    ///
    /// Computed from the recording rather than stored, so it cannot drift from the history it
    /// describes. This is what the replay subsystem checks a reconstruction against, so it is a change
    /// detector rather than a security primitive -- the security-relevant digests in this lab are the
    /// engine's own SHA-256 replay digests.
    ///
    /// It absorbs **every** field a reader would treat as the match's result, not just the ply boards.
    /// An earlier version bound the game, the seed, the two seats, and the per-ply boards -- and left
    /// `outcome`, `winner`, `plies`, `tested_side`, `illegal_moves`, `final_board`, and `failure`
    /// unbound. That meant a recording edited to claim `loss` where the engine recorded `draw` still
    /// produced the same digest, and the digest is displayed in the replay panel as the evidence that
    /// the recording is intact. A digest that does not cover the result is not a digest of the result.
    ///
    /// The field list is enumerated rather than derived from `Debug`, for the reason
    /// `ArenaMatchView` is `Debug`-only: a `format!("{:?}")` would change the digest whenever someone
    /// reformats a struct or a field's `Debug` impl, which is how a wire format becomes a liability.
    pub fn replay_digest(&self) -> String {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        let mut absorb = |text: &str| {
            for byte in text.as_bytes() {
                hash ^= u64::from(*byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        };
        // Identity: which match this is, and what produced it.
        absorb(self.match_id.as_str());
        absorb(self.game.as_str());
        absorb(self.game_name.as_str());
        absorb(&self.seed.to_string());
        absorb(self.tested_agent.as_str());
        absorb(self.opponent_agent.as_str());
        absorb(match self.tested_side {
            GameSide::First => "first",
            GameSide::Second => "second",
        });
        // Result: the fields the panel renders as what happened.
        absorb(match self.outcome {
            ArenaOutcome::Win => "win",
            ArenaOutcome::Draw => "draw",
            ArenaOutcome::Loss => "loss",
            ArenaOutcome::NoResult => "no_result",
        });
        absorb(match self.winner {
            None => "none",
            Some(GameSide::First) => "first",
            Some(GameSide::Second) => "second",
        });
        absorb(&self.plies.to_string());
        // The winning line is part of the result the panel renders, so a recording claiming a
        // different line has to move the digest for the same reason a changed outcome does.
        absorb(
            self.winning_line
                .as_ref()
                .map(|line| {
                    line.iter()
                        .map(|cell| format!("{},{}", cell[0], cell[1]))
                        .collect::<Vec<_>>()
                        .join(";")
                })
                .unwrap_or_default()
                .as_str(),
        );
        absorb(
            serde_json::to_string(&self.final_board)
                .unwrap_or_default()
                .as_str(),
        );
        // Rule violations: a recording claiming no illegal moves when the engine recorded one is a
        // different match.
        absorb(&self.illegal_moves.len().to_string());
        for illegal in &self.illegal_moves {
            absorb(&illegal.ply.to_string());
            absorb(illegal.reason.as_str());
        }
        // Terminal condition, when the match did not complete.
        absorb(self.failure.as_deref().unwrap_or(""));
        // The ply history itself.
        for ply in &self.history {
            absorb(&ply.ply.to_string());
            absorb(match ply.side {
                GameSide::First => "first",
                GameSide::Second => "second",
            });
            absorb(
                serde_json::to_string(&ply.board)
                    .unwrap_or_default()
                    .as_str(),
            );
            absorb(
                serde_json::to_string(&ply.played)
                    .unwrap_or_default()
                    .as_str(),
            );
        }
        format!("{hash:016x}")
    }
}

/// A digest over an ordered list of strings.
///
/// # Why this exists
///
/// [`ArenaMatchView::replay_digest`] fingerprints one recording. Statistics over a group of recordings
/// need the same thing one level up: a single value that changes if, and only if, the set of
/// recordings behind a number changes. That lets two statistics blocks carrying the same digest be
/// known to have come from identical inputs, so they cannot disagree unless one of them is wrong.
///
/// It lives here rather than in the statistics module because this is where the hashing primitive
/// already is. A second copy of FNV-1a in a sibling module is the kind of quiet duplication that lets
/// two digests of "the same" thing stop matching, and neither copy would be wrong on its own.
///
/// # Order matters
///
/// The digest is over a *sequence*, not a set. Reordering the inputs changes it. That is deliberate:
/// statistics are computed in a defined order and the output is stable, so the provenance should be
/// sensitive to the order rather than hiding it.
pub fn provenance_digest<'a>(parts: impl IntoIterator<Item = &'a str>) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for byte in part.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        // A separator, so ["ab", "c"] and ["a", "bc"] cannot collide.
        hash ^= 0x1f;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Captures what the engine records during a match, in memory, and tees it onward.
///
/// The engine already writes every ply to a recorder; this holds a copy so the panel can project a
/// board history, and forwards the identical event to the durable sink when one is installed. Doing
/// both in one `record` call is the point: if forwarding were a separate step it could be skipped on
/// an early return, and the stored record and the panel would then disagree about the same match.
///
/// `turns` is filled by the runner's per-ply callback rather than by parsing telemetry events back
/// apart. The engine already hands the runner a fully-formed [`PlyOutcome`] with the provider
/// metadata attached; recovering the same facts from a round-tripped event would mean re-deriving
/// fields the engine had in hand, and a round trip is where such fields quietly go missing.
#[derive(Clone, Default)]
struct MatchCapture {
    plies: Arc<Mutex<Vec<GameTelemetry>>>,
    turns: Arc<Mutex<Vec<TurnTelemetry>>>,
    timeline: Arc<Mutex<LabTimeline>>,
    integrity: Arc<Mutex<Vec<IntegrityEventKind>>>,
    /// The durable recorder, when the caller installed one.
    sink: Arc<Mutex<Option<Arc<dyn TelemetryRecorder>>>>,
}

impl MatchCapture {
    /// Installs the durable recorder this capture forwards to.
    fn tee_to(&self, sink: Arc<dyn TelemetryRecorder>) -> Result<(), String> {
        *self
            .sink
            .lock()
            .map_err(|_| CAPTURE_LOCK_POISONED.to_owned())? = Some(sink);
        Ok(())
    }

    fn push_turn(&self, turn: TurnTelemetry) {
        if let Ok(mut turns) = self.turns.lock() {
            turns.push(turn);
        }
    }
}

/// The message a poisoned capture lock reports.
///
/// The capture is written from inside the engine's step loop, so a panic on these locks would abort a
/// live match -- and because it is called once per ply, a single early fault would poison it for the
/// rest of the match. Reporting an error instead means the match ends with a stated reason rather than
/// taking the process with it.
const CAPTURE_LOCK_POISONED: &str =
    "the arena match capture was left locked by an earlier fault; this match cannot be recorded";

impl TelemetryRecorder for MatchCapture {
    fn record(&self, event: &TelemetryEvent) -> Result<(), String> {
        // Forward first, so a sink failure is reported as a failure rather than silently dropped while
        // the in-memory copy succeeds.
        //
        // The clone is bound to a local *before* the `if let`. Leaving it in the scrutinee would hold
        // the guard for the whole block under edition 2021 temporary-lifetime rules, which means the
        // lock would still be held across the `sink.record(..)` call below -- a foreign call that
        // writes to SQLite and, in the desktop wiring, emits a Tauri event.
        let sink = self
            .sink
            .lock()
            .map_err(|_| CAPTURE_LOCK_POISONED.to_owned())?
            .clone();
        if let Some(sink) = sink {
            sink.record(event)?;
        }
        // Integrity is recorded from the event itself, because that is the only place the environment
        // states it. A model claiming it was blocked would be a claim, not a record.
        if let Some(kind) = event.integrity_event {
            self.integrity
                .lock()
                .map_err(|_| CAPTURE_LOCK_POISONED.to_owned())?
                .push(kind);
            let lab_event = LabEvent::integrity(
                0,
                kind,
                EventSource::Telemetry,
                &format!("match {} recorded {kind:?}", event.run_id),
            );
            self.timeline
                .lock()
                .map_err(|_| CAPTURE_LOCK_POISONED.to_owned())?
                .push(lab_event);
        }
        if let Some(game) = event.game.as_ref() {
            self.plies
                .lock()
                .map_err(|_| CAPTURE_LOCK_POISONED.to_owned())?
                .push(game.clone());
            self.timeline
                .lock()
                .map_err(|_| CAPTURE_LOCK_POISONED.to_owned())?
                .push(LabEvent::new(
                    0,
                    "arena.ply_recorded",
                    EventSource::Telemetry,
                    Severity::Info,
                    &format!(
                        "ply {} by {:?}{}",
                        game.ply,
                        game.side,
                        match game.played {
                            Some(played) => format!(" playing {played:?}"),
                            None => String::new(),
                        }
                    ),
                ));
        }
        Ok(())
    }
}

/// A completed match, ready to be shown.
#[derive(Clone, Debug)]
pub struct ArenaMatch {
    /// The projection the panel renders.
    pub view: ArenaMatchView,
    /// The engine's own result, for callers that want the unprojected facts.
    pub result: Option<MatchResult>,
}

/// Everything about a match that is fixed before the first ply.
///
/// A struct rather than seven positional parameters, for the same reason the engine has its own
/// `MatchConfig`: at that arity, three adjacent `&str`s and two `u64`s in a row is a call site nobody
/// can review, and swapping the tested agent with the opponent compiles silently and runs the match
/// backwards. It also gives the result a single source for the fields it reports.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArenaMatchSpec {
    /// Which game to play.
    pub game: GameKind,
    /// The agent under test.
    pub tested_agent: String,
    /// The opponent policy.
    pub opponent_agent: String,
    /// Which side the tested agent plays.
    pub tested_side: GameSide,
    /// The seed the opening derives from, so the match replays.
    pub seed: u64,
    /// The identifier this match is recorded under.
    pub match_id: String,
}

impl ArenaMatchSpec {
    /// Builds a specification.
    pub fn new(
        game: GameKind,
        tested_agent: &str,
        opponent_agent: &str,
        tested_side: GameSide,
        seed: u64,
        match_id: &str,
    ) -> Self {
        Self {
            game,
            tested_agent: tested_agent.to_owned(),
            opponent_agent: opponent_agent.to_owned(),
            tested_side,
            seed,
            match_id: match_id.to_owned(),
        }
    }
}

/// Runs arena matches.
pub struct ArenaRunner {
    agents: AgentRegistry,
}

impl ArenaRunner {
    /// Builds a runner over the agent registry.
    pub fn new() -> Self {
        Self {
            agents: AgentRegistry::built_in(),
        }
    }

    /// Builds a runner over a supplied registry, for tests and for a registry with extra entries.
    pub fn with_agents(agents: AgentRegistry) -> Self {
        Self { agents }
    }

    /// Every agent that can legally be seated in `game`.
    ///
    /// The filter is the registry's own `supported_games`, so compatibility is a backend fact. An agent
    /// with no entry for a game is never returned, which means adding a game cannot surface a policy
    /// that has no idea what the rules are.
    pub fn agents_for(&self, game: GameKind) -> Vec<crate::agents::AgentSpec> {
        self.agents
            .specs()
            .into_iter()
            .filter(|spec| spec.supported_games.contains(&game))
            .collect()
    }

    /// Refuses an agent that cannot legally play this game.
    ///
    /// Checked here rather than in the UI, which is the whole point of putting `supported_games` in
    /// the registry: a frontend cannot offer a pairing the arena would reject, and adding a game later
    /// cannot silently surface policies that have no idea what its rules are.
    fn require_compatible(&self, agent: &str, game: GameKind) -> Result<(), ArenaRunError> {
        let spec = self
            .agents
            .spec(agent)
            .ok_or_else(|| ArenaRunError::UnknownAgent(agent.to_owned()))?;
        if !spec.supported_games.contains(&game) {
            return Err(ArenaRunError::IncompatibleAgent {
                agent: agent.to_owned(),
                game: game.as_str().to_owned(),
            });
        }
        Ok(())
    }

    /// The plies recorded during a match.
    ///
    /// Read after the match has already finished, so a poisoned lock is a fault in something else
    /// rather than a failure of this match. Panicking here would discard a completed match because a
    /// cache could not be read, so the failure travels through the existing `ArenaRunError::Engine`
    /// rather than introducing a variant that means nothing outside this file.
    fn capture_plies(&self, capture: &MatchCapture) -> Result<Vec<GameTelemetry>, String> {
        capture
            .plies
            .lock()
            .map(|plies| plies.clone())
            .map_err(|_| CAPTURE_LOCK_POISONED.to_owned())
    }

    fn capture_turns(&self, capture: &MatchCapture) -> Result<Vec<TurnTelemetry>, String> {
        capture
            .turns
            .lock()
            .map(|turns| turns.clone())
            .map_err(|_| CAPTURE_LOCK_POISONED.to_owned())
    }

    /// The timeline recorded during a match.
    fn capture_timeline(&self, capture: &MatchCapture) -> Result<LabTimeline, String> {
        capture
            .timeline
            .lock()
            .map(|timeline| timeline.clone())
            .map_err(|_| CAPTURE_LOCK_POISONED.to_owned())
    }

    /// Builds both seats for a specification from the agent registry.
    ///
    /// Performs the compatibility check first, so an unsupported pairing is a refusal rather than a
    /// match that fails on the first ply. Kept separate from [`ArenaRunner::play`] because a caller
    /// may seat a *model*, which is not a registry entry at all — it is a runtime configuration — and
    /// the two paths must reach the same `play` for that to be safe.
    pub fn seats_for(
        &self,
        spec: &ArenaMatchSpec,
    ) -> Result<(BoxedSeat, BoxedSeat), ArenaRunError> {
        self.require_supported_game(spec.game)?;
        self.require_compatible(&spec.tested_agent, spec.game)?;
        self.require_compatible(&spec.opponent_agent, spec.game)?;
        let tested = self
            .agents
            .build_seat(&spec.tested_agent, spec.seed)
            .map_err(|_| ArenaRunError::UnknownAgent(spec.tested_agent.clone()))?;
        let opponent = self
            .agents
            .build_seat(&spec.opponent_agent, spec.seed)
            .map_err(|_| ArenaRunError::UnknownAgent(spec.opponent_agent.clone()))?;
        Ok((tested, opponent))
    }

    /// Refuses a game this build does not implement.
    fn require_supported_game(&self, game: GameKind) -> Result<(), ArenaRunError> {
        if matches!(game, GameKind::TicTacToe | GameKind::ConnectFour) {
            return Ok(());
        }
        Err(ArenaRunError::UnknownGame(format!("{game:?}")))
    }

    /// Plays one match with no live view and no cancel button.
    ///
    /// The convenience form of [`ArenaRunner::play`], for callers that genuinely have neither a UI to
    /// drive nor a way to stop — a batch run, a test, a one-shot command. It is a thin wrapper rather
    /// than a separate implementation so a batch run and a watched run execute identical code; the
    /// only difference is that nobody is watching.
    pub fn play_simple(
        &self,
        spec: &ArenaMatchSpec,
        sink: Option<Arc<dyn TelemetryRecorder>>,
    ) -> Result<ArenaMatch, ArenaRunError> {
        let (tested, opponent) = self.seats_for(spec)?;
        self.play(
            spec,
            tested,
            opponent,
            sink,
            None,
            &mut |_turn: &TurnTelemetry| {},
        )
    }

    /// Plays one match from two already-built seats and projects it.
    ///
    /// Takes seats rather than registry identifiers so a caller can seat a real model: a model is a
    /// runtime configuration, not a registry entry, and pretending otherwise would mean either
    /// registering every conceivable model (impossible) or refusing model seats (which loses the
    /// feature this exists for).
    ///
    /// The seats still arrive through the same `ArenaSeat` boundary, so neither can reach the board,
    /// the rules, the telemetry sink, or anything the environment did not put in the request.
    /// Compatibility for registry policies is checked by [`ArenaRunner::seats_for`], which a caller
    /// building seats itself is expected to have consulted; the game check happens here, because
    /// playing a game with no implementation would be a silent nonsense result rather than an error.
    ///
    /// `on_turn` is called after each ply with the authoritative board and that turn's cost, so a
    /// caller can drive a live view; `cancel` is polled at the same point, so stopping a match needs no
    /// second mechanism and cannot race with the turn loop.
    pub fn play(
        &self,
        spec: &ArenaMatchSpec,
        mut tested: BoxedSeat,
        mut opponent: BoxedSeat,
        sink: Option<Arc<dyn TelemetryRecorder>>,
        cancel: Option<&AtomicBool>,
        on_turn: &mut dyn FnMut(&TurnTelemetry),
    ) -> Result<ArenaMatch, ArenaRunError> {
        self.require_supported_game(spec.game)?;
        let tested_seat_kind = tested.kind();
        let opponent_seat_kind = opponent.kind();

        let capture = MatchCapture::default();
        // The durable recorder is installed before the first ply, so the tee forwards from the very
        // first event rather than being attached part-way through and leaving a gap in the stored
        // record that the panel's copy does not have.
        if let Some(sink) = sink {
            capture.tee_to(sink).map_err(ArenaRunError::Engine)?;
        }
        let config = MatchConfig::new(
            &spec.tested_agent,
            &spec.opponent_agent,
            spec.seed,
            &spec.match_id,
            &spec.match_id,
        );

        // One arm per implemented game, rather than a boxed `dyn Game`.
        //
        // `Game` has an associated `State`, so `Box<dyn Game>` is not expressible, and wrapping it in
        // an enum would mean a variant per game plus a match to reconstruct one — all to avoid a
        // two-line match. Dispatching here keeps the engine's generic `play_match` generic, and adding
        // a game later is exactly one more arm.
        let mut record_turn = |outcome: PlyOutcome| {
            let turn = TurnTelemetry::from(&outcome);
            on_turn(&turn);
            capture.push_turn(turn);
            // The single cancellation point. Polled in the same callback that drives the live view,
            // so "stop" and "observe" can never disagree about which ply is current, and there is no
            // second code path in which a match stops.
            cancel.is_none_or(|flag| !flag.load(Ordering::SeqCst))
        };
        let played = match spec.game {
            GameKind::TicTacToe => {
                let game = probelab_engine::arena::tictactoe::TicTacToe::default_game();
                play_match(
                    &game,
                    tested.as_mut(),
                    opponent.as_mut(),
                    spec.tested_side,
                    &config,
                    &capture,
                    &mut record_turn,
                )
            }
            GameKind::ConnectFour => {
                let game = probelab_engine::arena::connect_four::ConnectFour::default_game();
                play_match(
                    &game,
                    tested.as_mut(),
                    opponent.as_mut(),
                    spec.tested_side,
                    &config,
                    &capture,
                    &mut record_turn,
                )
            }
            // Unreachable: the guard above already refused every other kind. Kept exhaustive so a
            // future GameKind variant cannot fall through silently into a broken match.
            other => return Err(ArenaRunError::UnknownGame(format!("{other:?}"))),
        };

        match played {
            Ok(result) => {
                let plies = self
                    .capture_plies(&capture)
                    .map_err(ArenaRunError::Engine)?;
                let turns = self
                    .capture_turns(&capture)
                    .map_err(ArenaRunError::Engine)?;
                let final_board = plies
                    .last()
                    .map(|ply| ply.board.clone())
                    .unwrap_or_else(|| empty_board(spec.game));
                let mut timeline = self
                    .capture_timeline(&capture)
                    .map_err(ArenaRunError::Engine)?;
                timeline.push(LabEvent::new(
                    0,
                    "arena.match_completed",
                    EventSource::Telemetry,
                    Severity::Info,
                    &format!(
                        "{:?} after {} plies, {} illegal",
                        result.outcome,
                        result.plies,
                        result.illegal_moves.len()
                    ),
                ));
                // Read out of the terminal board before it is moved into the view: the winning line
                // is the engine's own answer about that position, not a second opinion drawn later.
                let winning_line = winning_line_for(&final_board, result.outcome, spec.tested_side);
                Ok(ArenaMatch {
                    view: ArenaMatchView {
                        match_id: spec.match_id.clone(),
                        game: spec.game.as_str().to_owned(),
                        game_name: game_name(spec.game),
                        outcome: match result.outcome {
                            MatchOutcome::Win => ArenaOutcome::Win,
                            MatchOutcome::Draw => ArenaOutcome::Draw,
                            MatchOutcome::Loss => ArenaOutcome::Loss,
                        },
                        tested_agent: spec.tested_agent.clone(),
                        opponent_agent: spec.opponent_agent.clone(),
                        tested_seat: tested_seat_kind,
                        opponent_seat: opponent_seat_kind,
                        tested_side: spec.tested_side,
                        seed: spec.seed,
                        plies: result.plies,
                        final_board,
                        winner: match result.outcome {
                            MatchOutcome::Win => Some(spec.tested_side),
                            MatchOutcome::Loss => Some(spec.tested_side.other()),
                            MatchOutcome::Draw => None,
                        },
                        winning_line,
                        illegal_moves: result
                            .illegal_moves
                            .iter()
                            .map(IllegalMoveView::from)
                            .collect(),
                        history: plies.iter().map(PlyView::from).collect(),
                        turns,
                        timeline: timeline.events().to_vec(),
                        failure: None,
                        cancelled: false,
                    },
                    result: Some(result),
                })
            }
            Err(error) => {
                // A failed match is a real outcome with its own record. It is reported as "no result"
                // and never as a loss, and the illegal move the engine refused is kept if it named one.
                let plies = self
                    .capture_plies(&capture)
                    .map_err(ArenaRunError::Engine)?;
                let turns = self
                    .capture_turns(&capture)
                    .map_err(ArenaRunError::Engine)?;
                let mut timeline = self
                    .capture_timeline(&capture)
                    .map_err(ArenaRunError::Engine)?;
                timeline.push(LabEvent::new(
                    0,
                    "arena.match_failed",
                    EventSource::Experiment,
                    Severity::Error,
                    &error.to_string(),
                ));
                let illegal = match &error {
                    ArenaError::IllegalMove { ply, reason } => vec![IllegalMoveView {
                        ply: *ply,
                        // The engine's refusal names the ply and the reason but not the move, so the
                        // move stays absent rather than being invented to fill the field.
                        attempted: None,
                        reason: reason.clone(),
                    }],
                    // A seat that named an unusable move is recorded as the rule violation it is, so
                    // it is counted beside every other illegal move rather than filed as a fault.
                    // The ply stays absent for the same reason as above: the parser refused the move
                    // before it became a board action, so there is no move to report.
                    ArenaError::SeatRefused { label, failure } => vec![IllegalMoveView {
                        ply: 0,
                        attempted: None,
                        reason: format!("{label} returned a move the board refuses: {failure}"),
                    }],
                    _ => Vec::new(),
                };
                let cancelled = matches!(error, ArenaError::Cancelled { .. });
                Ok(ArenaMatch {
                    view: ArenaMatchView {
                        match_id: spec.match_id.clone(),
                        game: spec.game.as_str().to_owned(),
                        game_name: game_name(spec.game),
                        outcome: ArenaOutcome::NoResult,
                        tested_agent: spec.tested_agent.clone(),
                        opponent_agent: spec.opponent_agent.clone(),
                        tested_seat: tested_seat_kind,
                        opponent_seat: opponent_seat_kind,
                        tested_side: spec.tested_side,
                        seed: spec.seed,
                        plies: u32::try_from(plies.len()).unwrap_or(u32::MAX),
                        final_board: plies
                            .last()
                            .map(|ply| ply.board.clone())
                            .unwrap_or_else(|| empty_board(spec.game)),
                        winner: None,
                        // A match that failed has no winner, so it has no winning line either.
                        winning_line: None,
                        illegal_moves: illegal,
                        history: plies.iter().map(PlyView::from).collect(),
                        turns,
                        timeline: timeline.events().to_vec(),
                        failure: Some(error.to_string()),
                        cancelled,
                    },
                    result: None,
                })
            }
        }
    }
}

/// The opening board, for a match that produced no plies at all.
///
/// Nine empty cells or forty-two, never an empty vector: a zero-length board would render as nothing
/// and look like a missing result rather than a real one. This is reached only when a match failed on
/// its very first ply.
fn empty_board(kind: GameKind) -> GameBoard {
    match kind {
        GameKind::TicTacToe => GameBoard::TicTacToe {
            cells: vec![probelab_core::TicTacToeCell::Empty; 9],
        },
        GameKind::ConnectFour => GameBoard::ConnectFour {
            columns: probelab_engine::arena::connect_four::COLUMNS,
            rows: probelab_engine::arena::connect_four::ROWS,
            cells: vec![
                probelab_core::ConnectFourCell::Empty;
                probelab_engine::arena::connect_four::CELLS
            ],
        },
        other => {
            // Unreachable for the two implemented games. Reached only if a third game is added
            // without a board to show, in which case an empty tic-tac-toe is a visible placeholder
            // rather than a silent panic.
            debug_assert_eq!(
                other,
                GameKind::Rps,
                "unimplemented game reached empty_board"
            );
            GameBoard::TicTacToe {
                cells: vec![probelab_core::TicTacToeCell::Empty; 9],
            }
        }
    }
}

/// The cells a winner completed, read out of the engine's own winning-line rule.
///
/// `None` for a draw, for a loss, and for any game without lines — a board nobody won on has no
/// winning line, and inventing one from the final position would be a claim the engine never made.
fn winning_line_for(
    board: &GameBoard,
    outcome: MatchOutcome,
    tested_side: GameSide,
) -> Option<Vec<[u8; 2]>> {
    if outcome == MatchOutcome::Draw {
        return None;
    }
    let winner = match outcome {
        MatchOutcome::Win => tested_side,
        MatchOutcome::Loss => tested_side.other(),
        MatchOutcome::Draw => return None,
    };
    match board {
        GameBoard::TicTacToe { cells } => {
            if cells.len() != 9 {
                return None;
            }
            let mut fixed: [probelab_core::TicTacToeCell; 9] =
                [probelab_core::TicTacToeCell::Empty; 9];
            for (slot, cell) in fixed.iter_mut().zip(cells.iter()) {
                *slot = *cell;
            }
            probelab_engine::arena::tictactoe::winning_line(&fixed, winner)
                .map(|line| line.iter().map(|index| [*index as u8, 0u8]).collect())
        }
        GameBoard::ConnectFour { .. } => {
            probelab_engine::arena::connect_four::winning_cells_for(board, winner).map(|line| {
                line.iter()
                    .map(|(column, row)| [*column as u8, *row as u8])
                    .collect()
            })
        }
        _ => None,
    }
}

impl Default for ArenaRunner {
    fn default() -> Self {
        Self::new()
    }
}
