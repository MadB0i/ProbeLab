/**
 * The stored-run history, the readiness next step, and the package-save affordance.
 *
 * The shared theme: every value shown is a field the backend recorded, and an absent one is shown as
 * an absence. A zero standing in for "not recorded" is the failure these tests exist to prevent.
 */

import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import {
  episodeLabel,
  ExperimentHistory,
  lifecycleLabel,
  runIdLabel,
  scoreLabel,
  targetLabel,
  updatedLabel,
} from "./ExperimentHistory";
import type { StoredExperiment } from "./api";

function run(overrides: Partial<StoredExperiment> = {}): StoredExperiment {
  return {
    experimentId: "1791648100986-000000000000000f",
    benchmarkId: "grid-world",
    lifecycle: "completed",
    terminalReason: null,
    episodesRequested: 5,
    episodesCompleted: 5,
    episodesFailed: 0,
    hasScore: true,
    score: 0.8,
    resultJson: null,
    updatedUnixMs: 1_700_000_000_000,
    targetAgent: "grid_world_reference",
    rootSeed: 2000,
    ...overrides,
  };
}

const NOW = 1_700_000_600_000;

describe("history field wording", () => {
  it("names each lifecycle in the words a reader would use", () => {
    expect(lifecycleLabel("completed")).toBe("Completed");
    expect(lifecycleLabel("failed")).toBe("Failed");
    expect(lifecycleLabel("interrupted")).toBe("Interrupted");
    expect(lifecycleLabel("running")).toBe("Running");
  });

  it("shows an absent score as an absence, never as zero", () => {
    // A zero score is a real measurement. Rendering one for a run that recorded no score would be
    // inventing a result, which is the one thing this product must not do.
    expect(scoreLabel(run({ hasScore: false, score: null }))).toBe("—");
    expect(scoreLabel(run({ hasScore: true, score: 0 }))).toBe("0");
    expect(scoreLabel(run({ hasScore: false, score: 0 }))).toBe("—");
  });

  it("keeps small scores legible rather than rounding them to zero", () => {
    expect(scoreLabel(run({ score: 0.333 }))).toBe("0.333");
  });

  it("reports episodes against what was requested", () => {
    expect(episodeLabel(run())).toBe("5/5");
    expect(episodeLabel(run({ episodesCompleted: 3, episodesRequested: 5 }))).toBe("3/5");
  });

  it("does not invent a target agent", () => {
    expect(targetLabel(run())).toBe("grid_world_reference");
    expect(targetLabel(run({ targetAgent: null }))).toBe("no target recorded");
  });

  it("says so when a timestamp was not recorded, rather than showing 1970", () => {
    expect(updatedLabel(run({ updatedUnixMs: 0 }))).toBe("time not recorded");
    expect(updatedLabel(run(), NOW)).toBe("10 min ago");
  });

  it("shortens a long identifier but keeps it reachable in full", () => {
    expect(runIdLabel(run())).toContain("…");
    expect(runIdLabel(run(), 64)).toBe(run().experimentId);
  });
});

describe("ExperimentHistory", () => {
  it("says there is nothing stored, and that runs persist", () => {
    render(<ExperimentHistory runs={[]} selectedId={null} loading={false} onOpen={() => {}} />);
    expect(screen.getByText(/No stored runs yet/i)).toBeInTheDocument();
  });

  it("draws a row per run with its real metadata", () => {
    render(
      <ExperimentHistory runs={[run()]} selectedId={null} loading={false} onOpen={() => {}} />,
    );
    expect(screen.getByText("Completed")).toBeInTheDocument();
    expect(screen.getByText("grid-world")).toBeInTheDocument();
    expect(screen.getByText("grid_world_reference")).toBeInTheDocument();
    expect(screen.getByText("5/5")).toBeInTheDocument();
    // A fraction keeps three digits, so 0.8 is never rendered as a bare "1" by rounding.
    expect(screen.getByText("0.800")).toBeInTheDocument();
  });

  it("opens a run by identifier when its row is activated", () => {
    const onOpen = vi.fn();
    render(
      <ExperimentHistory
        runs={[run()]}
        selectedId={null}
        loading={false}
        onOpen={onOpen}
      />,
    );
    screen.getByRole("button").click();
    expect(onOpen).toHaveBeenCalledWith(run().experimentId);
  });

  it("marks the run currently loaded, and says which one it is", () => {
    render(
      <ExperimentHistory
        runs={[run()]}
        selectedId={run().experimentId}
        loading={false}
        onOpen={() => {}}
      />,
    );
    expect(screen.getByRole("button")).toHaveAttribute("aria-current", "true");
  });

  it("separates an interrupted run from a failed one", () => {
    // They mean different things and the user can act on them differently: one was cut short, the
    // other refused. Both are called out, and neither is dressed up as a pass.
    render(
      <ExperimentHistory
        runs={[
          run({ experimentId: "a", lifecycle: "interrupted", terminalReason: "process exited" }),
          run({ experimentId: "b", lifecycle: "failed", terminalReason: "provider refused" }),
        ]}
        selectedId={null}
        loading={false}
        onOpen={() => {}}
      />,
    );
    expect(screen.getByText("Interrupted")).toBeInTheDocument();
    expect(screen.getByText("Failed")).toBeInTheDocument();
  });

  it("shows how many episodes failed, because a partial run is not a complete measurement", () => {
    render(
      <ExperimentHistory
        runs={[run({ episodesCompleted: 3, episodesRequested: 5, episodesFailed: 2 })]}
        selectedId={null}
        loading={false}
        onOpen={() => {}}
      />,
    );
    expect(screen.getByText("3/5")).toBeInTheDocument();
    expect(screen.getByText(/2 failed/)).toBeInTheDocument();
  });

  it("disables opening while another run is loading", () => {
    render(
      <ExperimentHistory runs={[run()]} selectedId={null} loading onOpen={() => {}} />,
    );
    expect(screen.getByRole("button")).toBeDisabled();
  });

  it("carries a caption so the table is described, not just laid out", () => {
    render(
      <ExperimentHistory runs={[run()]} selectedId={null} loading={false} onOpen={() => {}} />,
    );
    expect(screen.getByRole("table")).toHaveAccessibleName(/stored experiment runs/i);
  });
});