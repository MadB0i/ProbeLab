/**
 * Tasks panel tests.
 */

import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { TasksPanel, formatPercent, provenanceLabel } from "./TasksPanel";
import type { TaskReport } from "./api";

describe("provenance helpers", () => {
  it("labels both provenances distinctly", () => {
    expect(provenanceLabel("observed_run")).toBe("OBSERVED RUN");
    expect(provenanceLabel("manual_submission")).toBe("MANUAL SUBMISSION");
  });

  it("never renders a missing score as zero", () => {
    expect(formatPercent(null)).toBe("no score");
    expect(formatPercent(0)).toBe("0.0%");
  });
});

describe("TasksPanel", () => {
  it("renders its heading before any data arrives", () => {
    render(<TasksPanel />);
    expect(screen.getByText("TASK BENCHMARKS")).toBeDefined();
  });
});

export function exampleReport(): TaskReport {
  return {
    benchmark: "coding_repair",
    definitionVersion: "1",
    definitionDigest: "sha256:abc",
    policy: "reference-optimal",
    provenance: "observed_run",
    successes: 70,
    trials: 70,
    successRate: 1,
    intervalLower: 0.94,
    intervalUpper: 1,
    meanPartialCredit: 1,
    seeds: [2000000],
    taskSuccess: [true],
    executionNotes: [],
  };
}
