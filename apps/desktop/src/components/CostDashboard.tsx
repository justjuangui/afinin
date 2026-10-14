// SPDX-License-Identifier: MIT OR Apache-2.0

import { useEffect, useMemo, useState } from "react";
import { api, errorMessage } from "../api";
import { PRESETS, presetRange, previousRange, todayUtc, type DateRange } from "../dates";
import { money, percent } from "../format";
import type {
  CostComparisonRow,
  CostMetric,
  CostOverview,
  CostQuery,
  CostSeries,
  DimensionValue,
  Granularity,
  GroupBy,
  Settings,
} from "../types";
import { CostChart, type ChartSeries } from "./CostChart";
import { MultiSelect } from "./MultiSelect";
import { Callout } from "./Callout";

/** Categorical slots of the validated chart palette; the rest folds into "Other". */
const SLOTS = 7;
const OTHER = "__other__";

const METRICS: { id: CostMetric; label: string }[] = [
  { id: "unblended", label: "Unblended" },
  { id: "amortized", label: "Amortized" },
  { id: "net_unblended", label: "Net unblended" },
];

const groupId = (g: GroupBy) => (g.kind === "tag" ? `tag:${g.key}` : g.kind);

function groupOptions(tagKeys: string[], cur = false): { id: string; label: string; value: GroupBy; disabled?: boolean }[] {
  return [
    { id: "service", label: "Service", value: { kind: "service" } },
    { id: "account", label: "Account", value: { kind: "account" } },
    { id: "region", label: "Region", value: { kind: "region" } },
    { id: "usage_type", label: "Usage type", value: { kind: "usage_type" } },
    ...tagKeys.map((k) => ({ id: `tag:${k}`, label: `Tag: ${k}`, value: { kind: "tag", key: k } as GroupBy })),
    { id: "resource", label: cur ? "Resource" : "Resource (needs CUR)", value: { kind: "resource" }, disabled: !cur },
  ];
}

/** Keeps each group's color while it stays in the top slots (color follows the entity). */
function assignSlots(prev: Record<string, number>, topKeys: string[]): Record<string, number> {
  const next: Record<string, number> = {};
  const used = new Set<number>();
  for (const k of topKeys) {
    if (k in prev && !used.has(prev[k])) {
      next[k] = prev[k];
      used.add(prev[k]);
    }
  }
  let free = 0;
  for (const k of topKeys) {
    if (k in next) continue;
    while (used.has(free)) free++;
    next[k] = free;
    used.add(free);
  }
  return next;
}

interface Props {
  workspaceId: string;
  settings: Settings;
  /** Bumped when synced data changes. */
  dataVersion: number;
  syncing: boolean;
  onSync: () => void;
  /** CUR data is loaded, enabling per-resource breakdowns. */
  curAvailable: boolean;
}

export function CostDashboard({ workspaceId, settings, dataVersion, syncing, onSync, curAvailable }: Props) {
  const [range, setRange] = useState<DateRange>(() => presetRange("mtd"));
  const [metric, setMetric] = useState<CostMetric>(settings.default_metric);
  const [groupBy, setGroupBy] = useState<GroupBy>({ kind: "service" });
  const [accounts, setAccounts] = useState<string[]>([]);
  const [services, setServices] = useState<string[]>([]);
  const [compare, setCompare] = useState(false);

  const [overview, setOverview] = useState<CostOverview | null>(null);
  const [series, setSeries] = useState<CostSeries | null>(null);
  const [comparison, setComparison] = useState<CostComparisonRow[] | null>(null);
  const [accountOptions, setAccountOptions] = useState<DimensionValue[]>([]);
  const [serviceOptions, setServiceOptions] = useState<DimensionValue[]>([]);
  const [slots, setSlots] = useState<Record<string, number>>({});
  const [error, setError] = useState<string | null>(null);

  const serviceFilterAvailable = groupBy.kind === "service" || groupBy.kind === "account";
  // Cost data source shown with the breakdown.
  const source = groupBy.kind === "resource" ? "CUR 2.0" : "Cost Explorer";
  const query: CostQuery = useMemo(
    () => ({
      workspace_id: workspaceId,
      start: range.start,
      end: range.end,
      granularity: range.granularity,
      metric,
      group_by: groupBy,
      filter: { linked_accounts: accounts, services: serviceFilterAvailable ? services : [] },
    }),
    [workspaceId, range, metric, groupBy, accounts, services, serviceFilterAvailable],
  );
  const prev = useMemo(() => previousRange(range), [range]);

  useEffect(() => {
    let cancelled = false;
    Promise.all([
      api.dimensionValues(workspaceId, { kind: "account" }),
      api.dimensionValues(workspaceId, { kind: "service" }),
    ]).then(
      ([a, s]) => {
        if (cancelled) return;
        setAccountOptions(a);
        setServiceOptions(s);
      },
      (e) => !cancelled && setError(errorMessage(e)),
    );
    return () => {
      cancelled = true;
    };
  }, [workspaceId, dataVersion]);

  useEffect(() => {
    let cancelled = false;
    Promise.all([
      api.costOverview(workspaceId, metric, accounts),
      api.costSeries(query),
      compare ? api.compareCosts(query, prev.start, prev.end) : Promise.resolve(null),
    ]).then(
      ([o, s, c]) => {
        if (cancelled) return;
        setOverview(o);
        setSeries(s);
        setComparison(c);
        setSlots((p) => assignSlots(p, s.groups.slice(0, SLOTS).map((g) => g.key)));
        setError(null);
      },
      (e) => !cancelled && setError(errorMessage(e)),
    );
    return () => {
      cancelled = true;
    };
  }, [workspaceId, metric, accounts, query, compare, prev, dataVersion]);

  const chartSeries: ChartSeries[] = useMemo(() => {
    if (!series) return [];
    const top = series.groups.slice(0, SLOTS).filter((g) => g.key in slots);
    const rest = series.groups.slice(top.length);
    const out: ChartSeries[] = top.map((g) => ({
      key: g.key,
      label: g.label,
      colorVar: `--series-${slots[g.key] + 1}`,
      values: g.values,
    }));
    if (rest.length) {
      out.push({
        key: OTHER,
        label: `Other (${rest.length})`,
        colorVar: "--series-other",
        values: series.periods.map((_, i) => rest.reduce((sum, g) => sum + g.values[i], 0)),
      });
    }
    return out;
  }, [series, slots]);

  const setPreset = (id: string) => {
    if (id === "custom") setRange((r) => ({ ...r, preset: "custom" }));
    else setRange(presetRange(id as Exclude<DateRange["preset"], "custom">));
  };

  const changeGroup = (id: string) => {
    const option = groupOptions(settings.cost_tag_keys, curAvailable).find((o) => o.id === id);
    if (!option) return;
    setGroupBy(option.value);
    setSlots({});
  };

  const exportCsv = async () => {
    try {
      await api.exportCsv(
        query,
        compare ? [prev.start, prev.end] : null,
        `afinin-${groupId(groupBy).replace(":", "-")}-${range.start}-to-${range.end}.csv`,
      );
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  const unit = series?.unit ?? overview?.unit ?? "USD";
  const hasData = !!overview?.data_through;
  const compareByKey = new Map(comparison?.map((r) => [r.key, r]));

  if (overview && !hasData) {
    return (
      <section className="empty">
        <h2>No cost data yet</h2>
        <p className="muted">
          Sync your accounts to download costs from AWS Cost Explorer. Each request costs $0.01; a
          first sync usually takes a few dozen requests, later syncs only a handful.
        </p>
        <button className="primary" onClick={onSync} disabled={syncing}>
          {syncing ? "Syncing…" : "Sync now"}
        </button>
      </section>
    );
  }

  return (
    <div className="dashboard">
      {error && (
        <Callout tone="danger" title="Couldn’t load costs" onDismiss={() => setError(null)} dismissLabel="Dismiss">
          {error}
        </Callout>
      )}

      {overview && (
        <section className="kpis" aria-label="Overview">
          <div className="kpi">
            <div className="kpi-label">Month to date</div>
            <div className="kpi-value">{money(overview.month_to_date, unit)}</div>
            <div className="kpi-sub">
              <Delta pct={overview.month_over_month_pct} /> vs same days last month
            </div>
          </div>
          <div className="kpi">
            <div className="kpi-label">Forecast, end of month</div>
            <div className="kpi-value">
              {overview.forecast_month_end === null ? "—" : money(overview.forecast_month_end, unit)}
            </div>
            <div className="kpi-sub muted">
              {overview.forecast_month_end === null
                ? accounts.length
                  ? "Not available with an account filter"
                  : "AWS has no forecast yet"
                : "AWS forecast + actuals"}
            </div>
          </div>
          <div className="kpi">
            <div className="kpi-label">Last month</div>
            <div className="kpi-value">{money(overview.last_month_total, unit)}</div>
            <div className="kpi-sub muted">Data through {overview.data_through}</div>
          </div>
        </section>
      )}

      <section className="filters" aria-label="Filters">
        <select value={range.preset} onChange={(e) => setPreset(e.target.value)} aria-label="Date range">
          {PRESETS.map((p) => (
            <option key={p.id} value={p.id}>
              {p.label}
            </option>
          ))}
          <option value="custom">Custom…</option>
        </select>
        {range.preset === "custom" && (
          <>
            <input
              type="date"
              aria-label="Start date"
              value={range.start}
              max={range.end}
              onChange={(e) => e.target.value && setRange((r) => ({ ...r, start: e.target.value }))}
            />
            <span className="muted">to</span>
            <input
              type="date"
              aria-label="End date (exclusive)"
              value={range.end}
              min={range.start}
              max={todayUtc()}
              onChange={(e) => e.target.value && setRange((r) => ({ ...r, end: e.target.value }))}
            />
          </>
        )}
        <div className="segmented" role="group" aria-label="Granularity">
          {(["daily", "monthly"] as Granularity[]).map((g) => (
            <button
              key={g}
              className={range.granularity === g ? "active" : ""}
              onClick={() => setRange((r) => ({ ...r, granularity: g }))}
            >
              {g === "daily" ? "Daily" : "Monthly"}
            </button>
          ))}
        </div>
        <select value={groupId(groupBy)} onChange={(e) => changeGroup(e.target.value)} aria-label="Group by">
          {groupOptions(settings.cost_tag_keys, curAvailable).map((o) => (
            <option key={o.id} value={o.id} disabled={o.disabled}>
              Group by {o.label.toLowerCase()}
            </option>
          ))}
        </select>
        <select value={metric} onChange={(e) => setMetric(e.target.value as CostMetric)} aria-label="Cost metric">
          {METRICS.map((m) => (
            <option key={m.id} value={m.id}>
              {m.label}
            </option>
          ))}
        </select>
        <MultiSelect label="Accounts" options={accountOptions} selected={accounts} onChange={setAccounts} />
        {serviceFilterAvailable && (
          <MultiSelect label="Services" options={serviceOptions} selected={services} onChange={setServices} />
        )}
        <label className="check inline">
          <input type="checkbox" checked={compare} onChange={(e) => setCompare(e.target.checked)} />
          Compare with previous period
        </label>
        <span className="spacer" />
        <button className="ghost" onClick={exportCsv}>
          Export CSV
        </button>
      </section>

      {series && (
        <section className="panel">
          <header className="panel-header">
            <h2>
              {money(series.total, unit)} <span className="muted">in selected period</span>
            </h2>
            <span className="muted small">
              Source: {source}
              {series.estimated_periods.length > 0 && " · * includes AWS estimates"}
            </span>
          </header>
          {series.groups.length === 0 ? (
            <p className="muted">No costs in this period.</p>
          ) : (
            <CostChart
              periods={series.periods}
              series={chartSeries}
              granularity={range.granularity}
              unit={unit}
              estimatedPeriods={series.estimated_periods}
            />
          )}
        </section>
      )}

      {series && series.groups.length > 0 && (
        <section className="panel">
          <table className="breakdown">
            <thead>
              <tr>
                <th>{groupOptions(settings.cost_tag_keys, curAvailable).find((o) => o.id === groupId(groupBy))?.label}</th>
                <th className="num">Cost</th>
                <th className="num">Share</th>
                {compare && (
                  <>
                    <th className="num">Previous</th>
                    <th className="num">Change</th>
                  </>
                )}
              </tr>
            </thead>
            <tbody>
              {series.groups.map((g) => {
                const c = compareByKey.get(g.key);
                const slot = slots[g.key];
                return (
                  <tr key={g.key}>
                    <td>
                      <span
                        className="swatch"
                        style={{ background: `var(${slot === undefined ? "--series-other" : `--series-${slot + 1}`})` }}
                      />
                      {g.label}
                    </td>
                    <td className="num">{money(g.total, unit)}</td>
                    <td className="num muted">{series.total ? ((g.total / series.total) * 100).toFixed(1) : "0.0"}%</td>
                    {compare && (
                      <>
                        <td className="num muted">{money(c?.previous ?? 0, unit)}</td>
                        <td className="num">
                          <Delta pct={c?.delta_pct ?? null} amount={c?.delta} unit={unit} />
                        </td>
                      </>
                    )}
                  </tr>
                );
              })}
              {compare &&
                comparison
                  ?.filter((r) => r.current === 0)
                  .map((r) => (
                    <tr key={`gone-${r.key}`}>
                      <td>
                        <span className="swatch" style={{ background: "var(--series-other)" }} />
                        {r.label} <span className="muted small">(no cost now)</span>
                      </td>
                      <td className="num">{money(0, unit)}</td>
                      <td className="num muted">0.0%</td>
                      <td className="num muted">{money(r.previous, unit)}</td>
                      <td className="num">
                        <Delta pct={r.delta_pct} amount={r.delta} unit={unit} />
                      </td>
                    </tr>
                  ))}
            </tbody>
          </table>
        </section>
      )}
    </div>
  );
}

/** Cost change: up is bad (spend increased), shown with an arrow and sign, never color alone. */
function Delta({ pct, amount, unit }: { pct: number | null; amount?: number; unit?: string }) {
  if (pct === null && !amount) return <span className="muted">—</span>;
  const up = (amount ?? pct ?? 0) > 0;
  const down = (amount ?? pct ?? 0) < 0;
  return (
    <span className={up ? "delta up" : down ? "delta down" : "delta"}>
      {up ? "▲" : down ? "▼" : ""} {amount !== undefined && unit ? `${amount > 0 ? "+" : ""}${money(amount, unit)} ` : ""}
      {pct !== null && <span className="muted">({percent(pct)})</span>}
    </span>
  );
}
