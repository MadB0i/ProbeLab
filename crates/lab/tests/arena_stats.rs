//! Statistics over recorded arena matches.
//!
//! Most of what is covered here is what the module *refuses* to do, because every rule it follows is a
//! rule about not overstating a result: not turning an undecided match into a loss, not merging two
//! games, not averaging a policy's silence into a model's cost, and not reporting a rate without the
//! sample size behind it.
//!
//! The distribution tests build `ArenaMatchView` values directly, because an engine match cannot be
//! asked to lose on demand. The last test plays real matches, so the module is also exercised against
//! genuine engine output rather than only against hand-built fixtures.

use probelab_core::{GameBoard, GameKind, GameSide, TicTacToeCell, TokenUsage};
use probelab_engine::arena::SeatKind;
use probelab_lab::arena_stats::{arena_statistics, DEFAULT_CONFIDENCE, SMALL_SAMPLE_MATCHES};
use probelab_lab::{
    ArenaMatchSpec, ArenaMatchView, ArenaOutcome, ArenaRunner, ArenaTermination, IllegalMoveView,
    PlyView, TurnTelemetry,
};

/// The default pairing used by the distribution fixtures.
const TESTED: &str = "qwen2.5:0.5b";
const OPPONENT: &str = "depth_bounded_connect_four";

fn empty_tictactoe_board() -> GameBoard {
    GameBoard::TicTacToe {
        cells: vec![TicTacToeCell::Empty; 9],
    }
}

/// A recorded match with everything a statistics fold reads, and nothing it does not.
fn recorded(
    match_id: &str,
    game: &str,
    outcome: ArenaOutcome,
    turns: Vec<TurnTelemetry>,
) -> ArenaMatchView {
    recorded_with(match_id, game, outcome, None, turns)
}

/// As [`recorded`], with an explicit termination, so the non-conditional terminations can be exercised
/// without a hand-built engine match that fails to reach a provider.
fn recorded_with(
    match_id: &str,
    game: &str,
    outcome: ArenaOutcome,
    termination: Option<ArenaTermination>,
    turns: Vec<TurnTelemetry>,
) -> ArenaMatchView {
    ArenaMatchView {
        match_id: match_id.to_owned(),
        game: game.to_owned(),
        game_name: match game {
            "tic_tac_toe" => "Tic-Tac-Toe".to_owned(),
            "connect_four" => "Connect Four".to_owned(),
            other => other.to_owned(),
        },
        outcome,
        termination,
        tested_agent: TESTED.to_owned(),
        opponent_agent: OPPONENT.to_owned(),
        tested_seat: SeatKind::Model,
        opponent_seat: SeatKind::ReferencePolicy,
        tested_side: GameSide::First,
        seed: 7,
        plies: u32::try_from(turns.len()).unwrap_or(0),
        final_board: empty_tictactoe_board(),
        winner: None,
        winning_line: None,
        illegal_moves: Vec::new(),
        history: Vec::<PlyView>::new(),
        turns,
        timeline: Vec::new(),
        failure: None,
        cancelled: false,
    }
}

/// A model turn that reported tokens.
fn model_turn(elapsed_ms: u64, prompt: u64, completion: u64) -> TurnTelemetry {
    TurnTelemetry {
        ply: 1,
        side: GameSide::First,
        seat: SeatKind::Model,
        seat_label: TESTED.to_owned(),
        elapsed_ms,
        usage: Some(TokenUsage {
            prompt_tokens: Some(prompt),
            completion_tokens: Some(completion),
            total_tokens: Some(prompt + completion),
        }),
        prompt_template_version: Some("connect_four_v1".to_owned()),
        response_text: Some("column 4".to_owned()),
        failure: None,
        request_board: empty_tictactoe_board(),
    }
}

/// A policy turn: no tokens, because a shipped policy has none to report.
fn policy_turn(elapsed_ms: u64) -> TurnTelemetry {
    TurnTelemetry {
        ply: 2,
        side: GameSide::Second,
        seat: SeatKind::ReferencePolicy,
        seat_label: OPPONENT.to_owned(),
        elapsed_ms,
        usage: None,
        prompt_template_version: None,
        response_text: None,
        failure: None,
        request_board: empty_tictactoe_board(),
    }
}

fn one_group(matches: Vec<ArenaMatchView>) -> probelab_lab::MatchStatistics {
    let groups = arena_statistics(&matches).groups;
    assert_eq!(groups.len(), 1, "expected exactly one group");
    groups.into_iter().next().expect("one group")
}

fn wins(count: usize, prefix: &str) -> Vec<ArenaMatchView> {
    (0..count)
        .map(|index| {
            recorded(
                &format!("{prefix}{index}"),
                "connect_four",
                ArenaOutcome::Win,
                Vec::new(),
            )
        })
        .collect()
}

#[test]
fn a_match_that_decided_nothing_is_never_counted_as_a_loss() {
    // The rule the module exists to protect. A provider that refused the connection, a model that was
    // never installed, and a match the operator stopped are all `no_result`; none of them is a defeat.
    let group = one_group(vec![
        recorded("a", "connect_four", ArenaOutcome::Win, Vec::new()),
        recorded("b", "connect_four", ArenaOutcome::Loss, Vec::new()),
        recorded("c", "connect_four", ArenaOutcome::NoResult, Vec::new()),
        recorded("d", "connect_four", ArenaOutcome::NoResult, Vec::new()),
    ]);

    assert_eq!(group.decisions.outcomes.no_result, 2);
    assert_eq!(
        group.decisions.outcomes.loss, 1,
        "a refusal is not a defeat"
    );
    // Neither rate lets the two undecided matches into a denominator.
    assert_eq!(group.decisions.outcomes.decided(), 2);
    assert_eq!(group.decisions.outcomes.decisive(), 2);
    assert!((group.decisions.win_rate.point - 0.5).abs() < 1e-9);
}

#[test]
fn the_win_rate_denominator_is_the_decided_matches_only() {
    let mut matches = wins(2, "w");
    matches.push(recorded(
        "l",
        "connect_four",
        ArenaOutcome::Loss,
        Vec::new(),
    ));
    // Two undecided matches that must not dilute the denominator.
    matches.push(recorded(
        "n1",
        "connect_four",
        ArenaOutcome::NoResult,
        Vec::new(),
    ));
    matches.push(recorded(
        "n2",
        "connect_four",
        ArenaOutcome::NoResult,
        Vec::new(),
    ));

    let group = one_group(matches);
    // 2 wins of 3 decided, not 2 of 5.
    assert!(
        (group.decisions.win_rate.point - 2.0 / 3.0).abs() < 1e-9,
        "expected 0.6667, got {}",
        group.decisions.win_rate.point
    );
}

#[test]
fn a_draw_sits_in_the_decided_denominator_and_not_the_numerator() {
    // Two readings, because they answer different questions and collapsing them to one answers neither.
    let group = one_group(vec![
        recorded("a", "connect_four", ArenaOutcome::Win, Vec::new()),
        recorded("b", "connect_four", ArenaOutcome::Draw, Vec::new()),
        recorded("c", "connect_four", ArenaOutcome::Loss, Vec::new()),
    ]);

    assert!((group.decisions.win_rate.point - 1.0 / 3.0).abs() < 1e-9);
    assert!((group.decisions.decisive_win_rate.point - 0.5).abs() < 1e-9);
    assert_eq!(group.decisions.outcomes.decided(), 3);
    assert_eq!(group.decisions.outcomes.decisive(), 2);
}

#[test]
fn a_match_with_no_decision_reports_a_full_interval_rather_than_a_confident_zero() {
    // One cancelled match is the common case, and reporting "0% win rate" for it would be a claim
    // about a match that never ran.
    let group = one_group(vec![recorded(
        "a",
        "connect_four",
        ArenaOutcome::NoResult,
        Vec::new(),
    )]);

    assert!(!group.decisions.has_decided);
    assert_eq!(group.decisions.outcomes.decided(), 0);
    assert_eq!(group.decisions.win_rate.lower, 0.0);
    assert_eq!(group.decisions.win_rate.upper, 1.0);
    assert!(group.sample_is_small);
}

#[test]
fn two_games_are_never_merged_into_one_number() {
    // A tic-tac-toe win and a connect-four win are different events with different difficulty and
    // different opponent strength. A figure spanning them would be valid arithmetic and meaningless.
    let statistics = arena_statistics(&[
        recorded("a", "tic_tac_toe", ArenaOutcome::Win, Vec::new()),
        recorded("b", "connect_four", ArenaOutcome::Loss, Vec::new()),
        recorded("c", "connect_four", ArenaOutcome::Win, Vec::new()),
    ]);

    assert_eq!(
        statistics.groups.len(),
        2,
        "one group per game, never a combined figure"
    );
    assert_eq!(statistics.matches_read, 3);

    let connect = statistics
        .groups
        .iter()
        .find(|group| group.game == "connect_four")
        .expect("connect four group");
    assert_eq!(connect.decisions.outcomes.win, 1);
    assert_eq!(connect.decisions.outcomes.loss, 1);

    let tictactoe = statistics
        .groups
        .iter()
        .find(|group| group.game == "tic_tac_toe")
        .expect("tic-tac-toe group");
    assert_eq!(tictactoe.matches, 1);
}

#[test]
fn different_pairings_are_separate_groups() {
    // A win rate against one opponent is not a measure of an agent, so it is never pooled with a
    // win rate against another.
    let mut against_legal = recorded("a", "connect_four", ArenaOutcome::Win, Vec::new());
    against_legal.opponent_agent = "first_legal".to_owned();
    let against_depth = recorded("b", "connect_four", ArenaOutcome::Win, Vec::new());

    let statistics = arena_statistics(&[against_legal, against_depth]);
    assert_eq!(statistics.groups.len(), 2);
    assert!(statistics
        .groups
        .iter()
        .all(|group| group.matches == 1 && group.sample_is_small));
}

#[test]
fn group_order_is_stable_across_input_order() {
    // A panel that re-renders must not reshuffle its rows, so the output is sorted rather than
    // following the order the archive happened to return.
    let first = recorded("b", "tic_tac_toe", ArenaOutcome::Win, Vec::new());
    let second = recorded("a", "connect_four", ArenaOutcome::Win, Vec::new());

    let forward: Vec<String> = arena_statistics(&[first.clone(), second.clone()])
        .groups
        .into_iter()
        .map(|group| group.game)
        .collect();
    let backward: Vec<String> = arena_statistics(&[second, first])
        .groups
        .into_iter()
        .map(|group| group.game)
        .collect();

    assert_eq!(forward, backward);
    assert_eq!(
        forward,
        vec!["connect_four".to_owned(), "tic_tac_toe".to_owned()]
    );
}

#[test]
fn a_small_sample_is_labelled_as_one() {
    let at_threshold = one_group(wins(SMALL_SAMPLE_MATCHES, "a"));
    assert!(
        !at_threshold.sample_is_small,
        "exactly the threshold is not a small sample"
    );

    let below = one_group(wins(SMALL_SAMPLE_MATCHES - 1, "b"));
    assert!(below.sample_is_small);
}

#[test]
fn undecided_matches_do_not_count_toward_the_small_sample_threshold() {
    // Nine decided and twenty cancelled is still a sample of nine.
    let mut matches = wins(9, "w");
    matches.extend((0..20).map(|index| {
        recorded(
            &format!("x{index}"),
            "connect_four",
            ArenaOutcome::NoResult,
            Vec::new(),
        )
    }));

    let group = one_group(matches);
    assert_eq!(group.matches, 29);
    assert_eq!(group.decisions.outcomes.decided(), 9);
    assert!(
        group.sample_is_small,
        "the threshold counts decided matches only"
    );
}

#[test]
fn token_totals_cover_only_the_turns_that_reported_them() {
    let group = one_group(vec![recorded(
        "a",
        "connect_four",
        ArenaOutcome::Win,
        vec![
            model_turn(1000, 100, 5),
            policy_turn(2),
            model_turn(2000, 200, 10),
        ],
    )]);

    assert_eq!(group.costs.prompt_tokens, Some(300));
    assert_eq!(group.costs.completion_tokens, Some(15));
    assert_eq!(group.costs.total_tokens, Some(315));
    // The policy turn reported no usage block. That is "did not say", not "was free".
    assert_eq!(group.costs.turns_without_usage, 1);
}

#[test]
fn latency_averages_every_measured_turn() {
    let group = one_group(vec![recorded(
        "a",
        "connect_four",
        ArenaOutcome::Win,
        vec![model_turn(1000, 1, 1), model_turn(3000, 1, 1)],
    )]);

    assert_eq!(group.costs.measured_turns, 2);
    assert_eq!(group.costs.total_elapsed_ms, 4000);
    assert!((group.costs.mean_elapsed_ms.expect("mean") - 2000.0).abs() < 1e-9);
    assert_eq!(group.costs.max_elapsed_ms, Some(3000));
}

#[test]
fn a_match_with_no_turns_reports_no_mean_rather_than_a_zero() {
    let group = one_group(vec![recorded(
        "a",
        "connect_four",
        ArenaOutcome::Win,
        Vec::new(),
    )]);
    assert_eq!(group.costs.measured_turns, 0);
    assert_eq!(group.costs.mean_elapsed_ms, None);
    assert_eq!(group.costs.prompt_tokens, None);
}

#[test]
fn illegal_moves_are_counted_apart_from_outcomes() {
    // A seat that answered with a move the board refused is a fifth kind of event: not a loss, not a
    // win, and the match may still reach a decision.
    let mut view = recorded(
        "a",
        "connect_four",
        ArenaOutcome::Win,
        vec![model_turn(10, 1, 1)],
    );
    view.illegal_moves = vec![
        IllegalMoveView {
            ply: 1,
            attempted: None,
            reason: "column 9 is not a column".to_owned(),
        },
        IllegalMoveView {
            ply: 1,
            attempted: None,
            reason: "column 9 is not a column".to_owned(),
        },
    ];

    let group = one_group(vec![view]);
    assert_eq!(group.illegal_moves, 2);
    assert_eq!(group.matches_with_illegal_moves, 1);
    // Still a win: the engine's own outcome is untouched by how many moves were refused.
    assert_eq!(group.decisions.outcomes.win, 1);
}

#[test]
fn ply_statistics_span_every_match_including_the_undecided_ones() {
    let group = one_group(vec![
        recorded(
            "a",
            "connect_four",
            ArenaOutcome::Win,
            vec![policy_turn(1), policy_turn(1), policy_turn(1)],
        ),
        recorded(
            "b",
            "connect_four",
            ArenaOutcome::NoResult,
            vec![policy_turn(1)],
        ),
    ]);

    assert_eq!(group.plies.total, 4);
    assert!((group.plies.mean.expect("mean") - 2.0).abs() < 1e-9);
    assert_eq!(group.plies.min, Some(1));
    assert_eq!(group.plies.max, Some(3));
}

#[test]
fn a_cancelled_match_is_reported_apart_from_another_failure() {
    // "You stopped this" and "the provider died" are different events, and only one is about the
    // operator. Both are `no_result`; only one is a cancellation.
    let mut cancelled = recorded("a", "connect_four", ArenaOutcome::NoResult, Vec::new());
    cancelled.cancelled = true;
    let mut failed = recorded("b", "connect_four", ArenaOutcome::NoResult, Vec::new());
    failed.failure = Some("the provider refused the connection".to_owned());

    let group = one_group(vec![cancelled, failed]);
    assert_eq!(group.decisions.cancelled, 1);
    assert_eq!(group.decisions.with_failure, 1);
    assert_eq!(group.decisions.outcomes.no_result, 2);
    assert_eq!(group.decisions.outcomes.loss, 0);
}

#[test]
fn every_group_names_the_recordings_behind_it() {
    // A number with no list behind it cannot be checked.
    let group = one_group(vec![
        recorded("m2", "connect_four", ArenaOutcome::Win, Vec::new()),
        recorded("m1", "connect_four", ArenaOutcome::Loss, Vec::new()),
    ]);

    assert_eq!(group.match_ids, vec!["m2".to_owned(), "m1".to_owned()]);
    assert_eq!(group.matches, 2);
    assert_eq!(
        group.provenance_digest.len(),
        16,
        "a 64-bit digest, hex encoded"
    );
}

#[test]
fn the_provenance_digest_tracks_the_recordings_behind_it() {
    let base = wins(2, "w");
    let first = one_group(base.clone());
    let same = one_group(base);
    assert_eq!(
        first.provenance_digest, same.provenance_digest,
        "identical inputs must produce an identical digest"
    );

    let different = one_group(wins(1, "w"));
    assert_ne!(
        first.provenance_digest, different.provenance_digest,
        "a different recording must change the digest"
    );
}

#[test]
fn no_recordings_produce_no_groups_rather_than_a_zero_row() {
    let statistics = arena_statistics(&[]);
    assert!(statistics.is_empty());
    assert_eq!(statistics.matches_read, 0);
    assert_eq!(statistics.ungrouped, 0);
}

#[test]
fn a_recording_without_an_identity_is_counted_as_ungrouped_not_dropped() {
    // Discarding it silently would make `matches_read` disagree with the archive, which is worse than
    // saying so.
    let mut nameless = recorded("a", "connect_four", ArenaOutcome::Win, Vec::new());
    nameless.tested_agent = String::new();

    let statistics = arena_statistics(&[nameless]);
    assert_eq!(statistics.ungrouped, 1);
    assert_eq!(statistics.matches_read, 0);
    assert!(statistics.groups.is_empty());
}

#[test]
fn the_provenance_sentence_states_the_sample_and_the_undecided_count() {
    let group = one_group(vec![
        recorded("a", "connect_four", ArenaOutcome::Win, Vec::new()),
        recorded("b", "connect_four", ArenaOutcome::NoResult, Vec::new()),
    ]);
    let sentence = group.provenance_sentence();
    assert!(sentence.contains(TESTED), "{sentence}");
    assert!(sentence.contains("Connect Four"), "{sentence}");
    assert!(
        sentence.contains("1 of them reached a result"),
        "{sentence}"
    );
    assert!(sentence.contains("1 did not"), "{sentence}");
}

#[test]
fn every_interval_is_reported_at_the_one_confidence_the_product_uses() {
    let group = one_group(vec![
        recorded("a", "connect_four", ArenaOutcome::Win, Vec::new()),
        recorded("b", "connect_four", ArenaOutcome::Loss, Vec::new()),
    ]);
    assert_eq!(group.decisions.win_rate.confidence, DEFAULT_CONFIDENCE);
    assert_eq!(
        group.decisions.decisive_win_rate.confidence,
        DEFAULT_CONFIDENCE
    );
}

#[test]
fn an_interval_is_never_narrower_than_its_point_estimate() {
    let mut matches = wins(4, "w");
    matches.extend((0..3).map(|index| {
        recorded(
            &format!("l{index}"),
            "connect_four",
            ArenaOutcome::Loss,
            Vec::new(),
        )
    }));
    let group = one_group(matches);

    for interval in [group.decisions.win_rate, group.decisions.decisive_win_rate] {
        assert!(interval.lower <= interval.point, "{interval:?}");
        assert!(interval.point <= interval.upper, "{interval:?}");
        assert!(
            interval.lower >= 0.0 && interval.upper <= 1.0,
            "{interval:?}"
        );
    }
}

#[test]
fn statistics_computed_from_real_engine_matches() {
    // The distribution tests above use hand-built views, because an engine match cannot be asked to
    // lose on demand. This one plays real matches, so the module is exercised against genuine engine
    // output: real outcomes, real per-turn telemetry, and a real digest.
    let played: Vec<ArenaMatchView> = (0..6)
        .map(|index| {
            ArenaRunner::new()
                .play_simple(
                    &ArenaMatchSpec::new(
                        GameKind::TicTacToe,
                        "first_legal_tictactoe",
                        "perfect_tictactoe",
                        if index % 2 == 0 {
                            GameSide::First
                        } else {
                            GameSide::Second
                        },
                        100 + u64::try_from(index).unwrap_or(0),
                        &format!("played-{index}"),
                    ),
                    None,
                )
                .unwrap_or_else(|error| panic!("tic-tac-toe should play: {error}"))
                .view
        })
        .collect();

    let statistics = arena_statistics(&played);
    assert_eq!(statistics.groups.len(), 1);
    let group = &statistics.groups[0];

    assert_eq!(group.game, "tic_tac_toe");
    assert_eq!(group.matches, 6);
    assert_eq!(statistics.matches_read, 6);

    // Every match reached a result, so nothing is a refusal and nothing is a loss-by-default.
    assert_eq!(
        group.decisions.outcomes.no_result, 0,
        "a played policy-versus-policy match always reaches a result"
    );
    assert_eq!(
        group.decisions.outcomes.decided(),
        6,
        "all six were decided"
    );
    assert!(
        group.decisions.has_decided,
        "six decided matches means there is a rate to report"
    );

    // The real turns carried real latencies, so a mean exists.
    assert!(group.costs.measured_turns > 0);
    assert!(group.costs.mean_elapsed_ms.is_some());
    assert!(group.plies.total > 0);
    assert!(group.plies.min.is_some() && group.plies.max.is_some());

    // Policy seats report no usage, which must stay "unmeasured" rather than become a zero total.
    assert_eq!(group.costs.turns_without_usage, group.costs.measured_turns);
    assert_eq!(group.costs.total_tokens, None);

    // Every recording's identifier is listed, so the numbers can be traced back.
    assert_eq!(group.match_ids.len(), 6);
    for index in 0..6 {
        assert!(
            group.match_ids.contains(&format!("played-{index}")),
            "{:?} is missing played-{index}",
            group.match_ids
        );
    }
}

// ============================================================================
// Audit finding B6 — failure accounting.
//
// The defect these cover: a seat that was reached, answered, and named something no legal move can be
// read from produced `NoResult`, which is excluded from every rate denominator. A model that failed
// every single match therefore reported *no results* rather than *zero wins out of every match*, and a
// batch with three clean matches and seven fortresses showed a 100% win rate over a population of
// three.
// ============================================================================

fn no_turns() -> Vec<TurnTelemetry> {
    Vec::new()
}

#[test]
fn a_forfeit_is_a_loss_and_stays_in_the_denominator() {
    let views = vec![
        recorded_with(
            "won-1",
            "tic_tac_toe",
            ArenaOutcome::Win,
            Some(ArenaTermination::Decided),
            no_turns(),
        ),
        recorded_with(
            "forfeit-1",
            "tic_tac_toe",
            ArenaOutcome::Loss,
            Some(ArenaTermination::ModelRuleViolation),
            no_turns(),
        ),
    ];
    let group = &arena_statistics(&views).groups[0];

    // The forfeit is a loss, and losses are in the decided denominator. One win and one loss.
    assert_eq!(group.decisions.outcomes.loss, 1);
    assert_eq!(group.decisions.outcomes.win, 1);
    assert_eq!(group.decisions.outcomes.decided(), 2);
    assert_eq!(
        group.decisions.win_rate.point, 0.5,
        "a forfeited match must count against the win rate, not vanish from it"
    );
    assert_eq!(group.population.conditional, 2);
    assert_eq!(group.population.forfeited, 1);
}

#[test]
fn zero_success_reports_a_zero_win_rate_rather_than_no_results() {
    // The exact failure B6 describes: every match forfeited. Nothing was ever won.
    let views: Vec<ArenaMatchView> = (0..5)
        .map(|index| {
            recorded_with(
                &format!("forfeit-{index}"),
                "tic_tac_toe",
                ArenaOutcome::Loss,
                Some(ArenaTermination::ModelRuleViolation),
                no_turns(),
            )
        })
        .collect();
    let group = &arena_statistics(&views).groups[0];

    assert_eq!(group.decisions.outcomes.win, 0);
    assert_eq!(group.decisions.outcomes.decided(), 5);
    assert_eq!(
        group.decisions.win_rate.point, 0.0,
        "five forfeits must read as a 0% win rate, not as an absence of results"
    );
    assert_eq!(group.population.conditional, 5);
    assert_eq!(group.population.forfeited, 5);
    assert_eq!(group.population.excluded(), 0);
}

#[test]
fn infrastructure_failures_are_excluded_from_the_rate_but_counted_beside_it() {
    // Three decided matches and seven the harness never managed. The rate is about the three; the
    // population has to say so out loud, which is the whole point of B6.
    let mut views: Vec<ArenaMatchView> = (0..3)
        .map(|index| {
            recorded_with(
                &format!("clean-{index}"),
                "tic_tac_toe",
                ArenaOutcome::Win,
                Some(ArenaTermination::Decided),
                no_turns(),
            )
        })
        .collect();
    for index in 0..7 {
        views.push(recorded_with(
            &format!("provider-down-{index}"),
            "tic_tac_toe",
            ArenaOutcome::NoResult,
            Some(ArenaTermination::InfrastructureFailure),
            no_turns(),
        ));
    }
    let group = &arena_statistics(&views).groups[0];

    assert_eq!(group.decisions.outcomes.win, 3);
    assert_eq!(group.decisions.outcomes.decided(), 3);
    assert_eq!(group.population.attempted, 10);
    assert_eq!(group.population.conditional, 3);
    assert_eq!(group.population.infrastructure_failures, 7);
    assert_eq!(group.population.excluded(), 7);
    assert_eq!(
        group.population.reliability(),
        Some(0.3),
        "three of ten attempted matches produced a conditional outcome"
    );
}

#[test]
fn a_cancellation_is_not_an_infrastructure_failure_and_not_a_loss() {
    let views = vec![
        recorded_with(
            "decided-1",
            "tic_tac_toe",
            ArenaOutcome::Win,
            Some(ArenaTermination::Decided),
            no_turns(),
        ),
        recorded_with(
            "cancelled-1",
            "tic_tac_toe",
            ArenaOutcome::NoResult,
            Some(ArenaTermination::Cancelled),
            no_turns(),
        ),
        recorded_with(
            "provider-down-1",
            "tic_tac_toe",
            ArenaOutcome::NoResult,
            Some(ArenaTermination::InfrastructureFailure),
            no_turns(),
        ),
    ];
    let group = &arena_statistics(&views).groups[0];

    assert_eq!(group.population.cancelled, 1);
    assert_eq!(group.population.infrastructure_failures, 1);
    assert_eq!(group.population.conditional, 1);
    assert_eq!(group.population.attempted, 3);
    // An operator pressing stop has not made the seat lose, so nothing moved into the loss bucket.
    assert_eq!(group.decisions.outcomes.loss, 0);
    assert_eq!(group.decisions.outcomes.win, 1);
}

#[test]
fn a_historical_recording_without_a_termination_is_unclassified_never_assumed_decided() {
    // A row written before the termination existed. Guessing `decided` would move infrastructure
    // failures into the outcome population and inflate the conditional denominator.
    let views = vec![
        recorded_with(
            "legacy-1",
            "tic_tac_toe",
            ArenaOutcome::NoResult,
            None,
            no_turns(),
        ),
        recorded_with(
            "legacy-2",
            "tic_tac_toe",
            ArenaOutcome::Win,
            None,
            no_turns(),
        ),
    ];
    let group = &arena_statistics(&views).groups[0];

    assert_eq!(group.population.unclassified, 2);
    assert_eq!(group.population.conditional, 0);
    assert_eq!(group.population.attempted, 2);
}

#[test]
fn the_two_populations_are_reported_together_and_sum_to_the_attempted_count() {
    let mut views: Vec<ArenaMatchView> = Vec::new();
    for (index, (outcome, termination)) in [
        (ArenaOutcome::Win, Some(ArenaTermination::Decided)),
        (ArenaOutcome::Loss, Some(ArenaTermination::Decided)),
        (ArenaOutcome::Draw, Some(ArenaTermination::Decided)),
        (
            ArenaOutcome::Loss,
            Some(ArenaTermination::ModelRuleViolation),
        ),
        (
            ArenaOutcome::NoResult,
            Some(ArenaTermination::InfrastructureFailure),
        ),
        (ArenaOutcome::NoResult, Some(ArenaTermination::Cancelled)),
        (ArenaOutcome::NoResult, None),
    ]
    .into_iter()
    .enumerate()
    {
        views.push(recorded_with(
            &format!("mix-{index}"),
            "tic_tac_toe",
            outcome,
            termination,
            no_turns(),
        ));
    }
    let group = &arena_statistics(&views).groups[0];
    let population = group.population;

    // Every attempted match is accounted for exactly once. A statistic that drops a bucket is the
    // failure this whole finding is about.
    assert_eq!(population.attempted, 7);
    assert_eq!(
        population.conditional
            + population.infrastructure_failures
            + population.cancelled
            + population.unclassified,
        population.attempted,
        "a match went missing between the buckets"
    );
    assert_eq!(population.conditional, 4);
    assert_eq!(population.forfeited, 1);
    assert_eq!(population.infrastructure_failures, 1);
    assert_eq!(population.cancelled, 1);
    assert_eq!(population.unclassified, 1);
    // Conditional win rate: 1 win of 4 conditional matches, forfeits included.
    assert_eq!(group.decisions.outcomes.decided(), 4);
    assert_eq!(group.decisions.win_rate.point, 0.25);
}
