// SPDX-License-Identifier: MIT OR Apache-2.0
// Force-directed layout for the network view. Deterministic (same graph, same
// picture) so filtering or selecting never reshuffles nodes. Each connected
// group is laid out on its own (Fruchterman–Reingold), then groups are packed
// largest first; lone resources fill a grid at the end.

import type { GraphEdge, GraphNode } from "./types";

export interface Point {
  x: number;
  y: number;
}

/** Ideal distance between connected nodes, in layout units. */
const K = 70;
const GAP = 60;

/** Connected components, each as node ids sorted for determinism. */
function components(nodes: GraphNode[], edges: GraphEdge[]): string[][] {
  const adj = new Map<string, string[]>(nodes.map((n) => [n.id, []]));
  for (const e of edges) {
    adj.get(e.source)?.push(e.target);
    adj.get(e.target)?.push(e.source);
  }
  const seen = new Set<string>();
  const out: string[][] = [];
  for (const id of [...adj.keys()].sort()) {
    if (seen.has(id)) continue;
    const comp: string[] = [];
    const stack = [id];
    seen.add(id);
    while (stack.length) {
      const cur = stack.pop()!;
      comp.push(cur);
      for (const nb of adj.get(cur) ?? []) {
        if (!seen.has(nb)) {
          seen.add(nb);
          stack.push(nb);
        }
      }
    }
    out.push(comp.sort());
  }
  return out;
}

/** Fruchterman–Reingold for one component; positions centred on (0, 0). */
function forceLayout(ids: string[], edges: GraphEdge[]): Map<string, Point> {
  const n = ids.length;
  const index = new Map(ids.map((id, i) => [id, i]));
  const links = edges
    .map((e) => [index.get(e.source), index.get(e.target)] as const)
    .filter((l): l is readonly [number, number] => l[0] !== undefined && l[1] !== undefined && l[0] !== l[1]);
  // Start on a circle (deterministic), big enough not to start crushed.
  const radius = K * Math.sqrt(n);
  const xs = ids.map((_, i) => radius * Math.cos((2 * Math.PI * i) / n));
  const ys = ids.map((_, i) => radius * Math.sin((2 * Math.PI * i) / n));
  // O(n²) per iteration: fewer iterations for big components.
  const iterations = n < 150 ? 300 : n < 600 ? 120 : 50;
  let temp = radius / 4;
  const cool = temp / (iterations + 1);
  const dx = new Float64Array(n);
  const dy = new Float64Array(n);
  for (let it = 0; it < iterations; it++) {
    dx.fill(0);
    dy.fill(0);
    for (let i = 0; i < n; i++) {
      for (let j = i + 1; j < n; j++) {
        let ddx = xs[i] - xs[j];
        let ddy = ys[i] - ys[j];
        let d2 = ddx * ddx + ddy * ddy;
        if (d2 < 0.01) {
          // Coincident: nudge apart along a fixed direction.
          ddx = 0.1 * (i - j);
          ddy = 0.1;
          d2 = ddx * ddx + ddy * ddy;
        }
        const f = (K * K) / d2; // repulsion / distance
        dx[i] += ddx * f;
        dy[i] += ddy * f;
        dx[j] -= ddx * f;
        dy[j] -= ddy * f;
      }
    }
    for (const [a, b] of links) {
      const ddx = xs[a] - xs[b];
      const ddy = ys[a] - ys[b];
      const d = Math.sqrt(ddx * ddx + ddy * ddy) || 0.01;
      const f = d / K; // attraction / distance
      dx[a] -= ddx * f;
      dy[a] -= ddy * f;
      dx[b] += ddx * f;
      dy[b] += ddy * f;
    }
    for (let i = 0; i < n; i++) {
      const d = Math.sqrt(dx[i] * dx[i] + dy[i] * dy[i]) || 1;
      const step = Math.min(d, temp);
      xs[i] += (dx[i] / d) * step;
      ys[i] += (dy[i] / d) * step;
    }
    temp -= cool;
  }
  const cx = xs.reduce((a, b) => a + b, 0) / n;
  const cy = ys.reduce((a, b) => a + b, 0) / n;
  return new Map(ids.map((id, i) => [id, { x: xs[i] - cx, y: ys[i] - cy }]));
}

/** Room for a node's label, which sits to its right. */
export const LABEL_W = 190;
const ROW_H = 40;
/** Width:height the packing aims for, close to the canvas. */
const ASPECT = 3;

/** Rotates a group so its long side is vertical: labels sit to the right. */
function upright(pos: Map<string, Point>): Map<string, Point> {
  const xs = [...pos.values()].map((p) => p.x);
  const ys = [...pos.values()].map((p) => p.y);
  const w = Math.max(...xs) - Math.min(...xs);
  const h = Math.max(...ys) - Math.min(...ys);
  return w <= h ? pos : new Map([...pos].map(([id, p]) => [id, { x: p.y, y: p.x }]));
}

/**
 * Pixels per layout unit when the whole layout is fitted into a canvas
 * (labels sit to the right of nodes, so they count towards the width).
 */
export function fitScale(positions: Map<string, Point>, width: number, height: number): number {
  if (positions.size === 0 || width <= 0 || height <= 0) return 1;
  const xs = [...positions.values()].map((p) => p.x);
  const ys = [...positions.values()].map((p) => p.y);
  const w = Math.max(...xs) - Math.min(...xs) + LABEL_W;
  const h = Math.max(...ys) - Math.min(...ys) + ROW_H;
  return Math.min(width / w, height / h);
}

export function networkLayout(nodes: GraphNode[], edges: GraphEdge[]): Map<string, Point> {
  const comps = components(nodes, edges);
  const groups = comps.filter((c) => c.length > 1).sort((a, b) => b.length - a.length || a[0].localeCompare(b[0]));
  // Lone resources read best grouped by type, then name.
  const byId = new Map(nodes.map((n) => [n.id, n]));
  const sortKey = (id: string) => `${byId.get(id)?.resource_type ?? ""}|${byId.get(id)?.label ?? id}`;
  const loners = comps
    .filter((c) => c.length === 1)
    .map((c) => c[0])
    .sort((a, b) => sortKey(a).localeCompare(sortKey(b)));
  const out = new Map<string, Point>();

  // Each group's box includes room for labels on the right and a margin.
  const laid = groups.map((ids) => {
    const pos = upright(forceLayout(ids, edges));
    const xs = [...pos.values()].map((p) => p.x);
    const ys = [...pos.values()].map((p) => p.y);
    const minX = Math.min(...xs);
    const minY = Math.min(...ys);
    const w = Math.max(...xs) - minX + LABEL_W;
    const h = Math.max(...ys) - minY + ROW_H;
    return { pos, minX, minY, w, h };
  });
  const lonerCols = (width: number) => Math.max(1, Math.floor(width / LABEL_W));
  // Width that makes the whole drawing about ASPECT wide (area-based).
  const area = laid.reduce((a, l) => a + (l.w + GAP) * (l.h + GAP), 0) + loners.length * LABEL_W * ROW_H;
  const width = Math.max(LABEL_W * 3, ...laid.map((l) => l.w), Math.sqrt(area * ASPECT));

  // Shelf packing, tallest first, so rows waste little space.
  let x = 0;
  let y = 0;
  let rowH = 0;
  for (const l of [...laid].sort((a, b) => b.h - a.h)) {
    if (x > 0 && x + l.w > width) {
      x = 0;
      y += rowH + GAP;
      rowH = 0;
    }
    for (const [id, p] of l.pos) out.set(id, { x: x + p.x - l.minX, y: y + p.y - l.minY });
    x += l.w + GAP;
    rowH = Math.max(rowH, l.h);
  }

  // Lone resources: columns of labels below the groups, grouped by type.
  if (loners.length) {
    y += laid.length ? rowH + GAP : 0;
    const cols = lonerCols(width);
    const rows = Math.ceil(loners.length / cols);
    // Fill column by column so a type's resources stay together.
    loners.forEach((id, i) => out.set(id, { x: Math.floor(i / rows) * LABEL_W, y: y + (i % rows) * ROW_H }));
  }
  return out;
}
