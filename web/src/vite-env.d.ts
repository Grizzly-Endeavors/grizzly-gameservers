/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** "1" in the build the server serves: the app talks to the real API. */
  readonly VITE_LIVE?: string;
}

interface Window {
  __GARY__?: { channel?: string };
}
