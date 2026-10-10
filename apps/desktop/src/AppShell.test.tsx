/**
 * Shell tests.
 *
 * Routing is the one piece of this UI with real state and a keyboard contract, so it is the piece
 * most worth pinning: a nav that silently fails to switch pages looks identical to a nav that works
 * until someone relies on it, and an unreachable destination cannot be reported by a user who cannot
 * find it.
 *
 * The assertions deliberately test behaviour a user can observe -- which page is shown, what is
 * announced to a screen reader, whether the back button works -- rather than the implementation.
 */

import { fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AppShell, hash_for, route_from_hash, type RouteId } from "./AppShell";

const PAGES: Record<RouteId, string> = {
  readiness: "READINESS CONTENT",
  lab: "COMMAND CENTER CONTENT",
  agents: "AGENTS CONTENT",
  benchmarks: "BENCHMARKS CONTENT",
  tasks: "TASKS CONTENT",
  comparison: "COMPARISON CONTENT",
  settings: "SETTINGS CONTENT",
  providers: "PROVIDERS CONTENT",
  arena: "ARENA CONTENT",
  security: "SECURITY CONTENT",
  replay: "REPLAY CONTENT",
};

const routes = Object.fromEntries(
  Object.entries(PAGES).map(([id, content]) => [id, <p>{content}</p>]),
) as Record<RouteId, JSX.Element>;

const renderShell = () => render(<AppShell routes={routes} />);

const nav = () => screen.getByRole("navigation", { name: /command center sections/i });

afterEach(() => {
  window.location.hash = "";
});

describe("hash parsing", () => {
  it("accepts every destination's own hash", () => {
    for (const id of Object.keys(PAGES) as RouteId[]) {
      expect(route_from_hash(hash_for(id))).toBe(id);
    }
  });

  it("accepts a hash written without the leading slash", () => {
    expect(route_from_hash("#arena")).toBe("arena");
    expect(route_from_hash("#/arena")).toBe("arena");
  });

  it("ignores case and surrounding whitespace", () => {
    expect(route_from_hash(" #/ARENA ")).toBe("arena");
  });

  /**
   * An unknown hash falls back to the command center rather than rendering nothing. The alternative
   * is a blank page from a stale link, which the user cannot tell apart from a crash.
   */
  it("falls back to the command center for an unknown or empty hash", () => {
    expect(route_from_hash("#/does-not-exist")).toBe("lab");
    expect(route_from_hash("")).toBe("lab");
    expect(route_from_hash("#")).toBe("lab");
  });
});

describe("destination navigation", () => {
  it("shows the command center when no hash is set", () => {
    renderShell();
    expect(screen.getByText(PAGES.lab)).toBeInTheDocument();
  });

  it("switches pages when an item is clicked", () => {
    renderShell();
    fireEvent.click(within(nav()).getByRole("button", { name: /security/i }));
    expect(screen.getByText(PAGES.security)).toBeInTheDocument();
    expect(screen.queryByText(PAGES.lab)).not.toBeInTheDocument();
  });

  it("writes the destination into the hash so a page can be linked to", () => {
    renderShell();
    fireEvent.click(within(nav()).getByRole("button", { name: /replay/i }));
    expect(window.location.hash).toBe("#/replay");
  });

  it("marks exactly one destination as the current page for assistive technology", () => {
    renderShell();
    const current = within(nav()).getAllByRole("button").filter((button) =>
      button.getAttribute("aria-current") === "page",
    );
    expect(current).toHaveLength(1);
    expect(current[0]).toHaveTextContent("LAB");
  });

  it("moves the current-page marker with the selection", () => {
    renderShell();
    fireEvent.click(within(nav()).getByRole("button", { name: /arena/i }));
    const current = within(nav()).getAllByRole("button").filter((button) =>
      button.getAttribute("aria-current") === "page",
    );
    expect(current).toHaveLength(1);
    expect(current[0]).toHaveTextContent("ARENA");
  });

  it("follows a hash change made elsewhere, so the back button works", () => {
    renderShell();
    window.location.hash = "#/providers";
    fireEvent(window, new HashChangeEvent("hashchange"));
    expect(screen.getByText(PAGES.providers)).toBeInTheDocument();
  });

  it("names the page it is on", () => {
    renderShell();
    fireEvent.click(within(nav()).getByRole("button", { name: /benchmarks/i }));
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Benchmarks");
  });
});

describe("keyboard navigation", () => {
  it("moves to the next destination with the down arrow", () => {
    renderShell();
    // Down follows the rail order the reader can see, which is the grouped order -- so the item
    // below LAB is TASKS, not AGENTS. Asserting against the rendered order rather than the
    // declaration order is what makes this a test of the keyboard agreeing with the screen.
    fireEvent.keyDown(within(nav()).getByRole("button", { name: "LAB" }), { key: "ArrowDown" });
    expect(screen.getByText(PAGES.tasks)).toBeInTheDocument();
  });

  it("moves to the previous destination with the up arrow", () => {
    renderShell();
    fireEvent.keyDown(within(nav()).getByRole("button", { name: "LAB" }), { key: "ArrowUp" });
    // Up from the command center reaches readiness, which sits first in the list.
    expect(screen.getByText(PAGES.readiness)).toBeInTheDocument();
  });

  it("wraps in both directions so the list is not a dead end", () => {
    renderShell();
    // The last rail item, found from the DOM rather than named, so this still tests the wrap when
    // the rail's last destination changes.
    const items = within(nav()).getAllByRole("button");
    fireEvent.keyDown(items[items.length - 1], { key: "ArrowDown" });
    expect(screen.getByText(PAGES.readiness)).toBeInTheDocument();
  });

  /**
   * A user can focus a nav item without activating it -- clicking is not the only way to focus. The
   * arrow key must then move relative to the item under focus, not to whatever page is open, or the
   * press would appear to do nothing.
   */
  it("arrows from the focused item, not from the currently open page", () => {
    renderShell();
    const security = within(nav()).getByRole("button", { name: "SECURITY" });
    // Focus without activating: no click, and the command center is still the open page.
    security.focus();
    fireEvent.keyDown(security, { key: "ArrowDown" });
    expect(screen.getByText(PAGES.replay)).toBeInTheDocument();
  });

  it("jumps to the first and last destinations with Home and End", () => {
    renderShell();
    const items = within(nav()).getAllByRole("button");
    // SETTINGS is last in the rail; that it is last is pinned by the group-order test below.
    fireEvent.keyDown(items[1], { key: "End" });
    expect(screen.getByText(PAGES.settings)).toBeInTheDocument();
    fireEvent.keyDown(screen.getByRole("button", { name: "SETTINGS" }), { key: "Home" });
    expect(screen.getByText(PAGES.readiness)).toBeInTheDocument();
  });

  /*
   * The rail is grouped, and the group order is the reading order. It is pinned here because the
   * keyboard walk, the printed indices, and the visual order are all derived from it: if it changed,
   * three things would move together and none of them individually looks wrong.
   */
  it("renders the destinations in the declared group order", () => {
    renderShell();
    expect(within(nav()).getAllByRole("button").map((button) => button.textContent?.replace(/\d/g, ""))).toEqual([
      "READY",
      "LAB",
      "TASKS",
      "AGENTS",
      "BENCHMARKS",
      "PROVIDERS",
      "COMPARE",
      "ARENA",
      "SECURITY",
      "REPLAY",
      "SETTINGS",
    ]);
  });

  it("numbers the rail so a destination can be named by position", () => {
    renderShell();
    const indices = Array.from(
      nav().querySelectorAll<HTMLElement>(".nav-item-index"),
    ).map((node) => node.textContent);
    expect(indices).toHaveLength(Object.keys(PAGES).length);
    expect(indices[0]).toBe("01");
    expect(indices.at(-1)).toBe(Object.keys(PAGES).length.toString().padStart(2, "0"));
  });

  /*
   * The index is a visual affordance for scanning the rail. It must not reach the accessible name,
   * because a destination announced as "zero two, Command Center" is worse for a screen-reader user
   * than one announced as "Command Center".
   */
  it("keeps the rail index out of each destination's accessible name", () => {
    renderShell();
    expect(within(nav()).getByRole("button", { name: "LAB" })).toBeInTheDocument();
    expect(within(nav()).queryByRole("button", { name: /0?2.*LAB/ })).toBeNull();
  });

  /**
   * The roving focus has to follow the selection. If it did not, the keyboard and the screen would
   * disagree: the page would have changed while focus sat on the previous item, and the next arrow
   * press would jump from the wrong place.
   */
it("moves focus with the selection", () => {
    renderShell();
    fireEvent.keyDown(within(nav()).getByRole("button", { name: "LAB" }), { key: "ArrowDown" });
    expect(within(nav()).getByRole("button", { name: "TASKS" })).toHaveFocus();
  });

  /**
   * Deliberately not a focus trap. Tab has to leave the sidebar, because a user tabbing through the
   * application must reach the page content without arrowing past a navigation column first.
   */
  it("does not swallow keys it does not use, so Tab still leaves the nav", () => {
    const preventDefault = vi.fn();
    renderShell();
    const event = new KeyboardEvent("keydown", { key: "Tab", cancelable: true });
    event.preventDefault = preventDefault;
    fireEvent(within(nav()).getByRole("button", { name: "LAB" }), event);
    expect(preventDefault).not.toHaveBeenCalled();
  });

  it("keeps every destination reachable by tabbing through the nav", () => {
    renderShell();
    // The nav holds real buttons, so they are in the tab order without a roving tabindex. This test
    // pins that: a roving tabindex that left only one item focusable would break keyboard access,
    // and it is the classic way a sidebar becomes unusable.
    const items = within(nav()).getAllByRole("button");
    expect(items).toHaveLength(Object.keys(PAGES).length);
    for (const item of items) {
      expect(item).not.toHaveAttribute("tabindex", "-1");
    }
  });
});

describe("accessibility affordances", () => {
  it("exposes the nav as a labelled landmark", () => {
    renderShell();
    expect(nav()).toBeInTheDocument();
  });

  it("offers a skip link to the content for keyboard users", () => {
    renderShell();
    const skip = screen.getByRole("link", { name: /skip to content/i });
    expect(skip).toHaveAttribute("href", "#main-content");
  });

  it("gives the content region an id the skip link can target", () => {
    renderShell();
    expect(document.getElementById("main-content")).not.toBeNull();
  });

  it("states on the nav what is not here, instead of offering dead entries", () => {
    renderShell();
    // No disabled or "coming soon" buttons: a reader can tell what is missing from this note, and
    // every button in the nav is a destination that works.
    expect(within(nav()).queryAllByRole("button")).toHaveLength(Object.keys(PAGES).length);
    for (const button of within(nav()).getAllByRole("button")) {
      expect(button).toBeEnabled();
    }
    expect(within(nav()).getByText(/documented in the readme/i)).toBeInTheDocument();
  });
});

describe("the skip link", () => {
  /*
   * The skip link used to be a child of `<main>`, rendered after the header, with `<nav>` before
   * `<main>` in DOM order. Tab order was therefore nine sidebar buttons and *then* the skip link: a
   * control whose entire purpose is bypassing the sidebar was reachable only after tabbing through
   * the sidebar. The mouse hides this completely, which is why it survived.
   *
   * Asserted on document order rather than on a CSS property, because document order *is* tab order
   * for a link with no positive tabindex.
   */
  it("is the first focusable element in the document", () => {
    const { container } = renderShell();
    const focusable = container.querySelectorAll<HTMLElement>(
      "a[href], button, input, select, textarea, summary, [tabindex]:not([tabindex='-1'])",
    );
    expect(focusable.length).toBeGreaterThan(1);
    expect(focusable[0].classList.contains("skip-link")).toBe(true);
  });

  it("is outside the navigation landmark, so it is not read as a destination", () => {
    renderShell();
    const link = screen.getByRole("link", { name: /skip to content/i });
    expect(within(nav()).queryByRole("link", { name: /skip to content/i })).toBeNull();
  });

  it("points at the main landmark it claims to skip to", () => {
    renderShell();
    const link = screen.getByRole("link", { name: /skip to content/i });
    const target = link.getAttribute("href")?.replace("#", "");
    expect(target).toBeTruthy();
    expect(document.querySelector(`#${target}`)?.tagName.toLowerCase()).toBe("main");
  });
});

describe("route changes are announced", () => {
  /*
   * The whole panel swaps silently on navigation, so a screen-reader user pressing a nav button got no
   * confirmation of where they went. The `<h1>` is the one element that changes on every route, which
   * makes it the right live region -- and `polite` rather than `assertive`, because a navigation is not
   * an error and should not interrupt whatever is being read.
   */
  it("marks the title as a polite live region", () => {
    const { container } = renderShell();
    const title = container.querySelector("h1");
    expect(title?.getAttribute("aria-live")).toBe("polite");
  });

  it("keeps the live region on the element that actually changes", () => {
    const { container } = renderShell();
    // The text is read *before* the click, because `querySelector` returns a live reference and
    // comparing the same node's textContent to itself would pass for any value.
    const before = container.querySelector("h1")?.textContent;
    fireEvent.click(screen.getByRole("button", { name: /arena/i }));
    const after = container.querySelector("h1");
    expect(after?.getAttribute("aria-live")).toBe("polite");
    expect(after?.textContent).not.toBe(before);
  });

});

describe("the nav list is not a list of nothing", () => {
  /*
   * `role="list"` on a container of `<button>` elements announces an empty list, because ARIA requires
   * a list to contain `listitem`s. These are navigation links; the buttons carry their own semantics
   * and `aria-current` already says which is active.
   */
  it("does not claim to be a list", () => {
    const { container } = renderShell();
    const items = container.querySelector(".nav-items");
    expect(items?.getAttribute("role")).toBeNull();
  });

  it("marks exactly one destination as the current page", () => {
    renderShell();
    const current = screen.getAllByRole("button").filter(
      (button) => button.getAttribute("aria-current") === "page",
    );
    expect(current).toHaveLength(1);
  });
});
