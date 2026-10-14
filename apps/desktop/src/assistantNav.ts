// SPDX-License-Identifier: MIT OR Apache-2.0
// Turns the queries behind an assistant answer into "open this in the app" links.

import { addDays, daysBetween } from "./dates";
import type { Citation, CostMetric, GroupBy, NavTarget, Turn } from "./types";

export interface NavLink {
  label: string;
  target: NavTarget;
}

const METRICS: CostMetric[] = ["unblended", "amortized", "net_unblended"];

function str(v: unknown): string | null {
  return typeof v === "string" && v ? v : null;
}

function strings(v: unknown): string[] {
  return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : [];
}

function groupBy(input: Record<string, unknown>): GroupBy | null {
  switch (input.group_by) {
    case "service":
    case "account":
    case "region":
    case "usage_type":
    case "resource":
      return { kind: input.group_by };
    case "tag": {
      const key = str(input.tag_key);
      return key ? { kind: "tag", key } : null;
    }
    default:
      return null;
  }
}

function groupLabel(g: GroupBy): string {
  return g.kind === "tag" ? `tag ${g.key}` : g.kind.replace("_", " ");
}

/** `end` is exclusive; labels show the last included day. */
function rangeLabel(start: string, end: string): string {
  return `${start} to ${addDays(end, -1)}`;
}

function costLink(c: Citation, compare: boolean): NavLink | null {
  const input = c.input;
  const start = str(compare ? input.current_start : input.start);
  const end = str(compare ? input.current_end : input.end);
  const g = groupBy(input);
  if (!start || !end || !g || daysBetween(start, end) <= 0) return null;
  const metric = [input.metric, (c.result as Record<string, unknown> | null)?.metric].find(
    (m): m is CostMetric => METRICS.includes(m as CostMetric),
  );
  return {
    label: `${compare ? "Open comparison" : "Open"} in Costs · by ${groupLabel(g)}, ${rangeLabel(start, end)}`,
    target: {
      view: "costs",
      start,
      end,
      groupBy: g,
      metric: metric ?? null,
      accounts: strings(input.account_ids),
      services: strings(input.services),
      compare,
    },
  };
}

function inventoryLink(c: Citation): NavLink | null {
  const result = (c.result ?? {}) as Record<string, unknown>;
  const arn = str(c.input.arn);
  if (!arn) return null;
  const name = str(result.name) ?? arn.split(/[:/]/).pop() ?? arn;
  return { label: `Show ${name} in Inventory`, target: { view: "inventory", arn } };
}

/** Distinct links for the successful queries of one answer, at most `limit`. */
export function navLinks(turn: Turn, limit = 3): NavLink[] {
  const seen = new Set<string>();
  const out: NavLink[] = [];
  for (const c of turn.citations) {
    if (!c.ok) continue;
    const link =
      c.tool === "query_costs"
        ? costLink(c, false)
        : c.tool === "compare_periods"
          ? costLink(c, true)
          : c.tool === "get_resource" || c.tool === "get_neighbors"
            ? inventoryLink(c)
            : null;
    if (!link) continue;
    const key = JSON.stringify(link.target);
    if (seen.has(key)) continue;
    seen.add(key);
    out.push(link);
    if (out.length >= limit) break;
  }
  return out;
}
