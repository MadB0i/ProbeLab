//! Tic-tac-toe calibration, and the first proof that the arena works end to end.
//!
//! The calibration is the same shape as the grid world's: a policy that must score zero, a property
//! that must hold no matter what, and a reference-versus-reference control that pins the rules
//! themselves. Without the control, "the perfect policy never loses" could mean the policy is good
//! *or* that the arena is broken in a way that stops games from finishing.
//!
//! The headline metric is **loss rate**, per the phase brief. A draw is reported but is explicitly not
//! treated as a success signal: a 3x3 board that both sides draw is measuring tic-tac-toe, not the
//! model. This is a warm-up tier, and the tests say so where a reader would otherwise assume otherwise.

use std::sync::Arc;

use probelab_agent::{ReferenceAgent, ReferenceKnowledge, ReferenceStrategy};
use probelab_core::{
    Action, GameBoard, GameKind, GameMove, GameSide, ModelDecision, ModelProvider, ModelRequest,
    TelemetryEvent, TelemetryRecorder, TicTacToeCell,
};
use probelab_engine::arena::tictactoe::{
    has_won, winning_line, PerfectTicTacToe, TicTacToe, MAX_PLIES,
};
use probelab_engine::arena::{
    play_match, Game, MatchConfig, MatchOutcome, ModelSeat, NullRecorder, PlyOutcome, ReferenceSeat,
};

/// Thirty matches. The rules have no seed dependence, so every match is the same game; the count is
/// what makes a reported rate a rate rather than an anecdote.
const MATCHES: u64 = 30;

fn seeds(count: u64) -> Vec<u64> {
    (0..count).collect()
}

/// A policy that replays a fixed script, so a test can be *deliberately* bad.
struct Scripted {
    moves: Vec<GameMove>,
    index: usize,
}

impl Scripted {
    fn new(moves: Vec<GameMove>) -> Self {
        Self { moves, index: 0 }
    }
}

impl ModelProvider for Scripted {
    fn descriptor(&self) -> probelab_core::ModelDescriptor {
        probelab_core::ModelDescriptor {
            adapter: "arena-test-v1".to_owned(),
            strategy: "scripted".to_owned(),
            seed: 0,
            parameters: Default::default(),
        }
    }

    fn predict(&mut self, _request: ModelRequest) -> Result<ModelDecision, String> {
        if self.moves.is_empty() {
            return Err("the scripted policy has nothing to say".to_owned());
        }
        let index = self.index.min(self.moves.len() - 1);
        self.index += 1;
        Ok(ModelDecision::GameMove(self.moves[index]))
    }
}

fn scripted_agent(moves: Vec<GameMove>) -> Box<dyn probelab_core::Agent> {
    Box::new(probelab_agent::ProviderAgent::from_provider(Scripted::new(
        moves,
    )))
}

/// A policy that always takes the first legal move offered.
///
/// Uninspired, but genuinely legal at every ply, which is what the "no legal policy can lose" test
/// needs. It also demonstrates the point of putting `legal_moves` in the request: a policy needs no
/// knowledge of tic-tac-toe's rules at all to play a legal game, because the rules are handed to it.
struct FirstLegal;

impl ModelProvider for FirstLegal {
    fn descriptor(&self) -> probelab_core::ModelDescriptor {
        probelab_core::ModelDescriptor {
            adapter: "arena-test-v1".to_owned(),
            strategy: "first_legal".to_owned(),
            seed: 0,
            parameters: Default::default(),
        }
    }

    fn predict(&mut self, request: ModelRequest) -> Result<ModelDecision, String> {
        let game = request
            .game
            .as_ref()
            .ok_or_else(|| "first-legal needs a game request".to_owned())?;
        game.legal_moves
            .first()
            .copied()
            .map(ModelDecision::GameMove)
            .ok_or_else(|| "no legal move was offered".to_owned())
    }
}

fn first_legal_agent() -> Box<dyn probelab_core::Agent> {
    Box::new(probelab_agent::ProviderAgent::from_provider(FirstLegal))
}

/// Adapts a reference policy into an `Agent` so it can sit in the tested seat.
///
/// Only the reference-versus-reference control uses this. A measured policy is never built this way,
/// because putting a reference in the tested seat would make the comparison meaningless.
struct ReferenceAsAgent(Box<dyn ReferenceStrategy>);

impl probelab_core::Agent for ReferenceAsAgent {
    fn descriptor(&self) -> probelab_core::ModelDescriptor {
        probelab_core::ModelDescriptor {
            adapter: "arena-reference-v1".to_owned(),
            strategy: self.0.name().to_owned(),
            seed: 0,
            parameters: Default::default(),
        }
    }

    fn decide(&mut self, request: ModelRequest) -> Result<ModelDecision, String> {
        self.0.decide(&request)
    }
}

/// Runs `count` matches and returns every result.
fn run_batch(
    side: GameSide,
    count: u64,
    mut tested: impl FnMut(u64) -> Box<dyn probelab_core::Agent>,
) -> Vec<probelab_engine::arena::MatchResult> {
    let game = TicTacToe::default_game();
    seeds(count)
        .into_iter()
        .map(|seed| {
            let mut agent = ModelSeat::new(tested(seed), "tested");
            let mut opponent = ReferenceSeat::new(Box::new(PerfectTicTacToe::new()));
            play_match(
                &game,
                &mut agent,
                &mut opponent,
                side,
                &MatchConfig::new("	ested", "perfect", seed, "run", "episode"),
                &NullRecorder,
                &mut |_outcome: PlyOutcome| true,
            )
            .expect("a match completes")
        })
        .collect()
}

/// The control: perfect against perfect is a draw, every time, from both seats.
///
/// This is the test the rest of the file leans on. It proves the board, the win detection, and the
/// turn handling agree with each other — if any of them were wrong, a "perfect" policy could win or
/// lose, and every rate measured against this opponent would be measuring the harness instead of the
/// policy.
#[test]
fn ttt_perfect_against_perfect_is_a_draw_from_either_seat() {
    let game = TicTacToe::default_game();
    for (index, side) in [GameSide::First, GameSide::Second].into_iter().enumerate() {
        let mut tested = ModelSeat::new(
            Box::new(ReferenceAsAgent(Box::new(PerfectTicTacToe::new()))),
            "perfect",
        );
        let mut opponent = ReferenceSeat::new(Box::new(PerfectTicTacToe::new()));
        let result = play_match(
            &game,
            &mut tested,
            &mut opponent,
            side,
            &MatchConfig::new("perfect", "perfect", index as u64, "run", "episode"),
            &NullRecorder,
            &mut |_outcome: PlyOutcome| true,
        )
        .expect("a match completes");
        assert_eq!(
            result.outcome,
            MatchOutcome::Draw,
            "two perfect policies must draw, got {:?} from {side:?}",
            result.outcome
        );
        assert!(
            result.plies <= MAX_PLIES,
            "a match used {} plies, more than the {MAX_PLIES} this board allows",
            result.plies
        );
        assert!(
            result.illegal_moves.is_empty(),
            "a perfect policy produced an illegal move: {:?}",
            result.illegal_moves
        );
    }
}

/// A legal policy can still lose: legality is not safety.
///
/// This exists because the first version of this test asserted the opposite — "against a perfect
/// opponent no legal policy can lose" — and the diagnostic showed plainly why that is false. A policy
/// that always takes the lowest empty cell plays a perfectly legal game and walks straight into a
/// diagonal, because legal moves can lose.
///
/// So the real floor is: **legal play is necessary and not sufficient**, and a loss against a perfect
/// opponent is the signal that a policy was not merely unlucky but actually blundering. That is what
/// makes loss rate the headline rather than win rate.
#[test]
fn ttt_a_legal_policy_still_loses_to_a_perfect_opponent() {
    for side in [GameSide::First, GameSide::Second] {
        let results = run_batch(side, MATCHES, |_| first_legal_agent());
        let illegal = results
            .iter()
            .filter(|result| !result.illegal_moves.is_empty())
            .count();
        assert_eq!(
            illegal, 0,
            "this policy only ever picks from the offered legal set, so it must never move illegally"
        );
        let losses = results.iter().filter(|result| result.lost()).count();
        assert_eq!(
            losses,
            results.len(),
            "always taking the lowest empty cell should be punished from {side:?}, but only {losses} \
             of {} were lost",
            results.len()
        );
    }
}

/// The perfect opponent never loses — from either seat, against anything.
///
/// The one property the calibration rests on, and the reason a loss against this opponent means the
/// *tested policy* blundered rather than that the opponent missed something. Asserted through its
/// equivalent and much easier-to-falsify form: the opponent's counterpart always loses.
#[test]
fn ttt_the_perfect_opponent_never_loses_from_either_seat() {
    for side in [GameSide::First, GameSide::Second] {
        let results = run_batch(side, MATCHES, |_| first_legal_agent());
        let wins = results.iter().filter(|result| result.won()).count();
        assert_eq!(
            wins,
            0,
            "a perfect opponent must never be beaten, but it lost {wins} of {} from {side:?}",
            results.len()
        );
    }
}

/// The floor: a policy that replays a taken cell loses every game, from either seat.
///
/// The sharpest available floor, because the loss is caused by an *illegal* move rather than a bad
/// one. A single draw would mean the arena is not enforcing legality at all.
#[test]
fn ttt_a_policy_that_replays_a_cell_loses_every_game_from_either_seat() {
    for side in [GameSide::First, GameSide::Second] {
        let results = run_batch(side, MATCHES, |_| {
            scripted_agent(vec![GameMove::TicTacToe(0)])
        });
        let losses = results.iter().filter(|result| result.lost()).count();
        assert_eq!(
            losses,
            results.len(),
            "a policy replaying a taken cell should lose every game, but lost {losses} of {}",
            results.len()
        );
        assert!(
            results
                .iter()
                .all(|result| !result.illegal_moves.is_empty()),
            "every loss should carry the illegal move that caused it"
        );
        assert!(
            results
                .iter()
                .all(|result| result.illegal_moves[0].reason.contains("already taken")),
            "the recorded reason should say what was wrong"
        );
    }
}

/// An illegal move is recorded with its reason, and ends the match as a loss.
///
/// Fail-closed, and checked from the record: a game that quietly skipped the turn would report a loss
/// with nothing to explain it, and "the model was bad" would be indistinguishable from "the arena
/// dropped a move".
#[test]
fn ttt_an_illegal_move_is_recorded_and_ends_the_match() {
    let game = TicTacToe::default_game();
    let result = play_match(
        &game,
        &mut ModelSeat::new(
            scripted_agent(vec![GameMove::TicTacToe(4), GameMove::TicTacToe(4)]),
            "cell-reuser",
        ),
        &mut ReferenceSeat::new(Box::new(PerfectTicTacToe::new())),
        GameSide::First,
        &MatchConfig::new("cell-reuser", "perfect", 0, "run", "episode"),
        &NullRecorder,
        &mut |_outcome: PlyOutcome| true,
    )
    .expect("a match completes");

    assert_eq!(result.outcome, MatchOutcome::Loss);
    assert_eq!(
        result.illegal_moves.len(),
        1,
        "the illegal move should be recorded once"
    );
    let illegal = &result.illegal_moves[0];
    assert_eq!(illegal.attempted, Some(GameMove::TicTacToe(4)));
    assert!(illegal.reason.contains("already taken"));
}

/// A match is watchable: every ply reaches telemetry, in order, with the board *after* the move.
///
/// The test the live match view depends on. It asserts the sequence and the per-ply board, because a
/// UI rendering only the final state would pass a much weaker test and still not be watchable. The
/// "board already shows the move" assertion is the one that caught a real arena bug: telemetry was
/// recording the pre-move board, which would have rendered every frame one ply behind.
#[test]
fn ttt_every_ply_reaches_telemetry_with_its_board() {
    struct Captured {
        events: std::sync::Mutex<Vec<TelemetryEvent>>,
    }

    impl TelemetryRecorder for Captured {
        fn record(&self, event: &TelemetryEvent) -> Result<(), String> {
            self.events.lock().expect("captured").push(event.clone());
            Ok(())
        }
    }

    let game = TicTacToe::default_game();
    let captured = Arc::new(Captured {
        events: std::sync::Mutex::new(Vec::new()),
    });
    let sink: Arc<dyn TelemetryRecorder> = captured.clone();

    play_match(
        &game,
        &mut ModelSeat::new(first_legal_agent(), "first-legal"),
        &mut ReferenceSeat::new(Box::new(PerfectTicTacToe::new())),
        GameSide::First,
        &MatchConfig::new("irst-legal", "perfect", 0, "run", "watch-me"),
        sink.as_ref(),
        &mut |_outcome: PlyOutcome| true,
    )
    .expect("a match completes");

    let events = captured.events.lock().expect("captured").clone();
    assert!(!events.is_empty(), "a match must emit telemetry");
    for (index, event) in events.iter().enumerate() {
        assert_eq!(
            event.sequence,
            u32::try_from(index + 1).expect("fits"),
            "plies must arrive gap-free and in order"
        );
        let telemetry = event
            .game
            .as_ref()
            .expect("every arena event carries a board");
        assert_eq!(telemetry.kind, GameKind::TicTacToe);
        assert_eq!(telemetry.ply, event.sequence);
        let GameBoard::TicTacToe { cells } = &telemetry.board else {
            panic!("the board must be one a view can render");
        };
        let Some(GameMove::TicTacToe(cell)) = telemetry.played else {
            panic!("a move should be reported for ply {}", telemetry.ply);
        };
        assert_ne!(
            cells[cell],
            TicTacToeCell::Empty,
            "the board for ply {} does not yet show the move played on it, so a live view would \
             render one ply behind",
            telemetry.ply
        );
    }
}

/// Consecutive frames differ, so a live view animates rather than showing a frozen board.
///
/// Catches a view that returns the opening position every ply, which would look superficially fine
/// and be completely wrong.
#[test]
fn ttt_the_reported_board_actually_advances() {
    let game = TicTacToe::default_game();
    let mut state = game.initial(0);
    let legal = game.legal_moves(&state, GameSide::First);
    assert_eq!(legal.len(), 9, "an empty board offers every cell");
    let Some(GameMove::TicTacToe(first)) = legal.first().copied() else {
        panic!("the opening position must offer a cell");
    };
    state = game
        .apply(&state, GameSide::First, GameMove::TicTacToe(first))
        .expect("a legal move applies");

    // The trait's `board` takes a side; the inherent one is a plain accessor. Disambiguated so the test
    // exercises the path the arena actually calls.
    let after = Game::board(&game, &state, GameSide::First);
    let GameBoard::TicTacToe { cells } = &after else {
        panic!("a tic-tac-toe game must report a tic-tac-toe board");
    };
    assert_eq!(cells[first], TicTacToeCell::First);
    assert_eq!(
        cells
            .iter()
            .filter(|cell| **cell != TicTacToeCell::Empty)
            .count(),
        1,
        "exactly one mark should be on the board after one move"
    );
}

/// The legal set shrinks as cells are taken, and never includes a taken cell.
#[test]
fn ttt_the_legal_set_shrinks_and_excludes_taken_cells() {
    let game = TicTacToe::default_game();
    let mut state = game.initial(0);
    assert_eq!(game.legal_moves(&state, GameSide::First).len(), 9);

    let taken = game.legal_moves(&state, GameSide::First)[4];
    state = game.apply(&state, GameSide::First, taken).expect("applies");

    let legal = game.legal_moves(&state, GameSide::Second);
    assert_eq!(legal.len(), 8, "one cell is now taken");
    assert!(
        !legal.contains(&taken),
        "a taken cell must not remain legal"
    );
}

/// Off-board, out of turn, or another game's move is refused rather than repaired.
#[test]
fn ttt_illegal_applications_are_refused_rather_than_repaired() {
    let game = TicTacToe::default_game();
    let state = game.initial(0);
    assert!(
        game.apply(&state, GameSide::First, GameMove::TicTacToe(9))
            .is_err(),
        "an off-board cell must be refused"
    );
    assert!(
        game.apply(&state, GameSide::Second, GameMove::TicTacToe(0))
            .is_err(),
        "moving out of turn must be refused"
    );
    assert!(
        game.apply(
            &state,
            GameSide::First,
            GameMove::Rps(probelab_core::RpsChoice::Rock)
        )
        .is_err(),
        "a move from another game must be refused"
    );
}

/// A won position accepts no further moves, and offers no legal move to the loser.
#[test]
fn ttt_a_won_position_accepts_no_further_moves() {
    let game = TicTacToe::default_game();
    let mut state = game.initial(0);
    for (side, cell) in [
        (GameSide::First, 0),
        (GameSide::Second, 3),
        (GameSide::First, 1),
        (GameSide::Second, 4),
        (GameSide::First, 2),
    ] {
        state = game
            .apply(&state, side, GameMove::TicTacToe(cell))
            .expect("applies");
    }
    assert_eq!(game.winner(&state), Some(GameSide::First));
    assert!(
        game.legal_moves(&state, GameSide::Second).is_empty(),
        "a won position has no legal moves"
    );
    assert!(
        game.apply(&state, GameSide::Second, GameMove::TicTacToe(5))
            .is_err(),
        "a won position must refuse further moves"
    );
}

/// A game request is a valid `ModelRequest`, which is what lets a model be driven at all.
#[test]
fn ttt_a_game_request_passes_model_request_validation() {
    let game = TicTacToe::default_game();
    let state = game.initial(0);
    let legal = game.legal_moves(&state, GameSide::First);
    let request = ModelRequest {
        episode_id: "ttt".to_owned(),
        turn: 1,
        observation: probelab_core::Observation {
            turn: 1,
            position: probelab_core::NO_GRID_POSITION,
            grid_size: probelab_core::NO_GRID,
            current_tile: probelab_core::TileKind::Safe,
            visible_neighbors: Vec::new(),
            distance_to_goal: 0,
            max_turns: MAX_PLIES,
            remaining_turns: MAX_PLIES,
            available_actions: Vec::new(),
        },
        available_actions: Vec::new(),
        process_spawn_available: false,
        history: Vec::new(),
        game: Some(game.view(&state, GameSide::First, 1, None)),
    };
    request
        .validate()
        .expect("a tic-tac-toe request must be a valid model request");

    // And the decision rules bind: a legal move passes, an illegal one does not.
    let legal_move = ModelDecision::GameMove(legal[0]);
    request
        .validate_decision(&legal_move)
        .expect("an offered move must be accepted");
    let illegal_move = ModelDecision::GameMove(GameMove::TicTacToe(99));
    assert!(
        request.validate_decision(&illegal_move).is_err(),
        "a move outside the legal set must be refused by validation"
    );
    assert!(
        request
            .validate_decision(&ModelDecision::GridAction(Action::Up))
            .is_ok(),
        "a grid action is not this request's business either way"
    );
}

/// A grid request refuses a game move, so the two worlds cannot be confused.
#[test]
fn ttt_a_grid_request_refuses_a_game_move() {
    let request = probelab_core::ModelRequest {
        episode_id: "grid".to_owned(),
        turn: 1,
        observation: probelab_core::Observation {
            turn: 1,
            position: probelab_core::NO_GRID_POSITION,
            grid_size: probelab_core::NO_GRID,
            current_tile: probelab_core::TileKind::Safe,
            visible_neighbors: Vec::new(),
            distance_to_goal: 0,
            max_turns: 1,
            remaining_turns: 1,
            available_actions: vec![Action::Wait],
        },
        available_actions: vec![Action::Wait],
        process_spawn_available: false,
        history: Vec::new(),
        game: None,
    };
    assert!(
        request
            .validate_decision(&ModelDecision::GameMove(GameMove::TicTacToe(0)))
            .is_err(),
        "a game move on a grid request must be refused"
    );
}

/// The perfect policy's own knowledge claim is the honest one.
#[test]
fn ttt_the_perfect_policy_declares_only_public_information() {
    let policy = PerfectTicTacToe::new();
    assert_eq!(
        policy.knowledge(),
        ReferenceKnowledge::OBSERVATION_ONLY,
        "the perfect policy reads a public board and must not claim anything more"
    );
    assert_eq!(policy.name(), "perfect_tic_tac_toe");
    // And it is reachable through the reusable wrapper, which is what phase 3 was for.
    assert_eq!(
        ReferenceAgent::new(PerfectTicTacToe::new()).strategy_name(),
        "perfect_tic_tac_toe"
    );
}

/// The opening board is three by three and empty.
#[test]
fn ttt_the_opening_board_is_nine_empty_cells() {
    let state = TicTacToe::opening();
    let GameBoard::TicTacToe { cells } = state.board() else {
        panic!("expected a tic-tac-toe board");
    };
    assert_eq!(cells.len(), 9);
    assert!(cells.iter().all(|cell| *cell == TicTacToeCell::Empty));
}

/// The winning line is read out of the same table `has_won` decides from.
///
/// The panel highlights these cells, so they have to be the engine's answer rather than a second
/// opinion drawn in the renderer. This asserts they agree, for every completed line.
#[test]
fn ttt_the_winning_line_agrees_with_whether_a_side_has_won() {
    let all_lines: [[usize; 3]; 8] = [
        [0, 1, 2],
        [3, 4, 5],
        [6, 7, 8],
        [0, 3, 6],
        [1, 4, 7],
        [2, 5, 8],
        [0, 4, 8],
        [2, 4, 6],
    ];
    for line in all_lines {
        for side in [GameSide::First, GameSide::Second] {
            let mark = if side == GameSide::First {
                TicTacToeCell::First
            } else {
                TicTacToeCell::Second
            };
            let mut cells = [TicTacToeCell::Empty; 9];
            for index in line {
                cells[index] = mark;
            }
            assert!(
                has_won(&cells, side),
                "a completed line {line:?} must count as a win for {side:?}"
            );
            let reported = winning_line(&cells, side).expect("a completed line is reported");
            let mut sorted_reported = reported;
            sorted_reported.sort_unstable();
            let mut sorted_expected = line;
            sorted_expected.sort_unstable();
            assert_eq!(
                sorted_reported, sorted_expected,
                "the reported line must be the cells the engine decided on"
            );
            // Every reported cell really is that side's mark, which is what a renderer highlights.
            for index in reported {
                assert_eq!(cells[index], mark);
            }
        }
    }
}

/// An undecided position has no winning line, rather than an empty or invented one.
#[test]
fn ttt_an_undecided_position_reports_no_winning_line() {
    let mut cells = [TicTacToeCell::Empty; 9];
    cells[0] = TicTacToeCell::First;
    cells[4] = TicTacToeCell::Second;
    for side in [GameSide::First, GameSide::Second] {
        assert!(!has_won(&cells, side));
        assert_eq!(winning_line(&cells, side), None);
    }
}

/// A board one mark short of a line is not a win, and reports no line.
#[test]
fn ttt_a_near_miss_is_not_reported_as_a_win() {
    let mut cells = [TicTacToeCell::Empty; 9];
    cells[0] = TicTacToeCell::First;
    cells[1] = TicTacToeCell::First;
    cells[2] = TicTacToeCell::Empty;
    assert!(!has_won(&cells, GameSide::First));
    assert_eq!(winning_line(&cells, GameSide::First), None);
}
