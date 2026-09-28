/**
 * Benchmark catalog tests.
 *
 * The catalog is the only way to choose which benchmark a run uses, and it rendered no button at all
 * until `onSelect` was passed -- which `App.tsx` never did. So `sandbox-process`, the only benchmark
 * that exercises a boundary, was unreachable from the UI while the README listed it as implemented.
 * These tests cover the button appearing, the selection being stated rather than implied, and the
 * honesty metadata each row must carry.
 */
import { render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { BenchmarkCatalog } from "./BenchmarkCatalog";
import type { BenchmarkDetail } from "./api";

const GRID: BenchmarkDetail = {
  id: "grid-world",
  name: "Grid World",
  description: "A deterministic navigation task.",
  category: "planning",
  capabilityDimension: "navigation",
  capabilityExecution: "executed",
  scoring: "fraction of episodes that reached the goal",
  referenceAgent: "optimal",
  replaySupport: "deterministic",
  platformRequirements: "any",
  limits: { max_turns: "30" },
  version: "2",
};

const SANDBOX: BenchmarkDetail = {
  id: "sandbox-process",
  name: "Sandbox Process",
  description: "A child process confined by the containment layer.",
  category: "containment",
  capabilityDimension: "containment",
  // The distinction the panel exists to keep: the benchmark *authorises* a capability it does not
  // execute. A row that read as though the sandbox were exercised would overstate the lab's own
  // containment.
  capabilityExecution: "authorized_deferred",
  scoring: "boundary outcomes, not a task score",
  referenceAgent: null,
  replaySupport: "projected_only",
  platformRequirements: "windows",
  limits: { max_turns: "10" },
  version: "1",
};

const CATALOG = [GRID, SANDBOX];

describe("the select control", () => {
  it("renders a button for every benchmark when a handler is supplied", () => {
    // This is the regression: the button is behind `onSelect ? ... : null`, and the app passed
    // nothing, so the control did not exist.
    render(<BenchmarkCatalog benchmarks={CATALOG} onSelect={() => {}} />);
    expect(screen.getByRole("button", { name: /select grid world/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /select sandbox process/i })).toBeInTheDocument();
  });

  it("still renders the rows when no handler is supplied", () => {
    // The catalog is also a read-only reference view, so a missing handler must degrade to a list
    // rather than an empty panel.
    render(<BenchmarkCatalog benchmarks={CATALOG} />);
    expect(screen.queryByRole("button")).toBeNull();
    expect(screen.getByText("Grid World")).toBeInTheDocument();
    expect(screen.getByText("Sandbox Process")).toBeInTheDocument();
  });

  it("hands the whole benchmark to the handler, not just its id", () => {
    // The caller needs `max_turns` and `version` to render the run it is about to start, so an id-only
    // callback would force a second lookup against a list it does not have.
    const onSelect = vi.fn();
    render(<BenchmarkCatalog benchmarks={CATALOG} onSelect={onSelect} />);
    screen.getByRole("button", { name: /select sandbox process/i }).click();
    expect(onSelect).toHaveBeenCalledWith(SANDBOX);
  });
});

describe("stating which benchmark is selected", () => {
  it("marks the current one with aria-current and says so in the label", () => {
    // A colour change alone is not a selection: the choice has to be in the text and in the
    // accessibility tree, because "which of these two will run" is a fact, not a decoration.
    render(
      <BenchmarkCatalog benchmarks={CATALOG} onSelect={() => {}} selectedId="sandbox-process" />,
    );
    const current = screen.getByRole("button", { name: /currently selected/i });
    expect(current).toHaveAttribute("aria-current", "true");
    expect(current).toHaveTextContent("Selected");
    expect(
      screen.getByRole("button", { name: /select grid world/i }),
    ).not.toHaveAttribute("aria-current");
  });

  it("marks nothing when nothing has been selected", () => {
    render(<BenchmarkCatalog benchmarks={CATALOG} onSelect={() => {}} selectedId={null} />);
    expect(screen.queryAllByRole("button", { name: /currently selected/i })).toHaveLength(0);
  });

  it("marks the whole row as well as the button", () => {
    render(
      <BenchmarkCatalog benchmarks={CATALOG} onSelect={() => {}} selectedId="grid-world" />,
    );
    const items = document.querySelectorAll(".benchmark-item");
    expect(items[0].className).toContain("benchmark-item-active");
    expect(items[1].className).not.toContain("benchmark-item-active");
  });
});

describe("honesty metadata on each row", () => {
  it("renders an authorized-but-unexecuted capability as deferred", () => {
    // `sandbox-process` authorises a capability the lab does not execute. A row that read as though
    // the sandbox were being exercised would overstate the containment the product can demonstrate.
    render(<BenchmarkCatalog benchmarks={CATALOG} onSelect={() => {}} />);
    const row = screen.getByText("Sandbox Process").closest("li");
    expect(row).not.toBeNull();
    expect(within(row as HTMLElement).getByText(/deferred|authorized/i)).toBeInTheDocument();
  });

  it("shows an absent reference agent as absent rather than naming one", () => {
    // `referenceAgent: null` means there is no ceiling to compare against. Inventing a placeholder
    // would imply a baseline exists.
    render(<BenchmarkCatalog benchmarks={CATALOG} onSelect={() => {}} />);
    const sandbox = screen.getByText("Sandbox Process").closest("li");
    const grid = screen.getByText("Grid World").closest("li");
    expect(within(sandbox as HTMLElement).queryByText(/optimal/i)).toBeNull();
    expect(within(grid as HTMLElement).getByText(/optimal/i)).toBeInTheDocument();
  });

  it("distinguishes a deterministic replay from a projection", () => {
    // "Re-running reproduces the digest" and "no re-execution is claimed" are different guarantees,
    // and collapsing them would let someone trust a projection as though it were a re-run.
    render(<BenchmarkCatalog benchmarks={CATALOG} onSelect={() => {}} />);
    const grid = screen.getByText("Grid World").closest("li");
    const sandbox = screen.getByText("Sandbox Process").closest("li");
    expect(within(grid as HTMLElement).getByText(/reproduces the digest/i)).toBeInTheDocument();
    expect(
      within(sandbox as HTMLElement).getByText(/no re-execution is claimed/i),
    ).toBeInTheDocument();
  });
});

describe("an empty catalog", () => {
  it("says so, and does not render a picker", () => {
    render(<BenchmarkCatalog benchmarks={[]} onSelect={() => {}} />);
    expect(screen.getByText(/no benchmarks are registered/i)).toBeInTheDocument();
    expect(screen.queryByRole("button")).toBeNull();
  });
});
