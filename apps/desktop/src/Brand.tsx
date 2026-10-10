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
 * # The mark: the Signal Aperture
 *
 * Four shapes, no more: a grounded navy stem, a blue aperture bowl with a wedge mouth, a cyan probe
 * needle entering through that mouth, and a cyan node resting in the counter of the P.
 *
 * The geometry is inlined rather than referenced as an `<img>` for two reasons. First, the same
 * artwork has to render at 32px in the rail and at 512px as the app icon, and a scaled raster would be
 * resampled at both. Second, the in-app mark is drawn with `currentColor` on the stem so it follows
 * whichever surface it lands on -- the navy rail or the white toolbar -- while the blue and cyan stay
 * constant because they are the brand, not the theme.
 *
 * It is `aria-hidden` because the wordmark beside it is the accessible name; a decorative graphic read
 * out as "the letter P with a probe" before "ProbeLab" is noise.
 *
 * # Why the paths are duplicated here
 *
 * These coordinates are the same ones in `src-tauri/brand/signal-aperture.svg` and
 * `src-tauri/app-icon.svg`, and keeping them in one hand-editable place is deliberate: the icon set is
 * generated from the SVG, the in-app mark is generated from this component, and `Brand.test.tsx` asserts
 * that both still describe four shapes. If they drift, that test fails rather than the product quietly
 * shipping two marks.
 */

/** The Signal Aperture. Decorative: the wordmark carries the name. */
export function BrandMark({ size = 38 }: { size?: number }) {
  return (
    <svg
      className="brand-mark-svg"
      width={size}
      height={size}
      /*
        The viewBox is cropped to the artwork's real bounds (x 148-361, y 99-408) with a little air,
        rather than to the full 512 canvas the icon is generated from. A mark that occupies 42% of its
        canvas renders as a 13px smudge in a 32px slot; cropping to the ink means the 32px slot is
        filled by the mark itself.
      */
      viewBox="132 82 244 344"
      fill="none"
      aria-hidden="true"
      focusable="false"
    >
      {/* Stem: the grounded structure. `currentColor` so it inverts on the white toolbar. */}
      <path
        d="M 148 388 L 148 156 Q 148 128 176 128 L 196 128 Q 224 128 224 156 L 224 388 Q 224 408 204 408 L 168 408 Q 148 408 148 388 Z"
        fill="currentColor"
      />
      {/*
        The bowl is two arc segments with a 36-degree wedge between them.

        The gap is real geometry, not a colour change, so it survives monochrome, 16px rasterisation
        and embossing. This is also the only place in the mark that reads as an "aperture": a closed
        ring would be the generic target icon this mark exists to replace.
      */}
      <path
        d="M 210.8 98.7 A 126 126 0 0 1 340.8 271.2 L 275.9 245.0 A 56 56 0 0 0 218.1 168.3 Z"
        fill="var(--brand-blue)"
      />
      <path
        d="M 290.8 330.9 A 126 126 0 0 1 210.8 349.3 L 218.1 279.7 A 56 56 0 0 0 253.7 271.5 Z"
        fill="var(--brand-blue)"
      />
      {/* Needle: the probe entering the mouth. It stops at the counter, never crosses through it. */}
      <line
        x1="346.6"
        y1="326.8"
        x2="271.5"
        y2="263.9"
        stroke="var(--brand-cyan)"
        strokeWidth="28"
        strokeLinecap="round"
      />
      {/*
        Node: the measurement itself, in the eye of the P.

        Centred at (244,216) r28, so its edge stops 6.5px short of the 56px counter wall. Two smaller
        revisions were measured before this one: (252,216) r32 reached 61px and ate 5px into the bowl,
        and (244,216) r30 left only 4.5px. Either is invisible at 256px and fatal at 16px, where the
        node and the bowl fuse into one cyan blob and the aperture stops being an aperture. The gap is
        a design rule; `Brand.test.tsx` asserts at least 6px of it.
      */}
      <circle cx="244" cy="216" r="28" fill="var(--brand-cyan)" />
    </svg>
  );
}

/**
 * The wordmark and subtitle, at one of two sizes.
 *
 * `compact` is for the toolbar, which already sits in a short slot; the default is for the nav column,
 * which has room for the full lockup. Both render the same words. The separator is a real `//` because
 * that is the product's name, not decoration.
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
      <BrandMark size={compact ? 34 : 30} />
      <div className="brand-text">
        <div className="brand-name">
          PROBE<span>//</span>LAB
        </div>
        <div className="brand-subtitle">{subtitle}</div>
      </div>
    </div>
  );
}