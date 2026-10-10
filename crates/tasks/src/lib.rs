//! Seed-varying task benchmarks: coding repair and cue memory.
//!
//! # Why a second benchmark shape
//!
//! The turn-loop engine measures grid navigation well and nothing else. A
//! coding task is not a grid episode: the work is one artifact, the evidence
//! is the file the agent left behind, and the score comes from hidden checks
//! the agent never sees. Forcing that through `BenchmarkRunner` would mean a
//! grid-shaped hole for a file-shaped task.
//!
//! So this crate measures tasks, not episodes, while reusing the laboratory's
//! measurement method: declared seed pools, per-seed fresh policies, binary
//! task success with a Wilson interval, definition digests, and reference
//! policies that calibrate the instrument before any model touches it.
//!
//! # What each benchmark guarantees
//!
//! * Versioned definitions with a SHA-256 digest over the task corpus. A
//!   silent edit changes the digest, and a digest mismatch blocks comparison.
//! * Seed-varying tasks: identifiers and values derive from the seed, so a
//!   memorized answer does not transfer.
//! * Independent verification: the evaluator reads the artifact, never the
//!   agent's claims. Static checks only (documented in
//!   [`coding::verification_limits`]); dynamic execution is deferred.
//! * Baselines with disjoint intervals, asserted in tests.
//! * Provenance on every report: [`EvidenceProvenance`] distinguishes an
//!   observed run from a manual submission.

#![deny(missing_docs)]

pub mod coding;
pub mod memory;
pub mod report;
pub mod runner;

pub use coding::{RepairScore, RepairTask, CODING_REPAIR_VERSION};
pub use memory::{LatchAnswer, LatchTask, MEMORY_LATCH_VERSION};
pub use report::{EvidenceProvenance, TaskBenchmark, TaskReport};
pub use runner::{TaskPolicy, TaskRunError};
