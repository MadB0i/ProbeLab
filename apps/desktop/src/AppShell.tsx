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

/** One destination in the command center. */
export type RouteId =
  | "lab"
  | "agents"
  | "benchmarks"
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

const ROUTES: { id: RouteId; label: string; title: string; purpose: string }[] = [
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

export function AppShell({ routes }: { routes: Record<RouteId, ReactNode> }) {
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
   * navigation region. Arrow keys move within it because a seven-item column is tedious to Tab
   * through and the convention is well established.
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
      const index = focusedIndex >= 0 ? focusedIndex : ROUTES.findIndex((route) => route.id === active);
      const next =
        event.key === "Home"
          ? 0
          : event.key === "End"
            ? ROUTES.length - 1
            : event.key === "ArrowDown"
              ? (index + 1) % ROUTES.length
              : (index - 1 + ROUTES.length) % ROUTES.length;
      go(ROUTES[next].id);
      buttons?.[next]?.focus();
    },
    [active, go],
  );

  const definition = ROUTES.find((route) => route.id === active) ?? ROUTES[0];

  return (
    <div className="shell-frame">
      <nav className="shell-nav" aria-label="Command center sections">
        <div className="nav-brand">
          <span className="nav-brand-mark">PROBE//LAB</span>
          <span className="nav-brand-sub">EVALUATION LABORATORY</span>
        </div>
        <div className="nav-items" ref={navRef} onKeyDown={onKeyDown} role="list">
          {ROUTES.map((route) => (
            <button
              key={route.id}
              type="button"
              className={`nav-item${route.id === active ? " nav-item--active" : ""}`}
              aria-current={route.id === active ? "page" : undefined}
              onClick={() => go(route.id)}
            >
              <span className="nav-item-mark" aria-hidden="true" />
              {route.label}
            </button>
          ))}
        </div>
        <p className="nav-note">
          Every section opens real data. Unavailable areas are documented in the README and the feature
          matrix rather than shown as empty pages.
        </p>
      </nav>

      <main className="shell-main" id="main-content">
        <header className="shell-header">
          <h1 className="shell-title">{definition.title}</h1>
          <p className="shell-purpose">{definition.purpose}</p>
        </header>
        {/* A skip link, so a keyboard user can bypass the sidebar on every page. */}
        <a className="skip-link" href="#main-content">
          Skip to content
        </a>
        <div className="shell-content">{routes[definition.id]}</div>
      </main>
    </div>
  );
}
