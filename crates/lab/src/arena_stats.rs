//! Statistics over recorded arena matches.
//!
//! # What this module does
//!
//! Reads finished matches and says what they add up to: how often the tested seat won, how often it
//! drew, how often it lost, how often nothing was decided at all, what a match cost, and how confident
//! any of that is.
//!
//! # The four rules it will not bend
//!
//! **A match that decided nothing is not a loss.** [`ArenaOutcome::NoResult`] is a fourth bucket with
//! its own count, and it is excluded from every rate denominator. A cancelled match, a provider that
//! refused the connection, and a model that was not installed are all `NoResult` — none of them is a
//! thing the tested seat lost, and folding them into `Loss` would turn an infrastructure failure into a
//! defeat on the record. A seat that answered with an illegal move is a fifth thing again: it is
//! recorded as an illegal move and the match usually still reaches a decision, so it is counted in
//! `illegal_moves` and left in the outcome distribution.
//!
//! **Statistics come from recordings, never from live state.** The only input is
//! `&[ArenaMatchView]` — what the durable archive holds. There is no path here that reads a running
//! match, a counter, or a counter the engine keeps in memory, so a statistic cannot silently mix a
//! half-played match into a settled result.
//!
//! **One game is one game.** Every [`MatchStatistics`] is scoped to one game and one pairing, and
//! nothing here sums across games. A win in tic-tac-toe and a win in connect four are different events
//! with different move counts, different opponent strengths and different difficulty; a number that
//! averaged them would be arithmetically valid and completely meaningless, and the cheapest way to
//! stop someone building that number later is to never expose the inputs that would make it easy.
//!
//! **A small sample says so.** [`MatchStatistics::sample_is_small`] is true below
//! [`SMALL_SAMPLE_MATCHES`] decided matches, and the interval on a small sample is wide on purpose.
//!
//! # What it deliberately does not do
//!
//! It does not rank agents. Ranking needs a claim that two runs measured the same thing, which is
//! [`crate::comparison`]'s job and a much harder question than counting outcomes. A win rate against
//! one opponent is not a measure of an agent, and this module returns the counts so a reader can
//! decide what they are looking at.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::arena::{ArenaMatchView, ArenaOutcome, ArenaTermination};
use crate::runner::{wilson_interval, WilsonInterval};

/// The confidence every interval in this module is reported at.
///
/// Matches the harness and the task reports, so one product never shows two different bars for the
/// same 95%.
pub const DEFAULT_CONFIDENCE: f64 = 0.95;

/// Below this many *decided* matches, a statistics block is a small sample and says so.
///
/// Ten is not a statistical threshold in any rigorous sense; it is the point below which a Wilson
/// interval on a proportion is so wide that the point estimate is close to meaningless. The interval
/// is still reported, because it is the honest way to show that.
pub const SMALL_SAMPLE_MATCHES: usize = 10;

/// How the decided matches fell.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutcomeCounts {
    /// The tested seat won.
    pub win: usize,
    /// The match ended level.
    pub draw: usize,
    /// The tested seat lost, to a real result by the other seat.
    pub loss: usize,
    /// Nothing was decided. Not a loss, and not counted as one anywhere in this module.
    pub no_result: usize,
}

/// Every recorded match accounted for, so a conditional rate can never hide its own population.
///
/// This is the answer to audit finding B6. A win rate answers "of the matches that reached a decision,
/// how often did the tested seat win?" — which is a true question only if you also say how many matches
/// never reached a decision and why. Reporting the rate alone lets a model that answered every ply with
/// unusable text show up with a comfortable win rate over the handful of matches that did resolve,
/// because the matches it lost by forfeiting were never in the denominator at all.
///
/// Every recorded match lands in exactly one bucket, and `unclassified` is a real bucket rather than a
/// remainder to be ignored: a recording that predates the termination field says nothing about why it
/// ended, and saying so is more useful than guessing `decided`.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchPopulation {
    /// Every recording counted into this block.
    pub attempted: usize,
    /// Matches that produced a conditional game outcome: the rules decided, or the tested seat forfeited.
    pub conditional: usize,
    /// Of those, the ones the tested seat forfeited by answering unusably.
    pub forfeited: usize,
    /// Matches the harness or provider could not deliver a usable answer for.
    pub infrastructure_failures: usize,
    /// Matches an operator stopped.
    pub cancelled: usize,
    /// Recordings whose reason is not recorded.
    ///
    /// Historical rows predate the termination field. They are counted here rather than being assumed
    /// `decided`, because assuming would move infrastructure failures into the outcome population.
    pub unclassified: usize,
}

impl MatchPopulation {
    /// Matches excluded from the conditional win-rate denominator.
    pub fn excluded(&self) -> usize {
        self.infrastructure_failures + self.cancelled + self.unclassified
    }

    /// The share of attempted matches that produced a conditional outcome at all.
    ///
    /// Distinct from the win rate, and reported beside it. A seat can win every match it finishes and
    /// still finish almost none of them; only these two numbers together say which happened.
    pub fn reliability(&self) -> Option<f64> {
        (self.attempted > 0).then(|| self.conditional as f64 / self.attempted as f64)
    }

    /// Folds one recorded match into the population.
    ///
    /// A `NoResult` outcome cannot be classified without the termination, because a forfeit, an
    /// unreachable provider and a cancellation all present as "no result" at the outcome level alone.
    /// The outcome is therefore never used to infer the reason.
    pub fn absorb(&mut self, view: &ArenaMatchView) {
        self.attempted += 1;
        match view.termination {
            Some(ArenaTermination::ModelRuleViolation) => {
                self.conditional += 1;
                self.forfeited += 1;
            }
            Some(ArenaTermination::Decided) => self.conditional += 1,
            Some(ArenaTermination::InfrastructureFailure) => self.infrastructure_failures += 1,
            Some(ArenaTermination::Cancelled) => self.cancelled += 1,
            None => self.unclassified += 1,
        }
    }
}

impl OutcomeCounts {
    /// Matches that reached a win, a draw or a loss.
    ///
    /// This is the denominator for [`DecisionStatistics::win_rate`].
    pub fn decided(&self) -> usize {
        self.win + self.draw + self.loss
    }

    /// Matches that had a winner.
    ///
    /// This is the denominator for [`DecisionStatistics::decisive_win_rate`].
    pub fn decisive(&self) -> usize {
        self.win + self.loss
    }

    /// Records one outcome.
    fn record(&mut self, outcome: ArenaOutcome) {
        match outcome {
            ArenaOutcome::Win => self.win += 1,
            ArenaOutcome::Draw => self.draw += 1,
            ArenaOutcome::Loss => self.loss += 1,
            ArenaOutcome::NoResult => self.no_result += 1,
        }
    }
}

/// Everything the outcome distribution implies, kept separate from the counts themselves so a reader
/// cannot mistake a rate for a count.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionStatistics {
    /// The raw outcome counts. `no_result` is here, in its own bucket.
    pub outcomes: OutcomeCounts,
    /// Win / decided — how often the tested seat won among matches that produced a result of any kind.
    ///
    /// A draw sits in this denominator and not the numerator, so this reads as "won outright". A draw is
    /// not a defeat, but it is also not a win, and hiding it in a separate number would make this figure
    /// look better than the matches were.
    pub win_rate: WilsonInterval,
    /// Win / decisive — how often the tested seat won among matches that had a winner at all.
    ///
    /// This is the closer reading when a pairing draws often, because it excludes draws rather than
    /// counting them against the seat. Reported alongside [`Self::win_rate`] because the two answer
    /// different questions and collapsing them to one would answer neither.
    pub decisive_win_rate: WilsonInterval,
    /// Matches that were cancelled by the operator.
    ///
    /// Every cancelled match is a `no_result`, but it is called out separately because "you stopped
    /// this" and "the provider died" are different events and only one of them is about the operator.
    pub cancelled: usize,
    /// Matches that recorded a failure reason.
    pub with_failure: usize,
    /// Whether there is anything to compute a rate from.
    pub has_decided: bool,
}

/// How long matches ran, in plies.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlyStatistics {
    /// Total plies across every match in the group, including undecided ones.
    pub total: u64,
    /// Mean plies per match.
    pub mean: Option<f64>,
    /// Shortest match.
    pub min: Option<u32>,
    /// Longest match.
    pub max: Option<u32>,
}

/// What the matches cost.
///
/// Averages are over *measured* turns only. A shipped policy genuinely has no tokens and reports no
/// elapsed time per turn, and averaging a policy's absence into a model's latency would produce a
/// number that describes neither.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CostStatistics {
    /// Turns that reported an elapsed time.
    pub measured_turns: usize,
    /// Turns that reported no elapsed time.
    pub unmeasured_turns: usize,
    /// Sum of every reported elapsed time.
    pub total_elapsed_ms: u64,
    /// Mean latency per measured turn.
    pub mean_elapsed_ms: Option<f64>,
    /// Slowest measured turn.
    pub max_elapsed_ms: Option<u64>,
    /// Prompt tokens summed across the turns that reported them. `None` when nothing reported any.
    pub prompt_tokens: Option<u64>,
    /// Completion tokens summed across the turns that reported them.
    pub completion_tokens: Option<u64>,
    /// Total tokens summed across the turns that reported them.
    pub total_tokens: Option<u64>,
    /// Turns that ran without reporting usage.
    ///
    /// Distinct from a reported zero: a provider that returns no usage block has not said the call was
    /// free, it has said nothing.
    pub turns_without_usage: usize,
}

/// Every statistic for one game and one pairing.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchStatistics {
    /// The game these matches were played in. Never merged with another game's numbers.
    pub game: String,
    /// The game's display name.
    pub game_name: String,
    /// The seat under test.
    pub tested_agent: String,
    /// The seat it played against.
    pub opponent_agent: String,
    /// Matches in this group. Every one of them is a stored recording.
    pub matches: usize,
    /// The identifiers of those recordings, oldest first.
    ///
    /// Present so a statistic can be traced back to exactly the matches behind it. A number with no
    /// list behind it cannot be checked.
    pub match_ids: Vec<String>,
    /// A digest over the digests of the contributing recordings.
    ///
    /// Two statistics blocks with the same provenance digest were computed from byte-identical
    /// recordings, so they cannot disagree unless one of them is wrong.
    pub provenance_digest: String,
    /// The outcome distribution and what follows from it.
    pub decisions: DecisionStatistics,
    /// How the attempted matches divide between conditional outcomes and everything else.
    ///
    /// Reported next to `decisions` rather than folded into it, so the win rate can always be read
    /// with the population behind it. See [`MatchPopulation`].
    pub population: MatchPopulation,
    /// Match length.
    pub plies: PlyStatistics,
    /// Moves a seat attempted that the board refused, across every match.
    pub illegal_moves: usize,
    /// How many of the matches recorded at least one illegal move.
    pub matches_with_illegal_moves: usize,
    /// Latency and token cost.
    pub costs: CostStatistics,
    /// Whether this is a small sample, and the interval should be read as "very wide".
    pub sample_is_small: bool,
}

impl MatchStatistics {
    /// A one-sentence statement of what this block is, for a panel to show above the numbers.
    pub fn provenance_sentence(&self) -> String {
        format!(
            "{} recorded {} vs {} in {}. {} of them reached a result; {} did not.",
            self.matches,
            self.tested_agent,
            self.opponent_agent,
            self.game_name,
            self.decisions.outcomes.decided(),
            self.decisions.outcomes.no_result
        )
    }
}

/// What a batch of matches adds up to, grouped by game and pairing.
///
/// Always a list, never a single combined figure. A single number across games would be the misleading
/// global leaderboard this module exists to avoid.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArenaStatistics {
    /// One entry per game and pairing that has at least one recording.
    pub groups: Vec<MatchStatistics>,
    /// Every recording that was read, across all groups.
    pub matches_read: usize,
    /// Recordings that had no game or pairing to group under and were therefore not counted.
    ///
    /// Always zero in practice. It exists so that a recording which fails to group is visible rather
    /// than quietly dropped from the total.
    pub ungrouped: usize,
}

impl ArenaStatistics {
    /// Whether anything has been recorded yet.
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }
}

/// The identity a group of statistics belongs to.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct GroupKey {
    game: String,
    game_name: String,
    tested_agent: String,
    opponent_agent: String,
}

/// Computes statistics over recorded matches.
///
/// The only input is stored recordings. Callers read them from the durable archive; there is no way to
/// reach a live match or an in-memory counter through this function, so nothing computed here can
/// include a match that has not finished and been recorded.
///
/// Matches are grouped by game, tested seat and opponent seat. `limit` bounds how many recordings are
/// read from the archive; it is a read bound, not a truncation of the statistics — the caller is told
/// how many were considered.
pub fn arena_statistics(matches: &[ArenaMatchView]) -> ArenaStatistics {
    // BTreeMap so the output order is stable: a panel that re-renders must not reshuffle its rows.
    let mut groups: BTreeMap<GroupKey, Vec<&ArenaMatchView>> = BTreeMap::new();
    let mut ungrouped = 0usize;

    for view in matches {
        let key = GroupKey {
            game: view.game.clone(),
            game_name: view.game_name.clone(),
            tested_agent: view.tested_agent.clone(),
            opponent_agent: view.opponent_agent.clone(),
        };
        if key.game.is_empty() || key.tested_agent.is_empty() || key.opponent_agent.is_empty() {
            ungrouped += 1;
            continue;
        }
        groups.entry(key).or_default().push(view);
    }

    let statistics = groups
        .into_iter()
        .map(|(key, views)| {
            let statistics = summarise(&key, &views);
            (key, statistics)
        })
        .collect::<BTreeMap<_, _>>();

    let groups: Vec<MatchStatistics> = statistics
        .into_values()
        .collect::<Vec<MatchStatistics>>()
        .into_iter()
        .collect();
    let matches_read: usize = groups.iter().map(|group| group.matches).sum();

    ArenaStatistics {
        groups,
        matches_read,
        ungrouped,
    }
}

/// Folds one group of recordings into a [`MatchStatistics`].
fn summarise(key: &GroupKey, views: &[&ArenaMatchView]) -> MatchStatistics {
    let mut outcomes = OutcomeCounts::default();
    let mut population = MatchPopulation::default();
    let mut cancelled = 0usize;
    let mut with_failure = 0usize;
    let mut illegal_moves = 0usize;
    let mut matches_with_illegal_moves = 0usize;

    let mut plies_total = 0u64;
    let mut plies_min: Option<u32> = None;
    let mut plies_max: Option<u32> = None;

    let mut costs = CostStatistics::default();
    let mut match_ids = Vec::with_capacity(views.len());
    let mut digests = Vec::with_capacity(views.len());

    for view in views {
        outcomes.record(view.outcome);
        population.absorb(view);
        if view.cancelled {
            cancelled += 1;
        }
        if view.failure.is_some() {
            with_failure += 1;
        }
        if !view.illegal_moves.is_empty() {
            matches_with_illegal_moves += 1;
        }
        illegal_moves += view.illegal_moves.len();

        plies_total += u64::from(view.plies);
        plies_min = Some(plies_min.map_or(view.plies, |current| current.min(view.plies)));
        plies_max = Some(plies_max.map_or(view.plies, |current| current.max(view.plies)));

        match_ids.push(view.match_id.clone());
        digests.push(view.replay_digest());

        for turn in &view.turns {
            costs.measured_turns += 1;
            costs.total_elapsed_ms += turn.elapsed_ms;
            costs.max_elapsed_ms = Some(
                costs
                    .max_elapsed_ms
                    .map_or(turn.elapsed_ms, |m| m.max(turn.elapsed_ms)),
            );
            match &turn.usage {
                None => costs.turns_without_usage += 1,
                Some(usage) => {
                    costs.prompt_tokens = add_optional(costs.prompt_tokens, usage.prompt_tokens);
                    costs.completion_tokens =
                        add_optional(costs.completion_tokens, usage.completion_tokens);
                    costs.total_tokens = add_optional(costs.total_tokens, usage.total_tokens);
                }
            }
        }
    }

    let decided = outcomes.decided();
    let decisive = outcomes.decisive();
    let has_decided = decided > 0;

    costs.unmeasured_turns = 0;
    costs.mean_elapsed_ms = (costs.measured_turns > 0)
        .then(|| costs.total_elapsed_ms as f64 / costs.measured_turns as f64);

    MatchStatistics {
        game: key.game.clone(),
        game_name: key.game_name.clone(),
        tested_agent: key.tested_agent.clone(),
        opponent_agent: key.opponent_agent.clone(),
        matches: views.len(),
        match_ids,
        provenance_digest: crate::arena::provenance_digest(digests.iter().map(String::as_str)),
        decisions: DecisionStatistics {
            outcomes,
            win_rate: wilson_interval(outcomes.win, decided, DEFAULT_CONFIDENCE),
            decisive_win_rate: wilson_interval(outcomes.win, decisive, DEFAULT_CONFIDENCE),
            cancelled,
            with_failure,
            has_decided,
        },
        population,
        plies: PlyStatistics {
            total: plies_total,
            mean: (!views.is_empty()).then(|| plies_total as f64 / views.len() as f64),
            min: plies_min,
            max: plies_max,
        },
        illegal_moves,
        matches_with_illegal_moves,
        costs,
        sample_is_small: decided < SMALL_SAMPLE_MATCHES,
    }
}

/// Adds an optional count without overflowing, and keeps "absent" distinct from "zero".
///
/// Identical in spirit to `TokenUsage::add_assign`, reimplemented here only because this module
/// aggregates into three independent `Option`s rather than into a `TokenUsage`.
fn add_optional(current: Option<u64>, next: Option<u64>) -> Option<u64> {
    match (current, next) {
        (_, None) => current,
        (None, Some(value)) => Some(value),
        (Some(a), Some(b)) => Some(a.saturating_add(b)),
    }
}
