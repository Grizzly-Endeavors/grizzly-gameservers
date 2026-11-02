/**
 * Small shared pieces both consoles use, so the host's view and the owner's view read
 * as one product: a card, a status badge, a KPI tile, and the flag that says where a
 * number came from.
 */
import type { ReactNode } from 'react';
import type { ServerStatus } from '../data/scenarios';

export function Card({ title, aside, children, className = '' }: { title?: ReactNode; aside?: ReactNode; children: ReactNode; className?: string }) {
  return (
    <section className={`card ${className}`}>
      {(title || aside) && (
        <header className="card-head">
          {title && <h2 className="card-title">{title}</h2>}
          {aside && <div className="card-aside">{aside}</div>}
        </header>
      )}
      {children}
    </section>
  );
}

const STATUS_TONE: Record<ServerStatus, 'ok' | 'warn' | 'bad' | 'off'> = { Running: 'ok', Starting: 'warn', Stopped: 'off', Crashed: 'bad' };

export function StatusBadge({ status }: { status: ServerStatus }) {
  return (
    <span className={`badge badge-${STATUS_TONE[status]}`}>
      <span className={`lamp lamp-${STATUS_TONE[status]}`} aria-hidden="true" />
      {status}
    </span>
  );
}

export function Kpi({ label, value, sub, tone }: { label: string; value: ReactNode; sub?: ReactNode; tone?: 'ok' | 'warn' | 'bad' | 'brand' }) {
  return (
    <div className={`kpi ${tone ? `kpi-${tone}` : ''}`}>
      <div className="kpi-label">{label}</div>
      <div className="kpi-value">{value}</div>
      {sub && <div className="kpi-sub">{sub}</div>}
    </div>
  );
}

/**
 * Where a number came from. Measured results, industry estimates, illustrative demo
 * data and placeholders each say so, because the rules judge honest reporting.
 */
export function Provenance({ kind, children }: { kind: 'measured' | 'estimated' | 'demo' | 'sample'; children?: ReactNode }) {
  const text = { measured: 'Measured on held-out set', estimated: 'Estimate', demo: 'Demo data', sample: 'Sample data' }[kind];
  return (
    <span className={`prov prov-${kind}`} title={typeof children === 'string' ? children : undefined}>
      {text}
      {children && typeof children !== 'string' ? children : null}
    </span>
  );
}
