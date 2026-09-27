//! Multi-episode capability measurement for ProbeLab.
//!
//! # Why this exists
//!
//! A single episode produces a single number with no error bar, and the grid-world audit found that
//! is exactly what the previous version of this benchmark did: one hardcoded seed, one pass or fail,
//! and no baseline to compare against. A binary outcome from `n = 1` is an anecdote. This module is
//! the smallest thing that makes the number mean something: many episodes, over seeds that were held
//! out from development, scored as a rate with a confidence interval, and reported next to the two
//! trivial policies so there is a floor to beat.
//!
//! # Scope: this is a library and CLI surface, not the desktop app
//!
//! [`Harness::measure`] runs N episodes and returns an [`AggregateScore`]. It is deliberately **not**
//! wired into the Tauri `run_benchmark` command, which still runs exactly one episode against one
//! seed. The desktop app therefore does not show a Wilson interval, does not show a success rate, and
//! does not show a baseline. That gap is real and is follow-on UI work; it is called out here so the
//! existence of this module is not mistaken for the app having gained a statistics panel.
//!
//! # Intervals
//!
//! The interval is a Wilson score interval rather than the normal approximation. At the sample sizes
//! this harness actually runs — 30 to 70 episodes, and success rates that can sit at or near 0 or 1
//! for a weak policy — the normal approximation is at its worst exactly where it matters. Wilson
//! stays inside `[0, 1]` and keeps sensible width at a 0 % or 100 % rate, which a
//! `p ± 1.96·se` formula does not.

#![deny(missing_docs)]

use std::collections::BTreeMap;
use std::sync::Arc;

use probelab_core::{Agent, Benchmark, CompletionStatus, Evaluator};
use serde::{Deserialize, Serialize};

use crate::BenchmarkRunner;

/// The default number of episodes a measurement runs.
///
/// Thirty is the smallest sample whose Wilson interval is narrow enough to distinguish a policy that
/// solves the task from one that does not. A caller may run fewer for a smoke check, but a reported
/// capability number should not come from fewer.
pub const DEFAULT_EPISODES: usize = 30;

/// The confidence level used for the reported interval.
pub const DEFAULT_CONFIDENCE: f64 = 0.95;

/// Which pool a measurement draws seeds from.
///
/// The two pools are disjoint by construction in the benchmark definition and that disjointness is
/// validated when the definition loads, so a measurement physically cannot draw a development seed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SeedPool {
    /// Seeds reserved for building and tuning the benchmark. Never a measurement.
    Dev,
    /// Seeds reserved for capability measurement.
    Eval,
}

impl SeedPool {
    /// Returns the stable name of this pool.
    pub fn name(self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Eval => "eval",
        }
    }
}

/// A validated, non-overlapping pair of seed pools.
#[derive(Clone, Debug)]
pub struct SeedPools {
    dev: Vec<u64>,
    eval: Vec<u64>,
}

impl SeedPools {
    /// Builds a pair of pools, refusing any seed that appears in both.
    ///
    /// This is checked here as well as in the benchmark definition because the harness is the thing
    /// that actually draws seeds, and a measurement that silently included a tuned seed would be
    /// worse than no measurement at all.
    pub fn new(dev: Vec<u64>, eval: Vec<u64>) -> Result<Self, HarnessError> {
        for seed in &eval {
            if dev.contains(seed) {
                return Err(HarnessError::OverlappingSeedPools { seed: *seed });
            }
        }
        if dev.is_empty() {
            return Err(HarnessError::EmptySeedPool { pool: "dev" });
        }
        if eval.is_empty() {
            return Err(HarnessError::EmptySeedPool { pool: "eval" });
        }
        Ok(Self { dev, eval })
    }

    /// Returns the seeds in one pool.
    pub fn seeds(&self, pool: SeedPool) -> &[u64] {
        match pool {
            SeedPool::Dev => &self.dev,
            SeedPool::Eval => &self.eval,
        }
    }

    /// Reports whether the two pools share no seed.
    pub fn are_disjoint(&self) -> bool {
        self.eval.iter().all(|seed| !self.dev.contains(seed))
    }
}

/// How a measurement is configured.
#[derive(Clone, Debug)]
pub struct MeasurementPlan {
    /// Which pool to draw seeds from.
    pub pool: SeedPool,
    /// How many episodes to run.
    pub episodes: usize,
    /// Confidence level for the reported interval.
    pub confidence: f64,
}

impl Default for MeasurementPlan {
    fn default() -> Self {
        Self {
            pool: SeedPool::Eval,
            episodes: DEFAULT_EPISODES,
            confidence: DEFAULT_CONFIDENCE,
        }
    }
}

impl MeasurementPlan {
    /// Returns a plan running `episodes` episodes over the evaluation pool.
    pub fn eval(episodes: usize) -> Self {
        Self {
            episodes,
            ..Self::default()
        }
    }

    /// Returns the plan's episodes, refusing zero.
    pub fn validated(&self) -> Result<usize, HarnessError> {
        if self.episodes == 0 {
            return Err(HarnessError::NoEpisodesRequested);
        }
        if !(0.0..1.0).contains(&self.confidence) {
            return Err(HarnessError::InvalidConfidence {
                confidence: self.confidence,
            });
        }
        Ok(self.episodes)
    }
}

/// The outcome of one episode inside a measurement.
#[derive(Clone, Debug)]
pub struct EpisodeOutcome {
    /// The seed this episode ran.
    pub seed: u64,
    /// How the episode ended.
    pub completion_status: CompletionStatus,
    /// Whether the benchmark's own evaluator judged the task complete.
    ///
    /// This is the value the success rate counts, and it is deliberately **not** the same thing as
    /// `completion_status == Success`. For the grid world the two coincide, so nothing changes there.
    /// They part company the moment a benchmark's success condition is stricter than "the episode
    /// ended well" — the sandbox-process benchmark requires a contained child to have run *and* the
    /// goal to be reached, and an episode can absolutely reach the goal without the child.
    ///
    /// Counting the terminal status instead would silently report a policy as succeeding at a
    /// benchmark whose actual task it never performed.
    pub task_success: bool,
    /// The shaped reward, which distinguishes partial performance among non-successes.
    pub total_reward: i64,
    /// Turns the episode consumed.
    pub turns_used: u32,
}

/// A single scalar with a confidence interval.
///
/// `Default` and the serde derives are here because an interval reaches result bundles and read
/// models, and a type that cannot be serialised cannot be reported. The default is a degenerate
/// zero-width interval at zero, which is why a caller should treat an interval from a run with no
/// episodes as absent rather than as a measurement.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct Interval {
    /// The point estimate.
    pub point: f64,
    /// Lower bound.
    pub lower: f64,
    /// Upper bound.
    pub upper: f64,
    /// The confidence level the bounds correspond to.
    pub confidence: f64,
}

/// The aggregate of a measurement.
#[derive(Clone, Debug)]
pub struct AggregateScore {
    /// A stable label for whatever was measured.
    pub label: String,
    /// Episodes actually run.
    pub episodes: usize,
    /// Episodes that reached the goal.
    pub successes: usize,
    /// Success rate with its Wilson interval.
    pub success_rate: Interval,
    /// Mean shaped reward across every episode.
    pub mean_reward: f64,
    /// Mean turns consumed across every episode.
    pub mean_turns: f64,
    /// Counts per terminal status, so a failure mode is visible rather than lumped together.
    pub status_counts: BTreeMap<String, usize>,
}

impl AggregateScore {
    /// Returns the success rate as a plain fraction.
    pub fn success_fraction(&self) -> f64 {
        self.success_rate.point
    }
}

/// A harness failure that is not a benchmark or agent failure.
#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    /// A seed appeared in both pools.
    #[error("seed {seed} is present in both the dev and eval pools")]
    OverlappingSeedPools {
        /// The offending seed.
        seed: u64,
    },
    /// A pool was empty.
    #[error("the {pool} seed pool is empty")]
    EmptySeedPool {
        /// Name of the empty pool.
        pool: &'static str,
    },
    /// A plan asked for zero episodes.
    #[error("a measurement must run at least one episode")]
    NoEpisodesRequested,
    /// A plan supplied an impossible confidence level.
    #[error("confidence must be strictly between 0 and 1, got {confidence}")]
    InvalidConfidence {
        /// The rejected level.
        confidence: f64,
    },
    /// The engine refused or failed an episode.
    #[error("episode {seed} failed: {reason}")]
    EpisodeFailed {
        /// The seed whose episode failed.
        seed: u64,
        /// Why it failed.
        reason: String,
    },
}

/// The Wilson score interval for a binomial proportion.
///
/// Chosen over the normal approximation because it is well behaved at the boundaries this benchmark
/// actually hits: a policy that never succeeds and a policy that always succeeds both produce an
/// interval of sensible width here, where `p ± 1.96·se` would collapse to zero width or run past
/// `[0, 1]`.
pub fn wilson_interval(successes: usize, trials: usize, confidence: f64) -> Interval {
    if trials == 0 {
        return Interval {
            point: 0.0,
            lower: 0.0,
            upper: 1.0,
            confidence,
        };
    }
    let n = trials as f64;
    let successes = successes as f64;
    let phat = successes / n;
    // z for a two-sided interval at the requested confidence. Only the handful of levels anyone
    // actually uses are tabulated; an unlisted level falls back to 95 %, which is stated rather than
    // silently pretending to be exact.
    let z = z_for(confidence);
    let z2 = z * z;
    let denominator = 1.0 + z2 / n;
    let centre = phat + z2 / (2.0 * n);
    let spread = z * ((phat * (1.0 - phat) / n) + (z2 / (4.0 * n * n))).sqrt();
    Interval {
        point: phat,
        lower: ((centre - spread) / denominator).clamp(0.0, 1.0),
        upper: ((centre + spread) / denominator).clamp(0.0, 1.0),
        confidence,
    }
}

fn z_for(confidence: f64) -> f64 {
    // Two-sided normal quantiles for the levels this project uses.
    const TABLE: [(f64, f64); 5] = [
        (0.80, 1.281_551_565_544_600_4),
        (0.90, 1.644_853_626_951_472_2),
        (0.95, 1.959_963_984_540_054),
        (0.98, 2.326_347_874_040_840_8),
        (0.99, 2.575_829_303_548_900_4),
    ];
    for (level, z) in TABLE {
        if (confidence - level).abs() < f64::EPSILON {
            return z;
        }
    }
    1.959_963_984_540_054
}

/// Runs episodes and aggregates them into a scored result.
///
/// The harness owns no policy of its own. Every measurement is a closure that builds a fresh agent
/// for a given seed, so a scripted baseline and a real model travel the identical path: the same
/// benchmark, the same engine, the same telemetry, the same evaluator. A baseline measured any other
/// way would not be a baseline.
#[derive(Clone)]
pub struct Harness {
    pools: SeedPools,
    recorder_factory: RecorderFactory,
}

/// Builds the telemetry recorder one episode writes into.
///
/// The harness takes this from its caller rather than choosing a recorder itself, so an episode's
/// telemetry is produced by exactly the same recorder a normal run would use. A measurement that
/// recorded to a different place, or to nothing, would not be comparable with a real run.
pub type RecorderFactory =
    Arc<dyn Fn(&str) -> Arc<dyn probelab_core::TelemetryRecorder> + Send + Sync>;

impl Harness {
    /// Creates a harness over a pair of disjoint seed pools.
    pub fn new(pools: SeedPools, recorder_factory: RecorderFactory) -> Self {
        Self {
            pools,
            recorder_factory,
        }
    }

    /// Returns the seed pools this harness draws from.
    pub fn pools(&self) -> &SeedPools {
        &self.pools
    }

    /// Measures one policy over `plan.episodes` seeds from `plan.pool`.
    ///
    /// `build_agent` is called once per episode with that episode's seed, so no state can leak from
    /// one episode into the next. That matters: a policy that accumulated memory across episodes
    /// would be measuring something no deployed model does.
    pub fn measure(
        &self,
        benchmark: &dyn Benchmark,
        evaluator: &dyn Evaluator,
        runner: &BenchmarkRunner,
        plan: &MeasurementPlan,
        label: &str,
        build_agent: &mut dyn FnMut(u64) -> Box<dyn Agent>,
    ) -> Result<AggregateScore, HarnessError> {
        let episodes = plan.validated()?;
        let seeds = self.pools.seeds(plan.pool);
        let mut outcomes = Vec::with_capacity(episodes);
        for index in 0..episodes {
            // Cycling the pool means a plan larger than the pool revisits seeds, which is stated
            // rather than hidden: the reported episode count is always the number actually run.
            let seed = seeds[index % seeds.len()];
            let agent = build_agent(seed);
            let pool = plan.pool.name();
            let run_id = format!("{label}-{pool}-{index}-seed{seed}");
            let recorder = (self.recorder_factory)(&run_id);
            let completed = runner
                .run(benchmark, agent, evaluator, seed, run_id, recorder)
                .map_err(|error| HarnessError::EpisodeFailed {
                    seed,
                    reason: error.to_string(),
                })?;
            let result = completed;
            outcomes.push(EpisodeOutcome {
                seed: result.seed,
                completion_status: result.capability.completion_status,
                task_success: result.capability.task_success,
                total_reward: result.capability.total_reward,
                turns_used: result.capability.turns_used,
            });
        }
        Ok(aggregate(label, &outcomes, plan.confidence))
    }
}

/// A scratch directory that cannot collide with another run's, ever.
///
/// The obvious implementation is `<label>-<pid>-<counter>`, and it is wrong. Windows recycles process
/// identifiers, and these directories are never cleaned up, so a later test binary that happens to be
/// assigned a pid an earlier one used inherits its telemetry database — which already holds events
/// numbered from one. The fresh run then inserts sequence 1 again and the recorder refuses it with
/// "telemetry sequence must be gap-free".
///
/// That failure is load-dependent rather than obvious, because pid reuse depends on how busy the
/// machine is. It showed up roughly one full-suite run in five, always in whichever test happened to
/// draw the recycled pid, and never when the suite ran alone. Keying on the process start time as well
/// as the pid makes the name unique per *process launch* rather than per identifier, and the counter
/// keeps it unique within a launch. Nothing needs cleaning up afterwards.
///
/// Deliberately not a random name: a reproducible-looking path is easier to correlate with a failing
/// run, and the timestamp is already unique enough for that to be useful.
///
/// The directory is created, and creation failure is returned rather than asserted. An earlier version
/// called `create_dir_all(..).expect(..)`, which is a process abort for an environmental condition --
/// `%TEMP%` unwritable, the disk full, or endpoint protection blocking a new directory under
/// `%TEMP%\probelab-*` are all realistic on the locked-down Windows host this product targets. Every
/// caller here is a test that already propagates the error, so a `Result` costs nothing and turns an
/// abort into a named failure. It also matters that the directory is genuinely created: returning a
/// path alone would leave `SqliteTelemetryRecorder` to fail later with
/// `unable to open database file`, which reads like a storage bug rather than the cause.
pub fn unique_scratch_root(label: &str) -> std::io::Result<std::path::PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or_default();
    let root = std::env::temp_dir().join(format!(
        "probelab-{label}-{}-{started}-{sequence}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root)?;
    Ok(root)
}

fn aggregate(label: &str, outcomes: &[EpisodeOutcome], confidence: f64) -> AggregateScore {
    let episodes = outcomes.len();
    let successes = outcomes
        .iter()
        .filter(|outcome| outcome.task_success)
        .count();
    // Saturating rather than `sum()`, because a plain `sum()` on `i64`/`u32` panics on overflow in a
    // debug build and silently wraps in a release one. Neither is acceptable for a number that ends up
    // in a published measurement, and `plan.episodes` is a caller-supplied `usize` with no upper bound.
    // Saturating means an absurd run reports a saturated total rather than a plausible wrong one.
    let total_reward: i64 = outcomes.iter().fold(0i64, |accumulator, outcome| {
        accumulator.saturating_add(outcome.total_reward)
    });
    let total_turns: u32 = outcomes.iter().fold(0u32, |accumulator, outcome| {
        accumulator.saturating_add(outcome.turns_used)
    });

    let mut status_counts: BTreeMap<String, usize> = BTreeMap::new();
    for outcome in outcomes {
        // `wire_name`, not `format!("{:?}").to_lowercase()`. That produced `turnlimit` where the
        // serialized form is `turn_limit`, so the engine's aggregate and the lab's aggregate -- which
        // document themselves as sharing semantics -- used different keys for the same field.
        *status_counts
            .entry(outcome.completion_status.wire_name().to_owned())
            .or_insert(0) += 1;
    }
    let divisor = episodes.max(1) as f64;
    AggregateScore {
        label: label.to_owned(),
        episodes,
        successes,
        success_rate: wilson_interval(successes, episodes, confidence),
        mean_reward: total_reward as f64 / divisor,
        mean_turns: f64::from(total_turns) / divisor,
        status_counts,
    }
}
