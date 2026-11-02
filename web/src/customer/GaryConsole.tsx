/**
 * The server owner's console: a real game-server workbench with Gary built in.
 *
 * Layout:
 * - Top: Console header + active FaultAlertBar (when a server is crashed) + Gary Dispatch composer.
 * - Left / Main column: Active Server Workbench (server switcher, vitals, players online,
 *   installed runtime/mods & config summary, quick lifecycle actions).
 * - Right column: Full-height Inspector Panel with 3 tabs:
 *     1. Gary Activity (unified conversation + inline tool trace + approvals + safety stepper)
 *     2. Server Log (live latest.log tail + one-click "Send error to Gary")
 *     3. Snapshots & Config (pre-change rollback points + key server config state)
 */
import { useEffect, useRef, useState } from 'react';
import Markdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { GaryMark } from '../app/Shell';
import { Icon } from '../app/icons';
import { useStore, type Turn } from '../app/store';
import { SCENARIOS, type OwnedServer } from '../data/scenarios';
import { describeTool, isMutating, safetyOf, serversTouched, type SafetyStage } from '../gary/derive';
import { StatusBadge } from '../ui/ui';

type RightTab = 'activity' | 'logs' | 'snapshots';

const STARTERS: { label: string; prompt: string }[] = [
  { label: 'Check all servers', prompt: 'How are my servers doing?' },
  { label: 'Inspect logs for lag', prompt: 'Check the recent server logs for any lag spikes or warnings.' },
  { label: 'Verify mod compatibility', prompt: 'Check if any installed mods on mc-friends-smp have updates or conflicts.' },
];

interface ServerProfile {
  address: string;
  tps: string;
  uptime: string;
  roster: string[];
  mods: { name: string; version: string; status: 'Loaded' | 'Missing dep' | 'Idle' }[];
  config: { key: string; value: string }[];
  defaultLogs: string[];
  snapshots: { id: string; at: string; trigger: string; size: string }[];
}

const SERVER_PROFILES: Record<string, ServerProfile> = {
  'mc-friends-smp': {
    address: 'mc-friends-smp.grizzly.gg:25565',
    tps: '20.0 TPS',
    uptime: '6d 14h',
    roster: ['josh_f', 'kyle_m', 'sara_v', 'nora_t'],
    mods: [
      { name: 'fabric-loader', version: '0.16.7', status: 'Loaded' },
      { name: 'lithium-fabric', version: '0.13.1', status: 'Loaded' },
      { name: 'ferritecore', version: '7.0.2', status: 'Loaded' },
      { name: 'voicechat-fabric', version: '2.5.24', status: 'Loaded' },
    ],
    config: [
      { key: 'view-distance', value: '10 chunks' },
      { key: 'simulation-distance', value: '8 chunks' },
      { key: 'max-tick-time', value: '60000 ms' },
      { key: 'white-list', value: 'true (12 members)' },
    ],
    defaultLogs: [
      '[15:42:01] [Server thread/INFO]: Starting minecraft server version 1.21.1',
      '[15:42:02] [Server thread/INFO]: Loading properties (server.properties)',
      '[15:42:03] [Server thread/INFO]: Preparing level "world"',
      '[15:42:05] [Server thread/INFO]: Done (2.412s)! For help, type "help"',
      '[15:48:19] [Server thread/INFO]: josh_f joined the game',
      '[15:51:04] [Server thread/INFO]: kyle_m joined the game',
      '[16:02:44] [Server thread/INFO]: [voicechat] Connected 4 client streams',
    ],
    snapshots: [
      { id: 'snap-2026-11-02-1540', at: 'Today · 3:40 PM', trigger: 'Pre-restart auto snapshot', size: '1.42 GB' },
      { id: 'snap-2026-11-02-1000', at: 'Today · 10:00 AM', trigger: 'Scheduled 6h world checkpoint', size: '1.41 GB' },
      { id: 'snap-2026-11-01-2200', at: 'Yesterday · 10:00 PM', trigger: 'Before server.properties edit', size: '1.38 GB' },
    ],
  },
  'mc-creative': {
    address: 'mc-creative.grizzly.gg:25566',
    tps: '20.0 TPS',
    uptime: '3h 12m',
    roster: ['alex_b', 'maya_k'],
    mods: [
      { name: 'vanilla-server', version: '1.21.1', status: 'Loaded' },
      { name: 'worldedit-bukkit', version: '7.3.6', status: 'Loaded' },
    ],
    config: [
      { key: 'gamemode', value: 'creative' },
      { key: 'view-distance', value: '12 chunks' },
      { key: 'spawn-protection', value: '0' },
      { key: 'pvp', value: 'false' },
    ],
    defaultLogs: [
      '[13:10:11] [Server thread/INFO]: Starting minecraft server version 1.21.1',
      '[13:10:13] [Server thread/INFO]: Done (1.894s)! For help, type "help"',
      '[14:22:09] [Server thread/INFO]: alex_b joined the game',
      '[15:04:51] [Server thread/INFO]: maya_k joined the game',
      "[16:01:18] [Server thread/INFO]: Disconnecting /203.0.113.24: Outdated server! I'm still on 1.21.1 (client 1.21.3)",
    ],
    snapshots: [
      { id: 'snap-2026-11-02-1310', at: 'Today · 1:10 PM', trigger: 'Pre-boot world snapshot', size: '640 MB' },
    ],
  },
  'valheim-vikings': {
    address: 'valheim-vikings.grizzly.gg:2456',
    tps: '60 Hz sim',
    uptime: '2d 05h',
    roster: ['ulfric', 'freya', 'bjorn'],
    mods: [
      { name: 'BepInExPack_Valheim', version: '5.4.2202', status: 'Loaded' },
      { name: 'ValheimPlus', version: '0.9.14', status: 'Loaded' },
    ],
    config: [
      { key: 'world', value: 'MidgardSeed42' },
      { key: 'crossplay', value: 'enabled (PlayFab)' },
      { key: 'save-interval', value: '1800s' },
      { key: 'modifiers', value: 'resources 1.5x · raids normal' },
    ],
    defaultLogs: [
      '11/02/2026 14:00:12: Starting headless Valheim server 0.219.16',
      '11/02/2026 14:00:16: World MidgardSeed42 loaded (412892 zdos)',
      '11/02/2026 15:12:40: Got handshake from client 76561198042109',
      '11/02/2026 16:00:16: World saved (318.4 ms)',
    ],
    snapshots: [
      { id: 'snap-2026-11-02-1600', at: 'Today · 4:00 PM', trigger: 'Automated world.db + .fwl backup', size: '284 MB' },
      { id: 'snap-2026-11-02-1000', at: 'Today · 10:00 AM', trigger: 'Automated world.db + .fwl backup', size: '281 MB' },
    ],
  },
  'terraria-weekend': {
    address: 'terraria-weekend.grizzly.gg:7777',
    tps: 'Asleep',
    uptime: ' Hibernating',
    roster: [],
    mods: [
      { name: 'tModLoader', version: '1.4.4.9', status: 'Idle' },
      { name: 'CalamityMod', version: '2.0.4.3', status: 'Idle' },
    ],
    config: [
      { key: 'world', value: 'CalamityLarge.wld' },
      { key: 'difficulty', value: 'Expert (2)' },
      { key: 'autocreate', value: '3 (Large)' },
      { key: 'wake-on-join', value: 'enabled (socket proxy)' },
    ],
    defaultLogs: [
      '[02:14:00] Terraria Server v1.4.4.9 — last player disconnected.',
      '[02:29:00] Idle timer reached (15m). Saving world CalamityLarge.wld...',
      '[02:29:02] Entering socket-activated sleep mode. Will wake on TCP:7777.',
    ],
    snapshots: [
      { id: 'snap-2026-11-02-0229', at: 'Today · 2:29 AM', trigger: 'Pre-sleep world save', size: '94 MB' },
    ],
  },
};

export function GaryConsole() {
  const [rightTab, setRightTab] = useState<RightTab>('activity');
  const { turns } = useStore();
  const prevTurnsLen = useRef(turns.length);

  // Whenever a new turn starts, make sure the Gary Activity tab is visible so the user sees him work.
  useEffect(() => {
    if (turns.length > prevTurnsLen.current) {
      setRightTab('activity');
    }
    prevTurnsLen.current = turns.length;
  }, [turns.length]);

  return (
    <div className="screen cx">
      <ConsoleHeader />
      <FaultAlertBar onEngage={() => setRightTab('activity')} />
      <DispatchBar onDispatch={() => setRightTab('activity')} />
      <div className="cx-workbench">
        <ServerDashboard onOpenTab={setRightTab} />
        <InspectorPanel tab={rightTab} setTab={setRightTab} />
      </div>
    </div>
  );
}

function ConsoleHeader() {
  const { servers, focus, setFocus } = useStore();
  const down = servers.filter((x) => x.state.status === 'Crashed').length;
  const running = servers.filter((x) => x.state.status === 'Running').length;

  return (
    <header className="cx-head">
      <div className="cx-title">
        <h1>
          {down > 0 ? `${down} server halted · operator action required` : 'Server workbench'}
        </h1>
        <span className="cx-detail">
          {running} of {servers.length} servers running
          {down > 0 && <span className="cx-down"> · {down} crashed</span>}
        </span>
      </div>
      <div className="cx-srv-tabs" role="tablist" aria-label="Active server">
        {servers.map((x) => (
          <button
            key={x.id}
            type="button"
            role="tab"
            aria-selected={x.id === focus}
            className={`cx-srv-tab ${x.id === focus ? 'is-on' : ''} ${x.state.status === 'Crashed' ? 'is-crashed' : ''}`}
            onClick={() => setFocus(x.id)}
          >
            <span className={`lamp lamp-${x.state.status === 'Running' ? 'ok' : x.state.status === 'Crashed' ? 'bad' : 'off'}`} aria-hidden="true" />
            <span>{x.id}</span>
          </button>
        ))}
      </div>
    </header>
  );
}

/**
 * When a server is crashed or a fault is staged, surface the exception and the
 * one-click fix trigger at the very top of the screen before anything else.
 */
function FaultAlertBar({ onEngage }: { onEngage: () => void }) {
  const { servers, scenario, ask, busy, setFocus } = useStore();
  const crashed = servers.find((s) => s.state.status === 'Crashed');
  if (!crashed && !scenario) return null;

  const targetServer = crashed?.id ?? scenario?.server ?? '';
  const detail = crashed?.state.detail ?? scenario?.before.detail ?? 'Fault detected';
  const logLines = crashed?.state.logTail ?? scenario?.before.logTail;
  const prompt = scenario?.prompt ?? `My server ${targetServer} crashed, please inspect the logs and fix it.`;

  return (
    <section className="cx-fault-bar" aria-label="Active server fault">
      <div className="cx-fault-main">
        <div className="cx-fault-head">
          <span className="badge badge-bad">
            <span className="lamp lamp-bad" aria-hidden="true" />
            {crashed ? 'Server crashed' : 'Attention required'}
          </span>
          <strong className="cx-fault-srv">{targetServer}</strong>
          <span className="muted">{detail}</span>
        </div>
        {logLines && logLines.length > 0 && (
          <pre className="cx-fault-log">{logLines.slice(0, 2).join('\n')}</pre>
        )}
      </div>
      <div className="cx-fault-actions">
        <button
          type="button"
          className="btn btn-primary"
          disabled={busy}
          onClick={() => {
            setFocus(targetServer);
            onEngage();
            ask(prompt);
          }}
        >
          <Icon name="send" size={15} />
          Have Gary diagnose &amp; fix
        </button>
      </div>
    </section>
  );
}

function DispatchBar({ onDispatch }: { onDispatch: () => void }) {
  const { draft, setDraft, ask, busy, focus } = useStore();

  const submit = (promptText: string) => {
    if (!promptText.trim() || busy) return;
    onDispatch();
    ask(promptText);
  };

  return (
    <form
      className="cx-dispatch"
      onSubmit={(e) => {
        e.preventDefault();
        submit(draft);
      }}
    >
      <div className="cx-composer-row">
        <span className="cx-dispatch-badge">
          <GaryMark size={18} />
          <span>Ask Gary</span>
        </span>
        <textarea
          id="gary-input"
          rows={1}
          value={draft}
          aria-label={`Message Gary about ${focus}`}
          placeholder={`Message Gary about ${focus}`}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && !e.shiftKey) {
              e.preventDefault();
              submit(draft);
            }
          }}
        />
        <button type="submit" className="btn btn-primary" disabled={busy || !draft.trim()}>
          <Icon name="send" size={15} />
          Dispatch Gary
        </button>
      </div>
      <div className="cx-quick-bar">
        <span className="cx-quick-label muted">Quick commands:</span>
        {STARTERS.map((s) => (
          <button key={s.prompt} type="button" className="cx-quick-chip" disabled={busy} onClick={() => submit(s.prompt)}>
            {s.label}
          </button>
        ))}
      </div>
    </form>
  );
}

/**
 * Left / Main column: the selected server's live operational dashboard.
 * Replaces the old "demo scenario list" with real server vitals, player roster,
 * installed runtime/mods, config summary, and direct actions.
 */
function ServerDashboard({ onOpenTab }: { onOpenTab: (t: RightTab) => void }) {
  const { servers, focus, ask, busy, showToast } = useStore();
  const active: OwnedServer = servers.find((x) => x.id === focus) ?? servers[0];
  const st = active.state;
  const profile = SERVER_PROFILES[active.id] ?? SERVER_PROFILES['mc-friends-smp'];
  const memRatio = st.memory[1] ? Math.min(1, st.memory[0] / st.memory[1]) : 0;
  const isCrashed = st.status === 'Crashed';
  const isRunning = st.status === 'Running';

  return (
    <section className="cx-srv-panel" aria-label={`${active.id} server status and controls`}>
      <div className="cx-srv-hero">
        <div className="cx-srv-id-block">
          <div className="cx-srv-title-row">
            <h2 className="cx-srv-name">{active.id}</h2>
            <StatusBadge status={st.status} />
            <span className="cx-srv-engine">{active.game} · {st.detail}</span>
          </div>
          <div className="cx-srv-addr-row">
            <span className="mono cx-srv-addr">{active.live ? st.detail : profile.address}</span>
            <button
              type="button"
              className="cx-copy-btn"
              onClick={() => {
                void navigator.clipboard?.writeText(active.live ? st.detail : profile.address);
                showToast(`Copied ${active.live ? st.detail : profile.address}`);
              }}
            >
              Copy address
            </button>
          </div>
        </div>

        <div className="cx-srv-controls">
          {isCrashed ? (
            <button
              type="button"
              className="btn btn-primary btn-small"
              disabled={busy}
              onClick={() => {
                onOpenTab('activity');
                ask(`My server ${active.id} crashed on startup. Read the logs, snapshot before any changes, and fix it.`);
              }}
            >
              <Icon name="wrench" size={14} />
              Diagnose &amp; repair crash
            </button>
          ) : (
            <button
              type="button"
              className="btn btn-secondary btn-small"
              disabled={busy}
              onClick={() => {
                onOpenTab('activity');
                ask(`Run a health check on ${active.id} and check the recent logs.`);
              }}
            >
              <Icon name="check" size={14} />
              Verify health
            </button>
          )}
          <button
            type="button"
            className="btn btn-secondary btn-small"
            disabled={busy}
            onClick={() => {
              onOpenTab('snapshots');
              showToast(`Snapshot taken for ${active.id}`);
            }}
          >
            Take snapshot
          </button>
          <button
            type="button"
            className="btn btn-quiet btn-small"
            onClick={() => onOpenTab('logs')}
          >
            View logs
          </button>
        </div>
      </div>

      {/* Vital gauges */}
      <dl className="cx-vitals-grid">
        <div className="cx-vital-cell">
          <dt>Players online</dt>
          <dd>
            {st.players[0]}
            <span className="muted"> / {st.players[1]}</span>
          </dd>
          <span className="cx-vital-sub">
            {isRunning && st.players[0] > 0
              ? profile.roster.slice(0, st.players[0]).join(', ')
              : isCrashed
                ? 'Connection refused'
                : 'No active sessions'}
          </span>
        </div>

        <div className="cx-vital-cell">
          <dt>Memory allocation</dt>
          <dd>
            {st.memory[0].toFixed(1)}
            <span className="muted"> / {st.memory[1]} GB</span>
          </dd>
          <div className="cx-bar" aria-hidden="true">
            <span style={{ width: `${memRatio * 100}%` }} className={memRatio > 0.88 ? 'is-hot' : ''} />
          </div>
        </div>

        <div className="cx-vital-cell">
          <dt>Tick rate &amp; uptime</dt>
          <dd>{isCrashed ? '0.0 TPS' : isRunning ? profile.tps : 'Stopped'}</dd>
          <span className="cx-vital-sub">
            {isCrashed ? `${st.restarts} failed restarts` : `Uptime ${profile.uptime} · ${st.restarts} restarts`}
          </span>
        </div>
      </dl>

      {/* If there is an active log warning or crash tail on this server, surface a preview */}
      {st.logTail && st.logTail.length > 0 && (
        <div className={`cx-srv-log-callout ${isCrashed ? 'is-bad' : 'is-warn'}`}>
          <div className="cx-callout-head">
            <strong>{isCrashed ? 'Fatal exception in logs/latest.log' : 'Recent warning in logs/latest.log'}</strong>
            <button type="button" className="cx-link-btn" onClick={() => onOpenTab('logs')}>
              Open full server log
            </button>
          </div>
          <pre className="cx-log">{st.logTail.join('\n')}</pre>
        </div>
      )}

      {/* Two-column operational detail: Installed Mods/Runtime + Server Configuration */}
      <div className="cx-srv-subgrid">
        <div className="cx-subpanel">
          <div className="cx-subpanel-head">
            <h3>Installed mods</h3>
            <button
              type="button"
              className="cx-link-btn"
              disabled={busy}
              onClick={() => {
                onOpenTab('activity');
                ask(`Check installed mods on ${active.id} and let me know if anything is missing or outdated.`);
              }}
            >
              Audit mods
            </button>
          </div>
          <ul className="cx-kv-list">
            {isCrashed && active.id === 'mc-friends-smp' && (
              <li className="cx-kv-row is-err">
                <span className="mono">iris-mc1.21.1-1.8.1.jar</span>
                <span className="badge badge-bad">Missing fabric-api</span>
              </li>
            )}
            {profile.mods.map((m) => (
              <li key={m.name} className="cx-kv-row">
                <span className="mono">{m.name}</span>
                <span className="muted mono">{m.version}</span>
              </li>
            ))}
          </ul>
        </div>

        <div className="cx-subpanel">
          <div className="cx-subpanel-head">
            <h3>Server properties</h3>
            <button
              type="button"
              className="cx-link-btn"
              onClick={() => onOpenTab('snapshots')}
            >
              Backups
            </button>
          </div>
          <ul className="cx-kv-list">
            {profile.config.map((c) => (
              <li key={c.key} className="cx-kv-row">
                <span className="mono muted">{c.key}</span>
                <span className="mono">{c.value}</span>
              </li>
            ))}
          </ul>
        </div>
      </div>

      {/* Rollback & snapshot safety strip */}
      <div className="cx-srv-foot">
        <div className="cx-srv-foot-main">
          <span className="lamp lamp-ok" aria-hidden="true" />
          <span>
            <strong>Write guard active:</strong> automatic snapshot before any file or mod change
            {profile.snapshots[0] ? ` · Latest: ${profile.snapshots[0].id} (${profile.snapshots[0].at})` : ''}
          </span>
        </div>
        <button type="button" className="cx-link-btn" onClick={() => onOpenTab('snapshots')}>
          Manage snapshots ({profile.snapshots.length})
        </button>
      </div>
    </section>
  );
}

/**
 * Right column: Full-height Inspector Panel with 3 unified tabs:
 * 1. Gary Activity (conversation + inline tool trace + safety stepper + approvals)
 * 2. Server Log (latest.log live tail + one-click send to Gary)
 * 3. Snapshots & Config (pre-change backups + restore triggers)
 */
function InspectorPanel({ tab, setTab }: { tab: RightTab; setTab: (t: RightTab) => void }) {
  const { turns, servers, focus } = useStore();
  const active = servers.find((x) => x.id === focus) ?? servers[0];
  const totalSteps = turns.reduce((acc, t) => acc + t.steps.length, 0);
  const hasPendingApproval = turns.some((t) => t.approval?.state === 'pending');
  const isRunning = turns.some((t) => t.status === 'running');
  const hasLogError = active.state.status === 'Crashed' || (active.state.logTail && active.state.logTail.length > 0);
  const profile = SERVER_PROFILES[active.id] ?? SERVER_PROFILES['mc-friends-smp'];

  return (
    <aside className="cx-inspector" aria-label="Operator activity, server logs, and snapshots">
      <div className="cx-inspector-tabs" role="tablist" aria-label="Inspector views">
        <button
          type="button"
          role="tab"
          aria-selected={tab === 'activity'}
          className={`cx-insp-tab ${tab === 'activity' ? 'is-on' : ''}`}
          onClick={() => setTab('activity')}
        >
          <span>Gary Activity</span>
          {(isRunning || hasPendingApproval || totalSteps > 0) && (
            <span className={`cx-tab-count ${hasPendingApproval ? 'is-warn' : isRunning ? 'is-active' : ''}`}>
              {hasPendingApproval ? 'Approval' : isRunning ? 'Running' : `${totalSteps}`}
            </span>
          )}
        </button>

        <button
          type="button"
          role="tab"
          aria-selected={tab === 'logs'}
          className={`cx-insp-tab ${tab === 'logs' ? 'is-on' : ''}`}
          onClick={() => setTab('logs')}
        >
          <span>Server Log</span>
          {hasLogError && <span className={`lamp ${active.state.status === 'Crashed' ? 'lamp-bad' : 'lamp-warn'}`} aria-hidden="true" />}
        </button>

        <button
          type="button"
          role="tab"
          aria-selected={tab === 'snapshots'}
          className={`cx-insp-tab ${tab === 'snapshots' ? 'is-on' : ''}`}
          onClick={() => setTab('snapshots')}
        >
          <span>Snapshots &amp; Backups</span>
          <span className="cx-tab-count">{profile.snapshots.length}</span>
        </button>
      </div>

      <div className="cx-inspector-body">
        {tab === 'activity' && <GaryActivityTab />}
        {tab === 'logs' && <ServerLogTab onSendToGary={() => setTab('activity')} />}
        {tab === 'snapshots' && <SnapshotsTab onSendToGary={() => setTab('activity')} />}
      </div>
    </aside>
  );
}

const STAGES: { key: SafetyStage; label: string }[] = [
  { key: 'snapshot', label: 'Snapshot' },
  { key: 'apply', label: 'Apply fix' },
  { key: 'verify', label: 'Restart & verify' },
  { key: 'done', label: 'Healthy' },
];

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

/**
 * Tab 1: Stateless Operator Run Inspector (latest run in focus + inline tool trace + safety stepper).
 * Intentionally structured as bounded operator tickets rather than an open-ended chat bubble thread.
 */
function GaryActivityTab() {
  const { turns, focus, ask, busy } = useStore();
  const [showHistory, setShowHistory] = useState(false);
  const activeRun = turns[turns.length - 1];
  const previousRuns = turns.slice(0, -1).reverse();

  const latestTurn = [...turns].reverse().find((t) => t.steps.length > 0 || t.status !== 'running') ?? activeRun;
  const safety = safetyOf(
    latestTurn?.status === 'done'
      ? latestTurn.response
      : latestTurn
        ? { ...emptyResponse, tool_calls: latestTurn.steps }
        : undefined,
  );

  const isReadOnlyDone = latestTurn?.status === 'done' && !safety.mutated && !latestTurn.approval;
  const isConsoleOnlyDone = latestTurn?.status === 'done' && safety.consoleOnly;

  return (
    <div className="cx-activity-pane">
      {/* Safety loop bar stays pinned at the top of the activity inspector */}
      <div className="cx-safety-bar">
        {isReadOnlyDone ? (
          <div className="cx-safety-readonly">
            <span className="badge badge-ok">
              <Icon name="check" size={12} />
              Read-only · No mutation needed
            </span>
            <span className="muted">Inspected server state without modifying files or restarting</span>
          </div>
        ) : isConsoleOnlyDone ? (
          <div className="cx-safety-readonly">
            <span className="badge badge-ok">
              <Icon name="check" size={12} />
              Live console command · No disk write
            </span>
            <span className="muted">Resolved in memory without touching server files or restarting</span>
          </div>
        ) : (
          <ol className="cx-stepper" aria-label="Safety loop">
            {STAGES.map((s) => {
              const done = safety.reached.includes(s.key);
              const label = s.key === 'done' && safety.outcome === 'rolled_back' ? 'Rolled back' : s.label;
              return (
                <li key={s.key} className={done ? 'is-done' : ''}>
                  <span className="cx-stepper-dot">{done && <Icon name="check" size={11} />}</span>
                  {label}
                </li>
              );
            })}
          </ol>
        )}
      </div>

      {turns.length === 0 ? (
        <div className="cx-activity-empty">
          <div className="cx-empty-badge">
            <GaryMark size={24} />
            <div>
              <strong>Gary is standing by on {focus}</strong>
              <p className="muted">
                Dispatch a task above. Each request runs as a bounded operator ticket with an audited tool trace and health check.
              </p>
            </div>
          </div>

          <dl className="cx-log-rules">
            <div>
              <dt>Execution model</dt>
              <dd className="mono">Single-run ticket (stateless)</dd>
            </div>
            <div>
              <dt>Read tools</dt>
              <dd className="mono">list_servers · read_logs · read_file</dd>
            </div>
            <div>
              <dt>Write guard</dt>
              <dd className="mono">snapshot_server before any edit</dd>
            </div>
            <div>
              <dt>Destructive gate</dt>
              <dd className="mono">Requires owner approval inline</dd>
            </div>
          </dl>

          <div className="cx-empty-cta">
            <button
              type="button"
              className="btn btn-secondary btn-small"
              disabled={busy}
              onClick={() => ask(`Check ${focus} status and recent logs.`)}
            >
              Run status check on {focus}
            </button>
          </div>
        </div>
      ) : (
        <div className="cx-thread">
          {activeRun && (
            <RunView
              key={activeRun.id}
              turn={activeRun}
              runNumber={turns.length}
              isLatest={true}
            />
          )}

          {previousRuns.length > 0 && (
            <div className="cx-run-history">
              <button
                type="button"
                className="cx-history-toggle"
                aria-expanded={showHistory}
                onClick={() => setShowHistory((v) => !v)}
              >
                <span>Previous runs in this session ({previousRuns.length})</span>
                <span className={`cx-step-chev ${showHistory ? 'is-open-chev' : ''}`}>
                  <Icon name="chevron" size={13} />
                </span>
              </button>
              {showHistory && (
                <div className="cx-history-list">
                  {previousRuns.map((t, idx) => (
                    <RunView
                      key={t.id}
                      turn={t}
                      runNumber={turns.length - 1 - idx}
                      isLatest={false}
                    />
                  ))}
                </div>
              )}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function RunView({ turn, runNumber, isLatest }: { turn: Turn; runNumber: number; isLatest: boolean }) {
  const isPendingApproval = turn.approval?.state === 'pending';
  const isEscalated = turn.response?.escalated;

  const statusBadge =
    turn.status === 'running' ? (
      <span className="badge badge-brand">Running…</span>
    ) : turn.status === 'error' ? (
      <span className="badge badge-bad">Failed</span>
    ) : isPendingApproval ? (
      <span className="badge badge-warn">Awaiting approval</span>
    ) : isEscalated ? (
      <span className="badge badge-warn">Handed to staff</span>
    ) : (
      <span className="badge badge-ok">Ticket closed</span>
    );

  return (
    <article className={`cx-run-card ${isLatest ? 'is-latest' : 'is-past'}`}>
      <div className="cx-run-head">
        <div className="cx-run-meta">
          <span className="mono cx-run-id">RUN #{runNumber}</span>
          {statusBadge}
          <ServerTags turn={turn} />
        </div>
        <span className="cx-run-kind muted">Stateless dispatch</span>
      </div>

      <div className="cx-run-prompt">
        <span className="cx-run-prompt-label muted">Dispatched instruction</span>
        <p>{turn.prompt}</p>
      </div>

      <div className="cx-gary-body">
        {/* Inline expandable tool trace right inside the run */}
        {turn.steps.length > 0 && (
          <div className="cx-inline-trace">
            <div className="cx-trace-head">
              <span>Tool execution ({turn.steps.length} {turn.steps.length === 1 ? 'step' : 'steps'})</span>
              {turn.status === 'running' && <span className="cx-trace-live">Running…</span>}
            </div>
            <ol className="cx-steps">
              {turn.steps.map((s, i) => (
                <Step
                  key={i}
                  n={i + 1}
                  name={s.name}
                  line={describeTool(s)}
                  mutating={isMutating(s)}
                  args={s.arguments}
                  result={s.result}
                />
              ))}
            </ol>
          </div>
        )}

        {turn.status === 'running' && (
          <div className="cx-working" aria-live="polite">
            <div className="cx-working-head">
              <span className="cx-dots" aria-hidden="true">
                <i />
                <i />
                <i />
              </span>
              Gary is executing this run…
            </div>
          </div>
        )}

        {turn.status === 'error' && (
          <div className="banner banner-block">
            <span>Gary could not finish this run: {turn.error}</span>
          </div>
        )}

        {turn.status === 'done' && turn.response && (
          <>
            {turn.approval && <Approval turn={turn} />}
            {turn.response.escalated && (
              <div className="cx-escalation">
                <Icon name="alert" size={16} />
                <div>
                  <strong>Handed to staff</strong>
                  <p>{turn.response.escalation_reason ?? 'This needs a person. Your host support team has the details.'}</p>
                </div>
              </div>
            )}
            <div className="cx-reply">
              <Markdown remarkPlugins={[remarkGfm]}>{turn.response.reply}</Markdown>
            </div>

            {isLatest && !isPendingApproval && (
              <div className="cx-run-closed">
                <span className="muted">
                  Run #{runNumber} complete · Gary operates on single-task dispatches, not open chat sessions.
                </span>
                <button
                  type="button"
                  className="cx-link-btn"
                  onClick={() => {
                    const el = document.getElementById('gary-input') as HTMLTextAreaElement | null;
                    el?.focus();
                    el?.scrollIntoView({ behavior: 'smooth', block: 'center' });
                  }}
                >
                  New dispatch ↗
                </button>
              </div>
            )}
          </>
        )}
      </div>
    </article>
  );
}

function ServerTags({ turn }: { turn: Turn }) {
  const { setFocus, focus } = useStore();
  const ids = serversTouched(turn.steps);
  if (!ids.length) return null;
  return (
    <div className="cx-tags">
      {ids.map((id) => (
        <button key={id} type="button" className={`cx-tag ${focus === id ? 'is-on' : ''}`} onClick={() => setFocus(id)}>
          <Icon name="server" size={12} />
          {id}
        </button>
      ))}
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
        <strong>Action required: {a.title}</strong>
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
        <div className={`cx-approval-done ${state === 'approved' ? 'is-ok' : ''}`}>
          {state === 'approved' ? 'Approved. Gary restored the backup and verified the world.' : 'Denied. Nothing on the server was touched.'}
        </div>
      )}
    </div>
  );
}

function Step({
  n,
  name,
  line,
  mutating,
  args,
  result,
}: {
  n: number;
  name: string;
  line: string;
  mutating: boolean;
  args: string;
  result: string;
}) {
  const [open, setOpen] = useState(false);
  let pretty = args;
  try {
    pretty = JSON.stringify(JSON.parse(args), null, 2);
  } catch {
    // Verbatim fallback
  }
  return (
    <li className={`cx-stepline ${open ? 'is-open' : ''}`}>
      <button type="button" className="cx-step-btn" aria-expanded={open} onClick={() => setOpen(!open)}>
        <span className="cx-step-n">{n}</span>
        <span className="cx-step-text">{line}</span>
        <span className={`cx-step-kind ${mutating ? 'is-write' : ''}`}>{mutating ? 'write' : 'read'}</span>
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

/**
 * Tab 2: Full-height live server log tail with error highlighting and one-click send to Gary.
 */
function ServerLogTab({ onSendToGary }: { onSendToGary: () => void }) {
  const { servers, focus, ask, busy } = useStore();
  const active = servers.find((x) => x.id === focus) ?? servers[0];
  const profile = SERVER_PROFILES[active.id] ?? SERVER_PROFILES['mc-friends-smp'];
  const faultLines = active.state.logTail ?? [];
  const allLines = [...profile.defaultLogs, ...faultLines];

  return (
    <div className="cx-log-pane">
      <div className="cx-pane-bar">
        <div>
          <strong className="mono">{active.id}:/logs/latest.log</strong>
          <span className="muted"> · {allLines.length} lines</span>
        </div>
        <button
          type="button"
          className="btn btn-secondary btn-small"
          disabled={busy}
          onClick={() => {
            onSendToGary();
            ask(
              faultLines.length > 0
                ? `Inspect the error in ${active.id} logs/latest.log and fix it.`
                : `Read the latest logs on ${active.id} and summarize any warnings.`,
            );
          }}
        >
          <Icon name="send" size={13} />
          {faultLines.length > 0 ? 'Send error to Gary' : 'Ask Gary to analyze'}
        </button>
      </div>

      <div className="cx-terminal" role="log" aria-label={`Log output for ${active.id}`}>
        {allLines.map((line, i) => {
          const isErr = /ERROR|Exception|Caused by|corrupt|Incompatible|Outdated/i.test(line);
          const isWarn = /WARN|Can't keep up|overloaded/i.test(line);
          return (
            <div key={i} className={`cx-term-line ${isErr ? 'is-err' : isWarn ? 'is-warn' : ''}`}>
              <span className="cx-term-ln">{i + 1}</span>
              <span className="cx-term-code">{line}</span>
            </div>
          );
        })}
      </div>
    </div>
  );
}

/**
 * Tab 3: Pre-change snapshots and rollback safety points for the active server.
 */
function SnapshotsTab({ onSendToGary }: { onSendToGary: () => void }) {
  const { servers, focus, turns, ask, busy, showToast } = useStore();
  const active = servers.find((x) => x.id === focus) ?? servers[0];
  const profile = SERVER_PROFILES[active.id] ?? SERVER_PROFILES['mc-friends-smp'];

  // Include any snapshots Gary created during the current session
  const sessionWrites = turns
    .flatMap((t) => t.steps)
    .filter((s) => isMutating(s) && s.name !== 'send_command');

  const snapshots = [
    ...(sessionWrites.length > 0
      ? [
          {
            id: 'snap-session-latest',
            at: 'Just now · Gary supervisor',
            trigger: `Auto-snapshot before ${sessionWrites[0].name}`,
            size: profile.snapshots[0]?.size ?? '1.42 GB',
          },
        ]
      : []),
    ...profile.snapshots,
  ];

  return (
    <div className="cx-snap-pane">
      <div className="cx-pane-bar">
        <div>
          <strong>Rollback points for {active.id}</strong>
          <span className="muted"> · auto-snapshotted before every file write</span>
        </div>
        <button
          type="button"
          className="btn btn-secondary btn-small"
          onClick={() => showToast(`Created snapshot for ${active.id}`)}
        >
          New snapshot
        </button>
      </div>

      <ul className="cx-snap-list">
        {snapshots.map((snap) => (
          <li key={snap.id} className="cx-snap-row">
            <div className="cx-snap-info">
              <div className="cx-snap-top">
                <strong className="mono">{snap.id}</strong>
                <span className="badge">{snap.size}</span>
              </div>
              <div className="cx-snap-sub muted">
                {snap.at} · {snap.trigger}
              </div>
            </div>
            <button
              type="button"
              className="btn btn-secondary btn-small"
              disabled={busy}
              onClick={() => {
                onSendToGary();
                ask(`Restore ${active.id} from snapshot ${snap.id} and verify the server comes back healthy.`);
              }}
            >
              Restore
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}
