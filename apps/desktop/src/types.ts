// SPDX-License-Identifier: MIT OR Apache-2.0
// Mirrors the serialized types of `afinin-core`.

export type ProfileKind =
  | "sso"
  | "login"
  | "assume_role"
  | "credential_process"
  | "web_identity"
  | "static_keys"
  | "unknown";

export interface SsoSettings {
  session_name: string | null;
  start_url: string;
  region: string;
  account_id: string | null;
  role_name: string | null;
}

export interface AwsProfile {
  name: string;
  kind: ProfileKind;
  region: string | null;
  sso: SsoSettings | null;
  role_arn: string | null;
  source_profile: string | null;
}

export interface Workspace {
  id: string;
  name: string;
  created_at: string;
}

export interface ConnectionTest {
  ok: boolean;
  tested_at: string;
  aws_account_id: string | null;
  arn: string | null;
  error: string | null;
  missing_permissions: string[];
}

export interface Account {
  id: string;
  workspace_id: string;
  name: string;
  profile: string;
  kind: ProfileKind;
  aws_account_id: string | null;
  last_test: ConnectionTest | null;
  created_at: string;
  sync: SyncStatus | null;
}

export interface SsoLoginPrompt {
  verification_uri: string;
  verification_uri_complete: string | null;
  user_code: string;
  expires_in: number;
  interval: number;
}

export type SsoLoginStatus = "pending" | "complete" | "expired";

export type SyncState = "running" | "ok" | "error";

export interface SyncStatus {
  state: SyncState;
  started_at: string;
  finished_at: string | null;
  last_success_at: string | null;
  error: string | null;
}

export interface SyncReport {
  api_calls: number;
  estimated_cost_usd: number;
  rows: number;
  warnings: string[];
}

export interface AccountSyncReport {
  costs: SyncReport | null;
  inventory: { regions: number; resources: number; warnings: string[] } | null;
  errors: string[];
}

export type CostMetric = "unblended" | "amortized" | "net_unblended";
export type Granularity = "daily" | "monthly";

export type GroupBy =
  | { kind: "account" }
  | { kind: "service" }
  | { kind: "region" }
  | { kind: "usage_type" }
  | { kind: "tag"; key: string }
  | { kind: "resource" };

export interface CostFilter {
  linked_accounts: string[];
  services: string[];
}

export interface CostQuery {
  workspace_id: string;
  start: string;
  end: string;
  granularity: Granularity;
  metric: CostMetric;
  group_by: GroupBy;
  filter: CostFilter;
}

export interface CostGroup {
  key: string;
  label: string;
  values: number[];
  total: number;
}

export interface CostSeries {
  periods: string[];
  groups: CostGroup[];
  period_totals: number[];
  total: number;
  unit: string;
  estimated_periods: string[];
}

export interface CostComparisonRow {
  key: string;
  label: string;
  current: number;
  previous: number;
  delta: number;
  delta_pct: number | null;
}

export interface CostOverview {
  metric: CostMetric;
  unit: string;
  month_to_date: number;
  previous_month_to_date: number;
  month_over_month_pct: number | null;
  last_month_total: number;
  forecast_month_end: number | null;
  data_through: string | null;
}

export interface DimensionValue {
  key: string;
  label: string;
}

export interface Settings {
  sync_interval_hours: number;
  daily_history_days: number;
  monthly_history_months: number;
  cost_tag_keys: string[];
  default_metric: CostMetric;
  inventory_enabled: boolean;
  inventory_regions: string[];
  anomaly_sensitivity: "low" | "medium" | "high";
  snapshot_age_days: number;
  commitments_enabled: boolean;
  import_aws_anomalies: boolean;
  llm: LlmSettings;
  retention_months: number;
  theme: "system" | "light" | "dark";
}

export interface ApiUsage {
  service: string;
  operation: string;
  calls: number;
  cost_usd: number;
}

export type Tier = "edge" | "balancing" | "routing" | "compute" | "storage" | "data" | "messaging" | "network";
export type RelationKind =
  | "routes_to"
  | "attached"
  | "triggers"
  | "publishes_to"
  | "in_network"
  | "secured_by"
  | "member_of"
  | "owns";

export type CostBasis = "service" | "resource" | "list_price";

export interface GraphNode {
  id: string;
  resource_type: string;
  type_label: string;
  tier: Tier;
  label: string;
  native_id: string;
  account_id: string;
  region: string;
  vpc_id: string | null;
  tags: Record<string, string>;
  attrs: Record<string, unknown>;
  /** Own cost (CUR), public monthly price, or the service total when it is the service's only resource. */
  cost: number | null;
  cost_basis: CostBasis | null;
  /** Cost Explorer service, and its 30-day total in the account (shared by its resources). */
  service: string | null;
  service_cost: number | null;
  /** The tool that made it (infrastructure as code), when there is evidence. */
  managed_by: ManagedBy | null;
  context: boolean;
}

export interface GraphEdge {
  source: string;
  target: string;
  kind: RelationKind;
}

export interface Graph {
  nodes: GraphNode[];
  edges: GraphEdge[];
  total_resources: number;
  cost_basis: "service" | "resource";
  unit: string;
}

export interface GraphFilter {
  accounts: string[];
  regions: string[];
  vpcs: string[];
  types: string[];
  /** `missing`: resources without the key (or with an empty value). */
  tag: { key: string; value: string | null; missing?: boolean } | null;
  include_network: boolean;
  /** Tools that made the resources (`IacTool` ids), or "none". */
  managed_by: string[];
}

export interface Facet {
  key: string;
  label: string;
  count: number;
}

export interface InventoryFacets {
  accounts: Facet[];
  regions: Facet[];
  vpcs: Facet[];
  types: Facet[];
  tag_keys: Facet[];
  /** Tools that made the resources, and "none". */
  managed_by: Facet[];
  total: number;
}

export interface Application {
  value: string;
  resource_count: number;
  resource_types: Record<string, number>;
  cost: number | null;
}

export interface InventoryScan {
  account_id: string;
  scan_region: string;
  scanned_at: string;
  resource_count: number;
  warnings: string[];
}

export interface CurConfig {
  account_id: string;
  bucket: string;
  prefix: string;
  export_name: string;
  region: string;
}

export interface CurValidation {
  checks: { status: "ok" | "warning" | "error"; message: string }[];
  detected: CurConfig | null;
  billing_periods: string[];
}

export interface CurStatus {
  configs: CurConfig[];
  billing_periods: string[];
  first_date: string | null;
  last_date: string | null;
  last_ingested_at: string | null;
}

export type Severity = "low" | "medium" | "high";
export type FindingKind = "waste" | "anomaly" | "pattern";
export type FindingStatus =
  | { status: "open" }
  | { status: "dismissed"; note: string | null }
  | { status: "snoozed"; until: string };

export interface Finding {
  id: string;
  rule: string;
  kind: FindingKind;
  severity: Severity;
  title: string;
  summary: string;
  how_to_fix: string;
  account_id: string;
  region: string;
  resources: { arn: string; label: string; resource_type: string | null }[];
  monthly_savings: number | null;
  savings_basis: "actual" | "public_price" | null;
  impact: number | null;
  evidence: Record<string, unknown>;
  first_seen: string | null;
  status: FindingStatus;
}

export interface CommitmentSummary {
  kind: "savings_plans" | "reservations";
  /** Reservations: the service they cover. */
  service?: string | null;
  coverage_pct: number | null;
  utilization_pct: number | null;
  on_demand_cost: number | null;
  unused_commitment: number | null;
  start: string;
  end: string;
}

export interface Insights {
  top_movers: CostComparisonRow[];
  data_transfer: { key: string; label: string; total: number }[];
  data_transfer_total: number;
  commitments: [string, CommitmentSummary][];
  /** How much of the inventory was made as code. */
  iac: IacSummary;
  unit: string;
}

export type IacTool =
  | "cdk"
  | "cloud_formation"
  | "sam"
  | "serverless"
  | "elastic_beanstalk"
  | "copilot"
  | "amplify"
  | "terraform"
  | "pulumi";

export interface ManagedBy {
  tool: IacTool;
  confidence: "certain" | "likely" | "hint";
  /** Why, in words: "tag aws:cloudformation:stack-name = api". */
  evidence: string;
  stack: string | null;
}

export interface IacSummary {
  /** Resources that count (network interfaces, snapshots and log groups excluded). */
  total: number;
  by_tool: { tool: IacTool; label: string; resources: number; certain: number }[];
  unmanaged: number;
  unmanaged_by_type: { resource_type: string; label: string; count: number }[];
}

export type ProviderKind = "none" | "open_ai_compatible" | "anthropic" | "bedrock";

export interface LlmSettings {
  provider: ProviderKind;
  base_url: string;
  model: string;
  redact: boolean | null;
  bedrock_account_id: string | null;
  bedrock_region: string;
  /** Prices by model ID, USD per million tokens. */
  prices: Record<string, TokenPrice>;
  /** Stop asking once this month's estimated assistant cost reaches it. */
  monthly_budget_usd: number | null;
}

/** USD per million tokens. Cache prices default to 0.1× / 1.25× input. */
export interface TokenPrice {
  input: number;
  output: number;
  cache_read: number | null;
  cache_write: number | null;
}

export interface Citation {
  n: number;
  tool: string;
  input: Record<string, unknown>;
  result: unknown;
  ok: boolean;
  /** What the call did, in words; empty for answers saved before traces. */
  label: string;
  /** Time running the tool locally. */
  ms: number;
  /** Size of the result as sent to the model, in characters. */
  sent_chars: number;
  /** Same call as this earlier source: not run again, the model got a pointer. */
  duplicate_of: number | null;
}

/** One request to the model within an answer. */
export interface Round {
  n: number;
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_write_tokens: number;
  ms: number;
  stop: string;
  sources: number[];
  log_id: number | null;
  cost_usd: number | null;
}

export type TraceFlag =
  | { kind: "repeated_call"; label: string; times: number }
  | { kind: "round_limit"; rounds: number }
  | { kind: "tool_errors"; count: number }
  | { kind: "large_result"; source: number; tokens: number };

export interface Turn {
  conversation_id: string;
  answer: string;
  citations: Citation[];
  stop: string;
  model: string;
  redacted: boolean;
  masked_values: number;
  /** Input tokens at the full rate (excludes cache reads and writes), all rounds. */
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_write_tokens: number;
  rounds: Round[];
  /** Patterns in the trace that cost tokens for little gain. */
  flags: TraceFlag[];
  /** Estimated from the model's price in Settings. */
  cost_usd: number | null;
  elapsed_ms: number;
  /** How the question was interpreted, from the tool calls. */
  assumptions: string[];
  /** Workspace context revision sent with the question (0 = none). */
  context_revision: number;
  /** Figures in the answer that match nothing in the data. */
  unverified_figures: string[];
  /** Saved turn id, for ratings; null if it couldn't be saved. */
  turn_id: number | null;
  /** Latest successful sync when the answer was given. */
  data_as_of: string | null;
  /** Data the answer needed but Afinín doesn't have. */
  missing_data: MissingData[];
  /** Reference answers whose queries guided this one. */
  reference_examples: number;
}

export type MissingNeed = "cur" | "inventory_sync" | "cost_allocation_tags" | "longer_history" | "other";

export interface MissingData {
  need: MissingNeed;
  detail: string;
}

export interface LlmUsage {
  provider: string;
  model: string;
  requests: number;
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_write_tokens: number;
  cost_usd: number | null;
}

export type Rating = "up" | "down";

export interface ConversationSummary {
  id: string;
  title: string;
  created_at: string;
  updated_at: string;
  turns: number;
}

export interface StoredTurn {
  id: number;
  question: string;
  screen: ScreenContext | null;
  turn: Turn;
  rating: Rating | null;
  rating_note: string | null;
  /** Kept as a reference answer to re-check. */
  reference: boolean;
  /** Values a re-check's answer must state (figures or text). */
  expected: string[];
  created_at: string;
}

export interface Recheck {
  turn_id: number;
  question: string;
  before: Turn;
  after: Turn;
  queries_removed: string[];
  queries_added: string[];
  /** The reference's expected values, checked against the new answer. */
  expectations: { expected: string; met: boolean }[];
}

/** A stage of a running sync, for the account `account_id`. */
export type SyncProgress = { account_id: string } & (
  | { stage: "connecting" | "costs" | "cur" | "findings" }
  | { stage: "inventory"; done: number; total: number }
);

/** `recheck_turn_id` is set while a reference answer is being re-checked. */
export type AssistantProgress = { workspace_id: string; recheck_turn_id?: number } & (
  | { stage: "thinking"; round: number }
  | { stage: "tool"; name: string; label: string }
);

/** A tag (`team`, `env=prod`) or business term and what it means. */
export interface ContextEntry {
  key: string;
  meaning: string;
}

export interface WorkspaceContext {
  business: string;
  /** Linked AWS account ID → what it is used for. */
  accounts: Record<string, string>;
  tags: ContextEntry[];
  glossary: ContextEntry[];
  instructions: string;
}

/** What the user is looking at, sent with each question. */
export interface ScreenContext {
  view: string;
  details: ContextEntry[];
}

/** A place in the app an assistant answer can open. `end` is exclusive. */
export type NavTarget =
  | {
      view: "costs";
      start: string;
      end: string;
      groupBy: GroupBy;
      metric: CostMetric | null;
      accounts: string[];
      services: string[];
      compare: boolean;
    }
  | { view: "inventory"; arn: string };

export interface StoredContext {
  context: WorkspaceContext;
  revision: number;
  updated_at: string | null;
}

export interface ContextRevision {
  revision: number;
  created_at: string;
  context: WorkspaceContext;
}

export interface LlmLogEntry {
  id: number;
  conversation_id: string;
  provider: string;
  model: string;
  redacted: boolean;
  request: string;
  response: string;
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_write_tokens: number;
  /** Round within its answer; null for older entries. */
  round: number | null;
  latency_ms: number | null;
  created_at: string;
}
