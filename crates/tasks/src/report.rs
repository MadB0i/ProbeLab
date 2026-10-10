//! Independently verified task reports with Wilson uncertainty.
//!
//! The report is the same shape for every task benchmark, so the comparison
//! and export layers handle coding and memory identically. A report carries
//! its own provenance: an observed run and a manual submission are different
//! kinds of evidence, and the type keeps them different.

use serde::{Deserialize, Serialize};

/// How the evidence behind a report was produced.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceProvenance {
    /// The lab executed every task through a supported adapter and observed
    /// the artifacts itself.
    ObservedRun,
    /// A human ran a guided package elsewhere and submitted the outputs,
    /// which the lab verified independently.
    ManualSubmission,
}

impl EvidenceProvenance {
    /// A one-line label for panels and exports.
    pub fn label(self) -> &'static str {
        match self {
            EvidenceProvenance::ObservedRun => "observed run",
            EvidenceProvenance::ManualSubmission => "manual submission",
        }
    }
}

/// Which task benchmark a report describes.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskBenchmark {
    /// Seed-varying single-bug repair tasks with hidden static checks.
    CodingRepair,
    /// Cross-turn cue retention with a chance baseline.
    MemoryLatch,
}

impl TaskBenchmark {
    /// Stable identifier.
    pub fn id(self) -> &'static str {
        match self {
            TaskBenchmark::CodingRepair => "coding-repair",
            TaskBenchmark::MemoryLatch => "memory-latch",
        }
    }

    /// The definition version this report was measured against.
    pub fn version(self) -> &'static str {
        match self {
            TaskBenchmark::CodingRepair => crate::CODING_REPAIR_VERSION,
            TaskBenchmark::MemoryLatch => crate::MEMORY_LATCH_VERSION,
        }
    }
}

/// One task benchmark's measured outcome.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskReport {
    /// Which benchmark was measured.
    pub benchmark: TaskBenchmark,
    /// Definition version pinned at measurement time.
    pub definition_version: String,
    /// SHA-256 over the task corpus, so a silent edit is detectable.
    pub definition_digest: String,
    /// Which policy or agent produced the artifacts.
    pub policy: String,
    /// How the evidence was produced. Never omitted, never defaulted.
    pub provenance: EvidenceProvenance,
    /// Tasks that verified as successful.
    pub successes: usize,
    /// Tasks attempted.
    pub trials: usize,
    /// Success fraction, or `None` when nothing completed.
    pub success_rate: Option<f64>,
    /// Wilson interval lower bound (95%).
    pub interval_lower: f64,
    /// Wilson interval upper bound (95%).
    pub interval_upper: f64,
    /// Mean partial credit across trials, 0.0–1.0.
    pub mean_partial_credit: f64,
    /// Seeds measured, in order.
    pub seeds: Vec<u64>,
    /// Per-task success, in seed order.
    pub task_success: Vec<bool>,
    /// Execution notes, e.g. isolation levels and timeouts. Part of the
    /// report so a reader can tell a contained run from a workdir-only one
    /// without trusting a label elsewhere.
    pub execution_notes: Vec<String>,
}

/// The per-task outcomes one measurement produced.
///
/// Grouped so the report constructor takes one argument rather than eight
/// parallel vectors that a caller could misalign.
pub struct TaskOutcomes {
    /// Which benchmark was measured.
    pub benchmark: TaskBenchmark,
    /// Definition version pinned at measurement time.
    pub definition_version: String,
    /// SHA-256 over the task corpus.
    pub definition_digest: String,
    /// Which policy or agent produced the artifacts.
    pub policy: String,
    /// How the evidence was produced.
    pub provenance: EvidenceProvenance,
    /// Seeds measured, in order.
    pub seeds: Vec<u64>,
    /// Per-task success, in seed order.
    pub task_success: Vec<bool>,
    /// Per-task partial credit, in seed order.
    pub partial_credit: Vec<f64>,
}

impl TaskReport {
    /// Builds a report from per-task outcomes. The interval is Wilson at 95%,
    /// the same method the grid harness uses, so uncertainty reads the same
    /// everywhere in the product.
    pub fn from_outcomes(outcomes: TaskOutcomes) -> Self {
        let TaskOutcomes {
            benchmark,
            definition_version,
            definition_digest,
            policy,
            provenance,
            seeds,
            task_success,
            partial_credit,
        } = outcomes;
        let trials = task_success.len();
        let successes = task_success.iter().filter(|success| **success).count();
        let interval = probelab_engine::harness::wilson_interval(successes, trials, 0.95);
        let mean_partial_credit = if partial_credit.is_empty() {
            0.0
        } else {
            partial_credit.iter().sum::<f64>() / partial_credit.len() as f64
        };
        Self {
            benchmark,
            definition_version,
            definition_digest,
            policy,
            provenance,
            successes,
            trials,
            success_rate: if trials == 0 {
                None
            } else {
                Some(successes as f64 / trials as f64)
            },
            interval_lower: interval.lower,
            interval_upper: interval.upper,
            mean_partial_credit: mean_partial_credit.clamp(0.0, 1.0),
            seeds,
            task_success,
            execution_notes: Vec::new(),
        }
    }

    /// Attaches execution notes (isolation, timeouts, fallbacks).
    pub fn with_notes(mut self, notes: Vec<String>) -> Self {
        self.execution_notes = notes;
        self
    }
}
