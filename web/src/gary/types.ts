/**
 * The wire contracts the UI reads. `GaryRequest`/`GaryResponse` mirror
 * crates/bot/src/http/operator.rs; the eval and rate-table shapes are the ones frozen
 * in docs/fireworks-deploy-prd.md (FR-4.3). A token count of `null` means the provider
 * reported none: unknown, never zero.
 */

export type Access = 'admin' | 'manager' | 'read-only';

export interface GaryRequest {
  prompt: string;
  new_session: boolean;
  channel: string;
  access: Access;
  confirm: boolean;
}

export interface GenerationTrace {
  model: string;
  prompt_tokens: number | null;
  cached_tokens: number | null;
  completion_tokens: number | null;
  total_tokens: number | null;
  finish_reason: string | null;
  latency_ms: number;
}

export interface UsageTotals {
  prompt_tokens: number | null;
  cached_tokens: number | null;
  completion_tokens: number | null;
  total_tokens: number | null;
}

export interface ToolTrace {
  name: string;
  /** The model's raw JSON argument string, verbatim. */
  arguments: string;
  /** The dispatcher's text result, byte-identical to what the model read. */
  result: string;
}

export interface GaryResponse {
  reply: string;
  escalated: boolean;
  escalation_kind: 'agent_requested' | 'round_budget_exhausted' | null;
  escalation_reason: string | null;
  rounds: number;
  generations: GenerationTrace[];
  usage: UsageTotals;
  latency_ms: number;
  tool_calls: ToolTrace[];
}

export interface ErrorBody {
  error: string;
}

/** Per-1M-token USD rates, keyed by model id. Shared with the eval scorer. */
export interface ModelRate {
  label: string;
  provider: string;
  input_per_1m_usd: number;
  cached_input_per_1m_usd: number;
  output_per_1m_usd: number;
}
export type RateTable = Record<string, ModelRate>;

export type SystemKind = 'measured' | 'estimated';
export type Harness = 'gary' | 'claude_code' | 'human';

export interface EvalSystem {
  system_id: string;
  label: string;
  kind: SystemKind;
  model: string;
  harness: Harness;
  n_cases: number;
  verified_success_rate: number;
  success_ci_95: [number, number];
  unintended_change_rate: number;
  mean_turns: number | null;
  mean_tokens: number | null;
  latency_p50_ms: number;
  latency_p95_ms: number;
  cost_per_1k_usd: number;
  notes: string;
}

export interface EvalCaseResult {
  verified_success: boolean;
  unintended_changes: number;
  turns: number;
  latency_ms: number;
  cost_usd: number;
  tool_calls: string[];
}

export interface EvalCase {
  case_id: string;
  game: string;
  task_area: 'config' | 'troubleshooting';
  category: string;
  prompt: string;
  results_by_system: Record<string, EvalCaseResult>;
}

export interface EvalResults {
  /** Set on the bundled placeholder so the UI can say so; absent on real results. */
  sample?: boolean;
  generated_at?: string;
  summary: { systems: EvalSystem[] };
  cases: EvalCase[];
}
