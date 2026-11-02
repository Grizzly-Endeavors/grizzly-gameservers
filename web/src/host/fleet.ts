/**
 * Invented demo data for the hosting team's screens: a fictional host's fleet and the
 * incidents Gary handled across it. None of it is measured, which is why every screen
 * that reads this file wears a "Demo data" flag. Benchmark numbers live elsewhere.
 *
 * Incidents reuse the scenario traces where one fits, so the steps a host sees here
 * are the same shape the owner's console shows.
 */
import { GARY_MODEL, SCENARIOS, type ServerStatus } from '../data/scenarios';
import type { GaryResponse, GenerationTrace, ToolTrace } from '../gary/types';

export interface FleetServer {
  name: string;
  customer: string;
  game: string;
  version: string;
  status: ServerStatus;
  players: [number, number];
  memory: [number, number];
  lastAction: string;
  /** Minutes since Gary last acted here. */
  ago: number;
  attention?: boolean;
}

export type Outcome = 'healthy' | 'rolled_back' | 'escalated' | 'no_change';

export interface Incident {
  id: string;
  server: string;
  customer: string;
  game: string;
  ago: number;
  /** One line a support lead can scan: what was wrong. */
  diagnosis: string;
  outcome: Outcome;
  escalationReason?: string;
  response: GaryResponse;
}

/** What one human T1/T2 ticket costs a host. An industry-style estimate, not measured. */
export const HUMAN_TICKET_USD_ESTIMATE = 6.5;

/** Today's ticket totals across the fleet (demo). */
export const TODAY = { tickets: 212, escalated: 9, medianFixSeconds: 74 };

export const FLEET: FleetServer[] = [
  { name: 'mc-friends-smp', customer: 'Sam Rivera', game: 'Minecraft', version: 'Fabric 1.21.1', status: 'Running', players: [4, 10], memory: [2.6, 4], lastAction: 'Added Fabric API, restarted', ago: 6 },
  { name: 'craftlands', customer: 'Nadia Okafor', game: 'Minecraft', version: 'Paper 1.21.3', status: 'Running', players: [17, 30], memory: [5.1, 8], lastAction: 'Cleared 9,412 dropped items', ago: 14 },
  { name: 'blockbusters', customer: 'Theo Lindqvist', game: 'Minecraft', version: 'Forge 1.20.1', status: 'Crashed', players: [0, 12], memory: [0.3, 6], lastAction: 'Escalated: mod needs a newer Forge', ago: 9, attention: true },
  { name: 'skyblock-crew', customer: 'Priya Natarajan', game: 'Minecraft', version: 'Vanilla 1.21.1', status: 'Running', players: [3, 8], memory: [1.9, 3], lastAction: 'Turned PvP off', ago: 41 },
  { name: 'terra-tuesdays', customer: 'Marcus Bell', game: 'Terraria', version: 'tModLoader 1.4.4', status: 'Running', players: [5, 8], memory: [1.4, 2], lastAction: 'Raised max players to 8', ago: 73 },
  { name: 'calamity-run', customer: 'Jun Park', game: 'Terraria', version: 'tModLoader 1.4.4', status: 'Starting', players: [0, 6], memory: [0.9, 3], lastAction: 'Restored world from 09:00 backup', ago: 3 },
  { name: 'viking-vacation', customer: 'Elena Sokolova', game: 'Valheim', version: '0.219.14', status: 'Running', players: [4, 10], memory: [3.3, 6], lastAction: 'Installed BepInEx pack', ago: 128 },
  { name: 'ashlands-or-bust', customer: 'Diego Alvarez', game: 'Valheim', version: '0.219.14', status: 'Crashed', players: [0, 10], memory: [0.2, 6], lastAction: 'Escalated: disk quota full', ago: 22, attention: true },
  { name: 'pal-party', customer: 'Hannah Weiss', game: 'Palworld', version: 'v0.6.1', status: 'Running', players: [8, 16], memory: [11.2, 16], lastAction: 'Lowered pal spawn rate', ago: 55 },
  { name: 'pal-ranch', customer: 'Kwame Mensah', game: 'Palworld', version: 'v0.6.1', status: 'Stopped', players: [0, 16], memory: [0, 16], lastAction: 'Shut down after 2 h empty', ago: 190 },
  { name: 'factory-must-grow', customer: 'Lena Hoffmann', game: 'Satisfactory', version: '1.1', status: 'Running', players: [2, 4], memory: [9.4, 12], lastAction: 'Checked memory: no change needed', ago: 34 },
  { name: 'spaghetti-belts', customer: 'Owen Gallagher', game: 'Satisfactory', version: '1.1', status: 'Stopped', players: [0, 4], memory: [0, 12], lastAction: 'Backed up before shutdown', ago: 260 },
];

function gen(prompt: number, cached: number, completion: number, latency: number, finish = 'tool_calls'): GenerationTrace {
  return { model: GARY_MODEL, prompt_tokens: prompt, cached_tokens: cached, completion_tokens: completion, total_tokens: prompt + completion, finish_reason: finish, latency_ms: latency };
}

function respond(reply: string, tool_calls: ToolTrace[], generations: GenerationTrace[], escalation_reason: string | null = null): GaryResponse {
  const sum = (k: 'prompt_tokens' | 'cached_tokens' | 'completion_tokens') => generations.reduce((n, g) => n + (g[k] ?? 0), 0);
  return {
    reply,
    escalated: escalation_reason != null,
    escalation_kind: escalation_reason != null ? 'agent_requested' : null,
    escalation_reason,
    rounds: generations.length,
    generations,
    usage: { prompt_tokens: sum('prompt_tokens'), cached_tokens: sum('cached_tokens'), completion_tokens: sum('completion_tokens'), total_tokens: sum('prompt_tokens') + sum('completion_tokens') },
    latency_ms: generations.reduce((n, g) => n + g.latency_ms, 0),
    tool_calls,
  };
}

const t = (name: string, a: Record<string, unknown>, result: string): ToolTrace => ({ name, arguments: JSON.stringify(a), result });
const scenario = (id: string): GaryResponse => SCENARIOS.find((s) => s.id === id)!.response;

export const INCIDENTS: Incident[] = [
  {
    id: 'inc-1041',
    server: 'calamity-run',
    customer: 'Jun Park',
    game: 'Terraria',
    ago: 3,
    diagnosis: 'World file cut off mid-save; owner approved restoring the 09:00 backup',
    outcome: 'healthy',
    response: SCENARIOS.find((s) => s.id === 'world-corruption')!.approval!.approved,
  },
  {
    id: 'inc-1040',
    server: 'mc-friends-smp',
    customer: 'Sam Rivera',
    game: 'Minecraft',
    ago: 6,
    diagnosis: 'Iris shaders needed Fabric API, which was missing',
    outcome: 'healthy',
    response: scenario('crash-fabric-api'),
  },
  {
    id: 'inc-1039',
    server: 'blockbusters',
    customer: 'Theo Lindqvist',
    game: 'Minecraft',
    ago: 9,
    diagnosis: 'Create 6.0 needs Forge 47.3; the server runs 47.1',
    outcome: 'escalated',
    escalationReason: 'Upgrading the Forge loader replaces the server jar, which is outside what I can change safely. A staff member needs to pick the new loader version.',
    response: respond(
      "Your server crashes because the **Create** mod needs a newer Forge than the one installed. Changing Forge is a bigger job than I can do safely, so I've passed this to the support team with everything they need.",
      [
        t('read_logs', { server: 'blockbusters', lines: 120 }, "Mod create requires forge 47.3.0 or above\nCurrently, forge is 47.1.3"),
        t('about_server', { server: 'blockbusters' }, 'blockbusters · Crashed · 0/12 players · Forge 1.20.1 (47.1.3)'),
        t('escalate', { reason: 'loader upgrade required' }, 'Handed off to staff with the log excerpt and the required Forge version.'),
      ],
      [gen(9240, 8960, 92, 388), gen(9470, 9216, 70, 276), gen(9650, 9472, 133, 352, 'stop')],
      'Upgrading the Forge loader replaces the server jar, which is outside what I can change safely. A staff member needs to pick the new loader version.',
    ),
  },
  {
    id: 'inc-1038',
    server: 'craftlands',
    customer: 'Nadia Okafor',
    game: 'Minecraft',
    ago: 14,
    diagnosis: '9,412 dropped items near spawn were slowing every tick',
    outcome: 'healthy',
    response: scenario('lag-entity'),
  },
  {
    id: 'inc-1037',
    server: 'ashlands-or-bust',
    customer: 'Diego Alvarez',
    game: 'Valheim',
    ago: 22,
    diagnosis: 'Disk quota full; world saves are failing',
    outcome: 'escalated',
    escalationReason: 'The volume is at its quota and the plan has no headroom. Old backups or a bigger plan is a billing decision for staff.',
    response: respond(
      "Valheim can't save the world because the server's disk is full. Freeing space means deleting backups or moving to a bigger plan, which needs the support team, so I've handed it over.",
      [
        t('read_logs', { server: 'ashlands-or-bust', lines: 80 }, 'Failed to save world: IOException: No space left on device'),
        t('run_shell', { server: 'ashlands-or-bust', command: 'du -sh worlds_local backups' }, '1.2G\tworlds_local\n8.7G\tbackups'),
        t('escalate', { reason: 'disk quota full' }, 'Handed off to staff.'),
      ],
      [gen(8890, 8704, 81, 364), gen(9120, 8960, 88, 301), gen(9300, 9088, 121, 344, 'stop')],
      'The volume is at its quota and the plan has no headroom. Old backups or a bigger plan is a billing decision for staff.',
    ),
  },
  {
    id: 'inc-1036',
    server: 'factory-must-grow',
    customer: 'Lena Hoffmann',
    game: 'Satisfactory',
    ago: 34,
    diagnosis: 'Memory looked high; it was normal for a late-game save',
    outcome: 'no_change',
    response: respond(
      'Memory is at 9.4 of 12 GB, which is normal for a factory this size, and the server has no warnings. I left everything as it is.',
      [t('server_resources', { server: 'factory-must-grow' }, 'CPU 1.1 / 4 cores · memory 9.4 / 12 GiB'), t('read_logs', { server: 'factory-must-grow', lines: 60 }, '(no warnings in the last 60 lines)')],
      [gen(8960, 8704, 70, 322), gen(9150, 8960, 96, 297, 'stop')],
    ),
  },
  {
    id: 'inc-1035',
    server: 'skyblock-crew',
    customer: 'Priya Natarajan',
    game: 'Minecraft',
    ago: 41,
    diagnosis: 'Owner asked to turn PvP off',
    outcome: 'healthy',
    response: respond(
      'PvP is off. I changed `pvp` in server.properties, restarted, and the server came back in 21 seconds.',
      [
        t('run_shell', { server: 'skyblock-crew', command: 'grep -n pvp server.properties' }, '31:pvp=true'),
        t('edit_file', { server: 'skyblock-crew', path: 'server.properties', find: 'pvp=true', replace: 'pvp=false' }, 'Saved a snapshot of server.properties, then edited it (1 line changed).'),
        t('restart_server', { server: 'skyblock-crew' }, 'Restarted. The server reported ready after 21 s and is healthy.'),
      ],
      [gen(8920, 8704, 74, 318), gen(9100, 8960, 102, 344), gen(9290, 9088, 61, 259), gen(9420, 9216, 77, 281, 'stop')],
    ),
  },
  {
    id: 'inc-1034',
    server: 'pal-party',
    customer: 'Hannah Weiss',
    game: 'Palworld',
    ago: 55,
    diagnosis: 'Bad value for PalSpawnNumRate; rolled back automatically',
    outcome: 'rolled_back',
    response: respond(
      "I lowered the pal spawn rate, but the server refused the new setting on startup, so it **rolled back automatically** to the last working config and is up again. I then set it in the format Palworld expects, and it's now at 0.7.",
      [
        t('edit_file', { server: 'pal-party', path: 'PalWorldSettings.ini', find: 'PalSpawnNumRate=1.000000', replace: 'PalSpawnNumRate=0.7' }, 'Saved a snapshot of PalWorldSettings.ini, then edited it (1 line changed).'),
        t('restart_server', { server: 'pal-party' }, 'The server failed its ready check after the change. Rolled back to the snapshot; restarted and healthy.'),
        t('edit_file', { server: 'pal-party', path: 'PalWorldSettings.ini', find: 'PalSpawnNumRate=1.000000', replace: 'PalSpawnNumRate=0.700000' }, 'Saved a snapshot of PalWorldSettings.ini, then edited it (1 line changed).'),
        t('restart_server', { server: 'pal-party' }, 'Restarted. The server reported ready after 63 s and is healthy.'),
      ],
      [gen(9310, 9088, 118, 402), gen(9520, 9344, 66, 288), gen(9740, 9472, 131, 377), gen(9900, 9728, 64, 264), gen(10070, 9856, 141, 365, 'stop')],
    ),
  },
  {
    id: 'inc-1033',
    server: 'terra-tuesdays',
    customer: 'Marcus Bell',
    game: 'Terraria',
    ago: 73,
    diagnosis: 'Owner asked for 8 player slots instead of 6',
    outcome: 'healthy',
    response: respond(
      'Max players is now 8. I updated serverconfig.txt and restarted; the server is back up.',
      [
        t('read_file', { server: 'terra-tuesdays', path: 'serverconfig.txt' }, '4 maxplayers=6\n(22 lines)'),
        t('edit_file', { server: 'terra-tuesdays', path: 'serverconfig.txt', find: 'maxplayers=6', replace: 'maxplayers=8' }, 'Saved a snapshot of serverconfig.txt, then edited it (1 line changed).'),
        t('restart_server', { server: 'terra-tuesdays' }, 'Restarted. The server reported ready after 17 s and is healthy.'),
      ],
      [gen(8870, 8704, 70, 301), gen(9050, 8832, 96, 333), gen(9200, 9088, 60, 255), gen(9330, 9216, 72, 268, 'stop')],
    ),
  },
  {
    id: 'inc-1032',
    server: 'mc-friends-smp',
    customer: 'Sam Rivera',
    game: 'Minecraft',
    ago: 96,
    diagnosis: "Friend's client on 1.21.3; server fine",
    outcome: 'no_change',
    response: scenario('false-alarm'),
  },
];

export const OUTCOME_LABEL: Record<Outcome, string> = {
  healthy: 'Fixed and verified',
  rolled_back: 'Rolled back, then fixed',
  escalated: 'Escalated to staff',
  no_change: 'No change needed',
};

export const OUTCOME_TONE: Record<Outcome, 'ok' | 'warn' | 'bad' | 'off'> = {
  healthy: 'ok',
  rolled_back: 'warn',
  escalated: 'bad',
  no_change: 'off',
};

export function agoText(min: number): string {
  if (min < 1) return 'just now';
  if (min < 60) return `${min} min ago`;
  const h = Math.floor(min / 60);
  return `${h} h ago`;
}
