// SPDX-License-Identifier: MIT OR Apache-2.0
// ECharts option for the network view. Pure (colors come in as arguments) so
// it renders the same in the app and in offline checks.

import { money } from "./format";
import type { Point } from "./networkLayout";
import type { Graph, GraphNode } from "./types";

export interface NetworkColors {
  text: string;
  muted: string;
  edge: string;
  accent: string;
  warn: string;
  surface: string;
  /** Cost ramp, low → high. */
  cost: string[];
}

export interface NetworkState {
  selected: string | null;
  matches: string[];
  /** Cost ramp step per node (0-based), when the overlay is on. */
  steps: Map<string, number>;
  /** Short type codes (λ, EC2, LS…) shown before names. */
  short: Record<string, string>;
  /** Nodes drawn without a label (the tooltip still names them). */
  unlabeled?: Set<string>;
}

/**
 * Canvas margins around the drawing, in pixels: the legend (top), the notice
 * (bottom left) and the labels of the right-most nodes, which the fit ignores.
 */
export const NETWORK_PADDING = { top: 56, bottom: 72, left: 24, right: 180 };

/** Names longer than this are shortened; the tooltip has the full name. */
const LABEL_CHARS = 24;

function shorten(s: string): string {
  return s.length > LABEL_CHARS ? `${s.slice(0, LABEL_CHARS - 1)}…` : s;
}

function costLabel(n: GraphNode, unit: string): string {
  if (n.cost === null) return "";
  if (n.cost_basis === "list_price") return n.cost === 0 ? "included" : `${money(n.cost, "USD")}/mo public price`;
  return `${money(n.cost, unit)} last 30 days${n.cost_basis === "service" ? " (its service's total)" : ""}`;
}

export function networkOption(graph: Graph, positions: Map<string, Point>, s: NetworkState, c: NetworkColors) {
  const max = Math.max(1, ...graph.nodes.map((n) => n.cost ?? 0));
  const matched = new Set(s.matches);
  const neighbors = new Set<string>();
  if (s.selected) {
    for (const e of graph.edges) {
      if (e.source === s.selected) neighbors.add(e.target);
      if (e.target === s.selected) neighbors.add(e.source);
    }
  }
  return {
    animation: false,
    tooltip: {
      confine: true,
      backgroundColor: c.surface,
      borderColor: c.edge,
      textStyle: { color: c.text, fontSize: 12 },
      formatter: (p: { dataType?: string; data: { node?: GraphNode } }) => {
        const n = p.data.node;
        if (p.dataType !== "node" || !n) return "";
        const esc = (t: string) => t.replace(/[&<>"]/g, (ch) => `&#${ch.charCodeAt(0)};`);
        const cost = costLabel(n, graph.unit);
        return `<b>${esc(n.label)}</b><br/>${esc(n.type_label)}${n.context ? " · connected, not a match" : ""}${
          cost ? `<br/>${esc(cost)}` : ""
        }`;
      },
    },
    series: [
      {
        type: "graph",
        layout: "none",
        ...NETWORK_PADDING,
        // Without this ECharts stretches the layout to fill the box.
        preserveAspect: "contain",
        roam: true,
        scaleLimit: { min: 0.05, max: 6 },
        edgeSymbol: ["none", "arrow"],
        edgeSymbolSize: 5,
        label: { show: true, position: "right", fontSize: 11, color: c.text, textBorderColor: c.surface, textBorderWidth: 3 },
        // Labels that would still collide are hidden; zoom in or hover to read them.
        labelLayout: { hideOverlap: true },
        lineStyle: { color: c.edge, opacity: 0.7, width: 1, curveness: 0.12 },
        emphasis: { disabled: true },
        data: graph.nodes.map((n) => {
          const p = positions.get(n.id) ?? { x: 0, y: 0 };
          const step = s.steps.get(n.id);
          const isSel = n.id === s.selected;
          const faded = s.selected !== null && !isSel && !neighbors.has(n.id);
          return {
            id: n.id,
            name: n.label,
            node: n,
            x: p.x,
            y: p.y,
            // Area grows with cost: radius ~ sqrt(cost).
            symbolSize: n.cost ? 12 + 26 * Math.sqrt(n.cost / max) : 12,
            itemStyle: {
              color: step !== undefined ? c.cost[step] : n.context ? c.surface : c.muted,
              borderColor: isSel ? c.accent : matched.has(n.id) ? c.warn : n.context ? c.muted : c.surface,
              borderWidth: isSel || matched.has(n.id) ? 3 : 1,
              borderType: n.context ? "dashed" : "solid",
              opacity: faded ? 0.2 : n.context ? 0.6 : 1,
            },
            label: {
              show: !s.unlabeled?.has(n.id) || isSel || matched.has(n.id),
              formatter: `${s.short[n.resource_type] ?? "•"} ${shorten(n.label)}`,
              opacity: faded ? 0.3 : 1,
              fontWeight: isSel ? "bold" : "normal",
            },
          };
        }),
        links: graph.edges.map((e) => {
          const touches = s.selected !== null && (e.source === s.selected || e.target === s.selected);
          return {
            source: e.source,
            target: e.target,
            lineStyle: touches ? { color: c.accent, width: 2, opacity: 1 } : { opacity: s.selected !== null ? 0.15 : 0.7 },
          };
        }),
      },
    ],
  };
}
