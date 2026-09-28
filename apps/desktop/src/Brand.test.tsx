/**
 * Brand lockup tests.
 *
 * The product had three brand treatments at once, with two different subtitles, and the app icon was
 * an SVG nothing referenced. These tests exist to hold the two things that were fixed: one lockup, and
 * an icon source that is actually a source.
 */
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { BrandLockup, BrandMark } from "./Brand";

describe("BrandMark", () => {
  it("is decorative, so the wordmark carries the accessible name", () => {
    // A screen reader announcing "concentric circles" before "ProbeLab" is noise, and the wordmark
    // beside it already says the name.
    const { container } = render(<BrandMark />);
    const svg = container.querySelector("svg");
    expect(svg).not.toBeNull();
    expect(svg?.getAttribute("aria-hidden")).toBe("true");
    expect(svg?.getAttribute("focusable")).toBe("false");
    // No <title>, so there is nothing for a screen reader to pick up as a name either.
    expect(svg?.querySelector("title")).toBeNull();
  });

  it("draws the outer boundary open and the inner layers closed", () => {
    // The mark's whole claim: a subject inside nested boundaries, with the outermost one -- the
    // containment layer -- not closed. ProbeLab exists to find out whether a boundary holds, and
    // three unbroken circles would assert an answer nobody has earned.
    const { container } = render(<BrandMark />);
    const circles = [...(container.querySelectorAll("circle") ?? [])];
    // Three rings plus the solid subject at the centre.
    expect(circles).toHaveLength(4);

    const [outer, broker, adapter, subject] = circles;
    // Radii increase outwards; the subject is filled and the three layers are strokes.
    const radius = (c: Element) => Number(c.getAttribute("r"));
    expect(radius(outer)).toBeGreaterThan(radius(broker));
    expect(radius(broker)).toBeGreaterThan(radius(adapter));
    expect(subject.getAttribute("fill")).toBe("var(--cyan)");

    // Exactly the two layers that can refuse -- containment and the broker -- are drawn open. The
    // adapter ring is the tightest boundary and stays closed, because nothing refuses there.
    expect(outer.getAttribute("stroke-dasharray")).toBeTruthy();
    expect(broker.getAttribute("stroke-dasharray")).toBeTruthy();
    expect(adapter.getAttribute("stroke-dasharray")).toBeNull();
  });

  it("colours itself from the palette tokens rather than hard-coded hex", () => {
    // The desktop icon has to use literal hex because a .ico cannot read CSS variables; the in-app
    // mark can, and should, so it follows the theme.
    const { container } = render(<BrandMark />);
    const circles = [...container.querySelectorAll("circle")];
    // Every painted attribute is a token. The centre subject is filled, the three layers stroked, so
    // both attributes are checked -- an unpainted circle has neither and proves nothing.
    const painted = circles.flatMap((c) =>
      [c.getAttribute("stroke"), c.getAttribute("fill")].filter(
        (value): value is string => value !== null,
      ),
    );
    expect(painted).toHaveLength(4);
    for (const value of painted) {
      expect(value).toMatch(/^var\(--(cyan|violet)\)$/);
    }
  });
});

describe("BrandLockup", () => {
  it("renders the product name exactly once, in one spelling", () => {
    render(<BrandLockup />);
    // `PROBE//LAB` is split across elements by the violet separator, so the accessible name is
    // assembled from the text content rather than matched as one string.
    expect(document.body.textContent).toContain("PROBE//LAB");
  });

  it("defaults to the same subtitle in both sizes, so the two places cannot disagree", () => {
    const { unmount } = render(<BrandLockup />);
    const nav = document.body.textContent;
    unmount();

    render(<BrandLockup compact />);
    const topbar = document.body.textContent;

    expect(nav).toBe(topbar);
    // The subtitle that used to differ between the nav ("EVALUATION LABORATORY") and the topbar
    // ("AI CAPABILITY & AGENT EVALUATION"). One string now, so there is nothing left to drift.
    expect(topbar).toContain("AI CAPABILITY & AGENT EVALUATION");
    expect(topbar).not.toContain("EVALUATION LABORATORY");
  });

  it("still allows an explicit subtitle, for a context that needs its own wording", () => {
    render(<BrandLockup subtitle="EVALUATION LABORATORY" />);
    expect(document.body.textContent).toContain("EVALUATION LABORATORY");
  });
});
