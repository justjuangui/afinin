// SPDX-License-Identifier: MIT OR Apache-2.0

import { useEffect, useState } from "react";
import { api, errorMessage } from "../api";
import { relativeTime } from "../format";
import type { Recheck, StoredTurn } from "../types";
import { Callout } from "./Callout";

type Result = { state: "running"; step: string | null } | { state: "done"; recheck: Recheck } | { state: "failed"; error: string };

/**
 * Reference answers and their re-checks: each question is asked again
 * against current data and context, and the user compares the two answers.
 */
export function ReferenceChecks({ workspaceId }: { workspaceId: string }) {
  const [refs, setRefs] = useState<StoredTurn[] | null>(null);
  const [results, setResults] = useState<Record<number, Result>>({});
  const [running, setRunning] = useState(false);
  const [open, setOpen] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    api.referenceTurns(workspaceId).then(
      (r) => !cancelled && setRefs(r),
      (e) => !cancelled && setError(errorMessage(e)),
    );
    const unlisten = api.onAssistantProgress((p) => {
      if (p.workspace_id !== workspaceId || p.recheck_turn_id == null) return;
      const step = p.stage === "tool" ? p.label : p.round === 0 ? "Reading the question" : "Reviewing the results";
      const id = p.recheck_turn_id;
      setResults((r) => (r[id]?.state === "running" ? { ...r, [id]: { state: "running", step } } : r));
    });
    return () => {
      cancelled = true;
      void unlisten.then((f) => f());
    };
  }, [workspaceId]);

  const runAll = async () => {
    if (!refs) return;
    setRunning(true);
    setResults({});
    // One at a time: each re-check is a full assistant turn.
    for (const t of refs) {
      setResults((r) => ({ ...r, [t.id]: { state: "running", step: null } }));
      try {
        const recheck = await api.recheckReference(workspaceId, t.id);
        setResults((r) => ({ ...r, [t.id]: { state: "done", recheck } }));
      } catch (e) {
        setResults((r) => ({ ...r, [t.id]: { state: "failed", error: errorMessage(e) } }));
      }
    }
    setRunning(false);
  };

  const remove = async (t: StoredTurn) => {
    try {
      await api.setReferenceTurn(t.id, false);
      setRefs((list) => list?.filter((x) => x.id !== t.id) ?? null);
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  if (error) {
    return (
      <Callout tone="danger" title="Couldn’t load reference answers">
        {error}
      </Callout>
    );
  }
  if (!refs) return null;

  return (
    <section className="references">
      <div className="panel-header">
        <h3>Reference answers</h3>
        <button className="primary small" disabled={running || refs.length === 0} onClick={() => void runAll()}>
          {running ? "Re-checking…" : `Re-check all (${refs.length})`}
        </button>
      </div>
      {refs.length === 0 ? (
        <p className="muted small">
          Mark answers you trust with <b>Use as reference</b>. Re-checking asks them again against current data and
          context, so you can see when an answer changes after a sync, a context edit or a new model.
        </p>
      ) : (
        <p className="muted small">
          Re-checks ask each question again in a fresh conversation, from the screen it was asked on. New answers aren’t
          saved to History; their requests appear in “Show what was sent”.
        </p>
      )}
      <ul className="plain reference-list">
        {refs.map((t) => {
          const r = results[t.id];
          return (
            <li key={t.id}>
              <div className="reference-head">
                <button className="link" onClick={() => setOpen(open === t.id ? null : t.id)} disabled={r?.state !== "done"}>
                  {t.question}
                </button>
                <span className="muted small">answered {relativeTime(t.created_at)}</span>
                <span className="spacer" />
                {r && <Status result={r} />}
                <button className="ghost small" disabled={running} onClick={() => void remove(t)}>
                  Remove
                </button>
              </div>
              {r?.state === "failed" && <span className="error small">{r.error}</span>}
              {r?.state === "done" && open === t.id && <Comparison recheck={r.recheck} />}
            </li>
          );
        })}
      </ul>
    </section>
  );
}

/** Whether anything worth a look changed: the queries, or unmatched figures. */
function changed(r: Recheck): boolean {
  return r.queries_added.length > 0 || r.queries_removed.length > 0 || r.after.unverified_figures.length > 0;
}

function Status({ result }: { result: Result }) {
  if (result.state === "running") return <span className="status pending">{result.step ?? "Asking…"}</span>;
  if (result.state === "failed") return <span className="status fail">Failed</span>;
  return changed(result.recheck) ? (
    <span className="status pending">Changed · review</span>
  ) : (
    <span className="status ok">Same queries</span>
  );
}

function Comparison({ recheck }: { recheck: Recheck }) {
  const { before, after } = recheck;
  return (
    <div className="comparison">
      {(recheck.queries_removed.length > 0 || recheck.queries_added.length > 0) && (
        <Callout tone="warn" title="The assistant ran different queries">
          <ul className="plain">
            {recheck.queries_removed.map((q) => (
              <li key={`-${q}`}>− {q}</li>
            ))}
            {recheck.queries_added.map((q) => (
              <li key={`+${q}`}>+ {q}</li>
            ))}
          </ul>
        </Callout>
      )}
      {after.unverified_figures.length > 0 && (
        <Callout tone="warn" title="Some figures don’t match the data">
          {after.unverified_figures.join(", ")}
        </Callout>
      )}
      <div className="comparison-grid">
        <div>
          <h4 className="label">Reference{before.data_as_of && ` · data as of ${relativeTime(before.data_as_of)}`}</h4>
          <Assumptions list={before.assumptions} />
          <div className="answer-text">{before.answer}</div>
        </div>
        <div>
          <h4 className="label">Now{after.data_as_of && ` · data as of ${relativeTime(after.data_as_of)}`}</h4>
          <Assumptions list={after.assumptions} />
          <div className="answer-text">{after.answer}</div>
        </div>
      </div>
    </div>
  );
}

function Assumptions({ list }: { list: string[] }) {
  if (list.length === 0) return null;
  return (
    <div className="assumptions">
      {list.map((a) => (
        <span key={a}>{a}</span>
      ))}
    </div>
  );
}
