/**
 * The error boundary's own contract.
 *
 * A boundary is the last thing standing between a render fault and an unusable window, so its
 * behaviour is worth pinning exactly: contain the failure, name the surface, offer a way back, and
 * never pretend to have recovered when it has not.
 */

import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { PanelBoundary, PanelErrorBoundary } from "./ErrorBoundary";

/** A component that throws, on demand, so the failure can be produced and then removed. */
function Thrower({ when, error }: { when: boolean; error: Error }): JSX.Element {
  if (when) {
    throw error;
  }
  return <p>the panel body</p>;
}

describe("PanelErrorBoundary", () => {
  it("renders its children while they work", () => {
    render(
      <PanelBoundary label="Arena">
        <p>the panel body</p>
      </PanelBoundary>,
    );
    expect(screen.getByText("the panel body")).toBeInTheDocument();
    expect(screen.queryByText(/STOPPED/)).not.toBeInTheDocument();
  });

  it("contains a throw instead of unmounting the window", () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      render(
        <PanelBoundary label="Arena">
          <Thrower when error={new Error("the board renderer threw")} />
        </PanelBoundary>,
      );
      // The surrounding chrome survives — this is the whole point of a boundary.
      expect(screen.getByRole("heading", { name: /Arena stopped/i })).toBeInTheDocument();
      expect(screen.getByText(/could not be displayed/i)).toBeInTheDocument();
      // And it says so as an alert, because the content below must not be read as current.
      expect(screen.getByRole("alert")).toBeInTheDocument();
    } finally {
      consoleError.mockRestore();
    }
  });

  it("names the surface that failed", () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      render(
        <PanelBoundary label="Readiness">
          <Thrower when error={new Error("x")} />
        </PanelBoundary>,
      );
      expect(screen.getByRole("heading", { name: /Readiness stopped/i })).toBeInTheDocument();
    } finally {
      consoleError.mockRestore();
    }
  });

  it("shows the real message, not `[object Object]`", () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      render(
        <PanelBoundary label="Arena">
          <Thrower when error={new Error("the column index was off the board")} />
        </PanelBoundary>,
      );
      expect(screen.getByText(/the column index was off the board/)).toBeInTheDocument();
    } finally {
      consoleError.mockRestore();
    }
  });

  it("logs the failure rather than swallowing it", () => {
    // A boundary that neither shows nor logs a failure is indistinguishable from having none.
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      render(
        <PanelBoundary label="Arena">
          <Thrower when error={new Error("logged reason")} />
        </PanelBoundary>,
      );
      expect(
        consoleError.mock.calls.some((call) =>
          String(call[0]).includes("a panel failed to render"),
        ),
      ).toBe(true);
    } finally {
      consoleError.mockRestore();
    }
  });

  it("recovers when the underlying fault is removed and the user retries", () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      const { rerender } = render(
        <PanelBoundary label="Arena">
          <Thrower when error={new Error("transient")} />
        </PanelBoundary>,
      );
      expect(screen.getByRole("heading", { name: /Arena stopped/i })).toBeInTheDocument();
      // The fault clears — a transient data problem, a bad prop from a retry that has now succeeded.
      rerender(
        <PanelBoundary label="Arena">
          <Thrower when={false} error={new Error("transient")} />
        </PanelBoundary>,
      );
      // Still showing the boundary until asked, because recovery must be deliberate. An automatic
      // reset would hide a fault that is still there.
      expect(screen.getByRole("heading", { name: /Arena stopped/i })).toBeInTheDocument();
      fireEvent.click(screen.getByRole("button", { name: /retry render/i }));
      expect(screen.getByText("the panel body")).toBeInTheDocument();
      expect(screen.queryByRole("heading", { name: /Arena stopped/i })).not.toBeInTheDocument();
    } finally {
      consoleError.mockRestore();
    }
  });

  it("offers a surface-specific recovery action when one is supplied", () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    const onRecover = vi.fn();
    try {
      render(
        <PanelBoundary label="Arena" onRecover={onRecover} recoverLabel="RESTART RUN">
          <Thrower when error={new Error("x")} />
        </PanelBoundary>,
      );
      const recover = screen.getByRole("button", { name: /restart run/i });
      fireEvent.click(recover);
      expect(onRecover).toHaveBeenCalled();
    } finally {
      consoleError.mockRestore();
    }
  });

  it("tells the operator that other parts of the window are unaffected", () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      render(
        <PanelBoundary label="Arena">
          <Thrower when error={new Error("x")} />
        </PanelBoundary>,
      );
      expect(screen.getByText(/still usable/i)).toBeInTheDocument();
      expect(screen.getByText(/unaffected/i)).toBeInTheDocument();
    } finally {
      consoleError.mockRestore();
    }
  });

  it("keeps the technical detail out of the way until it is asked for", () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      const { container } = render(
        <PanelBoundary label="Arena">
          <Thrower when error={new Error("specific detail text")} />
        </PanelBoundary>,
      );
      const detail = container.querySelector(".panel-boundary__detail") as HTMLDetailsElement;
      expect(detail).toBeInTheDocument();
      expect(detail.open).toBe(false);
      expect(screen.getByText(/technical detail/i)).toBeInTheDocument();
    } finally {
      consoleError.mockRestore();
    }
  });

  it("has no fallback of its own when given nothing to protect", () => {
    // A boundary with no children is not a defect to guard against, but it must not crash.
    expect(() => render(<PanelErrorBoundary label="Arena">{null}</PanelErrorBoundary>)).not.toThrow();
  });
});