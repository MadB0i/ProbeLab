/**
 * The brand lockup, in one place.
 *
 * # Why this component exists
 *
 * The product had three different brand treatments live at once, and none of them agreed:
 *
 * | where                  | mark     | wordmark      | subtitle                          |
 * |------------------------|----------|---------------|-----------------------------------|
 * | `App.tsx` topbar       | `P//L`   | `PROBE//LAB`  | AI CAPABILITY & AGENT EVALUATION  |
 * | `AppShell.tsx` nav     | none     | `PROBE//LAB`  | EVALUATION LABORATORY             |
 * | `App.tsx` footer       | none     | `PROBE//LAB`  | DESKTOP VERTICAL SLICE            |
 *
 * Three spellings of one name, in two type sizes and two letter-spacings, with the monogram in one
 * place and not the others. A visitor who saw the sidebar and then the topbar could not tell whether
 * they were in the same product, and the two subtitles describe it differently. This renders one
 * lockup, so the two places cannot drift again.
 *
 * # The mark
 *
 * `BrandMark` draws the same containment rings as `src-tauri/app-icon.svg`: a subject at the centre,
 * nested boundaries, and the outermost one drawn open. The gap is deliberate -- ProbeLab is here to
 * find out whether a boundary holds, and three closed circles would assert an answer nobody has
 * earned. The outer ring is the containment layer and the only one not closed, which is also why it
 * is the one drawn open.
 *
 * The SVG is inlined rather than referenced as a file so the rings scale with the `currentColor`
 * token and stay crisp in a 38px slot. It is `aria-hidden` because the wordmark beside it is the
 * accessible name; a decorative graphic read out as "concentric circles" before "ProbeLab" is noise.
 */

/** The containment-rings mark. Decorative: the wordmark carries the name. */
export function BrandMark({ size = 38 }: { size?: number }) {
  const centre = 24;
  return (
    <svg
      className="brand-mark-svg"
      width={size}
      height={size}
      viewBox="0 0 48 48"
      fill="none"
      aria-hidden="true"
      focusable="false"
    >
      {/*
        Radii and dash lengths are the 512px source scaled by 1/10.67, so this file and
        `app-icon.svg` describe the same geometry and a change to one is a change to the mark in both
        places. The outer ring carries the only gap.
      */}
      <circle
        cx={centre}
        cy={centre}
        r="18.4"
        stroke="var(--cyan)"
        strokeWidth="2.4"
        strokeDasharray="84.4 31.1"
        transform="rotate(24 24 24)"
      />
      <circle
        cx={centre}
        cy={centre}
        r="13.1"
        stroke="var(--violet)"
        strokeWidth="2.1"
        strokeDasharray="65.5 16.9"
        transform="rotate(-42 24 24)"
      />
      <circle cx={centre} cy={centre} r="8.2" stroke="var(--cyan)" strokeWidth="1.9" />
      <circle cx={centre} cy={centre} r="2.8" fill="var(--cyan)" />
    </svg>
  );
}

/**
 * The wordmark and subtitle, at one of two sizes.
 *
 * `compact` is for the topbar, which already sits in a 38px-tall slot; the default is for the nav
 * column, which has room for the full lockup. Both render the same words. The separator is a real
 * `//` because that is the product's name, not decoration.
 */
export function BrandLockup({
  subtitle = "AI CAPABILITY & AGENT EVALUATION",
  compact = false,
}: {
  subtitle?: string;
  compact?: boolean;
}) {
  return (
    <div className={compact ? "brand-lockup brand-lockup--compact" : "brand-lockup"}>
      <BrandMark size={compact ? 38 : 32} />
      <div className="brand-text">
        <div className="brand-name">
          PROBE<span>//</span>LAB
        </div>
        <div className="brand-subtitle">{subtitle}</div>
      </div>
    </div>
  );
}
