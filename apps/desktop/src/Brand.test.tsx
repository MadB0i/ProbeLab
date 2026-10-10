/**
 * Brand lockup tests.
 *
 * The product had three brand treatments at once, with two different subtitles, and the app icon was
 * an SVG nothing referenced. These tests hold two things: that there is one lockup, and that the mark
 * in the app is the same four-shape mark the icon set is generated from.
 *
 * The geometry assertions are structural rather than pixel assertions on purpose. Comparing rendered
 * pixels against a fixture would pass for a mark that has drifted in a way nobody would notice at 32px,
 * and would fail on a harmless renderer difference. What matters is that the shape *count* and
 * *topology* are right -- four shapes, the bowl open, the needle stopping inside the counter -- because
 * those are the properties that make the mark legible at taskbar size.
 */
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { BrandLockup, BrandMark } from "./Brand";

describe("BrandMark", () => {
  it("is decorative, so the wordmark carries the accessible name", () => {
    // A screen reader announcing "the letter P with a probe" before "ProbeLab" is noise, and the
    // wordmark beside it already says the name.
    const { container } = render(<BrandMark />);
    const svg = container.querySelector("svg");
    expect(svg).not.toBeNull();
    expect(svg?.getAttribute("aria-hidden")).toBe("true");
    expect(svg?.getAttribute("focusable")).toBe("false");
    expect(svg?.querySelector("title")).toBeNull();
  });

  it("draws the Signal Aperture as exactly four shapes", () => {
    // Four: stem, two bowl blades, one node. The needle is a line and the bowl is two paths, so the
    // total painted element count is the thing worth pinning -- it is the number that decides whether
    // the mark still reads at 16px.
    const { container } = render(<BrandMark />);
    expect(container.querySelectorAll("path")).toHaveLength(3);
    expect(container.querySelectorAll("line")).toHaveLength(1);
    expect(container.querySelectorAll("circle")).toHaveLength(1);
  });

  it("leaves the aperture open rather than closing it into a ring", () => {
    /*
     * The bowl is two arcs with a wedge between them, and the gap is real path geometry.
     *
     * This is the property that distinguishes the mark from the generic concentric-target icon it was
     * commissioned to replace: a closed bowl would assert a completed boundary, and a probe lab exists
     * precisely because boundaries are untested. Two arcs cannot accidentally close into a ring even
     * if a renderer joins them, because they are separate subpaths.
     */
    const { container } = render(<BrandMark />);
    const bowls = [...container.querySelectorAll("path")].filter((p) =>
      (p.getAttribute("d") ?? "").includes("A 126 126"),
    );
    expect(bowls).toHaveLength(2);
    // Each bowl blade is an annulus segment: outer arc out, line in, inner arc back.
    for (const blade of bowls) {
      const d = blade.getAttribute("d") ?? "";
      expect(d.match(/A /g)).toHaveLength(2);
      expect(d).toContain("Z");
    }
  });

  it("stops the needle at the counter instead of crossing the whole mark", () => {
    // A needle that ran through and out the far side read as a Q-tail or a magnifier handle in the
    // concept review. The tip must land inside the bowl's inner radius.
    const { container } = render(<BrandMark />);
    const needle = container.querySelector("line");
    expect(needle).not.toBeNull();
    const tipX = Number(needle?.getAttribute("x2"));
    const tipY = Number(needle?.getAttribute("y2"));
    // Bowl centre (224,224), inner radius 56, plus the needle's own half-width of 14.
    const distanceFromBowlCentre = Math.hypot(tipX - 224, tipY - 224);
    expect(distanceFromBowlCentre).toBeLessThanOrEqual(56 + 14);

    // And it must reach out past the bowl's outer radius, or it is not entering anything.
    const tailX = Number(needle?.getAttribute("x1"));
    const tailY = Number(needle?.getAttribute("y1"));
    expect(Math.hypot(tailX - 224, tailY - 224)).toBeGreaterThan(126);
  });

  it("puts the node wholly inside the bowl's counter, clear of the stroke", () => {
    /*
     * The counter is the hole the node occupies; that negative space is the letter's eye.
     *
     * This test exists because it caught a real defect. The node sat 29px from the bowl centre with a
     * 32px radius, so its edge reached 61px against a 56px counter radius -- it ate 5px into the bowl.
     * At 256px that reads as a slightly tight counter; at 16px the node and the bowl fuse into a single
     * cyan blob and the "aperture" disappears. The clearance below is a design rule, not a snapshot of
     * current numbers: the gap has to survive the smallest raster.
     */
    const { container } = render(<BrandMark />);
    const node = container.querySelector("circle");
    const cx = Number(node?.getAttribute("cx"));
    const cy = Number(node?.getAttribute("cy"));
    const r = Number(node?.getAttribute("r"));

    const reach = Math.hypot(cx - 224, cy - 224) + r;
    expect(reach).toBeLessThanOrEqual(56);
    // A visible gap, not merely a non-overlap: below 24px the two cyan shapes must still read apart.
    expect(56 - reach).toBeGreaterThanOrEqual(6);
  });

  it("uses the stem's currentColor so the mark inverts between the navy rail and white toolbar", () => {
    // The blue and cyan are the brand and stay constant; the stem is structure and follows the surface.
    const { container } = render(<BrandMark />);
    const stem = container.querySelector('path[fill="currentColor"]');
    expect(stem).not.toBeNull();
  });

  it("references the two brand colour tokens rather than literal hex values", () => {
    /*
     * The blue and cyan must come from the stylesheet's `--brand-blue` / `--brand-cyan`, which are
     * mirrored from the SVG.
     *
     * An earlier version wrote `var(--brand-cyan, #00C6CC)` -- a token plus a hex fallback -- and the
     * intent was defensive. The effect was the opposite: the fallback made the mark render a *different*
     * colour from the icon set on any surface where the variable failed to resolve, which is precisely
     * the drift this file exists to catch, silently. No fallbacks; if the token is missing the shape is
     * unpainted and that is a louder, better failure.
     */
    const { container } = render(<BrandMark />);
    const html = container.innerHTML;
    expect(html.match(/#[0-9a-fA-F]{3,8}\b/g) ?? []).toHaveLength(0);
    // Every painted shape resolves to a token. `fill="none"` on the root is a declaration, not a
    // colour, so it is excluded rather than special-cased.
    const colours = [...html.matchAll(/(?:fill|stroke)="([^"]+)"/g)]
      .map((m) => m[1])
      .filter((value) => value !== "none");
    expect(colours).toHaveLength(5);
    for (const colour of colours) {
      expect(colour).toMatch(/^(currentColor|var\(--brand-[a-z]+\))$/);
    }
  });

  it("crops its viewBox to the artwork rather than the full canvas", () => {
    // A mark occupying 42% of its canvas renders as a 13px smudge in a 32px rail slot. The viewBox is
    // therefore the ink bounds (x 148-361, y 99-408) plus a little air.
    const { container } = render(<BrandMark />);
    const viewBox = container.querySelector("svg")?.getAttribute("viewBox") ?? "";
    const [, , w, h] = viewBox.split(" ").map(Number);
    expect(w).toBeLessThan(512);
    expect(h).toBeLessThan(512);
    // The ink must fill most of the box, or the crop did not happen.
    expect(w).toBeLessThan(280);
    expect(h).toBeLessThan(380);
  });

  it("describes the same four shapes as the SVG the icon set is generated from", async () => {
    /*
     * This is the drift guard.
     *
     * The React mark and `src-tauri/app-icon.svg` are two renderings of one design kept in two files,
     * because one has to be valid JSX and the other a standalone asset. They can drift, and the only
     * symptom would be an app whose taskbar icon and whose in-app rail are subtly different marks --
     * the kind of thing nobody notices and everybody feels.
     *
     * So the guard checks that the geometry defining the mark (bowl radii, arc endpoints, needle
     * endpoints, node centre and radius) appears in the SVG. The SVG is read through Vite's `?raw`
     * import rather than `node:fs`, which keeps the browser-side type environment happy.
     */
    const svg: string = (await import("../src-tauri/app-icon.svg?raw")).default;
    const { container } = render(<BrandMark />);
    const paths = [...container.querySelectorAll("path")].map((p) => p.getAttribute("d"));
    const needle = container.querySelector("line");
    const node = container.querySelector("circle");

    for (const d of paths) {
      expect(svg).toContain(d as string);
    }
    expect(svg).toContain(`x1="${needle?.getAttribute("x1")}"`);
    expect(svg).toContain(`x2="${needle?.getAttribute("x2")}"`);
    expect(svg).toContain(`y1="${needle?.getAttribute("y1")}"`);
    expect(svg).toContain(`y2="${needle?.getAttribute("y2")}"`);
    expect(svg).toContain(`cx="${node?.getAttribute("cx")}"`);
    expect(svg).toContain(`cy="${node?.getAttribute("cy")}"`);
    expect(svg).toContain(`r="${node?.getAttribute("r")}"`);
  });

  it("keeps the app-icon stem legible against its own tile", async () => {
    /*
     * The geometry guard above cannot catch this one, and it shipped exactly this failure.
     *
     * The master mark's stem is navy #172B39, correct for the white app chrome. The app icon puts that
     * same mark on a navy tile, where a navy stem rendered as a near-black slab: it ate the bowl's
     * stem-side edge and collapsed the P into a blob at 16px. Every structural assertion still passed,
     * because the shape was still there -- only its contrast against the thing behind it was gone.
     *
     * So the icon inverts the ink, not the geometry. This asserts the tile and the stem differ by a
     * real amount of luminance, which is the property that actually failed.
     */
    const svg: string = (await import("../src-tauri/app-icon.svg?raw")).default;

    const luminance = (hex: string): number => {
      const n = parseInt(hex.replace("#", ""), 16);
      const [r, g, b] = [(n >> 16) & 255, (n >> 8) & 255, n & 255].map((c) => {
        const s = c / 255;
        return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
      });
      return 0.2126 * r + 0.7152 * g + 0.0722 * b;
    };

    const tileStops = [...svg.matchAll(/stop-color="(#[0-9A-Fa-f]{6})"/g)].map((m) => m[1]);
    const stem = svg.match(/id="stem">\s*<path[^>]*fill="(#[0-9A-Fa-f]{6})"/)?.[1];

    expect(tileStops.length).toBeGreaterThan(0);
    expect(stem).toBeDefined();

    // The darkest tile stop is what the stem has to separate from at the tile's lower edge.
    const darkestTile = Math.min(...tileStops.map(luminance));
    const contrast = Math.abs(luminance(stem as string) - darkestTile);

    // ~0.08 is far below the visible threshold; a navy stem on a navy tile lands near 0.01.
    expect(contrast).toBeGreaterThan(0.08);
  });
});

describe("BrandLockup", () => {
  it("renders the product name exactly once, in one spelling", () => {
    render(<BrandLockup />);
    // `PROBE//LAB` is split across elements by the blue separator, so the accessible name is
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