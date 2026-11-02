import { defineConfig, loadEnv } from 'vite';
import react from '@vitejs/plugin-react';

/**
 * Live mode talks to Gary's operator harness through this dev-server proxy, so the
 * bearer token stays server-side and never reaches the browser bundle.
 *
 * Settings come from web/.env, falling back to the repo-root .env that
 * scripts/operator.sh already reads, so anyone who drives the bot needs nothing new.
 * The default target is the port-forward that script opens (localhost:19360).
 */
export default defineConfig(({ mode }) => {
  const root = loadEnv(mode, '..', '');
  const web = loadEnv(mode, process.cwd(), '');
  const target = web.GARY_OPERATOR_URL || root.GARY_OPERATOR_URL || 'http://localhost:19360';
  const token = web.GARY_OPERATOR_TOKEN || root.GAMESERVERS_OPERATOR_TOKEN || '';
  const channel = web.VITE_GARY_CHANNEL || root.GAMESERVERS_OPERATOR_CHANNEL || '';
  return {
    plugins: [react()],
    define: { 'import.meta.env.VITE_GARY_CHANNEL': JSON.stringify(channel) },
    server: {
      proxy: {
        '/operator': {
          target,
          changeOrigin: true,
          headers: token ? { Authorization: `Bearer ${token}` } : {},
          // A Gary turn can run for minutes; don't let the proxy cut it off first.
          timeout: 900_000,
          proxyTimeout: 900_000,
        },
      },
    },
  };
});
