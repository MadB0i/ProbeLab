/**
 * Application shell: the persistent sidebar and the routed content area.
 *
 * # Why a hash router and not a routing library
 *
 * The application is a Tauri desktop app with eight or nine destinations and no server. `react-router`
 * would add a dependency, a history listener, and a build of nested route config to solve a problem
 * that a `location.hash` and one `useState` solve completely — and it would need the dependency added
 * at the moment the product most needs to stay small. Hash routing also deep-links: a user can paste
 * `#/arena` into a terminal argument and land on the arena, which matters for a tool people script.
 *
 * # Why every item is real
 *
 * Each destination renders a panel that exists. There is no "coming soon" entry, because a disabled
 * nav item is a promise the product cannot keep and a reader has to evaluate: is this missing, or is
 * this broken? The unavailable areas are documented in the README and the feature matrix instead,
 * which is where someone looks when they want to know what the product does not do.
 *
 * # Keyboard and assistive behaviour
 *
 * The nav is a real list of buttons inside a labelled landmark, so it is reachable by keyboard and
 * announced as navigation rather than as a pile of links. Arrow keys move between items, Home and End
 * jump to the ends, and the current item carries `aria-current="page"`. Focus is never trapped: Tab
 * moves on, which is the behaviour someone using a keyboard expects from a sidebar.
 */

import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";

import { BrandLockup } from "./Brand";
import { IconPanelLeft, ROUTE_ICONS } from "./Icons";

/** One destination in the command center. */
export type RouteId =
  | "readiness"
  | "lab"
  | "agents"
  | "benchmarks"
  | "tasks"
  | "comparison"
  | "settings"
  | "providers"
  | "arena"
  | "security"
  | "replay";

/** Everything the shell needs to know about a destination. */
export type RouteDefinition = {
  id: RouteId;
  /** Short label for the sidebar. */
  label: string;
  /** Longer name for the page heading. */
  title: string;
  /** What the page is for, shown under the heading. */
  purpose: string;
  /** Rendered in the content area. */
  content: ReactNode;
};

/**
 * Rail groups.
 *
 * Eleven destinations in one undifferentiated column is a list, not a structure. A user cannot
 * tell that `agents` and `benchmarks` are the same kind of thing, or that `comparison` and `replay`
 * both read from stored runs rather than producing any. The groups state that relationship instead
 * of leaving it to be inferred from the labels.
 *
 * `RAIL` is the flattened group order, and it is what the rail renders and what the arrow keys walk.
 * `ROUTES` above stays the lookup table for a destination's title and purpose, so a route can be
 * declared in either order without the keyboard order depending on where it happens to sit in the
 * source. The reading order is: check what can run, run it, look at what exists, then look at what
 * was measured. `settings` sits alone at the foot of the rail because it is the one destination a
 * user visits deliberately rather than as part of a task.
 */
const GROUPS: { title: string; ids: RouteId[] }[] = [
  { title: "Run", ids: ["readiness", "lab", "tasks"] },
  { title: "Registry", ids: ["agents", "benchmarks", "providers"] },
  { title: "Analysis", ids: ["comparison", "arena", "security", "replay"] },
  { title: "System", ids: ["settings"] },
];

const RAIL: RouteId[] = GROUPS.flatMap((group) => group.ids);

const ROUTES: { id: RouteId; label: string; title: string; purpose: string }[] = [
  {
    id: "readiness",
    label: "READY",
    title: "Readiness",
    purpose:
      "What this machine can actually run, verified just now. Offline runtimes, coding agents, guided packages, and optional remote providers — with the reason whenever something cannot run.",
  },
  {
    id: "lab",
    label: "LAB",
    title: "Command Center",
    purpose:
      "The orchestrator's own state, the experiment it is supervising, and the run's event stream. This is the panel that reports; it cannot steer a security decision.",
  },
  {
    id: "agents",
    label: "AGENTS",
    title: "Agents",
    purpose:
      "Every agent the lab can build, from the authoritative registry. Each entry states what its number measures — a model capability, a lab baseline, or a boundary probe — because those are not interchangeable.",
  },
  {
    id: "benchmarks",
    label: "BENCHMARKS",
    title: "Benchmarks",
    purpose:
      "Every registered benchmark with its capability-execution mode stated in the row. A benchmark that authorizes a capability without executing it says so here rather than looking like a working feature.",
  },
  {
    id: "tasks",
    label: "TASKS",
    title: "Task Benchmarks",
    purpose:
      "Coding repair and cue memory over seed pools: reference ceilings, installed-agent runs, and guided submissions — each with its provenance stated above its score.",
  },
  {
    id: "comparison",
    label: "COMPARE",
    title: "Comparison",
    purpose:
      "Stored experiments, compared. Two runs are only ranked when they measured the same thing; where that does not hold, the panel says which dimension differs instead of putting a number on it.",
  },
  {
    id: "settings",
    label: "SETTINGS",
    title: "Settings",
    purpose:
      "The defaults future experiments and runs start from. Every value here is validated and stored by the backend; read-only facts are shown with the reason they cannot be changed.",
  },
  {
    id: "providers",
    label: "PROVIDERS",
    title: "Providers",
    purpose:
      "The provider protocols the lab genuinely speaks. Credentials are read from the environment by the backend and never reach this page.",
  },
  {
    id: "arena",
    label: "ARENA",
    title: "Arena",
    purpose:
      "Real matches through the engine. The board shown is the position the engine recorded, not a local simulation, and there is no human seat.",
  },
  {
    id: "security",
    label: "SECURITY",
    title: "Security Lab",
    purpose:
      "Registered security probes run through the real broker and sandbox path. A caught attempt is the lab working, not a vulnerability.",
  },
  {
    id: "replay",
    label: "REPLAY",
    title: "Replay",
    purpose:
      "Deterministic reconstruction of recorded runs. Replay does not re-run a match or contact a provider, and it says which of those a given recording supports.",
  },
];

/**
 * Parses a location hash into a route.
 *
 * The trim happens before the prefix is stripped, not after. A hash pasted with a stray space --
 * ` #/arena` -- would otherwise keep the leading space, fail to match the `#/` prefix, and leave the
 * route as the literal string `" #/arena"`, which matches nothing and silently lands the user on the
 * command center. A deep link that quietly opens the wrong page is worse than one that visibly
 * fails, so an unrecognised value routes to the known default instead of rendering nothing.
 */
export function route_from_hash(hash: string): RouteId {
  const candidate = hash.trim().replace(/^#\/?/, "").trim().toLowerCase();
  return ROUTES.some((route) => route.id === candidate)
    ? (candidate as RouteId)
    : "lab";
}

/** The hash a route is addressed by. */
export function hash_for(route: RouteId): string {
  return `#/${route}`;
}

/** The rail group a destination sits in, so the band can name the neighbourhood of the page. */
function group_title(id: RouteId): string {
  return GROUPS.find((group) => group.ids.includes(id))?.title ?? "";
}

export function AppShell({
  routes,
  systemStatus,
  systemTone,
}: {
  routes: Record<RouteId, ReactNode>;
  /**
   * The orchestrator's own read, shown in the toolbar.
   *
   * It lives here rather than being fetched by the shell because the shell has no business making
   * backend calls -- it is the frame. `App` already polls this, and passing it down keeps one owner
   * for the value so the toolbar and the Lab's own status can never disagree.
   */
  systemStatus?: string;
  /** A token name from the stylesheet, not a colour: the shell must not own the palette. */
  systemTone?: string;
}) {
  const [active, setActive] = useState<RouteId>(() =>
    route_from_hash(typeof window === "undefined" ? "" : window.location.hash),
  );
  const navRef = useRef<HTMLDivElement>(null);

  // Follow the hash in both directions, so a pasted deep link works and the back button does too.
  useEffect(() => {
    const onHashChange = () => setActive(route_from_hash(window.location.hash));
    window.addEventListener("hashchange", onHashChange);
    return () => window.removeEventListener("hashchange", onHashChange);
  }, []);

  const go = useCallback((route: RouteId) => {
    window.location.hash = hash_for(route);
    setActive(route);
  }, []);

  /**
   * Roving arrow-key navigation.
   *
   * Not a focus trap: Tab still leaves the sidebar, which is what a keyboard user expects from a
   * navigation region. Arrow keys move within it because an eleven-item column is tedious to Tab
   * through and the convention is well established.
   *
   * The walk is over `RAIL`, not over `ROUTES`, so it follows the grouped order the rail actually
   * renders. Walking the declaration order instead would jump across the screen while the focus ring
   * visibly moved down one row -- the keyboard and the page would disagree about what "next" means.
   *
   * The index comes from the focused button, not from the active route. Those are usually the same
   * item, but a user can Tab onto a button without activating it -- clicking, after all, is not the
   * only way to focus. Deriving the index from the active route would then make the next arrow press
   * move relative to a different item than the one under the user's cursor and the key they pressed,
   * which is the kind of small wrongness that makes a keyboard user conclude the page is broken.
   */
  const onKeyDown = useCallback(
    (event: React.KeyboardEvent<HTMLDivElement>) => {
      if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) {
        return;
      }
      event.preventDefault();
      const buttons = navRef.current?.querySelectorAll<HTMLButtonElement>(".nav-item");
      const focused = (event.target as HTMLElement | null)?.closest<HTMLButtonElement>(".nav-item");
      const focusedIndex = focused ? Array.from(buttons ?? []).indexOf(focused) : -1;
      const index = focusedIndex >= 0 ? focusedIndex : RAIL.indexOf(active);
      const next =
        event.key === "Home"
          ? 0
          : event.key === "End"
            ? RAIL.length - 1
            : event.key === "ArrowDown"
              ? (index + 1) % RAIL.length
              : (index - 1 + RAIL.length) % RAIL.length;
      go(RAIL[next]);
      buttons?.[next]?.focus();
    },
    [active, go],
  );

  const definition = ROUTES.find((route) => route.id === active) ?? ROUTES[0];

  /*
   * The rail's expanded state, open by default.
   *
   * Collapsed it is a 76px icon dock; expanded it is a labelled sidebar. The expanded state is the
   * default because eleven icons in a column is a memory test: the labels are what tell a first-time
   * reader that `COMPARE` and `REPLAY` both read stored runs rather than produce any. Collapsing is
   * a deliberate act by someone who already knows the rail and wants the width back, not the price
   * everyone pays for having a rail at all.
   *
   * It is local component state rather than a route or a stored preference, because it is a view
   * choice for this session, not a setting. `aria-expanded` on the toggle is what tells a screen
   * reader that the labels are visible.
   */
  const [railExpanded, setRailExpanded] = useState(true);

  return (
    <div className={`shell-frame${railExpanded ? " is-expanded" : ""}`}>
      {/*
        The skip link is the FIRST focusable node in the document, not a child of `<main>`.

        It used to sit inside `<main>` after the header, with `<nav>` before `<main>` in DOM order --
        so tab order was nine sidebar buttons, then the skip link, then the content. A control whose
        entire purpose is bypassing the sidebar was reachable only after tabbing through the sidebar,
        which is the opposite of the point and impossible to notice by using the mouse.

        It is also outside `<nav>`, so it is not part of the navigation landmark and a screen reader
        walking landmarks will not read it as a destination.
      */}
      <a className="skip-link" href="#main-content">
        Skip to content
      </a>

      {/*
        The rail is a wrapper, not the `<nav>`.

        The expand toggle and the brand are frame chrome: they change how the rail is drawn and they
        name the application. Neither is a destination, so both sit *outside* the navigation landmark.
        Putting the toggle inside `<nav>` would make it announce as a fourth kind of thing to choose
        between when it only resizes the thing it lives in -- and it would put a non-destination button
        into the same list as the eleven real ones.
      */}
      <div className="shell-rail">
        {/*
          The brand lockup is decorative chrome here, and is hidden from assistive technology.

          It sits outside `<nav>` on purpose -- it is not a destination, and a non-interactive logo in a
          navigation landmark announces as a thing you can choose. But that left it outside *every*
          landmark, which axe reports as `region`: "all page content should be contained by landmarks".
          Text that is not in a landmark cannot be reached by landmark navigation at all.

          Hiding it resolves that without moving it into `<nav>` and without restructuring the rail's
          landmarks, which is what the alternative would have cost. Nothing is lost: the application
          name is the document title, and the shell's landmarks -- navigation, main, banner, status --
          describe the app far better than a repeated wordmark does. The lockup is still rendered, so
          the sighted rail looks identical.

          This is scoped to this call site rather than to `BrandLockup` itself, so a future use in a
          content position -- an about panel, say -- stays readable.
        */}
        <div className="nav-brand" aria-hidden="true">
          <BrandLockup />
        </div>
        <button
          type="button"
          className="rail-toggle"
          aria-expanded={railExpanded}
          aria-label={railExpanded ? "Collapse navigation" : "Expand navigation"}
          onClick={() => setRailExpanded((value) => !value)}
        >
          <IconPanelLeft size={18} />
        </button>

        <nav className="shell-nav" aria-label="Command center sections">
          {/*
            No `role="list"`. The children are `<button>` elements, and ARIA requires a `list` to contain
            `listitem`s -- so the role announced an empty list. These are navigation links, so the
            container is a plain group of controls and the buttons carry their own semantics. `aria-current`
            is what tells a screen reader which destination is active.

            The group titles are `<p>`, not headings: a heading inside a `nav` landmark is announced as
            a section of the page, and these are grouping aids for the sighted reader scanning the rail,
            not part of the navigation structure a screen reader needs to walk. They are hidden while the
            rail is collapsed, where there is no room for them.
          */}
          <div className="nav-items" ref={navRef} onKeyDown={onKeyDown}>
            {GROUPS.map((group) => (
              <div className="nav-group" key={group.title}>
                <p className="nav-group-title">{group.title}</p>
                {group.ids.map((id) => {
                  const Glyph = ROUTE_ICONS[id];
                  const label = ROUTES.find((r) => r.id === id)?.label ?? id;
                  return (
                    <button
                      key={id}
                      type="button"
                      className={`nav-item${id === active ? " nav-item--active" : ""}`}
                      aria-current={id === active ? "page" : undefined}
                      onClick={() => go(id)}
                    >
                      {/*
                        One text node, two jobs.

                        Collapsed, the rail is icons only and this label becomes the hover tooltip.
                        Expanded, it is the sidebar text. It was previously written twice -- once as
                        the label and once inside a separate tooltip span -- which put the
                        destination name into the DOM twice and left the tooltip free to drift out of
                        sync with the real label. One span, repositioned by CSS, cannot disagree with
                        itself, and it keeps the accessible name correct in both states because the
                        text is never removed.
                      */}
                      <span className="nav-item-icon" aria-hidden="true">
                        <Glyph size={20} />
                      </span>
                      <span className="nav-item-label">{label}</span>
                    </button>
                  );
                })}
              </div>
            ))}
          </div>
          <p className="nav-note">
            Every section opens real data. Unavailable areas are documented in the README and the feature
            matrix rather than shown as empty pages.
          </p>
        </nav>
      </div>

      <main className="shell-main" id="main-content">
        {/*
          The toolbar.

          On a desktop application the page title lives in a toolbar with the window's global state and
          actions beside it -- not floating at the top of a scrolling document. That single move is
          most of the difference between an application and a web page, so the title, the live system
          read, and the global actions share one fixed band.

          `aria-live` stays on the title, because it is the one element that changes on every route and
          a route change must still be announced. `polite` rather than `assertive`: navigating is not an
          error and should not interrupt.
        */}
        <header className="topbar">
          <div className="shell-header">
            <span className="shell-index">{group_title(active)}</span>
            <h1 className="shell-title" aria-live="polite">
              {definition.title}
            </h1>
            <p className="shell-purpose">{definition.purpose}</p>
          </div>
          <div className="system-status" aria-live="polite">
            <span className={`status-light ${systemTone ?? ""}`} aria-hidden="true" />
            <span>SYSTEM {systemStatus ?? "—"}</span>
            <span className="status-divider" aria-hidden="true" />
            <span className="muted">LOCAL RUNTIME</span>
          </div>
        </header>
        {/*
          The scrolling content region.

          `tabIndex` is not decoration here. Once a route's content outgrew the viewport, the region
          became a scroll container, and a scroll container that holds focusable content has to be
          focusable itself: a keyboard user who cannot put focus in it has no way to scroll it with the
          arrow keys once the tab sequence has moved past the last control inside. axe caught this on
          every route the new audit reached, reported against the element that owns the scroll.

          The label is deliberately not a copy of the heading. It sits inside a `<main>` that already
          carries the route's title, and a landmark whose name duplicates its own heading adds a second
          stop to a screen reader's landmark list saying the same thing twice.
        */}
        <div
          className="shell-content"
          role="region"
          aria-label={`${definition.title} content`}
          tabIndex={0}
        >
          {routes[definition.id]}
        </div>
        {/*
          The status bar: where the application says where you are without asking you to look up.

          It is a plain `div`, not a live region and not a `footer` landmark. The route change is
          already announced by the `<h1>` above, so announcing it a second time from the bar would
          make a screen reader read the page name twice on every navigation; and a `footer` nested
          inside `<main>` is not a `contentinfo` landmark anyway, so the role would promise something
          the DOM cannot deliver.
        */}
        <div className="statusbar">
          <span className="statusbar-item is-key">
            <span className={`status-light ${systemTone ?? ""}`} aria-hidden="true" />
            {systemStatus ?? "—"}
          </span>
          <span className="statusbar-sep" aria-hidden="true" />
          <span className="statusbar-item">{group_title(active)}</span>
          <span className="statusbar-spacer" />
          {/*
            The scope claims used to sit in a footer under the workspace, which made them the last
            thing on a scrolling page rather than the frame's own statement -- and it put a second
            footer inside the application when the status bar already is one. They live here now,
            where the frame keeps them on screen at every width and every scroll position.
          */}
          <span className="statusbar-item statusbar-optional">NO AUTH · NO CLOUD</span>
          <span className="statusbar-sep" aria-hidden="true" />
          <span className="statusbar-item statusbar-optional">DESKTOP VERTICAL SLICE</span>
          <span className="statusbar-sep" aria-hidden="true" />
          <span className="statusbar-item">{definition.label}</span>
        </div>
      </main>
    </div>
  );
}
