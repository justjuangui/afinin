// SPDX-License-Identifier: MIT OR Apache-2.0

import { useEffect, useMemo, useRef } from "react";
import * as echarts from "echarts/core";
import { GraphChart } from "echarts/charts";
import { TooltipComponent } from "echarts/components";
import { CanvasRenderer } from "echarts/renderers";
import { money } from "../format";
import { networkLayout } from "../networkLayout";
import { useDarkMode } from "../theme";
import type { Graph, GraphNode } from "../types";

echarts.use([GraphChart, TooltipComponent, CanvasRenderer]);

interface Props {
  graph: Graph;
  /** Short type codes (λ, EC2, LS…) shown before names. */
  short: Record<string, string>;
  selected: string | null;
  onSelect: (id: string | null) => void;
  /** Search matches, outlined. */
  matches: string[];
  /** Cost ramp step per node (0-based), when the overlay is on. */
  steps: Map<string, number>;
  /** Center on this resource; a new object re-centers. */
  centerOn: { arn: string } | null;
}

/** Label every node only while it stays readable. */
const LABEL_ALL_BELOW = 80;

function cssVar(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

function costLabel(n: GraphNode, unit: string): string {
  if (n.cost === null) return "";
  if (n.cost_basis === "list_price") return n.cost === 0 ? "included" : `${money(n.cost, "USD")}/mo list price`;
  return `${money(n.cost, unit)} last 30 days${n.cost_basis === "service" ? " (service total)" : ""}`;
}

/**
 * The resource graph as a network: connected resources cluster together.
 * Same data, selection and cost overlay as the flow view; drawn with ECharts.
 */
export function NetworkGraph({ graph, short, selected, onSelect, matches, steps, centerOn }: Props) {
  const el = useRef<HTMLDivElement>(null);
  const chart = useRef<echarts.ECharts | null>(null);
  const dark = useDarkMode();
  const positions = useMemo(() => networkLayout(graph.nodes, graph.edges), [graph]);
  const index = useMemo(() => new Map(graph.nodes.map((n, i) => [n.id, i])), [graph]);

  // Chart instance, events and resizing.
  const onSelectRef = useRef(onSelect);
  useEffect(() => {
    onSelectRef.current = onSelect;
  }, [onSelect]);
  useEffect(() => {
    if (!el.current) return;
    const c = echarts.init(el.current, undefined, { renderer: "canvas" });
    chart.current = c;
    c.on("click", (p) => {
      if (p.dataType === "node") onSelectRef.current((p.data as { id: string }).id);
    });
    // Clicking empty space clears the selection, as in the flow view.
    c.getZr().on("click", (e) => {
      if (!e.target) onSelectRef.current(null);
    });
    const resize = new ResizeObserver(() => c.resize());
    resize.observe(el.current);
    return () => {
      resize.disconnect();
      c.dispose();
      chart.current = null;
    };
  }, []);

  // Draw. Positions are fixed (layout "none"), so restyling never moves nodes.
  useEffect(() => {
    const c = chart.current;
    if (!c) return;
    const text = cssVar("--text");
    const muted = cssVar("--muted");
    const edge = cssVar("--edge");
    const accent = cssVar("--accent");
    const warn = cssVar("--warn");
    const surface = cssVar("--surface");
    const costs = graph.nodes.map((n) => n.cost ?? 0);
    const max = Math.max(1, ...costs);
    const matched = new Set(matches);
    const neighbors = new Set<string>();
    if (selected) {
      for (const e of graph.edges) {
        if (e.source === selected) neighbors.add(e.target);
        if (e.target === selected) neighbors.add(e.source);
      }
    }
    const labelAll = graph.nodes.length < LABEL_ALL_BELOW;
    c.setOption(
      {
        animation: false,
        tooltip: {
          confine: true,
          formatter: (p: { dataType?: string; data: { node?: GraphNode } }) => {
            const n = p.data.node;
            if (p.dataType !== "node" || !n) return "";
            const esc = (s: string) => s.replace(/[&<>"]/g, (ch) => `&#${ch.charCodeAt(0)};`);
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
            roam: true,
            scaleLimit: { min: 0.05, max: 6 },
            edgeSymbol: ["none", "arrow"],
            edgeSymbolSize: 5,
            label: { position: "right", fontSize: 11, color: text },
            lineStyle: { color: edge, opacity: 0.7, width: 1, curveness: 0.12 },
            emphasis: { disabled: true },
            data: graph.nodes.map((n) => {
              const p = positions.get(n.id) ?? { x: 0, y: 0 };
              const step = steps.get(n.id);
              const isSel = n.id === selected;
              const faded = selected !== null && !isSel && !neighbors.has(n.id);
              return {
                id: n.id,
                name: n.label,
                node: n,
                x: p.x,
                y: p.y,
                // Area grows with cost: radius ~ sqrt(cost).
                symbolSize: n.cost ? 12 + 26 * Math.sqrt(n.cost / max) : 12,
                itemStyle: {
                  color: step !== undefined ? cssVar(`--cost-${step + 1}`) : n.context ? surface : muted,
                  borderColor: isSel ? accent : matched.has(n.id) ? warn : n.context ? muted : surface,
                  borderWidth: isSel || matched.has(n.id) ? 3 : 1,
                  borderType: n.context ? "dashed" : "solid",
                  opacity: faded ? 0.2 : n.context ? 0.6 : 1,
                },
                label: {
                  show: labelAll || isSel || neighbors.has(n.id) || matched.has(n.id),
                  formatter: `${short[n.resource_type] ?? "•"} ${n.label}`,
                  opacity: faded ? 0.3 : 1,
                },
              };
            }),
            links: graph.edges.map((e) => {
              const touches = selected !== null && (e.source === selected || e.target === selected);
              return {
                source: e.source,
                target: e.target,
                lineStyle: touches
                  ? { color: accent, width: 2, opacity: 1 }
                  : { opacity: selected !== null ? 0.15 : 0.7 },
              };
            }),
          },
        ],
      },
      // Merge (not replace) so zoom and pan survive selection changes.
    );
  }, [graph, positions, selected, matches, steps, short, dark]);

  // Fit the whole graph when it changes.
  useEffect(() => {
    chart.current?.setOption({ series: [{ center: null, zoom: 1 }] });
  }, [graph]);

  // Center on a resource (search, details links, assistant navigation).
  useEffect(() => {
    if (!centerOn || !index.has(centerOn.arn)) return;
    const p = positions.get(centerOn.arn);
    if (p) chart.current?.setOption({ series: [{ center: [p.x, p.y], zoom: 2 }] });
  }, [centerOn, positions, index]);

  return <div ref={el} className="network-canvas" role="img" aria-label="Resource network" />;
}
