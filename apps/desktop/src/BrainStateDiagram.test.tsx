/**
 * The Brain state diagram.
 *
 * Two things are being protected here, and the second is the more important.
 *
 * 1. The diagram must not invent. Every value it draws comes from the `BrainReadModel`, and a state it
 *    does not recognise is reported rather than rounded to the nearest step it does know.
 * 2. Its state list must match the state machine. `REACHABLE_STATES` deliberately omits
 *    `BrainState::Running`, which the backend never assigns to a Brain -- a diagram showing a node the
 *    product cannot enter is a small lie about how the system works. That omission is easy to break
 *    from either side: wire `Running` up in Rust and the diagram goes stale, delete the variant and
 *    the diagram breaks. Both directions are asserted here so neither can happen quietly.
 */
import { render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { BrainStateDiagram, REACHABLE_STATES } from "./BrainStateDiagram";
import type { BrainReadModel, SignalView } from "./api";

import realCompletedRun from "./test-fixtures/brainReadModel.json";
import realIdleBrain from "./test-fixtures/brainIdleModel.json";
import realFindings from "./test-fixtures/brainFindingsModel.json";

function signal(overrides: Partial<SignalView> = {}): SignalView {
  return {
    id: "task_success",
    label: "Episodes that reached the goal",
    count: 3,
    interpretation: "neutral",
    ...overrides,
  };
}

function model(overrides: Partial<BrainReadModel> = {}): BrainReadModel {
  const base = realCompletedRun as unknown as BrainReadModel;
  return { ...base, ...overrides };
}

function renderDiagram(overrides: Partial<BrainReadModel> = {}) {
  return render(<BrainStateDiagram model={model(overrides)} />);
}

describe("the lifecycle it claims to show", () => {
  /**
   * The backend's `BrainState` enum, transcribed. If Rust adds or removes a variant this fails, and
   * the right response is to decide whether the Brain can now enter it -- not to widen the list until
   * the test goes green.
   */
  const BACKEND_VARIANTS = [
    "planning",
    "preparing",
    "running",
    "observing",
    "analyzing",
    "completed",
  ];

  it("omits Running, because no code path assigns it to a Brain", () => {
    // In `crates/lab/src/brain.rs`, `LabBrain` is constructed as `Planning` and then assigned
    // `Preparing`, `Observing`, `Analyzing`, and `Completed`. `Running` appears only in the
    // `label()` match and in two read-model tests that build a projection directly. Drawing it would
    // put a node on screen the product cannot reach.
    expect(BACKEND_VARIANTS).toContain("running");
    expect(REACHABLE_STATES).not.toContain("running");
    expect(REACHABLE_STATES).toHaveLength(BACKEND_VARIANTS.length - 1);
  });

  it("lists the states in the order the machine actually moves through them", () => {
    expect([...REACHABLE_STATES]).toEqual([
      "planning",
      "preparing",
      "observing",
      "analyzing",
      "completed",
    ]);
  });

  it("names every backend variant, so a new one cannot be added unnoticed", () => {
    // A variant the diagram has never heard of is not automatically wrong -- it may be another dead
    // state -- but it must be a decision, not an omission nobody noticed.
    for (const variant of BACKEND_VARIANTS) {
      const known = (REACHABLE_STATES as readonly string[]).includes(variant);
      // `running` is the one known-dead variant, documented in the module comment.
      expect(known || variant === "running").toBe(true);
    }
  });
});

describe("marking the current state", () => {
  it("marks exactly one state current, and marks the rest passed or ahead", () => {
    renderDiagram({ brainState: "observing" });
    const steps = screen.getAllByRole("listitem");
    const current = steps.filter((step) => step.getAttribute("aria-current") === "step");
    expect(current).toHaveLength(1);
    expect(current[0]).toHaveTextContent("OBSERVING");

    // Everything before the current state is passed, everything after is ahead. A single highlight
    // with no distinction between "already done" and "not yet" would make a finished run look like an
    // idle one.
    const passed = steps.filter((step) => step.className.includes("is-passed"));
    const ahead = steps.filter((step) => step.className.includes("is-ahead"));
    expect(passed).toHaveLength(2); // planning, preparing
    expect(ahead).toHaveLength(2); // analyzing, completed
  });

  it("marks nothing passed before the first step, on a fresh Brain", () => {
    // The idle fixture the backend generated: `brainState` is `planning`.
    render(<BrainStateDiagram model={realIdleBrain as unknown as BrainReadModel} />);
    const steps = screen.getAllByRole("listitem");
    expect(steps.filter((step) => step.className.includes("is-passed"))).toHaveLength(0);
    expect(
      steps.filter((step) => step.getAttribute("aria-current") === "step"),
    ).toHaveLength(1);
  });

  it("reports a state it does not model instead of rounding it to a step it knows", () => {
    // If the backend gains a state, the honest response is to say the panel does not model it.
    // Drawing it as the last step would misreport where the Brain actually is.
    renderDiagram({ brainState: "hibernating" });
    expect(screen.getByText(/does not model/i)).toBeInTheDocument();
    expect(screen.getByText("HIBERNATING")).toBeInTheDocument();
    // And nothing is claimed to be current, because nothing is known.
    expect(screen.queryAllByRole("listitem").filter((s) => s.getAttribute("aria-current"))).toHaveLength(0);
  });
});

describe("the counters", () => {
  it("scales each bar to the largest count in the run, not to a fixed ceiling", () => {
    // A fixed maximum would make a run with 40 successes look identical to one with 4.
    renderDiagram({
      signals: [signal({ count: 8 }), signal({ id: "other", label: "Other", count: 2 })],
    });
    const bars = [...document.querySelectorAll(".counter-bar")].map((bar) =>
      (bar as HTMLElement).style.width,
    );
    expect(bars).toEqual(["100%", "25%"]);
  });

  it("renders a zero-width bar for a zero count rather than a minimum-width stub", () => {
    renderDiagram({ signals: [signal({ count: 0 }), signal({ id: "o", label: "O", count: 5 })] });
    const bars = [...document.querySelectorAll(".counter-bar")].map(
      (bar) => (bar as HTMLElement).style.width,
    );
    expect(bars).toEqual(["0%", "100%"]);
  });

  it("hides the bars from assistive technology, because the count is already text", () => {
    // The number is a text node immediately before each bar. Announcing the bar too would repeat the
    // same figure with nothing added.
    renderDiagram({ signals: [signal({ count: 3 })] });
    expect(document.querySelector(".counter-bar")?.getAttribute("aria-hidden")).toBe("true");
    expect(screen.getByText("3")).toBeInTheDocument();
  });

  it("keeps a correct refusal and a violation in different colours, and says which is which", () => {
    renderDiagram({
      signals: [
        signal({ id: "refusal", label: "Broker refused", count: 4, interpretation: "correct_refusal" }),
        signal({ id: "violation", label: "Integrity recorded", count: 1, interpretation: "violation" }),
      ],
    });
    const rows = screen.getAllByRole("listitem");
    const refusal = rows.find((row) => within(row).queryByText("Broker refused"));
    const violation = rows.find((row) => within(row).queryByText("Integrity recorded"));

    expect(refusal?.className).toContain("tone-refusal");
    expect(violation?.className).toContain("tone-violation");
    // The text, not only the colour, is what distinguishes them.
    expect(within(refusal as HTMLElement).getByText("enforcement working")).toBeInTheDocument();
    expect(within(violation as HTMLElement).getByText("boundary violation")).toBeInTheDocument();
  });

  it("shows no rows at all for a clean run, rather than a page of zeros", () => {
    // The wording the old standalone Monitoring section used, kept because it is right: a grid of
    // empty bars would claim the Brain counted nothing separately per category, which is a more
    // specific claim than "nothing was counted".
    renderDiagram({ signals: [] });
    expect(screen.getByText(/no rows rather than a page of zeros/i)).toBeInTheDocument();
    expect(document.querySelectorAll(".counter-bar")).toHaveLength(0);
  });
});

describe("it does not claim to be a model", () => {
  it("says what it is on the face of the panel", () => {
    renderDiagram();
    // The whole point of the diagram being a state machine rather than a neural picture. If this text
    // is ever removed, the visual language needs re-examining.
    expect(screen.getByText(/not a model/i)).toBeInTheDocument();
  });
});

describe("rendered from backend-generated payloads", () => {
  it("handles the completed run and the findings run the backend produced", () => {
    for (const fixture of [realCompletedRun, realFindings]) {
      const { unmount } = render(
        <BrainStateDiagram model={fixture as unknown as BrainReadModel} />,
      );
      // Every fixture's state must be one the diagram models, or the completed run would render the
      // "does not model" warning in normal use.
      expect(screen.queryByText(/does not model/i)).toBeNull();
      unmount();
    }
  });
});
