// SPDX-License-Identifier: MIT OR Apache-2.0
// Mirrors the serialized types of `afinin-core`.

export type ProfileKind =
  | "sso"
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

export type CostMetric = "unblended" | "amortized" | "net_unblended";
export type Granularity = "daily" | "monthly";

export type GroupBy =
  | { kind: "account" }
  | { kind: "service" }
  | { kind: "region" }
  | { kind: "usage_type" }
  | { kind: "tag"; key: string };

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
}

export interface ApiUsage {
  service: string;
  operation: string;
  calls: number;
  cost_usd: number;
}
