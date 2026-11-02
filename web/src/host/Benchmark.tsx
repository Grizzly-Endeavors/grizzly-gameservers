/**
 * The benchmark: Gary on an open model against the frontier baseline on the same
 * held-out cases. It carries the judging's heaviest criterion, so every row says
 * whether it was measured or estimated, and the bundled file says plainly when it is
 * only a placeholder. Swap src/data/eval_results.sample.json for the eval's output.
 */
import { Fragment, useMemo, useState } from 'react';
import { Bar, BarChart, CartesianGrid, Cell, ErrorBar, LabelList, ResponsiveContainer, Scatter, ScatterChart, Tooltip, XAxis, YAxis } from 'recharts';
import results from '../data/eval_results.sample.json';
import type { EvalCase, EvalResults, EvalSystem } from '../gary/types';
import { Card, Kpi, Provenance } from '../ui/ui';
import { dur, pct, ratio, usd } from './evidence-shared';

const DATA = results as unknown as EvalResults;
const OURS = 'gary_fireworks';
const BASELINE = 'frontier_cc';

/** A short name for chart axes, where the full label doesn't fit. */
const SHORT: Record<string, string> = {
  frontier_cc: 'Frontier · Claude Code',
  gary_fireworks: 'Gary · Fireworks',
  ablation_cc: 'Open model · Claude Code',
  human_t1t2: 'Human T1/T2',
};
const shortLabel = (s: EvalSystem) => SHORT[s.system_id] ?? s.label;

/** A placeholder file's "measured" rows were never measured, so they don't get to say so. */
const provOf = (s: EvalSystem) => (DATA.sample && s.kind === 'measured' ? 'sample' : s.kind);

function colorOf(s: EvalSystem): string {
  if (s.system_id === OURS) return 'var(--primary-bg)';
  if (s.kind === 'estimated') return 'var(--lamp-warn)';
  return 'var(--faint)';
}

const tooltipStyle = {
  contentStyle: { background: 'var(--raised)', border: '1px solid var(--line)', borderRadius: 8, color: 'var(--ink-strong)', fontSize: 13 },
  itemStyle: { color: 'var(--ink)' },
  labelStyle: { color: 'var(--ink-strong)', fontWeight: 600 },
};

function SuccessChart({ systems }: { systems: EvalSystem[] }) {
  const rows = systems.map((s) => ({
    name: shortLabel(s),
    rate: +(s.verified_success_rate * 100).toFixed(1),
    err: [+((s.verified_success_rate - s.success_ci_95[0]) * 100).toFixed(1), +((s.success_ci_95[1] - s.verified_success_rate) * 100).toFixed(1)],
    fill: colorOf(s),
  }));
  return (
    <div className="ev-chart" style={{ height: 56 * rows.length + 40 }}>
      <ResponsiveContainer width="100%" height="100%">
        <BarChart data={rows} layout="vertical" margin={{ top: 4, right: 48, bottom: 4, left: 8 }}>
          <CartesianGrid horizontal={false} stroke="var(--line-soft)" />
          <XAxis type="number" domain={[0, 100]} tickFormatter={(v: number) => `${v}%`} stroke="var(--faint)" tick={{ fill: 'var(--muted)', fontSize: 12 }} />
          <YAxis type="category" dataKey="name" width={150} stroke="var(--faint)" tick={{ fill: 'var(--ink)', fontSize: 12.5 }} />
          <Tooltip {...tooltipStyle} cursor={{ fill: 'var(--hover)' }} formatter={(v) => [`${String(v)}%`, 'Verified success']} />
          <Bar dataKey="rate" barSize={18} radius={[0, 4, 4, 0]} isAnimationActive={false}>
            {rows.map((r) => (
              <Cell key={r.name} fill={r.fill} />
            ))}
            <ErrorBar dataKey="err" direction="x" width={6} strokeWidth={1.5} stroke="var(--ink-strong)" />
            <LabelList dataKey="rate" position="right" offset={26} formatter={(v) => `${String(v)}%`} style={{ fill: 'var(--ink)', fontSize: 12 }} />
          </Bar>
        </BarChart>
      </ResponsiveContainer>
    </div>
  );
}

/** Log-axis bounds padded by half a decade each way, so no point sits on an edge. */
function logDomain(values: number[]): [number, number] {
  const lo = Math.min(...values);
  const hi = Math.max(...values);
  return [lo / Math.sqrt(10), hi * Math.sqrt(10)];
}

/** Ticks at round values inside a log domain: powers of ten, or the given candidates. */
function logTicks([lo, hi]: [number, number], candidates?: number[]): number[] {
  if (candidates) return candidates.filter((v) => v >= lo && v <= hi);
  const out: number[] = [];
  for (let k = Math.ceil(Math.log10(lo)); 10 ** k <= hi; k++) out.push(10 ** k);
  return out;
}

const SECONDS = [1, 10, 60, 600, 3600, 36_000, 86_400];

function CostLatencyChart({ systems }: { systems: EvalSystem[] }) {
  const points = systems.map((s) => ({ name: shortLabel(s), x: s.latency_p50_ms / 1000, y: s.cost_per_1k_usd, fill: colorOf(s) }));
  const xd = logDomain(points.map((p) => p.x));
  const yd = logDomain(points.map((p) => p.y));
  const maxX = Math.max(...points.map((p) => p.x));
  // One line per label, beside its dot; the right-most point's label sits on its left so it never runs off the chart.
  const label = (props: { x?: number | string; y?: number | string; value?: unknown; index?: number }) => {
    const p = points[props.index ?? 0];
    const right = p?.x === maxX;
    return (
      <text x={Number(props.x) + (right ? -12 : 12)} y={Number(props.y) + 4} textAnchor={right ? 'end' : 'start'} fill="var(--ink)" fontSize={12}>
        {String(props.value)}
      </text>
    );
  };
  return (
    <div className="ev-chart" style={{ height: 300 }}>
      <ResponsiveContainer width="100%" height="100%">
        <ScatterChart margin={{ top: 16, right: 24, bottom: 20, left: 8 }}>
          <CartesianGrid stroke="var(--line-soft)" />
          <XAxis
            type="number"
            dataKey="x"
            name="p50 latency"
            scale="log"
            domain={xd}
            ticks={logTicks(xd, SECONDS)}
            allowDataOverflow
            tickFormatter={(v: number) => dur(v * 1000)}
            stroke="var(--faint)"
            tick={{ fill: 'var(--muted)', fontSize: 12 }}
            label={{ value: 'p50 time to resolution (log)', position: 'insideBottom', offset: -12, fill: 'var(--muted)', fontSize: 12 }}
          />
          <YAxis
            type="number"
            dataKey="y"
            name="Cost per 1,000"
            scale="log"
            domain={yd}
            ticks={logTicks(yd)}
            allowDataOverflow
            tickFormatter={(v: number) => usd(v)}
            stroke="var(--faint)"
            tick={{ fill: 'var(--muted)', fontSize: 12 }}
            width={64}
          />
          <Tooltip
            {...tooltipStyle}
            cursor={{ stroke: 'var(--line-strong)' }}
            formatter={(v, n) => [n === 'p50 latency' ? dur(Number(v) * 1000) : usd(Number(v)), String(n)]}
          />
          <Scatter data={points} isAnimationActive={false}>
            {points.map((p) => (
              <Cell key={p.name} fill={p.fill} />
            ))}
            <LabelList dataKey="name" content={label} />
          </Scatter>
        </ScatterChart>
      </ResponsiveContainer>
    </div>
  );
}

function SystemsTable({ systems }: { systems: EvalSystem[] }) {
  return (
    <div className="tbl-wrap">
      <table className="tbl ev-systems">
        <thead>
          <tr>
            <th>System</th>
            <th>Source</th>
            <th className="num">Verified success</th>
            <th className="num">95% CI</th>
            <th className="num">Unintended changes</th>
            <th className="num">Mean turns</th>
            <th className="num">p50</th>
            <th className="num">p95</th>
            <th className="num">Cost / 1,000</th>
          </tr>
        </thead>
        <tbody>
          {systems.map((s) => (
            <tr key={s.system_id} className={s.system_id === OURS ? 'is-ours' : ''}>
              <td>
                <div className="ev-sys-name">{s.label}</div>
                {s.notes && <div className="muted ev-sys-note">{s.notes}</div>}
              </td>
              <td>
                <Provenance kind={provOf(s)} />
              </td>
              <td className="num">{pct(s.verified_success_rate, 1)}</td>
              <td className="num muted">{s.kind === 'measured' ? `${pct(s.success_ci_95[0], 1)} to ${pct(s.success_ci_95[1], 1)}` : 'n/a'}</td>
              <td className="num">{s.kind === 'measured' ? pct(s.unintended_change_rate, 1) : 'n/a'}</td>
              <td className="num">{s.mean_turns ?? 'n/a'}</td>
              <td className="num">{dur(s.latency_p50_ms)}</td>
              <td className="num">{dur(s.latency_p95_ms)}</td>
              <td className="num">{usd(s.cost_per_1k_usd)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

type Area = 'all' | EvalCase['task_area'];

function CaseInspector({ cases, measured }: { cases: EvalCase[]; measured: EvalSystem[] }) {
  const [area, setArea] = useState<Area>('all');
  const [open, setOpen] = useState<string | null>(null);
  const shown = cases.filter((c) => area === 'all' || c.task_area === area);
  const n = (a: Area) => cases.filter((c) => a === 'all' || c.task_area === a).length;
  return (
    <>
      <div className="filters" role="group" aria-label="Task area">
        {(['all', 'troubleshooting', 'config'] as Area[]).map((a) => (
          <button key={a} type="button" className={`filter ${area === a ? 'is-on' : ''}`} aria-pressed={area === a} onClick={() => setArea(a)}>
            {a === 'all' ? 'All' : a === 'config' ? 'Config changes' : 'Troubleshooting'}
            <span className="filter-n">{n(a)}</span>
          </button>
        ))}
      </div>
      <div className="tbl-wrap ev-cases">
        <table className="tbl">
          <thead>
            <tr>
              <th>Case</th>
              <th>Game</th>
              <th>Category</th>
              {measured.map((s) => (
                <th key={s.system_id} className="ev-mark-col">
                  {shortLabel(s)}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {shown.map((c) => (
              <Fragment key={c.case_id}>
                <tr className={`ev-case-row ${open === c.case_id ? 'is-open' : ''}`} onClick={() => setOpen(open === c.case_id ? null : c.case_id)}>
                  <td>
                    <button type="button" className="ev-case-btn mono" aria-expanded={open === c.case_id}>
                      {c.case_id}
                    </button>
                  </td>
                  <td>{c.game}</td>
                  <td className="mono">{c.category}</td>
                  {measured.map((s) => {
                    const r = c.results_by_system[s.system_id];
                    return (
                      <td key={s.system_id} className="ev-mark-col">
                        {r ? <span className={r.verified_success ? 'ev-ok' : 'ev-bad'}>{r.verified_success ? '✓' : '✗'}</span> : <span className="muted">–</span>}
                      </td>
                    );
                  })}
                </tr>
                {open === c.case_id && (
                  <tr className="ev-case-detail">
                    <td colSpan={3 + measured.length}>
                      <p className="ev-case-prompt">
                        <span className="muted">Prompt:</span> {c.prompt}
                      </p>
                      <div className="ev-case-grid">
                        {measured.map((s) => {
                          const r = c.results_by_system[s.system_id];
                          if (!r) return null;
                          return (
                            <div key={s.system_id} className={`ev-case-sys ${s.system_id === OURS ? 'is-ours' : ''}`}>
                              <div className="ev-case-sys-name">{shortLabel(s)}</div>
                              <div>{r.verified_success ? 'Verified fixed' : 'Not fixed'}</div>
                              <div className="muted">
                                {r.turns} turns · {dur(r.latency_ms)} · {usd(r.cost_usd)}
                                {r.unintended_changes > 0 && ` · ${r.unintended_changes} unintended change${r.unintended_changes > 1 ? 's' : ''}`}
                              </div>
                            </div>
                          );
                        })}
                      </div>
                    </td>
                  </tr>
                )}
              </Fragment>
            ))}
          </tbody>
        </table>
      </div>
    </>
  );
}

export function Benchmark() {
  const systems = DATA.summary.systems;
  const measured = useMemo(() => systems.filter((s) => s.kind === 'measured'), [systems]);
  const ours = systems.find((s) => s.system_id === OURS);
  const base = systems.find((s) => s.system_id === BASELINE);
  const n = Math.max(0, ...measured.map((s) => s.n_cases));

  return (
    <div className="screen ev-screen">
      <div className="screen-head">
        <h1>Benchmark (N={n})</h1>
        <p className="screen-sub">Verified recovery on held-out support cases. Success means the server came back healthy and the intended change took effect.</p>
      </div>

      {DATA.sample && (
        <div className="banners">
          <div className="banner banner-warn" role="note">
            <strong>Sample data.</strong> These numbers are placeholders until the held-out eval run lands.
          </div>
        </div>
      )}

      {ours && base && (
        <div className="ev-kpis">
          <Kpi
            label="Verified success"
            tone="brand"
            value={pct(ours.verified_success_rate)}
            sub={
              <>
                95% CI {pct(ours.success_ci_95[0])} to {pct(ours.success_ci_95[1])}. Frontier {pct(base.verified_success_rate)} ({pct(base.success_ci_95[0])} to{' '}
                {pct(base.success_ci_95[1])})
              </>
            }
          />
          <Kpi
            label="Cost per 1,000 incidents"
            tone="brand"
            value={usd(ours.cost_per_1k_usd)}
            sub={
              <>
                Frontier {usd(base.cost_per_1k_usd)}, {ratio(base.cost_per_1k_usd, ours.cost_per_1k_usd)} more
              </>
            }
          />
          <Kpi
            label="p50 time to resolution"
            tone="brand"
            value={dur(ours.latency_p50_ms)}
            sub={
              <>
                Frontier {dur(base.latency_p50_ms)}, {ratio(base.latency_p50_ms, ours.latency_p50_ms)} slower
              </>
            }
          />
        </div>
      )}
      <div className="ev-kpi-prov">
        <Provenance kind={DATA.sample ? 'sample' : 'measured'} />
        <span className="muted">Gary on Fireworks vs the frontier model in Claude Code, same {n} cases, same scoring.</span>
      </div>

      <div className="ev-charts">
        <Card title="Verified success rate" aside={<span>with 95% confidence interval</span>}>
          <SuccessChart systems={systems} />
          <ul className="ev-legend">
            {systems.map((s) => (
              <li key={s.system_id}>
                <span className="ev-swatch" style={{ background: colorOf(s) }} aria-hidden="true" />
                {shortLabel(s)} <Provenance kind={provOf(s)} />
              </li>
            ))}
          </ul>
        </Card>
        <Card title="Cost vs time" aside={<span>lower left is better</span>}>
          <CostLatencyChart systems={systems} />
        </Card>
      </div>

      <Card title="All systems" className="ev-block">
        <SystemsTable systems={systems} />
      </Card>

      <Card title="Case inspector" aside={<span>{DATA.cases.length} cases</span>} className="ev-block">
        <CaseInspector cases={DATA.cases} measured={measured} />
      </Card>
    </div>
  );
}
