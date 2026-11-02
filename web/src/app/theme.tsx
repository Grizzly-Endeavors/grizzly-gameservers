import { createContext, useCallback, useContext, useState, type ReactNode } from 'react';

/**
 * Light or dark, remembered.
 *
 * The palette lives entirely in CSS custom properties (styles/tokens.css); dark is
 * those same tokens redefined under a `.dark` class on <html>, so flipping the class
 * re-skins the whole app with no per-component restyle.
 *
 * Dark is the default: an operator console is left open on a big screen all day.
 * Light is an opt-in that is then remembered. It deliberately does
 * not follow the OS setting.
 *
 * index.html puts the class on <html> before React mounts, so there's no flash of
 * the other theme. This provider reads that starting state back out of the DOM,
 * and on toggle writes the change back to <html> and localStorage.
 */
type Theme = 'light' | 'dark';

const STORAGE_KEY = 'gary-theme';

const ThemeCtx = createContext<{ theme: Theme; toggle: () => void } | null>(null);

function readInitial(): Theme {
  return typeof document !== 'undefined' && document.documentElement.classList.contains('dark') ? 'dark' : 'light';
}

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [theme, setTheme] = useState<Theme>(readInitial);

  const toggle = useCallback(() => {
    setTheme((prev) => {
      const next = prev === 'dark' ? 'light' : 'dark';
      document.documentElement.classList.toggle('dark', next === 'dark');
      document.querySelector('meta[name="theme-color"]')?.setAttribute('content', next === 'dark' ? '#141517' : '#f6f7f8');
      try {
        localStorage.setItem(STORAGE_KEY, next);
      } catch {
        // Storage disabled: the toggle still works for this session, it just
        // won't be remembered.
      }
      return next;
    });
  }, []);

  return <ThemeCtx.Provider value={{ theme, toggle }}>{children}</ThemeCtx.Provider>;
}

export function useTheme() {
  const ctx = useContext(ThemeCtx);
  if (!ctx) throw new Error('useTheme must be used within ThemeProvider');
  return ctx;
}
