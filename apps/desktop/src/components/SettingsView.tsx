// SPDX-License-Identifier: MIT OR Apache-2.0

import { useEffect, useState } from "react";
import { api, errorMessage } from "../api";
import { money } from "../format";
import type { ApiUsage, CostMetric, Settings } from "../types";

interface Props {
  settings: Settings;
  onSaved: (s: Settings) => void;
}

const INTERVALS = [
  { hours: 0, label: "Manual only" },
  { hours: 6, label: "Every 6 hours" },
  { hours: 12, label: "Every 12 hours" },
  { hours: 24, label: "Daily" },
  { hours: 168, label: "Weekly" },
];

export function SettingsView({ settings, onSaved }: Props) {
  const [draft, setDraft] = useState(settings);
  const [tags, setTags] = useState(settings.cost_tag_keys.join(", "));
  const [regions, setRegions] = useState(settings.inventory_regions.join(", "));
  const [usage, setUsage] = useState<ApiUsage[] | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api.apiUsage().then(setUsage, (e) => setError(errorMessage(e)));
  }, []);

  const save = async (e: React.FormEvent) => {
    e.preventDefault();
    const next: Settings = {
      ...draft,
      cost_tag_keys: tags
        .split(",")
        .map((t) => t.trim())
        .filter(Boolean),
      inventory_regions: regions
        .split(",")
        .map((r) => r.trim())
        .filter(Boolean),
    };
    try {
      await api.saveSettings(next);
      onSaved(next);
      setStatus("Saved");
      setError(null);
    } catch (err) {
      setError(errorMessage(err));
      setStatus(null);
    }
  };

  const totalCost = usage?.reduce((s, u) => s + u.cost_usd, 0) ?? 0;
  const totalCalls = usage?.reduce((s, u) => s + u.calls, 0) ?? 0;

  return (
    <main className="content">
      <header className="page-header">
        <div>
          <h1>Settings</h1>
          <p className="muted">Stored locally, encrypted. Credentials are never part of settings.</p>
        </div>
      </header>

      <form className="panel settings" onSubmit={save} onChange={() => setStatus(null)}>
        <h2>Sync</h2>
        <label>
          Automatic sync
          <select
            value={draft.sync_interval_hours}
            onChange={(e) => setDraft({ ...draft, sync_interval_hours: Number(e.target.value) })}
          >
            {INTERVALS.map((i) => (
              <option key={i.hours} value={i.hours}>
                {i.label}
              </option>
            ))}
          </select>
          <span className="hint">Each sync makes about 6 Cost Explorer requests ($0.01 each) per account.</span>
        </label>
        <label>
          Daily history on first sync (days)
          <input
            type="number"
            min={1}
            max={365}
            value={draft.daily_history_days}
            onChange={(e) => setDraft({ ...draft, daily_history_days: Number(e.target.value) })}
          />
          <span className="hint">The previous month is always included. Longer history means more requests once.</span>
        </label>
        <label>
          Monthly history on first sync (months)
          <input
            type="number"
            min={1}
            max={14}
            value={draft.monthly_history_months}
            onChange={(e) => setDraft({ ...draft, monthly_history_months: Number(e.target.value) })}
          />
        </label>

        <h2>Inventory</h2>
        <label className="check">
          <input
            type="checkbox"
            checked={draft.inventory_enabled}
            onChange={(e) => setDraft({ ...draft, inventory_enabled: e.target.checked })}
          />
          Discover resources during sync
        </label>
        <label>
          Regions to scan
          <input placeholder="All enabled regions" value={regions} onChange={(e) => setRegions(e.target.value)} />
          <span className="hint">
            Comma-separated, e.g. <code>us-east-1, eu-west-1</code>. Leave empty to scan every enabled region.
            Inventory uses free Describe/List calls. S3 and CloudFront are always included.
          </span>
        </label>

        <h2>Findings</h2>
        <label>
          Anomaly sensitivity
          <select
            value={draft.anomaly_sensitivity}
            onChange={(e) => setDraft({ ...draft, anomaly_sensitivity: e.target.value as Settings["anomaly_sensitivity"] })}
          >
            <option value="low">Low: only large spikes (4σ and $25/day above baseline)</option>
            <option value="medium">Medium (3σ and $10/day)</option>
            <option value="high">High: smaller changes (2σ and $5/day)</option>
          </select>
          <span className="hint">Daily spend per account and service is compared with its previous 28 days.</span>
        </label>
        <label>
          Flag snapshots older than (days)
          <input
            type="number"
            min={7}
            max={3650}
            value={draft.snapshot_age_days}
            onChange={(e) => setDraft({ ...draft, snapshot_age_days: Number(e.target.value) })}
          />
        </label>
        <label className="check">
          <input
            type="checkbox"
            checked={draft.commitments_enabled}
            onChange={(e) => setDraft({ ...draft, commitments_enabled: e.target.checked })}
          />
          Track Savings Plans and RI coverage (weekly, 4 Cost Explorer requests = $0.04)
        </label>
        <label className="check">
          <input
            type="checkbox"
            checked={draft.import_aws_anomalies}
            onChange={(e) => setDraft({ ...draft, import_aws_anomalies: e.target.checked })}
          />
          Import AWS Cost Anomaly Detection findings ($0.01 per sync)
        </label>

        <h2>Costs</h2>
        <label>
          Cost allocation tag keys
          <input placeholder="e.g. app, env, team" value={tags} onChange={(e) => setTags(e.target.value)} />
          <span className="hint">
            Comma-separated. Tags must be activated as cost allocation tags in the AWS Billing console. New keys are
            fetched on the next sync.
          </span>
        </label>
        <label>
          Default cost metric
          <select
            value={draft.default_metric}
            onChange={(e) => setDraft({ ...draft, default_metric: e.target.value as CostMetric })}
          >
            <option value="unblended">Unblended</option>
            <option value="amortized">Amortized (spreads Savings Plans / RI fees)</option>
            <option value="net_unblended">Net unblended (after discounts)</option>
          </select>
        </label>

        {error && <p className="error">{error}</p>}
        <div className="form-actions">
          {status && <span className="status ok">{status}</span>}
          <button className="primary">Save settings</button>
        </div>
      </form>

      <section className="panel">
        <h2>What Afinín costs you on AWS this month</h2>
        <p className="muted">
          Cost Explorer charges $0.01 per API request. Dashboards read local data and cost nothing.
        </p>
        {usage && usage.length === 0 && <p className="muted">No billable requests yet this month.</p>}
        {usage && usage.length > 0 && (
          <table className="breakdown">
            <thead>
              <tr>
                <th>API</th>
                <th className="num">Requests</th>
                <th className="num">Estimated cost</th>
              </tr>
            </thead>
            <tbody>
              {usage.map((u) => (
                <tr key={`${u.service}:${u.operation}`}>
                  <td>
                    <code>
                      {u.service}:{u.operation}
                    </code>
                  </td>
                  <td className="num">{u.calls}</td>
                  <td className="num">{money(u.cost_usd)}</td>
                </tr>
              ))}
              <tr className="total">
                <td>Total</td>
                <td className="num">{totalCalls}</td>
                <td className="num">{money(totalCost)}</td>
              </tr>
            </tbody>
          </table>
        )}
      </section>
    </main>
  );
}
