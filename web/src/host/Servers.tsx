/**
 * The fleet as a support lead scans it: servers needing a human sit at the top of the
 * table with a direct link into the halted escalation; healthy servers sit below with
 * their last verified Gary action.
 */
import { useState } from 'react';
import { useStore } from '../app/store';
import { Card, Provenance, StatusBadge } from '../ui/ui';
import { FLEET, INCIDENTS, agoText, type FleetServer } from './fleet';

type Filter = 'all' | 'attention' | 'running' | 'stopped';

function MemoryBar({ used, limit }: { used: number; limit: number }) {
  const pct = limit ? Math.min(100, (used / limit) * 100) : 0;
  const tone = pct >= 90 ? 'bad' : pct >= 75 ? 'warn' : 'ok';
  return (
    <div className="host-mem" title={`${used} of ${limit} GB`}>
      <div className="host-mem-track" aria-hidden="true">
        <span className={`host-mem-fill host-mem-${tone}`} style={{ width: `${pct}%` }} />
      </div>
      <span className="host-mem-text">
        {used.toFixed(1)} / {limit} GB
      </span>
    </div>
  );
}

export function Servers() {
  const { openIncident, resolvedIds, go } = useStore();
  const [filter, setFilter] = useState<Filter>('all');

  const fleet: FleetServer[] = FLEET.map((s) => {
    const esc = INCIDENTS.find((i) => i.server === s.name && i.outcome === 'escalated');
    if (esc && resolvedIds.includes(esc.id)) {
      return { ...s, status: 'Running' as const, attention: false, lastAction: 'Staff approved fix; Gary verified healthy', ago: 1 };
    }
    return s;
  }).sort((a, b) => Number(!!b.attention) - Number(!!a.attention));

  const filters: { key: Filter; label: string; test: (s: FleetServer) => boolean }[] = [
    { key: 'all', label: 'All servers (attention first)', test: () => true },
    { key: 'attention', label: 'Needs staff', test: (s) => !!s.attention },
    { key: 'running', label: 'Running', test: (s) => s.status === 'Running' || s.status === 'Starting' },
    { key: 'stopped', label: 'Stopped', test: (s) => s.status === 'Stopped' || s.status === 'Crashed' },
  ];

  const rows = fleet.filter(filters.find((f) => f.key === filter)!.test);
  const attnCount = fleet.filter((s) => s.attention).length;

  return (
    <div className="screen">
      <div className="screen-head host-head">
        <div>
          <h1>
            { attnCount > 0 ? `${attnCount} of ${fleet.length} servers waiting on staff` : `All ${fleet.length} fleet servers healthy or idle` }
          </h1>
          <p className="screen-sub">
            Servers halted for a staff decision are pinned to the top. Every other server is managed autonomously by Gary.
          </p>
        </div>
        <div className="host-head-right">
          {attnCount > 0 && (
            <button type="button" className="btn btn-primary btn-small" onClick={() => go('overview')}>
              Open triage queue ({attnCount})
            </button>
          )}
          <Provenance kind="demo" />
        </div>
      </div>

      <div className="filters" role="group" aria-label="Filter servers">
        {filters.map((f) => (
          <button key={f.key} type="button" className={`filter ${filter === f.key ? 'is-on' : ''}`} aria-pressed={filter === f.key} onClick={() => setFilter(f.key)}>
            {f.label}
            <span className="filter-n">{fleet.filter(f.test).length}</span>
          </button>
        ))}
      </div>

      <Card>
        <div className="tbl-wrap">
          <table className="tbl host-fleet">
            <thead>
              <tr>
                <th>Server</th>
                <th>Game</th>
                <th>Status</th>
                <th className="num">Players</th>
                <th>Memory</th>
                <th>Gary&rsquo;s last action</th>
                <th className="num">Action</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((s) => {
                const inc = INCIDENTS.find((i) => i.server === s.name);
                return (
                  <tr key={s.name} className={s.attention ? 'host-row-attn' : ''}>
                    <td>
                      <div className="host-cell-main">{s.name}</div>
                      <div className="host-cell-sub muted">{s.customer}</div>
                    </td>
                    <td>
                      <div className="host-cell-main">{s.game}</div>
                      <div className="host-cell-sub muted">{s.version}</div>
                    </td>
                    <td>
                      <StatusBadge status={s.status} />
                    </td>
                    <td className="num">
                      {s.players[0]} / {s.players[1]}
                    </td>
                    <td>
                      <MemoryBar used={s.memory[0]} limit={s.memory[1]} />
                    </td>
                    <td>
                      <div className="host-cell-main">{s.lastAction}</div>
                      <div className="host-cell-sub muted">{agoText(s.ago)}</div>
                    </td>
                    <td className="num">
                      {s.attention && inc ? (
                        <button type="button" className="btn btn-primary btn-small" onClick={() => openIncident(inc.id)}>
                          Review escalation
                        </button>
                      ) : inc ? (
                        <button type="button" className="btn btn-quiet btn-small" onClick={() => openIncident(inc.id)}>
                          Inspect trace
                        </button>
                      ) : (
                        <span className="muted">Idle</span>
                      )}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
          {rows.length === 0 && (
            <div className="empty">
              <div className="empty-title">Nothing here</div>
              <div className="empty-body">No servers match this filter.</div>
            </div>
          )}
        </div>
      </Card>
    </div>
  );
}
