// SPDX-License-Identifier: MIT OR Apache-2.0

import { useEffect, useState } from "react";
import { api, errorMessage } from "../api";
import { duration, relativeTime, tokens, usd } from "../format";
import type { Recheck, StoredTurn, Turn } from "../types";
import { Callout } from "./Callout";

type Result =
  | { state: "running"; step: string | null }
  | { state: "done"; recheck: Recheck }
  | { state: "failed"; error: string };

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
      const step =
        p.stage === "tool"
          ? p.label
          : p.round === 0
            ? "Reading the question"
            : "Reviewing the results";
      const id = p.recheck_turn_id;
      setResults((r) =>
        r[id]?.state === "running"
          ? { ...r, [id]: { state: "running", step } }
          : r,
      );
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
        setResults((r) => ({
          ...r,
          [t.id]: { state: "failed", error: errorMessage(e) },
        }));
      }
    }
    setRunning(false);
  };

  const saveExpected = async (t: StoredTurn, expected: string[]) => {
    try {
      const stored = await api.setReferenceExpected(t.id, expected);
      setRefs(
        (list) =>
          list?.map((x) => (x.id === t.id ? { ...x, expected: stored } : x)) ??
          null,
      );
    } catch (e) {
      setError(errorMessage(e));
    }
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
        <button
          className="primary small"
          disabled={running || refs.length === 0}
          onClick={() => void runAll()}
        >
          {running ? "Re-checking…" : `Re-check all (${refs.length})`}
        </button>
      </div>
      {refs.length === 0 ? (
        <p className="muted small">
          Mark answers you trust with <b>Use as reference</b>. Re-checking asks
          them again against current data and context, so you can see when an
          answer changes after a sync, a context edit or a new model.
        </p>
      ) : (
        <p className="muted small">
          Re-checks ask each question again in a fresh conversation, from the
          screen it was asked on. New answers aren’t saved to History; their
          requests appear in “Show what was sent”. Add <b>expected values</b> to
          questions about closed periods (“September 2026”), whose figures
          shouldn’t change: a re-check fails when one is missing.
        </p>
      )}
      <Summary refs={refs} results={results} />
      <ul className="plain reference-list">
        {refs.map((t) => {
          const r = results[t.id];
          return (
            <li key={t.id}>
              <div className="reference-head">
                <button
                  className="link"
                  onClick={() => setOpen(open === t.id ? null : t.id)}
                  disabled={r?.state !== "done"}
                >
                  {t.question}
                </button>
                <span className="muted small">
                  answered {relativeTime(t.created_at)}
                </span>
                <span className="spacer" />
                {r && <Status result={r} />}
                <button
                  className="ghost small"
                  disabled={running}
                  onClick={() => void remove(t)}
                >
                  Remove
                </button>
              </div>
              <ExpectedValues
                turn={t}
                results={r?.state === "done" ? r.recheck.expectations : null}
                disabled={running}
                onChange={(expected) => void saveExpected(t, expected)}
              />
              {r?.state === "failed" && (
                <span className="error small">{r.error}</span>
              )}
              {r?.state === "done" && open === t.id && (
                <Comparison recheck={r.recheck} />
              )}
            </li>
          );
        })}
      </ul>
    </section>
  );
}

/** Whether anything worth a look changed: the queries, or unmatched figures. */
function changed(r: Recheck): boolean {
  return (
    r.queries_added.length > 0 ||
    r.queries_removed.length > 0 ||
    r.after.unverified_figures.length > 0
  );
}

const missed = (r: Recheck) => r.expectations.filter((e) => !e.met).length;

function Status({ result }: { result: Result }) {
  if (result.state === "running")
    return <span className="status pending">{result.step ?? "Asking…"}</span>;
  if (result.state === "failed")
    return <span className="status fail">Failed</span>;
  const r = result.recheck;
  if (missed(r) > 0)
    return <span className="status fail">Missing {missed(r)} expected</span>;
  if (changed(r))
    return <span className="status pending">Changed · review</span>;
  return (
    <span className="status ok">
      {r.expectations.length > 0 ? "Expected values met" : "Same queries"}
    </span>
  );
}

/** Total cost, or null when any answer has no price. */
function totalCost(turns: Turn[]): number | null {
  let total = 0;
  for (const t of turns) {
    if (t.cost_usd == null) return null;
    total += t.cost_usd;
  }
  return total;
}

/** After a run: how many passed, and what the run cost against the references. */
function Summary({
  refs,
  results,
}: {
  refs: StoredTurn[];
  results: Record<number, Result>;
}) {
  const rechecks = refs.flatMap((t) => {
    const r = results[t.id];
    return r?.state === "done" ? [r.recheck] : [];
  });
  if (rechecks.length === 0) return null;
  const errors = refs.filter((t) => results[t.id]?.state === "failed").length;
  const failed = rechecks.filter((r) => missed(r) > 0).length + errors;
  const review = rechecks.filter((r) => missed(r) === 0 && changed(r)).length;
  const passed = rechecks.filter((r) => missed(r) === 0 && !changed(r)).length;
  const now = totalCost(rechecks.map((r) => r.after));
  const before = totalCost(rechecks.map((r) => r.before));
  const roundsNow = rechecks.reduce((a, r) => a + r.after.rounds.length, 0);
  const roundsBefore = rechecks.reduce((a, r) => a + r.before.rounds.length, 0);
  return (
    <p className="small recheck-summary">
      <span className="status ok">{passed} passed</span>
      {review > 0 && <span className="status pending">{review} to review</span>}
      {failed > 0 && <span className="status fail">{failed} failed</span>}
      <span className="muted">
        {now != null &&
          `Cost ${usd(now)}${before != null ? ` (references: ${usd(before)})` : ""} · `}
        {roundsNow} rounds{roundsBefore > 0 && ` (references: ${roundsBefore})`}
      </span>
    </p>
  );
}

/** The values a reference's re-checks must state, with results after a run. */
function ExpectedValues({
  turn,
  results,
  disabled,
  onChange,
}: {
  turn: StoredTurn;
  results: Recheck["expectations"] | null;
  disabled: boolean;
  onChange: (expected: string[]) => void;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  const [figures, setFigures] = useState<string[] | null>(null);
  const edit = () => {
    setEditing(true);
    if (figures == null)
      api.referenceFigures(turn.id).then(setFigures, () => setFigures([]));
  };
  const met = (e: string) => results?.find((r) => r.expected === e)?.met;
  const suggestions = (figures ?? []).filter((f) => !turn.expected.includes(f));
  const add = (value: string) => {
    if (value.trim()) onChange([...turn.expected, value.trim()]);
    setDraft("");
  };

  if (!editing && turn.expected.length === 0) {
    return (
      <button
        className="link small expected-add"
        disabled={disabled}
        onClick={edit}
      >
        + Expected values
      </button>
    );
  }
  return (
    <div className="expected">
      <span className="muted small">Expected:</span>
      {turn.expected.map((e) => {
        const ok = met(e);
        return (
          <span
            key={e}
            className={`expected-chip${ok === false ? " missed" : ok ? " met" : ""}`}
          >
            {ok === false ? "✗ " : ok ? "✓ " : ""}
            {e}
            {editing && (
              <button
                className="ghost icon"
                aria-label={`Remove ${e}`}
                disabled={disabled}
                onClick={() => onChange(turn.expected.filter((x) => x !== e))}
              >
                ×
              </button>
            )}
          </span>
        );
      })}
      {editing ? (
        <>
          {suggestions.map((f) => (
            <button
              key={f}
              className="ghost small"
              disabled={disabled}
              onClick={() => add(f)}
              title="From the reference answer"
            >
              + {f}
            </button>
          ))}
          <form
            className="inline-row"
            onSubmit={(e) => {
              e.preventDefault();
              add(draft);
            }}
          >
            <input
              aria-label="Expected value"
              placeholder="A figure or text the answer must contain"
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
            />
            <button
              className="ghost small"
              disabled={disabled || !draft.trim()}
            >
              Add
            </button>
            <button
              type="button"
              className="ghost small"
              onClick={() => setEditing(false)}
            >
              Done
            </button>
          </form>
        </>
      ) : (
        <button className="link small" disabled={disabled} onClick={edit}>
          Edit
        </button>
      )}
    </div>
  );
}

/** Rounds, tokens, time and cost of an answer. */
function Cost({ turn }: { turn: Turn }) {
  if (turn.rounds.length === 0)
    return (
      <p className="muted small">
        No trace (saved before traces were recorded).
      </p>
    );
  const input =
    turn.input_tokens + turn.cache_read_tokens + turn.cache_write_tokens;
  return (
    <p className="muted small">
      {turn.rounds.length} round{turn.rounds.length === 1 ? "" : "s"} ·{" "}
      {tokens(input)} in · {tokens(turn.output_tokens)} out ·{" "}
      {duration(turn.elapsed_ms)}
      {turn.cost_usd != null && ` · ${usd(turn.cost_usd)}`}
    </p>
  );
}

function Comparison({ recheck }: { recheck: Recheck }) {
  const { before, after } = recheck;
  const missing = recheck.expectations.filter((e) => !e.met);
  return (
    <div className="comparison">
      {missing.length > 0 && (
        <Callout
          tone="danger"
          title="Expected values missing from the new answer"
        >
          {missing.map((e) => e.expected).join(", ")}. If the data really
          changed, edit the expected values; otherwise check the queries and the
          workspace context.
        </Callout>
      )}
      {(recheck.queries_removed.length > 0 ||
        recheck.queries_added.length > 0) && (
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
          <h4 className="label">
            Reference
            {before.data_as_of &&
              ` · data as of ${relativeTime(before.data_as_of)}`}
          </h4>
          <Cost turn={before} />
          <Assumptions list={before.assumptions} />
          <div className="answer-text">{before.answer}</div>
        </div>
        <div>
          <h4 className="label">
            Now
            {after.data_as_of &&
              ` · data as of ${relativeTime(after.data_as_of)}`}
          </h4>
          <Cost turn={after} />
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
