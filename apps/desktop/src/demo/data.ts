// SPDX-License-Identifier: MIT OR Apache-2.0
// Sample data for demo mode: Brightlane, a fictional booking SaaS on AWS.
// Every name, account ID and figure here is made up.

import type {
  Account,
  CostComparisonRow,
  CostGroup,
  CostOverview,
  CostQuery,
  CostSeries,
  DimensionValue,
  Finding,
  Graph,
  GraphEdge,
  GraphFilter,
  GraphNode,
  Insights,
  InventoryFacets,
  RelationKind,
  Settings,
  StoredTurn,
  Tier,
  Turn,
  Workspace,
} from "../types";

const DAY = 86_400_000;
const iso = (d: Date) => d.toISOString().slice(0, 10);
const parse = (s: string) => new Date(`${s}T00:00:00Z`);
export const today = parse(iso(new Date()));
const daysAgo = (n: number) => new Date(Date.now() - n * DAY).toISOString();

export const WORKSPACE: Workspace = { id: "ws-brightlane", name: "Brightlane", created_at: daysAgo(120) };

export const ACCOUNTS_META = [
  { id: "111122223333", name: "Brightlane Production", profile: "brightlane-prod", share: 0.74 },
  { id: "444455556666", name: "Brightlane Staging", profile: "brightlane-staging", share: 0.14 },
  { id: "777788889999", name: "Brightlane Data", profile: "brightlane-data", share: 0.12 },
];

export const ACCOUNTS: Account[] = ACCOUNTS_META.map((a, i) => ({
  id: `acct-${i + 1}`,
  workspace_id: WORKSPACE.id,
  name: a.name,
  profile: a.profile,
  kind: "sso",
  aws_account_id: a.id,
  last_test: {
    ok: true,
    tested_at: daysAgo(0.2),
    aws_account_id: a.id,
    arn: `arn:aws:sts::${a.id}:assumed-role/AfininReadOnly/demo`,
    error: null,
    missing_permissions: [],
  },
  created_at: daysAgo(118),
  sync: {
    state: "ok",
    started_at: daysAgo(0.1),
    finished_at: daysAgo(0.09),
    last_success_at: daysAgo(0.09),
    error: null,
  },
}));

export const SETTINGS: Settings = {
  sync_interval_hours: 24,
  daily_history_days: 60,
  monthly_history_months: 12,
  cost_tag_keys: ["app", "env"],
  default_metric: "unblended",
  inventory_enabled: true,
  inventory_regions: [],
  anomaly_sensitivity: "medium",
  snapshot_age_days: 90,
  commitments_enabled: true,
  import_aws_anomalies: false,
  usage_metrics: true,
  llm: {
    provider: "anthropic",
    base_url: "http://localhost:11434/v1",
    model: "",
    redact: null,
    bedrock_account_id: null,
    bedrock_region: "us-east-1",
    prices: {},
    monthly_budget_usd: 20,
  },
  retention_months: 0,
  theme: "system",
};

// ---- costs -----------------------------------------------------------------

/** Daily spend per service: base, and growth per day (a slow climb). */
const SERVICES: { name: string; base: number; growth: number; weekend: number }[] = [
  { name: "Amazon Elastic Compute Cloud - Compute", base: 34, growth: 0.06, weekend: 0.95 },
  { name: "Amazon Relational Database Service", base: 21.5, growth: 0.02, weekend: 1 },
  { name: "Amazon Elastic Container Service", base: 9.2, growth: 0.05, weekend: 0.9 },
  { name: "EC2 - Other", base: 8.1, growth: 0.03, weekend: 1 },
  { name: "Amazon Elastic Load Balancing", base: 5.6, growth: 0.005, weekend: 0.97 },
  { name: "Amazon CloudFront", base: 4.8, growth: 0.02, weekend: 0.8 },
  { name: "Amazon ElastiCache", base: 4.1, growth: 0, weekend: 1 },
  { name: "Amazon Simple Storage Service", base: 3.6, growth: 0.01, weekend: 1 },
  { name: "Amazon DynamoDB", base: 2.9, growth: 0.01, weekend: 1 },
  { name: "AmazonCloudWatch", base: 2.4, growth: 0.015, weekend: 1 },
  { name: "AWS Lambda", base: 1.3, growth: 0.01, weekend: 0.85 },
  { name: "Amazon Virtual Private Cloud", base: 1.1, growth: 0, weekend: 1 },
  { name: "AWS Secrets Manager", base: 0.4, growth: 0, weekend: 1 },
  { name: "Amazon Route 53", base: 0.2, growth: 0, weekend: 1 },
];

/** A small, repeatable wobble so days don't look identical. */
const wobble = (seed: number) => 1 + 0.06 * Math.sin(seed * 12.9898) * Math.cos(seed * 4.1414);

function daily(serviceIdx: number, day: Date): number {
  const s = SERVICES[serviceIdx];
  const n = Math.round((day.getTime() - today.getTime()) / DAY);
  const weekend = [0, 6].includes(day.getUTCDay()) ? s.weekend : 1;
  // A data-processing spike last week, worth an anomaly finding.
  const spike = s.name === "EC2 - Other" && n >= -9 && n <= -7 ? 2.4 : 1;
  return Math.max(0, (s.base + s.growth * n) * weekend * wobble(serviceIdx * 997 + n) * spike);
}

const SPLITS: Record<string, { key: string; label: string; share: number }[]> = {
  account: ACCOUNTS_META.map((a) => ({ key: a.id, label: a.name, share: a.share })),
  region: [
    { key: "us-east-1", label: "us-east-1", share: 0.78 },
    { key: "eu-west-1", label: "eu-west-1", share: 0.17 },
    { key: "global", label: "global", share: 0.05 },
  ],
  usage_type: [
    { key: "BoxUsage:m7i.xlarge", label: "EC2 instance hours: m7i.xlarge", share: 0.31 },
    { key: "InstanceUsage:db.r6g.large", label: "RDS instance hours: db.r6g.large", share: 0.2 },
    { key: "Fargate-vCPU-Hours:perCPU", label: "Fargate vCPU hours", share: 0.09 },
    { key: "NatGateway-Bytes", label: "NAT gateway data processed", share: 0.06 },
    { key: "DataTransfer-Out-Bytes", label: "Data transfer out to the internet", share: 0.07 },
    { key: "TimedStorage-ByteHrs", label: "S3 standard storage", share: 0.04 },
    { key: "LoadBalancerUsage", label: "Load balancer hours", share: 0.05 },
    { key: "Other", label: "Other usage types", share: 0.18 },
  ],
  "tag:app": [
    { key: "booking-api", label: "booking-api", share: 0.52 },
    { key: "web", label: "web", share: 0.21 },
    { key: "data-pipeline", label: "data-pipeline", share: 0.14 },
    { key: "", label: "(untagged)", share: 0.13 },
  ],
  "tag:env": [
    { key: "prod", label: "prod", share: 0.76 },
    { key: "staging", label: "staging", share: 0.15 },
    { key: "", label: "(untagged)", share: 0.09 },
  ],
};

function splitFor(q: CostQuery) {
  const g = q.group_by;
  if (g.kind === "service") return null;
  if (g.kind === "tag") return SPLITS[`tag:${g.key}`] ?? [{ key: "", label: "(untagged)", share: 1 }];
  if (g.kind === "resource") return SPLITS.usage_type;
  return SPLITS[g.kind];
}

function accountShare(q: CostQuery): number {
  const ids = q.filter.linked_accounts;
  if (!ids.length) return 1;
  return ACCOUNTS_META.filter((a) => ids.includes(a.id)).reduce((s, a) => s + a.share, 0);
}

function periodsOf(q: CostQuery): { label: string; days: Date[] }[] {
  const start = parse(q.start);
  const end = parse(q.end);
  const days: Date[] = [];
  for (let t = start.getTime(); t < end.getTime() && t < today.getTime() + DAY; t += DAY) days.push(new Date(t));
  if (q.granularity === "daily") return days.map((d) => ({ label: iso(d), days: [d] }));
  const months = new Map<string, Date[]>();
  for (const d of days) {
    const key = `${iso(d).slice(0, 7)}-01`;
    months.set(key, [...(months.get(key) ?? []), d]);
  }
  return [...months].map(([label, ds]) => ({ label, days: ds }));
}

export function costSeries(q: CostQuery): CostSeries {
  const periods = periodsOf(q);
  const services = SERVICES.map((s, i) => ({ ...s, i })).filter(
    (s) => !q.filter.services.length || q.filter.services.includes(s.name),
  );
  const share = accountShare(q);
  const perPeriod = (idx: number) =>
    periods.map((p) => p.days.reduce((sum, d) => sum + daily(idx, d), 0) * share);
  let groups: CostGroup[];
  const split = splitFor(q);
  if (!split) {
    groups = services.map((s) => {
      const values = perPeriod(s.i);
      return { key: s.name, label: s.name, values, total: values.reduce((a, b) => a + b, 0) };
    });
  } else {
    const totals = periods.map((_, pi) => services.reduce((sum, s) => sum + perPeriod(s.i)[pi], 0));
    const parts =
      q.group_by.kind === "account" && q.filter.linked_accounts.length
        ? split.filter((x) => q.filter.linked_accounts.includes(x.key))
        : split;
    const norm = parts.reduce((s, x) => s + x.share, 0);
    // Wobble each part, then rescale so every period still adds up to the
    // same total as grouping by service.
    // Accounts keep their fixed shares, so grouping by account and filtering
    // to one give the same figures.
    const wob = (gi: number, pi: number) => (q.group_by.kind === "account" ? 1 : wobble(gi * 31 + pi));
    const raw = parts.map((x, gi) => totals.map((t, pi) => t * (x.share / norm) * wob(gi, pi)));
    const scale = totals.map((t, pi) => {
      const sum = raw.reduce((s, values) => s + values[pi], 0);
      return sum ? t / sum : 0;
    });
    groups = parts.map((x, gi) => {
      const values = raw[gi].map((v, pi) => v * scale[pi]);
      return { key: x.key, label: x.label, values, total: values.reduce((a, b) => a + b, 0) };
    });
  }
  groups.sort((a, b) => b.total - a.total);
  const period_totals = periods.map((_, pi) => groups.reduce((s, g) => s + g.values[pi], 0));
  const recent = periods
    .filter((p) => p.days.some((d) => today.getTime() - d.getTime() < 2 * DAY))
    .map((p) => p.label);
  return {
    periods: periods.map((p) => p.label),
    groups,
    period_totals,
    total: period_totals.reduce((a, b) => a + b, 0),
    unit: "USD",
    estimated_periods: recent,
  };
}

export function compareCosts(q: CostQuery, prevStart: string, prevEnd: string): CostComparisonRow[] {
  const now = costSeries({ ...q, granularity: "monthly" });
  const before = costSeries({ ...q, start: prevStart, end: prevEnd, granularity: "monthly" });
  return now.groups.map((g) => {
    const previous = before.groups.find((b) => b.key === g.key)?.total ?? 0;
    const current = g.total;
    return {
      key: g.key,
      label: g.label,
      current,
      previous,
      delta: current - previous,
      delta_pct: previous ? ((current - previous) / previous) * 100 : null,
    };
  });
}

export function costOverview(linkedAccounts: string[]): CostOverview {
  const first = new Date(Date.UTC(today.getUTCFullYear(), today.getUTCMonth(), 1));
  const prevFirst = new Date(Date.UTC(today.getUTCFullYear(), today.getUTCMonth() - 1, 1));
  const span = Math.max(1, Math.round((today.getTime() - first.getTime()) / DAY));
  const q = (start: Date, end: Date): CostQuery => ({
    workspace_id: WORKSPACE.id,
    start: iso(start),
    end: iso(end),
    granularity: "daily",
    metric: "unblended",
    group_by: { kind: "service" },
    filter: { linked_accounts: linkedAccounts, services: [] },
  });
  // Early in a month, show the month so far through today.
  const mtd = costSeries(q(first, new Date(today.getTime() + DAY))).total;
  const prevMtd = costSeries(q(prevFirst, new Date(prevFirst.getTime() + (span + 1) * DAY))).total;
  const lastMonth = costSeries(q(prevFirst, first)).total;
  const daysInMonth = new Date(Date.UTC(today.getUTCFullYear(), today.getUTCMonth() + 1, 0)).getUTCDate();
  return {
    metric: "unblended",
    unit: "USD",
    month_to_date: mtd,
    previous_month_to_date: prevMtd,
    month_over_month_pct: prevMtd ? ((mtd - prevMtd) / prevMtd) * 100 : null,
    last_month_total: lastMonth,
    forecast_month_end: (lastMonth / 30) * daysInMonth * 1.04,
    data_through: iso(today),
  };
}

export function dimensionValues(kind: string, key?: string): DimensionValue[] {
  if (kind === "service") return SERVICES.map((s) => ({ key: s.name, label: s.name }));
  const split = kind === "tag" ? SPLITS[`tag:${key}`] : SPLITS[kind];
  return (split ?? []).filter((x) => x.key).map((x) => ({ key: x.key, label: x.label }));
}

// ---- inventory ---------------------------------------------------------------

const PROD = "111122223333";
const STAGING = "444455556666";

function node(
  id: string,
  resource_type: string,
  type_label: string,
  tier: Tier,
  label: string,
  extra: Partial<GraphNode> = {},
): GraphNode {
  return {
    id: `arn:aws:${resource_type.split(":")[0]}:us-east-1:${PROD}:${id}`,
    resource_type,
    type_label,
    tier,
    label,
    native_id: id,
    account_id: PROD,
    region: "us-east-1",
    vpc_id: "vpc-0b1a2c3d",
    tags: { app: "booking-api", env: "prod" },
    attrs: {},
    cost: null,
    cost_basis: null,
    service: null,
    service_cost: null,
    managed_by: { tool: "cdk", confidence: "certain", evidence: "stack BookingApiStack", stack: "BookingApiStack" },
    context: false,
    ...extra,
  };
}

const N = {
  cdn: node("distribution/E2DEMO", "cloudfront:distribution", "CloudFront distribution", "edge", "app.brightlane.example", {
    vpc_id: null,
    region: "global",
    tags: { app: "web", env: "prod" },
    cost: 148.2,
    cost_basis: "resource",
  }),
  alb: node("loadbalancer/app/booking-prod", "elasticloadbalancing:loadbalancer", "Load balancer", "balancing", "booking-prod-alb", {
    cost: 121.4,
    cost_basis: "resource",
    attrs: { type: "application", scheme: "internet-facing" },
  }),
  tg: node("targetgroup/booking-api", "elasticloadbalancing:targetgroup", "Target group", "routing", "booking-api-tg"),
  ecs: node("service/booking/api", "ecs:service", "ECS service", "compute", "booking-api", {
    cost: 268.9,
    cost_basis: "resource",
    attrs: { launch_type: "FARGATE", desired_count: 4 },
  }),
  web1: node("instance/i-0a11", "ec2:instance", "EC2 instance", "compute", "web-1", {
    tags: { app: "web", env: "prod", Name: "web-1" },
    cost: 140.2,
    cost_basis: "resource",
    attrs: { instance_type: "m7i.xlarge", state: "running" },
  }),
  web2: node("instance/i-0a12", "ec2:instance", "EC2 instance", "compute", "web-2", {
    tags: { app: "web", env: "prod", Name: "web-2" },
    cost: 140.2,
    cost_basis: "resource",
    attrs: { instance_type: "m7i.xlarge", state: "running" },
  }),
  worker: node("instance/i-0a13", "ec2:instance", "EC2 instance", "compute", "report-worker", {
    tags: { Name: "report-worker" },
    managed_by: null,
    cost: 61.3,
    cost_basis: "resource",
    attrs: { instance_type: "m4.xlarge", state: "running" },
  }),
  fn: node("function/send-reminders", "lambda:function", "Lambda function", "compute", "send-reminders", {
    cost: 18.4,
    cost_basis: "resource",
  }),
  queue: node("queue/reminders", "sqs:queue", "SQS queue", "messaging", "reminders", { cost: 2.1, cost_basis: "resource" }),
  db: node("db/booking-prod", "rds:db", "RDS instance", "data", "booking-prod", {
    cost: 512.6,
    cost_basis: "resource",
    attrs: { engine: "postgres", instance_class: "db.r6g.large", multi_az: true },
  }),
  cache: node("cluster/booking-sessions", "elasticache:cluster", "ElastiCache cluster", "data", "booking-sessions", {
    cost: 123.8,
    cost_basis: "resource",
    attrs: { node_type: "cache.r4.large" },
  }),
  table: node("table/availability", "dynamodb:table", "DynamoDB table", "data", "availability", {
    cost: 87.2,
    cost_basis: "resource",
    attrs: { billing_mode: "PROVISIONED", read_capacity: 400, write_capacity: 120 },
  }),
  bucket: node("bucket/brightlane-uploads", "s3:bucket", "S3 bucket", "data", "brightlane-uploads", {
    vpc_id: null,
    cost: 64.5,
    cost_basis: "resource",
  }),
  nat: node("natgateway/nat-0d1", "ec2:natgateway", "NAT gateway", "network", "nat-prod-a", {
    cost: 184.3,
    cost_basis: "resource",
    tags: { env: "prod" },
  }),
  vol: node("volume/vol-0f9", "ec2:volume", "EBS volume", "storage", "old-reports-data", {
    tags: {},
    managed_by: null,
    cost: 40,
    cost_basis: "list_price",
    attrs: { state: "available", size_gib: 500, volume_type: "gp2" },
  }),
  stagingAlb: node("loadbalancer/app/booking-staging", "elasticloadbalancing:loadbalancer", "Load balancer", "balancing", "booking-staging-alb", {
    account_id: STAGING,
    tags: { app: "booking-api", env: "staging" },
    cost: 16.4,
    cost_basis: "list_price",
    managed_by: { tool: "terraform", confidence: "likely", evidence: "tag ManagedBy=terraform", stack: null },
  }),
};

const E = (a: GraphNode, b: GraphNode, kind: RelationKind): GraphEdge => ({ source: a.id, target: b.id, kind });

const EDGES: GraphEdge[] = [
  E(N.cdn, N.alb, "routes_to"),
  E(N.cdn, N.bucket, "routes_to"),
  E(N.alb, N.tg, "routes_to"),
  E(N.tg, N.ecs, "routes_to"),
  E(N.tg, N.web1, "routes_to"),
  E(N.tg, N.web2, "routes_to"),
  E(N.ecs, N.db, "routes_to"),
  E(N.ecs, N.cache, "routes_to"),
  E(N.ecs, N.table, "routes_to"),
  E(N.ecs, N.queue, "publishes_to"),
  E(N.queue, N.fn, "triggers"),
  E(N.worker, N.db, "routes_to"),
  E(N.worker, N.bucket, "routes_to"),
];

export function graph(f: GraphFilter): Graph {
  const any = (list: string[], value: string) => !list.length || list.includes(value);
  const nodes = Object.values(N).filter((n) => {
    const tag = f.tag ? (n.tags[f.tag.key] ?? "") : null;
    return (
      any(f.accounts, n.account_id) &&
      any(f.types, n.resource_type) &&
      any(f.regions, n.region) &&
      (!f.vpcs.length || (n.vpc_id !== null && f.vpcs.includes(n.vpc_id))) &&
      any(f.managed_by, n.managed_by?.tool ?? "none") &&
      (!f.tag || (f.tag.missing ? !tag : f.tag.value === null ? !!tag : tag === f.tag.value))
    );
  });
  const ids = new Set(nodes.map((n) => n.id));
  return {
    nodes,
    edges: EDGES.filter((e) => ids.has(e.source) && ids.has(e.target)),
    total_resources: 214,
    cost_basis: "resource",
    unit: "USD",
  };
}

const count = <T,>(items: T[], key: (t: T) => string, label: (k: string) => string = (k) => k) => {
  const m = new Map<string, number>();
  for (const i of items) m.set(key(i), (m.get(key(i)) ?? 0) + 1);
  return [...m].map(([k, c]) => ({ key: k, label: label(k), count: c })).sort((a, b) => b.count - a.count);
};

export function facets(): InventoryFacets {
  const nodes = Object.values(N);
  const name = (id: string) => ACCOUNTS_META.find((a) => a.id === id)?.name ?? id;
  return {
    accounts: count(nodes, (n) => n.account_id, name),
    regions: count(nodes, (n) => n.region),
    vpcs: [{ key: "vpc-0b1a2c3d", label: "booking-prod (vpc-0b1a2c3d)", count: 13 }],
    types: count(nodes, (n) => n.resource_type, (k) => nodes.find((n) => n.resource_type === k)!.type_label),
    tag_keys: [
      { key: "app", label: "app", count: 12 },
      { key: "env", label: "env", count: 14 },
    ],
    managed_by: [
      { key: "cdk", label: "CDK", count: 12 },
      { key: "terraform", label: "Terraform", count: 1 },
      { key: "none", label: "Not detected", count: 3 },
    ],
    total: nodes.length,
  };
}

// ---- findings ----------------------------------------------------------------

const finding = (f: Partial<Finding> & Pick<Finding, "rule" | "title" | "summary" | "how_to_fix">): Finding => ({
  id: `${f.rule}:${f.title}`,
  kind: "waste",
  severity: "medium",
  account_id: PROD,
  region: "us-east-1",
  resources: [],
  monthly_savings: null,
  savings_basis: "public_price",
  impact: null,
  evidence: {},
  first_seen: daysAgo(12).slice(0, 10),
  status: { status: "open" },
  ...f,
});

const res = (n: GraphNode) => ({ arn: n.id, label: n.label, resource_type: n.resource_type });

export const FINDINGS: Finding[] = [
  finding({
    rule: "nat-idle",
    severity: "high",
    account_id: STAGING,
    title: "NAT gateway nat-staging-b has no running instances behind it",
    summary: "Every instance in its VPC is stopped, but the NAT gateway is billed every hour.",
    how_to_fix: "Delete the NAT gateway while staging is off, or schedule it with the instances.",
    resources: [{ arn: "arn:aws:ec2:us-east-1:444455556666:natgateway/nat-0s2", label: "nat-staging-b", resource_type: "ec2:natgateway" }],
    monthly_savings: 32.85,
    evidence: { hourly_price: 0.045, running_instances: 0 },
  }),
  finding({
    rule: "dynamodb-overprovisioned",
    severity: "high",
    title: "DynamoDB table availability uses a fraction of its provisioned capacity",
    summary:
      "Its busiest minute in 14 days averaged 61.0 reads and 18.0 writes per second, against 400 read and 120 write units provisioned.",
    how_to_fix: "Lower the capacity to about 122 read and 36 write units, turn on auto scaling, or switch to on-demand.",
    resources: [res(N.table)],
    monthly_savings: 66.39,
    evidence: { read_capacity: 400, write_capacity: 120, peak_read_per_s: 61, peak_write_per_s: 18 },
  }),
  finding({
    rule: "ebs-unattached",
    severity: "medium",
    title: "Unattached EBS volume old-reports-data",
    summary: "500 GiB gp2 volume not attached to any instance. You pay for provisioned storage whether it's used or not.",
    how_to_fix: "Snapshot it if you might need the data, then delete the volume.",
    resources: [res(N.vol)],
    monthly_savings: 50,
    evidence: { size_gib: 500, volume_type: "gp2", age_days: 143 },
  }),
  finding({
    rule: "elb-idle",
    severity: "medium",
    account_id: STAGING,
    title: "Load balancer booking-staging-alb has no healthy targets",
    summary: "No target behind it is registered, but the load balancer is billed every hour.",
    how_to_fix: "Delete it if staging no longer uses it, or register the targets it should serve.",
    resources: [res(N.stagingAlb)],
    monthly_savings: 16.43,
  }),
  finding({
    rule: "logs-no-retention",
    severity: "low",
    title: "Log group /ecs/booking-api never expires",
    summary: "Without a retention period, every log event is stored forever and the bill grows each month.",
    how_to_fix: "Set a retention period, e.g. 30 days, or what your audit rules require.",
    resources: [{ arn: "arn:aws:logs:us-east-1:111122223333:log-group:/ecs/booking-api", label: "/ecs/booking-api", resource_type: "logs:log-group" }],
    monthly_savings: 7.41,
    evidence: { stored_gib: 247 },
  }),
  finding({
    rule: "old-generation-type",
    severity: "low",
    title: "report-worker runs on previous-generation m4.xlarge",
    summary: "m4.xlarge is a previous generation. The current m7i family costs the same or less per hour at the same size.",
    how_to_fix: "Stop the instance, change its type to m7i.xlarge and start it again, in a maintenance window.",
    resources: [res(N.worker)],
    savings_basis: null,
    evidence: { instance_type: "m4.xlarge", current_generation: "m7i" },
  }),
  finding({
    rule: "old-generation-type",
    severity: "low",
    title: "booking-sessions runs on previous-generation cache.r4.large",
    summary: "cache.r4.large is a previous generation. The current cache.r7g family costs the same or less per hour.",
    how_to_fix: "Change the node type to cache.r7g.large in the ElastiCache console (Modify); compare memory first.",
    resources: [res(N.cache)],
    savings_basis: null,
    evidence: { node_type: "cache.r4.large", current_generation: "cache.r7g" },
  }),
  finding({
    rule: "anomaly",
    kind: "anomaly",
    severity: "high",
    title: "EC2 - Other spend jumped 2.4× for three days",
    summary: "Daily spend on EC2 - Other (NAT data processing) rose from about $8 to $19 in Brightlane Production.",
    how_to_fix: "Check the NAT gateway's traffic in those days: a gateway VPC endpoint for S3 keeps that traffic off NAT.",
    savings_basis: null,
    impact: 33.7,
    first_seen: daysAgo(8).slice(0, 10),
  }),
];

export function insights(): Insights {
  const q: CostQuery = {
    workspace_id: WORKSPACE.id,
    start: iso(new Date(today.getTime() - 30 * DAY)),
    end: iso(today),
    granularity: "daily",
    metric: "unblended",
    group_by: { kind: "service" },
    filter: { linked_accounts: [], services: [] },
  };
  const movers = compareCosts(q, iso(new Date(today.getTime() - 60 * DAY)), iso(new Date(today.getTime() - 30 * DAY)))
    .sort((a, b) => Math.abs(b.delta) - Math.abs(a.delta))
    .slice(0, 8);
  return {
    top_movers: movers,
    data_transfer: [
      { key: "USE1-DataTransfer-Out-Bytes", label: "Data transfer out to the internet (us-east-1)", total: 96.4 },
      { key: "USE1-NatGateway-Bytes", label: "NAT gateway data processed (us-east-1)", total: 71.2 },
      { key: "USE1-EUW1-AWS-Out-Bytes", label: "Between regions: us-east-1 → eu-west-1", total: 18.9 },
    ],
    data_transfer_total: 186.5,
    commitments: [
      [
        PROD,
        {
          kind: "savings_plans",
          coverage_pct: 46,
          utilization_pct: 99,
          on_demand_cost: 812,
          unused_commitment: 1.2,
          start: iso(new Date(today.getTime() - 30 * DAY)),
          end: iso(today),
        },
      ],
    ],
    iac: {
      total: 214,
      by_tool: [
        { tool: "cdk", label: "CDK", resources: 162, certain: 158 },
        { tool: "terraform", label: "Terraform", resources: 21, certain: 0 },
      ],
      unmanaged: 31,
      unmanaged_by_type: [
        { resource_type: "ec2:volume", label: "EBS volume", count: 14 },
        { resource_type: "ec2:instance", label: "EC2 instance", count: 6 },
        { resource_type: "s3:bucket", label: "S3 bucket", count: 5 },
      ],
    },
    tags: {
      total: 188,
      tagged: 161,
      by_key: [
        ["app", 149],
        ["env", 170],
      ],
      untagged_by_type: [
        { resource_type: "ec2:volume", label: "EBS volume", count: 15 },
        { resource_type: "ec2:instance", label: "EC2 instance", count: 7 },
        { resource_type: "sqs:queue", label: "SQS queue", count: 5 },
      ],
    },
    unit: "USD",
  };
}

// ---- assistant -----------------------------------------------------------------

export const CONVERSATION = {
  id: "conv-demo",
  title: `Why did costs go up in ${new Date(Date.UTC(today.getUTCFullYear(), today.getUTCMonth() - 1, 1)).toLocaleString("en-US", { month: "long", timeZone: "UTC" })}?`,
  created_at: daysAgo(1),
  updated_at: daysAgo(1),
  turns: 1,
};

/** Last full month against the one before, from the same sample data the
 * dashboard shows, so the answer's figures match it. */
function lastMonthAnswer(): { answer: string; month: string } {
  const first = new Date(Date.UTC(today.getUTCFullYear(), today.getUTCMonth(), 1));
  const last = new Date(Date.UTC(today.getUTCFullYear(), today.getUTCMonth() - 1, 1));
  const before = new Date(Date.UTC(today.getUTCFullYear(), today.getUTCMonth() - 2, 1));
  const month = last.toLocaleString("en-US", { month: "long", timeZone: "UTC" });
  const prev = before.toLocaleString("en-US", { month: "long", timeZone: "UTC" });
  const rows = compareCosts(
    {
      workspace_id: WORKSPACE.id,
      start: iso(last),
      end: iso(first),
      granularity: "monthly",
      metric: "unblended",
      group_by: { kind: "service" },
      filter: { linked_accounts: [], services: [] },
    },
    iso(before),
    iso(last),
  );
  const usd = (v: number) => v.toLocaleString("en-US", { style: "currency", currency: "USD" });
  const signed = (v: number) => (v >= 0 ? `+${usd(v)}` : `−${usd(-v)}`);
  const now = rows.reduce((s, r) => s + r.current, 0);
  const then = rows.reduce((s, r) => s + r.previous, 0);
  const pct = ((now - then) / then) * 100;
  const top = [...rows].sort((x, y) => y.delta - x.delta).slice(0, 3);
  const table = top.map((r) => `| ${r.label} | ${usd(r.current)} | ${usd(r.previous)} | ${signed(r.delta)} |`).join("\n");
  const answer = `${month} cost **${usd(now)}**, **${pct >= 0 ? "up" : "down"} ${Math.abs(pct).toFixed(1)}%** on ${prev} (${usd(then)}). Three services account for most of the change:

| Service | ${month} | ${prev} | Change |
|---|---:|---:|---:|
${table}

The EC2 - Other jump is **NAT gateway data processing** in Brightlane Production, three days in a row last week [2]. That traffic goes to S3 [3]: a free **gateway VPC endpoint for S3** would take it off the NAT gateway.

The compute growth follows the booking-api launch [1]. That is expected, but with **46% Savings Plans coverage** most of it runs at on-demand rates.`;
  return { answer, month };
}

const LAST_MONTH = lastMonthAnswer();
const QUESTION = `Why did our costs go up in ${LAST_MONTH.month}?`;

export const TURN: Turn = {
  conversation_id: CONVERSATION.id,
  answer: LAST_MONTH.answer,
  citations: [
    { n: 1, tool: "compare_costs", input: { group_by: "service" }, result: {}, ok: true, label: `Costs by service, ${LAST_MONTH.month} vs the month before`, ms: 41, sent_chars: 1830, duplicate_of: null },
    { n: 2, tool: "query_costs", input: { group_by: "usage_type", service: "EC2 - Other" }, result: {}, ok: true, label: "EC2 - Other by usage type, daily, last 14 days", ms: 37, sent_chars: 2410, duplicate_of: null },
    { n: 3, tool: "list_findings", input: { rule: "nat-data-processing" }, result: {}, ok: true, label: "Findings about NAT data processing", ms: 12, sent_chars: 960, duplicate_of: null },
  ],
  stop: "end_turn",
  model: "claude-opus-5-5",
  redacted: true,
  masked_values: 9,
  input_tokens: 3120,
  output_tokens: 412,
  cache_read_tokens: 10240,
  cache_write_tokens: 2310,
  rounds: [],
  flags: [],
  cost_usd: 0.0312,
  elapsed_ms: 9400,
  assumptions: [`"${LAST_MONTH.month}" is the whole calendar month, compared with the whole month before.`],
  context_revision: 3,
  unverified_figures: [],
  turn_id: 1,
  data_as_of: daysAgo(0.09),
  missing_data: [],
  reference_examples: 1,
};

export const STORED_TURN: StoredTurn = {
  id: 1,
  question: QUESTION,
  screen: { view: "Costs", details: [] },
  turn: TURN,
  rating: "up",
  rating_note: null,
  reference: true,
  expected: [],
  created_at: daysAgo(1),
};
