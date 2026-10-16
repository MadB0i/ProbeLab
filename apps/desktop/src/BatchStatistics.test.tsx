/**
 * Batch statistics.
 *
 * The tests here are about what the panel *refuses to claim*. Every assertion below corresponds to a way
 * a statistics display normally misleads: folding an infrastructure failure into a defeat, showing a
 * percentage with no denominator, summing two games together, or presenting four matches as though four
 * matches settle a question.
 */

import { render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { BatchStatistics } from "./BatchStatistics";
import type { ArenaStatistics, MatchStatistics } from "./api";

/*
 * No stylesheet is read here, and that is a decision rather than an omission.
 *
 * The obvious way to assert "the no-result bar is dashed so it cannot be read as a defeat" is to read
 * `styles.css` and match the declaration. Three attempts at that were worse than the class assertion it
 * replaced:
 *
 *   - `getComputedStyle` under jsdom returns an empty string for every property, because jsdom does not
 *     load the stylesheet. A `toMatch(/repeating-linear-gradient/)` against it fails for reasons that
 *     have nothing to do with this panel.
 *   - `import styles from "./styles.css?raw"` also resolved to an empty string under this runner's
 *     transform, so the test would have passed vacuously or failed spuriously.
 *   - Reading the file with `readFileSync` needs Node types, and `@types/node` is deliberately absent
 *     from this project — the app's `tsc` build is a browser project.
 *
 * What is asserted instead is the class contract: each bucket carries its own modifier, so the
 * stylesheet *can* treat a defeat and an unfinished match differently. Whether it does is a visual
 * question, and it is answered by looking at the rendered panel in Phase 5 rather than by reading its
 * own source back.
 */

function interval(point: number, lower: number, upper: number) {
  return { point, lower, upper, confidence: 0.95 };
}

function group(overrides: Partial<MatchStatistics> = {}): MatchStatistics {
  return {
    game: "connect_four",
    gameName: "Connect Four",
    testedAgent: "qwen2.5:0.5b",
    opponentAgent: "depth_bounded_connect_four",
    matches: 12,
    matchIds: ["m1", "m2", "m3", "m4", "m5", "m6", "m7", "m8", "m9", "m10", "m11", "m12"],
    provenanceDigest: "0123456789abcdef",
    decisions: {
      outcomes: { win: 5, draw: 3, loss: 2, noResult: 2 },
      winRate: interval(0.5, 0.24, 0.76),
      decisiveWinRate: interval(5 / 7, 0.36, 0.93),
      cancelled: 1,
      withFailure: 1,
      hasDecided: true,
    },
    plies: { total: 96, mean: 8, min: 5, max: 14 },
    population: {
      attempted: 12,
      conditional: 10,
      forfeited: 2,
      infrastructureFailures: 1,
      cancelled: 1,
      unclassified: 0,
    },
    illegalMoves: 3,
    matchesWithIllegalMoves: 2,
    costs: {
      measuredTurns: 40,
      unmeasuredTurns: 0,
      totalElapsedMs: 40000,
      meanElapsedMs: 1000,
      maxElapsedMs: 4200,
      promptTokens: 12000,
      completionTokens: 400,
      totalTokens: 12400,
      turnsWithoutUsage: 0,
    },
    sampleIsSmall: false,
    ...overrides,
  };
}

function statistics(groups: MatchStatistics[]): ArenaStatistics {
  return { groups, matchesRead: groups.reduce((n, g) => n + g.matches, 0), ungrouped: 0 };
}

function props(overrides: Partial<React.ComponentProps<typeof BatchStatistics>> = {}) {
  return {
    statistics: statistics([group()]),
    running: false,
    progress: null,
    requestedMatches: 10,
    alternateSides: true,
    canRun: true,
    onRunBatch: () => {},
    onCancelBatch: () => {},
    onAlternateSidesChange: () => {},
    onMatchesChange: () => {},
    ...overrides,
  };
}

describe("the outcome distribution", () => {
  it("draws four buckets, with no result as its own", () => {
    // The assertion that matters: a three-bucket chart cannot distinguish a defeat from a match that
    // was never played, so the count of buckets is checked, not just the labels.
    const { container } = render(<BatchStatistics {...props()} />);
    const rows = container.querySelectorAll(".stats-distribution__row");
    expect(rows).toHaveLength(4);
    const labels = [...rows].map((row) => row.querySelector(".stats-distribution__label")?.textContent);
    expect(labels).toEqual(["Won", "Drew", "Lost", "No result"]);
  });

  it("shows the real count beside every bucket", () => {
    // A bar with no number is a shape the reader has to measure.
    const { container } = render(<BatchStatistics {...props()} />);
    const counts = [...container.querySelectorAll(".stats-distribution__count")].map(
      (node) => node.textContent,
    );
    expect(counts).toEqual(["5", "3", "2", "2"]);
  });

  it("gives each bucket its own class, so a defeat and an unfinished match are distinguishable", () => {
    // Three similar solid bars invite a reader to collapse "lost" and "did not finish" into one fact.
    // The class contract is what the visual treatment hangs off, so this asserts the contract rather
    // than the paint: `styles.css` gives `.stats-distribution__row--noResult` a dashed fill and
    // `.stats-distribution__row--loss` a solid amber one, and that pairing is confirmed by eye in the
    // Phase 5 screenshots rather than from a jsdom computed style, which is always empty here.
    const { container } = render(<BatchStatistics {...props()} />);
    const loss = container.querySelector(".stats-distribution__row--loss");
    const noResult = container.querySelector(".stats-distribution__row--noResult");
    expect(loss).toBeTruthy();
    expect(noResult).toBeTruthy();
    expect(noResult?.className).not.toBe(loss?.className);
    // Each carries its own modifier, so the stylesheet can treat them differently.
    expect(loss?.classList.contains("stats-distribution__row--loss")).toBe(true);
    expect(noResult?.classList.contains("stats-distribution__row--noResult")).toBe(true);
  });

  it("renders a bar for every bucket even when its count is zero", () => {
    // A zero-width bar disappears, and a bucket that vanishes cannot be counted by eye. The `min-width`
    // in the stylesheet keeps it present; this asserts the element is there for it to apply to.
    const { container } = render(
      <BatchStatistics
        {...props({
          statistics: statistics([
            group({
              decisions: {
                ...group().decisions,
                outcomes: { win: 2, draw: 0, loss: 0, noResult: 0 },
                winRate: interval(1, 0.34, 1),
                decisiveWinRate: interval(1, 0.34, 1),
                hasDecided: true,
              },
            }),
          ]),
        })}
      />,
    );
    const fills = container.querySelectorAll(".stats-distribution__fill");
    expect(fills).toHaveLength(4);
    // The zero buckets are still in the DOM with a zero width, not omitted.
    const drawFill = container.querySelector(".stats-distribution__row--draw .stats-distribution__fill");
    expect(drawFill).toBeTruthy();
    expect(drawFill?.getAttribute("style")).toContain("width: 0%");
  });
});

describe("win rates", () => {
  it("puts the denominator on screen next to every percentage", () => {
    render(<BatchStatistics {...props()} />);
    // "50.0%" alone is a claim. "5 of 10" is a measurement.
    expect(screen.getByText("50.0%")).toBeInTheDocument();
    expect(screen.getByText("5 of 10")).toBeInTheDocument();
    expect(screen.getByText("5 of 7")).toBeInTheDocument();
  });

  it("reports both rates and says what each denominator means", () => {
    render(<BatchStatistics {...props()} />);
    expect(screen.getByText("Won, of matches that reached a result")).toBeInTheDocument();
    expect(screen.getByText("Won, of matches that had a winner")).toBeInTheDocument();
    // A reader must be able to tell why the two numbers differ.
    expect(screen.getByText(/draw counts here in the denominator/i)).toBeInTheDocument();
    expect(screen.getByText(/draws excluded/i)).toBeInTheDocument();
  });

  it("states the interval bounds in text, not only as a drawn bar", () => {
    const { container } = render(<BatchStatistics {...props()} />);
    expect(screen.getByText(/95% interval 24\.0%–76\.0%/)).toBeInTheDocument();
    // And the drawn bar carries the same numbers for anyone who cannot read the sentence.
    const bar = container.querySelector(".stats-interval");
    expect(bar).toHaveAttribute("role", "img");
    expect(bar?.getAttribute("aria-label")).toMatch(/confidence interval from 24\.0% to 76\.0%/);
    expect(bar?.getAttribute("aria-label")).toMatch(/95 percent confidence/);
  });

  it("reports no rate rather than zero when nothing reached a result", () => {
    // The common case after a cancelled or refused run. "0%" would be a claim about a match that never
    // happened.
    render(
      <BatchStatistics
        {...props({
          statistics: statistics([
            group({
              matches: 2,
              matchIds: ["a", "b"],
              sampleIsSmall: true,
              decisions: {
                outcomes: { win: 0, draw: 0, loss: 0, noResult: 2 },
                winRate: interval(0, 0, 1),
                decisiveWinRate: interval(0, 0, 1),
                cancelled: 1,
                withFailure: 1,
                hasDecided: false,
              },
            }),
          ]),
        })}
      />,
    );
    expect(screen.getAllByText(/nothing to measure/i).length).toBe(2);
    expect(screen.queryByText("0.0%")).not.toBeInTheDocument();
  });
});

describe("sample honesty", () => {
  it("warns in prose before the numbers when the sample is small", () => {
    // Nine decided matches: enough to be a distribution, not enough to be ten. This is the case the
    // warning exists for — a real run, short of the threshold — rather than a run of two.
    render(
      <BatchStatistics
        {...props({
          statistics: statistics([
            group({
              matches: 12,
              sampleIsSmall: true,
              decisions: {
                ...group().decisions,
                outcomes: { win: 5, draw: 2, loss: 2, noResult: 3 },
              },
            }),
          ]),
        })}
      />,
    );
    // Found by its class rather than by role.
    //
    // `role="note"` is a non-landmark role, and this group renders two of them whenever there are also
    // undecided matches — which there are here. Selecting the caution by role alone is ambiguous, and
    // selecting it by role plus text has the same problem: the test then depends on the very wording it
    // is meant to be checking. The class names the *kind* of note, which is the thing being selected.
    const caution = document.querySelector(".stats-group__caution");
    expect(caution).toBeTruthy();
    expect(caution).toHaveTextContent("Small sample");
    // The warning names the real counts rather than saying "sample too small", so a reader can judge how
    // far short the run fell. Decided matches are what count toward the threshold: the three undecided
    // matches in this group do not help it.
    // Whitespace-normalised, because JSX emits a newline plus indentation wherever a `{" "}` guard sits,
    // and asserting against the raw source layout would make this test fail on a reformat.
    expect((caution?.textContent ?? "").replace(/\s+/g, " ")).toMatch(
      /Only 9 decided matches of 12 matches reached a result/,
    );
    expect(caution).toHaveTextContent("below the 10");
  });

  it("does not warn when the sample is adequate", () => {
    render(<BatchStatistics {...props()} />);
    expect(screen.queryByText(/small sample/i)).not.toBeInTheDocument();
  });

  it("explains what happened to the undecided matches", () => {
    // Selected by class for the same reason as the caution: this panel renders several `role="note"`
    // elements, and the assertion is about which *kind* of note says this.
    render(<BatchStatistics {...props()} />);
    const note = document.querySelector(".stats-group__note");
    const text = (note?.textContent ?? "").replace(/\s+/g, " ");
    // The separation from "loss" is stated, not merely implemented in the arithmetic.
    expect(text).toMatch(/counted separately, not as defeats/);
    // Cancellation and failure are counted and described separately, because only one of them is about
    // the operator. Two undecided matches in this group: one the operator stopped, one that recorded a
    // failure reason.
    expect(text).toMatch(/1 match was stopped by the operator/);
    expect(text).toMatch(/1 match recorded a failure reason/);
    // The singular and plural forms are each asserted, because the verb has to agree with the number.
    // "1 matchs ... were stopped" is the failure this guards against.
    expect(text).not.toMatch(/\b1 matchs\b/);
    expect(text).not.toMatch(/1 match were stopped/);
  });
});

describe("game scoping", () => {
  it("keeps two games in separate groups and never sums them", () => {
    const { container } = render(
      <BatchStatistics
        {...props({
          statistics: statistics([
            group({ game: "tic_tac_toe", gameName: "Tic-Tac-Toe", matches: 9, matchIds: ["t1"] }),
            group({ game: "connect_four", gameName: "Connect Four", matches: 12 }),
          ]),
        })}
      />,
    );
    const groups = container.querySelectorAll(".stats-group");
    expect(groups).toHaveLength(2);
    expect(screen.getByText("Tic-Tac-Toe")).toBeInTheDocument();
    expect(screen.getByText("Connect Four")).toBeInTheDocument();
    // The scope claim is stated in the panel, so a combined total is explicitly ruled out rather than
    // merely absent.
    expect(screen.getByText(/games are never combined/i)).toBeInTheDocument();
    // And no cross-game figure exists anywhere. The wording is narrow on purpose: the panel's own prose
    // contains the word "combined" in its denial of one, so a broader pattern would match the rule and
    // pass for the wrong reason.
    const text = document.body.textContent ?? "";
    expect(text).not.toMatch(/overall win rate/i);
    expect(text).not.toMatch(/all games/i);
    expect(text).not.toMatch(/combined win rate/i);
  });

  it("names the game and both seats on every group", () => {
    // A win rate without its game and pairing is a number with no meaning attached.
    const { container } = render(<BatchStatistics {...props()} />);
    expect(container.querySelector(".stats-group__pairing")).toHaveTextContent(
      "qwen2.5:0.5b vs depth_bounded_connect_four",
    );
  });

  it("reports recordings it could not group rather than dropping them", () => {
    render(<BatchStatistics {...props({ statistics: { ...statistics([group()]), ungrouped: 2 } })} />);
    expect(screen.getByText(/2 recordings are not counted/i)).toBeInTheDocument();
  });
});

describe("provenance", () => {
  it("lists the digest and the recordings behind the numbers", () => {
    // The least attractive thing on the panel and the most important: a figure that cannot be traced
    // to its inputs cannot be checked.
    render(<BatchStatistics {...props()} />);
    expect(screen.getByText("0123456789abcdef")).toBeInTheDocument();
    expect(screen.getByText(/show the 12 recordings/i)).toBeInTheDocument();
    expect(document.querySelectorAll(".stats-match-ids li")).toHaveLength(12);
  });
});

describe("cost reporting", () => {
  it("says tokens were not reported rather than reporting zero", () => {
    // A provider that returns no usage block has not said the call was free.
    render(
      <BatchStatistics
        {...props({
          statistics: statistics([
            group({
              costs: {
                measuredTurns: 30,
                unmeasuredTurns: 0,
                totalElapsedMs: 900,
                meanElapsedMs: 30,
                maxElapsedMs: 90,
                promptTokens: null,
                completionTokens: null,
                totalTokens: null,
                turnsWithoutUsage: 30,
              },
            }),
          ]),
        })}
      />,
    );
    /*
     * Scoped to the cost figures rather than asserted document-wide.
     *
     * This used to be `queryByText(/^0$/)` over the whole panel, on the reasoning that a zero anywhere
     * would suggest something unreported had been turned into a number. The population block then
     * introduced genuine zeros — a run with no cancelled matches really did have zero cancelled matches
     * — and the broad assertion failed on a correct figure.
     *
     * The claim worth making is the original one: the *token* figures say "not reported" instead of 0.
     * So it is asserted against the token rows directly, which tests the intent rather than a proxy
     * that happened to be true when the panel was smaller.
     */
    const costFacts = screen.getByText("Total tokens").closest(".stats-stat") as HTMLElement;
    expect(costFacts).toHaveTextContent("not reported");
    expect(costFacts.textContent).not.toMatch(/\b0\b/);
    expect(screen.getAllByText("not reported").length).toBeGreaterThanOrEqual(3);
  });

  it("reports illegal moves as their own count", () => {
    render(<BatchStatistics {...props()} />);
    expect(screen.getByText("Illegal moves")).toBeInTheDocument();
  });
});

describe("the empty state", () => {
  it("says nothing is recorded rather than showing a zero row", () => {
    render(<BatchStatistics {...props({ statistics: null })} />);
    expect(screen.getByText(/nothing recorded yet/i)).toBeInTheDocument();
    expect(screen.queryByText("0.0%")).not.toBeInTheDocument();
    expect(screen.queryByText("Won")).not.toBeInTheDocument();
  });

  it("explains where the numbers come from", () => {
    render(<BatchStatistics {...props({ statistics: null })} />);
    expect(screen.getByText(/stored recordings/i)).toBeInTheDocument();
  });
});

describe("running a batch", () => {
  it("shows live progress as a running total, never accumulating locally", () => {
    render(
      <BatchStatistics
        {...props({
          running: true,
          canRun: false,
          progress: { completed: 4, requested: 10, outcome: "WIN" },
        })}
      />,
    );
    expect(screen.getByRole("status")).toHaveTextContent("4 of 10 played");
    expect(screen.getByRole("status")).toHaveTextContent("last WIN");
  });

  it("offers a stop control while running and a run control when idle", () => {
    const onCancelBatch = vi.fn();
    const onRunBatch = vi.fn();
    const { rerender } = render(
      <BatchStatistics {...props({ running: true, onCancelBatch })} />,
    );
    fireStop(onCancelBatch);
    expect(onCancelBatch).toHaveBeenCalled();

    rerender(<BatchStatistics {...props({ running: false, onRunBatch })} />);
    expect(screen.getByRole("button", { name: /run 10/i })).toBeEnabled();
    expect(screen.queryByRole("button", { name: /stop batch/i })).not.toBeInTheDocument();
  });

  it("disables running when the two seats are the same agent", () => {
    // The backend refuses this, and an enabled button for an action that will be rejected is a control
    // that lies about what is about to happen.
    render(<BatchStatistics {...props({ canRun: false })} />);
    expect(screen.getByRole("button", { name: /run 10/i })).toBeDisabled();
  });

  it("states that alternating sides is on and what that means", () => {
    render(<BatchStatistics {...props({ alternateSides: true })} />);
    const toggle = screen.getByRole("checkbox", { name: /alternate sides/i });
    expect(toggle).toBeChecked();
    expect(screen.getByText(/given a handicap, not tested/i)).toBeInTheDocument();
  });

  it("bounds the match count to what the backend accepts", () => {
    const { container } = render(<BatchStatistics {...props()} />);
    const input = container.querySelector(".arena-batch__controls input[type=number]")!;
    expect(input).toHaveAttribute("min", "1");
    expect(input).toHaveAttribute("max", "50");
  });

  it("locks the setup while a batch is running", () => {
    const { container } = render(<BatchStatistics {...props({ running: true })} />);
    expect(container.querySelector("input[type=number]")).toBeDisabled();
    expect(screen.getByRole("checkbox", { name: /alternate sides/i })).toBeDisabled();
  });
});

/** Clicks the stop control, which is only present while a batch is running. */
function fireStop(onCancelBatch: () => void) {
  const button = screen.getByRole("button", { name: /stop batch/i });
  button.click();
  expect(onCancelBatch).toHaveBeenCalled();
}

describe("accessibility", () => {
  it("names the statistics region", () => {
    render(<BatchStatistics {...props()} />);
    const region = screen.getByRole("region", { name: /batch statistics/i });
    expect(region).toBeInTheDocument();
  });

  it("keeps the caution available as a note rather than as colour alone", () => {
    render(
      <BatchStatistics
        {...props({
          statistics: statistics([group({ sampleIsSmall: true })]),
        })}
      />,
    );
    // `role="note"` is not focusable and is not announced unprompted, which is right for a caution that
    // accompanies figures the reader has already chosen to read — but it must exist in the tree.
    expect(within(screen.getByRole("region", { name: /batch statistics/i })).getAllByRole("note").length)
      .toBeGreaterThan(0);
  });
});

/*
 * Audit finding B6 — the population must be visible.
 *
 * The backend change is worthless if the panel still shows only a win rate: a reader would be looking
 * at "100%" over three matches while seven failures sat invisibly outside the denominator. These assert
 * that every bucket is drawn, with its denominator stated, and that the three figures cannot be
 * confused for one another.
 */
describe("execution reliability (audit B6)", () => {
  it("states the denominator beside every rate", () => {
    render(<BatchStatistics {...props()} />);
    expect(screen.getByText("Execution reliability")).toBeInTheDocument();
    // 10 of 12 attempted, from the fixture's population.
    expect(screen.getByText("10 of 12 attempted")).toBeInTheDocument();
  });

  it("draws every population bucket including the ones that are zero", () => {
    render(<BatchStatistics {...props()} />);
    for (const label of [
      "Attempted",
      "Reached a game outcome",
      "Forfeited by the tested seat",
      "Provider or harness failure",
      "Stopped by the operator",
      "Reason not recorded",
    ]) {
      expect(screen.getByText(label)).toBeInTheDocument();
    }
  });

  it("names the excluded matches so they cannot be read as a game outcome", () => {
    render(<BatchStatistics {...props()} />);
    expect(
      screen.getByText(/2 matches are outside the game outcomes and do not appear as a win, a loss or a forfeit/),
    ).toBeInTheDocument();
  });

  it("says a forfeit is counted as a loss rather than a fault", () => {
    render(<BatchStatistics {...props()} />);
const forfeited = screen.getByLabelText(/Counted as a loss/) as HTMLElement;
    expect(forfeited.textContent).toContain("2");
  });

  it("says an infrastructure failure is not a game outcome", () => {
    render(<BatchStatistics {...props()} />);
const failure = screen.getByLabelText(
      /^Provider or harness failure: 1\. Not a game outcome\./,
    ) as HTMLElement;
    expect(failure.textContent).toBe("1");
  });

  it("shows that a legacy recording's reason is unknown rather than guessing", () => {
    render(
      <BatchStatistics
        {...props({
          statistics: statistics([
            group({
              matches: 6,
              population: {
                attempted: 6,
                conditional: 3,
                forfeited: 0,
                infrastructureFailures: 0,
                cancelled: 0,
                unclassified: 3,
              },
            }),
          ]),
        })}
      />,
    );
const unknown = screen.getByLabelText(/not known and is not guessed/) as HTMLElement;
    expect(unknown.textContent).toContain("3");
  });

  it("reports zero reliability as 0.0% rather than hiding an absent figure", () => {
    // Every match failed to reach the provider: nothing was measured about the seat at all.
    render(
      <BatchStatistics
        {...props({
          statistics: statistics([
            group({
              matches: 4,
              decisions: {
                outcomes: { win: 0, draw: 0, loss: 0, noResult: 4 },
                winRate: { point: 0, lower: 0, upper: 0, confidence: 0.95 },
                decisiveWinRate: { point: 0, lower: 0, upper: 0, confidence: 0.95 },
                cancelled: 0,
                withFailure: 4,
                hasDecided: false,
              },
              population: {
                attempted: 4,
                conditional: 0,
                forfeited: 0,
                infrastructureFailures: 4,
                cancelled: 0,
                unclassified: 0,
              },
            }),
          ]),
        })}
      />,
    );
    expect(screen.getByText("0 of 4 attempted")).toBeInTheDocument();
    expect(
      screen.getByText(/4 matches are outside the game outcomes/),
    ).toBeInTheDocument();
  });
});
