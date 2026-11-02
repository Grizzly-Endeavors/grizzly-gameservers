/**
 * The demo scenarios (PRD "Demo Scenario Bar"). Each carries the server's state before
 * and after, the complaint a player would type, and a GaryResponse-shaped trace that
 * Replay mode plays back.
 *
 * `recorded: false` marks a trace written by hand to the harness's shape. Swap in a
 * real `just operator gary` capture and flip it to true; the UI labels the difference.
 */
import type { GaryResponse, GenerationTrace, ToolTrace } from '../gary/types';

export const GARY_MODEL = 'accounts/fireworks/models/glm-5p2';
export const FRONTIER_MODEL = 'claude-opus-5-5';

export type ServerStatus = 'Running' | 'Starting' | 'Stopped' | 'Crashed';

export interface ServerState {
  status: ServerStatus;

  detail: string;
  players: [number, number];
  memory: [number, number];
  /** The newest lines of latest.log, shown on the card when something is wrong. */
  logTail?: string[];
  restarts: number;
}

export interface ArenaRun {
  system_id: 'frontier_cc' | 'gary_fireworks' | 'ablation_cc';
  model: string;
  rounds: number;
  tools: string[];
  latency_ms: number;
  prompt_tokens: number;
  cached_tokens: number;
  completion_tokens: number;
  verified: boolean;
  reply: string;
}

export interface Scenario {
  id: string;
  /** Which of the owner's servers this scenario breaks. */
  server: string;
  label: string;
  short: string;
  prompt: string;
  before: ServerState;
  after: ServerState;
  response: GaryResponse;
  /** Set when the turn stops at an Approval Card: what Approve runs, and its result. */
  approval?: {
    tool: string;
    title: string;
    blast: string;
    approveLabel: string;
    approved: GaryResponse;
    denied: GaryResponse;
  };
  recorded: boolean;
  arena: ArenaRun[];
}

function gen(prompt: number, cached: number, completion: number, latency: number, finish = 'tool_calls'): GenerationTrace {
  return {
    model: GARY_MODEL,
    prompt_tokens: prompt,
    cached_tokens: cached,
    completion_tokens: completion,
    total_tokens: prompt + completion,
    finish_reason: finish,
    latency_ms: latency,
  };
}

function respond(reply: string, tool_calls: ToolTrace[], generations: GenerationTrace[]): GaryResponse {
  const sum = (k: 'prompt_tokens' | 'cached_tokens' | 'completion_tokens') => generations.reduce((n, g) => n + (g[k] ?? 0), 0);
  const prompt = sum('prompt_tokens');
  const completion = sum('completion_tokens');
  return {
    reply,
    escalated: false,
    escalation_kind: null,
    escalation_reason: null,
    rounds: generations.length,
    generations,
    usage: { prompt_tokens: prompt, cached_tokens: sum('cached_tokens'), completion_tokens: completion, total_tokens: prompt + completion },
    latency_ms: generations.reduce((n, g) => n + g.latency_ms, 0),
    tool_calls,
  };
}

const t = (name: string, a: Record<string, unknown>, result: string): ToolTrace => ({ name, arguments: JSON.stringify(a), result });

const healthy: ServerState = {
  status: 'Running',
  detail: 'Fabric 1.21.1',
  players: [4, 10],
  memory: [3.2, 4],
  restarts: 2,
};

const CREATIVE: ServerState = { status: 'Running', detail: 'Vanilla 1.21.1', players: [2, 8], memory: [1.9, 3], restarts: 0 };

export const SCENARIOS: Scenario[] = [
  {
    id: 'crash-fabric-api',
    server: 'mc-friends-smp',
    label: 'Crash: missing Fabric API',
    short: 'Missing mod',
    prompt: "The SMP won't start after I added Iris shaders, can you fix it?",
    before: {
      status: 'Crashed',
      detail: 'Startup crashed · exit code 1',
      players: [0, 10],
      memory: [0.4, 4],
      restarts: 3,
      logTail: [
        '[main/ERROR]: Incompatible mods found!',
        "net.fabricmc.loader.impl.FormattedException: Mod 'Iris' (iris) 1.8.1 requires any version of mod 'fabric-api'",
        "  - Install 'fabric-api', any version.",
        '[main/INFO]: Stopping server',
      ],
    },
    after: { ...healthy, players: [0, 10], memory: [2.6, 4], restarts: 4, detail: 'Fabric 1.21.1 · ready in 48 s' },
    recorded: false,
    response: respond(
      "**mc-friends-smp** crashed on startup because Iris needs **Fabric API**, which wasn't installed. I added Fabric API to your mod list, restarted the server, and it came back online in 48 seconds. Nothing else changed.",
      [
        t('read_logs', { name: 'mc-friends-smp', lines: 120 }, "Mod 'Iris' (iris) 1.8.1 requires any version of mod 'fabric-api', which is missing!\n[main/INFO]: Stopping server"),
        t('mod_info', { game: 'minecraft', slug: 'fabric-api' }, 'Fabric API (fabric-api) · Modrinth · latest 0.116.6+1.21.1 · loader: fabric · server-side: required'),
        t('read_file', { name: 'mc-friends-smp', path: '.grizzly/env' }, '1 MODRINTH_PROJECTS=lithium,iris,sodium\n2 MAX_MEMORY=4G\n(2 lines)'),
        t(
          'edit_file',
          { name: 'mc-friends-smp', path: '.grizzly/env', old_text: 'MODRINTH_PROJECTS=lithium,iris,sodium', new_text: 'MODRINTH_PROJECTS=lithium,iris,sodium,fabric-api' },
          'Saved a snapshot of .grizzly/env, then edited it (1 line changed).',
        ),
        t('restart_server', { name: 'mc-friends-smp' }, 'Restarted. The server reported ready after 48 s and is healthy.'),
      ],
      [gen(9120, 8704, 96, 412), gen(9384, 9088, 71, 288), gen(9560, 9344, 88, 301), gen(9810, 9536, 142, 377), gen(10020, 9792, 64, 266), gen(10210, 9984, 118, 352, 'stop')],
    ),
    arena: [
      {
        system_id: 'frontier_cc',
        model: FRONTIER_MODEL,
        rounds: 11,
        tools: ['Bash ls', 'Bash find', 'Read logs/latest.log', 'Bash grep', 'Read .grizzly/env', 'WebSearch', 'Edit .grizzly/env', 'Bash restart', 'Bash tail'],
        latency_ms: 38400,
        prompt_tokens: 212000,
        cached_tokens: 160000,
        completion_tokens: 4100,
        verified: true,
        reply: 'Fixed: Iris requires Fabric API. Added `fabric-api` to MODRINTH_PROJECTS and restarted; the server is up.',
      },
      {
        system_id: 'gary_fireworks',
        model: GARY_MODEL,
        rounds: 6,
        tools: ['read_logs', 'mod_info', 'read_file', 'edit_file', 'restart_server'],
        latency_ms: 1996,
        prompt_tokens: 58104,
        cached_tokens: 56448,
        completion_tokens: 579,
        verified: true,
        reply: 'Iris needed Fabric API. Added it, restarted, and the server came back online in 48 seconds.',
      },
      {
        system_id: 'ablation_cc',
        model: GARY_MODEL,
        rounds: 17,
        tools: ['Bash ls', 'Bash find', 'Read server.properties', 'Bash find mods', 'Read logs/latest.log', 'Bash grep', 'Write mods/fabric-api.jar', 'Bash restart'],
        latency_ms: 9800,
        prompt_tokens: 341000,
        cached_tokens: 250000,
        completion_tokens: 6900,
        verified: false,
        reply: 'I downloaded fabric-api into the mods folder. Please restart the server to apply it.',
      },
    ],
  },
  {
    id: 'lag-entity',
    server: 'mc-friends-smp',
    label: 'Lag: entity spike',
    short: 'Lag',
    prompt: 'Why is the SMP so laggy right now?',
    before: {
      status: 'Running',
      detail: 'Fabric 1.21.1 · TPS 11.4',
      players: [6, 10],
      memory: [3.8, 4],
      restarts: 2,
      logTail: ["[Server thread/WARN]: Can't keep up! Is the server overloaded? Running 4127ms or 82 ticks behind"],
    },
    after: { ...healthy, players: [6, 10], memory: [3.1, 4], detail: 'Fabric 1.21.1 · TPS 19.9' },
    recorded: false,
    response: respond(
      "The server was falling behind because about **9,400 dropped items** piled up near spawn (it looks like a farm overflowing). I cleared the dropped items with a console command; ticks are back to normal. Nobody was disconnected. If it comes back, the farm near spawn needs a collection hopper.",
      [
        t('server_resources', { name: 'mc-friends-smp' }, 'CPU 1.92 / 2.00 cores · memory 3.8 / 4.0 GiB'),
        t('read_logs', { name: 'mc-friends-smp', lines: 80 }, "Can't keep up! Is the server overloaded? Running 4127ms or 82 ticks behind (x14 in the last 5 min)"),
        t('send_command', { name: 'mc-friends-smp', command: 'execute if entity @e[type=item]' }, 'Test passed, count: 9412'),
        t('send_command', { name: 'mc-friends-smp', command: 'kill @e[type=item]' }, 'Killed 9412 entities'),
      ],
      [gen(9010, 8704, 84, 351), gen(9240, 8960, 66, 270), gen(9420, 9216, 73, 284), gen(9590, 9344, 59, 262), gen(9700, 9472, 131, 398, 'stop')],
    ),
    arena: [
      {
        system_id: 'frontier_cc',
        model: FRONTIER_MODEL,
        rounds: 8,
        tools: ['Bash top', 'Read logs/latest.log', 'Bash grep', 'Bash rcon list', 'Bash rcon kill'],
        latency_ms: 27100,
        prompt_tokens: 148000,
        cached_tokens: 112000,
        completion_tokens: 3000,
        verified: true,
        reply: 'TPS was low from ~9.4k item entities. Cleared them via RCON.',
      },
      {
        system_id: 'gary_fireworks',
        model: GARY_MODEL,
        rounds: 5,
        tools: ['server_resources', 'read_logs', 'send_command', 'send_command'],
        latency_ms: 1565,
        prompt_tokens: 46960,
        cached_tokens: 45696,
        completion_tokens: 413,
        verified: true,
        reply: 'About 9,400 dropped items piled up near spawn. Cleared them; ticks are back to normal.',
      },
      {
        system_id: 'ablation_cc',
        model: GARY_MODEL,
        rounds: 12,
        tools: ['Bash top', 'Bash ps', 'Read server.properties', 'Edit server.properties', 'Bash restart'],
        latency_ms: 7400,
        prompt_tokens: 236000,
        cached_tokens: 170000,
        completion_tokens: 5100,
        verified: false,
        reply: 'Lowered view-distance to 6 and restarted. That should reduce lag.',
      },
    ],
  },
  {
    id: 'world-corruption',
    server: 'mc-friends-smp',
    label: 'World corruption (approval)',
    short: 'Corrupt world',
    prompt: "The SMP crashed while saving and won't load the world.",
    before: {
      status: 'Crashed',
      detail: 'Crash loop · 3 restarts in 5 min',
      players: [0, 10],
      memory: [1.1, 4],
      restarts: 5,
      logTail: ['[Server thread/ERROR]: Failed to read chunk [12, -7]', 'java.io.EOFException: region/r.0.-1.mca: unexpected end of file', '[main/ERROR]: Exception stopping the server'],
    },
    after: { ...healthy, players: [0, 10], memory: [2.4, 4], restarts: 6, detail: 'Restored 10:00 backup · ready in 52 s' },
    recorded: false,
    response: respond(
      "The world file for one area near spawn (`region/r.0.-1.mca`) was cut off mid-save, so the server can't load it. The cleanest fix is to restore this morning's **10:00** backup, which loses about **22 minutes** of play. I need your OK before I touch the world.",
      [
        t('read_logs', { name: 'mc-friends-smp', lines: 120 }, 'java.io.EOFException: region/r.0.-1.mca: unexpected end of file\nFailed to read chunk [12, -7]'),
        t('run_shell', { name: 'mc-friends-smp', command: 'ls -l world/region | sort -k5 -n | head -3' }, '-rw-r--r-- 1 game game     4096 Nov  2 10:21 r.0.-1.mca\n-rw-r--r-- 1 game game  1847296 Nov  2 10:22 r.0.0.mca'),
        t('list_backups', { name: 'mc-friends-smp' }, '2026-11-02T10:00Z (22 min ago, 412 MB)\n2026-11-01T22:00Z (12 h ago, 409 MB)'),
      ],
      [gen(9160, 8704, 90, 402), gen(9410, 9088, 112, 341), gen(9640, 9344, 77, 288), gen(9880, 9600, 156, 421, 'stop')],
    ),
    approval: {
      tool: 'restore_server',
      title: 'Restore the world from the 10:00 backup?',
      blast: 'Rolls the world back 22 minutes. Anything built or mined since 10:00 is lost. The current world is kept as a backup first.',
      approveLabel: 'Approve restore',
      approved: respond(
        'Restored the 10:00 backup and kept the broken world as its own backup in case you want anything from it. The server loaded cleanly and has been healthy for a minute.',
        [
          t('backup_server', { name: 'mc-friends-smp', label: 'pre-restore' }, 'Backup saved: 2026-11-02T10:22Z (pre-restore)'),
          t('restore_server', { name: 'mc-friends-smp', backup: '2026-11-02T10:00Z', confirm: true }, 'Restored 2026-11-02T10:00Z. Restarted; ready after 52 s and healthy.'),
        ],
        [gen(10110, 9856, 70, 296), gen(10290, 10112, 64, 274), gen(10420, 10240, 98, 331, 'stop')],
      ),
      denied: respond(
        "OK, I won't touch the world. Another option is to delete just the damaged area so the game regenerates it from scratch; that keeps everyone's other progress but loses whatever was built in that spot. Or I can hand this to staff.",
        [],
        [gen(10080, 9856, 91, 318, 'stop')],
      ),
    },
    arena: [
      {
        system_id: 'frontier_cc',
        model: FRONTIER_MODEL,
        rounds: 9,
        tools: ['Read logs/latest.log', 'Bash ls region', 'Bash ls backups', 'AskUser'],
        latency_ms: 31800,
        prompt_tokens: 171000,
        cached_tokens: 129000,
        completion_tokens: 3600,
        verified: true,
        reply: 'r.0.-1.mca is truncated. Recommend restoring the 10:00 backup. Proceed?',
      },
      {
        system_id: 'gary_fireworks',
        model: GARY_MODEL,
        rounds: 4,
        tools: ['read_logs', 'run_shell', 'list_backups'],
        latency_ms: 1452,
        prompt_tokens: 38090,
        cached_tokens: 36736,
        completion_tokens: 435,
        verified: true,
        reply: 'One region file was cut off mid-save. Restoring the 10:00 backup loses about 22 minutes. Need your OK.',
      },
      {
        system_id: 'ablation_cc',
        model: GARY_MODEL,
        rounds: 10,
        tools: ['Read logs/latest.log', 'Bash rm region/r.0.-1.mca', 'Bash restart'],
        latency_ms: 6100,
        prompt_tokens: 198000,
        cached_tokens: 140000,
        completion_tokens: 4200,
        verified: false,
        reply: 'Deleted the corrupted region file and restarted the server.',
      },
    ],
  },
  {
    id: 'false-alarm',
    server: 'mc-creative',
    label: 'False alarm: client issue',
    short: 'False alarm',
    prompt: 'My friend gets "connection timed out" on the creative server. Is it broken?',
    before: CREATIVE,
    after: CREATIVE,
    recorded: false,
    response: respond(
      "**mc-creative** is fine: it's up, 2 players are on, and nothing in the logs points to a problem. Your friend's game is on **1.21.3**, but the server runs **1.21.1**, and their launcher shows that as a timeout. They should pick the 1.21.1 profile in their launcher. I didn't change anything.",
      [
        t('about_server', { name: 'mc-creative' }, 'mc-creative · Running · 2/8 players · Vanilla 1.21.1 · up 3 h 12 m'),
        t('read_logs', { name: 'mc-creative', lines: 60 }, 'Disconnecting /203.0.113.24: Outdated server! I\'m still on 1.21.1 (client 1.21.3)'),
      ],
      [gen(9020, 8704, 79, 334), gen(9210, 8960, 68, 262), gen(9390, 9216, 124, 377, 'stop')],
    ),
    arena: [
      {
        system_id: 'frontier_cc',
        model: FRONTIER_MODEL,
        rounds: 6,
        tools: ['Bash ps', 'Read logs/latest.log', 'Bash grep'],
        latency_ms: 18900,
        prompt_tokens: 96000,
        cached_tokens: 72000,
        completion_tokens: 2200,
        verified: true,
        reply: 'Server is healthy; the friend is on client 1.21.3 vs server 1.21.1.',
      },
      {
        system_id: 'gary_fireworks',
        model: GARY_MODEL,
        rounds: 3,
        tools: ['about_server', 'read_logs'],
        latency_ms: 973,
        prompt_tokens: 27620,
        cached_tokens: 26880,
        completion_tokens: 271,
        verified: true,
        reply: "Server's fine. Your friend is on 1.21.3; the server runs 1.21.1. No changes made.",
      },
      {
        system_id: 'ablation_cc',
        model: GARY_MODEL,
        rounds: 9,
        tools: ['Bash ps', 'Read server.properties', 'Edit server.properties', 'Bash restart'],
        latency_ms: 5200,
        prompt_tokens: 162000,
        cached_tokens: 118000,
        completion_tokens: 3900,
        verified: false,
        reply: 'Raised network-compression-threshold and restarted the server to fix the timeouts.',
      },
    ],
  },
];

/** The owner's servers. One Gary looks after all of them from one conversation. */
export interface OwnedServer {
  id: string;
  game: string;
  state: ServerState;
  /** From the live harness: only name, game, state and address are known. */
  live?: boolean;
}

export const SERVERS: OwnedServer[] = [
  { id: 'mc-friends-smp', game: 'Minecraft', state: healthy },
  { id: 'mc-creative', game: 'Minecraft', state: CREATIVE },
  { id: 'valheim-vikings', game: 'Valheim', state: { status: 'Running', detail: 'Valheim 0.219.16', players: [3, 10], memory: [5.1, 8], restarts: 1 } },
  { id: 'terraria-weekend', game: 'Terraria', state: { status: 'Stopped', detail: 'Terraria 1.4.4.9 · asleep, wakes on join', players: [0, 8], memory: [0, 2], restarts: 0 } },
];
