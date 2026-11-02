/**
 * The frame every screen sits in: the rail, a top bar, and the scrolling content.
 * The frame is fixed and only <main> scrolls, so the rail and top bar never move.
 *
 * One app, two audiences. The hosting company sees its fleet and the evidence; the
 * server's owner sees their server and Gary. The switch in the top bar flips between
 * them for the demo; each keeps its own rail.
 */
import { useEffect, useState, type ReactNode, type Ref } from 'react';
import { INCIDENTS } from '../host/fleet';
import { Icon, type IconName } from './icons';
import { RailTip } from './RailTip';
import { useSidebarCollapse } from './sidebar-collapse';
import { useStore, type Persona, type Screen } from './store';
import { useTheme } from './theme';
import { useRailIndicator } from './useRailIndicator';

const COLLAPSE_SEQUENCE_MS = 460;

const TITLES: Record<Screen, string> = {
  overview: 'Overview',
  servers: 'Servers',
  incidents: 'Incidents',
  arena: 'Model arena',
  benchmark: 'Benchmark',
  gary: 'Ask Gary',
  activity: 'Activity',
};

interface Row {
  key: Screen;
  label: string;
  icon: IconName;
  count?: number;
  hot?: boolean;
}

/**
 * The owner's rail is about Gary, not about one server: one Gary looks after all of
 * them from one conversation, and the servers themselves sit in the conversation's
 * side panel. A count on Ask Gary means he is waiting on an approval.
 */
function railGroups(persona: Persona, open: number, waiting: number): { label: string; rows: Row[] }[] {
  if (persona === 'customer')
    return [
      {
        label: 'Friday Night Crew',
        rows: [
          { key: 'gary', label: 'Ask Gary', icon: 'ask', count: waiting, hot: true },
          { key: 'activity', label: 'Activity', icon: 'activity' },
        ],
      },
    ];
  return [
    {
      label: 'Operations',
      rows: [
        { key: 'overview', label: 'Overview', icon: 'home' },
        { key: 'servers', label: 'Servers', icon: 'server' },
        { key: 'incidents', label: 'Incidents', icon: 'bolt', count: open, hot: true },
      ],
    },
    {
      label: 'Evidence',
      rows: [
        { key: 'arena', label: 'Model arena', icon: 'versus' },
        { key: 'benchmark', label: 'Benchmark', icon: 'chart' },
      ],
    },
  ];
}

/** Gary's mark: a rounded speech square with a spark, in the accent. */
export function GaryMark({ size = 22 }: { size?: number }) {
  return (
    <svg viewBox="0 0 24 24" width={size} height={size} aria-hidden="true">
      <rect x="1.5" y="1.5" width="21" height="21" rx="6" fill="var(--primary-bg)" />
      <path d="M12 6.2v3.1M12 14.7v3.1M6.2 12h3.1M14.7 12h3.1M8.4 8.4l1.6 1.6M14 14l1.6 1.6M8.4 15.6l1.6-1.6M14 10l1.6-1.6" stroke="#fff" strokeWidth="1.7" strokeLinecap="round" />
    </svg>
  );
}

function Sidebar({ open, onClose }: { open: boolean; onClose: () => void }) {
  const { persona, screen, go, turns, servers } = useStore();
  const { collapsed, collapsible, toggle } = useSidebarCollapse();
  const waiting = turns.filter((t) => t.approval?.state === 'pending').length;
  const groups = railGroups(persona, INCIDENTS.filter((i) => i.outcome === 'escalated').length, waiting);
  const sig = `${persona}|${groups.map((g) => g.rows.map((r) => r.key).join(',')).join('|')}`;
  const { ref: navRef, markerRef, style: indicatorStyle, follow } = useRailIndicator<HTMLElement>('button[aria-current="page"]', [screen, sig, collapsed], 0);
  useEffect(() => follow(COLLAPSE_SEQUENCE_MS), [collapsed, follow]);
  const who = persona === 'host' ? { name: 'Pinecrest Hosting', sub: 'Support operations', initials: 'PH' } : { name: 'Sam Rivera', sub: `Server owner · ${servers.length} servers`, initials: 'SR' };

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
            <span className="rail-sub">{persona === 'host' ? 'for hosting teams' : 'your server operator'}</span>
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
          <div className="rail-me">
            <span className="rail-avatar" aria-hidden="true">
              {who.initials}
            </span>
            <span className="rail-label rail-me-text">
              <span className="rail-me-name">{who.name}</span>
              <span className="rail-me-mail">{who.sub}</span>
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

/** Live or Replay. Replay plays bundled traces so a demo never waits on the network. */
function ModePill() {
  const { mode, setMode, busy } = useStore();
  const next = mode === 'live' ? 'replay' : 'live';
  const label = mode === 'live' ? 'Live cluster' : 'Recorded replay';
  return (
    <button
      type="button"
      className={`sys-pill ${mode === 'live' ? 'sys-ok' : 'sys-off'}`}
      onClick={() => setMode(next)}
      disabled={busy}
      title={`Switch to ${next === 'live' ? 'the live cluster' : 'recorded replay'}`}
    >
      <span className="sys-dot" aria-hidden="true" />
      <span>{label}</span>
    </button>
  );
}

function TopBar({ onMenu }: { onMenu: () => void }) {
  const { screen } = useStore();
  return (
    <header className="topbar">
      <button type="button" className="icon-btn topbar-menu" onClick={onMenu} aria-label="Open navigation">
        <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" strokeWidth={1.75} strokeLinecap="round" aria-hidden="true">
          <path d="M4 7h16M4 12h16M4 17h16" />
        </svg>
      </button>
      <h1 className="topbar-title">{TITLES[screen]}</h1>
      <div className="topbar-right">
        <PersonaSwitch />
        <ModePill />
        <ThemeToggle />
      </div>
    </header>
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
      {toast && (
        <div className="toast" role="status">
          {toast}
        </div>
      )}
    </div>
  );
}
