/**
 * The frame every screen sits in: the rail, a top bar, and the scrolling content.
 * The frame is fixed and only <main> scrolls, so the rail and top bar never move.
 *
 * One app, two audiences. The hosting company sees its triage queue, fleet, and eval
 * evidence; the server owner sees their operator console, live server list, and audit
 * activity.
 */
import { useEffect, useRef, useState, type ReactNode, type Ref } from 'react';
import { GARY_MODEL, SCENARIOS } from '../data/scenarios';
import { shortModel } from '../gary/derive';
import { INCIDENTS } from '../host/fleet';
import { Icon, type IconName } from './icons';
import { RailTip } from './RailTip';
import { useSidebarCollapse } from './sidebar-collapse';
import { useStore, type Persona, type Screen } from './store';
import { useTheme } from './theme';
import { useRailIndicator } from './useRailIndicator';

const COLLAPSE_SEQUENCE_MS = 460;

const TITLES: Record<Screen, string> = {
  overview: 'Triage queue',
  servers: 'Fleet servers',
  incidents: 'Incident log',
  arena: 'Model arena',
  benchmark: 'Benchmark',
  gary: 'Operator console',
  activity: 'Activity log',
};

interface Row {
  key: Screen;
  label: string;
  icon: IconName;
  count?: number;
  hot?: boolean;
}

function railGroups(persona: Persona, openEscalations: number, waitingApprovals: number): { label: string; rows: Row[] }[] {
  if (persona === 'customer')
    return [
      {
        label: 'Operator',
        rows: [
          { key: 'gary', label: 'Operator console', icon: 'ask', count: waitingApprovals || undefined, hot: true },
          { key: 'activity', label: 'Activity log', icon: 'activity' },
        ],
      },
    ];
  return [
    {
      label: 'Operations',
      rows: [
        { key: 'overview', label: 'Triage queue', icon: 'bolt', count: openEscalations || undefined, hot: openEscalations > 0 },
        { key: 'servers', label: 'Fleet servers', icon: 'server', count: openEscalations || undefined, hot: false },
        { key: 'incidents', label: 'Incident log', icon: 'home' },
      ],
    },
    {
      label: 'Evaluation',
      rows: [
        { key: 'benchmark', label: 'Benchmark', icon: 'chart' },
        { key: 'arena', label: 'Model arena', icon: 'versus' },
      ],
    },
  ];
}

/**
 * Gary's mark: a two-blade server chassis with a live status LED and operator prompt,
 * drawn in the Pinecrest hunter accent (never an AI sparkle).
 */
export function GaryMark({ size = 22 }: { size?: number }) {
  return (
    <svg viewBox="0 0 24 24" width={size} height={size} fill="none" aria-hidden="true">
      <rect x="2" y="3" width="20" height="8" rx="2" fill="var(--surface-2)" stroke="var(--brand-600)" strokeWidth="1.5" />
      <circle cx="6" cy="7" r="1.4" fill="var(--brand-600)" />
      <path d="M10 7h8" stroke="var(--ink-strong)" strokeWidth="1.5" strokeLinecap="round" />
      <rect x="2" y="13" width="20" height="8" rx="2" fill="var(--surface-2)" stroke="var(--line-strong)" strokeWidth="1.5" />
      <path d="M5.5 15.6L7.5 17l-2 1.4" stroke="var(--brand-600)" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
      <path d="M10 17h6" stroke="var(--muted)" strokeWidth="1.5" strokeLinecap="round" />
    </svg>
  );
}

const LAMP_TONE: Record<string, string> = {
  Running: 'ok',
  Starting: 'warn',
  Crashed: 'bad',
  Stopped: 'off',
};

function Sidebar({ open, onClose }: { open: boolean; onClose: () => void }) {
  const { persona, screen, go, turns, servers, focus, setFocus, mode, resolvedIds } = useStore();
  const { collapsed, collapsible, toggle } = useSidebarCollapse();
  const waiting = turns.filter((t) => t.approval?.state === 'pending').length;
  const openEscalations = INCIDENTS.filter((i) => i.outcome === 'escalated' && !resolvedIds.includes(i.id)).length;
  const groups = railGroups(persona, openEscalations, waiting);
  const sig = `${persona}|${groups.map((g) => g.rows.map((r) => r.key).join(',')).join('|')}`;
  const { ref: navRef, markerRef, style: indicatorStyle, follow } = useRailIndicator<HTMLElement>('button[aria-current="page"]', [screen, sig, collapsed], 0);
  useEffect(() => follow(COLLAPSE_SEQUENCE_MS), [collapsed, follow]);

  return (
    <>
      {open && <div className="rail-scrim" onClick={onClose} aria-hidden="true" />}
      <aside className={`rail ${collapsed ? 'is-collapsed' : ''} ${open ? 'is-open' : ''}`} aria-label="Main">
        <div className="rail-brand">
          <span className="rail-mark">
            <GaryMark />
          </span>
          <span className="rail-word">
            Gary
            <span className="rail-sub">{persona === 'host' ? 'fleet control plane' : 'server operator'}</span>
          </span>
          {!collapsed && collapsible && (
            <button type="button" className="rail-toggle" onClick={toggle} aria-expanded="true" aria-label="Collapse sidebar" title="Collapse sidebar">
              <Icon name="collapse" size={20} />
            </button>
          )}
        </div>

        <nav ref={navRef} className="rail-nav">
          <span ref={markerRef as Ref<HTMLSpanElement>} aria-hidden="true" className="rail-wash" style={indicatorStyle} />
          {groups.map((g, gi) => (
            <div key={g.label} className={`rail-group ${gi === 0 ? 'is-first' : ''}`}>
              <div className="rail-heading">
                <h3>{g.label}</h3>
              </div>
              <ul>
                {g.rows.map((r) => (
                  <li key={r.key}>
                    <RailTip label={r.count ? `${r.label}, ${r.count}` : r.label} enabled={collapsed}>
                      <button
                        type="button"
                        className={`rail-row ${screen === r.key ? 'is-on' : ''}`}
                        aria-current={screen === r.key ? 'page' : undefined}
                        onClick={() => {
                          go(r.key);
                          onClose();
                        }}
                      >
                        <span className="rail-icon" aria-hidden="true">
                          <Icon name={r.icon} />
                          {collapsed && !!r.count && <span className={`rail-dot ${r.hot ? 'is-hot' : ''}`} />}
                        </span>
                        <span className="rail-label">
                          {r.label}
                          {!!r.count && <span className="sr-only">, {r.count}</span>}
                        </span>
                        {!collapsed && !!r.count && (
                          <span aria-hidden="true" className={`rail-count ${r.hot ? 'is-hot' : ''}`}>
                            {r.count}
                          </span>
                        )}
                      </button>
                    </RailTip>
                  </li>
                ))}
              </ul>
            </div>
          ))}

          {persona === 'customer' && !collapsed && (
            <div className="rail-group">
              <div className="rail-heading">
                <h3>Attached servers</h3>
              </div>
              <ul className="rail-servers">
                {servers.map((srv) => {
                  const isFocused = screen === 'gary' && focus === srv.id;
                  const tone = LAMP_TONE[srv.state.status] ?? 'off';
                  return (
                    <li key={srv.id}>
                      <button
                        type="button"
                        className={`rail-srv-row ${isFocused ? 'is-focused' : ''} ${srv.state.status === 'Crashed' ? 'is-crashed' : ''}`}
                        onClick={() => {
                          setFocus(srv.id);
                          go('gary');
                          onClose();
                        }}
                      >
                        <span className={`lamp lamp-${tone}`} aria-hidden="true" />
                        <span className="rail-srv-id">{srv.id}</span>
                        <span className="rail-srv-meta">
                          {srv.state.status === 'Crashed'
                            ? 'Crashed'
                            : srv.state.status === 'Running'
                              ? `${srv.state.players[0]}/${srv.state.players[1]}`
                              : srv.state.status}
                        </span>
                      </button>
                    </li>
                  );
                })}
              </ul>
            </div>
          )}
        </nav>

        <div className="rail-foot">
          {collapsed && collapsible && (
            <RailTip label="Expand sidebar">
              <button type="button" className="rail-row" onClick={toggle} aria-expanded="false" aria-label="Expand sidebar">
                <span className="rail-icon" aria-hidden="true">
                  <Icon name="expand" />
                </span>
                <span className="rail-label">Expand</span>
              </button>
            </RailTip>
          )}
          <div className="rail-runtime">
            <span className={`lamp ${persona === 'host' ? (mode === 'live' ? 'lamp-ok' : 'lamp-off') : 'lamp-ok'}`} aria-hidden="true" />
            <span className="rail-label rail-runtime-text">
              {persona === 'host' ? (
                <>
                  <span className="rail-runtime-model mono">{shortModel(GARY_MODEL)} · Fireworks</span>
                  <span className="rail-runtime-mode">{mode === 'live' ? 'Live operator harness' : 'Replay harness'}</span>
                </>
              ) : (
                <>
                  <span className="rail-runtime-model">Friday Night Crew</span>
                  <span className="rail-runtime-mode">Write guard · auto-snapshot</span>
                </>
              )}
            </span>
          </div>
        </div>
      </aside>
    </>
  );
}

function ThemeToggle() {
  const { theme, toggle } = useTheme();
  const label = theme === 'dark' ? 'Switch to light mode' : 'Switch to dark mode';
  return (
    <button type="button" className="icon-btn" onClick={toggle} aria-label={label} title={label}>
      <Icon name={theme === 'dark' ? 'moon' : 'sun'} size={19} />
    </button>
  );
}

/** Whose view this is. Two buttons, one pressed: the demo flips between audiences. */
function PersonaSwitch() {
  const { persona, setPersona } = useStore();
  const opts: { key: Persona; label: string; icon: IconName }[] = [
    { key: 'customer', label: 'Server owner', icon: 'user' },
    { key: 'host', label: 'Hosting team', icon: 'building' },
  ];
  return (
    <div className="seg" role="group" aria-label="View as">
      {opts.map((o) => (
        <button key={o.key} type="button" className={`seg-btn ${persona === o.key ? 'is-on' : ''}`} aria-pressed={persona === o.key} onClick={() => setPersona(o.key)}>
          <Icon name={o.icon} size={16} />
          <span className="seg-text">{o.label}</span>
        </button>
      ))}
    </div>
  );
}

function TopBar({ onMenu }: { onMenu: () => void }) {
  const { screen, persona, go, servers, turns, resolvedIds, setFocus } = useStore();
  const scope = persona === 'host' ? 'Pinecrest Hosting' : 'Friday Night Crew';
  const home: Screen = persona === 'host' ? 'overview' : 'gary';
  const openEscalations = INCIDENTS.filter((i) => i.outcome === 'escalated' && !resolvedIds.includes(i.id)).length;
  const crashedServer = servers.find((s) => s.state.status === 'Crashed');
  const pendingApproval = turns.some((t) => t.approval?.state === 'pending');

  return (
    <header className="topbar">
      <button type="button" className="icon-btn topbar-menu" onClick={onMenu} aria-label="Open navigation">
        <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" strokeWidth={1.75} strokeLinecap="round" aria-hidden="true">
          <path d="M4 7h16M4 12h16M4 17h16" />
        </svg>
      </button>
      <div className="topbar-crumb" aria-label="Workspace scope">
        {screen === home ? (
          <span className="topbar-scope">{scope}</span>
        ) : (
          <button type="button" className="topbar-scope topbar-scope-link" onClick={() => go(home)}>
            {scope}
          </button>
        )}
        <span className="topbar-sep" aria-hidden="true">/</span>
        <span className="topbar-title">{TITLES[screen]}</span>
      </div>

      {persona === 'host' && openEscalations > 0 && screen !== 'overview' && (
        <button type="button" className="topbar-alert" onClick={() => go('overview')}>
          <span className="lamp lamp-bad" aria-hidden="true" />
          <span>{openEscalations} waiting on staff decision</span>
        </button>
      )}
      {persona === 'customer' && (crashedServer || pendingApproval) && (
        <button
          type="button"
          className="topbar-alert"
          onClick={() => {
            if (crashedServer) setFocus(crashedServer.id);
            go('gary');
          }}
        >
          <span className={`lamp ${pendingApproval ? 'lamp-warn' : 'lamp-bad'}`} aria-hidden="true" />
          <span>{pendingApproval ? 'Approval required to modify world' : `${crashedServer?.id} crashed`}</span>
        </button>
      )}

      <div className="topbar-right">
        <PersonaSwitch />
        <ThemeToggle />
      </div>
    </header>
  );
}

/**
 * Floating Next.js-style dev/chaos harness overlay in the bottom-left corner.
 * Keeps Replay/Live switching and fault injection completely out of the product UI.
 */
function DevHarnessOverlay() {
  const { mode, setMode, breakServer, resetDemo, busy, scenario, turns, ask, setFocus, openArena } = useStore();
  const [open, setOpen] = useState(false);
  const wrapRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (!wrapRef.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setOpen(false);
    };
    window.addEventListener('mousedown', onDown);
    window.addEventListener('keydown', onKey);
    return () => {
      window.removeEventListener('mousedown', onDown);
      window.removeEventListener('keydown', onKey);
    };
  }, [open]);

  const dirty = turns.length > 0 || !!scenario;

  return (
    <div ref={wrapRef} className={`dev-harness ${open ? 'is-open' : ''}`}>
      {open && (
        <div className="dev-popover" role="dialog" aria-label="Chaos and fault injection harness">
          <div className="dev-pop-head">
            <span className="dev-pop-title">
              <Icon name="bolt" size={14} />
              Fault &amp; Runtime Harness
            </span>
            <div className="dev-mode-switch" role="group" aria-label="Harness mode">
              <button
                type="button"
                className={`dev-mode-btn ${mode === 'replay' ? 'is-on' : ''}`}
                disabled={busy}
                onClick={() => setMode('replay')}
              >
                Replay
              </button>
              <button
                type="button"
                className={`dev-mode-btn ${mode === 'live' ? 'is-on' : ''}`}
                disabled={busy}
                onClick={() => setMode('live')}
              >
                Live
              </button>
            </div>
          </div>

          <div className="dev-pop-sub">Inject a server fault into the workspace</div>
          <ul className="dev-fault-list">
            {SCENARIOS.map((s, i) => {
              const isStaged = scenario?.id === s.id;
              return (
                <li key={s.id} className={`dev-fault-item ${isStaged ? 'is-staged' : ''}`}>
                  <div className="dev-fault-meta">
                    <span className="dev-fault-n">{i + 1}</span>
                    <div className="dev-fault-text">
                      <strong>{s.label}</strong>
                      <span className="mono">{s.server}</span>
                    </div>
                  </div>
                  <div className="dev-fault-btns">
                    <button
                      type="button"
                      className="btn btn-secondary btn-small"
                      disabled={busy}
                      onClick={() => {
                        breakServer(s.id);
                        setOpen(false);
                      }}
                    >
                      Break
                    </button>
                    <button
                      type="button"
                      className="btn btn-primary btn-small"
                      disabled={busy}
                      onClick={() => {
                        if (mode === 'replay') breakServer(s.id);
                        setFocus(s.server);
                        ask(s.prompt);
                        setOpen(false);
                      }}
                    >
                      Break &amp; run
                    </button>
                  </div>
                </li>
              );
            })}
          </ul>

          <div className="dev-pop-foot">
            <button
              type="button"
              className="btn btn-quiet btn-small"
              onClick={() => {
                const lastReplay = [...turns].reverse().find((t) => t.replay?.scenario && t.replay.scenario !== 'status')?.replay?.scenario;
                openArena(scenario?.id ?? lastReplay ?? SCENARIOS[0].id);
                setOpen(false);
              }}
            >
              <Icon name="versus" size={14} />
              Compare in Model Arena ↗
            </button>
            {dirty && (
              <button
                type="button"
                className="btn btn-quiet btn-small"
                disabled={busy}
                onClick={() => {
                  resetDemo();
                  setOpen(false);
                }}
              >
                <Icon name="reset" size={14} />
                Reset session
              </button>
            )}
          </div>
        </div>
      )}

      <div className={`dev-pill-bar ${scenario ? 'is-fault' : mode === 'live' ? 'is-live' : ''}`}>
        <button
          type="button"
          className={`dev-pill ${scenario ? 'is-fault' : mode === 'live' ? 'is-live' : ''}`}
          aria-expanded={open}
          onClick={() => setOpen((v) => !v)}
        >
          <span className="dev-pill-tag mono">CHAOS</span>
          <Icon name="bolt" size={14} />
          <span className="dev-pill-label">
            {scenario ? `Fault: ${scenario.short}` : 'Inject fault'}
          </span>
          <span className="dev-pill-mode mono">{mode === 'live' ? 'LIVE' : 'REPLAY'}</span>
        </button>
        {dirty && (
          <button
            type="button"
            className="dev-pill-reset"
            disabled={busy}
            onClick={resetDemo}
            title="Reset servers and session"
            aria-label="Reset servers and session"
          >
            <Icon name="reset" size={13} />
          </button>
        )}
      </div>
    </div>
  );
}

export function Shell({ children }: { children: ReactNode }) {
  const { screen, persona, toast } = useStore();
  const [navOpen, setNavOpen] = useState(false);

  useEffect(() => {
    document.title = `${TITLES[screen]} · Gary${persona === 'host' ? ' for hosting teams' : ''}`;
  }, [screen, persona]);

  return (
    <div className="app">
      <Sidebar open={navOpen} onClose={() => setNavOpen(false)} />
      <div className="stage">
        <TopBar onMenu={() => setNavOpen(true)} />
        <main id="main" className={`main main-${screen}`}>
          <div key={`${persona}-${screen}`} className="screen-wrap">
            {children}
          </div>
        </main>
      </div>
      <DevHarnessOverlay />
      {toast && (
        <div className="toast" role="status">
          {toast}
        </div>
      )}
    </div>
  );
}
