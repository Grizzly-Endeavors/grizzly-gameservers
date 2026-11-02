/**
 * The hosting team's landing view. The point it has to make in one glance: Gary closes
 * almost every ticket himself, for a fraction of a cent, so staff only see the few he
 * hands over. Four numbers, then the two lists a support lead actually acts on.
 */
import { useStore } from '../app/store';
import { fmtUsd, turnCost } from '../gary/derive';
import { Card, Kpi, Provenance } from '../ui/ui';
import { RATES } from './evidence-shared';
import { HUMAN_TICKET_USD_ESTIMATE, INCIDENTS, OUTCOME_LABEL, OUTCOME_TONE, TODAY, agoText } from './fleet';

/** Mean Gary cost per incident, priced from each turn's own usage. */
function meanGaryCost(): number | null {
  const costs = INCIDENTS.map((i) => turnCost(i.response, RATES));
  if (costs.some((c) => c == null)) return null;
  return (costs as number[]).reduce((a, b) => a + b, 0) / costs.length;
}

export function Overview() {
  const { go } = useStore();
  const resolved = TODAY.tickets - TODAY.escalated;
  const garyCost = meanGaryCost();
  const escalations = INCIDENTS.filter((i) => i.outcome === 'escalated');
  const recent = INCIDENTS.filter((i) => i.outcome !== 'escalated').slice(0, 6);

  return (
    <div className="screen host-overview">
      <div className="screen-head host-head">
        <div>
          <h1>Gary closed {resolved} of {TODAY.tickets} tickets today</h1>
          <p className="screen-sub">Staff only see what Gary hands over. Everything else was diagnosed, fixed and verified on the server.</p>
        </div>
        <Provenance kind="demo" />
      </div>

      <div className="host-kpis">
        <Kpi label="Tickets resolved by Gary" value={resolved} sub={`of ${TODAY.tickets} today`} tone="brand" />
        <Kpi label="Resolved without staff" value={`${((resolved / TODAY.tickets) * 100).toFixed(1)}%`} sub={`${TODAY.escalated} escalated`} tone="ok" />
        <Kpi label="Median time to fix" value={`${Math.floor(TODAY.medianFixSeconds / 60)}m ${TODAY.medianFixSeconds % 60}s`} sub="complaint to verified healthy" />
        <Kpi
          label="Cost per ticket"
          value={fmtUsd(garyCost, 4)}
          sub={
            <>
              vs ~${HUMAN_TICKET_USD_ESTIMATE.toFixed(2)} for a human ticket <Provenance kind="estimated" />
            </>
          }
        />
      </div>

      <div className="host-pair">
        <Card
          title="Needs staff"
          aside={
            <span className="badge badge-bad" aria-label={`${escalations.length} waiting`}>
              {escalations.length}
            </span>
          }
        >
          <ul className="host-list">
            {escalations.map((i) => (
              <li key={i.id} className="host-esc">
                <div className="host-esc-top">
                  <span className="lamp lamp-bad" aria-hidden="true" />
                  <strong>{i.server}</strong>
                  <span className="muted">
                    {i.customer} · {i.game}
                  </span>
                  <span className="host-ago muted">{agoText(i.ago)}</span>
                </div>
                <p className="host-esc-reason">{i.escalationReason}</p>
                <div>
                  <button type="button" className="btn btn-secondary btn-small" onClick={() => go('incidents')}>
                    Open
                  </button>
                </div>
              </li>
            ))}
          </ul>
        </Card>

        <Card
          title="Recent incidents"
          aside={
            <button type="button" className="btn btn-quiet btn-small" onClick={() => go('incidents')}>
              See all
            </button>
          }
        >
          <ul className="host-list">
            {recent.map((i) => (
              <li key={i.id} className="host-inc">
                <span className={`lamp lamp-${OUTCOME_TONE[i.outcome]}`} aria-hidden="true" />
                <div className="host-inc-main">
                  <div className="host-inc-line">
                    <strong>{i.server}</strong>
                    <span className="muted">{OUTCOME_LABEL[i.outcome]}</span>
                  </div>
                  <div className="host-inc-diag">{i.diagnosis}</div>
                </div>
                <div className="host-inc-side">
                  <span className="muted">{agoText(i.ago)}</span>
                  <span className="host-cost">{fmtUsd(turnCost(i.response, RATES), 4)}</span>
                </div>
              </li>
            ))}
          </ul>
        </Card>
      </div>
    </div>
  );
}
