/** Icons drawn as SVG rather than text glyphs, so they render identically
 *  whatever fonts the machine has.
 */

type IconProps = { className?: string; title?: string };

/** The compressor icons are one waveform drawn two ways, so the pair reads as
 *  a before and after. "Off" is five bars at their natural, uneven heights,
 *  the loudest reaching nearly the full height of the icon. "On" is the same
 *  five evened out to one short level, with a chevron above and below
 *  pressing in on them: the squeeze is drawn, not left to a difference in
 *  bar heights, which at button size is a pixel or two. Stroked a touch
 *  heavier than the outline icons, so five thin bars carry the same weight
 *  as the filled play and speaker beside them. */
const WAVE_X = [4.5, 8.25, 12, 15.75, 19.5];
const WAVE_WEIGHT = "2.2";
// Half-heights of each bar about the centre line, before the round caps.
const WAVE_LOUD = [3, 8.25, 5, 7, 2.5];
const WAVE_EVEN = [3, 3, 3, 3, 3];
// The chevrons: each spans the middle three bars and points at the centre.
const SQUEEZE = "M6 2.75 12 5.25 18 2.75M6 21.25 12 18.75 18 21.25";

function waveBars(halves: number[]): string {
  return WAVE_X.map((x, i) => `M${x} ${12 - halves[i]}V${12 + halves[i]}`).join("");
}

export function CompressorOn({ className, title }: IconProps) {
  return (
    <svg className={className} viewBox="0 0 24 24" aria-hidden={!title} focusable="false">
      {title ? <title>{title}</title> : null}
      <path d={waveBars(WAVE_EVEN)} fill="none" stroke="currentColor"
            strokeWidth={WAVE_WEIGHT} strokeLinecap="round" />
      <path d={SQUEEZE} fill="none" stroke="currentColor" strokeWidth="1.8"
            strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}

export function CompressorOff({ className, title }: IconProps) {
  return (
    <svg className={className} viewBox="0 0 24 24" aria-hidden={!title} focusable="false">
      {title ? <title>{title}</title> : null}
      <path d={waveBars(WAVE_LOUD)} fill="none" stroke="currentColor"
            strokeWidth={WAVE_WEIGHT} strokeLinecap="round" />
    </svg>
  );
}

/** A three-quarter circular arrow with the step written inside it.
 *
 *  Geometry is derived rather than eyeballed. The arrowhead sets the icon's
 *  outer bound, so the circle is sized against it (`cy - r - half` clears the
 *  viewBox edge) instead of leaving the ring floating in padding. The head is
 *  tangent to the circle: at the top of a circle the tangent is horizontal, so
 *  the base is vertical and the tip runs along the direction of travel. The
 *  gap sits where the head is, which is what makes the direction read.
 */
const CX = 12;
const CY = 12.4;
const R = 8.7;
const HEAD_HALF = 2.4;
const HEAD_DEPTH = 3.9;

function SkipArrow({
  back,
  seconds,
  showSeconds,
  className,
  title,
}: IconProps & { back: boolean; seconds: number; showSeconds: boolean }) {
  const topY = CY - R;
  // The arc must ARRIVE at the head, not leave from it: the line enters the
  // flat base and the point continues beyond. Drawing it the other way round
  // makes the stroke emerge from the tip, which reads as the wrong direction.
  // Screen angles increase clockwise, so sweep 0 runs 9->6->3->12 and reaches
  // the top moving left (rewind); sweep 1 runs 3->6->9->12 and reaches it
  // moving right. Either way the final point is the top, where the head sits.
  const d = `M${back ? CX - R : CX + R} ${CY} A${R} ${R} 0 1 ${
    back ? 0 : 1
  } ${CX} ${topY}`;
  const tip = back ? CX - HEAD_DEPTH : CX + HEAD_DEPTH;
  const head = `M${CX} ${topY - HEAD_HALF} L${CX} ${topY + HEAD_HALF} L${tip} ${topY} Z`;
  return (
    <svg className={className} viewBox="0 0 24 24" aria-hidden={!title} focusable="false">
      {title ? <title>{title}</title> : null}
      <path d={d} fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" />
      <path d={head} fill="currentColor" />
      {showSeconds ? (
        <text
          x={CX}
          y={CY + 2.9}
          textAnchor="middle"
          fontSize="9.5"
          fontWeight="600"
          letterSpacing="-0.3"
          fill="currentColor"
          // Part of the icon, not copyable text.
          style={{ userSelect: "none" }}
        >
          {seconds}s
        </text>
      ) : null}
    </svg>
  );
}

/** `showSeconds` is off where something beside the icon already says the
 *  number — the feedback bubble writes "5 seconds" under it, and the same
 *  figure twice in one badge reads as a mistake. */
export function SkipBack({
  seconds = 5,
  showSeconds = true,
  className,
  ...p
}: IconProps & { seconds?: number; showSeconds?: boolean }) {
  return (
    <SkipArrow back seconds={seconds} showSeconds={showSeconds} className={skipClass(className)} {...p} />
  );
}

export function SkipForward({
  seconds = 5,
  showSeconds = true,
  className,
  ...p
}: IconProps & { seconds?: number; showSeconds?: boolean }) {
  return (
    <SkipArrow
      back={false}
      seconds={seconds}
      showSeconds={showSeconds}
      className={skipClass(className)}
      {...p}
    />
  );
}

/** Marks the skip icons so the stylesheet can draw them larger than the
 *  rest. They are the only icons carrying a number, and at the size the
 *  others want, that number is about six pixels tall. */
function skipClass(className?: string): string {
  return className ? `${className} skip` : "skip";
}

export function Play({ className, title }: IconProps) {
  return (
    <svg className={className} viewBox="0 0 24 24" aria-hidden={!title} focusable="false">
      {title ? <title>{title}</title> : null}
      <path d="M8 5.2 L18.5 12 L8 18.8 Z" fill="currentColor" />
    </svg>
  );
}

export function Pause({ className, title }: IconProps) {
  return (
    <svg className={className} viewBox="0 0 24 24" aria-hidden={!title} focusable="false">
      {title ? <title>{title}</title> : null}
      <rect x="7" y="5" width="3.6" height="14" rx="1" fill="currentColor" />
      <rect x="13.4" y="5" width="3.6" height="14" rx="1" fill="currentColor" />
    </svg>
  );
}

/** Corner brackets: the same shape entering and leaving fullscreen, mirrored,
 *  so the control does not appear to change identity when toggled. */
export function Fullscreen({ className, title }: IconProps) {
  return (
    <svg className={className} viewBox="0 0 24 24" aria-hidden={!title} focusable="false">
      {title ? <title>{title}</title> : null}
      <path
        d="M4 9V5a1 1 0 0 1 1-1h4M15 4h4a1 1 0 0 1 1 1v4M20 15v4a1 1 0 0 1-1 1h-4M9 20H5a1 1 0 0 1-1-1v-4"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.9"
        strokeLinecap="round"
      />
    </svg>
  );
}

export function ExitFullscreen({ className, title }: IconProps) {
  return (
    <svg className={className} viewBox="0 0 24 24" aria-hidden={!title} focusable="false">
      {title ? <title>{title}</title> : null}
      <path
        d="M9 4v4a1 1 0 0 1-1 1H4M20 9h-4a1 1 0 0 1-1-1V4M15 20v-4a1 1 0 0 1 1-1h4M4 15h4a1 1 0 0 1 1 1v4"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.9"
        strokeLinecap="round"
      />
    </svg>
  );
}

/** Three states so the control reflects what it does, not just that it exists. */
export function Volume({ className, title, level }: IconProps & { level: "muted" | "low" | "high" }) {
  return (
    <svg className={className} viewBox="0 0 24 24" aria-hidden={!title} focusable="false">
      {title ? <title>{title}</title> : null}
      <path d="M4 9.5h3.5L12 5.5v13L7.5 14.5H4z" fill="currentColor" />
      {level === "muted" ? (
        <path d="M15.5 9.5l5 5M20.5 9.5l-5 5" fill="none" stroke="currentColor"
              strokeWidth="1.9" strokeLinecap="round" />
      ) : (
        <>
          <path d="M15.2 9.6a3.4 3.4 0 0 1 0 4.8" fill="none" stroke="currentColor"
                strokeWidth="1.8" strokeLinecap="round" />
          {level === "high" ? (
            <path d="M17.8 7.2a6.8 6.8 0 0 1 0 9.6" fill="none" stroke="currentColor"
                  strokeWidth="1.8" strokeLinecap="round" />
          ) : null}
        </>
      )}
    </svg>
  );
}

/** The app mark, same path as the launcher icon so the two cannot drift. */
export function Bolt({ className, title }: IconProps) {
  return (
    <svg className={className} viewBox="150 30 220 460" aria-hidden={!title} focusable="false">
      {title ? <title>{title}</title> : null}
      <path d="M300 40 L170 285 L258 285 L212 472 L352 232 L268 232 Z" fill="currentColor" />
    </svg>
  );
}

/** An arrow down onto a tray: "there is something to download", the update
 *  badge's mark. Stroked like the window buttons beside it. */
export function UpdateArrow({ className, title }: IconProps) {
  return (
    <svg className={className} viewBox="0 0 16 16" aria-hidden={!title} focusable="false">
      {title ? <title>{title}</title> : null}
      <path
        d="M8 2.5V10M4.75 6.75 8 10l3.25-3.25M3 13.5h10"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

/** A plain square. Stop is the one transport control with a settled shape;
 *  anything cleverer just makes people hesitate. */
export function Stop({ className, title }: IconProps) {
  return (
    <svg className={className} viewBox="0 0 24 24" aria-hidden={!title} focusable="false">
      {title ? <title>{title}</title> : null}
      <rect x="6.5" y="6.5" width="11" height="11" rx="1.4" fill="currentColor" />
    </svg>
  );
}

export function ChevronDown({ className, title }: IconProps) {
  return (
    <svg className={className} viewBox="0 0 24 24" aria-hidden={!title} focusable="false">
      {title ? <title>{title}</title> : null}
      <path d="M6 9.5 L12 15.5 L18 9.5" fill="none" stroke="currentColor"
            strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}

export function ChevronUp({ className, title }: IconProps) {
  return (
    <svg className={className} viewBox="0 0 24 24" aria-hidden={!title} focusable="false">
      {title ? <title>{title}</title> : null}
      <path d="M6 14.5 L12 8.5 L18 14.5" fill="none" stroke="currentColor"
            strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}

/** Chevron plus a shoulder, so "back" is distinct from "previous" at a glance. */
export function Back({ className, title }: IconProps) {
  return (
    <svg className={className} viewBox="0 0 24 24" aria-hidden={!title} focusable="false">
      {title ? <title>{title}</title> : null}
      <path d="M14.5 6 L8.5 12 L14.5 18" fill="none" stroke="currentColor"
            strokeWidth="2.1" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}

/** The mini player's resize grip: a chevron against a bar, `<|`, pointing
 *  the way the picture grows. Drawn rather than typed so it keeps its weight
 *  next to the rest of the chrome at any zoom. */
export function ResizeHandle({ className, title }: IconProps) {
  return (
    <svg className={className} viewBox="0 0 12 24" aria-hidden={!title} focusable="false">
      {title ? <title>{title}</title> : null}
      <path
        d="M7 8.5 L3.5 12 L7 15.5"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      <path
        d="M9.5 6.5 L9.5 17.5"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
      />
    </svg>
  );
}

/** A pushpin, upright. Outlined where it offers to pin, filled where the
 *  category already is — the same shape both ways, so the row's button and
 *  the pinned marker read as one idea. */
export function Pin({ className, title, filled }: IconProps & { filled?: boolean }) {
  return (
    <svg className={className} viewBox="0 0 24 24" aria-hidden={!title} focusable="false">
      {title ? <title>{title}</title> : null}
      <path
        d="M8.5 3.5h7l-1 6 3.25 3.5v1.75H6.25V13L9.5 9.5z"
        fill={filled ? "currentColor" : "none"}
        stroke="currentColor"
        strokeWidth="1.7"
        strokeLinejoin="round"
      />
      <path d="M12 14.75v6" fill="none" stroke="currentColor" strokeWidth="1.8"
            strokeLinecap="round" />
    </svg>
  );
}

/** Sort: a downward arrow beside three bars getting shorter. Generic on
 *  purpose — the button's label says which order, the icon only says that
 *  this is the thing that orders. */
export function SortOrder({ className, title }: IconProps) {
  return (
    <svg className={className} viewBox="0 0 24 24" aria-hidden={!title} focusable="false">
      {title ? <title>{title}</title> : null}
      <path d="M6 4.5v14M2.75 15.25 6 18.5l3.25-3.25" fill="none" stroke="currentColor"
            strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" />
      <path d="M12.5 6.5h9M12.5 12h6.5M12.5 17.5h4" fill="none" stroke="currentColor"
            strokeWidth="1.8" strokeLinecap="round" />
    </svg>
  );
}

/** The rail's fold: a window with its left column marked off, and a chevron
 *  saying which way the column goes — left to fold it away, right to bring
 *  it back. One drawing both ways, so the two buttons read as one switch
 *  even though they sit in different places. */
export function RailFold({ className, title, open }: IconProps & { open: boolean }) {
  return (
    <svg className={className} viewBox="0 0 24 24" aria-hidden={!title} focusable="false">
      {title ? <title>{title}</title> : null}
      <rect x="3.5" y="4.5" width="17" height="15" rx="2" fill="none" stroke="currentColor"
            strokeWidth="1.6" />
      <path d="M9 4.5v15" fill="none" stroke="currentColor" strokeWidth="1.6" />
      <path d={open ? "M15.5 9.25 12.75 12l2.75 2.75" : "M13.25 9.25 16 12l-2.75 2.75"}
            fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round"
            strokeLinejoin="round" />
    </svg>
  );
}

/** An eye struck through: the marker on a hidden category while hidden ones
 *  are being shown. The usual sign for "not shown", so it needs no legend. */
export function EyeOff({ className, title }: IconProps) {
  return (
    <svg className={className} viewBox="0 0 24 24" aria-hidden={!title} focusable="false">
      {title ? <title>{title}</title> : null}
      <path d="M2.75 12S6.25 5.75 12 5.75 21.25 12 21.25 12 17.75 18.25 12 18.25 2.75 12 2.75 12z"
            fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinejoin="round" />
      <circle cx="12" cy="12" r="2.75" fill="none" stroke="currentColor" strokeWidth="1.7" />
      <path d="M4 20 20 4" fill="none" stroke="currentColor" strokeWidth="1.8"
            strokeLinecap="round" />
    </svg>
  );
}

/** A tick: which of a menu's choices is the current one. */
export function Check({ className, title }: IconProps) {
  return (
    <svg className={className} viewBox="0 0 24 24" aria-hidden={!title} focusable="false">
      {title ? <title>{title}</title> : null}
      <path d="M5 12.5 9.5 17 19 7.5" fill="none" stroke="currentColor" strokeWidth="2.2"
            strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}

/** A crescent: the sleep timer. One circle with a second one cut out of it,
 *  so the shape is a moon at any size rather than a smudge. */
export function Moon({ className, title }: IconProps) {
  return (
    <svg className={className} viewBox="0 0 24 24" aria-hidden={!title} focusable="false">
      {title ? <title>{title}</title> : null}
      <path d="M19.5 14.6A8 8 0 1 1 9.4 4.5a6.3 6.3 0 0 0 10.1 10.1Z" fill="currentColor" />
    </svg>
  );
}

/** A speech balloon with two lines in it: the stream's audio tracks, which
 *  are almost always a choice of language. */
export function AudioTracks({ className, title }: IconProps) {
  return (
    <svg className={className} viewBox="0 0 24 24" aria-hidden={!title} focusable="false">
      {title ? <title>{title}</title> : null}
      <path d="M5 4.5h14a2 2 0 0 1 2 2v8.5a2 2 0 0 1-2 2h-7.5L7 20.5v-3.5H5a2 2 0 0 1-2-2V6.5a2 2 0 0 1 2-2Z"
            fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinejoin="round" />
      <path d="M7.5 9H16.5M7.5 12.5H13.5" fill="none" stroke="currentColor" strokeWidth="1.7"
            strokeLinecap="round" />
    </svg>
  );
}

/** Points at the submenu a menu row opens. */
export function ChevronRight({ className, title }: IconProps) {
  return (
    <svg className={className} viewBox="0 0 24 24" aria-hidden={!title} focusable="false">
      {title ? <title>{title}</title> : null}
      <path d="M9.5 6 15.5 12 9.5 18" fill="none" stroke="currentColor" strokeWidth="2"
            strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}

/** A setting's "back to the default": an arrow turning back on itself. */
export function ResetArrow({ className, title }: IconProps) {
  return (
    <svg className={className} viewBox="0 0 24 24" aria-hidden={!title} focusable="false">
      {title ? <title>{title}</title> : null}
      <path d="M4.5 12a7.5 7.5 0 1 0 2.2-5.3L4.5 8.9" fill="none" stroke="currentColor"
            strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" />
      <path d="M4.5 4.5v4.4h4.4" fill="none" stroke="currentColor" strokeWidth="2"
            strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}
