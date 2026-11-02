import { defineConfig, loadEnv } from 'vite';
import react from '@vitejs/plugin-react';

/**
 * Live mode talks to Gary's operator harness through this dev-server proxy, so the
 * bearer token stays in the local `.env` and never reaches the browser bundle.
 * GARY_OPERATOR_URL is the harness base (e.g. a port-forward to the bot's agent port).
 */
export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, process.cwd(), '');
  const target = env.GARY_OPERATOR_URL;
  return {
    plugins: [react()],
    server: target
      ? {
          proxy: {
            '/operator': {
              target,
              changeOrigin: true,
              headers: env.GARY_OPERATOR_TOKEN ? { Authorization: `Bearer ${env.GARY_OPERATOR_TOKEN}` } : {},
            },
          },
        }
      : undefined,
  };
});
