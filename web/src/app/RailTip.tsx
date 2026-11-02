/**
 * The label a rail row loses when the sidebar collapses, handed back on hover.
 *
 * It renders into document.body because the rail clips its own overflow, so a tip
 * positioned beside the row would be cut off at the rail's edge.
 */
import { useEffect, useRef, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';

export function RailTip({ label, enabled = true, children }: { label: string; enabled?: boolean; children: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  const [at, setAt] = useState<{ top: number; left: number } | null>(null);

  const show = () => {
    const el = ref.current;
    if (!enabled || !el) return;
    const r = el.getBoundingClientRect();
    setAt({ top: r.top + r.height / 2, left: r.right + 10 });
  };
  const hide = () => setAt(null);

  // Expanding the rail while a tip is up would strand it beside a row that no
  // longer needs naming.
  useEffect(() => {
    if (!enabled) setAt(null);
  }, [enabled]);

  // The tip is fixed to a position measured once, so a scroll or resize takes it down.
  useEffect(() => {
    if (!at) return;
    window.addEventListener('scroll', hide, true);
    window.addEventListener('resize', hide);
    return () => {
      window.removeEventListener('scroll', hide, true);
      window.removeEventListener('resize', hide);
    };
  }, [at]);

  return (
    // Deliberately unpositioned, so the row inside stays the indicator's offsetParent child.
    <div ref={ref} onPointerEnter={show} onPointerLeave={hide} onFocusCapture={show} onBlurCapture={hide} onClick={hide}>
      {children}
      {at &&
        createPortal(
          <span role="tooltip" className="rail-tip" style={{ top: at.top, left: at.left }}>
            {label}
          </span>,
          document.body,
        )}
    </div>
  );
}
