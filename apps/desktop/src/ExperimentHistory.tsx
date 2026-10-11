/**
 * The stored-run history.
 *
 * # Why this is a table and not a column of identifiers
 *
 * A returning user opens this with one question: which of these runs was mine, and did it finish? An
 * identifier cannot answer either — `1791648100986-000000000000000f` says when and nothing else. So
 * every column here is a field the durable store actually recorded, and a field the run does not have
 * is shown as an explicit absence rather than as a zero.
 *
 * # A zero is not the same as "not recorded"
 *
 * A run with no score displays `—`, not `0`. Zero is a measurement; inventing one for a run that
 * never produced a score would be the same class of error as inventing a duration.
 *
 * # Lifecycle colouring
 *
 * `interrupted` is separated from `failed` because they mean different things and the user can act on
 * them differently: an interrupted run was cut short (a crash, a restart), a failed one refused. Both
 * are called out; neither is dressed up as a pass.
 */

import type { StoredExperiment } from "./api";

/** The lifecycle, worded for a person rather than as a stored enum. */
export function lifecycleLabel(lifecycle: StoredExperiment["lifecycle"]): string {
  switch (lifecycle) {
    case "completed":
      return "Completed";
    case "running":
      return "Running";
    case "failed":
      return "Failed";
    case "interrupted":
      return "Interrupted";
    case "planned":
      return "Planned";
    default:
      return "Unknown";
  }
}

/** The score, or an explicit absence. Never a zero standing in for a missing measurement. */
export function scoreLabel(run: StoredExperiment): string {
  if (!run.hasScore || run.score === null) {
    return "—";
  }
  const value = run.score;
  if (value === 0) {
    return "0";
  }
  return Math.abs(value) < 1 ? value.toFixed(3) : value.toFixed(2);
}

/** Episodes finished, against those requested. */
export function episodeLabel(run: StoredExperiment): string {
  if (run.episodesRequested <= 0) {
    return run.episodesCompleted > 0 ? `${run.episodesCompleted}` : "—";
  }
  return `${run.episodesCompleted}/${run.episodesRequested}`;
}

/** How many episodes failed, shown only when something did. */
export function failedEpisodes(run: StoredExperiment): number {
  return run.episodesFailed;
}

/** When the record was last written. */
export function updatedLabel(run: StoredExperiment, now = Date.now()): string {
  if (!Number.isFinite(run.updatedUnixMs) || run.updatedUnixMs <= 0) {
    return "time not recorded";
  }
  const elapsed = now - run.updatedUnixMs;
  if (elapsed < 0) {
    return new Date(run.updatedUnixMs).toLocaleString();
  }
  const minutes = Math.floor(elapsed / 60_000);
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} h ago`;
  const days = Math.floor(hours / 24);
  if (days < 30) return `${days} d ago`;
  return new Date(run.updatedUnixMs).toLocaleDateString();
}

/**
 * The terminal reason, worded for a person.
 *
 * The stored value is a wire label, and it arrives as a quoted JSON string — `"completed"` — because
 * that is how the terminal reason is serialized. Rendering it verbatim put `completed` in quotes with
 * a preceding quote character in a column whose whole job is to be read at a glance. An unrecognised
 * label is shown as itself rather than being dropped: the panel's rule is that a missing fact is an
 * explicit absence, and inventing a wording for a reason it does not know would be the reverse.
 */
export function terminalReasonLabel(raw: string | null): string | null {
  if (!raw) {
    return null;
  }
  let label = raw.trim();
  try {
    const parsed: unknown = JSON.parse(label);
    if (typeof parsed === "string") {
      label = parsed;
    }
  } catch {
    // Not JSON; the label is already readable.
  }
  switch (label) {
    case "completed":
      return "reached the goal";
    case "not_success":
      return "ran out of budget";
    case "cancelled":
      return "cancelled";
    default:
      return label;
  }
}

/**
 * The agent a run measured, or an honest absence.
 *
 * A run executed through the mock has no target agent, and saying `mock` here would be an inference
 * about how it was executed rather than a recorded fact about what it measured.
 */
export function targetLabel(run: StoredExperiment): string {
  return run.targetAgent ?? "no target recorded";
}

/** A run identifier short enough to read in a table cell. */
export function runIdLabel(run: StoredExperiment, keep = 12): string {
  const id = run.experimentId;
  if (id.length <= keep + 1) {
    return id;
  }
  return `${id.slice(0, keep)}…`;
}

export function ExperimentHistory({
  runs,
  selectedId,
  loading,
  onOpen,
}: {
  runs: StoredExperiment[];
  selectedId: string | null;
  loading: boolean;
  onOpen: (experimentId: string) => void;
}) {
  if (runs.length === 0) {
    return (
      <p className="panel-note">
        No stored runs yet. Finished runs appear here and stay after the application is closed.
      </p>
    );
  }

  return (
    <div className="history-table-scroll">
      <table className="history-table">
        <caption className="visually-hidden">
          Stored experiment runs. Activate a row to load that run into the reporting panels.
        </caption>
        <thead>
          <tr>
            <th scope="col">Outcome</th>
            <th scope="col">Run</th>
            <th scope="col">Benchmark</th>
            <th scope="col">Target</th>
            <th scope="col">Episodes</th>
            <th scope="col">Score</th>
            <th scope="col">Updated</th>
          </tr>
        </thead>
        <tbody>
          {runs.map((run) => {
            const failed = failedEpisodes(run);
            const isSelected = run.experimentId === selectedId;
            const reason = terminalReasonLabel(run.terminalReason);
            return (
              <tr key={run.experimentId} className={isSelected ? "history-row--open" : undefined}>
                <td>
                  <span className={`lifecycle-pill lifecycle-pill--${run.lifecycle}`}>
                    {lifecycleLabel(run.lifecycle)}
                  </span>
                  {reason ? (
                    <span className="history-reason" title={reason}>
                      {reason}
                    </span>
                  ) : null}
                </td>
                <td>
                  <button
                    type="button"
                    className="history-open"
                    onClick={() => onOpen(run.experimentId)}
                    disabled={loading}
                    aria-current={isSelected ? "true" : undefined}
                    title={run.experimentId}
                  >
                    <span className="mono">{runIdLabel(run)}</span>
                  </button>
                </td>
                <td className="history-benchmark">{run.benchmarkId}</td>
                <td className="history-target">{targetLabel(run)}</td>
                <td className="mono">
                  {episodeLabel(run)}
                  {failed > 0 ? (
                    <span className="history-failed" title={`${failed} episode(s) failed`}>
                      {" "}
                      ({failed} failed)
                    </span>
                  ) : null}
                </td>
                <td className="mono">{scoreLabel(run)}</td>
                <td className="history-when">{updatedLabel(run)}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}