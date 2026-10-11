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

use std::sync::{Arc, Mutex};

use probelab_core::{
    GameBoard, GameKind, GameMove, GameSide, GameTelemetry, IntegrityEventKind, TelemetryEvent,
    TelemetryRecorder,
};
use probelab_engine::arena::{
    play_match, ArenaError, IllegalMove, MatchConfig, MatchOutcome, MatchResult,
};
use serde::{Deserialize, Serialize};

use crate::agents::AgentRegistry;
use crate::events::{EventSource, LabEvent, LabTimeline, Severity};

/// The games the arena can currently be asked to play.
///
/// A closed list rather than a probe of the registry, because it is a statement about what this build
/// supports. Connect four, rock-paper-scissors, and battleship have types in the schema and no
/// implementation behind this command, and listing them would offer a match that fails.
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
pub fn available_games() -> Vec<ArenaGame> {
    vec![ArenaGame {
        id: "tic_tac_toe".to_owned(),
        name: "Tic-Tac-Toe".to_owned(),
        description:
            "Three by three, first side first. Exhaustively solvable, so the ceiling is a \
                      true ceiling rather than a good heuristic."
                .to_owned(),
        width: 3,
        height: 3,
        max_plies: 9,
    }]
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
    /// Which side the tested policy played.
    pub tested_side: GameSide,
    /// The seed the opening was derived from, so the match replays.
    pub seed: u64,
    /// Plies actually played.
    pub plies: u32,
    /// The final board, as the engine recorded it.
    pub final_board: GameBoard,
    /// The side that won, when there is one.
    pub winner: Option<GameSide>,
    /// The three cells the winner completed a line on, when the game has lines.
    ///
    /// Produced by the engine's own winning-line rule rather than by the panel. The renderer used to
    /// have to be told nothing about a result and inferred everything from the final board, which
    /// meant a highlight drawn in the UI was a second, unaudited opinion about who won what.
    pub winning_line: Option<[u8; 3]>,
    /// Illegal moves the tested side returned.
    pub illegal_moves: Vec<IllegalMoveView>,
    /// The recorded ply history, in order.
    pub history: Vec<PlyView>,
    /// The match's own event stream.
    pub timeline: Vec<LabEvent>,
    /// Why the match ended, when it failed.
    pub failure: Option<String>,
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
                .map(|line| {
                    line.iter()
                        .map(|cell| cell.to_string())
                        .collect::<Vec<_>>()
                        .join(",")
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

/// Captures the telemetry the engine records during a match, in memory, and tees it onward.
///
/// The engine already writes every ply to a recorder; this holds a copy so the panel can project a
/// board history, and forwards the identical event to the durable sink when one is installed. Doing
/// both in one `record` call is the point: if forwarding were a separate step it could be skipped on
/// an early return, and the stored record and the panel would then disagree about the same match.
#[derive(Clone, Default)]
struct MatchCapture {
    plies: Arc<Mutex<Vec<GameTelemetry>>>,
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

    /// The timeline recorded during a match.
    fn capture_timeline(&self, capture: &MatchCapture) -> Result<LabTimeline, String> {
        capture
            .timeline
            .lock()
            .map(|timeline| timeline.clone())
            .map_err(|_| CAPTURE_LOCK_POISONED.to_owned())
    }

    /// Plays one match and projects it.
    ///
    /// Takes both seats as registry identifiers. A caller cannot hand the arena a policy that is not
    /// registered, which is what keeps the panel's agent list and the arena's capabilities the same
    /// set.
    pub fn play(
        &self,
        spec: &ArenaMatchSpec,
        sink: Option<Arc<dyn TelemetryRecorder>>,
    ) -> Result<ArenaMatch, ArenaRunError> {
        // The game is checked first, on purpose. A user who asks for connect four should be told this
        // build cannot play it — not that their agent is incompatible with it, which would send them
        // off to fix the wrong thing.
        if !matches!(spec.game, GameKind::TicTacToe) {
            return Err(ArenaRunError::UnknownGame(format!("{:?}", spec.game)));
        }

        // Compatibility is checked before anything runs, so an unsupported pairing is a refusal rather
        // than a match that fails on the first ply. Doing it early also means the error names the
        // actual problem instead of surfacing as a confusing engine failure.
        self.require_compatible(&spec.tested_agent, spec.game)?;
        self.require_compatible(&spec.opponent_agent, spec.game)?;

        let mut agent = self
            .agents
            .build(&spec.tested_agent, spec.seed)
            .map_err(|_| ArenaRunError::UnknownAgent(spec.tested_agent.clone()))?;
        let mut reference = self
            .agents
            .build_reference(&spec.opponent_agent, spec.seed)
            .map_err(|_| ArenaRunError::UnknownAgent(spec.opponent_agent.clone()))?;

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
        let played = match spec.game {
            GameKind::TicTacToe => {
                let tic_tac_toe = probelab_engine::arena::tictactoe::TicTacToe::default_game();
                play_match(
                    &tic_tac_toe,
                    agent.as_mut(),
                    reference.as_mut(),
                    spec.tested_side,
                    &config,
                    &capture,
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
                let final_board = plies
                    .last()
                    .map(|ply| ply.board.clone())
                    .unwrap_or_else(empty_tictactoe_board);
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
                        game_name: "Tic-Tac-Toe".to_owned(),
                        outcome: match result.outcome {
                            MatchOutcome::Win => ArenaOutcome::Win,
                            MatchOutcome::Draw => ArenaOutcome::Draw,
                            MatchOutcome::Loss => ArenaOutcome::Loss,
                        },
                        tested_agent: spec.tested_agent.clone(),
                        opponent_agent: spec.opponent_agent.clone(),
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
                        timeline: timeline.events().to_vec(),
                        failure: None,
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
                    _ => Vec::new(),
                };
                Ok(ArenaMatch {
                    view: ArenaMatchView {
                        match_id: spec.match_id.clone(),
                        game: spec.game.as_str().to_owned(),
                        game_name: "Tic-Tac-Toe".to_owned(),
                        outcome: ArenaOutcome::NoResult,
                        tested_agent: spec.tested_agent.clone(),
                        opponent_agent: spec.opponent_agent.clone(),
                        tested_side: spec.tested_side,
                        seed: spec.seed,
                        plies: u32::try_from(plies.len()).unwrap_or(u32::MAX),
                        final_board: plies
                            .last()
                            .map(|ply| ply.board.clone())
                            .unwrap_or(GameBoard::TicTacToe { cells: vec![] }),
                        winner: None,
                        // A match that failed has no winner, so it has no winning line either.
                        winning_line: None,
                        illegal_moves: illegal,
                        history: plies.iter().map(PlyView::from).collect(),
                        timeline: timeline.events().to_vec(),
                        failure: Some(error.to_string()),
                    },
                    result: None,
                })
            }
        }
    }
}

/// The opening board, for a match that produced no plies at all.
///
/// Nine empty cells rather than an empty vector, because a zero-length board would render as nothing
/// and look like a missing result rather than a real one. This is reached only when a match failed on
/// its very first ply.
fn empty_tictactoe_board() -> GameBoard {
    GameBoard::TicTacToe {
        cells: vec![probelab_core::TicTacToeCell::Empty; 9],
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
) -> Option<[u8; 3]> {
    if outcome == MatchOutcome::Draw {
        return None;
    }
    let GameBoard::TicTacToe { cells } = board else {
        return None;
    };
    if cells.len() != 9 {
        return None;
    }
    let mut fixed: [probelab_core::TicTacToeCell; 9] = [probelab_core::TicTacToeCell::Empty; 9];
    for (slot, cell) in fixed.iter_mut().zip(cells.iter()) {
        *slot = *cell;
    }
    let winner = match outcome {
        MatchOutcome::Win => tested_side,
        MatchOutcome::Loss => tested_side.other(),
        MatchOutcome::Draw => return None,
    };
    probelab_engine::arena::tictactoe::winning_line(&fixed, winner)
        .map(|line| line.map(|index| index as u8))
}
impl Default for ArenaRunner {
    fn default() -> Self {
        Self::new()
    }
}
