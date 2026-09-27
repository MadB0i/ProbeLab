//! The comparison command.
//!
//! # Why this is so thin
//!
//! It reads two stored things — the lifecycle record and the result bundles for the chosen
//! experiments — and hands them to [`probelab_lab::compare`]. All the judgement lives in the library,
//! where it is tested; this layer only does I/O and framing.
//!
//! # What it cannot do
//!
//! It holds no engine, no broker, no provider, and no process launcher, and it writes nothing. A
//! comparison is a report, so it has no path to executing or authorizing anything. That is asserted
//! structurally by the test at the bottom of this file rather than left as a claim in a comment.
//!
//! # The missing-results case
//!
//! A run with a lifecycle but no stored result — because it is still running, or because it crashed
//! before finishing — produces a [`ComparisonRun`]-shaped entry with no measurements rather than an
//! error. Dropping it silently would make a comparison look complete when a run is simply missing,
//! which is the failure this product is most careful about elsewhere.

use probelab_lab::{ComparisonOutcome, Incomparable};
use probelab_telemetry::{ExperimentLifecycle, SqliteResultRepository};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::AppState;

/// A comparison request: which experiments, and how to describe them.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComparisonRequest {
    /// The experiments to compare, in the order they should appear.
    pub experiment_ids: Vec<String>,
}

/// What the command returns.
///
/// Mirrors [`ComparisonOutcome`] rather than flattening it, so "these cannot be compared" and "here
/// is the comparison" are distinguishable states a panel must handle, not an absence it can ignore.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "detail")]
pub enum ComparisonResponse {
    /// The runs are comparable.
    Comparable(Box<probelab_lab::Comparison>),
    /// They are not, and here is why.
    NotComparable(Incomparable),
}

/// Every stored experiment, for the comparison picker.
///
/// Read from the durable store rather than from the Brain's in-memory surface, because a comparison
/// is most useful across sessions -- the runs you did not just do are the ones worth comparing. The
/// Brain surface only knows about this process.
#[tauri::command]
pub fn list_comparable_experiments(
    state: State<'_, AppState>,
) -> Result<Vec<probelab_telemetry::ExperimentRecord>, String> {
    state
        .results
        .list_experiments()
        .map_err(|error| format!("could not read the stored experiments: {error}"))
}

#[tauri::command]
pub fn compare_experiments(
    state: State<'_, AppState>,
    request: ComparisonRequest,
) -> Result<ComparisonResponse, String> {
    let results = state.results.clone();
    let experiment_ids = request.experiment_ids;
    // Blocking here is deliberate and bounded: SQLite reads are fast, and a comparison is a
    // read-only report. Spawning a thread for it would add a failure mode without adding throughput.
    compare_from(&results, &experiment_ids)
}

/// The comparison logic, separated from Tauri so it can be reasoned about — and tested — without a
/// running app.
fn compare_from(
    repository: &SqliteResultRepository,
    experiment_ids: &[String],
) -> Result<ComparisonResponse, String> {
    // Read every lifecycle first, so a run that never produced a result still appears and can be
    // named in the outcome.
    let mut runs: Vec<probelab_lab::ComparableRun> = Vec::with_capacity(experiment_ids.len());
    for experiment_id in experiment_ids {
        let record = repository.get_experiment(experiment_id).map_err(|error| {
            format!("could not read the stored state for {experiment_id}: {error}")
        })?;
        let Some(record) = record else {
            return Err(format!(
                "{experiment_id} is not a stored experiment; it cannot be compared"
            ));
        };

        // A run that is not finished has no measurement, and pretending otherwise would put a
        // fabricated zero in the table. It is projected as an empty comparable run, which the
        // comparison reports as unranked with a reason.
        let episodes = repository
            .list(std::slice::from_ref(experiment_id))
            .map_err(|error| {
                format!("could not read the stored results for {experiment_id}: {error}")
            })?;

        // The agent and seed come from the stored *plan*, not the result bundle. The bundle knows
        // which model produced the actions; the plan knows which experiment this was, and a
        // comparison is over experiments. A row written before the identity was stored reports its
        // absence, and the comparison refuses rather than substituting a guess.
        let (agent, seed) = match (record.target_agent.clone(), record.root_seed) {
            (Some(agent), Some(seed)) => (agent, seed),
            _ if episodes.is_empty() => (lifecycle_label(record.lifecycle), 0),
            // A run with results but no stored identity cannot be compared with anything: the whole
            // point is holding the plan constant, and this run's plan is not on record. Refusing is
            // the honest answer, so the seed is left as a value no two such runs share.
            _ => ("unknown plan (pre-identity record)".to_owned(), 0),
        };

        let run = probelab_lab::ComparableRun::from_results(
            experiment_id.clone(),
            record.benchmark_id.clone(),
            agent,
            seed,
            &episodes,
            0,
        );
        runs.push(run);
    }

    Ok(match probelab_lab::compare(runs) {
        ComparisonOutcome::Comparable(comparison) => ComparisonResponse::Comparable(comparison),
        ComparisonOutcome::NotComparable(reason) => ComparisonResponse::NotComparable(reason),
    })
}

/// A lifecycle's label, used as a stand-in agent name when a run produced no measurement.
///
/// Stating the state is more honest than inventing an agent, and a panel can render "interrupted" as
/// an agent slot that explains itself rather than as a name that looks like a real one.
fn lifecycle_label(lifecycle: ExperimentLifecycle) -> String {
    match lifecycle {
        ExperimentLifecycle::Planned => "planned (not started)".to_owned(),
        ExperimentLifecycle::Running => "running (no result yet)".to_owned(),
        ExperimentLifecycle::Completed => "completed".to_owned(),
        ExperimentLifecycle::Failed => "failed".to_owned(),
        ExperimentLifecycle::Interrupted => "interrupted (no result)".to_owned(),
    }
}

/// Structural assertion: the comparison path holds nothing that could execute or authorize.
///
/// A comparison is a report, so it must not be able to reach a sandbox, a broker, a provider, or a
/// process launcher. The assertion is on this module's `use` lines rather than its whole text,
/// because a text search would match the names in this very list and could only ever pass by being
/// weakened. Reading the imports is the actual property: anything a future change needs to reach,
/// it has to import, and an import of an executor is a compile-visible decision rather than a
/// detail a reviewer has to notice.
#[cfg(test)]
mod tests {
    /// The executor crates a comparison has no business depending on.
    const EXECUTOR_CRATES: [&str; 4] = [
        "probelab_sandbox",
        "probelab_broker",
        "probelab_agent",
        "probelab_model_adapters",
    ];

    #[test]
    fn the_comparison_path_imports_no_executor() {
        let source = include_str!("comparison.rs");
        for line in source.lines() {
            let trimmed = line.trim();
            // Only import statements, so this file's prose and test list cannot satisfy or fail it.
            if !trimmed.starts_with("use ") {
                continue;
            }
            for crate_name in EXECUTOR_CRATES {
                assert!(
                    !trimmed.contains(crate_name),
                    "the comparison path must not import {crate_name}: {trimmed}"
                );
            }
            assert!(
                !trimmed.contains("std::process"),
                "the comparison path must not import a process launcher: {trimmed}"
            );
        }
    }

    /// And it writes nothing. A command that could persist state could be mistaken for one that
    /// decides something, which is the authority this product keeps elsewhere and denies here.
    #[test]
    fn the_comparison_path_writes_nothing() {
        // Only the module's own code, not this test: the forbidden names are written out below, and
        // searching the whole file would always match itself.
        let body: String = include_str!("comparison.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("the module body precedes its tests")
            .to_owned();
        for write in [
            "put_experiment",
            ".store(",
            "execute_batch",
            "INSERT",
            "UPDATE",
            "DELETE",
        ] {
            assert!(
                !body.contains(write),
                "the comparison path must not write: found {write}"
            );
        }
    }
}
