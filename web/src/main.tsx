import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { Shell } from './app/Shell';
import { SidebarCollapseProvider } from './app/sidebar-collapse';
import { StoreProvider, useStore } from './app/store';
import { ThemeProvider } from './app/theme';
import { Activity } from './customer/Activity';
import { ServerConsole } from './customer/ServerConsole';
import { Arena } from './host/Arena';
import { Benchmark } from './host/Benchmark';
import { Incidents } from './host/Incidents';
import { Overview } from './host/Overview';
import { Servers } from './host/Servers';
import './styles/tokens.css';
import './styles/base.css';
import './styles/shell.css';
import './styles/ui.css';
import './styles/customer.css';
import './styles/host.css';
import './styles/evidence.css';

function App() {
  const { screen } = useStore();
  return (
    <>
      <a className="skip" href="#main">
        Skip to content
      </a>
      <Shell>
        {screen === 'server' && <ServerConsole />}
        {screen === 'activity' && <Activity />}
        {screen === 'overview' && <Overview />}
        {screen === 'servers' && <Servers />}
        {screen === 'incidents' && <Incidents />}
        {screen === 'arena' && <Arena />}
        {screen === 'benchmark' && <Benchmark />}
      </Shell>
    </>
  );
}

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <ThemeProvider>
      <SidebarCollapseProvider>
        <StoreProvider>
          <App />
        </StoreProvider>
      </SidebarCollapseProvider>
    </ThemeProvider>
  </StrictMode>,
);
