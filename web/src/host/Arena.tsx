/**
 * Model arena: one incident, the three systems side by side. It shows the per-incident
 * story behind the benchmark, and the ablation column shows what Gary's harness adds
 * over the same open model in a generic agent. The traces are illustrative until
 * recorded runs replace them, and every column says so.
 */
import { useState } from 'react';
import { SCENARIOS, type ArenaRun } from '../data/scenarios';
import { fmtMs, fmtTokens, shortModel } from '../gary/derive';
import { Provenance } from '../ui/ui';
import { RATES, RATES_ARE_PLACEHOLDER, ratio, runCost, usd } from './evidence-shared';

const COLUMN: Record<ArenaRun['system_id'], { title: string; sub: string }> = {
  frontier_cc: { title: 'Frontier model in Claude Code', sub: 'Required baseline' },
  gary_fireworks: { title: "Open model on Fireworks in Gary's harness", sub: 'Our system' },
  ablation_cc: { title: 'Same open model in Claude Code', sub: 'Ablation' },
};

const ORDER: ArenaRun['system_id'][] = ['frontier_cc', 'gary_fireworks', 'ablation_cc'];

function cost(r: ArenaRun) {
  return runCost(r.model, r.prompt_tokens, r.cached_tokens, r.completion_tokens);
}

function Column({ run }: { run: ArenaRun }) {
  const c = cost(run);
  const ours = run.system_id === 'gary_fireworks';
  const rate = RATES[run.model];
  return (
    <section className={`ev-col ${ours ? 'is-ours' : ''}`} aria-label={COLUMN[run.system_id].title}>
      <header className="ev-col-head">
        <span className="ev-col-sub">{COLUMN[run.system_id].sub}</span>
        <h2 className="ev-col-title">{COLUMN[run.system_id].title}</h2>
        <span className="ev-col-model mono" title={run.model}>
          {rate ? `${rate.label} · ${rate.provider}` : shortModel(run.model)}
        </span>
      </header>

      <div className={`badge ${run.verified ? 'badge-ok' : 'badge-bad'} ev-verdict`}>
        <span className={`lamp ${run.verified ? 'lamp-ok' : 'lamp-bad'}`} aria-hidden="true" />
        {run.verified ? 'Verified fixed' : 'Not fixed'}
      </div>

      <dl className="ev-stats">
        <div>
          <dt>Time</dt>
          <dd>{fmtMs(run.latency_ms)}</dd>
        </div>
        <div>
          <dt>Cost</dt>
          <dd>{c == null ? 'unpriced' : usd(c)}</dd>
        </div>
        <div>
          <dt>Rounds</dt>
          <dd>{run.rounds}</dd>
        </div>
      </dl>

      <div className="ev-tokens muted">
        {fmtTokens(run.prompt_tokens)} prompt · {fmtTokens(run.cached_tokens)} cached · {fmtTokens(run.completion_tokens)} out
      </div>

      <ol className="ev-tools" aria-label="Tool calls">
        {run.tools.map((tool, i) => (
          <li key={i} className="mono">
            {tool}
          </li>
        ))}
      </ol>

      <blockquote className="ev-reply">{run.reply}</blockquote>
    </section>
  );
}

export function Arena() {
  const [id, setId] = useState(SCENARIOS[0].id);
  const [ablation, setAblation] = useState(true);
  const sc = SCENARIOS.find((s) => s.id === id) ?? SCENARIOS[0];
  const by = (k: ArenaRun['system_id']) => sc.arena.find((r) => r.system_id === k);
  const gary = by('gary_fireworks');
  const frontier = by('frontier_cc');
  const runs = ORDER.filter((k) => ablation || k !== 'ablation_cc')
    .map(by)
    .filter((r): r is ArenaRun => !!r);

  const gc = gary && cost(gary);
  const fc = frontier && cost(frontier);

  return (
    <div className="screen ev-screen">
      <div className="screen-head">
        <h1>Model arena</h1>
        <p className="screen-sub">The same player complaint, handled by each system. Same server state, same tools, same prompt.</p>
      </div>

      <div className="ev-toolbar">
        <div className="filters" role="group" aria-label="Incident">
          {SCENARIOS.map((s) => (
            <button key={s.id} type="button" className={`filter ${s.id === id ? 'is-on' : ''}`} aria-pressed={s.id === id} onClick={() => setId(s.id)}>
              {s.label}
            </button>
          ))}
        </div>
        <label className="ev-toggle">
          <input type="checkbox" checked={ablation} onChange={(e) => setAblation(e.target.checked)} />
          Show ablation
        </label>
      </div>

      <p className="ev-prompt">
        <span className="muted">Player:</span> &ldquo;{sc.prompt}&rdquo;
      </p>

      {gary && frontier && (
        <div className="ev-deltas" aria-label="Gary compared with the frontier baseline">
          <div className="ev-delta">
            <strong>{ratio(frontier.latency_ms, gary.latency_ms)}</strong> faster
          </div>
          {gc != null && fc != null && gc > 0 && (
            <div className="ev-delta">
              <strong>{ratio(fc, gc)}</strong> cheaper
            </div>
          )}
          <div className="ev-delta">
            <strong>{frontier.rounds - gary.rounds}</strong> fewer rounds
          </div>
          <span className="ev-deltas-note muted">
            Gary vs the frontier baseline {!sc.recorded && <Provenance kind="sample">Illustrative trace; replaced by recorded runs</Provenance>}
          </span>
        </div>
      )}

      <div className={`ev-cols ev-cols-${runs.length}`}>
        {runs.map((r) => (
          <Column key={r.system_id} run={r} />
        ))}
      </div>

      <footer className="ev-foot">
        {!sc.recorded && <Provenance kind="sample">Illustrative trace; replaced by recorded runs</Provenance>}
        {!sc.recorded && <span className="muted">Illustrative trace; replaced by recorded runs.</span>}
        {RATES_ARE_PLACEHOLDER && <span className="muted">Costs use placeholder rates until the shared rate table lands.</span>}
      </footer>
    </div>
  );
}
