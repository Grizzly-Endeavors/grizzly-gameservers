/**
 * Everything the UI derives from a GaryResponse: a plain-English line per tool call,
 * where the turn got to in the safety loop, and what it cost. Gary's response carries
 * no card markup, so the cards are read off the tool trace he already returns.
 */
import type { GaryResponse, RateTable, ToolTrace } from './types';

/** Tools that change a server. Everything else only reads. */
const MUTATING = new Set([
  'edit_file',
  'write_file',
  'restore_file',
  'update_mods',
  'set_resources',
  'restart_server',
  'start_server',
  'shutdown_server',
  'restore_server',
  'recover_server',
  'archive_server',
  'destroy_server',
  'import_profile',
  'send_command',
]);

/** World-touching actions the UI holds behind an explicit Approve (PRD US-2). */
export const NEEDS_APPROVAL = new Set(['restore_server', 'restore_file', 'archive_server', 'destroy_server']);

function args(t: ToolTrace): Record<string, unknown> {
  try {
    const v: unknown = JSON.parse(t.arguments || '{}');
    return v && typeof v === 'object' ? (v as Record<string, unknown>) : {};
  } catch {
    return {};
  }
}

function str(v: unknown): string | undefined {
  return typeof v === 'string' || typeof v === 'number' ? String(v) : undefined;
}

/** One readable line for a tool call, e.g. "Read server logs (last 120 lines)". */
export function describeTool(t: ToolTrace): string {
  const a = args(t);
  const path = str(a.path);
  const server = str(a.server ?? a.name);
  switch (t.name) {
    case 'read_logs':
      return `Read server logs${a.lines ? ` (last ${str(a.lines)} lines)` : ''}`;
    case 'about_server':
      return `Checked server status${server ? ` for ${server}` : ''}`;
    case 'server_resources':
      return 'Checked CPU and memory';
    case 'list_servers':
      return 'Listed servers';
    case 'read_file':
      return `Read ${path ?? 'a file'}`;
    case 'browse_files':
      return `Browsed ${path ?? 'the server files'}`;
    case 'run_shell':
      return `Searched files: ${str(a.command) ?? 'shell'}`;
    case 'edit_file':
      return `Edited ${path ?? 'a file'}`;
    case 'write_file':
      return `Wrote ${path ?? 'a file'}`;
    case 'restore_file':
      return `Restored ${path ?? 'a file'} from its snapshot`;
    case 'search_mods':
      return `Searched mods for "${str(a.query) ?? ''}"`;
    case 'mod_info':
      return `Looked up mod ${str(a.slug ?? a.project ?? a.query) ?? ''}`.trim();
    case 'update_mods':
      return `${a.mode === 'check' ? 'Checked' : 'Applied'} mod updates`;
    case 'set_resources':
      return `Resized server${a.memory ? ` to ${str(a.memory)}` : ''}`;
    case 'restart_server':
      return 'Restarted the server and waited for it to be ready';
    case 'start_server':
      return 'Started the server';
    case 'shutdown_server':
      return 'Shut the server down';
    case 'backup_server':
      return 'Took a backup';
    case 'list_backups':
      return 'Listed backups';
    case 'restore_server':
      return `Restored the world from backup${a.backup ? ` ${str(a.backup)}` : ''}`;
    case 'send_command':
      return `Sent console command: ${str(a.command) ?? ''}`;
    case 'game_guide':
      return 'Loaded the game guide';
    case 'escalate':
      return 'Handed off to staff';
    default:
      return t.name.replace(/_/g, ' ');
  }
}

/** The servers a turn's tool calls named, in first-touched order. */
export function serversTouched(calls: ToolTrace[]): string[] {
  const seen: string[] = [];
  for (const t of calls) {
    const s = str(args(t).server);
    if (s && !seen.includes(s)) seen.push(s);
  }
  return seen;
}

export function isMutating(t: ToolTrace): boolean {
  return MUTATING.has(t.name);
}

export type SafetyStage = 'snapshot' | 'apply' | 'verify' | 'done';
export interface SafetyState {
  /** False when the turn only read: the stepper shows "No change needed". */
  mutated: boolean;
  reached: SafetyStage[];
  outcome: 'healthy' | 'rolled_back' | 'pending' | null;
}

/**
 * Where the turn got to in snapshot → apply → verify → healthy. The supervisor
 * snapshots before every typed write, so a write implies the snapshot stage.
 */
export function safetyOf(r: GaryResponse | undefined): SafetyState {
  const calls = r?.tool_calls ?? [];
  const writes = calls.filter((t) => isMutating(t) && t.name !== 'restart_server' && t.name !== 'start_server');
  const restarted = calls.some((t) => t.name === 'restart_server' || t.name === 'start_server' || /restart/i.test(t.result));
  const text = calls.map((t) => t.result).join('\n');
  const rolledBack = /rolled back|rollback/i.test(text);
  const healthy = /healthy|ready|back up|came back/i.test(text);
  if (!writes.length && !restarted) return { mutated: false, reached: [], outcome: null };
  const reached: SafetyStage[] = [];
  if (writes.length) reached.push('snapshot', 'apply');
  if (restarted) reached.push('verify');
  if (rolledBack || healthy) reached.push('done');
  return { mutated: true, reached, outcome: rolledBack ? 'rolled_back' : healthy ? 'healthy' : 'pending' };
}

/** USD for one turn, from per-generation usage and the shared rate table. Null if unpriced. */
export function turnCost(r: GaryResponse, rates: RateTable): number | null {
  let total = 0;
  for (const g of r.generations) {
    const rate = rates[g.model];
    if (!rate || g.prompt_tokens == null || g.completion_tokens == null) return null;
    const cached = g.cached_tokens ?? 0;
    total +=
      ((g.prompt_tokens - cached) * rate.input_per_1m_usd + cached * rate.cached_input_per_1m_usd + g.completion_tokens * rate.output_per_1m_usd) /
      1_000_000;
  }
  return total;
}

export function fmtUsd(v: number | null, digits = 5): string {
  if (v == null) return 'unpriced';
  return `$${v.toFixed(digits)}`;
}

export function fmtMs(ms: number): string {
  return ms >= 10_000 ? `${(ms / 1000).toFixed(0)} s` : ms >= 1000 ? `${(ms / 1000).toFixed(1)} s` : `${ms} ms`;
}

export function fmtTokens(n: number | null): string {
  if (n == null) return '?';
  return n >= 10_000 ? `${(n / 1000).toFixed(1)}k` : n.toLocaleString('en-US');
}

/** The model id without the provider's path, e.g. accounts/fireworks/models/glm-5p2 → glm-5p2. */
export function shortModel(id: string): string {
  return id.split('/').pop() ?? id;
}
