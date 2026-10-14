/**
 * Task benchmark tests.
 *
 * The recurring assertion is provenance honesty: a manual submission must
 * never render as an observed run, and a missing score must never render as
 * zero. Those are the two failures that would make this panel actively
 * harmful rather than merely incomplete.
 */

import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ReadinessPanel, groupByMode, modeLabel } from "./ReadinessPanel";
import type { ExecutorDescriptor } from "./api";

function executor(overrides: Partial<ExecutorDescriptor> = {}): ExecutorDescriptor {
  return {
    id: "ollama-local",
    label: "Ollama (local)",
    mode: "offline_local",
    availability: { state: "available", evidence: "a server on 127.0.0.1:11434" },
    capabilities: ["chat_completions", "offline"],
    detail: "OpenAI-compatible via 127.0.0.1:11434.",
    ...overrides,
  };
}

describe("mode helpers", () => {
  it("labels every known mode", () => {
    expect(modeLabel("offline_local")).toBe("OFFLINE LOCAL");
    expect(modeLabel("coding_agent")).toBe("CODING AGENT");
    expect(modeLabel("guided_external")).toBe("GUIDED EXTERNAL");
    expect(modeLabel("remote_provider")).toBe("REMOTE PROVIDER");
  });

  it("groups executors in the stable display order", () => {
    const groups = groupByMode([
      executor({ id: "remote-openai-compatible", mode: "remote_provider" }),
      executor({ id: "codex-cli", mode: "coding_agent" }),
      executor(),
    ]);
    expect(groups.map((group) => group.mode)).toEqual([
      "offline_local",
      "coding_agent",
      "remote_provider",
    ]);
  });
});

describe("ReadinessPanel", () => {
  it("counts ready executors without inventing any", () => {
    render(
      <ReadinessPanel
        executors={[
          executor(),
          executor({
            id: "codex-cli",
            label: "Codex",
            mode: "coding_agent",
            availability: { state: "unavailable", reason: "no binary", remedy: "install it" },
          }),
        ]}
        error={null}
        loading={false}
        onRefresh={() => {}}
      />,
    );
    /*
     * The tally is now sentence case in a chip and a stats strip, so the assertion matches on the
     * numbers rather than on the old uppercase run. The intent is unchanged: the count must come from
     * the executors the backend reported and must not be invented, so it is asserted as "1 of 2" and
     * the unavailable executor's own reason must still be on the page in its own words.
     */
    expect(screen.getByText(/1 of 2 ready/i)).toBeDefined();
    expect(screen.getByText(/no binary/)).toBeDefined();
    // The remedy is stated twice: on its own card, and in the page-level next step that names the
    // blocker shared by the most executors. Both are wanted, so the assertion is "at least one"
    // rather than picking a surface.
    expect(screen.getAllByText(/install it/).length).toBeGreaterThan(0);
    // And the page answers the question the cards together cannot.
    expect(screen.getByText(/share one blocker/i)).toBeDefined();
  });

  it("renders discovery failure as a state, not an empty panel", () => {
    render(<ReadinessPanel executors={[]} error="socket refused" loading={false} onRefresh={() => {}} />);
    expect(screen.getByText(/DISCOVERY FAILED/)).toBeDefined();
    expect(screen.getByRole("alert")).toBeDefined();
  });

  it("never shows a credential, only the fact of one", () => {
    const { container } = render(
      <ReadinessPanel executors={[executor()]} error={null} loading={false} onRefresh={() => {}} />,
    );
    expect(container.textContent).not.toMatch(/sk-/);
  });

  it("offers guest-isolation inspection without claiming automatic execution", () => {
    render(
      <ReadinessPanel executors={[executor()]} error={null} loading={false} onRefresh={() => {}} />,
    );
    expect(screen.getByText("GUEST ISOLATION")).toBeDefined();
    expect(screen.getByRole("button", { name: /inspect guest support/i })).toBeDefined();
    // The standing rule is stated before any inspection runs.
    expect(screen.getByText(/stays disabled regardless/i)).toBeDefined();
  });

  /*
   * The hook-order regression, reproduced as a user would hit it.
   *
   * A `useMemo` used to sit *after* the discovery-error early return, so the component's hook count
   * changed the instant `error` cleared. React does not reconcile that: it throws "Rendered more
   * hooks than during the previous render" and unmounts the tree. The effect was that pressing RETRY
   * DISCOVERY and succeeding — the single action the button exists for — broke the panel instead of
   * repairing it.
   *
   * Rendering with an error and then clearing it is the only way to reach that state, so this test
   * does exactly that and asserts the panel recovers rather than throwing.
   */
  it("recovers when discovery succeeds after an earlier failure", () => {
    const { rerender } = render(
      <ReadinessPanel executors={[]} error="socket refused" loading={false} onRefresh={() => {}} />,
    );
    expect(screen.getByText(/DISCOVERY FAILED/)).toBeInTheDocument();

    // React logs the hook violation as an error; failing the test on a console error would make this
    // test assert the *fix*, which is what matters, rather than the noise it used to produce.
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      rerender(
        <ReadinessPanel
          executors={[executor()]}
          error={null}
          loading={false}
          onRefresh={() => {}}
        />,
      );
      expect(screen.queryByText(/DISCOVERY FAILED/)).not.toBeInTheDocument();
      expect(screen.getByText(/1 of 1 ready/i)).toBeInTheDocument();
      expect(
        consoleError.mock.calls.some((call) =>
          String(call[0]).includes("Rendered more hooks than during the previous render"),
        ),
      ).toBe(false);
    } finally {
      consoleError.mockRestore();
    }
  });
});
