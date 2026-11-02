/** Whether the rail is collapsed, remembered per browser. */
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useState,
  type ReactNode,
} from 'react'

/**
 * Whether the navigation rail is showing labels or just icons — remembered.
 *
 * A chrome preference, so it works like the theme: written to localStorage and
 * read back synchronously on first render, because a rail that visibly snaps from
 * wide to narrow on every load is worse than one that never collapses.
 *
 * Collapsing is a *desktop* idea. Below `md` the rail is not a column beside the
 * content, it is an overlay drawer opened from the top bar — a 64px drawer of
 * unlabelled icons would be a worse drawer, not a smaller one. So the stored
 * preference is gated on the viewport: it is still remembered while you are on a
 * phone, it just doesn't apply there (and the toggle isn't offered).
 */
const STORAGE_KEY = 'gary-rail-collapsed'

/** 768px — the width at which the rail stops being an overlay drawer. */
const DESKTOP = '(min-width: 768px)'

export interface SidebarCollapse {
  /** Is the rail icons-only *right now*? Always false while it's a drawer. */
  collapsed: boolean
  /** Is collapsing on offer here at all? False below `md`, which hides the toggle. */
  collapsible: boolean
  toggle: () => void
}

const Ctx = createContext<SidebarCollapse | null>(null)

/** Outside the shell (sign-in, the landing page) a full-width rail is the honest
 *  answer, and there is nothing to toggle. */
const EXPANDED: SidebarCollapse = {
  collapsed: false,
  collapsible: false,
  toggle: () => {},
}

function readStored(): boolean {
  try {
    return localStorage.getItem(STORAGE_KEY) === '1'
  } catch {
    return false
  }
}

function isDesktop(): boolean {
  return typeof window !== 'undefined' && window.matchMedia(DESKTOP).matches
}

export function SidebarCollapseProvider({ children }: { children: ReactNode }) {
  const [preferred, setPreferred] = useState(readStored)
  const [desktop, setDesktop] = useState(isDesktop)

  useEffect(() => {
    const mq = window.matchMedia(DESKTOP)
    const onChange = (e: MediaQueryListEvent) => setDesktop(e.matches)
    mq.addEventListener('change', onChange)
    // The viewport may have moved between the first render and this effect.
    setDesktop(mq.matches)
    return () => mq.removeEventListener('change', onChange)
  }, [])

  const toggle = useCallback(() => {
    setPreferred((prev) => {
      const next = !prev
      try {
        localStorage.setItem(STORAGE_KEY, next ? '1' : '0')
      } catch {
        // Storage disabled (private mode, blocked cookies): the rail still
        // collapses for this session, it just won't be remembered. Not worth
        // telling anyone about.
      }
      return next
    })
  }, [])

  return (
    <Ctx.Provider
      value={{ collapsed: preferred && desktop, collapsible: desktop, toggle }}
    >
      {children}
    </Ctx.Provider>
  )
}

export function useSidebarCollapse(): SidebarCollapse {
  return useContext(Ctx) ?? EXPANDED
}
