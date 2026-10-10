//! Running task benchmarks through reference, CLI-agent, and guided policies.
//!
//! # The policy ladder
//!
//! Every task benchmark accepts the same policy shapes, ordered by how much
//! they prove:
//!
//! * Reference policies run in-process and calibrate the instrument
//!   (optimal vs. baseline, asserted disjoint in tests).
//! * CLI-agent policies execute a real installed agent in a disposable
//!   workspace via the executors crate. The artifacts are scored by the same
//!   hidden checks; the isolation level travels in the report notes.
//! * Guided policies score imported artifacts. The provenance is always
//!   [`EvidenceProvenance::ManualSubmission`], even when the score is high.

use std::collections::BTreeMap;

use probelab_executors::{
    execute_coding_task, generate_guided_package, import_guided_evidence, CodingAgentKind,
    CodingTask, EvidenceProvenance as ExecutorProvenance,
};

use crate::report::{EvidenceProvenance, TaskBenchmark, TaskReport};

/// Which policy produces the artifacts for a task-benchmark run.
#[derive(Clone, Copy, Debug)]
pub enum TaskPolicy {
    /// The benchmark's optimal reference. Calibrates the ceiling.
    ReferenceOptimal,
    /// The benchmark's baseline reference. Calibrates the floor.
    ReferenceBaseline,
    /// A real installed CLI agent, executed per task in a disposable
    /// workspace with the given wall-clock timeout per task.
    CliAgent {
        /// Which agent to run.
        kind: CodingAgentKind,
        /// Seconds per task before the agent is killed.
        timeout_seconds: u64,
    },
}

impl TaskPolicy {
    /// A stable name for reports.
    pub fn name(self) -> String {
        match self {
            TaskPolicy::ReferenceOptimal => "reference-optimal".to_owned(),
            TaskPolicy::ReferenceBaseline => "reference-baseline".to_owned(),
            TaskPolicy::CliAgent { kind, .. } => kind.executor_id().to_owned(),
        }
    }
}

/// Failures from a task-benchmark run.
#[derive(Clone, Debug, thiserror::Error, PartialEq)]
pub enum TaskRunError {
    /// The CLI agent is not installed or not authenticated.
    #[error("the CLI agent is not ready: {0}")]
    AgentNotReady(String),
    /// No seeds were requested.
    #[error("a task run must request at least one seed")]
    NoSeeds,
}

/// Measures `coding-repair` under one policy over the given seeds.
pub fn run_coding_repair(policy: TaskPolicy, seeds: &[u64]) -> Result<TaskReport, TaskRunError> {
    if seeds.is_empty() {
        return Err(TaskRunError::NoSeeds);
    }
    let mut success = Vec::with_capacity(seeds.len());
    let mut partial = Vec::with_capacity(seeds.len());
    let mut notes = Vec::new();
    for seed in seeds {
        let task = crate::coding::generate_repair_task(*seed);
        match policy {
            TaskPolicy::ReferenceOptimal => {
                let candidate =
                    crate::coding::apply_repair_policy(crate::coding::RepairPolicy::Optimal, &task);
                let score = crate::coding::score_repair(&task, &candidate);
                success.push(score.fixed);
                partial.push(score.partial_credit);
            }
            TaskPolicy::ReferenceBaseline => {
                let candidate =
                    crate::coding::apply_repair_policy(crate::coding::RepairPolicy::Noop, &task);
                let score = crate::coding::score_repair(&task, &candidate);
                success.push(score.fixed);
                partial.push(score.partial_credit);
            }
            TaskPolicy::CliAgent {
                kind,
                timeout_seconds,
            } => {
                let coding_task = CodingTask {
                    task_id: task.task_id.clone(),
                    instructions: format!(
                        "{}\n\nEdit `task.py` in place. Keep the function name and signature unchanged. Do not create new files.",
                        task.description,
                    ),
                    files: crate::coding::task_files(&task),
                    timeout_seconds,
                    max_output_bytes: 8192,
                };
                match execute_coding_task(kind, &coding_task) {
                    Ok(execution) => {
                        notes.push(format!(
                            "{}: isolation={} exit={:?} timeout={}",
                            task.task_id,
                            isolation_label(execution.isolation),
                            execution.exit_code,
                            execution.timed_out,
                        ));
                        let score = crate::coding::score_artifacts(&task, &execution.artifacts);
                        success.push(score.fixed);
                        partial.push(score.partial_credit);
                    }
                    Err(error) => {
                        return Err(TaskRunError::AgentNotReady(error.to_string()));
                    }
                }
            }
        }
    }
    Ok(TaskReport::from_outcomes(crate::report::TaskOutcomes {
        benchmark: TaskBenchmark::CodingRepair,
        definition_version: crate::CODING_REPAIR_VERSION.to_owned(),
        definition_digest: crate::coding::definition_digest(),
        policy: policy.name(),
        provenance: EvidenceProvenance::ObservedRun,
        seeds: seeds.to_vec(),
        task_success: success,
        partial_credit: partial,
    })
    .with_notes(deduplicated_notes(notes)))
}

/// Measures `memory-latch` under one policy over the given seeds.
pub fn run_memory_latch(policy: TaskPolicy, seeds: &[u64]) -> Result<TaskReport, TaskRunError> {
    if seeds.is_empty() {
        return Err(TaskRunError::NoSeeds);
    }
    let mut success = Vec::with_capacity(seeds.len());
    let mut notes = Vec::new();
    for seed in seeds {
        let task = crate::memory::generate_latch_task(*seed);
        match policy {
            TaskPolicy::ReferenceOptimal => {
                let answer =
                    crate::memory::apply_latch_policy(crate::memory::LatchPolicy::Memory, &task);
                success.push(crate::memory::score_latch(&task, &answer));
            }
            TaskPolicy::ReferenceBaseline => {
                let answer =
                    crate::memory::apply_latch_policy(crate::memory::LatchPolicy::Stateless, &task);
                success.push(crate::memory::score_latch(&task, &answer));
            }
            TaskPolicy::CliAgent {
                kind,
                timeout_seconds,
            } => {
                // The cue and the choice travel as one instruction text with
                // an explicit single-letter answer contract: a CLI agent has
                // no turn loop, so the transcript is presented at once and
                // the answer is parsed from its output. The retention demand
                // is preserved — the cue appears once, before the choice.
                let mut files = BTreeMap::new();
                files.insert("CUE.md".to_owned(), crate::memory::cue_observation(&task));
                files.insert("CHOICE.md".to_owned(), crate::memory::choice_observation());
                let coding_task = CodingTask {
                    task_id: task.task_id.clone(),
                    instructions: format!(
                        "Read CUE.md, then CHOICE.md. {choice} Answer with a single letter (A, B, or C) and nothing else.",
                        choice = crate::memory::choice_observation(),
                    ),
                    files,
                    timeout_seconds,
                    max_output_bytes: 4096,
                };
                match execute_coding_task(kind, &coding_task) {
                    Ok(execution) => {
                        notes.push(format!(
                            "{}: isolation={} exit={:?} timeout={}",
                            task.task_id,
                            isolation_label(execution.isolation),
                            execution.exit_code,
                            execution.timed_out,
                        ));
                        let text = execution.stdout_preview.clone();
                        let answer = crate::memory::parse_answer(&text).unwrap_or(
                            crate::memory::LatchAnswer {
                                door: String::new(),
                            },
                        );
                        success.push(crate::memory::score_latch(&task, &answer));
                    }
                    Err(error) => {
                        return Err(TaskRunError::AgentNotReady(error.to_string()));
                    }
                }
            }
        }
    }
    let partial: Vec<f64> = success
        .iter()
        .map(|hit| if *hit { 1.0 } else { 0.0 })
        .collect();
    Ok(TaskReport::from_outcomes(crate::report::TaskOutcomes {
        benchmark: TaskBenchmark::MemoryLatch,
        definition_version: crate::MEMORY_LATCH_VERSION.to_owned(),
        definition_digest: crate::memory::definition_digest(),
        policy: policy.name(),
        provenance: EvidenceProvenance::ObservedRun,
        seeds: seeds.to_vec(),
        task_success: success,
        partial_credit: partial,
    })
    .with_notes(deduplicated_notes(notes)))
}

fn isolation_label(isolation: probelab_executors::IsolationLevel) -> &'static str {
    match isolation {
        probelab_executors::IsolationLevel::WorkdirOnly => "workdir-only",
        probelab_executors::IsolationLevel::ContainedProcess => "contained-process",
        probelab_executors::IsolationLevel::Unverified => "unverified",
    }
}

fn deduplicated_notes(mut notes: Vec<String>) -> Vec<String> {
    notes.sort();
    notes.dedup();
    // Per-task notes would dominate the report; keep a bounded sample plus a
    // count so the detail survives without flooding.
    if notes.len() > 8 {
        let kept = notes[..8].to_vec();
        let mut out = kept;
        out.push(format!("…and {} further per-task notes", notes.len() - 8));
        out
    } else {
        notes
    }
}

/// Generates the guided package for one task of a benchmark.
pub fn guided_package(benchmark: TaskBenchmark, seed: u64) -> probelab_executors::GuidedPackage {
    match benchmark {
        TaskBenchmark::CodingRepair => {
            let task = crate::coding::generate_repair_task(seed);
            generate_guided_package(
                benchmark.id(),
                crate::CODING_REPAIR_VERSION,
                seed,
                crate::coding::task_files(&task),
            )
        }
        TaskBenchmark::MemoryLatch => {
            let task = crate::memory::generate_latch_task(seed);
            let mut files = BTreeMap::new();
            files.insert("CUE.md".to_owned(), crate::memory::cue_observation(&task));
            files.insert("CHOICE.md".to_owned(), crate::memory::choice_observation());
            generate_guided_package(benchmark.id(), crate::MEMORY_LATCH_VERSION, seed, files)
        }
    }
}

/// Scores a guided submission for one task. The provenance is always manual,
/// even on a perfect score.
pub fn score_guided_submission(
    benchmark: TaskBenchmark,
    seed: u64,
    artifacts: BTreeMap<String, String>,
    log: &str,
) -> Result<(TaskReport, ExecutorProvenance), String> {
    let package = guided_package(benchmark, seed);
    let (evidence, provenance) = import_guided_evidence(&package, artifacts, log)?;
    let (fixed, partial) = match benchmark {
        TaskBenchmark::CodingRepair => {
            let task = crate::coding::generate_repair_task(seed);
            let score = crate::coding::score_artifacts(&task, &evidence.artifacts);
            (score.fixed, score.partial_credit)
        }
        TaskBenchmark::MemoryLatch => {
            let task = crate::memory::generate_latch_task(seed);
            let text: String = evidence
                .artifacts
                .values()
                .cloned()
                .collect::<Vec<_>>()
                .join("\n");
            let answer = crate::memory::parse_answer(&text).unwrap_or(crate::memory::LatchAnswer {
                door: String::new(),
            });
            let hit = crate::memory::score_latch(&task, &answer);
            (hit, if hit { 1.0 } else { 0.0 })
        }
    };
    let digest = match benchmark {
        TaskBenchmark::CodingRepair => crate::coding::definition_digest(),
        TaskBenchmark::MemoryLatch => crate::memory::definition_digest(),
    };
    let version = benchmark.version().to_owned();
    let report = TaskReport::from_outcomes(crate::report::TaskOutcomes {
        benchmark,
        definition_version: version,
        definition_digest: digest,
        policy: "guided-submission".to_owned(),
        provenance: EvidenceProvenance::ManualSubmission,
        seeds: vec![seed],
        task_success: vec![fixed],
        partial_credit: vec![partial],
    });
    Ok((report, provenance))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_runs_reproduce_the_calibration() {
        let seeds = crate::coding::eval_seeds();
        let optimal = run_coding_repair(TaskPolicy::ReferenceOptimal, &seeds).expect("run");
        assert_eq!(optimal.successes, 70);
        assert_eq!(optimal.provenance, EvidenceProvenance::ObservedRun);
        let memory = run_memory_latch(TaskPolicy::ReferenceOptimal, &crate::memory::eval_seeds())
            .expect("run");
        assert_eq!(memory.successes, 70);
    }

    #[test]
    fn an_empty_seed_pool_is_refused() {
        assert_eq!(
            run_coding_repair(TaskPolicy::ReferenceOptimal, &[]),
            Err(TaskRunError::NoSeeds)
        );
        assert_eq!(
            run_memory_latch(TaskPolicy::ReferenceBaseline, &[]),
            Err(TaskRunError::NoSeeds)
        );
    }

    #[test]
    fn live_cli_execution_from_tests_requires_explicit_opt_in() {
        // Tests must never consume a subscription quota or run a third-party
        // agent by surprise: on a machine with an installed, authenticated
        // agent, `run_coding_repair` with a CLI policy would really execute
        // it. So live execution happens only when the operator sets
        // `PROBELAB_LIVE_CLI=1`; otherwise this test executes nothing and
        // asserts nothing about the host.
        if std::env::var("PROBELAB_LIVE_CLI").as_deref() != Ok("1") {
            return;
        }
        let seeds = [crate::coding::eval_seeds()[0]];
        let result = run_coding_repair(
            TaskPolicy::CliAgent {
                kind: CodingAgentKind::Codex,
                timeout_seconds: 60,
            },
            &seeds,
        );
        // Either a real measurement or a typed refusal when the agent is not
        // ready. Zeros-as-scores would be the failure; both outcomes below
        // are honest.
        match result {
            Ok(report) => assert_eq!(report.trials, 1),
            Err(TaskRunError::AgentNotReady(_)) => {}
            Err(TaskRunError::NoSeeds) => unreachable!("one seed was requested"),
        }
    }

    #[test]
    fn a_guided_coding_round_trip_scores_the_reference_fix() {
        // Package → artifacts → score, with the reference fix as the
        // submission: the import path must preserve verifiability end to end,
        // and the provenance must stay manual on a perfect score.
        let seed = crate::coding::eval_seeds()[0];
        let task = crate::coding::generate_repair_task(seed);
        let package = guided_package(TaskBenchmark::CodingRepair, seed);
        assert!(package.files.contains_key(&task.file_name));
        let mut artifacts = BTreeMap::new();
        artifacts.insert(task.file_name.clone(), task.reference_fix.clone());
        let (report, _) =
            score_guided_submission(TaskBenchmark::CodingRepair, seed, artifacts, "ran by hand")
                .expect("import");
        assert_eq!(report.successes, 1);
        assert_eq!(report.provenance, EvidenceProvenance::ManualSubmission);
    }

    #[test]
    fn a_guided_memory_round_trip_scores_the_cued_door() {
        let seed = crate::memory::eval_seeds()[0];
        let task = crate::memory::generate_latch_task(seed);
        let package = guided_package(TaskBenchmark::MemoryLatch, seed);
        // The cue is task input and must be present; the choice screen must
        // name all three doors equally, marking none.
        let cue = package.files.get("CUE.md").expect("cue file");
        assert!(cue.contains(&task.cue_door));
        let choice = package.files.get("CHOICE.md").expect("choice file");
        for door in ["A", "B", "C"] {
            assert!(choice.contains(door), "choice screen omits door {door}");
        }
        let mut artifacts = BTreeMap::new();
        artifacts.insert("answer.txt".to_owned(), task.cue_door.clone());
        let (report, _) =
            score_guided_submission(TaskBenchmark::MemoryLatch, seed, artifacts, "ran by hand")
                .expect("import");
        assert_eq!(report.successes, 1);
        assert_eq!(report.provenance, EvidenceProvenance::ManualSubmission);
    }

    #[test]
    fn guided_submissions_are_always_manual() {
        let mut artifacts = BTreeMap::new();
        artifacts.insert("task.py".to_owned(), "anything".to_owned());
        let (report, provenance) =
            score_guided_submission(TaskBenchmark::CodingRepair, 1, artifacts, "log")
                .expect("import");
        assert_eq!(report.provenance, EvidenceProvenance::ManualSubmission);
        assert_eq!(provenance, ExecutorProvenance::ManualSubmission);
        assert_eq!(report.trials, 1);
    }
}
