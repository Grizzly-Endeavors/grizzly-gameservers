/** The shared rate table, minus its `_`-prefixed notes. */
import raw from '../data/models.json';
import type { ModelRate, RateTable } from './types';

export const RATES: RateTable = Object.fromEntries(Object.entries(raw).filter(([k]) => !k.startsWith('_'))) as Record<string, ModelRate>;

/** True while models.json still carries its placeholder note. */
export const RATES_ARE_PLACEHOLDER = '_note' in raw;
