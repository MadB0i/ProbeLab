//! Task-benchmark commands: coding repair and cue memory.
//!
//! These benchmarks measure tasks, not grid episodes: the evidence is the
//! artifact a policy left behind, scored by hidden checks the policy never
//! sees. Every report carries its provenance — an observed run and a manual
//! submission are different kinds of evidence — and the panel must render
//! that distinction rather than a bare score.

use std::collections::BTreeMap;

use probelab_executors::CodingAgentKind;
use probelab_tasks::{
    coding, memory,
    report::{EvidenceProvenance, TaskBenchmark, TaskReport},
    runner::{TaskPolicy, TaskRunError},
};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::AppState;

/// Failures from the task-benchmark commands.
#[derive(Debug, thiserror::Error)]
pub enum TaskCommandError {
    /// The benchmark identifier is unknown.
    #[error("unknown task benchmark {0:?}; expected \"coding-repair\" or \"memory-latch\"")]
    UnknownBenchmark(String),
    /// The policy identifier is unknown.
    #[error("unknown task policy {0:?}; expected \"reference-optimal\", \"reference-baseline\", or \"cli:<agent>\"")]
    UnknownPolicy(String),
    /// The run itself failed.
    #[error("{0}")]
    Run(String),
    /// The guided submission could not be imported.
    #[error("the submission could not be imported: {0}")]
    Import(String),
}

impl Serialize for TaskCommandError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl From<TaskRunError> for TaskCommandError {
    fn from(error: TaskRunError) -> Self {
        TaskCommandError::Run(error.to_string())
    }
}

/// One task benchmark, with the honesty metadata a row needs.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskBenchmarkSummary {
    /// Stable identifier.
    pub id: String,
    /// Human name.
    pub name: String,
    /// What it measures, in one line.
    pub description: String,
    /// Definition version.
    pub version: String,
    /// SHA-256 over the task corpus.
    pub definition_digest: String,
    /// What verification can and cannot establish.
    pub limits: Vec<String>,
    /// Development seed count.
    pub dev_seeds: usize,
    /// Evaluation seed count.
    pub eval_seeds: usize,
}

/// Every task benchmark the lab can measure.
#[tauri::command(rename_all = "snake_case")]
pub fn list_task_benchmarks() -> Vec<TaskBenchmarkSummary> {
    vec![
        TaskBenchmarkSummary {
            id: TaskBenchmark::CodingRepair.id().to_owned(),
            name: "Coding repair".to_owned(),
            description: "Seed-varying single-bug Python repairs, scored by hidden static checks.".to_owned(),
            version: coding::CODING_REPAIR_VERSION.to_owned(),
            definition_digest: coding::definition_digest(),
            limits: coding::verification_limits(),
            dev_seeds: coding::dev_seeds().len(),
            eval_seeds: coding::eval_seeds().len(),
        },
        TaskBenchmarkSummary {
            id: TaskBenchmark::MemoryLatch.id().to_owned(),
            name: "Memory latch".to_owned(),
            description: "A cue shown once, then a choice without it. Retention measured against a chance baseline.".to_owned(),
            version: memory::MEMORY_LATCH_VERSION.to_owned(),
            definition_digest: memory::definition_digest(),
            limits: vec![
                "two-turn transcript: cue, then choice".to_owned(),
                "three doors: chance baseline is 1 in 3".to_owned(),
                "CLI policies receive cue and choice at once with a single-letter answer contract".to_owned(),
            ],
            dev_seeds: memory::dev_seeds().len(),
            eval_seeds: memory::eval_seeds().len(),
        },
    ]
}

/// A request to measure a task benchmark.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskEvalRequest {
    /// `coding-repair` or `memory-latch`.
    pub benchmark_id: String,
    /// `reference-optimal`, `reference-baseline`, or `cli:<agent-id>`
    /// (`cli:codex-cli`, `cli:opencode-cli`, `cli:claude-code-cli`).
    pub policy: String,
    /// Seeds to measure. Empty means the benchmark's evaluation pool.
    pub seeds: Vec<u64>,
    /// Seconds per task for CLI policies.
    pub timeout_seconds: u64,
}

fn parse_benchmark(id: &str) -> Result<TaskBenchmark, TaskCommandError> {
    match id {
        "coding-repair" => Ok(TaskBenchmark::CodingRepair),
        "memory-latch" => Ok(TaskBenchmark::MemoryLatch),
        _ => Err(TaskCommandError::UnknownBenchmark(id.to_owned())),
    }
}

fn parse_policy(policy: &str, timeout_seconds: u64) -> Result<TaskPolicy, TaskCommandError> {
    match policy {
        "reference-optimal" => Ok(TaskPolicy::ReferenceOptimal),
        "reference-baseline" => Ok(TaskPolicy::ReferenceBaseline),
        _ => match policy.strip_prefix("cli:") {
            Some("codex-cli") => Ok(TaskPolicy::CliAgent {
                kind: CodingAgentKind::Codex,
                timeout_seconds: timeout_seconds.max(1),
            }),
            Some("opencode-cli") => Ok(TaskPolicy::CliAgent {
                kind: CodingAgentKind::OpenCode,
                timeout_seconds: timeout_seconds.max(1),
            }),
            Some("claude-code-cli") => Ok(TaskPolicy::CliAgent {
                kind: CodingAgentKind::ClaudeCode,
                timeout_seconds: timeout_seconds.max(1),
            }),
            _ => Err(TaskCommandError::UnknownPolicy(policy.to_owned())),
        },
    }
}

/// Measures a task benchmark under one policy.
///
/// Reference policies run on the calling thread pool via `spawn_blocking`.
/// `cli:…` policies are **refused**: automatic execution of an installed
/// coding agent runs as the invoking user with that user's files readable and
/// — for cloud-backed agents — a network path out, and no workdir, temp
/// directory, or prompt contract turns that into a verified boundary (see
/// `docs/STATUS.md` §8). A refusal here is fail-closed; the supported path
/// for installed agents is a guided package, whose outputs are scored by the
/// same hidden checks and always marked as manual submissions.
#[tauri::command(rename_all = "snake_case")]
pub async fn run_task_eval(
    _state: State<'_, AppState>,
    request: TaskEvalRequest,
) -> Result<TaskReport, TaskCommandError> {
    let benchmark = parse_benchmark(&request.benchmark_id)?;
    let policy = parse_policy(&request.policy, request.timeout_seconds)?;
    refuse_automatic_cli_execution(&policy)?;
    let seeds = if request.seeds.is_empty() {
        match benchmark {
            TaskBenchmark::CodingRepair => coding::eval_seeds(),
            TaskBenchmark::MemoryLatch => memory::eval_seeds(),
        }
    } else {
        request.seeds.clone()
    };
    // Bounded: a request cannot measure more than the evaluation pool plus
    // the development pool in one call.
    if seeds.len() > 140 {
        return Err(TaskCommandError::Run(
            "a task run measures at most 140 seeds per call".to_owned(),
        ));
    }
    tauri::async_runtime::spawn_blocking(move || match benchmark {
        TaskBenchmark::CodingRepair => probelab_tasks::runner::run_coding_repair(policy, &seeds)
            .map_err(TaskCommandError::from),
        TaskBenchmark::MemoryLatch => {
            probelab_tasks::runner::run_memory_latch(policy, &seeds).map_err(TaskCommandError::from)
        }
    })
    .await
    .map_err(|error| TaskCommandError::Run(format!("the task worker stopped: {error}")))?
}

/// Refuses automatic CLI-agent execution at the product boundary.
///
/// Split out so the refusal is unit-testable without a Tauri runtime: the
/// property that matters — `cli:…` never reaches the executor from this
/// command — must fail here rather than in review.
fn refuse_automatic_cli_execution(
    policy: &probelab_tasks::runner::TaskPolicy,
) -> Result<(), TaskCommandError> {
    if matches!(policy, probelab_tasks::runner::TaskPolicy::CliAgent { .. }) {
        return Err(TaskCommandError::Run(
            "automatic execution of an installed coding agent is disabled: it cannot be isolated \
             from the user's files and credentials on this host. Generate a guided package instead, \
             run it yourself, and submit the artifacts for independent scoring."
                .to_owned(),
        ));
    }
    Ok(())
}

/// A guided package for external execution, as files plus instructions.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GuidedPackageView {
    /// Package format version.
    pub format_version: u32,
    /// The benchmark the package measures.
    pub benchmark_id: String,
    /// Pinned definition version.
    pub benchmark_version: String,
    /// The seed the package was generated from.
    pub seed: u64,
    /// Human instructions.
    pub instructions: String,
    /// Task files, relative path → contents.
    pub files: BTreeMap<String, String>,
    /// What a submission must contain.
    pub submission_requirements: Vec<String>,
}

/// Generates an isolated task package for external execution.
///
/// The package holds task inputs only — never hidden answers, scoring logic,
/// or credentials. Verification stays in the lab.
#[tauri::command(rename_all = "snake_case")]
pub fn guided_task_package(
    benchmark_id: String,
    seed: u64,
) -> Result<GuidedPackageView, TaskCommandError> {
    let benchmark = parse_benchmark(&benchmark_id)?;
    let package = probelab_tasks::runner::guided_package(benchmark, seed);
    Ok(GuidedPackageView {
        format_version: package.format_version,
        benchmark_id: package.benchmark_id,
        benchmark_version: package.benchmark_version,
        seed: package.seed,
        instructions: package.instructions,
        files: package.files,
        submission_requirements: package.submission_requirements,
    })
}

/// A submission for independent verification.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuidedSubmission {
    /// `coding-repair` or `memory-latch`.
    pub benchmark_id: String,
    /// The seed the package was generated from.
    pub seed: u64,
    /// Submitted artifacts, relative path → contents.
    pub artifacts: BTreeMap<String, String>,
    /// The execution log.
    pub log: String,
}

/// Verifies a guided submission independently.
///
/// The report's provenance is always `manual_submission`, even on a perfect
/// score: a submission the lab did not observe is a different kind of
/// evidence from a run it did.
#[tauri::command(rename_all = "snake_case")]
pub fn score_guided_submission(
    submission: GuidedSubmission,
) -> Result<TaskReport, TaskCommandError> {
    let benchmark = parse_benchmark(&submission.benchmark_id)?;
    let (report, _provenance) = probelab_tasks::runner::score_guided_submission(
        benchmark,
        submission.seed,
        submission.artifacts,
        &submission.log,
    )
    .map_err(TaskCommandError::Import)?;
    debug_assert_eq!(report.provenance, EvidenceProvenance::ManualSubmission);
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_task_catalog_lists_both_benchmarks_with_digests() {
        let benchmarks = list_task_benchmarks();
        assert_eq!(benchmarks.len(), 2);
        for benchmark in &benchmarks {
            assert!(benchmark.definition_digest.starts_with("sha256:"));
            assert_eq!(benchmark.eval_seeds, 70);
            assert_eq!(benchmark.dev_seeds, 40);
            assert!(!benchmark.limits.is_empty());
        }
    }

    #[test]
    fn unknown_benchmarks_and_policies_are_refused() {
        assert!(parse_benchmark("nope").is_err());
        assert!(parse_policy("nope", 10).is_err());
        assert!(parse_policy("cli:nope", 10).is_err());
        assert!(parse_policy("reference-optimal", 10).is_ok());
        assert!(parse_policy("cli:codex-cli", 0).is_ok());
    }

    #[test]
    fn automatic_cli_execution_is_refused_at_the_product_boundary() {
        // The library runner still supports CLI policies (for a future
        // verified boundary), but the product command must not execute them:
        // an installed agent runs as the invoking user, and no workdir makes
        // that safe.
        for agent in ["cli:codex-cli", "cli:opencode-cli", "cli:claude-code-cli"] {
            let policy = parse_policy(agent, 30).expect("shape parses");
            let refusal = refuse_automatic_cli_execution(&policy);
            assert!(
                refusal.is_err(),
                "{agent} must be refused at the product boundary"
            );
        }
        let reference = parse_policy("reference-optimal", 30).expect("reference parses");
        assert!(refuse_automatic_cli_execution(&reference).is_ok());
    }

    #[test]
    fn reference_task_runs_score_through_the_command_shape() {
        let report = probelab_tasks::runner::run_coding_repair(
            TaskPolicy::ReferenceOptimal,
            &coding::eval_seeds(),
        )
        .expect("reference run");
        assert_eq!(report.successes, 70);
    }
}
