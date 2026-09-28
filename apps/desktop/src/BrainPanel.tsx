/**
 * The Lab Brain panel.
 *
 * # What this component is
 *
 * A window onto the orchestrator that actually exists in `crates/lab/src/brain.rs`. Every value it
 * renders comes from the `BrainReadModel` the backend projects; there is no local state that
 * fabricates, estimates, or defaults a number. When the backend has nothing, this shows "nothing".
 *
 * # The one rule the whole component is organised around
 *
 * **A correct refusal and a real violation must never look alike.** A broker denying a write and a
 * policy gate recording a sandbox-escape attempt are different facts about different layers, and this
 * lab exists partly to keep them apart. So the security section renders two separate blocks, labels
 * them as what they are, and never sums them. `SecuritySummary` has no total field, and the component
 * does not invent one.
 *
 * # What this component deliberately does not do
 *
 * It shows no reasoning narrative and no confidence percentage. The backend stores neither, and a
 * panel that displayed a model's chain-of-thought would be presenting something it cannot verify. The
 * decisions shown are discrete structured facts read out of the plan: which benchmark, which agent,
 * how many episodes, and whether to continue or stop.
 */

import type {
  BrainReadModel,
  DecisionView,
  ExperimentView,
  SecuritySummary,
  SignalView,
  TerminalReasonView,
  TimelineEntryView,
} from "./api";

/**
 * Formats a success fraction, or refuses to.
 *
 * The `null` branch is the reason this is a function. `meanSuccessFraction` is `null` when no episode
 * completed, and a template string would happily print "0%" for it — making a run that measured
 * nothing indistinguishable from a model that failed every episode. So the absence is stated in words.
 */
export function formatScore(fraction: number | null): { text: string; known: boolean } {
  if (fraction === null || !Number.isFinite(fraction)) {
    return { text: "No completed episodes — no score exists yet", known: false };
  }
  return { text: `${(fraction * 100).toFixed(1)}%`, known: true };
}

/** Human wording for a Brain phase, without inventing phases the backend does not have. */
export function brainStateLabel(state: string): string {
  return state.replace(/([a-z])([A-Z])/g, "$1 $2").toUpperCase();
}

/** Human wording for a lifecycle state. */
export function lifecycleLabel(state: string): string {
  return state.replace(/_/g, " ").toUpperCase();
}

/** Human wording for a terminal reason, which the backend tags as `snake_case{kind,detail}`. */
/**
 * A human label for why a run stopped.
 *
 * `reason` is the adjacently tagged object the backend sends — `{"kind": "completed"}`, or
 * `{"kind": "provider_failure", "detail": {"kind": "timeout"}}` — not a string. This function used to
 * take a `string` and call `.replace()` on it, which threw on every real completed run. The string
 * branch is kept only as a defensive fallback: if the shape ever changes again, an unreadable label
 * is a far better outcome than a panel that throws while rendering a finished experiment.
 */
export function terminalLabel(reason: TerminalReasonView | null): string {
  if (reason === null) {
    return "Still running";
  }
  const kind =
    typeof reason === "string" ? reason : typeof reason.kind === "string" ? reason.kind : null;
  if (kind === null) {
    return "Ended for an unrecognised reason";
  }
  const words = kind.replace(/_/g, " ");
  // The tagged variants carry the adapter's own classification, and dropping it would report a
  // provider timeout as a bare "provider failure" — less specific than the backend already knows.
  const detail = typeof reason === "string" ? null : reason.detail;
  if (detail !== undefined && detail !== null && typeof detail === "object" && "kind" in detail) {
    const inner = (detail as { kind: unknown }).kind;
    if (typeof inner === "string" && inner.length > 0) {
      return `${words} (${inner.replace(/_/g, " ")})`;
    }
  }
  return words;
}

/** The tone for a timeline entry. Never conveys meaning by colour alone — see the text label. */
function severityTone(severity: string): string {
  switch (severity) {
    case "critical":
      return "tone-critical";
    case "error":
      return "tone-error";
    case "warning":
      return "tone-warning";
    default:
      return "tone-info";
  }
}

/** The tone for a monitoring signal, driven by interpretation rather than magnitude. */
function signalTone(signal: SignalView): string {
  switch (signal.interpretation) {
    case "violation":
      return "tone-violation";
    case "correct_refusal":
      return "tone-refusal";
    case "protocol_shortfall":
      return "tone-warning";
    case "operational":
      return "tone-error";
    default:
      return "tone-info";
  }
}

/** Plain-language explanation of what an interpretation means, so colour is never the only signal. */
function interpretationText(signal: SignalView): string {
  switch (signal.interpretation) {
    case "correct_refusal":
      return "enforcement working";
    case "violation":
      return "boundary violation";
    case "protocol_shortfall":
      return "response-format shortfall";
    case "operational":
      return "harness or provider";
    default:
      return "routine";
  }
}

function DecisionRow({ decision }: { decision: DecisionView }) {
  return (
    <li className="brain-decision">
      <span className="decision-kind">{decision.kind.replace(/_/g, " ").toUpperCase()}</span>
      <span className="decision-arrow" aria-hidden="true">
        →
      </span>
      <span className="decision-value">{decision.value}</span>
      <span className="decision-rationale">{decision.rationale}</span>
    </li>
  );
}

/**
 * The two security blocks, rendered apart.
 *
 * This is the component's most important function and the reason it is not a loop over one array: a
 * single combined list would invite a reader — and a future edit — to treat the two as comparable.
 */
function SecurityBlocks({ security }: { security: SecuritySummary }) {
  const quiet =
    security.integrityViolations === 0 && security.brokerRefusals === 0;
  if (quiet) {
    return (
      <div className="brain-security-quiet">
        <p className="brain-security-empty">
          No boundary was tested and nothing was refused. A clean run in both directions — this is not
          the same as a passing grade, it is an absence of attempts.
        </p>
      </div>
    );
  }
  return (
    <div className="brain-security">
      <section className="security-block security-block--violations">
        <h4>Policy violations</h4>
        <p className="security-count">{security.integrityViolations}</p>
        <p className="security-gloss">
          Boundary attempts the policy gate recorded as violations. These are security findings.
        </p>
        <CategoryBreakdown title="By boundary" entries={security.violationsByCategory} />
      </section>
      <section className="security-block security-block--refusals">
        <h4>Broker refusals</h4>
        <p className="security-count">{security.brokerRefusals}</p>
        <p className="security-gloss">
          Capability requests the broker declined. The system working — deliberately not counted with
          the violations above.
        </p>
        <CategoryBreakdown title="By capability" entries={security.refusalsByCategory} />
      </section>
      {security.invalidActions > 0 ? (
        <section className="security-block security-block--violations">
          <h4>Invalid actions</h4>
          <p className="security-count">{security.invalidActions}</p>
          <p className="security-gloss">
            Moves the environment rejected as illegal. A protocol measure, not a boundary finding.
          </p>
        </section>
      ) : null}
    </div>
  );
}

function CategoryBreakdown({
  title,
  entries,
}: {
  title: string;
  entries: Record<string, number>;
}) {
  const rows = Object.entries(entries);
  if (rows.length === 0) {
    return null;
  }
  return (
    <div className="security-breakdown">
      <h5>{title}</h5>
      <dl>
        {rows.map(([key, count]) => (
          <div key={key}>
            <dt>{key.replace(/_/g, " ")}</dt>
            <dd>{count}</dd>
          </div>
        ))}
      </dl>
    </div>
  );
}

function TimelineRow({ entry }: { entry: TimelineEntryView }) {
  return (
    <li className={`brain-event ${severityTone(entry.severity)}`}>
      <span className="event-sequence" title="Event order within the run">
        {String(entry.sequence).padStart(2, "0")}
      </span>
      <span className="event-code" title={`Source: ${entry.source}`}>
        {entry.code}
      </span>
      <span className="event-severity">{entry.severity}</span>
      <span className="event-detail">{entry.detail}</span>
      {entry.integrity ? (
        <span className="event-integrity" title="Classified by the infrastructure, not the model">
          {entry.integrity.replace(/_/g, " ")}
        </span>
      ) : null}
    </li>
  );
}

function ExperimentBody({ experiment }: { experiment: ExperimentView }) {
  const score = formatScore(experiment.meanSuccessFraction);
  return (
    <>
      <dl className="brain-facts">
        <div>
          <dt>Experiment</dt>
          <dd className="mono">{experiment.experimentId}</dd>
        </div>
        <div>
          <dt>Benchmark</dt>
          <dd>{experiment.benchmarkId}</dd>
        </div>
        <div>
          <dt>Agent</dt>
          <dd>{experiment.targetAgent}</dd>
        </div>
        <div>
          <dt>Provider</dt>
          <dd>{experiment.provider ?? "Not recorded"}</dd>
        </div>
        <div>
          <dt>Model</dt>
          <dd>{experiment.model ?? "Not recorded"}</dd>
        </div>
        <div>
          <dt>Probes</dt>
          <dd>
            {experiment.probeAgents.length === 0 ? "None applied" : experiment.probeAgents.join(", ")}
          </dd>
        </div>
        <div>
          <dt>Episodes</dt>
          <dd>
            {experiment.episodesCompleted} completed
            {experiment.episodesFailed > 0 ? `, ${experiment.episodesFailed} failed` : ""} of{" "}
            {experiment.episodesRequested}
          </dd>
        </div>
        <div>
          <dt>Score</dt>
          <dd className={score.known ? "score-known" : "score-unknown"}>{score.text}</dd>
        </div>
        <div>
          <dt>Lifecycle</dt>
          <dd>{lifecycleLabel(experiment.lifecycle)}</dd>
        </div>
        <div>
          <dt>Ended</dt>
          <dd>{terminalLabel(experiment.terminalReason)}</dd>
        </div>
      </dl>

      {experiment.failures.length > 0 ? (
        <div className="brain-failures">
          <h4>Episodes that could not be evaluated</h4>
          <ul>
            {experiment.failures.map((failure) => (
              <li key={`${failure.index}-${failure.seed}`}>
                <span className="failure-index">Episode {failure.index}</span>
                <span className="failure-seed">seed {failure.seed}</span>
                <span className="failure-detail">{failure.detail}</span>
              </li>
            ))}
          </ul>
        </div>
      ) : null}
    </>
  );
}

/**
 * The panel.
 *
 * `model === null` is the loading or failed state and is rendered as a real state, never as a panel of
 * zeros. There is no skeleton of invented numbers.
 */
export function BrainPanel({
  model,
  error,
  running,
  onRun,
  canRun,
}: {
  model: BrainReadModel | null;
  error: string | null;
  running: boolean;
  onRun: () => void;
  canRun: boolean;
}) {
  if (error) {
    return (
      <section className="panel brain-panel" aria-labelledby="brain-heading">
        <div className="panel-heading">
          <h2 id="brain-heading">LAB BRAIN</h2>
          <span className="tone-error">COMMAND FAILED</span>
        </div>
        <p className="brain-error" role="alert">
          {error}
        </p>
        <p className="panel-note">
          The panel shows only what the backend reported. Nothing here is reconstructed locally.
        </p>
      </section>
    );
  }

  if (model === null) {
    return (
      <section className="panel brain-panel" aria-labelledby="brain-heading">
        <div className="panel-heading">
          <h2 id="brain-heading">LAB BRAIN</h2>
          <span className="muted">CONNECTING</span>
        </div>
        <p className="panel-note">Waiting for the orchestrator to report its state.</p>
      </section>
    );
  }

  const experiment = model.experiment;
  const idle = experiment === null;

  return (
    <section className="panel brain-panel" aria-labelledby="brain-heading">
      <div className="panel-heading">
        <h2 id="brain-heading">LAB BRAIN</h2>
        <span className={`brain-state ${running ? "tone-running" : "tone-idle"}`}>
          {brainStateLabel(model.brainState)}
        </span>
      </div>

      {/*
       * The panel describes what the Brain may do. It is here because the absence of any override
       * control is a design decision a user should be able to verify, not something to take on trust.
       */}
      <p className="brain-authority" role="note">
        The Brain plans and monitors. It cannot authorize a capability, override a refusal, or change
        containment — those belong to the broker and the sandbox alone.
      </p>

      <div className="brain-status-grid">
        <div className="brain-status-card">
          <span className="status-label">Phase</span>
          <span className="status-value">{brainStateLabel(model.brainState)}</span>
        </div>
        <div className="brain-status-card">
          <span className="status-label">Next intent</span>
          <span className="status-value">
            {model.nextIntent.trim().length > 0 ? model.nextIntent : "None stated"}
          </span>
        </div>
        <div className="brain-status-card">
          <span className="status-label">Events observed</span>
          <span className="status-value">{model.observedEvents}</span>
        </div>
        <div className="brain-status-card">
          <span className="status-label">Findings</span>
          <span className="status-value">
            {model.findings.length}
            {model.findings.length > 0 ? (
              <span className="status-sub">
                {" "}
                ({model.findings.filter((f) => f.isSecurityFinding).length} security)
              </span>
            ) : null}
          </span>
        </div>
      </div>

      <div className="brain-actions">
        {/*
         * One control, and it starts work rather than steering it. There is deliberately no stop
         * button: the runner has no cancellation path, and a button that only changed local state
         * would be worse than no button.
         */}
        <button
          type="button"
          className="brain-run-button"
          onClick={onRun}
          disabled={!canRun || running}
        >
          {running ? "EXPERIMENT RUNNING…" : "RUN EXPERIMENT"}
        </button>
        {!canRun ? (
          <p className="brain-action-note">Select a benchmark and an agent to enable this.</p>
        ) : null}
      </div>

      {idle ? (
        <div className="brain-idle">
          <h3>No experiment under supervision</h3>
          <p>
            The Brain has been given nothing to run. This is its normal state between experiments, not
            an error — it is not idling because something failed.
          </p>
        </div>
      ) : (
        <>
          <section className="brain-section">
            <h3>Current experiment</h3>
            <ExperimentBody experiment={experiment} />
          </section>

          <section className="brain-section">
            <h3>Decisions recorded</h3>
            <p className="section-note">
              Structured choices read from the plan. This is not a reasoning narrative — the orchestrator
              stores none.
            </p>
            <ul className="brain-decisions">
              {experiment.decisions.map((decision) => (
                <DecisionRow key={decision.kind} decision={decision} />
              ))}
            </ul>
          </section>

          <section className="brain-section">
            <h3>Monitoring</h3>
            {model.signals.length === 0 ? (
              <p className="panel-note">
                Nothing to report. Signals appear only when there is something to count — a clean run
                shows no rows rather than a page of zeros.
              </p>
            ) : (
              <ul className="brain-signals">
                {model.signals.map((signal) => (
                  <li key={signal.id} className={`brain-signal ${signalTone(signal)}`}>
                    <span className="signal-label">{signal.label}</span>
                    <span className="signal-count">{signal.count}</span>
                    <span className="signal-meaning">{interpretationText(signal)}</span>
                  </li>
                ))}
              </ul>
            )}
          </section>

          <section className="brain-section">
            <h3>Security</h3>
            <SecurityBlocks security={experiment.security} />
          </section>

          {model.findings.length > 0 ? (
            <section className="brain-section">
              <h3>Anomalies</h3>
              <p className="section-note">
                Patterns the monitor found. A repeated pattern is not automatically a security finding.
              </p>
              <ul className="brain-findings">
                {model.findings.map((finding) => (
                  <li
                    key={finding.id}
                    className={`brain-finding ${finding.isSecurityFinding ? "tone-violation" : "tone-info"}`}
                  >
                    <span className="finding-id">{finding.id.replace(/_/g, " ")}</span>
                    <span className="finding-count">×{finding.occurrences}</span>
                    <span className="finding-kind">
                      {finding.isSecurityFinding ? "SECURITY FINDING" : "PATTERN ONLY"}
                    </span>
                    <span className="finding-interpretation">{finding.interpretation}</span>
                  </li>
                ))}
              </ul>
            </section>
          ) : null}

          <section className="brain-section">
            <h3>Event timeline</h3>
            <p className="section-note">
              The experiment&rsquo;s own recorded events, in order. Nothing here is generated by the UI.
            </p>
            {experiment.timeline.length === 0 ? (
              <p className="panel-note">This run recorded no events.</p>
            ) : (
              <ol className="brain-timeline">
                {experiment.timeline.map((entry) => (
                  <TimelineRow key={entry.sequence} entry={entry} />
                ))}
              </ol>
            )}
          </section>
        </>
      )}
    </section>
  );
}
