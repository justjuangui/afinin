// SPDX-License-Identifier: MIT OR Apache-2.0
// Tiered left-to-right layout: one band per tier (edge → load balancers →
// target groups → compute → attached IPs → storage → data → messaging →
// network). O(n log n),
// so thousands of nodes lay out instantly (NFR-12).

import type { GraphEdge, GraphNode, Tier } from "./types";

export const TIER_ORDER: Tier[] = ["edge", "balancing", "routing", "compute", "storage", "data", "messaging", "network"];

/** Bands left to right: tiers, plus "attached" right after compute. */
type Band = Tier | "attached";
const BAND_ORDER: Band[] = ["edge", "balancing", "routing", "compute", "attached", "storage", "data", "messaging", "network"];

/**
 * Network resources attached only to compute (Elastic IPs, Lightsail static
 * IPs) go in a band right after compute: in the network band, the line to
 * their instance crossed the storage and data bands and ran behind
 * unrelated nodes there.
 */
function band(n: GraphNode, edges: GraphEdge[], tierOf: Map<string, Tier>): Band {
  if (n.tier !== "network") return n.tier;
  const own = edges.filter((e) => e.source === n.id || e.target === n.id);
  const attachedToCompute =
    own.length > 0 &&
    own.every((e) => e.kind === "attached" && tierOf.get(e.source === n.id ? e.target : e.source) === "compute");
  return attachedToCompute ? "attached" : n.tier;
}

export const NODE_W = 200;
export const NODE_H = 52;
const COL_GAP = 28;
const ROW_GAP = 14;
const BAND_GAP = 72;

export interface Position {
  x: number;
  y: number;
}

/** Rows per sub-column: roughly square overall, within sane bounds. */
function rowsPerColumn(total: number): number {
  return Math.max(10, Math.min(40, Math.ceil(Math.sqrt(total) * 1.5)));
}

export function layout(nodes: GraphNode[], edges: GraphEdge[]): Map<string, Position> {
  const neighbors = new Map<string, string[]>();
  for (const e of edges) {
    (neighbors.get(e.source) ?? neighbors.set(e.source, []).get(e.source)!).push(e.target);
    (neighbors.get(e.target) ?? neighbors.set(e.target, []).get(e.target)!).push(e.source);
  }
  const rows = rowsPerColumn(nodes.length);
  const order = new Map<string, number>(); // rank within its band, for barycenters
  const positions = new Map<string, Position>();
  let x = 0;

  const tierOf = new Map(nodes.map((n) => [n.id, n.tier]));
  const bandOf = new Map(nodes.map((n) => [n.id, band(n, edges, tierOf)]));
  for (const b of BAND_ORDER) {
    const members = nodes.filter((n) => bandOf.get(n.id) === b);
    if (members.length === 0) continue;
    // Place each node near what it connects to in earlier bands (barycenter),
    // falling back to grouping by account, region and VPC.
    const bary = (n: GraphNode) => {
      const ranks = (neighbors.get(n.id) ?? []).map((id) => order.get(id)).filter((r): r is number => r !== undefined);
      return ranks.length ? ranks.reduce((a, b) => a + b, 0) / ranks.length : Number.POSITIVE_INFINITY;
    };
    const group = (n: GraphNode) => `${n.account_id}|${n.region}|${n.vpc_id ?? ""}|${n.resource_type}|${n.label}`;
    const sorted = members
      .map((n) => ({ n, b: bary(n), g: group(n) }))
      .sort((a, b) => a.b - b.b || a.g.localeCompare(b.g));
    sorted.forEach(({ n }, i) => {
      const col = Math.floor(i / rows);
      const row = i % rows;
      positions.set(n.id, { x: x + col * (NODE_W + COL_GAP), y: row * (NODE_H + ROW_GAP) });
      order.set(n.id, i);
    });
    const cols = Math.ceil(sorted.length / rows);
    x += cols * (NODE_W + COL_GAP) - COL_GAP + BAND_GAP;
  }
  return positions;
}
