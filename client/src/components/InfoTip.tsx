/**
 * InfoTip — small (i) icon that shows a tooltip popup on hover/focus.
 *
 * Usage: <InfoTip text="Explanation here" />
 * Renders an inline icon next to headings/labels for discoverability.
 */

import { useState, useRef, useEffect } from 'react';

interface InfoTipProps {
  text: string;
}

export function InfoTip({ text }: InfoTipProps) {
  const [visible, setVisible] = useState(false);
  const tipRef = useRef<HTMLSpanElement>(null);
  const popupRef = useRef<HTMLSpanElement>(null);

  // Reposition popup if it overflows the viewport
  useEffect(() => {
    if (!visible || !popupRef.current) return;
    const rect = popupRef.current.getBoundingClientRect();
    if (rect.right > window.innerWidth - 8) {
      popupRef.current.style.left = 'auto';
      popupRef.current.style.right = '0';
    }
    if (rect.left < 8) {
      popupRef.current.style.left = '0';
      popupRef.current.style.right = 'auto';
    }
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
