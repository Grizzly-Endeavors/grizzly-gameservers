/**
 * The hosting team's landing view: the Triage Queue.
 *
 * Gary closes 96% of tickets automatically, so a human support lead opens this console
 * for one reason: what couldn't Gary finish, and what decision is waiting on staff?
 * The escalations sit at the top with the log evidence Gary already gathered and a
 * direct staff action; the 24h auto-resolution telemetry sits in one quiet strip,
 * and Gary's recent autonomous fixes sit in a compact audit table below.
 */
import { useStore } from '../app/store';
import { fmtMs, fmtUsd, turnCost } from '../gary/derive';
import { Card, Provenance } from '../ui/ui';
import { RATES } from './evidence-shared';
import { FLEET, HUMAN_TICKET_USD_ESTIMATE, INCIDENTS, OUTCOME_LABEL, OUTCOME_TONE, TODAY, agoText, type Incident } from './fleet';

/** Mean Gary cost per incident, priced from each turn's own usage. */
function meanGaryCost(): number | null {
  const costs = INCIDENTS.map((i) => turnCost(i.response, RATES));
  if (costs.some((c) => c == null)) return null;
  return (costs as number[]).reduce((a, b) => a + b, 0) / costs.length;
}

const STAFF_ACTION: Record<string, { label: string; toast: string }> = {
  'inc-1039': {
    label: 'Approve Forge 47.3 upgrade',
    toast: 'Approved Forge 47.3.0 loader upgrade on blockbusters',
  },
  'inc-1037': {
    label: 'Purge old backups & retry save',
    toast: 'Cleared 8.7 GB backup archive on ashlands-or-bust',
  },
};

function evidenceLine(inc: Incident): { cmd: string; out: string } | null {
  const call = inc.response.tool_calls.find((t) => t.name === 'run_shell') ?? inc.response.tool_calls.find((t) => t.name !== 'escalate');
  if (!call) return null;
  let detail = call.name;
  try {
    const a = JSON.parse(call.arguments) as Record<string, unknown>;
    if (typeof a.command === 'string') detail = `${call.name} · $ ${a.command}`;
    else if (typeof a.path === 'string') detail = `${call.name} · ${String(a.server ?? '')}:${a.path}`;
    else if (typeof a.server === 'string') detail = `${call.name} · ${a.server}${typeof a.lines === 'number' ? ` (last ${a.lines} lines)` : ''}`;
  } catch {
    detail = call.name;
  }
  return { cmd: detail, out: call.result };
}

export function Overview() {
  const { go, openIncident, resolvedIds, resolveIncident } = useStore();
  const resolved = TODAY.tickets - TODAY.escalated + resolvedIds.length;
  const resolvedPct = ((resolved / TODAY.tickets) * 100).toFixed(1);
  const garyCost = meanGaryCost();

  const escalations = INCIDENTS.filter((i) => i.outcome === 'escalated' && !resolvedIds.includes(i.id));
  const autoHandled = INCIDENTS.filter((i) => i.outcome !== 'escalated' || resolvedIds.includes(i.id));

  const crashedLeft = Math.max(0, FLEET.filter((s) => s.status === 'Crashed').length - resolvedIds.length);
  const runningCount = FLEET.filter((s) => s.status === 'Running' || s.status === 'Starting').length + resolvedIds.length;

  return (
    <div className="screen host-overview">
      <header className="triage-top">
        <div className="triage-title-block">
          <div className="triage-h1-row">
            <h1>
              {escalations.length > 0
                ? `${escalations.length} escalation${escalations.length === 1 ? '' : 's'} waiting on staff decision`
                : 'All staff escalations cleared'}
            </h1>
            <Provenance kind="demo" />
          </div>
          <p className="screen-sub">
            Gary diagnosed both servers, captured the failing logs, and halted before making changes outside his safety policy.
          </p>
        </div>

        <dl className="triage-strip" aria-label="24-hour fleet summary">
          <div>
            <dt>Auto-resolved (24h)</dt>
            <dd>
              <strong>{resolved}</strong> <span className="muted">/ {TODAY.tickets} ({resolvedPct}%)</span>
            </dd>
          </div>
          <div>
            <dt>Median fix time</dt>
            <dd>
              <strong>
                {Math.floor(TODAY.medianFixSeconds / 60)}m {TODAY.medianFixSeconds % 60}s
              </strong>{' '}
              <span className="muted">verified</span>
            </dd>
          </div>
          <div>
            <dt>Mean Gary cost</dt>
            <dd>
              <strong>{fmtUsd(garyCost, 4)}</strong>{' '}
              <span className="muted">
                vs ~${HUMAN_TICKET_USD_ESTIMATE.toFixed(2)} T1/T2 <Provenance kind="estimated" />
              </span>
            </dd>
          </div>
          <div>
            <dt>Fleet health</dt>
            <dd>
              <strong>
                {runningCount}/{FLEET.length}
              </strong>{' '}
              <span className="muted">{crashedLeft > 0 ? `${crashedLeft} halted` : 'all online'}</span>
            </dd>
          </div>
        </dl>
      </header>

      <section className="triage-queue" aria-label="Escalations needing staff">
        {escalations.length === 0 ? (
          <div className="triage-clear">
            <span className="lamp lamp-ok" aria-hidden="true" />
            <div>
              <strong>No escalations waiting on staff.</strong>
              <span className="muted"> Gary is handling incoming tickets across all {FLEET.length} servers.</span>
            </div>
            <button type="button" className="btn btn-secondary btn-small" onClick={() => go('incidents')}>
              Open full incident log
            </button>
          </div>
        ) : (
          <ul className="triage-list">
            {escalations.map((i) => {
              const ev = evidenceLine(i);
              const act = STAFF_ACTION[i.id] ?? { label: 'Resolve & hand back to Gary', toast: `Resolved ${i.server}` };
              const srv = FLEET.find((s) => s.name === i.server);
              return (
                <li key={i.id} className="triage-item">
                  <div className="triage-item-main">
                    <div className="triage-item-head">
                      <span className="badge badge-bad">
                        <span className="lamp lamp-bad" aria-hidden="true" />
                        Halted · Needs staff
                      </span>
                      <strong className="triage-srv">{i.server}</strong>
                      <span className="muted">
                        {i.customer} · {i.game} {srv ? `(${srv.version})` : ''}
                      </span>
                      <span className="triage-ago muted">{agoText(i.ago)}</span>
                    </div>

                    <div className="triage-diag">{i.diagnosis}</div>
                    <p className="triage-reason">{i.escalationReason}</p>

                    {ev && (
                      <div className="triage-evidence">
                        <div className="triage-ev-head">
                          <span className="mono">{ev.cmd}</span>
                          <span className="muted">{i.response.rounds} rounds · {fmtMs(i.response.latency_ms)}</span>
                        </div>
                        <pre className="triage-ev-pre">{ev.out}</pre>
                      </div>
                    )}
                  </div>

                  <div className="triage-item-actions">
                    <button type="button" className="btn btn-primary" onClick={() => resolveIncident(i.id, act.toast)}>
                      {act.label}
                    </button>
                    <button type="button" className="btn btn-secondary btn-small" onClick={() => openIncident(i.id)}>
                      Inspect full trace ({i.response.tool_calls.length} steps)
                    </button>
                  </div>
                </li>
              );
            })}
          </ul>
        )}
      </section>

      <Card
        title="Handled autonomously by Gary"
        aside={
          <div className="triage-audit-aside">
            <span className="muted">Snapshotted before write · verified after restart</span>
            <button type="button" className="btn btn-quiet btn-small" onClick={() => go('incidents')}>
              Full audit log ({INCIDENTS.length})
            </button>
          </div>
        }
        className="triage-audit"
      >
        <div className="tbl-wrap">
          <table className="tbl triage-tbl">
            <thead>
              <tr>
                <th>Server</th>
                <th>Outcome</th>
                <th>Diagnosis &amp; action</th>
                <th className="num">Steps</th>
                <th className="num">Cost</th>
                <th className="num">When</th>
                <th className="num">Trace</th>
              </tr>
            </thead>
            <tbody>
              {autoHandled.map((i) => {
                const staffFixed = resolvedIds.includes(i.id);
                const tone = staffFixed ? 'ok' : OUTCOME_TONE[i.outcome];
                const label = staffFixed ? 'Staff approved · verified' : OUTCOME_LABEL[i.outcome];
                return (
                  <tr key={i.id}>
                    <td>
                      <div className="host-cell-main">{i.server}</div>
                      <div className="host-cell-sub muted">
                        {i.customer} · {i.game}
                      </div>
                    </td>
                    <td>
                      <span className={`badge badge-${tone}`}>
                        <span className={`lamp lamp-${tone}`} aria-hidden="true" />
                        {label}
                      </span>
                    </td>
                    <td className="triage-diag-cell">{i.diagnosis}</td>
                    <td className="num muted">{i.response.tool_calls.length}</td>
                    <td className="num host-cost">{fmtUsd(turnCost(i.response, RATES), 4)}</td>
                    <td className="num muted">{agoText(i.ago)}</td>
                    <td className="num">
                      <button type="button" className="btn btn-quiet btn-small" onClick={() => openIncident(i.id)}>
                        Audit
                      </button>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      </Card>
    </div>
  );
}
