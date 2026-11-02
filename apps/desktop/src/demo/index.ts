// SPDX-License-Identifier: MIT OR Apache-2.0
// Demo mode (`npm run demo`): the real UI in a browser, with the Tauri backend
// replaced by sample data. Used for screenshots and trying the app without AWS;
// never part of a release build.

import { mockIPC } from "@tauri-apps/api/mocks";
import type { CostQuery, FindingStatus, GraphFilter, GroupBy } from "../types";
import * as data from "./data";

const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));

const statuses = new Map<string, FindingStatus>();

type Args = Record<string, unknown>;

const handlers: Record<string, (a: Args) => unknown> = {
  startup_error: () => null,
  check_update: () => null,
  list_profiles: () => [],
  list_workspaces: () => [data.WORKSPACE],
  list_accounts: () => data.ACCOUNTS,
  get_settings: () => data.SETTINGS,
  save_settings: () => null,
  api_usage: () => [
    { service: "ce", operation: "GetCostAndUsage", calls: 182, cost_usd: 1.82 },
    { service: "cloudwatch", operation: "GetMetricData", calls: 31, cost_usd: 0.0006 },
  ],
  llm_usage: () => [
    {
      provider: "anthropic",
      model: "claude-opus-5-5",
      requests: 46,
      input_tokens: 61_200,
      output_tokens: 9_840,
      cache_read_tokens: 212_000,
      cache_write_tokens: 41_000,
      cost_usd: 0.72,
    },
  ],
  known_llm_price: () => [{ input: 4, output: 20, cache_read: 0.2, cache_write: null }, "October 2026"],
  has_llm_api_key: () => true,
  cur_status: () => ({ configs: [], billing_periods: [], first_date: null, last_date: null, last_ingested_at: null }),
  get_cur_config: () => null,

  cost_overview: (a) => data.costOverview((a.linkedAccounts as string[]) ?? []),
  cost_series: (a) => data.costSeries(a.query as CostQuery),
  compare_costs: (a) => data.compareCosts(a.query as CostQuery, a.prevStart as string, a.prevEnd as string),
  dimension_values: (a) => {
    const g = a.groupBy as GroupBy;
    return data.dimensionValues(g.kind, g.kind === "tag" ? g.key : undefined);
  },

  inventory_graph: (a) => data.graph(a.filter as GraphFilter),
  inventory_facets: () => data.facets(),
  tag_values: (a) =>
    a.key === "env"
      ? [
          { key: "prod", label: "prod", count: 13 },
          { key: "staging", label: "staging", count: 1 },
        ]
      : [
          { key: "booking-api", label: "booking-api", count: 10 },
          { key: "web", label: "web", count: 3 },
        ],
  applications: () => [
    { value: "booking-api", resource_count: 96, resource_types: { "ecs:service": 4, "rds:db": 2 }, cost: 1684.2 },
    { value: "web", resource_count: 41, resource_types: { "ec2:instance": 6 }, cost: 612.8 },
    { value: "data-pipeline", resource_count: 33, resource_types: { "lambda:function": 12 }, cost: 402.5 },
    { value: "", resource_count: 44, resource_types: { "ec2:volume": 15 }, cost: null },
  ],
  inventory_scans: () =>
    data.ACCOUNTS_META.map((a) => ({
      account_id: a.id,
      scan_region: "us-east-1",
      scanned_at: new Date(Date.now() - 2 * 3_600_000).toISOString(),
      resource_count: 71,
      warnings: [],
    })),

  // Dismissed and snoozed findings stay hidden until the page reloads.
  findings: (a) =>
    data.FINDINGS.map((f) => ({ ...f, status: statuses.get(f.id) ?? f.status })).filter(
      (f) => a.includeHidden || f.status.status === "open",
    ),
  set_finding_status: (a) => {
    statuses.set(a.findingId as string, a.status as FindingStatus);
    return null;
  },
  insights: () => data.insights(),

  workspace_context: () => ({
    context: {
      business: "Brightlane sells online booking for clinics and salons. booking-api is the product; web is the marketing site.",
      accounts: {
        "111122223333": "Production: everything customers use",
        "444455556666": "Staging, switched off at night",
        "777788889999": "Analytics and the data pipeline",
      },
      tags: [{ key: "app", meaning: "The product a resource belongs to" }],
      glossary: [{ key: "launch", meaning: "The booking-api v2 launch on the 20th" }],
      instructions: "Answer in short paragraphs; show money in USD.",
    },
    revision: 3,
    updated_at: new Date(Date.now() - 5 * 86_400_000).toISOString(),
  }),
  workspace_context_history: () => [],
  assistant_conversations: () => [data.CONVERSATION],
  assistant_turns: () => [data.STORED_TURN],
  assistant_gaps: () => [],
  reference_turns: () => [data.STORED_TURN],
  llm_log: () => [],
  rate_assistant_turn: () => null,
  set_reference_turn: () => null,
  set_reference_expected: (a) => a.expected,
  reference_figures: () => [],
  ask: async () => {
    await wait(1200);
    return data.TURN;
  },
};

mockIPC(
  async (cmd, payload) => {
    // Event plumbing (`listen`/`unlisten`) is handled by the mock itself.
    const handler = handlers[cmd];
    // Anything else fails like a backend error would, so screens show their
    // usual error instead of reading a missing result.
    if (!handler) throw `Not available in demo mode (${cmd}).`;
    return handler((payload ?? {}) as Args);
  },
  { shouldMockEvents: true },
);
