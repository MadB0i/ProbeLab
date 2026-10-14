/**
 * Accessibility and motion audit of every registered route.
 *
 * # Why every route, and not two
 *
 * The first version of this script scanned `#/lab` and `#/arena` — the two routes a phase had touched.
 * Every later audit inherited that scope, and nine routes went unmeasured for the life of the project.
 * One of them turned out to have twenty-seven serious contrast violations on it. The list is now read
 * out of `AppShell.tsx` rather than written here, so a new route is audited the day it is registered
 * instead of the day someone remembers.
 *
 * # What it checks, and why axe alone is not enough
 *
 * It injects the real axe-core into the running application and scans each route, then adds four things
 * axe does not do:
 *
 * * **Focus visibility**, over every focusable control rather than the handful axe samples. A control with
 *   `outline: none` and no replacement ring is invisible to a keyboard user and passes every automated
 *   rule there is.
 * * **Hidden focus traps**: an `aria-hidden` ancestor containing a focusable element is a control a
 *   screen-reader user cannot reach and a keyboard user can land on. The Arena once hid the wrapper that
 *   contained its board, which is how this class of defect got its first measurement.
 * * **Names on interactive elements**: a button whose accessible name is empty is unreachable by name,
 *   and nothing else fails it.
 * * **Overflow and fold geometry**: a document that scrolls sideways, or a panel whose subject starts
 *   below the visible content area, is a layout defect that no accessibility rule covers.
 *
 * Reduced motion is read from computed styles rather than assumed: with the preference set, the stage's
 * animations and transitions must actually be clamped, not merely accompanied by a media query that
 * happens to exist.
 *
 * Usage: node scripts/audit-a11y.mjs <port> [axePath] [repoRoot]
 */
import { readFileSync } from "node:fs";
import { resolve as resolvePath } from "node:path";

const port = process.argv[2] ?? "9336";
const axePath =
  process.argv[3] ?? `${process.env.TEMP ?? "."}\\probelab-axe\\node_modules\\axe-core\\axe.min.js`;
const repoRoot = process.argv[4] ?? "D:\\Projects\\ProbeLab";
const axeSource = readFileSync(axePath, "utf8");

/**
 * The registered routes, read out of the shell's own type.
 *
 * Parsed rather than listed, because a hand-written list is a list that goes stale exactly the way the
 * original two-route script did.
 */
function registeredRoutes() {
  const shell = readFileSync(resolvePath(repoRoot, "apps/desktop/src/AppShell.tsx"), "utf8");
  const block = shell.match(/export type RouteId =([\s\S]*?);/);
  if (!block) throw new Error("RouteId is no longer declared in AppShell.tsx");
  return [...block[1].matchAll(/"([a-z]+)"/g)].map((m) => m[1]);
}

const ROUTES = registeredRoutes();
console.log(`routes under audit (${ROUTES.length}): ${ROUTES.join(", ")}`);

const targets = await fetch(`http://127.0.0.1:${port}/json/list`).then((r) => r.json());
const page = targets.find((t) => t.type === "page" && t.webSocketDebuggerUrl);
const socket = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  socket.addEventListener("open", resolve, { once: true });
  socket.addEventListener("error", reject, { once: true });
});

let id = 0;
const pending = new Map();
socket.addEventListener("message", (event) => {
  const message = JSON.parse(event.data);
  if (pending.has(message.id)) {
    pending.get(message.id)(message);
    pending.delete(message.id);
  }
});
const send = (method, params = {}) => {
  const messageId = ++id;
  socket.send(JSON.stringify({ id: messageId, method, params }));
  return new Promise((resolve) => pending.set(messageId, resolve));
};
async function evaluate(expression) {
  const response = await send("Runtime.evaluate", {
    expression,
    returnByValue: true,
    awaitPromise: true,
  });
  if (response.result?.exceptionDetails) {
    throw new Error(response.result.exceptionDetails.exception?.description ?? "evaluate failed");
  }
  return response.result?.result?.value;
}
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

await send("Runtime.enable");
await send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-reduced-motion", value: "reduce" }] });

const failures = [];
const check = (name, ok, detail = "") => {
  console.log(`${ok ? "  PASS" : "  FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
  if (!ok) failures.push(name);
};

/** Inject axe once; it survives route changes because the page does. */
const injected = await evaluate(`(() => {
  if (window.axe) return true;
  const script = document.createElement("script");
  script.textContent = ${JSON.stringify(axeSource)};
  document.head.appendChild(script);
  return typeof window.axe !== "undefined";
})()`);
check("axe-core injected", injected === true);

for (const route of ROUTES) {
  console.log(`\n=== #/${route} ===`);
  await evaluate(`location.hash = "#/${route}"; true`);
  await wait(2400);

  const violations = JSON.parse(await evaluate(`(async () => {
    const result = await window.axe.run(document, {
      runOnly: { type: "tag", values: ["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"] },
    });
    return JSON.stringify(result.violations.map((v) => ({
      id: v.id,
      impact: v.impact,
      count: v.nodes.length,
      // Grouped by the element's own class, so a failure names the rule that owns the defect instead of
      // listing forty near-identical <dt>s.
      classes: Array.from(new Set(v.nodes.map((n) => (n.html.match(/class="([^"]*)"/) ?? [, "<no class>"])[1]))).slice(0, 8),
      // The measured pair, not just the count. A contrast failure naming "8 nodes" tells the reader
      // nothing they can act on; "var(--ink-3) #7c8a9e on #202939 at 4.16:1, needs 4.5:1" tells them
      // which token to lift and by how much.
      detail: Array.from(new Set(v.nodes.map((n) => {
        const data = n.any?.[0]?.data;
        if (!data || data.contrastRatio === undefined) return null;
        const fg = (data.fgColor || "").replace(/\\s+/g, " ").slice(0, 40);
        const bg = (data.bgColor || "").replace(/\\s+/g, " ").slice(0, 40);
        return data.contrastRatio + ":1 (" + fg + " on " + bg + ", needs " + data.expectedContrastRatio + ":1)";
      }).filter(Boolean))).slice(0, 8),
      examples: Array.from(new Set(v.nodes.map((n) => n.html.slice(0, 90)))).slice(0, 5),
    })));
  })()`));
  check(
    `#/${route}: no axe violations`,
    violations.length === 0,
    violations
      .map((v) => `${v.id} (${v.impact}, ${v.count}) on ${v.classes.join(", ")}${v.detail.length ? "\n         " + v.detail.join("\n         ") : ""}\n         ${v.examples.join("\n         ")}`)
      .join("\n       | "),
  );

  const traversal = JSON.parse(await evaluate(`(() => {
    const focusable = Array.from(document.querySelectorAll(
      'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
    )).filter((el) => el.offsetParent !== null || el.getClientRects().length > 0);
    /* A control with no accessible name cannot be reached by name, and nothing else fails it.
       This has to compute the name the way the platform does, not the way it is convenient to: an
       explicit label association and a wrapping label are both names. An earlier version of this
       check looked only at aria-label and text content, and reported sixteen labelled inputs on five
       routes as nameless — a checker that cries wolf gets turned off, so it is worse than no checker. */
    const nameOf = (el) => {
      const aria = (el.getAttribute("aria-label") || "").trim();
      if (aria) return aria;
      const labelledBy = (el.getAttribute("aria-labelledby") || "")
        .split(/\\s+/)
        .filter(Boolean)
        .map((id) => document.getElementById(id)?.textContent?.trim() ?? "")
        .filter(Boolean)
        .join(" ");
      if (labelledBy) return labelledBy;
      if (el.labels && el.labels.length > 0) {
        const fromLabels = Array.from(el.labels).map((l) => l.textContent?.trim() ?? "").filter(Boolean).join(" ");
        if (fromLabels) return fromLabels;
      }
      const wrapping = el.closest("label");
      if (wrapping && wrapping.textContent?.trim()) return wrapping.textContent.trim();
      const title = (el.getAttribute("title") || "").trim();
      if (title) return title;
      const own = (el.textContent || "").trim();
      if (own) return own;
      return (el.getAttribute("placeholder") || "").trim();
    };
    const rings = focusable.map((el) => {
      el.focus();
      const style = getComputedStyle(el);
      const hasOutline = style.outlineStyle !== "none" && parseFloat(style.outlineWidth) > 0;
      const hasShadow = style.boxShadow && style.boxShadow !== "none";
      return { label: nameOf(el).slice(0, 40) || el.tagName, ring: hasOutline || hasShadow };
    });
    const hidden = focusable.filter((el) => {
      let node = el;
      while (node && node !== document.body) {
        if (node.getAttribute && node.getAttribute("aria-hidden") === "true") return true;
        node = node.parentElement;
      }
      return false;
    }).map((el) => (nameOf(el) || el.tagName).slice(0, 40));
    const unnamed = focusable.filter((el) => nameOf(el).length === 0).map((el) => el.tagName);
    const scroller = document.querySelector(".shell-content");
    const view = scroller ? scroller.getBoundingClientRect() : { top: 0, bottom: window.innerHeight };
    /* Geometry, checked as clipping rather than as position.
       A panel that extends below the fold is ordinary: the content area scrolls. A panel whose bottom
       is cut off by an ancestor that cannot scroll is a defect, because its tail is unreachable by any
       means. The first version of this check flagged the former and so failed on nine of eleven
       routes for the crime of having content, which is how a check earns being ignored. */
    const clipped = Array.from(document.querySelectorAll(".panel, section.panel"))
      .filter((el) => {
        let node = el.parentElement;
        while (node && node !== document.body) {
          const style = getComputedStyle(node);
          if (style.overflowY === "hidden" || style.overflowY === "clip") {
            const box = node.getBoundingClientRect();
            return el.getBoundingClientRect().bottom > box.bottom + 1;
          }
          node = node.parentElement;
        }
        return false;
      })
      .map((el) => el.className.split(" ")[0]);
    /* Name the clipping ancestor, not just the victim: "panel inside .library" is a fix, "panel" is a
       mystery. Reachability is decided by walking outward and stopping at the first ancestor that can
       scroll: once a scroll container is passed, the panel's tail is reachable by scrolling and the
       layout is fine however tall the page gets. Only a panel cut off by an ancestor that cannot
       scroll is genuinely unreachable. The earlier version ignored scroll containers and reported the
       agents, benchmarks and comparison routes as clipped when all three simply have more content
       than fits. */
    const clipping = [];
    for (const el of document.querySelectorAll(".panel, section.panel")) {
      let node = el.parentElement;
      while (node && node !== document.body) {
        const style = getComputedStyle(node);
        const overflowY = style.overflowY;
        if (overflowY === "auto" || overflowY === "scroll") break;
        if (overflowY === "hidden" || overflowY === "clip") {
          const box = node.getBoundingClientRect();
          if (el.getBoundingClientRect().bottom > box.bottom + 1) {
            clipping.push(\`\${el.className.split(" ")[0]} in .\${node.className.split(" ")[0]}\`);
          }
          break;
        }
        node = node.parentElement;
      }
    }
    document.activeElement?.blur();
    return JSON.stringify({
      total: rings.length,
      withoutRing: rings.filter((r) => !r.ring).map((r) => r.label),
      hiddenFocusable: hidden,
      unnamed,
      overflowX: Math.max(0, document.documentElement.scrollWidth - document.documentElement.clientWidth),
      clipped: Array.from(new Set(clipping)),
      view: { top: Math.round(view.top), bottom: Math.round(view.bottom) },
    });
  })()`));
  check(`#/${route}: every focusable control shows a focus ring`,
    traversal.withoutRing.length === 0, traversal.withoutRing.join(", "));
  check(`#/${route}: no focusable control inside an aria-hidden ancestor`,
    traversal.hiddenFocusable.length === 0, traversal.hiddenFocusable.join(", "));
  check(`#/${route}: every focusable control has an accessible name`,
    traversal.unnamed.length === 0, traversal.unnamed.join(", "));
  check(`#/${route}: nothing scrolls sideways`, traversal.overflowX === 0, `${traversal.overflowX}px`);
  check(`#/${route}: no panel is clipped by an unscrollable ancestor`,
    traversal.clipped.length === 0, traversal.clipped.join(", "));
  console.log(`  ${traversal.total} focusable controls, content area ${traversal.view.top}..${traversal.view.bottom}`);
}

console.log("\n=== reduced motion ===");
/*
 * Measured with a probe rather than by reading the grid stage.
 *
 * The stage selectors are only present when a recorded run is loaded, so on a machine with no runs in
 * the database they report "not present" — which the previous version of this check counted as a pass.
 * That is a check that cannot fail. Instead the audit injects an element carrying a one-second
 * animation and a one-second transition and reads what the cascade did to it, which works on every
 * route and every database.
 *
 * And it measures both directions. With the preference forced on the probe must come back clamped; with
 * the preference forced off it must come back at its full second. Reading only the clamped case would
 * pass just as happily against a stylesheet where the media query was empty and nothing animated at all,
 * which is the failure this is here to catch.
 */
const probe = `(() => {
  const el = document.createElement("div");
  el.style.cssText = "position:absolute;opacity:0;pointer-events:none;animation:probe-spin 1s linear infinite;transition:color 1s ease;";
  document.body.appendChild(el);
  const style = getComputedStyle(el);
  const out = {
    animationDuration: style.animationDuration,
    animationIterationCount: style.animationIterationCount,
    transitionDuration: style.transitionDuration,
    scrollBehavior: getComputedStyle(document.documentElement).scrollBehavior,
    prefersReducedMotion: window.matchMedia("(prefers-reduced-motion: reduce)").matches,
  };
  el.remove();
  return JSON.stringify(out);
})()`;

await send("Emulation.setEmulatedMedia", { features: [] });
const motionOff = JSON.parse(await evaluate(probe));
check("the page reports motion is allowed when the preference is off",
  motionOff.prefersReducedMotion === false);
check("animations are NOT clamped when the preference is off, or the media query is inert",
  motionOff.animationDuration.split(",").every((p) => parseFloat(p) >= 0.9),
  motionOff.animationDuration);
check("transitions are NOT clamped when the preference is off, or the media query is inert",
  motionOff.transitionDuration.split(",").every((p) => parseFloat(p) >= 0.9),
  motionOff.transitionDuration);

await send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-reduced-motion", value: "reduce" }] });
await wait(400);
const motion = JSON.parse(await evaluate(probe));
check("reduced motion is in effect for the page", motion.prefersReducedMotion === true);
check("animations are clamped under reduced motion",
  motion.animationDuration.split(",").every((p) => parseFloat(p) <= 0.01),
  motion.animationDuration);
check("animations run once under reduced motion, not forever",
  motion.animationIterationCount.split(",").every((p) => parseFloat(p) <= 1),
  motion.animationIterationCount);
check("transitions are clamped under reduced motion",
  motion.transitionDuration.split(",").every((p) => parseFloat(p) <= 0.01),
  motion.transitionDuration);
check("smooth scrolling is disabled under reduced motion",
  motion.scrollBehavior !== "smooth", motion.scrollBehavior);

/*
 * The grid stage specifically, when there is a recording to show it. Reported as present or absent
 * rather than folded into the pass/fail above, because its absence is a property of the database this
 * machine happens to have, not of the stylesheet. `grid-trace.mjs` measures the stage with the same
 * preference forced.
 */
await evaluate(`location.hash = "#/lab"; true`);
await wait(1800);
const stage = JSON.parse(await evaluate(`(() => {
  const read = (selector, property) => {
    const el = document.querySelector(selector);
    return el ? getComputedStyle(el)[property] : null;
  };
  return JSON.stringify({
    present: !!document.querySelector(".gw-grid, .gw-agent"),
    markerTransition: read(".gw-agent", "transitionDuration"),
    sweepAnimation: read(".gw-sweep", "animationDuration"),
    routeAnimation: read(".gw-route-line", "animationDuration"),
  });
})()`));
console.log(stage.present
  ? `  grid stage present; marker ${stage.markerTransition}, sweep ${stage.sweepAnimation}, route ${stage.routeAnimation}`
  : "  grid stage not rendered on this route (no recording loaded); clamp measured by probe above");

console.log("");
const summary = {
  routes: ROUTES.length,
  failures: failures.length,
};
console.log(`routes audited: ${summary.routes}; failures: ${summary.failures}`);
await send("Emulation.setEmulatedMedia", { features: [] });
if (failures.length) {
  console.error(`FAILED ${failures.length}: ${failures.join(", ")}`);
  process.exitCode = 1;
} else {
  console.log(`PASS  accessibility and motion audit across all ${ROUTES.length} routes`);
}
socket.close();