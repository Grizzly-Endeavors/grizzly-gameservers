/** Slides one shared selection wash between rail rows instead of popping a background per row. */
import { useCallback, useEffect, useLayoutEffect, useRef, useState, type CSSProperties } from 'react'

/**
 * The sliding active marker every navigation rail uses.
 *
 * One shared element rather than one per row, so moving between items *slides*
 * it to the new selection (the house "swoop") instead of popping it on and off.
 * Its position is measured from whichever row the DOM marks as current; it snaps
 * rather than slides on first paint and after the rail was empty, and fades out
 * entirely when nothing matches.
 *
 * Shared by the shell Sidebar and the settings rail so the two can't drift. The
 * only things that differ are how "current" is spelled in the markup (`activeSelector`)
 * and how tall the marker sits against the row (`inset`).
 *
 * Both rails' marker is not an edge-bar but the row's own accent wash, so it must
 * be exactly the height of the row it marks — hence `inset` 0. The default suits a
 * short edge-bar.
 */
export interface RailIndicator {
  top: number
  height: number
  visible: boolean
  /** Slide to the new spot (true) vs. snap there (first show / after hiding). */
  animate: boolean
}

/** Inline style for the bar. Keyed off `animate` so a snap doesn't transition. */
export function railIndicatorStyle(i: RailIndicator): CSSProperties {
  return {
    top: i.top,
    height: i.height,
    opacity: i.visible ? 1 : 0,
    transition: i.animate
      ? 'top var(--dur-slow) var(--ease-swoop), height var(--dur-slow) var(--ease-swoop), opacity var(--dur-base) var(--ease-out-soft)'
      : 'opacity var(--dur-base) var(--ease-out-soft)',
  }
}

export function useRailIndicator<T extends HTMLElement>(
  /** CSS selector for the active row inside the ref'd container. */
  activeSelector: string,
  /** Re-measure whenever any of these change (route, active id, nav contents). */
  deps: unknown[],
  /** How far the marker is inset from the row's top and bottom edges. `0` makes
   *  it exactly the row (a full-row wash); `8` shortens it into a centred bar. */
  inset = 8,
) {
  const ref = useRef<T>(null)
  /** The marker node itself, so `follow` can paint it without a render. */
  const markerRef = useRef<HTMLElement>(null)
  const [indicator, setIndicator] = useState<RailIndicator>({
    top: 0,
    height: 0,
    visible: false,
    animate: false,
  })
  // Was the bar showing on the previous measure? Decides slide vs. snap.
  const wasVisible = useRef(false)

  const measure = useCallback(
    (animateOverride?: boolean) => {
      const root = ref.current
      if (!root) return
      const active = root.querySelector<HTMLElement>(activeSelector)
      if (!active) {
        wasVisible.current = false
        setIndicator((prev) =>
          prev.visible ? { ...prev, visible: false, animate: false } : prev,
        )
        return
      }
      const animate = animateOverride ?? wasVisible.current
      wasVisible.current = true
      setIndicator({
        top: active.offsetTop + inset,
        height: active.offsetHeight - inset * 2,
        visible: true,
        animate,
      })
    },
    [activeSelector, inset],
  )

  useLayoutEffect(() => {
    measure()
    // `deps` is the caller's own change signal; measure is stable.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [measure, ...deps])

  /**
   * Re-measure every frame for `ms`, following a row that is being *animated* to
   * a new position rather than jumping to one.
   *
   * A single measurement is right when the rows are already where they are going
   * to be — a route change, a space switch. It is wrong when the rail is
   * mid-transition: the shell's collapse walks the groups upward on a delay,
   * so a lone measurement taken at the state change reads the row's OLD position
   * and leaves the wash stranded behind its row for the length of the animation.
   *
   * Deliberately not a `transitionend` listener. That would fire once, at the
   * end, and the wash would then slide to catch up *after* the row had already
   * arrived — a second motion, chasing the first. Following frame by frame keeps
   * the wash welded to its row, which is the only way it reads as the row's own
   * background rather than as a separate thing tracking it.
   *
   * Writes `top`/`height` **straight to the marker node** instead of going through
   * state. Routing 60 measurements a second through `setState` costs a render each
   * and lands them a frame late, and a frame is not small here: the rows travel
   * ~7px per frame at the peak of the walk, so the wash would trail its own row by
   * up to 12.5px — clearly visible as the background sliding out from under the
   * icon it belongs to. Painting directly removes the lag entirely. One `setState` at
   * the end hands the final position back to React so the next render agrees.
   *
   * `markerRef` is optional: without it this degrades to the state path, which is
   * correct, just a frame behind.
   */
  const follow = useCallback(
    (ms: number) => {
      const started = performance.now()
      let raf = 0
      const step = () => {
        const root = ref.current
        const node = markerRef.current
        const active = root?.querySelector<HTMLElement>(activeSelector)
        if (node && active) {
          // No transition while being driven — it is being positioned each frame,
          // not animated toward anything.
          node.style.transition = 'none'
          node.style.top = `${active.offsetTop + inset}px`
          node.style.height = `${active.offsetHeight - inset * 2}px`
        } else {
          measure(false)
        }
        if (performance.now() - started < ms) {
          raf = requestAnimationFrame(step)
        } else {
          measure(false)
        }
      }
      raf = requestAnimationFrame(step)
      return () => cancelAnimationFrame(raf)
    },
    [measure, activeSelector, inset],
  )

  // Row geometry shifts with the viewport width; reposition without a slide.
  useEffect(() => {
    const onResize = () => measure(false)
    window.addEventListener('resize', onResize)
    return () => window.removeEventListener('resize', onResize)
  }, [measure])

  return {
    ref,
    markerRef,
    indicator,
    style: railIndicatorStyle(indicator),
    follow,
  }
}
