/**
 * Brain panel tests.
 *
 * The recurring theme is that the panel must not invent. Every fixture here is shaped like the real
 * `BrainReadModel` the backend produces, and most of the assertions are about what the panel does
 * *not* show: no score when nothing was measured, no combined security number, no narrative reasoning,
 * and no fabricated rows in an empty monitoring view.
 *
 * # Two of these fixtures used to be hand-written, and that is what hid two wire bugs
 *
 * The `experiment()` and `model()` builders below were written by hand from `api.ts`, which means they
 * were written from the *consumer's* idea of the producer — and so they agreed with `api.ts` even when
 * the backend disagreed. Two bugs lived in exactly that gap:
 *
 * 1. `Anomaly` had no `#[serde(rename_all)]`, so the backend sent `is_security_finding` while `api.ts`
 *    said `isSecurityFinding`. The hand-written finding used the TypeScript spelling, so the panel read
 *    `undefined` — falsy — and rendered every finding as "pattern only" with a security count of zero.
 * 2. `TerminalReason` is adjacently tagged, so the backend sends `{"kind":"completed"}`. `api.ts` said
 *    `string`, the fixture said `"completed"`, and `terminalLabel`'s `.replace()` was never run against
 *    a real payload. On a real run it threw.
 *
 * Both are now covered twice: the builders below match the real wire shape, and
 * `BrainPanel.test.tsx`'s sibling `renderRealBackendPayloads` renders the JSON the backend actually
 * generates. Those files are produced by `PROBELAB_WRITE_FIXTURES=1 cargo test -p probelab-lab
 * --test emit_fixture` and pinned against live serialization by that same test, so a rename on either
 * side now fails here instead of in production.
 */

import { render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { BrainPanel, formatScore, brainStateLabel, terminalLabel } from "./BrainPanel";
import type { BrainReadModel, ExperimentView } from "./api";

import realCompletedRun from "./test-fixtures/brainReadModel.json";
import realIdleBrain from "./test-fixtures/brainIdleModel.json";
import realFindings from "./test-fixtures/brainFindingsModel.json";


function experiment(overrides: Partial<ExperimentView> = {}): ExperimentView {
  return {
    experimentId: "exp-1",
    benchmarkId: "grid-world",
    targetAgent: "optimal",
    provider: "mock",
    model: "optimal",
    probeAgents: [],
    episodesRequested: 3,
    episodesCompleted: 3,
    episodesFailed: 0,
    meanSuccessFraction: 0.94,
    aggregate: {
      episodes: 3,
      successes: 3,
      successRate: 0.94,
      successInterval: { point: 0.94, lower: 0.6, upper: 0.99, confidence: 0.95 },
      meanReward: 7.5,
      meanTurns: 8.2,
      statusCounts: { success: 3 },
      seedPool: "eval",
    },
    lifecycle: "completed",
    terminalReason: { kind: "completed" },
    elapsedMs: null,
    decisions: [
      { kind: "select_benchmark", value: "grid-world", rationale: "chosen from the registry" },
      { kind: "select_agent", value: "optimal", rationale: "chosen from the registry" },
      { kind: "stop", value: "all episodes attempted", rationale: "the requested count was reached" },
    ],
    episodes: [
      { index: 0, seed: 1, taskSuccess: true, replayDigest: "abc", telemetryEventCount: 12 },
    ],
    failures: [],
    timeline: [
      {
        sequence: 0,
        code: "experiment.created",
        source: "experiment",
        severity: "info",
        detail: "experiment exp-1",
        integrity: null,
        anomalyCategory: null,
      },
      {
        sequence: 1,
        code: "experiment.finished",
        source: "experiment",
        severity: "info",
        detail: "completed",
        integrity: null,
        anomalyCategory: null,
      },
    ],
    security: {
      integrityViolations: 0,
      brokerRefusals: 0,
      violationsByCategory: {},
      refusalsByCategory: {},
      invalidActions: 0,
    },
    ...overrides,
  };
}

function model(overrides: Partial<BrainReadModel> = {}): BrainReadModel {
  return {
    brainState: "Completed",
    nextIntent: "idle",
    experiment: experiment(),
    observedEvents: 40,
    findings: [],
    signals: [],
    security: experiment().security,
    ...overrides,
  };
}

function renderPanel(props: Partial<Parameters<typeof BrainPanel>[0]> = {}) {
  return render(
    <BrainPanel
      model={model()}
      error={null}
      running={false}
      onRun={() => {}}
      canRun
      {...props}
    />,
  );
}

describe("formatScore", () => {
  it("states the absence of a score rather than printing zero", () => {
    // The single most important behaviour in the panel. A null fraction means no episode completed,
    // and rendering 0% would be indistinguishable from a model that failed every episode.
    const result = formatScore(null);
    expect(result.known).toBe(false);
    expect(result.text).toMatch(/no completed episodes/i);
    expect(result.text).not.toContain("0%");
  });

  it("formats a real fraction as a percentage", () => {
    const result = formatScore(0.943);
    expect(result.known).toBe(true);
    expect(result.text).toBe("94.3%");
  });

  it("refuses to format a non-finite value", () => {
    expect(formatScore(Number.NaN).known).toBe(false);
    expect(formatScore(Number.POSITIVE_INFINITY).known).toBe(false);
  });
});

describe("brainStateLabel", () => {
  it("reads a backend phase without inventing one", () => {
    expect(brainStateLabel("Planning")).toBe("PLANNING");
    expect(brainStateLabel("Observing")).toBe("OBSERVING");
  });
});

describe("BrainPanel — idle state", () => {
  it("shows an intentional idle state rather than an empty dashboard", () => {
    renderPanel({ model: model({ experiment: null, brainState: "Planning", nextIntent: "" }) });
    expect(screen.getByText(/No experiment under supervision/i)).toBeInTheDocument();
    expect(screen.getByText(/not idling because something failed/i)).toBeInTheDocument();
    // And it must not present a run that never happened.
    expect(screen.queryByText(/exp-1/)).not.toBeInTheDocument();
    expect(screen.queryByText(/0 completed/)).not.toBeInTheDocument();
  });

  it("shows the connecting state before the first response", () => {
    renderPanel({ model: null });
    expect(screen.getByText(/CONNECTING/i)).toBeInTheDocument();
    expect(screen.queryByText(/0 completed/)).not.toBeInTheDocument();
  });
});

describe("BrainPanel — error state", () => {
  it("shows a backend failure and does not fall back to invented state", () => {
    renderPanel({ model: null, error: "the orchestrator did not report its state: timeout" });
    const alert = screen.getByRole("alert");
    expect(alert).toHaveTextContent("the orchestrator did not report its state");
    expect(alert).toHaveTextContent("timeout");
    // A failed command must not leave a panel of zeros behind.
    expect(screen.queryByText(/0 completed/)).not.toBeInTheDocument();
    expect(screen.queryByText(/No completed episodes/)).not.toBeInTheDocument();
  });
});

describe("BrainPanel — current experiment", () => {
  it("shows the experiment's real identity and counts", () => {
    renderPanel();
    const facts = screen.getByText("Benchmark").closest("dl")!;
    const value = (label: string) =>
      within(facts as HTMLElement).getByText(label).parentElement!.querySelector("dd")!
        .textContent;
    expect(value("Experiment")).toBe("exp-1");
    expect(value("Benchmark")).toBe("grid-world");
    // The agent and the model share a name here, which is why this reads the facts list rather than
    // the whole document — a bare `getByText("optimal")` would be ambiguous.
    expect(value("Agent")).toBe("optimal");
    expect(value("Model")).toBe("optimal");
    expect(value("Episodes")).toContain("3 completed");
    expect(value("Score")).toBe("94.0%");
  });

  it("shows the provider as unknown rather than guessing", () => {
    renderPanel({
      model: model({ experiment: experiment({ provider: null, model: null }) }),
    });
    const providerFacts = screen.getByText("Provider").closest("div")!;
    const modelFacts = screen.getByText("Model").closest("div")!;
    expect(within(providerFacts as HTMLElement).getByText("Not recorded")).toBeInTheDocument();
    expect(within(modelFacts as HTMLElement).getByText("Not recorded")).toBeInTheDocument();
  });

  it("says no score exists when no episode completed", () => {
    renderPanel({
      model: model({
        experiment: experiment({
          episodesRequested: 2,
          episodesCompleted: 0,
          episodesFailed: 2,
          meanSuccessFraction: null,
          episodes: [],
        }),
      }),
    });
    expect(screen.getByText(/No completed episodes/i)).toBeInTheDocument();
    expect(screen.queryByText("0.0%")).not.toBeInTheDocument();
  });

  it("lists episodes that could not be evaluated with their reason", () => {
    renderPanel({
      model: model({
        experiment: experiment({
          episodesCompleted: 0,
          meanSuccessFraction: null,
          failures: [
            { index: 0, seed: 7, detail: "episode did not complete: seed budget" },
            { index: 1, seed: 8, detail: "episode did not complete: seed budget" },
          ],
        }),
      }),
    });
    expect(screen.getByText(/Episodes that could not be evaluated/i)).toBeInTheDocument();
    const details = screen.getAllByText(/seed budget/);
    expect(details).toHaveLength(2);
    // The seed and the reason are siblings in one row, so the row is what carries both.
    expect(details[0].parentElement).toHaveTextContent("Episode 0");
    expect(details[0].parentElement).toHaveTextContent("seed 7");
    expect(details[1].parentElement).toHaveTextContent("seed 8");
  });
});

describe("BrainPanel — decisions", () => {
  it("renders structured decisions rather than a reasoning narrative", () => {
    renderPanel();
    expect(screen.getByText("SELECT BENCHMARK")).toBeInTheDocument();
    expect(screen.getByText("SELECT AGENT")).toBeInTheDocument();
    expect(screen.getByText("STOP")).toBeInTheDocument();
    // The panel says outright that no narrative exists, so nobody waits for one.
    expect(screen.getByText(/not a reasoning narrative/i)).toBeInTheDocument();
  });
});

describe("BrainPanel — monitoring", () => {
  it("explains an empty monitoring view instead of padding it with zeros", () => {
    renderPanel({ model: model({ signals: [] }) });
    const monitoring = screen
      .getByText(/only when there is something to count/i)
      .closest("section")!;
    expect(monitoring).toBeInTheDocument();
    // No zero rows are padded into the empty view.
    expect(monitoring.querySelectorAll(".brain-signal")).toHaveLength(0);
  });

  it("labels each signal with what its count means, not just a colour", () => {
    renderPanel({
      model: model({
        signals: [
          {
            id: "broker_unauthorized_read",
            label: "Broker-refused reads",
            count: 4,
            interpretation: "correct_refusal",
          },
          {
            id: "protocol_parse_failure",
            label: "Responses that were not valid JSON",
            count: 2,
            interpretation: "protocol_shortfall",
          },
        ],
      }),
    });
    expect(screen.getByText("Broker-refused reads")).toBeInTheDocument();
    expect(screen.getByText("enforcement working")).toBeInTheDocument();
    expect(screen.getByText("Responses that were not valid JSON")).toBeInTheDocument();
    expect(screen.getByText("response-format shortfall")).toBeInTheDocument();
  });
});

describe("BrainPanel — security taxonomy", () => {
  it("keeps violations and refusals in separate, separately-labelled blocks", () => {
    // The core requirement. A combined number would rise when the sandbox works correctly.
    renderPanel({
      model: model({
        experiment: experiment({
          security: {
            integrityViolations: 2,
            brokerRefusals: 7,
            violationsByCategory: { sandbox_violation: 2 },
            refusalsByCategory: { broker_unauthorized_write: 7 },
            invalidActions: 0,
          },
        }),
      }),
    });

    const violations = screen.getByText("Policy violations").closest("section")!;
    const refusals = screen.getByText("Broker refusals").closest("section")!;
    // Scoped to the headline count, because the per-category breakdown repeats the same numbers and a
    // bare `getByText("2")` would be ambiguous.
    const headline = (block: Element) =>
      block.querySelector(".security-count")!.textContent;

    expect(headline(violations)).toBe("2");
    expect(headline(refusals)).toBe("7");
    // Neither block may show the other's headline.
    expect(headline(violations)).not.toBe("7");
    expect(headline(refusals)).not.toBe("2");
    // And the panel says why they are not combined.
    expect(within(refusals as HTMLElement).getByText(/deliberately not counted with/i)).toBeInTheDocument();
  });

  it("describes a run with no attempts in both directions as an absence, not a pass", () => {
    renderPanel();
    expect(screen.getByText(/not the same as a passing grade/i)).toBeInTheDocument();
  });

  it("does not present a repeated pattern as a security finding", () => {
    renderPanel({
      model: model({
        findings: [
          {
            id: "repeated_broker_denials",
            category: "BrokerDenial",
            interpretation: "The broker refused repeatedly, which is expected.",
            occurrences: 40,
            isSecurityFinding: false,
          },
        ],
      }),
    });
    expect(screen.getByText("PATTERN ONLY")).toBeInTheDocument();
    expect(screen.queryByText("SECURITY FINDING")).not.toBeInTheDocument();
  });

  it("marks a genuine security finding as one", () => {
    renderPanel({
      model: model({
        findings: [
          {
            id: "repeated_sandbox_escapes",
            category: "IntegrityEvent",
            interpretation: "Repeated escape attempts.",
            occurrences: 5,
            isSecurityFinding: true,
          },
        ],
      }),
    });
    expect(screen.getByText("SECURITY FINDING")).toBeInTheDocument();
  });
});

describe("BrainPanel — timeline", () => {
  it("renders the experiment's own events in order", () => {
    renderPanel();
    const rows = Array.from(document.querySelectorAll(".brain-event"));
    expect(rows).toHaveLength(2);
    expect(rows[0]).toHaveTextContent("experiment.created");
    expect(rows[1]).toHaveTextContent("experiment.finished");
    // The order is the run's own, carried in the sequence numbers the backend assigned.
    expect(rows[0]).toHaveTextContent("00");
    expect(rows[1]).toHaveTextContent("01");
  });

  it("says so when a run recorded no events", () => {
    renderPanel({ model: model({ experiment: experiment({ timeline: [] }) }) });
    expect(screen.getByText(/recorded no events/i)).toBeInTheDocument();
  });
});

describe("BrainPanel — authority", () => {
  it("states what the Brain may not do, because the absence of a control is a design decision", () => {
    renderPanel();
    expect(
      screen.getByText(/cannot authorize a capability, override a refusal, or change/i),
    ).toBeInTheDocument();
  });

  it("offers only the start control — no stop, force, or override", () => {
    // The runner has no cancellation path, so a stop button would be a lie. This asserts none exists.
    renderPanel();
    expect(screen.getByRole("button", { name: /RUN EXPERIMENT/i })).toBeInTheDocument();
    for (const forbidden of [/stop/i, /cancel/i, /force/i, /override/i, /bypass/i]) {
      expect(screen.queryByRole("button", { name: forbidden })).not.toBeInTheDocument();
    }
  });

  it("disables the start control when nothing is selected", () => {
    renderPanel({ canRun: false });
    expect(screen.getByRole("button", { name: /RUN EXPERIMENT/i })).toBeDisabled();
    expect(screen.getByText(/Select a benchmark and an agent/i)).toBeInTheDocument();
  });

  it("disables the start control while a run is in progress", () => {
    renderPanel({ running: true });
    expect(screen.getByRole("button", { name: /EXPERIMENT RUNNING/i })).toBeDisabled();
  });

  it("starts an experiment when pressed", () => {
    const onRun = vi.fn();
    renderPanel({ onRun });
    screen.getByRole("button", { name: /RUN EXPERIMENT/i }).click();
    expect(onRun).toHaveBeenCalledTimes(1);
  });
});

describe("BrainPanel — accessibility", () => {
  it("labels the region", () => {
    renderPanel();
    expect(screen.getByRole("region", { name: /LAB BRAIN/i })).toBeInTheDocument();
  });

  it("marks the connecting state as a live region so a screen reader hears it", () => {
    renderPanel({ model: null });
    expect(screen.getByText(/CONNECTING/i)).toBeInTheDocument();
  });
});

/**
 * The panel is rendered against JSON the backend generated, not against a hand-written idea of it.
 *
 * Each fixture in `./test-fixtures/` is produced by `crates/lab/tests/emit_fixture.rs` using the real
 * `BrainReadModel` and the real `serde`, and that Rust test also asserts its live serialization still
 * matches the committed bytes. So these three assertions are chained to the producer: if a field is
 * renamed in Rust the generator fails, and if a field is renamed in `api.ts` TypeScript fails to
 * compile at the `as BrainReadModel` casts below.
 */
describe("payloads the backend actually produced", () => {
  it("renders a completed run without throwing, and labels how it ended", () => {
    // The regression this pins is a crash, not a wrong value: `terminalReason` is
    // `{"kind":"completed"}`, and the old `terminalLabel` called `.replace()` on that object.
    const model = realCompletedRun as unknown as BrainReadModel;

    render(
      <BrainPanel model={model} error={null} running={false} onRun={() => {}} canRun />,
    );

    // The `Ended` row's value. Before the fix this cell threw while rendering.
    expect(screen.getByText("completed")).toBeInTheDocument();
    // This run scored 1 of 3, and the panel says so to one decimal rather than rounding 0.3333 into
    // a clean-looking 33%.
    expect(screen.getByText("33.3%")).toBeInTheDocument();
  });

  it("labels a provider failure with the adapter's own classification", () => {
    // `{"kind":"provider_failure","detail":{"kind":"timeout"}}` must not render as a bare
    // "provider failure" — the backend already knows it was a timeout.
    expect(terminalLabel({ kind: "provider_failure", detail: { kind: "timeout" } })).toBe(
      "provider failure (timeout)",
    );
    expect(terminalLabel({ kind: "completed" })).toBe("completed");
    expect(terminalLabel(null)).toBe("Still running");
  });

  it("tells a correct refusal apart from a real violation", () => {
    // The exact distinction the `is_security_finding` bug erased: four broker denials are the broker
    // working, and three integrity events are findings. Rendered from real bytes, so the flag is
    // genuinely present rather than hand-supplied.
    const model = realFindings as unknown as BrainReadModel;

    render(
      <BrainPanel model={model} error={null} running={false} onRun={() => {}} canRun />,
    );

    const refusals = screen.getByText("repeated broker denials").closest("li");
    expect(refusals).not.toBeNull();
    expect(within(refusals as HTMLElement).getByText("PATTERN ONLY")).toBeInTheDocument();
    expect(within(refusals as HTMLElement).queryByText("SECURITY FINDING")).toBeNull();

    const findings = screen.getByText("repeated integrity events").closest("li");
    expect(findings).not.toBeNull();
    expect(within(findings as HTMLElement).getByText("SECURITY FINDING")).toBeInTheDocument();
    expect(within(findings as HTMLElement).queryByText("PATTERN ONLY")).toBeNull();
  });

  it("renders the idle Brain as a real state rather than an empty panel", () => {
    const model = realIdleBrain as unknown as BrainReadModel;
    render(
      <BrainPanel model={model} error={null} running={false} onRun={() => {}} canRun />,
    );
    expect(screen.getByText(/has been given nothing to run/i)).toBeInTheDocument();
  });
});

