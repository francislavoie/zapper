// The app's tooltips: one element, one set of listeners, every target found
// by attribute.
//
// Why not `title`. The native tooltip waits most of a second before it
// appears, cannot be styled, and is drawn by the toolkit in its own light
// theme over a dark app. The rule is that nothing here uses it — see
// src/__checks__/no-title.test.mjs, which enforces it.
//
// Why attributes rather than a <Tip> wrapper. The channel list and the rail
// are virtualised and re-render their rows on every scroll step; a component
// per row with its own hover state would be hundreds of subscriptions for a
// thing that is on screen once at a time. Instead a row says what its tip is
// in a `data-` attribute, which costs nothing, and one delegated listener on
// the document works out what is under the pointer when it moves. It also
// needs no wrapper element, so nothing about a row's layout changes.
//
// The two attributes:
//
//   data-tip="text"           always shown on hover or keyboard focus.
//   data-tip-overflow         shown only while the element is actually cut
//                             short by its ellipsis; the text is the
//                             attribute's value, or the element's own text
//                             when the value is empty.
//
// A truncation tip that is not needed (the text fits) is passed over, and the
// nearest tipped ancestor gets its turn — so a cut-off name inside a button
// with its own tip shows the name when it is cut off and the button's tip
// when it is not.
//
// Text that can be read in full gets no tip: a name, a title or a heading
// takes `data-tip-overflow`, never `data-tip`. `data-tip` is for controls
// whose label is not on them (an icon) or says less than the tip.
//
// Where it goes (logic/tooltip.ts tipAnchor): a truncation tip starts where
// the cut text does, so the whole name reads on from the same place; any
// other tip centres on a control, and over something wider — a row — on
// the pointer, never on the middle of a full-width box.
//
// No delay, deliberately: a tooltip that waits is a tooltip nobody sees,
// and the point of these is to be read in passing.

import { placeTip, tipAnchor } from "./logic/tooltip";
import { documentScale } from "./zoom";

const TIPPED = "[data-tip],[data-tip-overflow]";

/** The tip's element id, for `aria-describedby`. */
export const TIP_ID = "app-tooltip";

/** Whether an element's text is cut short right now.
 *
 *  `scrollWidth > clientWidth` is the usual test, but both are rounded to
 *  whole pixels, and not always the same way: WebKit can round a box of
 *  939.05px to a client width of 939 and a scroll width of 940, and a name
 *  that fits with room to spare then counts as cut short — the tooltip that
 *  repeats a fully visible episode title. So a difference of a pixel is not
 *  trusted either way. Beyond that the integers decide; within it, when the
 *  element is one that ellipsizes, the text's own laid-out width is
 *  measured against the box it has to fit in, unrounded (a name that
 *  overflows by half a pixel is still drawn with an ellipsis).
 *
 *  Line-clamped text (the programme description) overflows downwards
 *  instead, and is checked that way — but only when a clamp is in force,
 *  and by more than a pixel, because a single line's height and its scroll
 *  height can differ by a descender or a rounding without anything being
 *  hidden. A clamp that bites hides a whole line. */
function isTruncated(el: HTMLElement): boolean {
  if (el.scrollWidth - el.clientWidth > 1) return true;
  const cs = getComputedStyle(el);
  const clamp = cs.getPropertyValue("-webkit-line-clamp");
  if (clamp && clamp !== "none" && el.scrollHeight - el.clientHeight > 1) return true;
  if (cs.textOverflow !== "ellipsis") return false;

  const range = document.createRange();
  range.selectNodeContents(el);
  const text = range.getBoundingClientRect().width;
  // Computed lengths are CSS pixels; the rects are visual ones.
  const inset =
    (parseFloat(cs.paddingLeft) +
      parseFloat(cs.paddingRight) +
      parseFloat(cs.borderLeftWidth) +
      parseFloat(cs.borderRightWidth)) *
    documentScale();
  const room = el.getBoundingClientRect().width - inset;
  return text > room + 0.5;
}

/** What `el` would say, or null if it has nothing to say at the moment. */
function textOf(el: Element): string | null {
  const plain = el.getAttribute("data-tip");
  if (plain) return plain;
  if (!el.hasAttribute("data-tip-overflow")) return null;
  if (!(el instanceof HTMLElement) || !isTruncated(el)) return null;
  // A bare `data-tip-overflow` in JSX arrives as the string "true", which is
  // never the text anybody meant; write `data-tip-overflow=""` for "my own
  // text", but do not show "true" if somebody forgets.
  const given = el.getAttribute("data-tip-overflow");
  if (given && given !== "true") return given;
  return el.textContent?.trim() || null;
}

/** Where `el`'s text begins on screen, from the left: its laid-out contents,
 *  kept inside the element's own box (`box`, already measured). An element
 *  with no text to measure starts where it does. */
function textLeft(el: HTMLElement, box: DOMRect): number {
  const range = document.createRange();
  range.selectNodeContents(el);
  const r = range.getBoundingClientRect();
  if (!(r.width > 0)) return box.left;
  return Math.min(Math.max(r.left, box.left), box.right);
}

/** The nearest tipped element at or above `start` that has something to say. */
function resolve(start: EventTarget | null): { el: HTMLElement; text: string } | null {
  let el = start instanceof Element ? start.closest(TIPPED) : null;
  while (el) {
    const text = textOf(el);
    if (text && el instanceof HTMLElement) return { el, text };
    el = el.parentElement?.closest(TIPPED) ?? null;
  }
  return null;
}

/** Starts the tooltip behaviour, drawing into `tip`. Returns the teardown.
 *
 *  `tip` must be positioned against the viewport's origin — it is placed
 *  with `left`/`top` computed from client rects. */
export function installTooltips(tip: HTMLElement): () => void {
  /** What the tip is showing for, and why. */
  let current: HTMLElement | null = null;
  let viaFocus = false;
  /** Whether `aria-describedby` on `current` was put there by this. */
  let described = false;
  /** Pressed while showing: stays quiet until the pointer moves to
   *  something else, as a native tooltip does. A click is a sign the tip
   *  has been read, and one reappearing over the result of the click — a
   *  play button that has just become a pause button — is in the way. */
  let quiet: HTMLElement | null = null;
  /** Where the pointer came onto `current`, horizontally; null when it was
   *  focus that showed the tip. Taken once per target, so a tip over a wide
   *  row holds still while the pointer moves along it. */
  let pointerX: number | null = null;

  const place = () => {
    if (!current) return;
    // Measured at the origin first: a box placed near the right edge would
    // otherwise be squeezed by the edge and measure narrower than it is.
    tip.style.left = "0px";
    tip.style.top = "0px";
    const t = current.getBoundingClientRect();
    const r = tip.getBoundingClientRect();
    // Visual pixels in, CSS pixels out; see documentScale.
    const scale = documentScale();
    const overflow = !current.getAttribute("data-tip");
    // Lined up text to text: the tip's own inset taken off, so the full name
    // starts exactly where the cut one does.
    const cs = getComputedStyle(tip);
    const inset = (parseFloat(cs.paddingLeft) + parseFloat(cs.borderLeftWidth)) * scale;
    const anchor = tipAnchor(t, overflow, textLeft(current, t) - inset, pointerX);
    const at = placeTip(t, r.width, r.height, window.innerWidth, window.innerHeight, anchor);
    tip.style.left = `${at.left / scale}px`;
    tip.style.top = `${at.top / scale}px`;
    tip.dataset.side = at.side;
  };

  const show = (el: HTMLElement, text: string, focus: boolean, x: number | null = null) => {
    if (current !== el) {
      release();
      current = el;
      pointerX = focus ? null : x;
      // Only when the text adds something. A button already named "Stop"
      // described as "Stop" would be read out twice.
      if (!el.hasAttribute("aria-describedby") && el.getAttribute("aria-label") !== text) {
        el.setAttribute("aria-describedby", TIP_ID);
        described = true;
      }
    }
    viaFocus = focus;
    if (tip.textContent !== text) tip.textContent = text;
    // Unhidden after the text is in, and placed before the frame is drawn:
    // the fade-in restarts each time it goes from hidden to shown, and a tip
    // moving between neighbours stays up without blinking.
    tip.hidden = false;
    place();
    watch();
  };

  /** Lets go of `current` without touching the tip itself. */
  const release = () => {
    if (current && described) current.removeAttribute("aria-describedby");
    described = false;
    current = null;
  };

  const hide = () => {
    release();
    viaFocus = false;
    tip.hidden = true;
    unwatch();
  };

  // While a tip is up, the page can change under it without the pointer
  // moving: a scroll replaces the row, a mode change removes the button, the
  // player's bar fades out, the button's own label flips from Play to Pause.
  // A tip left behind by any of those points at nothing. Observing only while
  // showing keeps this off the hot path the rest of the time.
  const observer = new MutationObserver((records) => {
    if (!current) return;
    if (!current.isConnected) return hide();
    // A faded bar turns pointer events off on its way out, and so it is
    // a reliable sign the control is no longer there to be pointed at —
    // more reliable than opacity, which is still mid-transition when the
    // class changes.
    if (!viaFocus && getComputedStyle(current).pointerEvents === "none") return hide();
    if (records.some((r) => r.type === "attributes" && r.target === current)) {
      const text = textOf(current);
      if (text) show(current, text, viaFocus);
      else hide();
    }
  });
  let watching = false;
  const watch = () => {
    if (watching) return;
    watching = true;
    observer.observe(document.body, {
      subtree: true,
      childList: true,
      attributes: true,
      attributeFilter: ["class", "data-tip", "data-tip-overflow", "hidden", "disabled"],
    });
  };
  const unwatch = () => {
    if (!watching) return;
    watching = false;
    observer.disconnect();
  };

  const onOver = (e: PointerEvent) => {
    const found = resolve(e.target);
    if (!found) {
      quiet = null;
      if (!viaFocus) hide();
      return;
    }
    if (found.el === quiet) return;
    quiet = null;
    show(found.el, found.text, false, e.clientX);
  };

  const onOut = (e: PointerEvent) => {
    // Out of the window altogether. Moving between elements is handled by
    // the `pointerover` that follows on the next one.
    if (!e.relatedTarget && !viaFocus) {
      quiet = null;
      hide();
    }
  };

  const onDown = () => {
    if (current && !viaFocus) quiet = current;
    hide();
  };

  const onFocusIn = (e: FocusEvent) => {
    const el = e.target;
    // Keyboard focus only. A click focuses its button too, and a tip that
    // appeared on every click would be the very thing `quiet` prevents.
    if (!(el instanceof HTMLElement) || !el.matches(":focus-visible")) return;
    const found = resolve(el);
    if (found) show(found.el, found.text, true);
  };

  const onFocusOut = () => {
    if (viaFocus) hide();
  };

  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") hide();
  };

  // Anything that moves the page out from under the tip.
  const onAway = () => hide();

  const doc = document;
  doc.addEventListener("pointerover", onOver, true);
  doc.addEventListener("pointerout", onOut, true);
  doc.addEventListener("pointerdown", onDown, true);
  doc.addEventListener("focusin", onFocusIn, true);
  doc.addEventListener("focusout", onFocusOut, true);
  doc.addEventListener("keydown", onKey, true);
  doc.addEventListener("scroll", onAway, true);
  window.addEventListener("resize", onAway);
  window.addEventListener("blur", onAway);

  return () => {
    hide();
    doc.removeEventListener("pointerover", onOver, true);
    doc.removeEventListener("pointerout", onOut, true);
    doc.removeEventListener("pointerdown", onDown, true);
    doc.removeEventListener("focusin", onFocusIn, true);
    doc.removeEventListener("focusout", onFocusOut, true);
    doc.removeEventListener("keydown", onKey, true);
    doc.removeEventListener("scroll", onAway, true);
    window.removeEventListener("resize", onAway);
    window.removeEventListener("blur", onAway);
  };
}
