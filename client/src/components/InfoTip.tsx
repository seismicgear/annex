/**
 * InfoTip — small (i) icon that shows a tooltip popup on hover/focus.
 *
 * Usage: <InfoTip text="Explanation here" />
 * Renders an inline icon next to headings/labels for discoverability.
 */

import { useState, useRef, useEffect } from 'react';

/** Keep this much clear of the viewport edge. */
const MARGIN = 8;

interface InfoTipProps {
  text: string;
}

export function InfoTip({ text }: InfoTipProps) {
  const [visible, setVisible] = useState(false);
  const tipRef = useRef<HTMLSpanElement>(null);
  const popupRef = useRef<HTMLSpanElement>(null);

  /**
   * Keep the popup inside the viewport, by exactly as much as it overflows.
   *
   * The previous version set `right: 0` (or `left: 0`) and left the base
   * `transform: translateX(-50%)` in place, so the popup moved an EXTRA half
   * of its own width past where it was aimed. On a phone, opening the
   * rightmost tip in the peer dialog threw a 280px popup clean off the left
   * edge of a 390px screen: the text was cut at x=0, unreadable, and nowhere
   * near the icon it belonged to. The correction meant to prevent overflow
   * caused it, on the opposite side.
   *
   * Shifting the existing transform by the measured overflow is bounded —
   * the popup is at most 280px wide against any supported viewport, so one
   * pass always lands it inside. The arrow is pinned back to the icon with
   * `--tip-shift` so it still points at what it describes.
   */
  useEffect(() => {
    const el = popupRef.current;
    if (!visible || !el) return;
    const rect = el.getBoundingClientRect();
    const overflowRight = rect.right - (window.innerWidth - MARGIN);
    const overflowLeft = MARGIN - rect.left;
    const shift = overflowRight > 0 ? -overflowRight : overflowLeft > 0 ? overflowLeft : 0;
    if (shift === 0) return;
    el.style.transform = `translateX(calc(-50% + ${shift}px))`;
    el.style.setProperty('--tip-shift', `${shift}px`);
  }, [visible]);

  return (
    <span
      className="info-tip"
      ref={tipRef}
      onMouseEnter={() => setVisible(true)}
      onMouseLeave={() => setVisible(false)}
      onFocus={() => setVisible(true)}
      onBlur={() => setVisible(false)}
      tabIndex={0}
      role="button"
      aria-label={text}
    >
      <svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor" className="info-tip-icon">
        <path d="M8 1a7 7 0 1 0 0 14A7 7 0 0 0 8 1zm0 12.5a5.5 5.5 0 1 1 0-11 5.5 5.5 0 0 1 0 11z"/>
        <path d="M8 6.5a.75.75 0 0 1 .75.75v3a.75.75 0 0 1-1.5 0v-3A.75.75 0 0 1 8 6.5zM8 4.5a.75.75 0 1 0 0 1.5.75.75 0 0 0 0-1.5z"/>
      </svg>
      {visible && (
        <span className="info-tip-popup" ref={popupRef} role="tooltip">
          {text}
        </span>
      )}
    </span>
  );
}
