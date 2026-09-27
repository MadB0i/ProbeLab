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
pub mod tictactoe;

use std::sync::Arc;

use probelab_core::{
    Agent, GameBoard, GameMove, GameRequest, GameSide, GameTelemetry, IntegrityEventKind,
    ModelDecision, ModelRequest, Observation, TelemetryEvent, TelemetryRecorder, TileKind, NO_GRID,
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
}

/// Runs one match and records every ply to telemetry.
///
/// Both sides move through the same request construction, the same validation, and the same
/// recorder, so a reference policy and a model are playing under identical conditions. A match where
/// the opponent got a different board or a different legality check would not be a match.
pub fn play_match<G: Game>(
    game: &G,
    tested: &mut dyn Agent,
    reference: &mut dyn probelab_agent::ReferenceStrategy,
    tested_side: GameSide,
    config: &MatchConfig,
    recorder: &dyn TelemetryRecorder,
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
            // a model failure, so it refuses rather than inventing an outcome.
            return Err(ArenaError::NoLegalMove);
        }

        let request = build_request(game, &state, side, ply, last, &legal)?;
        // The chosen move is all both paths need; the decision itself is only used to build the
        // request, and reporting it per side would duplicate information the board already carries.
        let chosen = if side == tested_side {
            let decision = tested.decide(request).map_err(ArenaError::InvalidRequest)?;
            let chosen = match decision {
                ModelDecision::GameMove(found) => found,
                other => {
                    return Err(ArenaError::NotAGameMove {
                        kind: game.kind().as_str(),
                        decision: format!("{other:?}"),
                    })
                }
            };
            chosen
        } else {
            let decision = reference
                .decide(&request)
                .map_err(ArenaError::InvalidRequest)?;
            let ModelDecision::GameMove(chosen) = decision else {
                return Err(ArenaError::NotAGameMove {
                    kind: game.kind().as_str(),
                    decision: format!("{decision:?}"),
                });
            };
            chosen
        };

        let state_after = match game.apply(&state, side, chosen) {
            Ok(after) => after,
            Err(reason) => {
                if side == tested_side {
                    // Recorded and fatal. The model cannot be corrected and the match cannot continue
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
                        },
                    )?;
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
        // Both sides' moves are reported, not just the tested model's. A live match view has to label
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
            },
        )?;

        last = Some(chosen);
        state = state_after;
        side = side.other();
    }
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
/// the board a model saw and the board it was allowed to move on are the same board.
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
    } = *record;
    let event = TelemetryEvent {
        run_id: config.run_id.clone(),
        episode_id: config.episode_id.clone(),
        sequence: ply,
        recorded_at_unix_ms: 0,
        turn: ply,
        observation: no_grid_observation(ply, game.max_plies()),
        selected_action: None,
        game: Some(GameTelemetry {
            kind: game.kind(),
            side,
            board: game.board(state, side),
            played,
            ply,
        }),
        requested_capability: None,
        action_outcome: None,
        reward: 0,
        invalid_action: false,
        timeout: false,
        resource_info: None,
        integrity_event,
        model_call: None,
        model_failure: None,
        broker_denial: None,
        process_lifecycle: None,
    };
    let _ = last;
    recorder.record(&event).map_err(ArenaError::Telemetry)
}

/// Convenience alias for a boxed reference strategy, so callers do not fight lifetimes at the call site.
pub type BoxedReference = Box<dyn probelab_agent::ReferenceStrategy>;

/// Runs a batch of matches and returns every result, in order.
///
/// The seed decides the opening, so a batch is reproducible from its seed list alone. Both sides
/// alternate across the batch by default, because a model that is only good from one seat has not been
/// tested — that is a real and common failure, and a single-seat result hides it.
pub fn play_batch<G: Game>(
    game: &G,
    tested_factory: &mut dyn FnMut(u64) -> Box<dyn Agent>,
    reference_factory: &mut dyn FnMut(u64) -> BoxedReference,
    batch: &BatchConfig,
    recorder: &dyn TelemetryRecorder,
) -> Result<Vec<MatchResult>, ArenaError> {
    let mut results = Vec::with_capacity(batch.seeds.len());
    for (index, seed) in batch.seeds.iter().copied().enumerate() {
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
        let mut reference = reference_factory(seed);
        results.push(play_match(
            game,
            tested.as_mut(),
            reference.as_mut(),
            side,
            &config,
            recorder,
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
