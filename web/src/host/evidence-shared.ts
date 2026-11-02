/**
 * Pricing and number formatting for the host screens. Rates come from the one shared
 * table (gary/rates.ts); the formula matches turnCost in gary/derive.ts, so a column
 * here and a turn on the owner's screen agree.
 */
import { RATES } from '../gary/rates';

export { RATES, RATES_ARE_PLACEHOLDER } from '../gary/rates';

export function runCost(model: string, prompt: number, cached: number, completion: number): number | null {
  const r = RATES[model];
  if (!r) return null;
  return ((prompt - cached) * r.input_per_1m_usd + cached * r.cached_input_per_1m_usd + completion * r.output_per_1m_usd) / 1_000_000;
}

/** "12.3x", or "<1x" phrasing left to the caller. One decimal under 10, none above. */
export function ratio(a: number, b: number): string {
  const x = a / b;
  return `${x >= 10 ? x.toFixed(0) : x.toFixed(1)}x`;
}

export function pct(v: number, digits = 0): string {
  return `${(v * 100).toFixed(digits)}%`;
}

/** USD with sensible precision for anything from a fraction of a cent to thousands. */
export function usd(v: number): string {
  if (v >= 100) return `$${Math.round(v).toLocaleString('en-US')}`;
  if (v >= 1) return `$${v.toFixed(2)}`;
  if (v >= 0.01) return `$${v.toFixed(3)}`;
  return `$${v.toFixed(5)}`;
}

/** Durations from milliseconds to days, in the largest unit that reads well. */
export function dur(ms: number): string {
  if (ms >= 3_600_000) return `${(ms / 3_600_000).toFixed(ms >= 36_000_000 ? 0 : 1)} h`;
  if (ms >= 60_000) return `${(ms / 60_000).toFixed(1)} min`;
  if (ms >= 1000) return `${(ms / 1000).toFixed(1)} s`;
  return `${Math.round(ms)} ms`;
}
