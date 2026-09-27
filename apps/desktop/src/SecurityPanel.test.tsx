/**
 * Security panel tests.
 *
 * The recurring assertion is that a *caught* attempt reads as success, not as a breach. That is the
 * failure mode that would make this panel actively harmful: a lab that catches every probe would
 * render a rising alarm state, and a reader would learn to ignore the one number that matters.
 */

import { render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import {
  SecurityPanel,
  outcomeExplanation,
  outcomeLabel,
  outcomeTone,
} from "./SecurityPanel";
import type { SecurityLabReport, SecurityProbeListing, SecurityProbeReport } from "./api";

const PROBES: SecurityProbeListing[] = [
  {
    id: "security_probe_sandbox_violation",
    name: "Sandbox escape probe",
    description: "Requests an escape from the sandbox.",
  },
  {
    id: "security_probe_unauthorized_access",
    name: "Evaluator access probe",
    description: "Requests denied evaluator access.",
  },
];

function probe(overrides: Partial<SecurityProbeReport> = {}): SecurityProbeReport {
  return {
    probeId: "security_probe_sandbox_violation",
    probeName: "Sandbox escape probe",
    benchmarkId: "grid-world",
    seed: 11,
    outcome: "policy_denied",
    isSecurityFinding: false,
    integrityEvents: [["sandbox_violation", 9]],
    brokerRefusals: [],
    episodes: 2,
    timeline: [],
    failure: null,
    ...overrides,
  };
}

function report(overrides: Partial<SecurityLabReport> = {}): SecurityLabReport {
  return {
    outcomes: [
      probe(),
      probe({
        probeId: "security_probe_unauthorized_access",
        probeName: "Evaluator access probe",
        outcome: "broker_refused",
        integrityEvents: [],
        brokerRefusals: [["broker_unauthorized_read", 4]],
      }),
    ],
    summary: {
      integrityViolations: 9,
      brokerRefusals: 4,
      violationsByCategory: { sandbox_violation: 9 },
      refusalsByCategory: { broker_unauthorized_read: 4 },
      invalidActions: 0,
    },
    benchmarkId: "grid-world",
    ...overrides,
  };
}

function renderPanel(props: Partial<Parameters<typeof SecurityPanel>[0]> = {}) {
  return render(
    <SecurityPanel
      probes={PROBES}
      report={null}
      error={null}
      running={false}
      benchmarkId="grid-world"
      onRun={() => {}}
      onBenchmarkChange={() => {}}
      canRun
      {...props}
    />,
  );
}

describe("outcome vocabulary", () => {
  it("names each state distinctly", () => {
    expect(outcomeLabel("broker_refused")).toBe("BROKER REFUSED");
    expect(outcomeLabel("policy_denied")).toBe("POLICY DENIED");
    expect(outcomeLabel("refused_and_denied")).toBe("REFUSED AND DENIED");
    expect(outcomeLabel("not_attempted")).toBe("NOT ATTEMPTED");
    expect(outcomeLabel("failed")).toBe("FAILED");
  });

  it("styles a held boundary calmly and an untested probe as untested", () => {
    // The tone is the first thing a reader sees, so "held" must not look like "bad".
    expect(outcomeTone("broker_refused")).toBe("tone-held");
    expect(outcomeTone("policy_denied")).toBe("tone-held");
    expect(outcomeTone("refused_and_denied")).toBe("tone-held");
    // And an untested probe must not be styled as a pass.
    expect(outcomeTone("not_attempted")).toBe("tone-untested");
    expect(outcomeTone("not_attempted")).not.toBe(outcomeTone("policy_denied"));
  });

  it("explains that a denied attempt is the boundary holding", () => {
    expect(outcomeExplanation("policy_denied")).toMatch(/boundary held/i);
    expect(outcomeExplanation("broker_refused")).toMatch(/enforcement working/i);
  });

  it("says plainly that an untested probe is not a pass", () => {
    expect(outcomeExplanation("not_attempted")).toMatch(/not a pass/i);
  });
});

describe("SecurityPanel — empty state", () => {
  it("shows an intentional no-sweep state", () => {
    renderPanel();
    // Present in both the status badge and the empty-state heading, by design.
    expect(screen.getAllByText("NO SWEEP RUN").length).toBeGreaterThan(0);
    expect(screen.getByText(/2 probes are registered and ready/i)).toBeInTheDocument();
  });

  it("says so when no probes are registered", () => {
    renderPanel({ probes: [] });
    expect(screen.getByText(/no security probes are registered/i)).toBeInTheDocument();
  });

  it("states the probe/boundary distinction before any data is shown", () => {
    // This sentence is the panel's reason for existing; it is present even when idle.
    renderPanel();
    expect(screen.getByText(/test of the boundary/i)).toBeInTheDocument();
  });
});

describe("SecurityPanel — results", () => {
  it("shows one row per probe with the layer that acted", () => {
    renderPanel({ report: report() });
    const rows = document.querySelectorAll(".probe-row");
    expect(rows).toHaveLength(2);
    expect(screen.getByText("Sandbox escape probe")).toBeInTheDocument();
    expect(screen.getByText("Evaluator access probe")).toBeInTheDocument();
    expect(screen.getByText("POLICY DENIED")).toBeInTheDocument();
    expect(screen.getByText("BROKER REFUSED")).toBeInTheDocument();
  });

  it("reports boundaries held and never as a breach", () => {
    renderPanel({ report: report() });
    expect(screen.getByText("Boundaries held")).toBeInTheDocument();
    expect(screen.getByText("Findings about the lab")).toBeInTheDocument();
    // The findings figure is zero by construction, and the panel says zero is expected.
    const findingsCard = screen.getByText("Findings about the lab").closest("div")!;
    expect(findingsCard.querySelector(".security-summary-value")?.textContent).toBe("0");
    expect(findingsCard.textContent).toMatch(/expected result/i);
  });

  it("never presents a caught probe as a vulnerability", () => {
    const { container } = renderPanel({ report: report() });
    const text = container.textContent ?? "";
    for (const forbidden of [/attacks blocked/i, /breach(es)?\b/i, /vulnerabilit/i, /severity/i]) {
      expect(text).not.toMatch(forbidden);
    }
  });

  it("keeps the two totals in separate blocks and says why", () => {
    renderPanel({ report: report() });
    // The label also appears per-probe, so the totals section is scoped by its heading first.
    const totals = screen.getByText("Totals by layer").closest("section")!;
    const violations = within(totals).getByText("Policy-gate records").closest("div")!;
    const refusals = within(totals).getByText("Broker refusals").closest("div")!;
    expect(violations.querySelector(".security-count")?.textContent).toBe("9");
    expect(refusals.querySelector(".security-count")?.textContent).toBe("4");
    // Neither block shows the other's headline.
    expect(violations.textContent).not.toContain("4");
    expect(refusals.textContent).not.toContain("9");
    expect(screen.getByText(/deliberately not combined/i)).toBeInTheDocument();
  });

  it("shows the target, seed, and episodes for each probe", () => {
    renderPanel({ report: report() });
    const first = document.querySelector(".probe-row")!;
    expect(first.textContent).toContain("grid-world");
    expect(first.textContent).toContain("11");
    expect(first.textContent).toContain("Held");
  });

  it("states the absence of a record rather than omitting the section", () => {
    renderPanel({
      report: report({ outcomes: [probe({ integrityEvents: [], brokerRefusals: [] })] }),
    });
    expect(screen.getByText("Nothing recorded.")).toBeInTheDocument();
    expect(screen.getByText("No capability request reached the broker.")).toBeInTheDocument();
  });

  it("shows an untested probe as untested, not as a held boundary", () => {
    renderPanel({
      report: report({
        outcomes: [
          probe({ outcome: "not_attempted", integrityEvents: [], brokerRefusals: [] }),
        ],
        summary: {
          integrityViolations: 0,
          brokerRefusals: 0,
          violationsByCategory: {},
          refusalsByCategory: {},
          invalidActions: 0,
        },
      }),
    });
    expect(screen.getByText("NOT ATTEMPTED")).toBeInTheDocument();
    const heldCard = screen.getByText("Boundaries held").closest("div")!;
    expect(heldCard.querySelector(".security-summary-value")?.textContent).toBe("0");
    expect(
      screen.getByText("Never reached an enforcement layer").closest("div")!
        .querySelector(".security-summary-value")?.textContent,
    ).toBe("1");
  });

  it("surfaces a probe failure rather than hiding it", () => {
    renderPanel({
      report: report({
        outcomes: [probe({ outcome: "failed", failure: "engine refused the run" })],
      }),
    });
    expect(screen.getByText("engine refused the run")).toBeInTheDocument();
  });
});

describe("SecurityPanel — errors", () => {
  it("shows a backend failure and reports no results", () => {
    renderPanel({ report: null, error: "The sweep could not run: unknown benchmark" });
    expect(screen.getByRole("alert")).toHaveTextContent("unknown benchmark");
    expect(screen.queryByText(/Boundaries held/)).not.toBeInTheDocument();
  });
});

describe("SecurityPanel — controls", () => {
  it("runs a sweep when pressed", () => {
    const onRun = vi.fn();
    renderPanel({ onRun });
    screen.getByRole("button", { name: /RUN SECURITY SWEEP/i }).click();
    expect(onRun).toHaveBeenCalledTimes(1);
  });

  it("disables the control while sweeping and when no probes exist", () => {
    const running = renderPanel({ running: true });
    expect(screen.getByRole("button", { name: /SWEEPING/i })).toBeDisabled();
    running.unmount();
    renderPanel({ canRun: false });
    expect(screen.getByRole("button", { name: /RUN SECURITY SWEEP/i })).toBeDisabled();
  });

  it("offers only benchmarks the backend can sweep against", () => {
    renderPanel();
    const select = screen.getByLabelText("TARGET BENCHMARK") as HTMLSelectElement;
    expect(Array.from(select.options).map((o) => o.value)).toEqual([
      "grid-world",
      "sandbox-process",
    ]);
  });
});

describe("SecurityPanel — no fabricated data", () => {
  it("shows no risk score, grade, or percentage", () => {
    const { container } = renderPanel({ report: report() });
    // The authority note is excluded, because it *disavows* a score and therefore contains the word.
    // Scanning it would make this test pass or fail on the disclaimer rather than on the data.
    const note = container.querySelector(".security-authority");
    const text = (container.textContent ?? "")
      .toLowerCase()
      .replace(note?.textContent?.toLowerCase() ?? "", "");
    for (const forbidden of ["score", "grade", "risk level", "%"]) {
      expect(text).not.toContain(forbidden);
    }
    // And nothing is presented as a risk figure: the only large numbers are the per-layer totals,
    // which are labelled as records and refusals respectively.
    expect(container.querySelectorAll(".security-summary-value")).toHaveLength(3);
  });

  it("shows no invented history before a sweep", () => {
    const { container } = renderPanel();
    expect(container.querySelectorAll(".probe-row")).toHaveLength(0);
  });
});
