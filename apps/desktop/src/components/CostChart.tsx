// SPDX-License-Identifier: MIT OR Apache-2.0

import { useEffect, useRef, useState } from "react";
import * as echarts from "echarts/core";
import { BarChart } from "echarts/charts";
import { GridComponent, LegendComponent, TooltipComponent } from "echarts/components";
import { CanvasRenderer } from "echarts/renderers";
import { money, periodLabel } from "../format";
import type { Granularity } from "../types";

echarts.use([BarChart, GridComponent, LegendComponent, TooltipComponent, CanvasRenderer]);

export interface ChartSeries {
  key: string;
  label: string;
  /** CSS custom property holding the series color, e.g. `--series-1`. */
  colorVar: string;
  values: number[];
}

interface Props {
  periods: string[];
  series: ChartSeries[];
  granularity: Granularity;
  unit: string;
  estimatedPeriods: string[];
}

function useColorScheme(): string {
  const query = "(prefers-color-scheme: dark)";
  const [dark, setDark] = useState(() => window.matchMedia(query).matches);
  useEffect(() => {
    const mql = window.matchMedia(query);
    const onChange = (e: MediaQueryListEvent) => setDark(e.matches);
    mql.addEventListener("change", onChange);
    return () => mql.removeEventListener("change", onChange);
  }, []);
  return dark ? "dark" : "light";
}

const escapeHtml = (s: string) =>
  s.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);

/** Stacked bars of cost per period. Colors come from CSS tokens so both themes are validated sets. */
export function CostChart({ periods, series, granularity, unit, estimatedPeriods }: Props) {
  const el = useRef<HTMLDivElement>(null);
  const chart = useRef<echarts.ECharts | null>(null);
  const scheme = useColorScheme();

  useEffect(() => {
    if (!el.current) return;
    const instance = echarts.init(el.current);
    chart.current = instance;
    const observer = new ResizeObserver(() => instance.resize());
    observer.observe(el.current);
    return () => {
      observer.disconnect();
      instance.dispose();
      chart.current = null;
    };
  }, []);

  useEffect(() => {
    const instance = chart.current;
    if (!instance || !el.current) return;
    const css = getComputedStyle(el.current);
    const token = (name: string) => css.getPropertyValue(name).trim();
    const surface = token("--surface");
    const estimated = new Set(estimatedPeriods);
    const labels = periods.map((p) => periodLabel(p, granularity) + (estimated.has(p) ? "*" : ""));
    const last = series.length - 1;

    instance.setOption(
      {
        animation: false,
        textStyle: { fontFamily: token("--font"), color: token("--chart-muted") },
        grid: { left: 8, right: 8, top: 40, bottom: 8, containLabel: true },
        legend: {
          type: "scroll",
          top: 0,
          left: 0,
          icon: "roundRect",
          itemWidth: 10,
          itemHeight: 10,
          textStyle: { color: token("--text") },
        },
        tooltip: {
          trigger: "axis",
          axisPointer: { type: "shadow", shadowStyle: { color: token("--chart-hover") } },
          backgroundColor: surface,
          borderColor: token("--border"),
          textStyle: { color: token("--text") },
          formatter: (params: { dataIndex: number; seriesName: string; value: number; color: string }[]) => {
            if (!params.length) return "";
            const i = params[0].dataIndex;
            const total = params.reduce((sum, p) => sum + (p.value || 0), 0);
            const rows = params
              .filter((p) => p.value)
              .sort((a, b) => b.value - a.value)
              .map(
                (p) =>
                  `<div class="tt-row"><span class="tt-swatch" style="background:${p.color}"></span>` +
                  `<span class="tt-label">${escapeHtml(p.seriesName)}</span><span class="tt-value">${money(p.value, unit)}</span></div>`,
              )
              .join("");
            const note = estimated.has(periods[i]) ? `<div class="tt-note">* AWS estimate, may still change</div>` : "";
            return `<div class="tt"><div class="tt-head">${escapeHtml(periodLabel(periods[i], granularity))}` +
              `<span>${money(total, unit)}</span></div>${rows}${note}</div>`;
          },
        },
        xAxis: {
          type: "category",
          data: labels,
          axisTick: { show: false },
          axisLine: { lineStyle: { color: token("--chart-axis") } },
          axisLabel: { color: token("--chart-muted"), hideOverlap: true },
        },
        yAxis: {
          type: "value",
          splitLine: { lineStyle: { color: token("--chart-grid") } },
          axisLabel: { color: token("--chart-muted"), formatter: (v: number) => money(v, unit, true) },
        },
        series: series.map((s, idx) => ({
          name: s.label,
          type: "bar",
          stack: "cost",
          barMaxWidth: 28,
          data: s.values,
          emphasis: { focus: "series" },
          itemStyle: {
            color: token(s.colorVar),
            // Surface-colored gap between stacked segments instead of outlines.
            borderColor: surface,
            borderWidth: 1,
            borderRadius: idx === last ? [4, 4, 0, 0] : 0,
          },
        })),
      },
      { notMerge: true },
    );
  }, [periods, series, granularity, unit, estimatedPeriods, scheme]);

  return <div ref={el} className="cost-chart" role="img" aria-label="Cost per period, stacked by group. See the table below for values." />;
}
