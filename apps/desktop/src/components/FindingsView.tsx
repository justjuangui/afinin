// SPDX-License-Identifier: MIT OR Apache-2.0

import { useEffect, useMemo, useState } from "react";
import { api, errorMessage } from "../api";
import { addDays, todayUtc } from "../dates";
import { money, percent } from "../format";
import type { ContextEntry, Finding, FindingKind, Insights, Severity } from "../types";
import { Callout } from "./Callout";

const SEVERITY: Record<Severity, { label: string; icon: string }> = {
  high: { label: "High", icon: "▲" },
  medium: { label: "Medium", icon: "◆" },
  low: { label: "Low", icon: "●" },
};

const KINDS: { id: FindingKind | "all"; label: string }[] = [
  { id: "all", label: "All" },
  { id: "waste", label: "Waste" },
  { id: "anomaly", label: "Anomalies" },
  { id: "pattern", label: "Patterns" },
];

interface Props {
  workspaceId: string;
  dataVersion: number;
  onExplain: (prompt: string) => void;
  /** Reports what the list shows, for the assistant. */
  onScreen: (details: ContextEntry[]) => void;
}

export function FindingsView({ workspaceId, dataVersion, onExplain, onScreen }: Props) {
  const [findings, setFindings] = useState<Finding[] | null>(null);
  const [insights, setInsights] = useState<Insights | null>(null);
  const [kind, setKind] = useState<FindingKind | "all">("all");
  const [showHidden, setShowHidden] = useState(false);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [version, setVersion] = useState(0);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    Promise.all([api.findings(workspaceId, showHidden), api.insights(workspaceId)]).then(
      ([f, i]) => {
        if (cancelled) return;
        setFindings(f);
        setInsights(i);
      },
      (e) => !cancelled && setError(errorMessage(e)),
    );
    return () => {
      cancelled = true;
    };
  }, [workspaceId, showHidden, dataVersion, version]);

  const visible = useMemo(() => (findings ?? []).filter((f) => kind === "all" || f.kind === kind), [findings, kind]);
  const open = (findings ?? []).filter((f) => f.status.status === "open");
  const savings = open.reduce((s, f) => s + (f.monthly_savings ?? 0), 0);
  const estimated = open.some((f) => f.savings_basis === "public_price");
  const anomalies = open.filter((f) => f.kind === "anomaly");
  const unit = insights?.unit ?? "USD";

  useEffect(() => {
    onScreen([
      { key: "showing", meaning: KINDS.find((k) => k.id === kind)?.label ?? kind },
      { key: "dismissed and snoozed", meaning: showHidden ? "included" : "hidden" },
    ]);
  }, [kind, showHidden, onScreen]);

  const setStatus = async (f: Finding, status: Finding["status"]) => {
    try {
      await api.setFindingStatus(f.id, status);
      setVersion((v) => v + 1);
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  return (
    <div className="findings">
      {error && (
        <Callout tone="danger" title="Couldn’t load or update findings" onDismiss={() => setError(null)} dismissLabel="Dismiss">
          {error}
        </Callout>
      )}
      <section className="kpis">
        <div className="kpi">
          <div className="kpi-label">Potential savings</div>
          <div className="kpi-value">
            {money(savings, unit)}
            <span className="kpi-unit">/mo</span>
          </div>
          <div className="kpi-sub muted">{estimated ? "Includes estimates from public prices" : "From actual costs (CUR)"}</div>
        </div>
        <div className="kpi">
          <div className="kpi-label">Open findings</div>
          <div className="kpi-value">{open.length}</div>
          <div className="kpi-sub muted">{open.filter((f) => f.severity === "high").length} high severity</div>
        </div>
        <div className="kpi">
          <div className="kpi-label">Cost anomalies, last 7 days</div>
          <div className="kpi-value">{anomalies.length}</div>
          <div className="kpi-sub muted">
            {anomalies.length ? `${money(anomalies.reduce((s, f) => s + (f.impact ?? 0), 0), unit)} above baseline` : "Spend is within its usual range"}
          </div>
        </div>
      </section>

      <div className="findings-layout">
        <section className="findings-list">
          <div className="filters">
            <div className="segmented" role="group" aria-label="Finding type">
              {KINDS.map((k) => (
                <button key={k.id} className={kind === k.id ? "active" : ""} onClick={() => setKind(k.id)}>
                  {k.label}
                </button>
              ))}
            </div>
            <label className="check inline">
              <input type="checkbox" checked={showHidden} onChange={(e) => setShowHidden(e.target.checked)} />
              Show dismissed and snoozed
            </label>
          </div>

          {findings && visible.length === 0 && (
            <div className="empty">
              <h2>Nothing to look at</h2>
              <p className="muted">
                No {kind === "all" ? "" : `${KINDS.find((k) => k.id === kind)?.label.toLowerCase()} `}findings. Afinín checks
                after every sync for unattached volumes and disks, idle IPs, load balancers and NAT gateways, stopped EC2 and
                Lightsail instances, old snapshots and unusual spend.
              </p>
            </div>
          )}

          <ul className="finding-cards">
            {visible.map((f) => {
              const isOpen = expanded === f.id;
              return (
                <li key={f.id} className={`finding ${f.status.status !== "open" ? "hidden-finding" : ""}`}>
                  <button className="finding-head" onClick={() => setExpanded(isOpen ? null : f.id)} aria-expanded={isOpen}>
                    <span className={`sev sev-${f.severity}`}>
                      <span aria-hidden>{SEVERITY[f.severity].icon}</span> {SEVERITY[f.severity].label}
                    </span>
                    <span className="finding-title">
                      <strong>{f.title}</strong>
                      <span className="muted small">
                        {f.kind === "anomaly" ? "Anomaly" : f.kind === "pattern" ? "Pattern" : "Waste"} · {f.account_id}
                        {f.region && ` · ${f.region}`}
                        {f.first_seen && ` · since ${f.first_seen}`}
                        {f.status.status === "dismissed" && " · dismissed"}
                        {f.status.status === "snoozed" && ` · snoozed until ${f.status.until}`}
                      </span>
                    </span>
                    <span className="finding-amount">
                      {f.monthly_savings !== null ? (
                        <>
                          <strong>{money(f.monthly_savings, unit)}/mo</strong>
                          <span className="muted small">
                            {f.savings_basis === "actual" ? "actual (CUR)" : "estimate (public price)"}
                          </span>
                        </>
                      ) : f.impact !== null ? (
                        <>
                          <strong>+{money(f.impact, unit)}</strong>
                          <span className="muted small">{f.kind === "anomaly" ? "above baseline" : "last 30 days"}</span>
                        </>
                      ) : null}
                    </span>
                  </button>
                  {isOpen && (
                    <div className="finding-body">
                      <p>{f.summary}</p>
                      <h4>How to fix</h4>
                      <p>{f.how_to_fix}</p>
                      {f.resources.length > 0 && (
                        <>
                          <h4>Resources</h4>
                          <ul className="plain">
                            {f.resources.map((r) => (
                              <li key={r.arn}>
                                {r.label} <code className="small wrap">{r.arn}</code>
                              </li>
                            ))}
                          </ul>
                        </>
                      )}
                      <h4>Evidence</h4>
                      <dl className="evidence">
                        {Object.entries(f.evidence).map(([k, v]) => (
                          <div key={k} className="kv">
                            <dt>{k.replace(/_/g, " ")}</dt>
                            <dd>{Array.isArray(v) ? v.join("; ") : String(v)}</dd>
                          </div>
                        ))}
                      </dl>
                      <div className="finding-actions">
                        <button
                          className="ghost"
                          onClick={() =>
                            onExplain(`Explain finding ${f.id}: why it matters, what it costs and how to fix it. Use get_finding.`)
                          }
                        >
                          Explain this
                        </button>
                        {f.status.status === "open" ? (
                          <>
                            <button className="ghost" onClick={() => setStatus(f, { status: "snoozed", until: addDays(todayUtc(), 7) })}>
                              Snooze 7 days
                            </button>
                            <button className="ghost" onClick={() => setStatus(f, { status: "snoozed", until: addDays(todayUtc(), 30) })}>
                              Snooze 30 days
                            </button>
                            <button className="ghost" onClick={() => setStatus(f, { status: "dismissed", note: null })}>
                              Dismiss
                            </button>
                          </>
                        ) : (
                          <button className="ghost" onClick={() => setStatus(f, { status: "open" })}>
                            Reopen
                          </button>
                        )}
                      </div>
                    </div>
                  )}
                </li>
              );
            })}
          </ul>
        </section>

        {insights && <Trends insights={insights} />}
      </div>
    </div>
  );
}

function Trends({ insights }: { insights: Insights }) {
  const unit = insights.unit;
  return (
    <aside className="trends">
      <section className="panel">
        <h2>Top movers</h2>
        <p className="muted small">Last 30 days vs the 30 before, by service.</p>
        {insights.top_movers.length === 0 ? (
          <p className="muted small">Not enough history yet.</p>
        ) : (
          <table className="breakdown compact">
            <tbody>
              {insights.top_movers.map((r) => (
                <tr key={r.key}>
                  <td className="clamp-cell">{r.label}</td>
                  <td className="num">
                    <span className={r.delta > 0 ? "delta up" : r.delta < 0 ? "delta down" : "delta"}>
                      {r.delta > 0 ? "▲" : r.delta < 0 ? "▼" : ""} {money(Math.abs(r.delta), unit)}
                    </span>
                  </td>
                  <td className="num muted small">{percent(r.delta_pct)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </section>

      <section className="panel">
        <h2>Data transfer</h2>
        <p className="muted small">{money(insights.data_transfer_total, unit)} in the last 30 days.</p>
        <table className="breakdown compact">
          <tbody>
            {insights.data_transfer.map((d) => (
              <tr key={d.key} title={d.key}>
                <td className="clamp-cell">{d.label}</td>
                <td className="num">{money(d.total, unit)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </section>

      <section className="panel">
        <h2>Savings Plans &amp; RIs</h2>
        {insights.commitments.length === 0 ? (
          <p className="muted small">Coverage is fetched weekly during sync (enable it in Settings).</p>
        ) : (
          <table className="breakdown compact">
            <thead>
              <tr>
                <th>Account</th>
                <th className="num">Coverage</th>
                <th className="num">Utilization</th>
              </tr>
            </thead>
            <tbody>
              {insights.commitments.map(([account, c]) => (
                <tr key={`${account}-${c.kind}-${c.service ?? ""}`}>
                  <td>
                    {account}{" "}
                    <span className="muted small">
                      {c.kind === "savings_plans" ? "SP" : `RI${c.service ? ` · ${shortService(c.service)}` : ""}`}
                    </span>
                  </td>
                  <td className="num">{c.coverage_pct === null ? "—" : `${c.coverage_pct.toFixed(0)}%`}</td>
                  <td className="num">{c.utilization_pct === null ? "—" : `${c.utilization_pct.toFixed(0)}%`}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </section>
    </aside>
  );
}

/** Short names for the services reservations cover. */
function shortService(service: string): string {
  const short: Record<string, string> = {
    "Amazon Elastic Compute Cloud - Compute": "EC2",
    "Amazon Relational Database Service": "RDS",
    "Amazon ElastiCache": "ElastiCache",
    "Amazon Redshift": "Redshift",
    "Amazon Elasticsearch Service": "OpenSearch",
  };
  return short[service] ?? service;
}
