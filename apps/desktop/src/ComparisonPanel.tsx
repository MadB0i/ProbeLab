/**
 * The comparison panel.
 *
 * # What this panel is careful about
 *
 * A comparison is the easiest place in this product to say something confident and wrong. The
 * backend already refuses to compare runs that measured different things, and this panel's job is to
 * make sure that refusal and every qualification it is given are *visible*, not merely available.
 *
 * Three rules follow from that, and each has a test:
 *
 * 1. **A refusal is a result, not an error.** When runs cannot be compared, the panel shows which
 *    dimension differs and the values on each side. It does not show a generic failure, and it does
 *    not silently show nothing — a blank panel is indistinguishable from a bug.
 * 2. **A missing score is a missing score.** A run that completed no episode shows "no measurement"
 *    and an explicit dash. It never shows 0%, because 0% is a claim the environment made.
 * 3. **A confound is stated above the numbers, not below them.** When the compared runs used
 *    different agents, that is printed as a banner over the table. Burying it in a footnote is how
 *    a reader ends up treating a mock's score as a model's.
 *
 * Nothing here computes a comparison. The panel renders what the backend decided and refuses to
 * second-guess it, because a panel that recomputed a ranking would be a second, untested
 * implementation of the same judgement.
 */

import { useCallback, useEffect, useMemo, useState } from "react";
import {
  compareExperiments,
  listComparableExperiments,
  type ComparableRun,
  type Comparison,
  type ComparisonResponse,
  type StoredExperiment,
} from "./api";

/** Formats a fraction as a percentage, or states that there is none. */
function formatFraction(value: number | null): string {
  return value === null ? "—" : `${(value * 100).toFixed(1)}%`;
}

/** Formats a decimal, or states that there is none. */
function formatDecimal(value: number | null, digits = 2): string {
  return value === null ? "—" : value.toFixed(digits);
}

/** A short label for a lifecycle state. */
function lifecycleLabel(lifecycle: StoredExperiment["lifecycle"]): string {
  switch (lifecycle) {
    case "planned":
      return "PLANNED";
    case "running":
      return "RUNNING";
    case "completed":
      return "COMPLETED";
    case "failed":
      return "FAILED";
    case "interrupted":
      return "INTERRUPTED";
  }
}

/** The rows of one ranking, in the order the backend returned them. */
function StandingRows({
  title,
  standings,
  format,
  /** Whether a larger value is better, for the arrow. */
  higherIsBetter,
}: {
  title: string;
  standings: { experimentId: string; rank: number | null; value: number | null; interval: { lower: number; upper: number } | null; unrankedReason: string | null }[];
  format: (value: number | null) => string;
  higherIsBetter: boolean;
}) {
  return (
    <div className="compare-block">
      <h4 className="compare-block-title">{title}</h4>
      <table className="compare-table">
        <thead>
          <tr>
            <th scope="col">RANK</th>
            <th scope="col">EXPERIMENT</th>
            <th scope="col">VALUE</th>
            <th scope="col">INTERVAL</th>
          </tr>
        </thead>
        <tbody>
          {standings.map((standing) => (
            <tr key={`${title}-${standing.experimentId}`}>
              <td>{standing.rank === null ? "—" : `#${standing.rank}`}</td>
              <td>
                <code>{standing.experimentId}</code>
              </td>
              <td className="compare-value">
                {standing.rank === null ? (
                  <span
                    className="compare-absent"
                    title={standing.unrankedReason ?? "no value to rank"}
                  >
                    no measurement
                  </span>
                ) : (
                  <>
                    {format(standing.value)}
                    <span
                      className="compare-direction"
                      aria-label={higherIsBetter ? "higher is better" : "lower is better"}
                    >
                      {higherIsBetter ? "↑" : "↓"}
                    </span>
                  </>
                )}
              </td>
              <td>
                {standing.interval ? (
                  <span title="Wilson interval">
                    [{formatDecimal(standing.interval.lower, 3)},{" "}
                    {formatDecimal(standing.interval.upper, 3)}]
                  </span>
                ) : (
                  "—"
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/** The refusal case, rendered as an explanation rather than a failure. */
function NotComparable({
  experimentIds,
  dimensions,
}: {
  experimentIds: string[];
  dimensions: { dimension: string; values: string[] }[];
}) {
  return (
    <div className="compare-refusal" role="status">
      <p className="compare-refusal-head">
        These runs did not measure the same thing, so they are not compared.
      </p>
      <ul className="compare-refusal-list">
        {dimensions.map((dimension) => (
          <li key={dimension.dimension}>
            <strong>{dimension.dimension}</strong> differs:{" "}
            {dimension.values.map((value, index) => (
              <span key={`${dimension.dimension}-${value}-${index}`}>
                {index > 0 ? " vs " : ""}
                <code>{value}</code>
              </span>
            ))}
          </li>
        ))}
      </ul>
      <p className="compare-refusal-note">
        Ranking them would put a number on a difference in what was measured. Re-run on the same{" "}
        {dimensions.map((dimension) => dimension.dimension).join(" and ")} to compare them.
      </p>
      <p className="compare-refusal-ids">
        Requested: {experimentIds.map((id) => <code key={id}>{id}</code>)}
      </p>
    </div>
  );
}

/** The comparable case. */
function ComparisonTable({ comparison }: { comparison: Comparison }) {
  const byId = useMemo(
    () => new Map<string, ComparableRun>(comparison.runs.map((run) => [run.experimentId, run])),
    [comparison.runs],
  );

  return (
    <div className="compare-result">
      {/* The confound banner, above the numbers, where a reader's eye meets the table first. */}
      {comparison.variesAcross.length > 0 && (
        <div className="compare-confound" role="note">
          <strong>Not held constant:</strong>{" "}
          {comparison.variesAcross.map((dimension) => (
            <span key={dimension.dimension} className="compare-confound-item">
              {dimension.dimension} ({dimension.values.join(" vs ")})
            </span>
          ))}
          . The ranking below is real, but it compares runs that differ in more than episode count.
        </div>
      )}

      <dl className="compare-facts">
        <div>
          <dt>BENCHMARK</dt>
          <dd>{comparison.benchmarkId}</dd>
        </div>
        <div>
          <dt>SEED</dt>
          <dd>{comparison.rootSeed}</dd>
        </div>
        <div>
          <dt>ADAPTER</dt>
          <dd>{comparison.adapter}</dd>
        </div>
        <div>
          <dt>INTEGRITY EVENTS</dt>
          <dd>{comparison.integrityEventTotal}</dd>
        </div>
        <div>
          <dt>BROKER REFUSALS</dt>
          <dd>{comparison.brokerRefusalTotal}</dd>
        </div>
        <div>
          <dt>SECURITY FINDING</dt>
          <dd className={comparison.anySecurityFinding ? "compare-finding" : undefined}>
            {comparison.anySecurityFinding ? "RECORDED" : "NONE"}
          </dd>
        </div>
      </dl>

      <StandingRows
        title="By success fraction"
        standings={comparison.bySuccess}
        format={(value) => formatFraction(value)}
        higherIsBetter
      />
      <StandingRows
        title="By mean reward"
        standings={comparison.byReward}
        format={(value) => formatDecimal(value)}
        higherIsBetter
      />
      <StandingRows
        title="By mean turns (fewest first)"
        standings={comparison.byTurns}
        format={(value) => formatDecimal(value, 1)}
        higherIsBetter={false}
      />
      <StandingRows
        title="By mean invalid actions (fewest first)"
        standings={comparison.byInvalidActions}
        format={(value) => formatDecimal(value, 1)}
        higherIsBetter={false}
      />

      <details className="compare-runs">
        <summary>Per-run measurements</summary>
        <table className="compare-table">
          <thead>
            <tr>
              <th scope="col">EXPERIMENT</th>
              <th scope="col">AGENT</th>
              <th scope="col">EPISODES</th>
              <th scope="col">SUCCESS</th>
              <th scope="col">REWARD</th>
              <th scope="col">TURNS</th>
              <th scope="col">INVALID</th>
            </tr>
          </thead>
          <tbody>
            {comparison.runs.map((run) => (
              <tr key={run.experimentId}>
                <td>
                  <code>{run.experimentId}</code>
                </td>
                <td>{run.targetAgent}</td>
                <td>
                  {run.episodesCompleted}
                  {run.episodesFailed > 0 ? ` +${run.episodesFailed} failed` : ""}
                </td>
                <td className="compare-value">{formatFraction(run.successFraction)}</td>
                <td>{formatDecimal(run.meanReward)}</td>
                <td>{formatDecimal(run.meanTurns, 1)}</td>
                <td>{formatDecimal(run.meanInvalidActions, 1)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </details>
    </div>
  );
}

export function ComparisonPanel() {
  const [experiments, setExperiments] = useState<StoredExperiment[]>([]);
  const [selected, setSelected] = useState<string[]>([]);
  const [outcome, setOutcome] = useState<ComparisonResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [comparing, setComparing] = useState(false);

  useEffect(() => {
    let active = true;
    listComparableExperiments()
      .then((records) => {
        if (active) {
          setExperiments(records);
          setLoading(false);
        }
      })
      .catch((cause: unknown) => {
        if (active) {
          setError(
            `The stored experiments could not be read: ${cause instanceof Error ? cause.message : String(cause)}`,
          );
          setLoading(false);
        }
      });
    return () => {
      active = false;
    };
  }, []);

  const toggle = useCallback((id: string) => {
    setOutcome(null);
    setSelected((current) =>
      current.includes(id) ? current.filter((entry) => entry !== id) : [...current, id],
    );
  }, []);

  const compare = useCallback(() => {
    if (selected.length < 2) {
      return;
    }
    setComparing(true);
    setError(null);
    compareExperiments(selected)
      .then((response) => setOutcome(response))
      .catch((cause: unknown) =>
        setError(
          `The comparison could not be made: ${cause instanceof Error ? cause.message : String(cause)}`,
        ),
      )
      .finally(() => setComparing(false));
  }, [selected]);

  // A run with no stored identity cannot be held constant, so it is not offered for selection. The
  // control is disabled with a stated reason rather than silently excluded from the list.
  const unselectableReason = (experiment: StoredExperiment): string | null => {
    if (experiment.targetAgent === null || experiment.rootSeed === null) {
      return "This run predates stored plan identity, so nothing can be held constant.";
    }
    return null;
  };

  /*
   * What the selection would compare, stated before the button is pressed.
   *
   * The refusal is not a change to the panel's behaviour — the gate is untouched and still decides —
   * but a user who can see that two picked runs differ in benchmark or seed does not have to press
   * Compare and read a refusal to find out. The preview is descriptive and does not rank anything, so
   * it cannot be mistaken for the result.
   */
  const selectionPreview = useMemo(() => {
    if (selected.length < 2) {
      return null;
    }
    const picked = experiments.filter((experiment) => selected.includes(experiment.experimentId));
    const differing: string[] = [];
    for (const [label, read] of [
      ["benchmark", (run: StoredExperiment) => run.benchmarkId],
      ["seed", (run: StoredExperiment) => (run.rootSeed === null ? null : String(run.rootSeed))],
      ["target agent", (run: StoredExperiment) => run.targetAgent],
    ] as const) {
      const values = new Set(picked.map(read));
      if (values.size > 1) {
        differing.push(label);
      }
    }
    return { count: picked.length, differing };
  }, [selected, experiments]);

  return (
    <section className="panel compare-panel" aria-label="Comparison">
      <div className="panel-heading">
        <h2>Compare experiments</h2>
        <span className="muted">{experiments.length} STORED</span>
      </div>
      <p className="field-description">
        Two runs are compared only when they measured the same thing: the same benchmark, on the same
        seed. Where that does not hold, the panel says so instead of ranking them.
      </p>

      {loading && <div className="state-message">Reading the stored experiments…</div>}
      {error && (
        <div className="error-message" role="alert">
          {error}
        </div>
      )}

      {!loading && experiments.length === 0 && (
        <div className="empty-state">
          No experiments are stored yet. Run one from the command center and it will appear here.
        </div>
      )}

      {experiments.length > 0 && (
        <>
          <ul className="compare-picker">
            {experiments.map((experiment) => {
              const disabled = unselectableReason(experiment);
              const isSelected = selected.includes(experiment.experimentId);
              return (
                <li key={experiment.experimentId}>
                  <label
                    className={disabled ? "compare-pick compare-pick--disabled" : "compare-pick"}
                    title={disabled ?? undefined}
                  >
                    <input
                      type="checkbox"
                      checked={isSelected}
                      disabled={disabled !== null}
                      onChange={() => toggle(experiment.experimentId)}
                    />
                    <span className="compare-pick-id">
                      <code>{experiment.experimentId}</code>
                    </span>
                    <span className="compare-pick-meta">
                      {experiment.benchmarkId} ·{" "}
                      {experiment.targetAgent ?? "identity not recorded"} · seed{" "}
                      {experiment.rootSeed ?? "—"} · {experiment.episodesCompleted}/
                      {experiment.episodesRequested} episodes ·{" "}
                      <span className="compare-pick-lifecycle">
                        {lifecycleLabel(experiment.lifecycle)}
                      </span>
                    </span>
                    {disabled && <span className="compare-pick-note">{disabled}</span>}
                  </label>
                </li>
              );
            })}
          </ul>

          <div className="compare-actions">
            <button
              type="button"
              className="run-button"
              onClick={compare}
              disabled={selected.length < 2 || comparing}
            >
              <span>{comparing ? "COMPARING" : "COMPARE SELECTED"}</span>
              <span className="button-arrow" aria-hidden="true">
                →
              </span>
            </button>
            <span className="muted">
              {selected.length < 2
                ? "Select at least two runs."
                : `${selected.length} selected.`}
            </span>
          </div>

          {selectionPreview ? (
            selectionPreview.differing.length > 0 ? (
              <div className="compare-preview compare-preview--will-refuse" role="note">
                <strong>These {selectionPreview.count} runs differ in:</strong>{" "}
                {selectionPreview.differing.join(", ")}. Comparing them will be refused, because a
                ranking across a difference in what was measured is not a comparison.
              </div>
            ) : (
              <div className="compare-preview" role="note">
                These {selectionPreview.count} runs match on benchmark, seed, and target agent, so they can be compared.
              </div>
            )
          ) : null}

          {outcome?.kind === "comparable" && <ComparisonTable comparison={outcome.detail} />}
          {outcome?.kind === "not_comparable" && (
            <NotComparable
              experimentIds={outcome.detail.experimentIds}
              dimensions={outcome.detail.dimensions}
            />
          )}
        </>
      )}
    </section>
  );
}
