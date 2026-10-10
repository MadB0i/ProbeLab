/**
 * The benchmark catalog panel.
 *
 * The important column is `capabilityExecution`. A benchmark that declares `filesystem.write`,
 * `filesystem.read`, and `network.connect` has them *authorized* by the broker and *refused at
 * execution time* — only `process.spawn` has a real executor. A row that rendered that as a working
 * feature would be claiming a sandbox the lab does not have, so the deferred state is rendered as
 * deferred, in the row the user is looking at, rather than left to a document they may not read.
 */

import type { BenchmarkDetail } from "./api";

const EXECUTION_LABEL: Record<string, { text: string; tone: string; note: string }> = {
  executed: {
    text: "Executed",
    tone: "tone-ok",
    note: "Every declared capability has a real executor.",
  },
  authorized_deferred: {
    text: "Authorized / execution deferred",
    tone: "tone-deferred",
    note:
      "The broker authorizes this and refuses it at execution time. The run stops explicitly. This is "
      + "not a working feature.",
  },
  not_applicable: {
    text: "No extra capabilities",
    tone: "tone-neutral",
    note: "The benchmark grants nothing beyond ordinary movement.",
  },
};

function BenchmarkRow({ benchmark }: { benchmark: BenchmarkDetail }) {
  const execution = EXECUTION_LABEL[benchmark.capabilityExecution] ?? {
    text: benchmark.capabilityExecution,
    tone: "tone-neutral",
    note: "",
  };
  return (
    <div className="benchmark-row">
      <div className="benchmark-row-head">
        <span className="benchmark-name">{benchmark.name}</span>
        <span className={`badge ${execution.tone}`}>{execution.text}</span>
      </div>
      <p className="benchmark-description">{benchmark.description}</p>
      <dl className="benchmark-facts">
        <div>
          <dt>Measures</dt>
          <dd>{benchmark.capabilityDimension}</dd>
        </div>
        <div>
          <dt>Ceiling</dt>
          <dd>
            {benchmark.referenceAgent ??
              "None registered — nothing to compare this against yet."}
          </dd>
        </div>
        <div>
          <dt>Replay</dt>
          <dd>
            {benchmark.replaySupport === "deterministic"
              ? "Re-running reproduces the digest."
              : "Only the recorded view; no re-execution is claimed."}
          </dd>
        </div>
        <div>
          <dt>Platform</dt>
          <dd>{benchmark.platformRequirements}</dd>
        </div>
        <div>
          <dt>Version</dt>
          <dd>{benchmark.version}</dd>
        </div>
      </dl>
      <p className="benchmark-scoring">
        <strong>Scoring:</strong> {benchmark.scoring}
      </p>
      {execution.note ? <p className="benchmark-execution-note">{execution.note}</p> : null}
      {Object.keys(benchmark.limits).length > 0 ? (
        <ul className="benchmark-limits">
          {Object.entries(benchmark.limits).map(([key, value]) => (
            <li key={key}>
              <span className="limit-key">{key}</span>
              <span className="limit-value">{value}</span>
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}

export function BenchmarkCatalog({
  benchmarks,
  onSelect,
  selectedId,
}: {
  benchmarks: BenchmarkDetail[];
  onSelect?: (benchmark: BenchmarkDetail) => void;
  /**
   * The benchmark that will actually run, so the catalog can say which one is chosen.
   *
   * Separate from `onSelect` because a picker and a selection indicator are different facts. A
   * component that only knew it had a handler would render a list of identical buttons and leave the
   * reader to guess which one was active; passing the current id lets the row carry `aria-current`
   * and change its label, so the choice is stated rather than inferred from what is not highlighted.
   */
  selectedId?: string | null;
}) {
  if (benchmarks.length === 0) {
    return (
      <p className="empty">
        No benchmarks are registered. Every build ships at least the grid world and the sandbox-process
        benchmark.
      </p>
    );
  }
  return (
    <ul className="benchmark-list">
      {benchmarks.map((benchmark) => {
        const isSelected = benchmark.id === selectedId;
        return (
          /*
           * The `<li>` is the entry itself.
           *
           * It used to be a `<div class="benchmark-item">` wrapping a `<li class="benchmark-row">`,
           * which put a non-list element directly inside a `<ul>`. The browser's parser is
           * entitled to close the list when it sees a `<div>`, so the list semantics a screen reader
           * relies on were not reliably present -- and nothing about the entry needed a wrapper the
           * `<li>` could not be.
           */
          <li
            key={benchmark.id}
            className={isSelected ? "benchmark-item benchmark-item-active" : "benchmark-item"}
          >
            {onSelect ? (
              <button
                type="button"
                className="benchmark-select"
                onClick={() => onSelect(benchmark)}
                /*
                 * `aria-current` rather than a pressed state: this chooses which benchmark a run will
                 * use, which is navigation within a set, not a toggle. Announcing "current" is the
                 * fact; a colour change alone is not.
                 */
                aria-current={isSelected ? "true" : undefined}
                aria-label={
                  isSelected
                    ? `${benchmark.name}, currently selected`
                    : `Select ${benchmark.name}`
                }
              >
                {isSelected ? "Selected" : "Select"}
              </button>
            ) : null}
            <BenchmarkRow benchmark={benchmark} />
          </li>
        );
      })}
    </ul>
  );
}
