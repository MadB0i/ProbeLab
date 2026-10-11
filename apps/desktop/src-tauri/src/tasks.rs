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
// `Manager` is what puts `path()` on the handle. Without it the write target would have to be
// passed in from the frontend, which is the thing this command exists to avoid.
use tauri::{Manager, State};

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
    /// The package could not be written where it was asked to go.
    ///
    /// Its own variant rather than a reuse of [`TaskCommandError::Run`], because a failure to write
    /// is not a failure to run anything: the task was never executed here, and saying otherwise in
    /// an error message would misdescribe what happened.
    #[error("{0}")]
    Storage(String),
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

/// A package written to disk, and where it landed.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedPackage {
    /// The directory the package was written into.
    pub directory: String,
    /// How many files were written, excluding the manifest.
    pub file_count: usize,
}

/// Writes a guided package into a ProbeLab-owned directory and opens nothing.
///
/// # Why the directory is chosen here and not by the caller
///
/// The package's file names come from the benchmark definition, so a path built from them is a path
/// built from data. Taking a destination from the frontend would make every one of those names a
/// traversal candidate. Instead the caller names the benchmark and seed, and this builds
/// `<Documents>/ProbeLab/Packages/<benchmark>-seed-<n>/`, refusing any component that is not a plain
/// identifier before it is ever joined onto a path.
///
/// # What is written
///
/// Exactly the files the package declares, plus one manifest recording what was written and when. No
/// file is created that the package did not name, and nothing outside the package directory is
/// touched. A file that exists is refused rather than overwritten, so a second click cannot silently
/// replace an edited task with a fresh copy.
/// The directory ProbeLab writes its packages into, or a typed refusal.
///
/// Tauri resolves Documents through the shell, which can legitimately fail: a redirected OneDrive
/// folder whose path the shell cannot resolve returns `None` rather than a wrong path. Falling back to
/// the application data directory keeps the feature working on such a machine, and the caller is told
/// which directory it got so the report cannot imply a location nothing was written to.
fn package_root(app: &tauri::AppHandle) -> Result<std::path::PathBuf, TaskCommandError> {
    if let Ok(documents) = app.path().document_dir() {
        if !documents.as_os_str().is_empty() {
            return Ok(documents.join("ProbeLab").join("Packages"));
        }
    }
    // No resolvable Documents folder. The app data directory always exists and is this product's own.
    app.path()
        .app_data_dir()
        .map(|dir| dir.join("Packages"))
        .map_err(|error| {
            TaskCommandError::Storage(format!(
                "this machine exposes neither a documents folder nor an application data folder: {error}"
            ))
        })
}

/// Writes a generated package into a ProbeLab-owned folder and opens nothing.
///
/// The destination is `<Documents>/ProbeLab/Packages/<benchmark>-seed-<n>/`, falling back to the
/// application data directory on a machine whose Documents folder the shell cannot resolve. The
/// caller names the benchmark and seed and nothing else, so no path in this command is supplied by the
/// frontend.
///
/// # What is written
///
/// Exactly the files the package declares, plus one manifest. No file is created that the package did
/// not name, and nothing outside the package directory is touched. A folder that already holds a
/// package is refused rather than overwritten, so a second click cannot silently replace an edited
/// task with a fresh copy.
#[tauri::command(rename_all = "snake_case")]
pub fn save_guided_package(
    app: tauri::AppHandle,
    benchmark_id: String,
    seed: u64,
) -> Result<SavedPackage, TaskCommandError> {
    let package = guided_task_package(benchmark_id.clone(), seed)?;
    let root = package_root(&app)?;

    // The folder name is built from the benchmark, so it is validated rather than trusted. This is
    // the only thing standing between a package name and the filesystem.
    let folder = format!("{}-seed-{}", safe_component(&benchmark_id)?, seed);
    let directory = root.join(&folder);
    if !directory.starts_with(&root) {
        return Err(TaskCommandError::Storage(
            "the package folder would fall outside the packages directory".to_owned(),
        ));
    }
    std::fs::create_dir_all(&directory).map_err(|error| {
        TaskCommandError::Storage(format!(
            "{} could not be created: {error}",
            directory.display()
        ))
    })?;

    // Refused before anything is written, so a folder that already holds a package is left exactly as
    // it is rather than being half-overwritten by a second click.
    let manifest_path = directory.join("probelab-package.json");
    if manifest_path.exists() {
        return Err(TaskCommandError::Storage(format!(
            "{} already holds a package; move or delete it before generating another",
            directory.display()
        )));
    }

    let mut written = 0usize;
    for (relative, contents) in &package.files {
        let target = resolve_inside(&directory, relative)?;
        if target.exists() {
            return Err(TaskCommandError::Storage(format!(
                "{} already exists; move or delete it before generating the package again",
                target.display()
            )));
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                TaskCommandError::Storage(format!(
                    "{} could not be created: {error}",
                    parent.display()
                ))
            })?;
        }
        std::fs::write(&target, contents).map_err(|error| {
            TaskCommandError::Storage(format!(
                "{} could not be written: {error}",
                target.display()
            ))
        })?;
        written += 1;
    }

    let manifest = serde_json::json!({
        "benchmarkId": package.benchmark_id,
        "benchmarkVersion": package.benchmark_version,
        "seed": package.seed,
        "formatVersion": package.format_version,
        "fileCount": written,
        "instructionsFile": "INSTRUCTIONS.md",
        "note": "Task inputs only. Hidden checks, scoring logic, and credentials are never written \
                 here; verification stays in ProbeLab.",
    });
    std::fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest).unwrap_or_default(),
    )
    .map_err(|error| {
        TaskCommandError::Storage(format!(
            "{} could not be written: {error}",
            manifest_path.display()
        ))
    })?;

    Ok(SavedPackage {
        directory: directory.to_string_lossy().to_string(),
        file_count: written,
    })
}

/// A path component that is a plain identifier, or an error.
///
/// Refuses anything containing a separator, a parent reference, or a character outside the set a
/// benchmark or seed name can legitimately use. This is what makes the package directory safe to
/// build from data rather than from a literal.
fn safe_component(raw: &str) -> Result<String, TaskCommandError> {
    let usable = !raw.is_empty()
        && raw.len() <= 64
        && raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if usable {
        return Ok(raw.to_owned());
    }
    Err(TaskCommandError::Storage(format!(
        "{raw:?} cannot be used as a folder name; only letters, digits, dash, and underscore are allowed"
    )))
}

/// Joins a package-relative path onto a directory, refusing anything that escapes it.
///
/// The check is on the components rather than on the joined string, so `a/../../b` is refused on the
/// component that tries to climb out rather than on the separator that made the climb possible.
fn resolve_inside(
    directory: &std::path::Path,
    relative: &str,
) -> Result<std::path::PathBuf, TaskCommandError> {
    let refuses = |detail: &str| {
        TaskCommandError::Storage(format!(
            "{relative:?} names a location outside the package folder ({detail}); refusing to write it"
        ))
    };
    let candidate = std::path::Path::new(relative);
    if candidate.is_absolute() {
        return Err(refuses("absolute path"));
    }
    if relative
        .split(['/', '\\'])
        .any(|part| part == ".." || part == "~")
    {
        return Err(refuses("parent-directory reference"));
    }
    let target = directory.join(candidate);
    if !target.starts_with(directory) {
        return Err(refuses("resolved outside the package folder"));
    }
    Ok(target)
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

    /*
     * The package folder name is built from the benchmark identifier, so it is the one place a
     * benchmark name becomes a filesystem path. Every shape that could climb out of, or escape, the
     * packages directory is refused here rather than sanitised into something the caller did not ask
     * for.
     */
    #[test]
    fn a_folder_name_refuses_anything_that_is_not_a_plain_identifier() {
        assert_eq!(
            safe_component("coding-repair").expect("usable"),
            "coding-repair"
        );
        assert_eq!(
            safe_component("memory_latch9").expect("usable"),
            "memory_latch9"
        );

        for hostile in [
            "",
            "..",
            "../escape",
            "a/b",
            "a\\b",
            "C:evil",
            "with space",
            "with*glob",
            "with\0null",
            "with:colon",
            "..\\..\\escape",
        ] {
            assert!(
                safe_component(hostile).is_err(),
                "{hostile:?} must not become a folder name"
            );
        }
    }

    #[test]
    fn a_folder_name_is_bounded() {
        // A 300-character name is legal on NTFS but is not a benchmark identifier, and an unbounded
        // name is how a path limit turns into an error the caller cannot act on.
        let long = "a".repeat(65);
        assert!(safe_component(&long).is_err());
        assert!(safe_component(&"a".repeat(64)).is_ok());
    }

    /*
     * Package file names come from the benchmark definition, so every one of them is untrusted input
     * to a path join. Each of these must be refused rather than written.
     */
    #[test]
    fn a_package_file_path_refuses_to_leave_the_package_folder() {
        let root = std::path::Path::new("/packages/coding-repair-seed-1");

        for hostile in [
            "../escape.txt",
            "..\\escape.txt",
            "sub/../../escape.txt",
            "~/escape.txt",
            "/etc/passwd",
            "C:\\Windows\\system32\\evil",
        ] {
            assert!(
                resolve_inside(root, hostile).is_err(),
                "{hostile:?} must not resolve inside the package folder"
            );
        }
    }

    #[test]
    fn a_package_file_path_that_stays_inside_is_accepted() {
        let root = std::path::Path::new("/packages/coding-repair-seed-1");
        for relative in ["task.py", "INSTRUCTIONS.md", "nested/dir/file.txt", "a.b.c"] {
            let resolved = resolve_inside(root, relative).expect("stays inside");
            assert!(
                resolved.starts_with(root),
                "{relative:?} resolved to {resolved:?}, outside {root:?}"
            );
        }
    }

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
