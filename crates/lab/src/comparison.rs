//! Cross-experiment comparison.
//!
//! # The one idea
//!
//! Two runs can only be compared if they measured the same thing. A success rate from a different
//! benchmark, a different model, or a different seed is not a better or worse result — it is a
//! different measurement, and putting the two numbers in one table invites exactly the wrong reading.
//! So this module refuses to produce a comparison it cannot stand behind, and names the dimension
//! that differs.
//!
//! That refusal is the feature. A comparison panel that silently averaged across benchmarks, or that
//! ranked a connected run above a mock one because one had more episodes, would be the most
//! confident and least defensible surface in the product.
//!
//! # What is compared
//!
//! Only quantities the infrastructure produced: success fraction with its Wilson interval,
//! environment-generated reward and turns, invalid actions, and the two integrity totals kept
//! strictly apart. Nothing here reads a model-reported claim. The comparison inherits the product's
//! rule that only infrastructure-classified facts are evidence.
//!
//! Episode count deliberately does *not* block a comparison — a longer run of the same experiment is
//! exactly what someone wants to compare — but the interval is reported alongside every success
//! fraction, so a one-episode run is visibly weaker evidence rather than a better agent.
//!
//! # Score absence
//!
//! A run with no completed episode has no success fraction, and it is carried as `None` through the
//! whole comparison. It is never coerced to `0.0` and never excluded silently: it appears in the
//! standings with its reason, because "this run produced no measurement" is itself the most important
//! thing a comparison can tell someone.

use std::cmp::Ordering;

use probelab_core::TestResult;
use probelab_engine::harness::{wilson_interval, Interval};
use serde::{Deserialize, Serialize};

/// One run as the comparison sees it.
///
/// A projection rather than the stored bundle: a comparison that carried whole results would let a
/// panel render fields the comparison does not reason about, and each of those would need its own
/// honesty argument.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComparableRun {
    /// The stored experiment this came from.
    pub experiment_id: String,
    /// The benchmark measured.
    pub benchmark_id: String,
    /// The agent measured.
    pub target_agent: String,
    /// The model adapter that produced the actions.
    pub adapter: String,
    /// The named strategy within that adapter.
    pub strategy: String,
    /// The provider that produced the actions, or `None` when the run recorded none.
    ///
    /// Part of the *identity* of the measurement, not a quality. Two runs of the same benchmark at the
    /// same seed by two different models are not comparable at all, and a comparison that ranked them
    /// would be answering a question nobody asked. `None` is kept distinct from any string a provider
    /// could report, so a run whose provider was never recorded cannot be silently equated with a
    /// recorded one -- it blocks, which is the safe direction.
    pub provider: Option<String>,
    /// The provider's model identifier, or `None` when the run recorded none.
    ///
    /// Same reasoning as [`Self::provider`], and the pair is what a reader actually needs: a provider
    /// name without a model is a vendor, and vendors ship several models whose behaviour differs.
    pub model: Option<String>,
    /// The content digest of the benchmark definition, or `None` for a run recorded before it existed.
    ///
    /// Blocks for the same reason `benchmark_id` does, and for an additional one: `benchmark_version`
    /// is a string a person maintains, and a definition can change without it. Ranking a run of the
    /// old task against a run of the new one is the failure this whole field exists to prevent.
    pub benchmark_definition_digest: Option<String>,
    /// The root seed. Two runs with different seeds are not comparable.
    pub root_seed: u64,
    /// Episodes that completed and produced a result.
    pub episodes_completed: usize,
    /// Episodes that failed outright.
    pub episodes_failed: usize,
    /// Success fraction, or `None` when no episode completed.
    pub success_fraction: Option<f64>,
    /// The Wilson interval for that fraction, present only when there is a fraction.
    pub success_interval: Option<Interval>,
    /// Integrity events the infrastructure classified.
    pub integrity_events: u32,
    /// Broker refusals, kept apart from integrity events.
    pub broker_refusals: u32,
    /// Whether any integrity event was a real boundary finding.
    pub is_security_finding: bool,
    /// Mean environment-generated reward, across completed episodes.
    pub mean_reward: f64,
    /// Mean turns consumed.
    pub mean_turns: f64,
    /// Mean invalid actions.
    pub mean_invalid_actions: f64,
}

/// The provider identifier the deterministic mock reports.
///
/// Duplicated as a literal rather than imported from `probelab-model-adapters`, because `probelab-lab`
/// deliberately does not depend on the adapter crates: this module decides what a comparison may claim
/// from stored data, and taking a constant from the thing it is judging would invert that. The two
/// must agree, and a test in `crates/lab/tests/comparison.rs` asserts the mock runs this module sees
/// really do carry this value.
const DETERMINISTIC_MOCK_PROVIDER: &str = "deterministic-mock";

/// An owned non-empty string, or `None`.
///
/// Empty is treated as absent because a run that recorded an empty provider recorded nothing, and
/// storing Some("") would make "recorded as blank" look like a real identity to every check above.
fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// How many distinct values were actually recorded across a set of runs.
///
/// The unit the identity checks are written in, and it is what makes *absence* different from
/// *difference*. A run that recorded no provider has not claimed a different subject from anyone; it
/// has simply not said. Counting only the recorded values means such a run neither blocks a comparison
/// on its own nor gets quietly equated with a recorded identity -- it is ignored, and the run is
/// already unranked for having measured nothing.
///
/// Two runs, one recorded as `anthropic` and one unrecorded, give 1. Two runs recorded as `anthropic`
/// and `openai` give 2, and block.
fn distinct_recorded(values: impl Iterator<Item = Option<String>>) -> usize {
    let mut distinct: Vec<String> = Vec::new();
    for value in values.flatten() {
        if !distinct.contains(&value) {
            distinct.push(value);
        }
    }
    distinct.len()
}

/// Formats an optional identity for comparison, so a recorded value never equals an absent one.
///
/// NUL is used as the absent marker because no provider id, model id, or content digest can contain
/// it -- they are vendor strings and lowercase hex. A spelled-out placeholder like "unknown" would
/// work too and would be more readable in a stored comparison, but it would also be a string a
/// confused upstream could genuinely produce, and this function's entire job is to make "not
/// recorded" impossible to confuse with "recorded as something".
fn identity_value(value: Option<&str>) -> String {
    value.map_or_else(|| "\u{0}not recorded".to_owned(), str::to_owned)
}
impl ComparableRun {
    /// Projects the stored episode results of one run into a comparable run.
    ///
    /// # Why the episode list is passed in
    ///
    /// A `TestResult` is one episode. A success fraction is a property of *several*, so computing
    /// one from a single bundle would report 100% or 0% for every run and turn the comparison into a
    /// comparison of episode counts. The bundles are the authority for what happened inside one
    /// episode; the caller supplies which episodes belong to the run.
    pub fn from_results(
        experiment_id: impl Into<String>,
        benchmark_id: impl Into<String>,
        target_agent: impl Into<String>,
        root_seed: u64,
        episodes: &[TestResult],
        episodes_failed: usize,
    ) -> Self {
        let completed = episodes.len();
        let successes = episodes
            .iter()
            .filter(|result| result.capability.task_success)
            .count();

        // Two counts, from two disjoint sets of categories, and never summed.
        //
        // `total_events()` includes the five broker categories, so using it for the "integrity events"
        // column counted every broker refusal *twice* -- once here and once in `broker_refusals` -- and
        // then set `is_security_finding` from that inflated number. The consequence was the exact
        // inversion this product is built to avoid: a run in which the broker correctly refused five
        // requests and nothing else happened reported a security finding. The broker's own refusal
        // proved the boundary held.
        //
        // `SecuritySummary::from_results` in `view.rs` and `split_by_layer` in `security.rs` both
        // already route these categories correctly. Two of the three aggregate paths were right and
        // this one was wrong, which is the worst possible distribution of a defect: the correct
        // implementations are not evidence about the incorrect one.
        let integrity_events: u32 = episodes
            .iter()
            .map(|result| result.integrity.total_policy_violations())
            .sum();
        let broker_refusals: u32 = episodes
            .iter()
            .map(|result| result.integrity.total_broker_denials())
            .sum();
        // A finding is a *policy* violation only. A broker refusal is the control working.
        let is_security_finding = integrity_events > 0;

        // No completed episode: no model descriptor to read and nothing to average. Every
        // measurement is absent rather than zero, because a run that measured nothing is not a run
        // that scored zero.
        let (adapter, strategy) = episodes
            .first()
            .map(|result| (result.model.adapter.clone(), result.model.strategy.clone()))
            .unwrap_or_else(|| ("none".to_owned(), "none".to_owned()));

        /*
         * Who produced the actions, and which task they were produced against.
         *
         * Read from the first episode's own recorded reproducibility metadata rather than from the
         * descriptor, because that metadata is what was *persisted with the result* -- it is the same
         * string a reader of the stored bundle will see, rather than something reconstructed here.
         *
         * A run with no completed episode has no identity at all, and that is `None` rather than a
         * placeholder. `None` blocks the comparison, which is right: a run that measured nothing
         * cannot be ranked alongside one that did, and quietly equating its identity with another run's
         * would let that through.
         */
        let (provider, model, benchmark_definition_digest) = episodes
            .first()
            .map(|result| {
                let provider = non_empty(&result.reproducibility.provider);
                let model = non_empty(&result.reproducibility.model);
                let digest = result
                    .benchmark_definition_digest
                    .as_deref()
                    .and_then(non_empty);
                (provider, model, digest)
            })
            .unwrap_or((None, None, None));

        // An `if` rather than `then(...).unwrap_or(...)`: with no completed episode there is nothing
        // to average, and the chain form reads as though a zero were a computed mean.
        let mean_of = |read: fn(&TestResult) -> f64| {
            if completed > 0 {
                episodes.iter().map(read).sum::<f64>() / completed as f64
            } else {
                0.0
            }
        };

        Self {
            experiment_id: experiment_id.into(),
            benchmark_id: benchmark_id.into(),
            target_agent: target_agent.into(),
            adapter,
            strategy,
            provider,
            model,
            benchmark_definition_digest,
            root_seed,
            episodes_completed: completed,
            episodes_failed,
            success_fraction: (completed > 0).then(|| successes as f64 / completed as f64),
            success_interval: (completed > 0).then(|| wilson_interval(successes, completed, 0.95)),
            integrity_events,
            broker_refusals,
            is_security_finding,
            mean_reward: mean_of(|result| result.capability.total_reward as f64),
            mean_turns: mean_of(|result| result.capability.turns_used as f64),
            mean_invalid_actions: mean_of(|result| result.capability.invalid_action_count as f64),
        }
    }
}

/// Why two runs cannot be compared.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IncomparableDimension {
    /// Which dimension differs: `benchmark`, `target_agent`, `adapter`, `strategy`, or `root_seed`.
    pub dimension: String,
    /// The values that differ, in the order the runs were given.
    pub values: Vec<String>,
}

/// A comparison that could not be made, and why.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Incomparable {
    /// The runs that were asked about.
    pub experiment_ids: Vec<String>,
    /// Every dimension that differs.
    pub dimensions: Vec<IncomparableDimension>,
}

impl Incomparable {
    /// A one-line reason, for a panel with room for one line.
    pub fn reason(&self) -> String {
        self.dimensions
            .iter()
            .map(|dimension| {
                format!(
                    "{} differs ({})",
                    dimension.dimension,
                    dimension.values.join(" vs ")
                )
            })
            .collect::<Vec<_>>()
            .join("; ")
    }
}

/// A run's standing relative to the others on one dimension.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Standing {
    /// The run this standing is about.
    pub experiment_id: String,
    /// Rank within the comparable set, 1 being best. `None` when the run has nothing to rank.
    pub rank: Option<usize>,
    /// The value that was ranked.
    pub value: Option<f64>,
    /// The Wilson interval, when the value is a success fraction.
    pub interval: Option<Interval>,
    /// Why the run has no rank, when it has none.
    pub unranked_reason: Option<String>,
}

/// A comparison that can be defended.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Comparison {
    /// The runs, in the order they were given.
    pub runs: Vec<ComparableRun>,
    /// The benchmark every run measured, stated so a panel can show what it is comparing.
    pub benchmark_id: String,
    /// The agent every run measured.
    pub target_agent: String,
    /// The adapter every run used.
    pub adapter: String,
    /// The strategy every run used.
    pub strategy: String,
    /// The seed every run used.
    pub root_seed: u64,
    /// Standings by success fraction, best first.
    pub by_success: Vec<Standing>,
    /// Standings by mean reward, best first.
    pub by_reward: Vec<Standing>,
    /// Standings by mean turns, fewest first.
    pub by_turns: Vec<Standing>,
    /// Standings by mean invalid actions, fewest first.
    pub by_invalid_actions: Vec<Standing>,
    /// Integrity events across the whole comparable set.
    pub integrity_event_total: u32,
    /// Broker refusals across the whole comparable set.
    pub broker_refusal_total: u32,
    /// Whether any run in the set recorded a real boundary finding.
    pub any_security_finding: bool,
    /// Identities that were *not* held constant across the runs.
    ///
    /// Non-empty means the ranking is qualified: a panel must show this above the numbers, because
    /// ranking two different agents is a legitimate thing to want and an illegitimate thing to do
    /// silently. Empty means every run differed only in how many episodes it had.
    pub varies_across: Vec<IncomparableDimension>,
}

/// The outcome of asking for a comparison.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ComparisonOutcome {
    /// The runs are comparable, and here is the comparison.
    Comparable(Box<Comparison>),
    /// The runs differ in a way that makes comparison meaningless.
    NotComparable(Incomparable),
}

/// Ranks runs on one dimension, best first, leaving unscorable runs visible and unranked.
///
/// A tie shares a rank. Two identical measurements are not one better than the other, and reporting
/// `1` and `2` for equal values would make the ranking look more precise than the data supports.
fn rank(
    runs: &[ComparableRun],
    value: impl Fn(&ComparableRun) -> Option<f64>,
    better: impl Fn(f64, f64) -> Ordering,
    interval: impl Fn(&ComparableRun) -> Option<Interval>,
    dimension: &str,
) -> Vec<Standing> {
    let mut scored: Vec<(usize, f64)> = runs
        .iter()
        .enumerate()
        .filter_map(|(index, run)| value(run).map(|value| (index, value)))
        .collect();
    scored.sort_by(|left, right| better(left.1, right.1));

    let mut standings: Vec<Standing> = scored
        .iter()
        .enumerate()
        .map(|(position, (index, value))| Standing {
            experiment_id: runs[*index].experiment_id.clone(),
            // Position is 1-based, and a tie reuses the rank of the value it equals.
            rank: Some(if position > 0 && scored[position - 1].1 == *value {
                position
            } else {
                position + 1
            }),
            value: Some(*value),
            interval: interval(&runs[*index]),
            unranked_reason: None,
        })
        .collect();

    for run in runs {
        if value(run).is_none() {
            standings.push(Standing {
                experiment_id: run.experiment_id.clone(),
                rank: None,
                value: None,
                interval: None,
                unranked_reason: Some(format!(
                    "{} completed no episode, so it has no {dimension} to rank",
                    run.experiment_id
                )),
            });
        }
    }
    standings
}

/// Compares runs, refusing when they did not measure the same thing.
///
/// # What blocks a comparison, and what only qualifies it
///
/// Two different *measurement identities* — a different benchmark or a different root seed — mean
/// the numbers are not about the same thing at all. Those are refused outright.
///
/// A different *agent*, adapter, or strategy is a confound, not a blocker. Refusing those would make
/// the feature useless, since comparing two agents is the obvious thing someone wants. But the
/// comparison is only honest if the confound travels with it, so every differing identity is
/// reported in [`Comparison::varies_across`] and the panel is required to show it above the
/// ranking. Silently ranking a mock against a connected model is the failure; refusing to let anyone
/// rank two models is the other failure, and it is not this product's call to make on the user's
/// behalf.
///
/// Episode count never blocks and never qualifies: a longer run of the same experiment is exactly
/// what someone wants to compare, and the Wilson interval is there to show the difference in
/// confidence.
pub fn compare(runs: Vec<ComparableRun>) -> ComparisonOutcome {
    if runs.len() < 2 {
        return ComparisonOutcome::NotComparable(Incomparable {
            experiment_ids: runs.iter().map(|run| run.experiment_id.clone()).collect(),
            dimensions: vec![IncomparableDimension {
                dimension: "run_count".to_owned(),
                values: vec![runs.len().to_string()],
            }],
        });
    }

    // The measurement identity. A difference here means these runs are not comparable at all.
    let mut blocking: Vec<IncomparableDimension> = Vec::new();
    let mut check = |dimension: &str, value_of: fn(&ComparableRun) -> String| {
        let values: Vec<String> = runs.iter().map(&value_of).collect();
        if values.iter().any(|value| value != &values[0]) {
            blocking.push(IncomparableDimension {
                dimension: dimension.to_owned(),
                values,
            });
        }
    };
    check("benchmark", |run| run.benchmark_id.clone());
    check("root_seed", |run| run.root_seed.to_string());
    /*
     * The subject, not the instrument.
     *
     * `adapter` and `strategy` are confounds reported in `varies_across`, because the lab's own
     * harness is the same everywhere and a difference there qualifies a comparison without voiding
     * it. A difference in provider or model is a different kind of fact: it means the two runs
     * measured two different things, and a ranking across them would be a claim about neither.
     *
     * This was previously not checked *at all* -- `ComparableRun` had no provider or model field, so
     * a comparison could not see which model produced a result even to warn about it. Two connected
     * runs of different models, same benchmark and seed, ranked as comparable with the difference
     * invisible. That is the gap this closes.
     *
     * # Absence is not difference
     *
     * Both checks count *distinct recorded values*, so a run that recorded nothing does not block on
     * its own. A run with no completed episodes has an identity only as far as the store recorded one,
     * and refusing to compare it with anything would make every such comparison impossible -- which is
     * a comparison someone reasonably wants, in order to see that run listed as unranked rather than
     * quietly ranked on another's results. A run that measured nothing cannot be mis-ranked into a
     * position, because it is already excluded from every ranking.
     *
     * # Why the model check is exempt for the mock, and why that is not a loophole
     *
     * The deterministic mock reports `model` as a string derived from its own strategy: `optimal`
     * becomes `native-mock-optimal`, `random` becomes `native-mock-random`. For two mock runs the
     * model field is not an independent subject at all -- it is a second name for `target_agent`, which
     * is already a *declared confound* the panel shows above the ranking. Blocking on it would refuse
     * the product's core capability, comparing two agents on one benchmark and seed, over a difference
     * the comparison already reports and qualifies.
     *
     * So the model check is skipped when every recorded provider is the deterministic mock. The
     * provider check still runs, and still blocks a mock run against a connected one, because there the
     * difference really is a different subject. The boundary is the honest one: `model` is load-bearing
     * only when it names something the agent did not already name.
     */
    if distinct_recorded(runs.iter().map(|run| run.provider.clone())) > 1 {
        check("provider", |run| identity_value(run.provider.as_deref()));
    }
    let every_recorded_provider_is_the_mock = runs
        .iter()
        .all(|run| run.provider.as_deref() == Some(DETERMINISTIC_MOCK_PROVIDER));
    if !every_recorded_provider_is_the_mock
        && distinct_recorded(runs.iter().map(|run| run.model.clone())) > 1
    {
        check("model", |run| identity_value(run.model.as_deref()));
    }

    /*
     * The task, which blocks regardless of provider.
     *
     * Same absence-is-not-difference rule, and for the same reason: a stored bundle from before the
     * digest existed has not said what task it ran, and refusing to compare it with anything would make
     * every pre-existing result unrankable rather than merely qualified.
     *
     * Where digests *are* recorded and differ, the task genuinely changed -- a seed pool grew, a turn
     * budget moved, the reward function changed without anyone bumping the version string -- and
     * ranking across that boundary is the exact failure this identifier exists to prevent.
     */
    if distinct_recorded(
        runs.iter()
            .map(|run| run.benchmark_definition_digest.clone()),
    ) > 1
    {
        check("benchmark_definition_digest", |run| {
            identity_value(run.benchmark_definition_digest.as_deref())
        });
    }

    if !blocking.is_empty() {
        return ComparisonOutcome::NotComparable(Incomparable {
            experiment_ids: runs.iter().map(|run| run.experiment_id.clone()).collect(),
            dimensions: blocking,
        });
    }

    // The confounds: differences that qualify the comparison instead of voiding it. Reported so a
    // panel cannot render a ranking without also saying what was not held constant.
    let mut varies_across: Vec<IncomparableDimension> = Vec::new();
    let mut note_confound = |dimension: &str, value_of: fn(&ComparableRun) -> String| {
        let values: Vec<String> = runs.iter().map(&value_of).collect();
        if values.iter().any(|value| value != &values[0]) {
            varies_across.push(IncomparableDimension {
                dimension: dimension.to_owned(),
                values,
            });
        }
    };
    note_confound("target_agent", |run| run.target_agent.clone());
    note_confound("adapter", |run| run.adapter.clone());
    note_confound("strategy", |run| run.strategy.clone());

    let first = &runs[0];
    let higher_is_better =
        |left: f64, right: f64| right.partial_cmp(&left).unwrap_or(Ordering::Equal);
    let lower_is_better =
        |left: f64, right: f64| left.partial_cmp(&right).unwrap_or(Ordering::Equal);
    let completed = |run: &ComparableRun| (run.episodes_completed > 0).then_some(());

    let integrity_event_total = runs.iter().map(|run| run.integrity_events).sum();
    let broker_refusal_total = runs.iter().map(|run| run.broker_refusals).sum();
    let any_security_finding = runs.iter().any(|run| run.is_security_finding);

    let by_success = rank(
        &runs,
        |run| run.success_fraction,
        higher_is_better,
        |run| run.success_interval,
        "success fraction",
    );
    let by_reward = rank(
        &runs,
        |run| completed(run).map(|()| run.mean_reward),
        higher_is_better,
        |_| None,
        "mean reward",
    );
    let by_turns = rank(
        &runs,
        |run| completed(run).map(|()| run.mean_turns),
        lower_is_better,
        |_| None,
        "mean turns",
    );
    let by_invalid_actions = rank(
        &runs,
        |run| completed(run).map(|()| run.mean_invalid_actions),
        lower_is_better,
        |_| None,
        "mean invalid actions",
    );

    ComparisonOutcome::Comparable(Box::new(Comparison {
        benchmark_id: first.benchmark_id.clone(),
        // The single-agent fields are only the shared value when the runs agree. When they differ,
        // the comparison is still produced but `varies_across` says so, and these carry the first
        // run's values for a caller that wants a label.
        target_agent: first.target_agent.clone(),
        adapter: first.adapter.clone(),
        strategy: first.strategy.clone(),
        root_seed: first.root_seed,
        runs,
        by_success,
        by_reward,
        by_turns,
        by_invalid_actions,
        integrity_event_total,
        broker_refusal_total,
        any_security_finding,
        varies_across,
    }))
}
