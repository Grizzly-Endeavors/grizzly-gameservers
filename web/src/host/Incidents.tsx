/**
 * Every ticket Gary took across the fleet, newest first. Escalated tickets carry an
 * inline staff resolution button; opening any row shows the exact tool trace Gary ran,
 * what he told the customer, and what the turn cost.
 */
import { useEffect, useState } from 'react';
import { Icon } from '../app/icons';
import { useStore, type Turn } from '../app/store';
import { SCENARIOS } from '../data/scenarios';
import { describeTool, fmtMs, fmtTokens, fmtUsd, isMutating, safetyOf, serversTouched, shortModel, turnCost } from '../gary/derive';
import { Card, Provenance } from '../ui/ui';
import { RATES } from './evidence-shared';
import { INCIDENTS, OUTCOME_LABEL, OUTCOME_TONE, agoText, type Incident, type Outcome } from './fleet';

type Filter = 'all' | Outcome;

const FILTERS: { key: Filter; label: string }[] = [
  { key: 'all', label: 'All incidents' },
  { key: 'escalated', label: 'Escalated to staff' },
  { key: 'rolled_back', label: 'Rolled back' },
  { key: 'healthy', label: 'Fixed' },
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

function matchScenarioId(inc: Incident): string | undefined {
  return SCENARIOS.find((s) => s.response === inc.response || s.id === inc.id.replace(/^session-.*-/, ''))?.id;
}

function sessionTurnToIncident(t: Turn, idx: number): Incident | null {
  if (t.status !== 'done' || !t.response) return null;
  const r = t.response;
  const safety = safetyOf(r);
  const touched = serversTouched(r.tool_calls)[0] ?? 'mc-friends-smp';
  const scId = t.replay?.scenario && t.replay.scenario !== 'status' ? t.replay.scenario : undefined;
  const outcome: Outcome = r.escalated
    ? 'escalated'
    : safety.outcome === 'rolled_back'
      ? 'rolled_back'
      : safety.mutated
        ? 'healthy'
        : 'no_change';
  return {
    id: `session-${idx}-${scId ?? t.id}`,
    server: touched,
    customer: 'Friday Night Crew (session)',
    game: touched.startsWith('valheim') ? 'Valheim' : touched.startsWith('terraria') ? 'Terraria' : 'Minecraft',
    ago: 0,
    diagnosis: t.prompt,
    outcome,
    escalationReason: r.escalation_reason ?? undefined,
    response: r,
  };
}

function IncidentRow({
  inc,
  open,
  resolvedByStaff,
  onToggle,
  onResolve,
  onCompareArena,
}: {
  inc: Incident;
  open: boolean;
  resolvedByStaff: boolean;
  onToggle: () => void;
  onResolve: () => void;
  onCompareArena?: (scenarioId: string) => void;
}) {
  const r = inc.response;
  const cost = turnCost(r, RATES);
  const per1k = cost != null ? `$${(cost * 1000).toFixed(2)} / 1k turns` : null;
  const scenarioId = matchScenarioId(inc);
  const panel = `${inc.id}-detail`;
  const effectiveOutcome: Outcome = resolvedByStaff ? 'healthy' : inc.outcome;
  const tone = OUTCOME_TONE[effectiveOutcome];
  const label = resolvedByStaff ? 'Staff approved · verified' : OUTCOME_LABEL[inc.outcome];
  const needsStaff = inc.outcome === 'escalated' && !resolvedByStaff;

  return (
    <li className={`host-ir ${open ? 'is-open' : ''} ${needsStaff ? 'is-attn' : ''}`}>
      <button type="button" className="host-ir-head" aria-expanded={open} aria-controls={panel} onClick={onToggle}>
        <span className={`lamp lamp-${tone}`} aria-hidden="true" />
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
          <span className={`badge badge-${tone}`}>{label}</span>
          <span className="muted host-ir-ago">{agoText(inc.ago)}</span>
        </span>
        <span className="host-ir-chev" aria-hidden="true">
          <Icon name="chevron" size={16} />
        </span>
      </button>
      {open && (
        <div id={panel} className="host-ir-body">
          {inc.escalationReason && (
            <div className={`banner ${needsStaff ? 'banner-warn' : 'banner-info'} host-ir-esc`}>
              <div>
                <strong>{needsStaff ? 'Waiting on staff decision:' : 'Escalation resolved by staff:'}</strong> {inc.escalationReason}
              </div>
              {needsStaff && (
                <button type="button" className="btn btn-primary btn-small" onClick={onResolve}>
                  Approve fix &amp; hand back to Gary
                </button>
              )}
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
            <span className="mono">{shortModel(r.generations[0]?.model ?? '')} · Fireworks</span>
            <span>{r.rounds} rounds</span>
            <span>{fmtMs(r.latency_ms)} model time</span>
            <span>
              {fmtTokens(r.usage.prompt_tokens)} in ({fmtTokens(r.usage.cached_tokens)} cached) · {fmtTokens(r.usage.completion_tokens)} out
            </span>
            <span className="host-cost">
              {fmtUsd(cost)}
              {per1k && <span className="muted"> ({per1k})</span>}
            </span>
            {scenarioId && onCompareArena && (
              <button
                type="button"
                className="cx-link-btn"
                onClick={() => onCompareArena(scenarioId)}
              >
                Compare in Model Arena ↗
              </button>
            )}
          </div>
        </div>
      )}
    </li>
  );
}

export function Incidents() {
  const { incident, openIncident, resolvedIds, resolveIncident, turns, openArena } = useStore();
  const [filter, setFilter] = useState<Filter>('all');
  const sessionIncidents = turns
    .map((t, idx) => sessionTurnToIncident(t, idx))
    .filter((x): x is Incident => x !== null)
    .reverse();
  const allIncidents = [...sessionIncidents, ...INCIDENTS];
  const firstEscalated = allIncidents.find((i) => i.outcome === 'escalated' && !resolvedIds.includes(i.id))?.id ?? allIncidents[0]?.id ?? null;
  const [open, setOpen] = useState<string | null>(incident ?? sessionIncidents[0]?.id ?? firstEscalated);

  useEffect(() => {
    if (!incident) return;
    setOpen(incident);
    document.getElementById(`${incident}-detail`)?.closest('li')?.scrollIntoView({ block: 'center', behavior: 'smooth' });
    openIncident(null);
  }, [incident, openIncident]);

  const outcomeOf = (i: Incident): Outcome => (resolvedIds.includes(i.id) ? 'healthy' : i.outcome);
  const rows = allIncidents.filter((i) => filter === 'all' || outcomeOf(i) === filter);

  return (
    <div className="screen">
      <div className="screen-head host-head">
        <div>
          <h1>Incident audit log</h1>
          <p className="screen-sub">Every ticket Gary took across the fleet, with model telemetry, token economics, exact tool trace, and customer reply.</p>
        </div>
        <Provenance kind="demo" />
      </div>

      <div className="filters" role="group" aria-label="Filter by outcome">
        {FILTERS.map((f) => (
          <button key={f.key} type="button" className={`filter ${filter === f.key ? 'is-on' : ''}`} aria-pressed={filter === f.key} onClick={() => setFilter(f.key)}>
            {f.label}
            <span className="filter-n">{allIncidents.filter((i) => f.key === 'all' || outcomeOf(i) === f.key).length}</span>
          </button>
        ))}
      </div>

      <Card className="host-ir-card">
        <ul className="host-ir-list">
          {rows.map((i) => (
            <IncidentRow
              key={i.id}
              inc={i}
              open={open === i.id}
              resolvedByStaff={resolvedIds.includes(i.id)}
              onToggle={() => setOpen(open === i.id ? null : i.id)}
              onResolve={() => resolveIncident(i.id, `Approved staff action on ${i.server}`)}
              onCompareArena={openArena}
            />
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
