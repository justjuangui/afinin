// SPDX-License-Identifier: MIT OR Apache-2.0

import { useEffect, useState } from "react";
import type { SyncProgress } from "../types";

/** Ideas for using Afinín, shown while a sync runs. */
const TIPS = [
  "Ask the assistant “Why did our bill go up last month?” Every figure in its answer links to the data behind it.",
  "The Context tab teaches the assistant your business: what each account is for, what your tags and terms mean.",
  "Mark an answer you trust as a reference. Similar questions follow its queries, and re-checks tell you when it changes.",
  "“Trace” under an answer shows each round, the tokens it used and what it cost.",
  "Switch the cost metric to Amortized to spread Savings Plan and reservation fees over the days they cover.",
  "The Network layout groups connected resources into clusters; Flow lays them out from entry points to data.",
  "Applications groups resources by a tag. The “(untagged)” group shows what nobody has claimed yet.",
  "Connect a CUR 2.0 export to see each resource’s own cost and savings at your real rates.",
  "Dismiss or snooze a finding you’ve decided to keep. Snoozed findings come back when the snooze ends.",
  "“Compare with previous period” on Costs shows what changed for each service, account or tag.",
  "Set a monthly budget for the assistant in Settings: it stops answering once the estimate reaches it.",
  "“Show what was sent” lists every request to the model, with account IDs and names masked.",
  "Settings → Backup exports everything (never credentials) to one password-encrypted file.",
];

const STAGE_LABEL: Record<SyncProgress["stage"], string> = {
  connecting: "Connecting",
  costs: "Fetching costs from Cost Explorer",
  cur: "Loading CUR files",
  inventory: "Scanning resources",
  findings: "Checking prices, reservations and anomalies",
};

const TIP_MS = 8000;

/** Afinín's tuning fork, vibrating: the name comes from “afinar”, to tune. */
function TuningFork() {
  return (
    <svg className="sync-fork" viewBox="0 0 512 512" aria-hidden="true">
      <g className="sync-fork-prongs">
        <path d="M196 120v150a60 60 0 0 0 120 0V120" fill="none" strokeWidth="40" strokeLinecap="round" />
      </g>
      <path d="M256 330v80" strokeWidth="40" strokeLinecap="round" />
      {[0, 1, 2].map((i) => (
        <g key={i} className="sync-wave" style={{ animationDelay: `${i * 0.6}s` }}>
          <path d="M96 236c20-28 40-28 60 0" fill="none" strokeWidth="22" strokeLinecap="round" />
          <path d="M356 236c20-28 40-28 60 0" fill="none" strokeWidth="22" strokeLinecap="round" />
        </g>
      ))}
    </svg>
  );
}

/**
 * Covers the workspace while a sync the user started runs: what it is doing,
 * and tips for using Afinín. “Keep working” hides it; the sync goes on.
 */
export function SyncOverlay({
  accounts,
  progress,
  onHide,
}: {
  /** The workspace's accounts in sync order, with names. */
  accounts: { id: string; name: string }[];
  progress: SyncProgress | null;
  onHide: () => void;
}) {
  const [tip, setTip] = useState(() => Math.floor(Math.random() * TIPS.length));

  useEffect(() => {
    const t = window.setInterval(() => setTip((i) => (i + 1) % TIPS.length), TIP_MS);
    return () => window.clearInterval(t);
  }, [tip]);

  const index = progress ? accounts.findIndex((a) => a.id === progress.account_id) : -1;
  const account = index >= 0 ? accounts[index] : null;
  const regions = progress?.stage === "inventory" ? progress : null;

  return (
    <div className="sync-overlay" role="dialog" aria-modal="true" aria-labelledby="sync-title">
      <div className="sync-card">
        <TuningFork />
        <h2 id="sync-title">Tuning your data…</h2>
        <p className="sync-stage" aria-live="polite">
          {progress ? STAGE_LABEL[progress.stage] : "Starting"}
          {regions && regions.total > 0 && ` · ${regions.done} of ${regions.total} regions`}
        </p>
        {account && (
          <p className="muted small">
            {account.name}
            {accounts.length > 1 && ` · account ${index + 1} of ${accounts.length}`}
          </p>
        )}
        <div className="sync-progress" aria-hidden={!regions}>
          {regions && regions.total > 0 ? (
            <span style={{ width: `${(regions.done / regions.total) * 100}%` }} />
          ) : (
            <span className="indeterminate" />
          )}
        </div>

        <figure className="sync-tip">
          <figcaption>Did you know?</figcaption>
          <p key={tip}>{TIPS[tip]}</p>
          <div className="sync-tip-nav">
            <button
              className="ghost small"
              aria-label="Previous tip"
              onClick={() => setTip((i) => (i - 1 + TIPS.length) % TIPS.length)}
            >
              ‹
            </button>
            <span className="muted small">
              {tip + 1} / {TIPS.length}
            </span>
            <button className="ghost small" aria-label="Next tip" onClick={() => setTip((i) => (i + 1) % TIPS.length)}>
              ›
            </button>
          </div>
        </figure>

        <button className="link small" onClick={onHide}>
          Keep working while it syncs
        </button>
      </div>
    </div>
  );
}
