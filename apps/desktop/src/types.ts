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
  cost: number | null;
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
  tag: { key: string; value: string | null } | null;
  include_network: boolean;
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
  unit: string;
}

export type ProviderKind = "none" | "open_ai_compatible" | "anthropic" | "bedrock";

export interface LlmSettings {
  provider: ProviderKind;
  base_url: string;
  model: string;
  redact: boolean | null;
  bedrock_account_id: string | null;
  bedrock_region: string;
}

export interface Citation {
  n: number;
  tool: string;
  input: Record<string, unknown>;
  result: unknown;
  ok: boolean;
}

export interface Turn {
  conversation_id: string;
  answer: string;
  citations: Citation[];
  stop: string;
  model: string;
  redacted: boolean;
  masked_values: number;
  input_tokens: number;
  output_tokens: number;
  /** How the question was interpreted, from the tool calls. */
  assumptions: string[];
  /** Workspace context revision sent with the question (0 = none). */
  context_revision: number;
  /** Figures in the answer that match nothing in the data. */
  unverified_figures: string[];
}

export type AssistantProgress = { workspace_id: string } & (
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
  created_at: string;
}
