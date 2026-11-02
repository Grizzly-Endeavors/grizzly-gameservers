/**
 * The fleet as a support lead scans it: what each customer's server is doing and what
 * Gary last did there. "Needs attention" is the filter that matters; the rest is context
 * for when a customer calls.
 */
import { useState } from 'react';
import { Card, Provenance, StatusBadge } from '../ui/ui';
import { FLEET, agoText, type FleetServer } from './fleet';

type Filter = 'all' | 'attention' | 'running' | 'stopped';

const FILTERS: { key: Filter; label: string; test: (s: FleetServer) => boolean }[] = [
  { key: 'all', label: 'All', test: () => true },
  { key: 'attention', label: 'Needs attention', test: (s) => !!s.attention },
  { key: 'running', label: 'Running', test: (s) => s.status === 'Running' || s.status === 'Starting' },
  { key: 'stopped', label: 'Stopped', test: (s) => s.status === 'Stopped' || s.status === 'Crashed' },
];

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
  const [filter, setFilter] = useState<Filter>('all');
  const rows = FLEET.filter(FILTERS.find((f) => f.key === filter)!.test);

  return (
    <div className="screen">
      <div className="screen-head host-head">
        <div>
          <h1>{FLEET.length} servers, one operator</h1>
          <p className="screen-sub">Every customer server has Gary on it. Two need a person right now.</p>
        </div>
        <Provenance kind="demo" />
      </div>

      <div className="filters" role="group" aria-label="Filter servers">
        {FILTERS.map((f) => (
          <button key={f.key} type="button" className={`filter ${filter === f.key ? 'is-on' : ''}`} aria-pressed={filter === f.key} onClick={() => setFilter(f.key)}>
            {f.label}
            <span className="filter-n">{FLEET.filter(f.test).length}</span>
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
                <th>Gary's last action</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((s) => (
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
                </tr>
              ))}
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
