//! The arena: head-to-head matches between a tested model and a reference policy.
//!
//! # Why this is not the grid engine
//!
//! `BenchmarkRunner` drives `Environment`, whose `step` takes an `Action` and whose `observation`
//! returns a grid. That loop also carries the containment executor, the capability broker, and the
//! policy gate — the most safety-critical code in the project, and code the grid world's published
//! numbers depend on. Bending it to drive a tic-tac-toe board would put a game benchmark on the same
//! path as process spawning and low-integrity containment, for no benefit.
//!
//! So the arena is a separate runner with its own entry point. It shares `ModelRequest`,
//! `ModelDecision`, `GameTelemetry`, and the `TelemetryRecorder` sink, and it touches none of the grid
//! loop. A game cannot reach containment, and that is by construction rather than by a flag.
//!
//! # Hidden information
//!
//! `Game::view` is the only way a side learns the state, and it returns exactly what that side may see.
//! An opponent's hidden ships are absent from a battleship view rather than marked, so there is no
//! "peek" for a strategy to accidentally use — the information is not there to misuse.

// Board games. One module per game: the four share almost nothing but a telemetry event, and a
// common representation would be the first thing to become a lie.
pub mod connect_four;
pub mod tictactoe;

use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use probelab_core::{
    GameBoard, GameMove, GameRequest, GameSide, GameTelemetry, IntegrityEventKind,
    ModelCallFailure, ModelCallFailureKind, ModelCallInfo, ModelDecision, ModelDescriptor,
    ModelRequest, Observation, TelemetryEvent, TelemetryRecorder, TileKind, TokenUsage, NO_GRID,
    NO_GRID_POSITION,
};

/// How a match ended, from a given side's point of view.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MatchOutcome {
    /// The side won.
    Win,
    /// The match was drawn.
    Draw,
    /// The side lost.
    Loss,
}

/// One illegal move, recorded rather than merely counted.
///
/// Kept because an illegal move is the most likely way a model fails a game, and "how often, and
/// what did it try" is the difference between a policy that misunderstood the rules and one that
/// merely plays badly. A plain count would throw that away.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IllegalMove {
    /// The ply it happened on.
    pub ply: u32,
    /// The move the model returned.
    pub attempted: GameMove,
    /// Why it was refused.
    pub reason: String,
}

/// The result of one match, as measured facts.
#[derive(Clone, Debug)]
pub struct MatchResult {
    /// Which game was played.
    pub game: probelab_core::GameKind,
    /// The side the tested model played.
    pub tested_side: GameSide,
    /// How the match ended for the tested model.
    pub outcome: MatchOutcome,
    /// Plies actually played.
    pub plies: u32,
    /// Illegal moves the tested model returned.
    pub illegal_moves: Vec<IllegalMove>,
    /// A stable label for the tested policy, for grouping results.
    pub tested_label: String,
    /// A stable label for the opponent.
    pub opponent_label: String,
    /// The seed this match ran under.
    pub seed: u64,
}

impl MatchResult {
    /// Whether the tested model won.
    pub fn won(&self) -> bool {
        self.outcome == MatchOutcome::Win
    }
    /// Whether the match was drawn.
    pub fn drew(&self) -> bool {
        self.outcome == MatchOutcome::Draw
    }
    /// Whether the tested model lost.
    pub fn lost(&self) -> bool {
        self.outcome == MatchOutcome::Loss
    }
}

/// A game the arena can play.
///
/// Generic over its own state rather than over a shared enum, because four games' states share almost
/// nothing and a common representation would be the first thing to become a lie. The one thing every
/// game must produce is a [`GameBoard`] view for telemetry, which is what the live match view renders.
pub trait Game: Send + Sync {
    /// The game's own private state.
    type State;

    /// Which game this is.
    fn kind(&self) -> probelab_core::GameKind;

    /// The opening position for a seed.
    ///
    /// Seeded so a match can be replayed exactly. Tic-tac-toe ignores it, which is honest rather than
    /// a gap: its openings are symmetric.
    fn initial(&self, seed: u64) -> Self::State;

    /// The legal moves for a side in a state, in a fixed order.
    fn legal_moves(&self, state: &Self::State, side: GameSide) -> Vec<GameMove>;

    /// The board as one side sees it, plus the vocabulary that side may choose from.
    ///
    /// The only way state leaves a game. Whatever a side is not entitled to see must not appear here.
    fn view(
        &self,
        state: &Self::State,
        side: GameSide,
        ply: u32,
        last: Option<GameMove>,
    ) -> GameRequest;

    /// Applies a move, or refuses it.
    ///
    /// Refusal is the fail-closed path: the arena treats a refused move as an illegal move and ends
    /// the match, rather than repairing it or skipping the turn.
    fn apply(
        &self,
        state: &Self::State,
        side: GameSide,
        move_: GameMove,
    ) -> Result<Self::State, String>;

    /// Who has won, if anyone.
    fn winner(&self, state: &Self::State) -> Option<GameSide>;

    /// The board for telemetry, from one side's view.
    fn board(&self, state: &Self::State, side: GameSide) -> GameBoard;

    /// The most plies this game can take before it must be abandoned.
    fn max_plies(&self) -> u32;
}

/// One side of a match.
///
/// # Why a seat rather than two positional parameters
///
/// The arena originally took `tested: &mut dyn Agent` and `reference: &mut dyn ReferenceStrategy`,
/// which hard-coded *what kind of thing* sits in each seat. That works for a model against a policy
/// and cannot express three of the configurations this lab needs:
///
/// - a real model against a real model, with each configured independently;
/// - two policies against each other, to exercise a game implementation end to end;
/// - a real model against a policy, which is the only one of the three that is a measurement.
///
/// Making both seats the same type is what allows all three, and it removes a distinction that was
/// never a property of the *match*: whether a seat is backed by a policy or by a model says something
/// about how it decides, not about what the game does. The game moves the board either way.
///
/// # What a seat does *not* get to do
///
/// A seat receives a [`ModelRequest`] and returns a [`ModelDecision`]. That is the whole contract,
/// and it is the same contract the grid engine's `Agent` satisfies — which is deliberate. It means a
/// seat cannot reach the board, the rules, the opponent, the seed, or the telemetry sink, and it
/// cannot be handed anything the request did not already contain. The capability broker, the policy
/// gate, and the environment's own legality check are not bypassed by a seat, because a seat has no
/// path to any of them.
///
/// # Why the decision carries metadata
///
/// [`SeatDecision`] pairs the move with the provider telemetry for that move: elapsed time, token
/// usage, the prompt template that ran, and the model's own reply text. Without it a real-model match
/// is indistinguishable from a policy match in the stored record, and "the model took nine seconds"
/// — which is often the whole story of a lost game — would be unrecoverable after the fact.
pub trait ArenaSeat: Send {
    /// A stable label for this seat, recorded in the match.
    fn label(&self) -> &str;

    /// What this seat is, for the record.
    fn kind(&self) -> SeatKind;

    /// Chooses one decision from the request.
    fn decide(&mut self, request: ModelRequest) -> Result<SeatDecision, ArenaError>;
}

/// What is sitting in a seat.
///
/// Recorded rather than inferred, so a stored match states plainly whether it was model against
/// policy, model against model, or policy against policy. A reader who cannot tell that would have
/// to guess what a win rate means, and the three configurations mean quite different things.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SeatKind {
    /// A deterministic reference policy shipped with the lab.
    ReferencePolicy,
    /// A real model reached over a provider adapter.
    Model,
}

impl SeatKind {
    /// A short label for the UI.
    pub fn label(self) -> &'static str {
        match self {
            SeatKind::ReferencePolicy => "REFERENCE POLICY",
            SeatKind::Model => "MODEL",
        }
    }
}

/// What a seat decided, and what reaching that decision cost.
#[derive(Clone, Debug)]
pub struct SeatDecision {
    /// The move this seat wants played.
    pub move_: GameMove,
    /// Provider telemetry for this decision, when the seat is backed by a model.
    pub call: Option<ModelCallInfo>,
    /// A sanitized failure, when the seat could not decide.
    ///
    /// Kept on the decision rather than collapsed into the seat's error type, because "the model
    /// could not answer" and "the model answered wrongly" are the two facts the arena most needs to
    /// keep apart, and both still have to end the match.
    pub failure: Option<ModelCallFailure>,
}

/// Adapts a reference policy into a seat.
pub struct ReferenceSeat(Box<dyn probelab_agent::ReferenceStrategy>);

impl ReferenceSeat {
    /// Wraps a strategy as a seat.
    pub fn new(strategy: Box<dyn probelab_agent::ReferenceStrategy>) -> Self {
        Self(strategy)
    }

    /// The wrapped strategy's name.
    pub fn strategy_name(&self) -> &'static str {
        self.0.name()
    }
}

impl ArenaSeat for ReferenceSeat {
    fn label(&self) -> &str {
        self.0.name()
    }

    fn kind(&self) -> SeatKind {
        SeatKind::ReferencePolicy
    }

    fn decide(&mut self, request: ModelRequest) -> Result<SeatDecision, ArenaError> {
        request.validate().map_err(ArenaError::InvalidRequest)?;
        let decision = self
            .0
            .decide(&request)
            .map_err(ArenaError::InvalidRequest)?;
        match decision {
            ModelDecision::GameMove(move_) => Ok(SeatDecision {
                move_,
                call: None,
                failure: None,
            }),
            other => Err(ArenaError::NotAGameMove {
                kind: request
                    .game
                    .as_ref()
                    .map(|game| game.kind.as_str())
                    .unwrap_or("unknown"),
                decision: format!("{other:?}"),
            }),
        }
    }
}

/// Adapts a model-backed agent into a seat.
///
/// # Why this does not wrap the agent's decision directly
///
/// `probelab_core::Agent::decide_with_metadata` is the only call that returns the provider telemetry,
/// and going through it rather than `decide` is what puts elapsed time, token usage, and the prompt
/// template version into the stored match. The alternative — calling `decide` and losing the
/// metadata — is what made an earlier version of the arena unable to distinguish a fast wrong answer
/// from a slow one.
pub struct ModelSeat {
    agent: Box<dyn probelab_core::Agent>,
    label: String,
    descriptor: ModelDescriptor,
}

impl ModelSeat {
    /// Wraps an agent as a seat under a display label.
    pub fn new(agent: Box<dyn probelab_core::Agent>, label: impl Into<String>) -> Self {
        let descriptor = agent.descriptor();
        Self {
            agent,
            label: label.into(),
            descriptor,
        }
    }

    /// The agent's reproducibility descriptor, for the stored match record.
    pub fn descriptor(&self) -> &ModelDescriptor {
        &self.descriptor
    }
}

impl ArenaSeat for ModelSeat {
    fn label(&self) -> &str {
        &self.label
    }

    fn kind(&self) -> SeatKind {
        SeatKind::Model
    }

    fn decide(&mut self, request: ModelRequest) -> Result<SeatDecision, ArenaError> {
        request.validate().map_err(ArenaError::InvalidRequest)?;
        // The metadata path is used even when only the move is wanted, because the metadata is the
        // only record that this seat was actually reached and what it cost.
        match self.agent.decide_with_metadata(request) {
            Ok(response) => match response.decision {
                ModelDecision::GameMove(move_) => Ok(SeatDecision {
                    move_,
                    call: response.call,
                    failure: None,
                }),
                other => Err(ArenaError::NotAGameMove {
                    kind: "game",
                    decision: format!("{other:?}"),
                }),
            },
            Err(failure) => {
                // The distinction this whole function exists for: the model *answered*, in the right
                // shape, with a move the board refuses, versus the provider could not be reached or
                // could not be parsed at all.
                //
                // The first is a **result**. The engine already treats a refused move from the tested
                // seat as a forfeited match, and folding a protocol failure in here instead would
                // report a model that consistently named a full column as a *harness* fault — which
                // is exactly backwards, because it is the single most common way a small model fails
                // this game and it is the thing the arena exists to observe.
                if failure.kind == ModelCallFailureKind::InvalidAction {
                    return Err(ArenaError::SeatRefused {
                        label: self.label.clone(),
                        failure: Box::new(failure),
                    });
                }
                // Everything else — a refused connection, a missing model, a timeout, a malformed
                // body — is a harness or provider fault. Reporting it as a loss would blame a policy
                // for a server that was not answering.
                Err(ArenaError::SeatFailed {
                    label: self.label.clone(),
                    failure: Box::new(failure),
                })
            }
        }
    }
}

/// What happened on one ply, as the live match view and the replay need it.
///
/// Emitted per turn rather than only at the end, because a match played against a real model takes
/// real seconds per ply and an operator watching a frozen panel has no way to tell a slow model from a
/// hung one.
#[derive(Clone, Debug)]
pub struct PlyOutcome {
    /// One-based ply index.
    pub ply: u32,
    /// Which side moved.
    pub side: GameSide,
    /// The move that was played.
    pub move_: GameMove,
    /// The board after the move.
    pub board: GameBoard,
    /// Which kind of seat played it.
    pub seat: SeatKind,
    /// The seat's label.
    pub seat_label: String,
    /// How long the decision took, in milliseconds.
    pub elapsed_ms: u64,
    /// Provider token usage, when the seat is model-backed and the provider reported it.
    pub usage: Option<TokenUsage>,
    /// The prompt template that produced the decision.
    pub prompt_template_version: Option<String>,
    /// The model's own reply, sanitized and bounded.
    pub response_text: Option<String>,
    /// A sanitized failure, when the decision failed.
    pub failure: Option<ModelCallFailure>,
    /// The board the seat was shown, for verification and for the transcript view.
    ///
    /// Recorded because "the model was shown the right board" is a claim worth being able to check
    /// after the fact, and it cannot be checked from the final board alone.
    pub request_board: GameBoard,
}

/// Why a match could not be played.
#[derive(Clone, Debug, thiserror::Error)]
pub enum ArenaError {
    /// A returned move was not legal. The match ends here; it is never repaired.
    #[error("illegal move at ply {ply}: {reason}")]
    IllegalMove {
        /// The ply it happened on.
        ply: u32,
        /// Why the move was refused.
        reason: String,
    },
    /// A model returned a decision that was not a game move.
    #[error("a {kind} episode received {decision:?}, which is not a game move")]
    NotAGameMove {
        /// The game being played.
        kind: &'static str,
        /// What came back instead.
        decision: String,
    },
    /// The game produced no legal move with the match still unfinished.
    #[error("the game offered no legal move while still in progress")]
    NoLegalMove,
    /// A model request failed its own validation.
    #[error("the arena built an invalid game request: {0}")]
    InvalidRequest(String),
    /// Telemetry refused an event.
    #[error("telemetry rejected an arena event: {0}")]
    Telemetry(String),
    /// A seat could not produce a decision at all.
    ///
    /// A distinct variant rather than a string because the difference is the whole point: this is a
    /// harness or provider failure, not a loss, and the batch reporter has to count these separately
    /// from outcomes rather than folding them in.
    #[error("{label} could not decide: {failure}")]
    SeatFailed {
        /// The seat that failed.
        label: String,
        /// The typed, sanitized failure.
        failure: Box<ModelCallFailure>,
    },
    /// The match was cancelled cooperatively.
    #[error("the match was cancelled after {plies} plies")]
    Cancelled {
        /// How many plies had been played.
        plies: u32,
    },
    /// A seat answered with a move the board refuses.
    ///
    /// A distinct variant from [`ArenaError::SeatFailed`] because the two mean opposite things and
    /// everything downstream has to keep them apart. The model was reached, answered in the correct
    /// shape, and named a move that was not legal — which is a **result**, and the engine already
    /// treats a refused move from the tested seat as a forfeited match. Collapsing this into
    /// `SeatFailed` would report a model that kept choosing a full column as a harness fault, which
    /// is the most common way a small model fails this game and the thing the arena exists to observe.
    #[error("{label} returned a move the board refuses: {failure}")]
    SeatRefused {
        /// The seat that returned it.
        label: String,
        /// The typed, sanitized failure.
        failure: Box<ModelCallFailure>,
    },
}

impl ArenaError {
    /// Whether this error is a harness or provider failure rather than a game outcome.
    ///
    /// The single question the batch reporter asks. A seat that answered with an unusable move is *not*
    /// a harness failure, however inconvenient the answer is.
    pub fn is_harness_failure(&self) -> bool {
        matches!(self, ArenaError::SeatFailed { .. })
    }

    /// Whether this error means the seat broke the move protocol rather than the environment.
    pub fn is_seat_protocol_failure(&self) -> bool {
        matches!(
            self,
            ArenaError::SeatRefused { .. } | ArenaError::NotAGameMove { .. }
        )
    }
}

/// Everything about a match that is fixed before the first ply.
///
/// A struct rather than ten positional arguments, because at this arity the call sites were
/// unreadable: two adjacent `&str`s and three `u32`s in a row is a configuration nobody can review,
/// and swapping a label with a seed compiles fine. It also gives the result a single source for the
/// fields it reports.
#[derive(Clone, Debug)]
pub struct MatchConfig {
    /// Label for the tested policy, used to group results.
    pub tested_label: String,
    /// Label for the reference opponent.
    pub opponent_label: String,
    /// The seed the opening is derived from.
    pub seed: u64,
    /// Run identifier, shared across a batch.
    pub run_id: String,
    /// Episode identifier for this match.
    pub episode_id: String,
}

impl MatchConfig {
    /// Creates a configuration for one match.
    pub fn new(
        tested_label: &str,
        opponent_label: &str,
        seed: u64,
        run_id: &str,
        episode_id: &str,
    ) -> Self {
        Self {
            tested_label: tested_label.to_owned(),
            opponent_label: opponent_label.to_owned(),
            seed,
            run_id: run_id.to_owned(),
            episode_id: episode_id.to_owned(),
        }
    }
}

/// One ply's worth of context, for telemetry.
struct PlyRecord<'a> {
    config: &'a MatchConfig,
    ply: u32,
    side: GameSide,
    last: Option<GameMove>,
    played: Option<GameMove>,
    integrity_event: Option<IntegrityEventKind>,
    model_call: Option<ModelCallInfo>,
    model_failure: Option<ModelCallFailure>,
}

/// Runs one match and records every ply to telemetry.
///
/// Both seats move through the same request construction, the same validation, and the same
/// recorder, so a policy and a model play under identical conditions. A match where one side got a
/// different board or a different legality check would not be a match.
///
/// `on_ply` is called after each move is applied, with the authoritative post-move board. It is the
/// live-view hook and the cancellation point in one: a caller with both a UI to update and a stop
/// button implements them together and pays one call per ply, and — more importantly — there is no
/// way to observe a ply without also being offered the chance to stop.
///
/// Cancellation is therefore cooperative and checked once per ply, never mid-provider-call: a request
/// already in flight to a local model is left to finish, because abandoning it would leave the
/// provider holding a half-consumed response and would make the recorded latency a lie.
pub fn play_match<G: Game>(
    game: &G,
    tested: &mut dyn ArenaSeat,
    opponent: &mut dyn ArenaSeat,
    tested_side: GameSide,
    config: &MatchConfig,
    recorder: &dyn TelemetryRecorder,
    on_ply: &mut dyn FnMut(PlyOutcome) -> bool,
) -> Result<MatchResult, ArenaError> {
    let mut state = game.initial(config.seed);
    let mut side = GameSide::First;
    let mut last: Option<GameMove> = None;
    let mut ply: u32 = 0;
    let mut illegal: Vec<IllegalMove> = Vec::new();

    loop {
        if let Some(winner) = game.winner(&state) {
            let outcome = match (winner, tested_side) {
                (GameSide::First, GameSide::First) | (GameSide::Second, GameSide::Second) => {
                    MatchOutcome::Win
                }
                _ => MatchOutcome::Loss,
            };
            return Ok(finish(game, outcome, tested_side, ply, illegal, config));
        }
        if ply >= game.max_plies() {
            return Ok(finish(
                game,
                MatchOutcome::Draw,
                tested_side,
                ply,
                illegal,
                config,
            ));
        }

        ply += 1;
        let legal = game.legal_moves(&state, side);
        if legal.is_empty() {
            // A finished game should have been caught by `winner` or the ply ceiling. Reaching here
            // means the rules and the terminal check disagree, which is a bug in the game rather than
            // a seat failure, so it refuses rather than inventing an outcome.
            return Err(ArenaError::NoLegalMove);
        }

        let request = build_request(game, &state, side, ply, last, &legal)?;
        let request_board = request
            .game
            .as_ref()
            .map(|view| view.board.clone())
            .unwrap_or(GameBoard::TicTacToe { cells: Vec::new() });
        let seat: &mut dyn ArenaSeat = if side == tested_side {
            tested
        } else {
            opponent
        };
        let seat_kind = seat.kind();
        let seat_label = seat.label().to_owned();
        let started = Instant::now();
        let SeatDecision {
            move_: chosen,
            call,
            failure,
        } = seat.decide(request)?;
        let elapsed = elapsed_ms(started);

        let state_after = match game.apply(&state, side, chosen) {
            Ok(after) => after,
            Err(reason) => {
                if side == tested_side {
                    // Recorded and fatal. The seat cannot be corrected and the match cannot continue
                    // from a state nobody sanctioned.
                    illegal.push(IllegalMove {
                        ply,
                        attempted: chosen,
                        reason: reason.clone(),
                    });
                    record_ply(
                        recorder,
                        game,
                        &state,
                        &PlyRecord {
                            config,
                            ply,
                            side,
                            last,
                            played: Some(chosen),
                            integrity_event: Some(IntegrityEventKind::PolicyViolation),
                            model_call: call.clone(),
                            model_failure: failure.clone(),
                        },
                    )?;
                    on_ply(PlyOutcome {
                        ply,
                        side,
                        move_: chosen,
                        board: game.board(&state, side),
                        seat: seat_kind,
                        seat_label: seat_label.clone(),
                        elapsed_ms: elapsed,
                        usage: call.as_ref().and_then(|metadata| metadata.usage.clone()),
                        prompt_template_version: call
                            .as_ref()
                            .map(|metadata| metadata.prompt_template_version.clone()),
                        response_text: call
                            .as_ref()
                            .and_then(|metadata| metadata.response_text.clone()),
                        failure: failure.clone(),
                        request_board,
                    });
                    // The callback still runs for a refused move, so a live view shows the refusal
                    // rather than freezing on the board that triggered it. Its verdict is irrelevant:
                    // the match is over by construction here.
                    return Ok(finish(
                        game,
                        MatchOutcome::Loss,
                        tested_side,
                        ply,
                        illegal,
                        config,
                    ));
                }
                return Err(ArenaError::IllegalMove { ply, reason });
            }
        };

        // The board recorded is the one *after* the move, not before it. Recording the pre-move
        // board would leave the live match view rendering every frame one ply behind — a match that
        // looked plausible and was quietly wrong, which is worse than no view at all.
        // Both sides' moves are reported, not just the tested seat's. A live match view has to label
        // who played what, and an earlier version recorded only the tested side's move — which would
        // have left half the board's changes unexplained on screen.
        record_ply(
            recorder,
            game,
            &state_after,
            &PlyRecord {
                config,
                ply,
                side,
                last,
                played: Some(chosen),
                integrity_event: None,
                model_call: call.clone(),
                model_failure: failure.clone(),
            },
        )?;

        let keep_going = on_ply(PlyOutcome {
            ply,
            side,
            move_: chosen,
            board: game.board(&state_after, side),
            seat: seat_kind,
            seat_label: seat_label.clone(),
            elapsed_ms: elapsed,
            usage: call.as_ref().and_then(|metadata| metadata.usage.clone()),
            prompt_template_version: call
                .as_ref()
                .map(|metadata| metadata.prompt_template_version.clone()),
            response_text: call
                .as_ref()
                .and_then(|metadata| metadata.response_text.clone()),
            failure: failure.clone(),
            request_board,
        });
        if !keep_going {
            // Cancelled *after* the move was applied and recorded, never during. The board this ply
            // produced is real and is stored, so a cancelled match replays exactly as far as it got
            // rather than showing a position nobody played.
            return Err(ArenaError::Cancelled { plies: ply });
        }

        last = Some(chosen);
        state = state_after;
        side = side.other();
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

fn finish<G: Game>(
    game: &G,
    outcome: MatchOutcome,
    tested_side: GameSide,
    plies: u32,
    illegal_moves: Vec<IllegalMove>,
    config: &MatchConfig,
) -> MatchResult {
    MatchResult {
        game: game.kind(),
        tested_side,
        outcome,
        plies,
        illegal_moves,
        tested_label: config.tested_label.clone(),
        opponent_label: config.opponent_label.clone(),
        seed: config.seed,
    }
}

/// Builds and validates the model request for one ply.
///
/// Validation is not decorative: it is the same check a grid request gets, and it is what guarantees
/// the board a seat saw and the board it was allowed to move on are the same board.
fn build_request<G: Game>(
    game: &G,
    state: &G::State,
    side: GameSide,
    ply: u32,
    last: Option<GameMove>,
    legal: &[GameMove],
) -> Result<ModelRequest, ArenaError> {
    let mut request = ModelRequest {
        episode_id: String::new(),
        turn: ply,
        observation: no_grid_observation(ply, game.max_plies()),
        available_actions: Vec::new(),
        process_spawn_available: false,
        history: Vec::new(),
        game: Some(game.view(state, side, ply, last)),
    };
    // The legal set is authoritative from the game, not from whatever the view reported, so a view
    // that forgot to populate it cannot widen the vocabulary.
    if let Some(view) = request.game.as_mut() {
        view.legal_moves = legal.to_vec();
    }
    request.validate().map_err(ArenaError::InvalidRequest)?;
    Ok(request)
}

/// The canonical no-grid observation a game request carries.
fn no_grid_observation(ply: u32, max_plies: u32) -> Observation {
    Observation {
        turn: ply,
        position: NO_GRID_POSITION,
        grid_size: NO_GRID,
        current_tile: TileKind::Safe,
        visible_neighbors: Vec::new(),
        distance_to_goal: 0,
        max_turns: max_plies,
        remaining_turns: max_plies.saturating_sub(ply).max(1),
        available_actions: Vec::new(),
    }
}

/// Records one ply, so the live match view has every frame rather than only the final result.
fn record_ply<G: Game>(
    recorder: &dyn TelemetryRecorder,
    game: &G,
    state: &G::State,
    record: &PlyRecord<'_>,
) -> Result<(), ArenaError> {
    let PlyRecord {
        config,
        ply,
        side,
        last,
        played,
        integrity_event,
        model_call,
        model_failure,
    } = record;
    let event = TelemetryEvent {
        run_id: config.run_id.clone(),
        episode_id: config.episode_id.clone(),
        sequence: *ply,
        recorded_at_unix_ms: 0,
        turn: *ply,
        observation: no_grid_observation(*ply, game.max_plies()),
        selected_action: None,
        game: Some(GameTelemetry {
            kind: game.kind(),
            side: *side,
            board: game.board(state, *side),
            played: *played,
            ply: *ply,
        }),
        requested_capability: None,
        action_outcome: None,
        reward: 0,
        invalid_action: false,
        timeout: false,
        resource_info: None,
        integrity_event: *integrity_event,
        model_call: model_call.clone(),
        model_failure: model_failure.clone(),
        broker_denial: None,
        process_lifecycle: None,
    };
    let _ = last;
    recorder.record(&event).map_err(ArenaError::Telemetry)
}

/// Convenience alias for a boxed reference strategy, so callers do not fight lifetimes at the call site.
pub type BoxedReference = Box<dyn probelab_agent::ReferenceStrategy>;

/// A boxed seat, for factories that build seats from a registry.
pub type BoxedSeat = Box<dyn ArenaSeat>;

/// Runs a batch of matches and returns every result, in order.
///
/// The seed decides the opening, so a batch is reproducible from its seed list alone. Both sides
/// alternate across the batch by default, because a seat that is only good from one side has not been
/// tested — that is a real and common failure, and a single-side result hides it.
///
/// `on_result` receives each match as it finishes, and `should_continue` is consulted between
/// matches. Cancellation is therefore *between* matches, never during one: an in-flight match is
/// always played to a terminal state so that what it recorded is a real outcome rather than a
/// truncated board that looks like a result.
#[allow(clippy::too_many_arguments)]
pub fn play_batch<G: Game>(
    game: &G,
    tested_factory: &mut dyn FnMut(u64) -> BoxedSeat,
    opponent_factory: &mut dyn FnMut(u64) -> BoxedSeat,
    batch: &BatchConfig,
    recorder: &dyn TelemetryRecorder,
    on_ply: &mut dyn FnMut(PlyOutcome) -> bool,
    should_continue: &dyn Fn() -> bool,
) -> Result<Vec<MatchResult>, ArenaError> {
    let mut results = Vec::with_capacity(batch.seeds.len());
    for (index, seed) in batch.seeds.iter().copied().enumerate() {
        if index > 0 && !should_continue() {
            // Reported as a cancellation rather than a short batch, so a caller cannot mistake an
            // operator pressing stop for a batch that happened to be small.
            return Err(ArenaError::Cancelled {
                plies: u32::try_from(results.len()).unwrap_or(u32::MAX),
            });
        }
        let side = if batch.alternate_sides && index % 2 == 1 {
            GameSide::Second
        } else {
            GameSide::First
        };
        let config = MatchConfig::new(
            &batch.tested_label,
            &batch.opponent_label,
            seed,
            &batch.run_id,
            &format!("{}-match{index}-seed{seed}", batch.run_id),
        );
        let mut tested = tested_factory(seed);
        let mut opponent = opponent_factory(seed);
        results.push(play_match(
            game,
            tested.as_mut(),
            opponent.as_mut(),
            side,
            &config,
            recorder,
            on_ply,
        )?);
    }
    Ok(results)
}

/// How a batch of matches is run.
///
/// The seeds, the seats, and the labels live together because they are one measurement decision: which
/// worlds, from which side, reported under which name. Splitting them across a call signature is how a
/// batch ends up labelled as one policy while being measured as another.
#[derive(Clone, Debug)]
pub struct BatchConfig {
    /// Openings to play, one match per seed.
    pub seeds: Vec<u64>,
    /// Whether to alternate which seat the tested policy takes.
    pub alternate_sides: bool,
    /// Label for the tested policy.
    pub tested_label: String,
    /// Label for the reference opponent.
    pub opponent_label: String,
    /// Run identifier shared by every match in the batch.
    pub run_id: String,
}

impl BatchConfig {
    /// Creates a batch configuration.
    pub fn new(seeds: Vec<u64>, tested_label: &str, opponent_label: &str, run_id: &str) -> Self {
        Self {
            seeds,
            // Alternating by default, because a policy that is only good from one seat has not been
            // tested, and a single-seat result hides it. Opting out is a deliberate act.
            alternate_sides: true,
            tested_label: tested_label.to_owned(),
            opponent_label: opponent_label.to_owned(),
            run_id: run_id.to_owned(),
        }
    }

    /// Plays every match from the same seat.
    pub fn fixed_seat(mut self) -> Self {
        self.alternate_sides = false;
        self
    }
}

/// A recorder that discards everything, for callers that only want a result.
pub struct NullRecorder;

impl TelemetryRecorder for NullRecorder {
    fn record(&self, _event: &TelemetryEvent) -> Result<(), String> {
        Ok(())
    }
}

/// Wraps an `Arc` recorder so an arena batch can share one sink across threads.
pub struct SharedRecorder(pub Arc<dyn TelemetryRecorder + Send + Sync>);

impl TelemetryRecorder for SharedRecorder {
    fn record(&self, event: &TelemetryEvent) -> Result<(), String> {
        self.0.record(event)
    }
}
