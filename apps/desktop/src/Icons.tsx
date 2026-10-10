/**
 * The icon set.
 *
 * WHY THESE ARE INLINED
 * The project ships React, React DOM and the Tauri API and nothing else. Adding an icon package to
 * draw fourteen glyphs would put a dependency and a build step into a product whose whole premise is
 * that it runs offline with no cloud -- and the package would still be tree-shaken down to the same
 * path data. So the set is written here, once, at a single 24px grid with a single stroke weight, and
 * everything inherits from that.
 *
 * WHY A SINGLE GRID MATTERS
 * Mixed icon styles are the fastest way to make an interface look assembled rather than designed. A
 * 20px outline glyph next to a 16px filled glyph reads as two products. Every path below is authored on
 * a 24x24 box with `currentColor`, 1.75 stroke, round caps and round joins, so they sit on the same
 * optical baseline and scale to any size without re-tuning.
 *
 * DECORATIVE BY DEFAULT
 * `Icons.tsx` renders `aria-hidden` and `focusable="false"`, because an icon is nearly always
 * decoration sitting beside a real text label or inside a button that already has an accessible name.
 * Where an icon is the *only* content of a control, that control must carry its own `aria-label` --
 * see the rail, which uses a tooltip for sighted users and keeps the text label in the DOM for
 * assistive technology.
 */

import type { SVGProps } from "react";

type IconProps = SVGProps<SVGSVGElement> & {
  /** Rendered size in px. The viewBox stays 24 so scaling never re-strokes. */
  size?: number;
};

function Icon({ size = 18, children, ...rest }: IconProps) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.75}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
      {...rest}
    >
      {children}
    </svg>
  );
}

/* -- Destinations ---------------------------------------------------------- */

/** A flask: the product is a laboratory. */
export function IconFlask(props: IconProps) {
  return (
    <Icon {...props}>
      <path d="M9 3h6" />
      <path d="M10 3v6.5L4.8 18a2 2 0 0 0 1.7 3h11a2 2 0 0 0 1.7-3L14 9.5V3" />
      <path d="M7.5 15h9" />
    </Icon>
  );
}

/** A gauge: readiness. */
export function IconGauge(props: IconProps) {
  return (
    <Icon {...props}>
      <path d="M4 18a8 8 0 1 1 16 0" />
      <path d="M12 14.5 15.5 10" />
      <circle cx="12" cy="15" r="1.4" />
    </Icon>
  );
}

/** Checked lines: task benchmarks. */
export function IconChecklist(props: IconProps) {
  return (
    <Icon {...props}>
      <path d="m3 6 2 2 3.5-3.5" />
      <path d="M3 13l2 2 3.5-3.5" />
      <path d="M12 6.5h9" />
      <path d="M12 13.5h9" />
      <path d="M12 20.5h5" />
    </Icon>
  );
}

/** A chip: agents and models. */
export function IconChip(props: IconProps) {
  return (
    <Icon {...props}>
      <rect x="7" y="7" width="10" height="10" rx="1.5" />
      <path d="M10 3v4M14 3v4M10 17v4M14 17v4M3 10h4M3 14h4M17 10h4M17 14h4" />
    </Icon>
  );
}

/** A grid: benchmarks. */
export function IconGrid(props: IconProps) {
  return (
    <Icon {...props}>
      <rect x="3.5" y="3.5" width="7" height="7" rx="1.5" />
      <rect x="13.5" y="3.5" width="7" height="7" rx="1.5" />
      <rect x="3.5" y="13.5" width="7" height="7" rx="1.5" />
      <rect x="13.5" y="13.5" width="7" height="7" rx="1.5" />
    </Icon>
  );
}

/** Two scales: comparison. */
export function IconScales(props: IconProps) {
  return (
    <Icon {...props}>
      <path d="M12 4v16" />
      <path d="M7 20h10" />
      <path d="M4 8h16" />
      <path d="M4 8 1.5 14a3 3 0 0 0 5 0L4 8Z" />
      <path d="M20 8l-2.5 6a3 3 0 0 0 5 0L20 8Z" />
    </Icon>
  );
}

/** A server: providers and runtimes. */
export function IconServer(props: IconProps) {
  return (
    <Icon {...props}>
      <rect x="3" y="4" width="18" height="6" rx="2" />
      <rect x="3" y="14" width="18" height="6" rx="2" />
      <path d="M7 7h.01M7 17h.01" />
    </Icon>
  );
}

/** A board with a piece: the arena. */
export function IconBoard(props: IconProps) {
  return (
    <Icon {...props}>
      <rect x="3.5" y="3.5" width="17" height="17" rx="2" />
      <path d="M3.5 9h17M3.5 15h17M9 3.5v17M15 3.5v17" />
    </Icon>
  );
}

/** A shield: security. */
export function IconShield(props: IconProps) {
  return (
    <Icon {...props}>
      <path d="M12 3 5 6v5.5c0 4.2 2.8 8 7 9.5 4.2-1.5 7-5.3 7-9.5V6l-7-3Z" />
      <path d="m9 12 2 2 4-4" />
    </Icon>
  );
}

/** Rewind: replay. */
export function IconReplay(props: IconProps) {
  return (
    <Icon {...props}>
      <path d="M3 10a9 9 0 1 1 2 6" />
      <path d="M3 4v6h6" />
      <path d="M12 8v4.5l3 1.8" />
    </Icon>
  );
}

/** Sliders: settings. */
export function IconSliders(props: IconProps) {
  return (
    <Icon {...props}>
      <path d="M4 6h10M18 6h2M4 12h4M12 12h8M4 18h10M18 18h2" />
      <circle cx="16" cy="6" r="2" />
      <circle cx="10" cy="12" r="2" />
      <circle cx="16" cy="18" r="2" />
    </Icon>
  );
}

/* -- Actions --------------------------------------------------------------- */

export function IconPlay(props: IconProps) {
  return (
    <Icon {...props}>
      <path d="M7 4.5v15l12-7.5-12-7.5Z" fill="currentColor" strokeWidth={1.25} />
    </Icon>
  );
}

export function IconStop(props: IconProps) {
  return (
    <Icon {...props}>
      <rect x="6" y="6" width="12" height="12" rx="2" fill="currentColor" strokeWidth={1.25} />
    </Icon>
  );
}

export function IconScan(props: IconProps) {
  return (
    <Icon {...props}>
      <path d="M4 8V6a2 2 0 0 1 2-2h2M16 4h2a2 2 0 0 1 2 2v2M20 16v2a2 2 0 0 1-2 2h-2M8 20H6a2 2 0 0 1-2-2v-2" />
      <path d="M4 12h16" />
    </Icon>
  );
}

export function IconDownload(props: IconProps) {
  return (
    <Icon {...props}>
      <path d="M12 3v11" />
      <path d="m8 11 4 4 4-4" />
      <path d="M4 19h16" />
    </Icon>
  );
}

export function IconSearch(props: IconProps) {
  return (
    <Icon {...props}>
      <circle cx="10.5" cy="10.5" r="6" />
      <path d="m15 15 4.5 4.5" />
    </Icon>
  );
}

export function IconRefresh(props: IconProps) {
  return (
    <Icon {...props}>
      <path d="M20 11a8 8 0 0 0-14-4.5L4 9" />
      <path d="M4 5v4h4" />
      <path d="M4 13a8 8 0 0 0 14 4.5L20 15" />
      <path d="M20 19v-4h-4" />
    </Icon>
  );
}

export function IconPlus(props: IconProps) {
  return (
    <Icon {...props}>
      <path d="M12 5v14M5 12h14" />
    </Icon>
  );
}

export function IconChevronDown(props: IconProps) {
  return (
    <Icon {...props}>
      <path d="m6 9 6 6 6-6" />
    </Icon>
  );
}

export function IconChevronRight(props: IconProps) {
  return (
    <Icon {...props}>
      <path d="m9 6 6 6-6 6" />
    </Icon>
  );
}

export function IconArrowRight(props: IconProps) {
  return (
    <Icon {...props}>
      <path d="M4 12h15" />
      <path d="m13 6 6 6-6 6" />
    </Icon>
  );
}

export function IconPanelLeft(props: IconProps) {
  return (
    <Icon {...props}>
      <rect x="3" y="4" width="18" height="16" rx="2" />
      <path d="M9.5 4v16" />
    </Icon>
  );
}

/* -- Status ---------------------------------------------------------------- */

export function IconCheck(props: IconProps) {
  return (
    <Icon {...props}>
      <path d="m5 12.5 4.5 4.5L19 7" />
    </Icon>
  );
}

export function IconAlert(props: IconProps) {
  return (
    <Icon {...props}>
      <path d="M12 4 2.5 20h19L12 4Z" />
      <path d="M12 10v4.5M12 17.5h.01" />
    </Icon>
  );
}

export function IconLock(props: IconProps) {
  return (
    <Icon {...props}>
      <rect x="4.5" y="10.5" width="15" height="10" rx="2" />
      <path d="M8 10.5V7.5a4 4 0 0 1 8 0v3" />
    </Icon>
  );
}

export function IconInfo(props: IconProps) {
  return (
    <Icon {...props}>
      <circle cx="12" cy="12" r="9" />
      <path d="M12 11v5.5M12 7.8h.01" />
    </Icon>
  );
}

export function IconSpark(props: IconProps) {
  return (
    <Icon {...props}>
      <path d="M12 3v4M12 17v4M3 12h4M17 12h4" />
      <path d="M12 8.5 13.4 11l2.6 1-2.6 1-1.4 2.5L10.6 13 8 12l2.6-1L12 8.5Z" fill="currentColor" strokeWidth={1} />
    </Icon>
  );
}

/** A brain-in-a-box mark: used for the app logo fallback and the empty state. */
export function IconMark(props: IconProps) {
  return (
    <Icon {...props} strokeWidth={1.5}>
      <rect x="3.5" y="3.5" width="17" height="17" rx="4" />
      <circle cx="12" cy="12" r="5.5" />
      <circle cx="12" cy="12" r="2" fill="currentColor" strokeWidth={1} />
    </Icon>
  );
}

/**
 * The icon bound to each destination.
 *
 * Keeping this next to the routes rather than inside each panel means the rail and any other
 * navigation surface cannot drift apart, and adding a destination forces a decision about its glyph
 * rather than defaulting to a generic dot.
 */
export const ROUTE_ICONS = {
  readiness: IconGauge,
  lab: IconFlask,
  tasks: IconChecklist,
  agents: IconChip,
  benchmarks: IconGrid,
  providers: IconServer,
  comparison: IconScales,
  arena: IconBoard,
  security: IconShield,
  replay: IconReplay,
  settings: IconSliders,
} as const;