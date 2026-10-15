// SPDX-License-Identifier: MIT OR Apache-2.0

import { useEffect, useMemo, useRef, useState } from "react";
import * as echarts from "echarts/core";
import { GraphChart } from "echarts/charts";
import { TooltipComponent } from "echarts/components";
import { LabelLayout } from "echarts/features";
import { CanvasRenderer } from "echarts/renderers";
import { networkOption, type NetworkColors } from "../networkChart";
import { fitScale, networkLayout } from "../networkLayout";
import { useDarkMode } from "../theme";
import { Callout } from "./Callout";
import type { Graph } from "../types";

echarts.use([GraphChart, TooltipComponent, LabelLayout, CanvasRenderer]);

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

function cssVar(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

/**
 * The resource graph as a network: connected resources cluster together.
 * Same data, selection and cost overlay as the flow view; drawn with ECharts.
 */
export function NetworkGraph({ graph, short, selected, onSelect, matches, steps, centerOn }: Props) {
  const el = useRef<HTMLDivElement>(null);
  const chart = useRef<echarts.ECharts | null>(null);
  const dark = useDarkMode();
  // Resources with no connections say little in a network and crowd the
  // clusters out of view: hidden unless asked for, searched or selected.
  const [showLoners, setShowLoners] = useState(false);
  const loners = useMemo(() => {
    const linked = new Set(graph.edges.flatMap((e) => [e.source, e.target]));
    return new Set(graph.nodes.filter((n) => !linked.has(n.id)).map((n) => n.id));
  }, [graph]);
  const wanted = (id: string | null | undefined) => !!id && loners.has(id);
  const includeLoners =
    showLoners || loners.size === graph.nodes.length || wanted(selected) || wanted(centerOn?.arn) || matches.some(wanted);
  const shown = useMemo(
    () => (includeLoners ? graph : { ...graph, nodes: graph.nodes.filter((n) => !loners.has(n.id)) }),
    [graph, loners, includeLoners],
  );
  const positions = useMemo(() => networkLayout(shown.nodes, shown.edges), [shown]);
  const index = useMemo(() => new Map(shown.nodes.map((n, i) => [n.id, i])), [shown]);
  // Canvas size, to know whether unconnected resources' labels still fit.
  const [size, setSize] = useState({ width: 0, height: 0 });
  // User zoom on top of the fit (ECharts series zoom, 1 = fitted).
  const [zoom, setZoom] = useState(1);
  // Shrunk to fit, the grid of unconnected resources' labels overlaps: draw
  // those as dots (named in the tooltip) until there is room again.
  const unlabeled = useMemo(
    () =>
      includeLoners && fitScale(positions, size.width, size.height) * zoom < 0.85
        ? new Set([...loners].filter((id) => positions.has(id)))
        : undefined,
    [includeLoners, positions, size, zoom, loners],
  );

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
    // Zooming in far enough brings hidden labels back.
    c.on("graphRoam", () => {
      const series = (c.getOption() as { series?: { zoom?: number }[] }).series;
      setZoom(series?.[0]?.zoom ?? 1);
    });
    // Clicking empty space clears the selection, as in the flow view.
    c.getZr().on("click", (e) => {
      if (!e.target) onSelectRef.current(null);
    });
    const resize = new ResizeObserver(([entry]) => {
      c.resize();
      setSize({ width: entry.contentRect.width, height: entry.contentRect.height });
    });
    resize.observe(el.current);
    return () => {
      resize.disconnect();
      c.dispose();
      chart.current = null;
    };
  }, []);

  // Draw. Positions are fixed (layout "none"), so restyling never moves nodes.
  useEffect(() => {
    const colors: NetworkColors = {
      text: cssVar("--text"),
      muted: cssVar("--muted"),
      edge: cssVar("--edge"),
      accent: cssVar("--accent"),
      warn: cssVar("--warn"),
      surface: cssVar("--surface"),
      cost: [1, 2, 3, 4, 5].map((i) => cssVar(`--cost-${i}`)),
    };
    // Merge (not replace) so zoom and pan survive selection changes.
    chart.current?.setOption(networkOption(shown, positions, { selected, matches, steps, short, unlabeled }, colors));
  }, [shown, positions, selected, matches, steps, short, unlabeled, dark]);

  // Fit the whole graph when it changes.
  useEffect(() => {
    chart.current?.setOption({ series: [{ center: null, zoom: 1 }] });
    // Track the reset zoom after this frame (state isn't set during the effect).
    const frame = requestAnimationFrame(() => setZoom(1));
    return () => cancelAnimationFrame(frame);
  }, [shown]);

  // Center on a resource (search, details links, assistant navigation).
  useEffect(() => {
    if (!centerOn || !index.has(centerOn.arn)) return;
    const p = positions.get(centerOn.arn);
    if (!p) return;
    chart.current?.setOption({ series: [{ center: [p.x, p.y], zoom: 2 }] });
    const frame = requestAnimationFrame(() => setZoom(2));
    return () => cancelAnimationFrame(frame);
  }, [centerOn, positions, index]);

  const hidden = includeLoners ? 0 : loners.size;
  return (
    <>
      <div ref={el} className="network-canvas" role="img" aria-label="Resource network" />
      {hidden > 0 ? (
        <div className="network-loners">
          <Callout
            tone="action"
            title={`${hidden} ${hidden === 1 ? "resource has" : "resources have"} no connections`}
            actions={
              <button className="primary small" onClick={() => setShowLoners(true)}>
                Show {hidden === 1 ? "it" : "them"}
              </button>
            }
          >
            Hidden so the clusters stay readable. Search finds them anyway.
          </Callout>
        </div>
      ) : (
        showLoners &&
        loners.size < graph.nodes.length && (
          <div className="network-loners">
            <button className="ghost small" onClick={() => setShowLoners(false)}>
              Hide resources with no connections
            </button>
          </div>
        )
      )}
    </>
  );
}
