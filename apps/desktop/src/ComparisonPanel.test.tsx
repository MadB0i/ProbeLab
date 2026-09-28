/**
 * Comparison panel tests.
 *
 * Weighted towards what the panel refuses to say. A comparison table that renders a wrong number is a
 * bug someone reports; a table that renders a plausible number for runs that were never comparable is
 * a bug nobody reports until they act on it.
 */

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ComparisonPanel } from "./ComparisonPanel";
import * as api from "./api";
import type { ComparisonResponse, StoredExperiment } from "./api";

const stored = (overrides: Partial<StoredExperiment> = {}): StoredExperiment => ({
  experimentId: "exp-a",
  benchmarkId: "grid-world",
  lifecycle: "completed",
  terminalReason: "completed",
  episodesRequested: 5,
  episodesCompleted: 5,
  episodesFailed: 0,
  hasScore: true,
  score: 0.8,
  resultJson: null,
  updatedUnixMs: 1_700_000_000_000,
  targetAgent: "optimal",
  rootSeed: 4242,
  ...overrides,
});

const run = (id: string, overrides: Partial<Parameters<typeof comparableRun>[0]> = {}) =>
  comparableRun({ experimentId: id, ...overrides });

function comparableRun(input: {
  experimentId: string;
  successFraction?: number | null;
  episodesCompleted?: number;
  targetAgent?: string;
  benchmarkId?: string;
  rootSeed?: number;
}) {
  const episodes = input.episodesCompleted ?? 5;
  return {
    experimentId: input.experimentId,
    benchmarkId: input.benchmarkId ?? "grid-world",
    targetAgent: input.targetAgent ?? "optimal",
    adapter: "deterministic-mock-v1",
    strategy: "optimal",
    // Identity, not decoration. The backend refuses a comparison when two runs recorded different
    // providers or models, so these are the fields that decide whether a ranking may exist at all --
    // and they are the reason this fixture is checked in rather than assumed.
    provider: "deterministic-mock",
    model: "native-mock-optimal",
    benchmarkDefinitionDigest: "sha256:test",
    rootSeed: input.rootSeed ?? 4242,
    episodesCompleted: episodes,
    episodesFailed: 0,
    successFraction: input.successFraction === undefined ? 0.8 : input.successFraction,
    successInterval:
      input.successFraction === null
        ? null
        : { point: 0.8, lower: 0.42, upper: 0.96, confidence: 0.95 },
    integrityEvents: 0,
    brokerRefusals: 0,
    isSecurityFinding: false,
    meanReward: 8.4,
    meanTurns: 5.2,
    meanInvalidActions: 0.1,
  };
}

const comparable: ComparisonResponse = {
  kind: "comparable",
  detail: {
    runs: [run("exp-a"), run("exp-b", { successFraction: 0.4 })],
    benchmarkId: "grid-world",
    targetAgent: "optimal",
    adapter: "deterministic-mock-v1",
    strategy: "optimal",
    rootSeed: 4242,
    bySuccess: [
      {
        experimentId: "exp-a",
        rank: 1,
        value: 0.8,
        interval: { point: 0.8, lower: 0.42, upper: 0.96, confidence: 0.95 },
        unrankedReason: null,
      },
      {
        experimentId: "exp-b",
        rank: 2,
        value: 0.4,
        interval: { point: 0.4, lower: 0.12, upper: 0.76, confidence: 0.95 },
        unrankedReason: null,
      },
    ],
    byReward: [],
    byTurns: [],
    byInvalidActions: [],
    integrityEventTotal: 0,
    brokerRefusalTotal: 0,
    anySecurityFinding: false,
    variesAcross: [],
  },
};

const refused: ComparisonResponse = {
  kind: "not_comparable",
  detail: {
    experimentIds: ["exp-a", "exp-b"],
    dimensions: [{ dimension: "benchmark", values: ["grid-world", "sandbox-process"] }],
  },
};

let listResult: StoredExperiment[] = [];
let compareResult: ComparisonResponse = comparable;

beforeEach(() => {
  listResult = [stored(), stored({ experimentId: "exp-b" })];
  compareResult = comparable;
  vi.spyOn(api, "listComparableExperiments").mockImplementation(async () => listResult);
  vi.spyOn(api, "compareExperiments").mockImplementation(async () => compareResult);
});

afterEach(() => {
  vi.restoreAllMocks();
});

const selectBoth = async () => {
  render(<ComparisonPanel />);
  const boxes = await screen.findAllByRole("checkbox");
  fireEvent.click(boxes[0]);
  fireEvent.click(boxes[1]);
  fireEvent.click(screen.getByRole("button", { name: /compare selected/i }));
  await waitFor(() => expect(api.compareExperiments).toHaveBeenCalled());
};

describe("loading stored experiments", () => {
  it("lists what is stored", async () => {
    render(<ComparisonPanel />);
    expect(await screen.findByText("2 STORED")).toBeInTheDocument();
    expect(await screen.findByText("exp-a")).toBeInTheDocument();
  });

  it("says so when nothing is stored rather than showing an empty table", async () => {
    listResult = [];
    render(<ComparisonPanel />);
    expect(await screen.findByText(/no experiments are stored yet/i)).toBeInTheDocument();
  });

  /**
   * A read failure must be visible. Silently showing an empty list would look exactly like "you have
   * not run anything yet", which sends someone to run experiments that already exist.
   */
  it("reports a read failure instead of an empty list", async () => {
    vi.spyOn(api, "listComparableExperiments").mockRejectedValue(new Error("database is locked"));
    render(<ComparisonPanel />);
    expect(await screen.findByRole("alert")).toHaveTextContent(/database is locked/);
  });
});

describe("selection", () => {
  it("needs two runs before the comparison can be made", async () => {
    render(<ComparisonPanel />);
    const boxes = await screen.findAllByRole("checkbox");
    expect(screen.getByRole("button", { name: /compare selected/i })).toBeDisabled();
    fireEvent.click(boxes[0]);
    expect(screen.getByRole("button", { name: /compare selected/i })).toBeDisabled();
    expect(screen.getByText(/select at least two runs/i)).toBeInTheDocument();
    fireEvent.click(boxes[1]);
    expect(screen.getByRole("button", { name: /compare selected/i })).toBeEnabled();
  });

  it("deselects on a second click", async () => {
    render(<ComparisonPanel />);
    const boxes = await screen.findAllByRole("checkbox");
    fireEvent.click(boxes[0]);
    expect(boxes[0]).toBeChecked();
    fireEvent.click(boxes[0]);
    expect(boxes[0]).not.toBeChecked();
  });

  /**
   * A run with no recorded plan identity cannot be held constant, so offering it would produce a
   * comparison about something unknown. The control is disabled with a stated reason, rather than the
   * run being quietly left out of the list.
   */
  it("will not offer a run whose plan identity was never stored", async () => {
    listResult = [stored(), stored({ experimentId: "exp-old", targetAgent: null, rootSeed: null })];
    render(<ComparisonPanel />);
    const boxes = await screen.findAllByRole("checkbox");
    expect(boxes[1]).toBeDisabled();
    expect(screen.getByText(/predates stored plan identity/i)).toBeInTheDocument();
  });
});

describe("the refusal case", () => {
  it("names the dimension that differs and the values on each side", async () => {
    compareResult = refused;
    await selectBoth();
    const panel = await screen.findByText(/did not measure the same thing/i);
    const refusal = panel.closest(".compare-refusal") as HTMLElement;
    expect(within(refusal).getByText("benchmark")).toBeInTheDocument();
    expect(within(refusal).getByText("grid-world")).toBeInTheDocument();
    expect(within(refusal).getByText("sandbox-process")).toBeInTheDocument();
  });

  /**
   * A refusal is an answer, not a failure. It must not be rendered as an error, because a user who
   * sees a red banner would go looking for a bug rather than reading the explanation.
   */
  it("is not rendered as an error", async () => {
    compareResult = refused;
    await selectBoth();
    await screen.findByText(/did not measure the same thing/i);
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("shows no rankings at all when the runs are not comparable", async () => {
    compareResult = refused;
    await selectBoth();
    await screen.findByText(/did not measure the same thing/i);
    expect(screen.queryByText(/by success fraction/i)).toBeNull();
  });
});

describe("the comparable case", () => {
  it("renders the rankings the backend produced", async () => {
    await selectBoth();
    const block = (await screen.findByText("By success fraction")).closest(
      ".compare-block",
    ) as HTMLElement;
    const rows = within(block).getAllByRole("row");
    expect(within(rows[1]).getByText("#1")).toBeInTheDocument();
    expect(within(rows[1]).getByText("80.0%")).toBeInTheDocument();
    expect(within(rows[2]).getByText("#2")).toBeInTheDocument();
    expect(within(rows[2]).getByText("40.0%")).toBeInTheDocument();
  });

  it("states what was held constant", async () => {
    await selectBoth();
    const facts = (await screen.findByText("BENCHMARK")).closest(".compare-facts") as HTMLElement;
    expect(within(facts).getByText("4242")).toBeInTheDocument();
  });

  it("shows a Wilson interval beside the score", async () => {
    await selectBoth();
    const block = (await screen.findByText("By success fraction")).closest(
      ".compare-block",
    ) as HTMLElement;
    expect(within(block).getAllByTitle("Wilson interval").length).toBe(2);
  });

  /**
   * The confound banner is the difference between an honest comparison and a misleading one. It has
   * to be visible without expanding anything, and it has to name what was not held constant.
   */
  it("declares a confound above the numbers when the runs differ in more than episode count", async () => {
    compareResult = {
      kind: "comparable",
      detail: {
        ...comparable.detail,
        variesAcross: [{ dimension: "target_agent", values: ["optimal", "random"] }],
      },
    } as ComparisonResponse;
    await selectBoth();

    const banner = (await screen.findByText(/not held constant/i)).closest(
      ".compare-confound",
    ) as HTMLElement;
    expect(banner).toHaveTextContent("target_agent");
    expect(banner).toHaveTextContent("optimal vs random");
    expect(banner).toHaveTextContent("differ in more than episode count");
    // Above the table, not below it: a reader meets the qualification before the numbers.
    const result = screen.getByText("By success fraction").closest(".compare-result") as HTMLElement;
    const children = Array.from(result.children).map((child) => child.className);
    expect(children.indexOf("compare-confound")).toBeLessThan(
      children.findIndex((className) => className === "compare-block"),
    );
  });

  it("shows no confound banner when nothing was left unheld", async () => {
    await selectBoth();
    await screen.findByText("By success fraction");
    expect(screen.queryByText(/not held constant/i)).toBeNull();
  });

  it("reports a security finding when the set recorded one", async () => {
    compareResult = {
      kind: "comparable",
      detail: { ...comparable.detail, anySecurityFinding: true, integrityEventTotal: 3 },
    } as ComparisonResponse;
    await selectBoth();
    expect(await screen.findByText("RECORDED")).toBeInTheDocument();
  });

  it("shows the per-run measurements on request", async () => {
    await selectBoth();
    await screen.findByText("By success fraction");
    const details = screen.getByText(/per-run measurements/i).closest("details") as HTMLDetailsElement;
    expect(details.open).toBe(false);
    fireEvent.click(screen.getByText(/per-run measurements/i));
    expect(details.open).toBe(true);
  });
});

/**
 * The single most important rendering rule: a run with no measurement must never read as 0%.
 */
describe("an absent measurement is never shown as zero", () => {
  it("renders a missing score as no measurement with its reason", async () => {
    compareResult = {
      kind: "comparable",
      detail: {
        ...comparable.detail,
        runs: [run("exp-a"), run("exp-b", { successFraction: null, episodesCompleted: 0 })],
        bySuccess: [
          {
            experimentId: "exp-a",
            rank: 1,
            value: 0.8,
            interval: { point: 0.8, lower: 0.42, upper: 0.96, confidence: 0.95 },
            unrankedReason: null,
          },
          {
            experimentId: "exp-b",
            rank: null,
            value: null,
            interval: null,
            unrankedReason:
              "exp-b completed no episode, so it has no success fraction to rank",
          },
        ],
      },
    } as ComparisonResponse;
    await selectBoth();

    const block = (await screen.findByText("By success fraction")).closest(
      ".compare-block",
    ) as HTMLElement;
    const rows = within(block).getAllByRole("row");
    const row = rows.find((candidate) => within(candidate).queryByText("exp-b")) as HTMLElement;
    expect(within(row).getByText("no measurement")).toBeInTheDocument();
    expect(within(row).getByTitle(/completed no episode/i)).toBeInTheDocument();
    // Both the rank and the interval are absent for this run. The assertion is that neither cell
    // holds a number, not that there is one dash.
    const cells = within(row).getAllByRole("cell");
    expect(cells[0]).toHaveTextContent("—");
    expect(cells[2]).toHaveTextContent("no measurement");
    expect(cells[3]).toHaveTextContent("—");
    // And nowhere in this ranking does a zero appear as if it were a score.
    expect(within(block).queryByText("0.0%")).toBeNull();
  });

  it("shows a dash, not a number, for a missing per-run score", async () => {
    compareResult = {
      kind: "comparable",
      detail: {
        ...comparable.detail,
        runs: [run("exp-a"), run("exp-b", { successFraction: null, episodesCompleted: 0 })],
      },
    } as ComparisonResponse;
    await selectBoth();
    fireEvent.click(await screen.findByText(/per-run measurements/i));
    const table = screen.getByText(/per-run measurements/i).closest("details") as HTMLElement;
    const rows = within(table).getAllByRole("row");
    const row = rows.find((candidate) => within(candidate).queryByText("exp-b")) as HTMLElement;
    expect(within(row).getByText("—")).toBeInTheDocument();
    expect(within(row).queryByText("0.0%")).toBeNull();
  });
});

describe("failures", () => {
  it("reports a comparison that could not be made", async () => {
    vi.spyOn(api, "compareExperiments").mockRejectedValue(new Error("one experiment is unknown"));
    await selectBoth();
    expect(await screen.findByRole("alert")).toHaveTextContent(/one experiment is unknown/);
  });
});
