/**
 * Every ticket Gary took across the fleet, newest first. Collapsed, a row is the
 * one-line story a lead needs; opened, it shows exactly what Gary did, what he told the
 * customer, and what it cost, so a host can audit him the way it would audit a hire.
 */
import { useEffect, useState } from 'react';
import { useStore } from '../app/store';
import { describeTool, fmtMs, fmtTokens, fmtUsd, isMutating, shortModel, turnCost } from '../gary/derive';
import { Icon } from '../app/icons';
import { Card, Provenance } from '../ui/ui';
import { RATES } from './evidence-shared';
import { INCIDENTS, OUTCOME_LABEL, OUTCOME_TONE, agoText, type Incident, type Outcome } from './fleet';

type Filter = 'all' | Outcome;

const FILTERS: { key: Filter; label: string }[] = [
  { key: 'all', label: 'All' },
  { key: 'healthy', label: 'Fixed' },
  { key: 'rolled_back', label: 'Rolled back' },
  { key: 'escalated', label: 'Escalated' },
  { key: 'no_change', label: 'No change' },
];

/** Bold markdown is the only formatting Gary's replies use here; render it without a parser. */
function Reply({ text }: { text: string }) {
  const parts = text.split(/(\*\*[^*]+\*\*|`[^`]+`)/g);
  return (
    <p className="host-reply">
      {parts.map((p, i) =>
        p.startsWith('**') ? <strong key={i}>{p.slice(2, -2)}</strong> : p.startsWith('`') ? <code key={i}>{p.slice(1, -1)}</code> : <span key={i}>{p}</span>,
      )}
    </p>
  );
}

function IncidentRow({ inc, open, onToggle }: { inc: Incident; open: boolean; onToggle: () => void }) {
  const r = inc.response;
  const cost = turnCost(r, RATES);
  const panel = `${inc.id}-detail`;
  return (
    <li className={`host-ir ${open ? 'is-open' : ''}`}>
      <button type="button" className="host-ir-head" aria-expanded={open} aria-controls={panel} onClick={onToggle}>
        <span className={`lamp lamp-${OUTCOME_TONE[inc.outcome]}`} aria-hidden="true" />
        <span className="host-ir-main">
          <span className="host-ir-line">
            <strong>{inc.server}</strong>
            <span className="muted">
              {inc.customer} · {inc.game}
            </span>
          </span>
          <span className="host-ir-diag">{inc.diagnosis}</span>
        </span>
        <span className="host-ir-side">
          <span className={`badge badge-${OUTCOME_TONE[inc.outcome]}`}>{OUTCOME_LABEL[inc.outcome]}</span>
          <span className="muted host-ir-ago">{agoText(inc.ago)}</span>
        </span>
        <span className="host-ir-chev" aria-hidden="true">
          <Icon name="chevron" size={16} />
        </span>
      </button>
      {open && (
        <div id={panel} className="host-ir-body">
          {inc.escalationReason && (
            <div className="banner banner-warn host-ir-esc">
              <strong>Why Gary handed this over:</strong> {inc.escalationReason}
            </div>
          )}
          <div className="host-ir-cols">
            <div>
              <h3 className="host-ir-h">What Gary did</h3>
              <ol className="host-steps">
                {r.tool_calls.map((t, i) => (
                  <li key={i} className={isMutating(t) ? 'is-write' : ''}>
                    <span className="host-step-n">{i + 1}</span>
                    <span>
                      <span className="host-step-text">{describeTool(t)}</span>
                      <span className="host-step-result muted">{t.result.split('\n')[0]}</span>
                    </span>
                  </li>
                ))}
                {r.tool_calls.length === 0 && <li className="muted">Answered without touching the server.</li>}
              </ol>
            </div>
            <div>
              <h3 className="host-ir-h">What he told the customer</h3>
              <Reply text={r.reply} />
            </div>
          </div>
          <div className="host-ir-meta">
            <span>{shortModel(r.generations[0]?.model ?? '')}</span>
            <span>{r.rounds} rounds</span>
            <span>{fmtMs(r.latency_ms)} model time</span>
            <span>
              {fmtTokens(r.usage.prompt_tokens)} in ({fmtTokens(r.usage.cached_tokens)} cached) · {fmtTokens(r.usage.completion_tokens)} out
            </span>
            <span className="host-cost">{fmtUsd(cost)}</span>
          </div>
        </div>
      )}
    </li>
  );
}

export function Incidents() {
  const { incident, openIncident } = useStore();
  const [filter, setFilter] = useState<Filter>('all');
  // Arriving from "Open" elsewhere: start with that incident expanded and in view.
  const [open, setOpen] = useState<string | null>(incident);
  useEffect(() => {
    if (!incident) return;
    document.getElementById(`${incident}-detail`)?.closest('li')?.scrollIntoView({ block: 'center', behavior: 'smooth' });
    openIncident(null);
  }, [incident, openIncident]);
  const rows = INCIDENTS.filter((i) => filter === 'all' || i.outcome === filter);

  return (
    <div className="screen">
      <div className="screen-head host-head">
        <div>
          <h1>Incidents</h1>
          <p className="screen-sub">Every ticket Gary took, with every step he ran. Open one to audit it.</p>
        </div>
        <Provenance kind="demo" />
      </div>

      <div className="filters" role="group" aria-label="Filter by outcome">
        {FILTERS.map((f) => (
          <button key={f.key} type="button" className={`filter ${filter === f.key ? 'is-on' : ''}`} aria-pressed={filter === f.key} onClick={() => setFilter(f.key)}>
            {f.label}
            <span className="filter-n">{INCIDENTS.filter((i) => f.key === 'all' || i.outcome === f.key).length}</span>
          </button>
        ))}
      </div>

      <Card className="host-ir-card">
        <ul className="host-ir-list">
          {rows.map((i) => (
            <IncidentRow key={i.id} inc={i} open={open === i.id} onToggle={() => setOpen(open === i.id ? null : i.id)} />
          ))}
        </ul>
        {rows.length === 0 && (
          <div className="empty">
            <div className="empty-title">Nothing here</div>
            <div className="empty-body">No incidents with this outcome.</div>
          </div>
        )}
      </Card>
    </div>
  );
}
