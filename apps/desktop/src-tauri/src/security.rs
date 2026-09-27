//! The Security Lab's command surface.
//!
//! Read-mostly, with one command that actually runs probes. Every probe executes through the same
//! `LabRunner` path an ordinary experiment takes — target, Broker, Sandbox, Observer, Evaluator — so
//! there is no shortcut by which a probe could report a result the engine did not produce.
//!
//! There is deliberately no command that marks an attack successful, and no command that reclassifies
//! an outcome. A security panel that could declare a breach would be the worst possible bug in this
//! product, so the absence is asserted in the tests below rather than left to review.

use probelab_lab::{probes_for, sweep, SecurityLabError, SecurityLabReport};
use serde::{Deserialize, Serialize};
use tauri::State;

use super::AppState;

/// The Security Lab's status, as the panel renders it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecurityProbeListing {
    /// Stable identifier.
    pub id: String,
    /// Name for the panel.
    pub name: String,
    /// What the probe attempts.
    pub description: String,
}

impl From<probelab_lab::AgentSpec> for SecurityProbeListing {
    fn from(spec: probelab_lab::AgentSpec) -> Self {
        Self {
            id: spec.id,
            name: spec.name,
            description: spec.description,
        }
    }
}

/// A request to run a security sweep.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecuritySweepRequest {
    /// The benchmark to sweep against.
    pub benchmark_id: String,
    /// The seed every probe uses, so the sweep replays.
    pub seed: u64,
    /// How many episodes each probe drives.
    pub episodes: usize,
}

/// A refusal the panel can display.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecurityCommandError {
    /// A stable code for the UI to branch on.
    pub code: String,
    /// A concise technical description.
    pub message: String,
}

impl From<SecurityLabError> for SecurityCommandError {
    fn from(error: SecurityLabError) -> Self {
        let code = match &error {
            SecurityLabError::UnknownProbe(_) => "unknown_probe",
            SecurityLabError::UnknownBenchmark(_) => "unknown_benchmark",
            SecurityLabError::Execution(_) => "execution_failed",
        };
        Self {
            code: code.to_owned(),
            message: error.to_string(),
        }
    }
}

/// The most recent sweep, or `None` when none has run.
///
/// `None` is a real state — an unprobed lab — and the panel renders it as one.
#[tauri::command(rename_all = "snake_case")]
pub fn latest_security_report(state: State<'_, AppState>) -> Option<SecurityLabReport> {
    state.security.latest()
}

/// Every security probe the registry holds.
///
/// Filtered by the registry's own `SecurityProbe` category, so the list is the registry's and not a
/// second one maintained here.
#[tauri::command(rename_all = "snake_case")]
pub fn list_security_probes(benchmark_id: Option<String>) -> Vec<SecurityProbeListing> {
    let benchmark = benchmark_id.unwrap_or_else(|| "grid-world".to_owned());
    let agents = probelab_lab::AgentRegistry::built_in();
    probes_for(&benchmark, &agents)
        .into_iter()
        .map(|probe| SecurityProbeListing {
            id: probe.id,
            name: probe.name,
            description: probe.description,
        })
        .collect()
}

/// Runs every registered security probe against a benchmark and returns what came back.
///
/// The one command that does real work. It runs the probes through the engine, so the report is a
/// projection of observed enforcement rather than a summary the panel composed.
///
/// It is synchronous on purpose, unlike the experiment path. A sweep is a short, bounded unit of work
/// whose result the user is waiting to read, and the panel disables its control while it runs. This is
/// noted rather than changed: a long sweep blocks the IPC task, and a future episode-count increase
/// would need this moved to a blocking worker the way `run_experiment` already is.
#[tauri::command(rename_all = "snake_case")]
pub fn run_security_sweep(
    state: State<'_, AppState>,
    request: SecuritySweepRequest,
) -> Result<SecurityLabReport, SecurityCommandError> {
    let report = sweep(&request.benchmark_id, request.seed, request.episodes)
        .map_err(SecurityCommandError::from)?;
    state.security.store(report.clone())?;
    Ok(report)
}

/// Retained sweep reports, bounded like the other surfaces.
#[derive(Clone, Default)]
pub struct SecuritySurface {
    reports: std::sync::Arc<std::sync::Mutex<Vec<SecurityLabReport>>>,
}

impl SecuritySurface {
    /// Stores a report.
    ///
    /// Reports on failure instead of panicking. A sweep that ran to completion is real evidence; if
    /// the surface could not retain it the caller still has it, and a typed error is a better outcome
    /// than turning one earlier fault into a permanently dead panel.
    pub fn store(&self, report: SecurityLabReport) -> Result<(), SecurityCommandError> {
        let mut reports = self.reports.lock().map_err(|_| SecurityCommandError {
            code: "surface_unavailable".to_owned(),
            message:
                "the security surface was left locked by an earlier fault and cannot retain this \
                      report; the sweep still ran and its result is returned"
                    .to_owned(),
        })?;
        if reports.len() >= 10 {
            reports.remove(0);
        }
        reports.push(report);
        Ok(())
    }

    /// The most recent report.
    ///
    /// Degrades to "none yet" on a poisoned lock, which the panel already renders as an unprobed lab.
    pub fn latest(&self) -> Option<SecurityLabReport> {
        self.reports.lock().ok()?.last().cloned()
    }

    /// How many reports are retained.
    pub fn len(&self) -> usize {
        self.reports
            .lock()
            .map(|reports| reports.len())
            .unwrap_or(0)
    }

    /// Whether any sweep has run.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use probelab_lab::{all_probes, BoundaryOutcome};

    #[test]
    fn an_empty_surface_reports_empty_rather_than_failing() {
        let surface = SecuritySurface::default();
        assert!(surface.latest().is_none());
        assert!(surface.is_empty());
    }

    #[test]
    fn a_stored_report_stays_reachable() {
        let surface = SecuritySurface::default();
        let report = sweep("grid-world", 3, 1).expect("sweep");
        surface.store(report.clone()).expect("store");
        assert_eq!(surface.latest().expect("latest"), report);
    }

    #[test]
    fn the_report_history_is_bounded() {
        let surface = SecuritySurface::default();
        for _ in 0..14 {
            surface
                .store(sweep("grid-world", 3, 1).expect("sweep"))
                .expect("store");
        }
        assert!(
            surface.len() <= 10,
            "history grew unbounded: {}",
            surface.len()
        );
    }

    /// A poisoned lock is a runtime condition, and the security panel must survive it.
    ///
    /// This matters more here than on the other surfaces. A panel that crashes while reporting *why*
    /// the boundary held is the worst possible place for an availability fault: it destroys the
    /// evidence the operator is reading. So the reads degrade to "nothing swept yet" and the write
    /// returns a typed refusal, and the surface stays usable afterwards.
    #[test]
    fn a_poisoned_security_surface_returns_errors_instead_of_crashing() {
        let surface = SecuritySurface::default();
        // The guard must be acquired inside the unwinding closure: a mutex is poisoned when a guard is
        // dropped during unwinding, so a guard held in the enclosing scope is released normally and the
        // lock is never poisoned at all. That would be a test that looks like it exercises poisoning and
        // does not, which is worse than no test.
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = surface.reports.lock().expect("lock to poison");
            panic!("a thread that panicked while holding the security surface lock");
        }));
        assert!(
            outcome.is_err(),
            "the poisoning panic did not unwind, so the lock was never poisoned"
        );
        assert!(
            surface.reports.lock().is_err(),
            "the lock should be poisoned; the test is not exercising what it claims"
        );

        assert!(surface.latest().is_none());
        assert_eq!(surface.len(), 0);
        assert!(surface.is_empty());
        let error = surface
            .store(sweep("grid-world", 3, 1).expect("sweep"))
            .expect_err("a write must be a typed refusal, not a panic");
        assert_eq!(error.code, "surface_unavailable");
        assert!(!error.message.is_empty());
    }

    #[test]
    fn the_probe_listing_comes_from_the_registry() {
        let listed = list_security_probes(Some("grid-world".to_owned()));
        assert!(!listed.is_empty(), "the registry holds security probes");
        for probe in &listed {
            assert!(!probe.description.is_empty());
        }
    }

    #[test]
    fn every_listed_probe_is_a_registered_security_probe() {
        let registered = all_probes();
        let listed = list_security_probes(Some("grid-world".to_owned()));
        assert_eq!(listed.len(), registered.len());
        for probe in &listed {
            assert!(
                registered.iter().any(|spec| spec.id == probe.id),
                "{} is listed but is not a registered security probe",
                probe.id
            );
        }
    }

    #[test]
    fn a_real_sweep_produces_a_report_with_observed_outcomes() {
        let report = sweep("grid-world", 3, 1).expect("sweep");
        assert!(!report.outcomes.is_empty());
        for outcome in &report.outcomes {
            assert!(
                outcome.failure.is_none(),
                "{} failed to run",
                outcome.probe_id
            );
            // Every probe reached an enforcement layer, so every boundary held.
            assert!(
                outcome.outcome.boundary_held(),
                "{} never reached an enforcement layer",
                outcome.probe_id
            );
        }
        assert!(
            report.findings().is_empty(),
            "the lab's own probes must not manufacture findings"
        );
    }

    #[test]
    fn an_unknown_benchmark_is_refused_with_a_stable_code() {
        let error =
            SecurityCommandError::from(SecurityLabError::UnknownBenchmark("nope".to_owned()));
        assert_eq!(error.code, "unknown_benchmark");
        assert!(error.message.contains("nope"));
    }

    /// The dangerous shape for a security panel is a command that can declare a breach or rewrite an
    /// outcome. If someone later adds one, this fails.
    #[test]
    fn no_command_can_declare_a_breach_or_reclassify_an_outcome() {
        let source = command_surface_source();
        for forbidden in [
            "fn mark_success",
            "fn declare_breach",
            "fn set_outcome",
            "fn reclassify",
            "fn override_result",
            "fn force_violation",
        ] {
            assert!(
                !source.contains(forbidden),
                "the security command surface contains {forbidden:?}; a security panel must never be \
                 able to declare an attack successful or rewrite an outcome"
            );
        }
        // And it must not be able to authorize or enforce anything.
        for forbidden in ["CapabilityBroker", "PolicyDecision", "CapabilityRequest"] {
            assert!(
                !source.contains(forbidden),
                "the security command surface contains {forbidden:?}; probes go through the broker \
                 like everything else, never around it"
            );
        }
    }

    /// A probe's outcome is derived from observation, and the panel cannot change its meaning.
    #[test]
    fn an_outcome_is_derived_from_the_enforcement_layers() {
        // A refused probe is a successful test, and the type says so without consulting a flag anyone
        // can set from the frontend.
        assert!(BoundaryOutcome::BrokerRefused.boundary_held());
        assert!(BoundaryOutcome::PolicyDenied.boundary_held());
        assert!(!BoundaryOutcome::NotAttempted.boundary_held());
        for outcome in [
            BoundaryOutcome::BrokerRefused,
            BoundaryOutcome::PolicyDenied,
            BoundaryOutcome::RefusedAndDenied,
            BoundaryOutcome::NotAttempted,
            BoundaryOutcome::Failed,
        ] {
            assert!(!outcome.is_security_finding());
        }
    }

    const SURFACE_SOURCE: &str = include_str!("security.rs");

    fn command_surface_source() -> &'static str {
        SURFACE_SOURCE
            .split("#[cfg(test)]")
            .next()
            .expect("the file has content before its test module")
    }
}
