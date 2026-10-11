/**
 * Task benchmarks panel: coding repair and cue memory.
 *
 * Three ways to measure, in order of how much they prove: reference policies
 * (calibration, in-process), CLI agents (a real installed agent in a
 * disposable workspace), and guided submissions (imported evidence, scored
 * independently, always marked manual). The provenance is rendered above the
 * score, because a score without its evidence kind is the thing this panel
 * must never show.
 */

import { useEffect, useState } from "react";

import type {
  GuidedPackageView,
  SavedPackage,
  TaskBenchmarkSummary,
  TaskReport,
} from "./api";
import {
  guidedTaskPackage,
  listTaskBenchmarks,
  runTaskEval,
  saveGuidedPackage,
  scoreGuidedSubmission,
} from "./api";

/** Human label for a report's provenance. The distinction is the substance. */
export function provenanceLabel(provenance: string): string {
  return provenance === "observed_run" ? "OBSERVED RUN" : "MANUAL SUBMISSION";
}

export function formatPercent(fraction: number | null): string {
  if (fraction === null) return "no score";
  return `${(fraction * 100).toFixed(1)}%`;
}

function ReportView({ report }: { report: TaskReport }) {
  const manual = report.provenance !== "observed_run";
  return (
    <div className="task-report">
      <p className={manual ? "tone-warning" : "tone-ok"} role="note">
        {provenanceLabel(report.provenance)}
        {manual ? " — evidence the lab did not observe, verified independently" : ""}
      </p>
      <dl className="brain-facts">
        <div>
          <dt>Policy</dt>
          <dd className="mono">{report.policy}</dd>
        </div>
        <div>
          <dt>Result</dt>
          <dd>
            {report.successes} of {report.trials} ({formatPercent(report.successRate)})
          </dd>
        </div>
        <div>
          <dt>Confidence</dt>
          <dd>
            {(report.intervalLower * 100).toFixed(1)}% to {(report.intervalUpper * 100).toFixed(1)}% at
            95%
          </dd>
        </div>
        <div>
          <dt>Partial credit</dt>
          <dd>{(report.meanPartialCredit * 100).toFixed(1)}%</dd>
        </div>
        <div>
          <dt>Definition</dt>
          <dd className="mono">
            {report.definitionVersion} · {report.definitionDigest.slice(0, 18)}…
          </dd>
        </div>
      </dl>
      {report.executionNotes.length > 0 ? (
        <ul className="panel-note-list" aria-label="Execution notes">
          {report.executionNotes.map((note) => (
            <li key={note} className="mono">
              {note}
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}

export function TasksPanel() {
  const [benchmarks, setBenchmarks] = useState<TaskBenchmarkSummary[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [benchmarkId, setBenchmarkId] = useState("coding-repair");
  const [policy, setPolicy] = useState("reference-optimal");
  const [running, setRunning] = useState(false);
  const [report, setReport] = useState<TaskReport | null>(null);
  const [runError, setRunError] = useState<string | null>(null);
  const [packageView, setPackageView] = useState<GuidedPackageView | null>(null);
  const [savedPackage, setSavedPackage] = useState<SavedPackage | null>(null);
  const [savingPackage, setSavingPackage] = useState(false);
  const [packageSeed, setPackageSeed] = useState("2000000");
  const [submissionText, setSubmissionText] = useState("");
  const [submissionLog, setSubmissionLog] = useState("");

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        setBenchmarks(await listTaskBenchmarks());
      } catch (err) {
        if (!cancelled) setError(err instanceof Error ? err.message : String(err));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const run = async () => {
    setRunning(true);
    setRunError(null);
    try {
      const result = await runTaskEval({ benchmarkId, policy, seeds: [], timeoutSeconds: 120 });
      setReport(result);
    } catch (err) {
      setRunError(err instanceof Error ? err.message : String(err));
    } finally {
      setRunning(false);
    }
  };

  const loadPackage = async () => {
    setRunError(null);
    try {
      const seed = Number.parseInt(packageSeed, 10);
      if (!Number.isFinite(seed)) throw new Error("the package seed must be a number");
      setPackageView(await guidedTaskPackage(benchmarkId, seed));
    } catch (err) {
      setRunError(err instanceof Error ? err.message : String(err));
    }
  };

  /**
   * Writes the generated package to disk.
   *
   * The destination is chosen by the backend from the benchmark and seed, not by anything this panel
   * sends — the package's file names come from the benchmark definition, so a caller-supplied path
   * would make every one of them a traversal candidate. Task inputs only; the hidden checks stay here.
   */
  const savePackage = async () => {
    setSavingPackage(true);
    setRunError(null);
    setSavedPackage(null);
    try {
      const seed = Number.parseInt(packageSeed, 10);
      if (!Number.isFinite(seed)) throw new Error("the package seed must be a number");
      setSavedPackage(await saveGuidedPackage(benchmarkId, seed));
    } catch (err) {
      setRunError(err instanceof Error ? err.message : String(err));
    } finally {
      setSavingPackage(false);
    }
  };

  const scoreSubmission = async () => {
    setRunError(null);
    try {
      const seed = Number.parseInt(packageSeed, 10);
      if (!Number.isFinite(seed)) throw new Error("the package seed must be a number");
      let artifacts: Record<string, string>;
      try {
        const parsed: unknown = JSON.parse(submissionText || "{}");
        if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
          throw new Error("not an object");
        }
        artifacts = parsed as Record<string, string>;
      } catch {
        throw new Error("artifacts must be a JSON object of filename to contents");
      }
      setReport(await scoreGuidedSubmission({ benchmarkId, seed, artifacts, log: submissionLog }));
    } catch (err) {
      setRunError(err instanceof Error ? err.message : String(err));
    }
  };

  if (error) {
    return (
      <section className="panel tasks-panel" aria-labelledby="tasks-heading">
        <div className="panel-heading">
          <h2 id="tasks-heading">TASK BENCHMARKS</h2>
          <span className="tone-error">LOAD FAILED</span>
        </div>
        <p className="brain-error" role="alert">
          {error}
        </p>
      </section>
    );
  }

  const selected = benchmarks.find((benchmark) => benchmark.id === benchmarkId);

  return (
    <section className="panel tasks-panel" aria-labelledby="tasks-heading">
      <div className="panel-heading">
        <h2 id="tasks-heading">TASK BENCHMARKS</h2>
        <span className="muted">{benchmarks.length} BENCHMARKS</span>
      </div>
      <p className="panel-note">
        Coding repair and cue memory, measured over seed pools with hidden checks and a Wilson
        interval. Reference policies run here directly. Installed coding agents are measured through
        guided packages below — automatic execution is disabled because an agent running as your user
        cannot be isolated from your files, and a workdir is not a boundary.
      </p>

      {/*
        Each benchmark is a card, not a stacked list entry.

        The entries read as one continuous column of text -- a name, a version line, a description, a
        limits list -- because every line ran full width with the same weight. Two benchmarks fit
        comfortably side by side, and side by side is where comparing them stops being work.
      */}
      <ul className="task-benchmark-grid">
        {benchmarks.map((benchmark) => (
          <li key={benchmark.id} className="task-benchmark-card">
            <div className="task-benchmark-head">
              <span className="executor-label">{benchmark.name}</span>
              <span className="mono task-benchmark-version">v{benchmark.version}</span>
            </div>
            <p className="task-benchmark-seeds mono">
              {benchmark.devSeeds} dev / {benchmark.evalSeeds} eval
            </p>
            <p className="panel-note">{benchmark.description}</p>
            <ul className="panel-note-list" aria-label={`Limits of ${benchmark.name}`}>
              {benchmark.limits.map((limit) => (
                <li key={limit}>{limit}</li>
              ))}
            </ul>
          </li>
        ))}
      </ul>

      <div className="task-controls">
        <label>
          Benchmark
          <select value={benchmarkId} onChange={(event) => setBenchmarkId(event.target.value)}>
            {benchmarks.map((benchmark) => (
              <option key={benchmark.id} value={benchmark.id}>
                {benchmark.name}
              </option>
            ))}
          </select>
        </label>
        <label>
          Policy
          <select value={policy} onChange={(event) => setPolicy(event.target.value)}>
            <option value="reference-optimal">reference-optimal (ceiling)</option>
            <option value="reference-baseline">reference-baseline (floor)</option>
          </select>
        </label>
        {selected ? (
          <p className="panel-note">
            {selected.evalSeeds} evaluation seeds · digest{" "}
            <span className="mono">{selected.definitionDigest.slice(0, 18)}…</span>
          </p>
        ) : null}
        <button type="button" className="button-quiet" onClick={() => void run()} disabled={running}>
          {running ? "MEASURING…" : "RUN TASK EVALUATION"}
        </button>
      </div>

      {runError ? (
        <p className="brain-error" role="alert">
          {runError}
        </p>
      ) : null}
      {report ? <ReportView report={report} /> : null}

      <div className="guided-block">
        <h3>Guided external evaluation</h3>
        <p className="panel-note">
          For models without an automated interface: generate an isolated package, run it elsewhere,
          paste the artifacts back as a JSON object of filename to contents. The score is always
          marked as a manual submission.
        </p>
        <label>
          Package seed
          <input
            type="text"
            value={packageSeed}
            onChange={(event) => setPackageSeed(event.target.value)}
            inputMode="numeric"
          />
        </label>
        <div className="brain-run-actions">
          <button type="button" className="button-quiet" onClick={() => void loadPackage()}>
            GENERATE PACKAGE
          </button>
          {packageView ? (
            <button
              type="button"
              className="button-quiet"
              onClick={() => void savePackage()}
              disabled={savingPackage}
              title="Write this package into a ProbeLab folder under Documents"
            >
              {savingPackage ? "SAVING…" : "SAVE PACKAGE TO DISK"}
            </button>
          ) : null}
        </div>
        {savedPackage ? (
          <div className="guided-saved" role="status">
            <p>
              <strong>
                {savedPackage.fileCount} file{savedPackage.fileCount === 1 ? "" : "s"} written
              </strong>{" "}
              to <span className="mono">{savedPackage.directory}</span>
            </p>
            <p className="panel-note">
              Task inputs only. Hidden checks, scoring logic, and credentials are never written there —
              verification stays in ProbeLab. A second save to the same folder is refused rather than
              overwriting what you may have edited.
            </p>
          </div>
        ) : null}
        {packageView ? (
          <div className="guided-package">
            <p className="mono">
              {packageView.benchmarkId} v{packageView.benchmarkVersion} · seed {packageView.seed}
            </p>
            <p>{packageView.instructions}</p>
            <ul aria-label="Package files">
              {Object.keys(packageView.files).map((name) => (
                <li key={name} className="mono">
                  {name}
                </li>
              ))}
            </ul>
          </div>
        ) : null}
        <label>
          Submitted artifacts (JSON)
          <textarea
            value={submissionText}
            onChange={(event) => setSubmissionText(event.target.value)}
            rows={4}
            placeholder='{"task.py": "def compute(n): ..."}'
          />
        </label>
        <label>
          Execution log
          <textarea
            value={submissionLog}
            onChange={(event) => setSubmissionLog(event.target.value)}
            rows={2}
          />
        </label>
        <div className="brain-run-actions">
          <button type="button" className="button-quiet" onClick={() => void scoreSubmission()}>
            SCORE SUBMISSION
          </button>
        </div>
      </div>
    </section>
  );
}
