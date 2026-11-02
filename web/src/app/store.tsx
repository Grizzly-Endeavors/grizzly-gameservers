/**
 * App state: who's looking (the hosting company or the server's owner), which screen,
 * Live or Replay, and the owner's one conversation with Gary, who looks after all of
 * their servers from it.
 *
 * Replay plays a scenario's recorded GaryResponse back a tool call at a time, paced by
 * the trace's own latency, so the demo never depends on the venue's network. Live posts
 * to Gary's operator harness through the dev-server proxy (see vite.config.ts).
 */
import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from 'react';
import { SCENARIOS, SERVERS, type OwnedServer, type Scenario, type ServerState } from '../data/scenarios';
import { serversTouched } from '../gary/derive';
import type { GaryRequest, GaryResponse, ToolTrace } from '../gary/types';

export type Persona = 'host' | 'customer';
export type HostScreen = 'overview' | 'servers' | 'incidents' | 'arena' | 'benchmark';
export type CustomerScreen = 'gary' | 'activity';
export type Screen = HostScreen | CustomerScreen;
export type Mode = 'replay' | 'live';

export interface Turn {
  id: string;
  prompt: string;
  at: number;
  status: 'running' | 'done' | 'error';
  /** Tool calls revealed so far while running; the whole list once done. */
  steps: ToolTrace[];
  response?: GaryResponse;
  /** Replay only: the trace came from the bundled scenario, and whether it was recorded. */
  replay?: { scenario: string; recorded: boolean };
  approval?: { scenario: string; state: 'pending' | 'approved' | 'denied' };
  error?: string;
}

interface Store {
  persona: Persona;
  setPersona: (p: Persona) => void;
  screen: Screen;
  go: (s: Screen) => void;
  mode: Mode;
  setMode: (m: Mode) => void;
  servers: OwnedServer[];
  /** The server the side panel is showing: the last one Gary touched, or one clicked. */
  focus: string;
  setFocus: (id: string) => void;
  scenario: Scenario | null;
  breakServer: (id: string) => void;
  resetDemo: () => void;
  draft: string;
  setDraft: (s: string) => void;
  turns: Turn[];
  busy: boolean;
  ask: (prompt: string) => void;
  decide: (turnId: string, approve: boolean) => void;
  toast: string | null;
  showToast: (s: string) => void;
}

const Ctx = createContext<Store | null>(null);

const HOME: Record<Persona, Screen> = { host: 'overview', customer: 'gary' };
const CHANNEL = import.meta.env.VITE_GARY_CHANNEL ?? '';
const LIVE_SLOW_MS = 15_000;
const LIVE_TIMEOUT_MS = 120_000;

let seq = 0;
const nextId = () => `t${Date.now().toString(36)}${(seq++).toString(36)}`;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** A read-only answer for a free-typed question in Replay with nothing broken. */
function statusAnswer(servers: OwnedServer[]): GaryResponse {
  const up = servers.filter((x) => x.state.status === 'Running');
  const line = (x: OwnedServer) => `${x.id} · ${x.state.status} · ${x.state.players[0]}/${x.state.players[1]} players · ${x.state.detail}`;
  const lines = servers.map((x) => `- **${x.id}**: ${x.state.status.toLowerCase()}${x.state.status === 'Running' ? `, ${x.state.players[0]} of ${x.state.players[1]} players on` : ''}`);
  return {
    reply: `${up.length} of your ${servers.length} servers are running and nothing in the recent logs needs attention.\n\n${lines.join('\n')}`,
    escalated: false,
    escalation_kind: null,
    escalation_reason: null,
    rounds: 2,
    generations: [
      { model: 'accounts/fireworks/models/glm-5p2', prompt_tokens: 9010, cached_tokens: 8704, completion_tokens: 61, total_tokens: 9071, finish_reason: 'tool_calls', latency_ms: 318 },
      { model: 'accounts/fireworks/models/glm-5p2', prompt_tokens: 9230, cached_tokens: 8960, completion_tokens: 132, total_tokens: 9362, finish_reason: 'stop', latency_ms: 341 },
    ],
    usage: { prompt_tokens: 18240, cached_tokens: 17664, completion_tokens: 193, total_tokens: 18433 },
    latency_ms: 659,
    tool_calls: [{ name: 'list_servers', arguments: '{}', result: servers.map(line).join('\n') }],
  };
}

export function StoreProvider({ children }: { children: ReactNode }) {
  const [persona, setPersonaState] = useState<Persona>(() => (new URLSearchParams(location.search).get('as') === 'host' ? 'host' : 'customer'));
  const [screen, setScreen] = useState<Screen>(HOME[persona]);
  const [mode, setMode] = useState<Mode>('replay');
  const [servers, setServers] = useState<OwnedServer[]>(SERVERS);
  const [focus, setFocus] = useState<string>(SERVERS[0].id);
  const [scenario, setScenario] = useState<Scenario | null>(null);
  const [draft, setDraft] = useState('');
  const [turns, setTurns] = useState<Turn[]>([]);
  const [toast, setToast] = useState<string | null>(null);
  const toastTimer = useRef<number>();
  const busy = turns.some((t) => t.status === 'running');

  const showToast = useCallback((s: string) => {
    setToast(s);
    window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToast(null), 4200);
  }, []);

  const setPersona = useCallback((p: Persona) => {
    setPersonaState(p);
    setScreen(HOME[p]);
    const u = new URL(location.href);
    u.searchParams.set('as', p);
    history.replaceState(null, '', u);
  }, []);

  const setState = useCallback((id: string, state: ServerState) => setServers((xs) => xs.map((x) => (x.id === id ? { ...x, state } : x))), []);

  const patch = useCallback((id: string, f: (t: Turn) => Turn) => setTurns((ts) => ts.map((t) => (t.id === id ? f(t) : t))), []);

  /** Reveal a response's tool calls one by one, paced by its generations' latency. */
  const play = useCallback(
    async (id: string, r: GaryResponse) => {
      const gens = r.generations;
      for (let i = 0; i < r.tool_calls.length; i++) {
        await sleep(Math.min(1400, Math.max(450, gens[i]?.latency_ms ?? 400) * 1.6));
        patch(id, (t) => ({ ...t, steps: r.tool_calls.slice(0, i + 1) }));
      }
      await sleep(Math.min(1200, Math.max(400, gens[gens.length - 1]?.latency_ms ?? 400)));
    },
    [patch],
  );

  const replay = useCallback(
    async (id: string, prompt: string) => {
      const sc = scenario ?? SCENARIOS.find((s) => s.prompt === prompt) ?? null;
      const r = sc ? sc.response : statusAnswer(servers);
      if (sc) setFocus(sc.server);
      await play(id, r);
      patch(id, (t) => ({
        ...t,
        status: 'done',
        steps: r.tool_calls,
        response: r,
        replay: sc ? { scenario: sc.id, recorded: sc.recorded } : { scenario: 'status', recorded: false },
        approval: sc?.approval ? { scenario: sc.id, state: 'pending' } : undefined,
      }));
      if (sc && !sc.approval) {
        setState(sc.server, sc.after);
        setScenario(null);
      }
    },
    [play, patch, scenario, servers, setState],
  );

  const live = useCallback(
    async (id: string, prompt: string, confirm = false) => {
      const body: GaryRequest = { prompt, new_session: false, channel: CHANNEL, access: 'manager', confirm };
      const ctl = new AbortController();
      const slow = window.setTimeout(() => showToast('Gary is still working. Switch to Replay from the top bar if the network is the problem.'), LIVE_SLOW_MS);
      const kill = window.setTimeout(() => ctl.abort(), LIVE_TIMEOUT_MS);
      try {
        const res = await fetch('/operator/gary', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body), signal: ctl.signal });
        const json: unknown = await res.json().catch(() => null);
        if (!res.ok) throw new Error((json as { error?: string } | null)?.error ?? `Gary's harness answered ${res.status}`);
        const r = json as GaryResponse;
        patch(id, (t) => ({ ...t, status: 'done', steps: r.tool_calls, response: r }));
        const touched = serversTouched(r.tool_calls);
        if (touched.length) setFocus(touched[touched.length - 1]);
      } catch (e) {
        const msg = ctl.signal.aborted ? 'Gary took longer than two minutes.' : e instanceof Error ? e.message : String(e);
        patch(id, (t) => ({ ...t, status: 'error', error: msg }));
      } finally {
        window.clearTimeout(slow);
        window.clearTimeout(kill);
      }
    },
    [patch, showToast],
  );

  const ask = useCallback(
    (prompt: string) => {
      const p = prompt.trim();
      if (!p || busy) return;
      const id = nextId();
      setTurns((ts) => [...ts, { id, prompt: p, at: Date.now(), status: 'running', steps: [] }]);
      setDraft('');
      void (mode === 'live' ? live(id, p) : replay(id, p));
    },
    [busy, mode, live, replay],
  );

  const decide = useCallback(
    (turnId: string, approve: boolean) => {
      const turn = turns.find((t) => t.id === turnId);
      const sc = SCENARIOS.find((s) => s.id === turn?.approval?.scenario);
      if (!turn?.approval || turn.approval.state !== 'pending') return;
      patch(turnId, (t) => ({ ...t, approval: t.approval && { ...t.approval, state: approve ? 'approved' : 'denied' } }));
      const id = nextId();
      const prompt = approve ? 'Approved. Go ahead.' : 'Denied. Don’t touch the world.';
      setTurns((ts) => [...ts, { id, prompt, at: Date.now(), status: 'running', steps: [] }]);
      if (mode === 'live') {
        void live(id, prompt, approve);
        return;
      }
      if (!sc?.approval) return;
      const r = approve ? sc.approval.approved : sc.approval.denied;
      void (async () => {
        await play(id, r);
        patch(id, (t) => ({ ...t, status: 'done', steps: r.tool_calls, response: r, replay: { scenario: sc.id, recorded: sc.recorded } }));
        if (approve) {
          setState(sc.server, sc.after);
          setScenario(null);
        }
      })();
    },
    [turns, mode, patch, play, live, setState],
  );

  const breakServer = useCallback(
    (id: string) => {
      const sc = SCENARIOS.find((s) => s.id === id);
      if (!sc || busy) return;
      setScenario(sc);
      setState(sc.server, sc.before);
      setFocus(sc.server);
      setDraft(sc.prompt);
      requestAnimationFrame(() => document.getElementById('gary-input')?.focus());
    },
    [busy, setState],
  );

  const resetDemo = useCallback(() => {
    setScenario(null);
    setServers(SERVERS);
    setFocus(SERVERS[0].id);
    setTurns([]);
    setDraft('');
  }, []);

  useEffect(() => () => window.clearTimeout(toastTimer.current), []);

  return (
    <Ctx.Provider
      value={{ persona, setPersona, screen, go: setScreen, mode, setMode, servers, focus, setFocus, scenario, breakServer, resetDemo, draft, setDraft, turns, busy, ask, decide, toast, showToast }}
    >
      {children}
    </Ctx.Provider>
  );
}

export function useStore(): Store {
  const s = useContext(Ctx);
  if (!s) throw new Error('useStore must be used within StoreProvider');
  return s;
}
