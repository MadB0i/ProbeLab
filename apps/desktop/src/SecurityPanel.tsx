/**
 * The Security Lab panel.
 *
 * # What this panel shows
 *
 * The result of running the lab's own security probes through the real experiment path — target,
 * Broker, Sandbox, Observer, Evaluator — and reading back what the enforcement layers recorded. No
 * probe is simulated and no outcome is composed here.
 *
 * # The one thing this panel must never get wrong
 *
 * **A caught attempt is the boundary working.** The security probes deliberately provoke denials, and
 * the policy gate recording a `PolicyViolation` while refusing the request is exactly the behaviour we
 * want. Reporting that as a breach would make a lab that catches every attack look identical to one
 * under constant attack — and would train a reader to ignore the number that actually matters.
 *
 * So every row is labelled with which layer acted, and there is deliberately no "attacks blocked"
 * headline, no severity score, and no combined total. A panel that had to rank the probes would be
 * ranking how interesting they are to watch, not how safe the lab is.
 */

import type {
  BoundaryOutcome,
  SecurityLabReport,
  SecurityProbeListing,
  SecurityProbeReport,
} from "./api";

/** How a boundary answered, in the panel's language. */
export function outcomeLabel(outcome: BoundaryOutcome): string {
  switch (outcome) {
    case "broker_refused":
      return "BROKER REFUSED";
    case "policy_denied":
      return "POLICY DENIED";
    case "refused_and_denied":
      return "REFUSED AND DENIED";
    case "not_attempted":
      return "NOT ATTEMPTED";
    default:
      return "FAILED";
  }
}

/** The tone for an outcome, driven by meaning rather than magnitude. */
export function outcomeTone(outcome: BoundaryOutcome): string {
  switch (outcome) {
    case "broker_refused":
    case "policy_denied":
    case "refused_and_denied":
      // The boundary held. Cool and calm, not alarming.
      return "tone-held";
    case "not_attempted":
      // Nothing was tested. Deliberately not styled as a pass.
      return "tone-untested";
    default:
      return "tone-failed";
  }
}

/** A one-line explanation of what the outcome means, shown under each row. */
export function outcomeExplanation(outcome: BoundaryOutcome): string {
  switch (outcome) {
    case "broker_refused":
      return "The capability broker declined the request. Enforcement working as designed.";
    case "policy_denied":
      return "The policy gate recorded the attempt and refused it. The boundary held.";
    case "refused_and_denied":
      return "The gate recorded it and the broker declined it. Both layers acted.";
    case "not_attempted":
      return "The attempt never reached an enforcement layer, so nothing was tested. Not a pass.";
    default:
      return "The probe could not run.";
  }
}

/** Renders a category/count pair list, or states the absence plainly. */
function Breakdown({
  title,
  entries,
  emptyText,
}: {
  title: string;
  entries: [string, number][];
  emptyText: string;
}) {
  return (
    <div className="probe-breakdown">
      <h5>{title}</h5>
      {entries.length === 0 ? (
        <p className="probe-breakdown-empty">{emptyText}</p>
      ) : (
        <ul>
          {entries.map(([category, count]) => (
            <li key={category}>
              <span className="probe-category">{category.replace(/_/g, " ")}</span>
              <span className="probe-count">{count}</span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

function ProbeRow({ probe }: { probe: SecurityProbeReport }) {
  return (
    <li className={`probe-row ${outcomeTone(probe.outcome)}`}>
      <div className="probe-row-head">
        <span className="probe-name">{probe.probeName}</span>
        <span className={`probe-outcome ${outcomeTone(probe.outcome)}`}>
          {outcomeLabel(probe.outcome)}
        </span>
      </div>
      <p className="probe-id mono">{probe.probeId}</p>
      <p className="probe-explanation">{outcomeExplanation(probe.outcome)}</p>
      <dl className="probe-facts">
        <div>
          <dt>Target</dt>
          <dd>{probe.benchmarkId}</dd>
        </div>
        <div>
          <dt>Seed</dt>
          <dd className="mono">{probe.seed}</dd>
        </div>
        <div>
          <dt>Episodes</dt>
          <dd className="mono">{probe.episodes}</dd>
        </div>
        <div>
          <dt>Containment</dt>
          <dd>
            {probe.failure ? "Run failed" : "Held — no action executed"}
          </dd>
        </div>
      </dl>
      <Breakdown
        title="Policy-gate records"
        entries={probe.integrityEvents}
        emptyText="Nothing recorded."
      />
      <Breakdown
        title="Broker refusals"
        entries={probe.brokerRefusals}
        emptyText="No capability request reached the broker."
      />
      {probe.failure ? <p className="probe-failure">{probe.failure}</p> : null}
    </li>
  );
}

export function SecurityPanel({
  probes,
  report,
  error,
  running,
  benchmarkId,
  onRun,
  onBenchmarkChange,
  canRun,
}: {
  probes: SecurityProbeListing[];
  report: SecurityLabReport | null;
  error: string | null;
  running: boolean;
  benchmarkId: string;
  onRun: () => void;
  onBenchmarkChange: (benchmark: string) => void;
  canRun: boolean;
}) {
  if (error) {
    return (
      <section className="panel security-panel" aria-labelledby="security-heading">
        <div className="panel-heading">
          <h2 id="security-heading">SECURITY LAB</h2>
          <span className="tone-error">COMMAND FAILED</span>
        </div>
        <p className="brain-error" role="alert">
          {error}
        </p>
        <p className="panel-note">
          Probes run through the real enforcement path. Nothing on this panel is simulated, and a
          failure to run is reported rather than filled in.
        </p>
      </section>
    );
  }

  const held = report?.outcomes.filter((probe) =>
    ["broker_refused", "policy_denied", "refused_and_denied"].includes(probe.outcome),
  ).length ?? 0;
  const untested = report?.outcomes.filter((probe) => probe.outcome === "not_attempted").length ?? 0;
  // isSecurityFinding is always false on the wire and asserted as such in the panel's tests, so
  // this is 0 by construction rather than by omission.
  const findings = report?.outcomes.filter((probe) => probe.isSecurityFinding === true).length ?? 0;

  return (
    <section className="panel security-panel" aria-labelledby="security-heading">
      <div className="panel-heading">
        <h2 id="security-heading">SECURITY LAB</h2>
        <span className={report ? "security-status security-status--run" : "security-status security-status--idle"}>
          {report ? `SWEEP AGAINST ${report.benchmarkId}` : "NO SWEEP RUN"}
        </span>
      </div>

      {/*
        * The statement below is the panel's most important sentence. The absence of an "attacks blocked"
        * counter is deliberate: the probes are supposed to be caught, so a rising number of caught
        * attempts is evidence the probes work, not evidence the lab is failing.
        */}
      <p className="security-authority" role="note">
        Every probe here is a <em>test of the boundary</em>, not a subject under evaluation. An attempt
        the policy gate records and refuses is the lab working. A finding would mean an action
        <em> executed</em> when it should not have — and this panel reports the observed facts rather
        than claiming a score it cannot compute.
      </p>

      {report ? (
        <div className="security-summary">
          <div className="security-summary-card security-summary-card--held">
            <span className="security-summary-label">Boundaries held</span>
            <span className="security-summary-value">{held}</span>
            <span className="security-summary-gloss">
              An enforcement layer acted on the attempt
            </span>
          </div>
          <div className="security-summary-card">
            <span className="security-summary-label">Never reached an enforcement layer</span>
            <span className="security-summary-value">{untested}</span>
            <span className="security-summary-gloss">
              Nothing was tested. Not a pass
            </span>
          </div>
          <div className="security-summary-card">
            <span className="security-summary-label">Findings about the lab</span>
            <span className="security-summary-value">{findings}</span>
            <span className="security-summary-gloss">
              Empty is the expected result for this sweep
            </span>
          </div>
        </div>
      ) : null}

      <div className="security-controls">
        <div className="field-group">
          <label htmlFor="security-benchmark">TARGET BENCHMARK</label>
          <select
            id="security-benchmark"
            value={benchmarkId}
            onChange={(event) => onBenchmarkChange(event.target.value)}
            disabled={running}
          >
            <option value="grid-world">Grid World</option>
            <option value="sandbox-process">Sandbox Process</option>
          </select>
        </div>
        <button
          type="button"
          className="brain-run-button"
          onClick={onRun}
          disabled={!canRun || running}
        >
          {running ? "SWEEPING…" : "RUN SECURITY SWEEP"}
        </button>
      </div>

      {report === null ? (
        <div className="security-empty">
          <h3>NO SWEEP RUN</h3>
          <p>
            {probes.length === 0
              ? "No security probes are registered against this target."
              : `${probes.length} probes are registered and ready. Running a sweep executes each one ` +
                "through the real broker and sandbox path and reports what the enforcement layers did."}
          </p>
        </div>
      ) : (
        <>
          <section className="brain-section">
            <h3>Probe results</h3>
            <p className="section-note">
              One row per probe, in registry order. Each reports which layer acted and what it recorded.
            </p>
            <ul className="probe-list">
              {report.outcomes.map((probe) => (
                <ProbeRow key={probe.probeId} probe={probe} />
              ))}
            </ul>
          </section>

          {/*
            * The two breakdowns are rendered apart and never summed. A combined "security events"
            * figure would rise as the sandbox worked correctly, which is the specific confusion this
            * lab exists to prevent.
            */}
          <section className="brain-section">
            <h3>Totals by layer</h3>
            <div className="security-totals">
              <div className="security-total-block security-total-block--violations">
                <h4>Policy-gate records</h4>
                <p className="security-count">{report.summary.integrityViolations}</p>
                <ul>
                  {Object.entries(report.summary.violationsByCategory).map(([key, count]) => (
                    <li key={key}>
                      <span>{key.replace(/_/g, " ")}</span>
                      <span className="mono">{count}</span>
                    </li>
                  ))}
                </ul>
              </div>
              <div className="security-total-block security-total-block--refusals">
                <h4>Broker refusals</h4>
                <p className="security-count">{report.summary.brokerRefusals}</p>
                <ul>
                  {Object.entries(report.summary.refusalsByCategory).map(([key, count]) => (
                    <li key={key}>
                      <span>{key.replace(/_/g, " ")}</span>
                      <span className="mono">{count}</span>
                    </li>
                  ))}
                </ul>
              </div>
            </div>
            <p className="section-note">
              Deliberately not combined. A single figure would rise as enforcement worked correctly,
              making a lab that catches every attempt look like one under constant attack.
            </p>
          </section>
        </>
      )}
    </section>
  );
}
