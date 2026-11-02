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
import { SCENARIOS, SERVERS, type OwnedServer, type Scenario, type ServerState, type ServerStatus } from '../data/scenarios';
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
  /** Sent from the previous run's follow-up field, so it reads as a continuation of that run. */
  followUp?: boolean;
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
  ask: (prompt: string, opts?: { followUp?: boolean }) => void;
  decide: (turnId: string, approve: boolean) => void;
  toast: string | null;
  showToast: (s: string) => void;
  /** The incident the Incidents screen should open with, set by "Open" elsewhere. */
  incident: string | null;
  openIncident: (id: string | null) => void;
  /** Scenario ID selected in the Model Arena, settable from cross-screen links. */
  arenaScenario: string;
  openArena: (scenarioId: string) => void;
  /** Escalation incident IDs resolved by staff during the session. */
  resolvedIds: string[];
  resolveIncident: (id: string, actionLabel: string) => void;
}

const Ctx = createContext<Store | null>(null);

const HOME: Record<Persona, Screen> = { host: 'overview', customer: 'gary' };
/** The deployed console gets its channel at pod start (public/runtime-config.js); dev gets it from web/.env. */
const CHANNEL = window.__GARY__?.channel || import.meta.env.VITE_GARY_CHANNEL || '';
const LIVE_SLOW_MS = 15_000;
/** Same ceiling as scripts/operator.sh: a 16-round turn can legitimately run for minutes. */
const LIVE_TIMEOUT_MS = 900_000;

/**
 * POST to Gary's operator harness. A 503 means the bot's Discord gateway is still
 * connecting after a rollout, which clears in seconds, so it is retried like the
 * operator script does.
 */
async function harness<T>(path: string, body: unknown, signal?: AbortSignal): Promise<T> {
  for (let attempt = 1; ; attempt++) {
    const res = await fetch(path, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body), signal });
    if (res.status === 503 && attempt < 3) {
      await sleep(3000);
      continue;
    }
    const json: unknown = await res.json().catch(() => null);
    if (!res.ok) throw new Error((json as { error?: string } | null)?.error ?? `Gary's harness answered ${res.status}`);
    return json as T;
  }
}

const LIVE_STATUS: Record<string, ServerStatus> = {
  running: 'Running',
  ready: 'Running',
  starting: 'Starting',
  stopped: 'Stopped',
  paused: 'Stopped',
  crashed: 'Crashed',
  unhealthy: 'Crashed',
};

/** Parse list_servers' "{name} (game: {game}, state: {state}, address: {address})" lines. */
export function parseServerList(text: string): OwnedServer[] {
  const out: OwnedServer[] = [];
  for (const m of text.matchAll(/^\W*(\S+) \(game: ([^,]+), state: ([^,]+), address: ([^)]+)\)/gm)) {
    const [, id, game, state, address] = m;
    out.push({
      id,
      game,
      live: true,
      state: { status: LIVE_STATUS[state.trim().toLowerCase()] ?? 'Stopped', detail: address.trim(), players: [0, 0], memory: [0, 0], restarts: 0 },
    });
  }
  return out;
}

let seq = 0;
const nextId = () => `t${Date.now().toString(36)}${(seq++).toString(36)}`;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** A read-only answer for a free-typed question in Replay with nothing broken. */
function statusAnswer(servers: OwnedServer[]): GaryResponse {
  const up = servers.filter((x) => x.state.status === 'Running');
  const line = (x: OwnedServer) => `${x.id} · ${x.state.status} · ${x.state.players[0]}/${x.state.players[1]} players · ${x.state.detail}`;
  const lines = servers.map(
    (x) => `- **${x.id}**: ${x.state.status.toLowerCase()}${x.state.status === 'Running' ? `, ${x.state.players[0]} of ${x.state.players[1]} players on` : ''}`,
  );
  return {
    reply: `${up.length} of your ${servers.length} servers are running and nothing in the recent logs needs attention.\n\n${lines.join('\n')}`,
    escalated: false,
    escalation_kind: null,
    escalation_reason: null,
    rounds: 2,
    generations: [
      {
        model: 'accounts/fireworks/models/glm-5p3-flash',
        prompt_tokens: 9010,
        cached_tokens: 8704,
        completion_tokens: 61,
        total_tokens: 9071,
        finish_reason: 'tool_calls',
        latency_ms: 318,
      },
      {
        model: 'accounts/fireworks/models/glm-5p3-flash',
        prompt_tokens: 9230,
        cached_tokens: 8960,
        completion_tokens: 132,
        total_tokens: 9362,
        finish_reason: 'stop',
        latency_ms: 341,
      },
    ],
    usage: { prompt_tokens: 18240, cached_tokens: 17664, completion_tokens: 193, total_tokens: 18433 },
    latency_ms: 659,
    tool_calls: [{ name: 'list_servers', arguments: '{}', result: servers.map(line).join('\n') }],
  };
}

/** Replay trace for a Discord friend request relayed from a bound server channel. */
function discordFriendAnswer(serverId: string): GaryResponse {
  return {
    reply: `Whitelisted \`sara_v\` on **${serverId}** (13 members). Checked tick health: **20.0 TPS**, 0 lag spikes in the last 30m.`,
    escalated: false,
    escalation_kind: null,
    escalation_reason: null,
    rounds: 3,
    generations: [
      {
        model: 'accounts/fireworks/models/glm-5p3-flash',
        prompt_tokens: 9120,
        cached_tokens: 8832,
        completion_tokens: 84,
        total_tokens: 9204,
        finish_reason: 'tool_calls',
        latency_ms: 295,
      },
      {
        model: 'accounts/fireworks/models/glm-5p3-flash',
        prompt_tokens: 9440,
        cached_tokens: 9088,
        completion_tokens: 72,
        total_tokens: 9512,
        finish_reason: 'tool_calls',
        latency_ms: 280,
      },
      {
        model: 'accounts/fireworks/models/glm-5p3-flash',
        prompt_tokens: 9780,
        cached_tokens: 9344,
        completion_tokens: 118,
        total_tokens: 9898,
        finish_reason: 'stop',
        latency_ms: 310,
      },
    ],
    usage: { prompt_tokens: 28340, cached_tokens: 27264, completion_tokens: 274, total_tokens: 28614 },
    latency_ms: 885,
    tool_calls: [
      {
        name: 'about_server',
        arguments: JSON.stringify({ server: serverId }),
        result: `${serverId}: state=running, players=4/20, tps=20.0, mspt=4.1ms, memory=3.4/8.0GB`,
      },
      {
        name: 'send_command',
        arguments: JSON.stringify({ server: serverId, command: 'whitelist add sara_v' }),
        result: 'Added sara_v to the whitelist (13 whitelisted players)',
      },
      {
        name: 'read_logs',
        arguments: JSON.stringify({ server: serverId, lines: 40 }),
        result: '[16:08:11] [Server thread/INFO]: Added sara_v to the whitelist\n[16:08:12] [Server thread/INFO]: Tick health nominal: 20.0 TPS, 0 overloaded warnings in last 30m',
      },
    ],
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
  const [incident, setIncident] = useState<string | null>(null);
  const [arenaScenario, setArenaScenario] = useState<string>(SCENARIOS[0].id);
  const [resolvedIds, setResolvedIds] = useState<string[]>([]);
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

  /** Live mode shows the real servers Gary can see, not the demo set. */
  const loadLiveServers = useCallback(async (): Promise<boolean> => {
    try {
      const r = await harness<{ result: string }>('/operator/tool', { tool: 'list_servers', arguments: {}, channel: CHANNEL, access: 'manager' });
      const list = parseServerList(r.result);
      if (list.length) {
        setServers(list);
        setFocus((f) => (list.some((x) => x.id === f) ? f : list[0].id));
      }
      return true;
    } catch {
      return false;
    }
  }, []);

  const changeMode = useCallback(
    (m: Mode) => {
      if (m === 'replay') {
        setMode('replay');
        setServers(SERVERS);
        setFocus(SERVERS[0].id);
        return;
      }
      if (!CHANNEL) {
        showToast('Live mode needs Gary’s harness details in web/.env. See web/README.md.');
        return;
      }
      void loadLiveServers().then((ok) => {
        if (ok) {
          setMode('live');
          setScenario(null);
        } else showToast('Couldn’t reach Gary’s harness. Staying on Replay.');
      });
    },
    [loadLiveServers, showToast],
  );

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
      const isDiscordRelay = !sc && (prompt.includes('sara_v') || prompt.startsWith('[Discord'));
      const r = sc ? sc.response : isDiscordRelay ? discordFriendAnswer(focus) : statusAnswer(servers);
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
    [play, patch, scenario, servers, focus, setState],
  );

  const live = useCallback(
    async (id: string, prompt: string, confirm = false) => {
      // `confirm` pre-approves a destructive tool and the harness only accepts it at admin.
      const body: GaryRequest = { prompt, new_session: false, channel: CHANNEL, access: confirm ? 'admin' : 'manager', confirm };
      const ctl = new AbortController();
      const slow = window.setTimeout(() => showToast('Gary is still working. Switch to Replay from the top bar if the network is the problem.'), LIVE_SLOW_MS);
      const kill = window.setTimeout(() => ctl.abort(), LIVE_TIMEOUT_MS);
      try {
        const r = await harness<GaryResponse>('/operator/gary', body, ctl.signal);
        patch(id, (t) => ({ ...t, status: 'done', steps: r.tool_calls, response: r }));
        const touched = serversTouched(r.tool_calls);
        if (touched.length) setFocus(touched[touched.length - 1]);
        void loadLiveServers();
      } catch (e) {
        const msg = ctl.signal.aborted ? 'Gary took longer than 15 minutes.' : e instanceof Error ? e.message : String(e);
        patch(id, (t) => ({ ...t, status: 'error', error: msg }));
      } finally {
        window.clearTimeout(slow);
        window.clearTimeout(kill);
      }
    },
    [patch, showToast, loadLiveServers],
  );

  const ask = useCallback(
    (prompt: string, opts?: { followUp?: boolean }) => {
      const p = prompt.trim();
      if (!p || busy) return;
      const id = nextId();
      setTurns((ts) => [...ts, { id, prompt: p, at: Date.now(), status: 'running', steps: [], followUp: opts?.followUp }]);
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

  const openIncident = useCallback((id: string | null) => {
    setIncident(id);
    if (id) setScreen('incidents');
  }, []);

  const openArena = useCallback((scenarioId: string) => {
    setArenaScenario(scenarioId);
    setPersonaState('host');
    setScreen('arena');
    const u = new URL(location.href);
    u.searchParams.set('as', 'host');
    history.replaceState(null, '', u);
  }, []);

  const resolveIncident = useCallback(
    (id: string, actionLabel: string) => {
      setResolvedIds((prev) => (prev.includes(id) ? prev : [...prev, id]));
      showToast(`${actionLabel}. Handed back to Gary for verification.`);
    },
    [showToast],
  );

  useEffect(() => () => window.clearTimeout(toastTimer.current), []);

  return (
    <Ctx.Provider
      value={{
        persona,
        setPersona,
        screen,
        go: setScreen,
        mode,
        setMode: changeMode,
        servers,
        focus,
        setFocus,
        scenario,
        breakServer,
        resetDemo,
        draft,
        setDraft,
        turns,
        busy,
        ask,
        decide,
        toast,
        showToast,
        incident,
        openIncident,
        arenaScenario,
        openArena,
        resolvedIds,
        resolveIncident,
      }}
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
