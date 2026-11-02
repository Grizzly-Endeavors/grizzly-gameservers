/**
 * The server owner's console: their server, and Gary. Talking to Gary is the main
 * thing, so the conversation takes the room; the server's vitals and what Gary
 * actually did sit beside it, because "what did it change?" is the first question
 * anyone has about an agent that edits their server.
 *
 * Gary's response carries no card markup. Every card here is read off the tool trace
 * the harness already returns (see gary/derive.ts).
 */
import { useEffect, useRef, useState } from 'react';
import Markdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { GaryMark } from '../app/Shell';
import { Icon } from '../app/icons';
import { useStore, type Turn } from '../app/store';
import { SCENARIOS, type ServerState } from '../data/scenarios';
import { describeTool, fmtMs, fmtTokens, fmtUsd, safetyOf, shortModel, turnCost, type SafetyStage } from '../gary/derive';
import { RATES, RATES_ARE_PLACEHOLDER } from '../gary/rates';
import { Card, Provenance, StatusBadge } from '../ui/ui';

const STARTERS = ['Is the server up?', 'Who is online right now?'];

export function ServerConsole() {
  const { server } = useStore();
  return (
    <div className="screen cx">
      <ServerHeader server={server} />
      <div className="cx-grid">
        <Conversation />
        <aside className="cx-side">
          <Vitals server={server} />
          <ActionLog />
        </aside>
      </div>
    </div>
  );
}

function ServerHeader({ server }: { server: ServerState }) {
  const { breakServer, resetServer, busy, scenario, turns } = useStore();
  const [open, setOpen] = useState(false);
  const menu = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => !menu.current?.contains(e.target as Node) && setOpen(false);
    const esc = (e: KeyboardEvent) => e.key === 'Escape' && setOpen(false);
    window.addEventListener('mousedown', close);
    window.addEventListener('keydown', esc);
    return () => {
      window.removeEventListener('mousedown', close);
      window.removeEventListener('keydown', esc);
    };
  }, [open]);
  return (
    <header className="cx-head">
      <div className="cx-title">
        <h1>mc-friends-smp</h1>
        <StatusBadge status={server.status} />
        <span className="cx-detail">{server.detail}</span>
      </div>
      <div className="cx-demo" ref={menu}>
        <button type="button" className="btn btn-secondary btn-small" aria-haspopup="menu" aria-expanded={open} disabled={busy} onClick={() => setOpen(!open)}>
          <Icon name="bolt" size={15} />
          Break the server
        </button>
        {open && (
          <div className="cx-menu" role="menu">
            <div className="cx-menu-note">Demo: stage a known failure</div>
            {SCENARIOS.map((s, i) => (
              <button
                key={s.id}
                type="button"
                role="menuitem"
                className={`cx-menu-item ${scenario?.id === s.id ? 'is-on' : ''}`}
                onClick={() => {
                  breakServer(s.id);
                  setOpen(false);
                }}
              >
                <span className="cx-menu-n">{i + 1}</span>
                {s.label}
              </button>
            ))}
          </div>
        )}
        {(turns.length > 0 || scenario) && (
          <button type="button" className="icon-btn" onClick={resetServer} disabled={busy} aria-label="Reset the demo" title="Reset the demo">
            <Icon name="reset" size={17} />
          </button>
        )}
      </div>
    </header>
  );
}

function Conversation() {
  const { turns, draft, setDraft, ask, busy } = useStore();
  const end = useRef<HTMLDivElement>(null);
  const last = turns[turns.length - 1];
  useEffect(() => {
    end.current?.scrollIntoView({ behavior: 'smooth', block: 'end' });
  }, [turns.length, last?.steps.length, last?.status]);

  return (
    <section className="cx-chat" aria-label="Conversation with Gary">
      <div className="cx-thread">
        {turns.length === 0 ? (
          <div className="cx-empty">
            <GaryMark size={40} />
            <h2>Hi, I&rsquo;m Gary.</h2>
            <p>I look after this server. Tell me what&rsquo;s wrong in plain English and I&rsquo;ll read the logs, fix it, and check it came back healthy. I ask before touching your world.</p>
            <div className="cx-starters">
              {STARTERS.map((s) => (
                <button key={s} type="button" className="chip" onClick={() => ask(s)}>
                  {s}
                </button>
              ))}
            </div>
          </div>
        ) : (
          turns.map((t) => <TurnView key={t.id} turn={t} />)
        )}
        <div ref={end} />
      </div>
      <form
        className="cx-composer"
        onSubmit={(e) => {
          e.preventDefault();
          ask(draft);
        }}
      >
        <textarea
          id="gary-input"
          rows={1}
          value={draft}
          placeholder="Tell Gary what's going on…"
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && !e.shiftKey) {
              e.preventDefault();
              ask(draft);
            }
          }}
        />
        <button type="submit" className="btn btn-primary" disabled={busy || !draft.trim()}>
          <Icon name="send" size={16} />
          Ask Gary
        </button>
      </form>
    </section>
  );
}

function TurnView({ turn }: { turn: Turn }) {
  return (
    <div className="cx-turn">
      <div className="cx-you">
        <p>{turn.prompt}</p>
      </div>
      <div className="cx-gary">
        <span className="cx-avatar">
          <GaryMark size={28} />
        </span>
        <div className="cx-gary-body">
          {turn.status === 'running' && <Working turn={turn} />}
          {turn.status === 'error' && (
            <div className="banner banner-block">
              <span>Gary couldn&rsquo;t answer: {turn.error}</span>
            </div>
          )}
          {turn.status === 'done' && turn.response && (
            <>
              {turn.response.escalated && (
                <div className="cx-escalation">
                  <Icon name="alert" size={16} />
                  <div>
                    <strong>Handed to staff</strong>
                    <p>{turn.response.escalation_reason ?? 'This needs a person. Your host’s support team has the details.'}</p>
                  </div>
                </div>
              )}
              <div className="cx-reply">
                <Markdown remarkPlugins={[remarkGfm]}>{turn.response.reply}</Markdown>
              </div>
              {turn.approval && <Approval turn={turn} />}
              <Telemetry turn={turn} />
            </>
          )}
        </div>
      </div>
    </div>
  );
}

/** The run under way: each tool as it lands, the newest still turning. */
function Working({ turn }: { turn: Turn }) {
  return (
    <div className="cx-working" aria-live="polite">
      <div className="cx-working-head">
        <span className="cx-dots" aria-hidden="true">
          <i />
          <i />
          <i />
        </span>
        Gary is working
      </div>
      {turn.steps.length > 0 && (
        <ol className="cx-working-steps">
          {turn.steps.map((s, i) => (
            <li key={i} className="cx-step-in">
              <Icon name="check" size={14} />
              {describeTool(s)}
            </li>
          ))}
        </ol>
      )}
    </div>
  );
}

function Approval({ turn }: { turn: Turn }) {
  const { decide, busy } = useStore();
  const sc = SCENARIOS.find((s) => s.id === turn.approval?.scenario);
  if (!sc?.approval || !turn.approval) return null;
  const a = sc.approval;
  const state = turn.approval.state;
  return (
    <div className={`cx-approval is-${state}`}>
      <div className="cx-approval-head">
        <Icon name="lock" size={16} />
        <strong>{a.title}</strong>
      </div>
      <p>{a.blast}</p>
      {state === 'pending' ? (
        <div className="cx-approval-actions">
          <button type="button" className="btn btn-danger" disabled={busy} onClick={() => decide(turn.id, true)}>
            {a.approveLabel}
          </button>
          <button type="button" className="btn btn-secondary" disabled={busy} onClick={() => decide(turn.id, false)}>
            Deny
          </button>
        </div>
      ) : (
        <div className={`cx-approval-done ${state === 'approved' ? 'is-ok' : ''}`}>{state === 'approved' ? 'You approved this.' : 'You said no. Nothing was touched.'}</div>
      )}
    </div>
  );
}

/** One quiet line: which model, how many rounds, how long, how many tokens, what it cost. */
function Telemetry({ turn }: { turn: Turn }) {
  const r = turn.response;
  if (!r) return null;
  const cost = turnCost(r, RATES);
  const model = r.generations[0]?.model;
  return (
    <div className="cx-telemetry">
      <span className="cx-tel-model">{model ? shortModel(model) : 'unknown model'}</span>
      <span>{r.rounds} rounds</span>
      <span>{fmtMs(r.latency_ms)}</span>
      <span title={`${r.usage.prompt_tokens ?? '?'} prompt (${r.usage.cached_tokens ?? '?'} cached) · ${r.usage.completion_tokens ?? '?'} completion`}>
        {fmtTokens(r.usage.prompt_tokens)} in · {fmtTokens(r.usage.completion_tokens)} out
      </span>
      <span title={RATES_ARE_PLACEHOLDER ? 'Placeholder rates until the shared rate table lands' : undefined}>
        {fmtUsd(cost)}
        {cost != null && <span className="muted"> · {fmtUsd(cost * 1000, 2)}/1k</span>}
      </span>
      {turn.replay && !turn.replay.recorded && <Provenance kind="sample">{'Illustrative replay trace'}</Provenance>}
    </div>
  );
}

function Vitals({ server }: { server: ServerState }) {
  const mem = Math.min(1, server.memory[0] / server.memory[1]);
  return (
    <Card title="Server" aside={<StatusBadge status={server.status} />} className={`cx-vitals is-${server.status.toLowerCase()}`}>
      <dl className="cx-stats">
        <div>
          <dt>Players</dt>
          <dd>
            {server.players[0]}
            <span className="muted"> / {server.players[1]}</span>
          </dd>
        </div>
        <div>
          <dt>Memory</dt>
          <dd>
            {server.memory[0].toFixed(1)}
            <span className="muted"> / {server.memory[1]} GB</span>
          </dd>
          <div className="cx-bar" aria-hidden="true">
            <span style={{ width: `${mem * 100}%` }} className={mem > 0.9 ? 'is-hot' : ''} />
          </div>
        </div>
        <div>
          <dt>Restarts</dt>
          <dd>{server.restarts}</dd>
        </div>
      </dl>
      {server.logTail && server.status !== 'Running' && (
        <pre className="cx-log" aria-label="Latest log lines">
          {server.logTail.join('\n')}
        </pre>
      )}
      {server.logTail && server.status === 'Running' && <p className="cx-warnline">{server.logTail[0]}</p>}
    </Card>
  );
}

const STAGES: { key: SafetyStage; label: string }[] = [
  { key: 'snapshot', label: 'Snapshot' },
  { key: 'apply', label: 'Apply fix' },
  { key: 'verify', label: 'Restart & verify' },
  { key: 'done', label: 'Healthy' },
];

/** What Gary did on the latest turn, readable first, raw on request. */
function ActionLog() {
  const { turns } = useStore();
  const turn = [...turns].reverse().find((t) => t.steps.length > 0 || t.status !== 'running') ?? turns[turns.length - 1];
  const safety = safetyOf(turn?.status === 'done' ? turn.response : turn ? { ...emptyResponse, tool_calls: turn.steps } : undefined);
  const steps = turn?.steps ?? [];
  return (
    <Card title="What Gary did" aside={turn && steps.length > 0 ? `${steps.length} step${steps.length === 1 ? '' : 's'}` : undefined} className="cx-log-card">
      {!turn ? (
        <p className="muted cx-log-empty">Every step Gary takes shows up here, with the exact command and what came back.</p>
      ) : (
        <>
          {safety.mutated ? (
            <ol className="cx-stepper" aria-label="Safety loop">
              {STAGES.map((s) => {
                const done = safety.reached.includes(s.key);
                const label = s.key === 'done' && safety.outcome === 'rolled_back' ? 'Rolled back' : s.label;
                return (
                  <li key={s.key} className={done ? 'is-done' : ''}>
                    <span className="cx-stepper-dot">{done && <Icon name="check" size={12} />}</span>
                    {label}
                  </li>
                );
              })}
            </ol>
          ) : (
            turn.status === 'done' && (
              <div className="cx-nochange">
                <Icon name="check" size={15} />
                Read only. Nothing on the server was changed.
              </div>
            )
          )}
          <ol className="cx-steps">
            {steps.map((s, i) => (
              <Step key={i} n={i + 1} name={s.name} line={describeTool(s)} args={s.arguments} result={s.result} />
            ))}
            {turn.status === 'running' && <li className="cx-steps-pending muted">Working…</li>}
          </ol>
        </>
      )}
    </Card>
  );
}

const emptyResponse = {
  reply: '',
  escalated: false,
  escalation_kind: null,
  escalation_reason: null,
  rounds: 0,
  generations: [],
  usage: { prompt_tokens: null, cached_tokens: null, completion_tokens: null, total_tokens: null },
  latency_ms: 0,
  tool_calls: [],
};

function Step({ n, name, line, args, result }: { n: number; name: string; line: string; args: string; result: string }) {
  const [open, setOpen] = useState(false);
  let pretty = args;
  try {
    pretty = JSON.stringify(JSON.parse(args), null, 2);
  } catch {
    // A malformed call stays verbatim, which is the point of showing it.
  }
  return (
    <li className={`cx-stepline ${open ? 'is-open' : ''}`}>
      <button type="button" className="cx-step-btn" aria-expanded={open} onClick={() => setOpen(!open)}>
        <span className="cx-step-n">{n}</span>
        <span className="cx-step-text">{line}</span>
        <span className="cx-step-chev">
          <Icon name="chevron" size={14} />
        </span>
      </button>
      {open && (
        <div className="cx-step-raw">
          <div className="cx-raw-label">
            Tool <span className="mono cx-raw-tool">{name}</span>
          </div>
          <div className="cx-raw-label">Arguments</div>
          <pre>{pretty}</pre>
          <div className="cx-raw-label">Result</div>
          <pre>{result}</pre>
        </div>
      )}
    </li>
  );
}
