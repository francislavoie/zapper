/** Icons drawn as SVG rather than text glyphs, so they render identically
 *  whatever fonts the machine has.
 */

type IconProps = { className?: string; title?: string };

/** The compressor icons are one waveform drawn two ways, so the pair reads as
 *  a before and after. "Off" is five bars at their natural, uneven heights.
 *  "On" is the same five squeezed toward an even level between two rails —
 *  the rails are what tells them apart at button size, where a difference in
 *  bar heights alone is a pixel or two. */
const WAVE_X = [4.5, 8.25, 12, 15.75, 19.5];
// Half-heights of each bar about the centre line, before the round caps.
const WAVE_LOUD = [2.5, 7.5, 4.5, 6.5, 3];
const WAVE_SQUEEZED = [3.2, 4.6, 3.8, 4.3, 3.4];

function waveBars(halves: number[]): string {
  return WAVE_X.map((x, i) => `M${x} ${12 - halves[i]}V${12 + halves[i]}`).join("");
}

export function CompressorOn({ className, title }: IconProps) {
  return (
    <svg className={className} viewBox="0 0 24 24" aria-hidden={!title} focusable="false">
      {title ? <title>{title}</title> : null}
      <path d="M2.5 4H21.5M2.5 20H21.5" fill="none" stroke="currentColor"
            strokeWidth="1.7" strokeLinecap="round" />
      <path d={waveBars(WAVE_SQUEEZED)} fill="none" stroke="currentColor"
            strokeWidth="2" strokeLinecap="round" />
    </svg>
  );
}

export function CompressorOff({ className, title }: IconProps) {
  return (
    <svg className={className} viewBox="0 0 24 24" aria-hidden={!title} focusable="false">
      {title ? <title>{title}</title> : null}
      <path d={waveBars(WAVE_LOUD)} fill="none" stroke="currentColor"
            strokeWidth="2" strokeLinecap="round" />
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
