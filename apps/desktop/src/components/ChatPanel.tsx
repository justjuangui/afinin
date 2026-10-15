// SPDX-License-Identifier: MIT OR Apache-2.0

import { Fragment, useEffect, useRef, useState, type ReactNode } from "react";
import { api, errorMessage } from "../api";
import { navLinks } from "../assistantNav";
import { duration, relativeTime, tokens, usd } from "../format";
import type {
  AssistantProgress,
  Citation,
  ConversationSummary,
  LlmLogEntry,
  MissingData,
  MissingNeed,
  NavTarget,
  StoredTurn,
  Rating,
  ScreenContext,
  Turn,
} from "../types";
import { Callout } from "./Callout";
import { ConfirmDialog, Modal } from "./Modal";
import { ReferenceChecks } from "./ReferenceChecks";
import { TraceDialog } from "./TraceDialog";

interface Entry {
  role: "user" | "assistant";
  text: string;
  turn?: Turn;
  rating?: Rating | null;
  ratingNote?: string | null;
  reference?: boolean;
}

interface Props {
  workspaceId: string;
  /** A prepared question ("Explain this"), sent as soon as it changes. */
  prompt: { text: string; key: number } | null;
  onClose: () => void;
  onOpenSettings: () => void;
  onOpenContext: () => void;
  /** What the user is looking at; sent with each question. */
  screen: ScreenContext;
  onNavigate: (target: NavTarget) => void;
  /** Starts the fix for missing data (CUR setup, sync, settings). */
  onFix: (need: MissingNeed) => void;
}

/** How to get each kind of missing data; `null` action when there is no in-app fix. */
const FIX: Record<MissingNeed, { label: string; action: string | null }> = {
  cur: { label: "Per-resource costs need a CUR 2.0 export", action: "Set up CUR" },
  inventory_sync: { label: "Resources need an inventory sync", action: "Sync now" },
  cost_allocation_tags: { label: "Costs by tag need cost allocation tags in Settings", action: "Open settings" },
  longer_history: { label: "Older periods need more cost history", action: "Open settings" },
  other: { label: "Some data isn’t available in Afinín", action: null },
};

function MissingDataCallout({ missing, onFix }: { missing: MissingData[]; onFix: (need: MissingNeed) => void }) {
  const needs = [...new Set(missing.map((m) => m.need))];
  return (
    <Callout
      tone="action"
      title="This answer needs more data"
      actions={needs
        .filter((n) => FIX[n].action)
        .map((n) => (
          <button key={n} className={n === needs[0] ? "primary small" : "ghost small"} onClick={() => onFix(n)}>
            {FIX[n].action}
          </button>
        ))}
    >
      <ul className="plain">
        {missing.map((m) => (
          <li key={`${m.need}:${m.detail}`}>{m.detail || FIX[m.need].label}</li>
        ))}
      </ul>
    </Callout>
  );
}

/** "Not now" on the workspace-context callout, remembered per workspace. */
const contextHintKey = (workspaceId: string) => `afinin.chat.contextHintDismissed.${workspaceId}`;

function contextHintDismissed(workspaceId: string): boolean {
  try {
    return localStorage.getItem(contextHintKey(workspaceId)) === "1";
  } catch {
    return false;
  }
}

function progressLabel(p: AssistantProgress): string {
  if (p.stage === "tool") return p.label;
  return p.round === 0 ? "Reading your question" : "Reviewing the results";
}

const SUGGESTIONS = [
  "Why did our bill go up last month?",
  "What are the top 5 savings opportunities?",
  "Which services grew the most in the last 30 days?",
  "What is connected to our biggest database?",
];

/** Inline markdown subset: **bold**, `code` and [n] citations. Never renders raw HTML. */
function inline(text: string, onCite: (n: number) => void): ReactNode[] {
  const parts = text.split(/(\*\*[^*]+\*\*|`[^`]+`|\[\d+(?:,\s*\d+)*\])/g);
  return parts.map((p, i) => {
    if (p.startsWith("**") && p.endsWith("**")) return <strong key={i}>{p.slice(2, -2)}</strong>;
    if (p.startsWith("`") && p.endsWith("`")) return <code key={i}>{p.slice(1, -1)}</code>;
    const cite = /^\[(\d+(?:,\s*\d+)*)\]$/.exec(p);
    if (cite) {
      return (
        <Fragment key={i}>
          {cite[1].split(/,\s*/).map((n) => (
            <button key={n} className="cite" onClick={() => onCite(Number(n))} title="Show the data behind this">
              {n}
            </button>
          ))}
        </Fragment>
      );
    }
    return <Fragment key={i}>{p}</Fragment>;
  });
}

/** Cells of a `| a | b |` table row. */
const cells = (line: string) =>
  line
    .trim()
    .replace(/^\|/, "")
    .replace(/\|$/, "")
    .split("|")
    .map((c) => c.trim());

const isTableRow = (line: string) => /^\s*\|.*\|\s*$/.test(line);
const isTableRule = (line: string) => /^\s*\|?(\s*:?-{2,}:?\s*\|)+\s*:?-*:?\s*\|?\s*$/.test(line);

/** Block markdown subset: paragraphs, headings, bullet lists and tables. */
function Markdown({ text, onCite }: { text: string; onCite: (n: number) => void }) {
  const blocks: ReactNode[] = [];
  let list: string[] = [];
  let table: string[][] = [];
  let header = false;
  const flush = () => {
    if (list.length) {
      blocks.push(
        <ul key={blocks.length}>
          {list.map((l, i) => (
            <li key={i}>{inline(l, onCite)}</li>
          ))}
        </ul>,
      );
      list = [];
    }
    if (table.length) {
      const head = header ? table[0] : null;
      const body = header ? table.slice(1) : table;
      blocks.push(
        <div key={blocks.length} className="md-table">
          <table>
            {head && (
              <thead>
                <tr>
                  {head.map((c, i) => (
                    <th key={i}>{inline(c, onCite)}</th>
                  ))}
                </tr>
              </thead>
            )}
            <tbody>
              {body.map((row, r) => (
                <tr key={r}>
                  {row.map((c, i) => (
                    <td key={i}>{inline(c, onCite)}</td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>,
      );
      table = [];
      header = false;
    }
  };
  const lines = text.split("\n").map((l) => l.trimEnd());
  lines.forEach((line, i) => {
    if (isTableRow(line) || (table.length > 0 && isTableRule(line))) {
      if (list.length) flush();
      // The rule under the first row makes it a header.
      if (isTableRule(line)) header = table.length === 1;
      else table.push(cells(line));
      return;
    }
    // Models sometimes leave blank lines between table rows.
    if (table.length > 0 && !line.trim() && isTableRow(lines.slice(i + 1).find((l) => l.trim()) ?? "")) return;
    const bullet = /^\s*(?:[-*]|\d+\.)\s+(.*)$/.exec(line);
    if (bullet) {
      if (table.length) flush();
      list.push(bullet[1]);
      return;
    }
    flush();
    if (!line.trim()) return;
    const heading = /^#{1,4}\s+(.*)$/.exec(line);
    blocks.push(
      heading ? (
        <h4 key={blocks.length}>{inline(heading[1], onCite)}</h4>
      ) : (
        <p key={blocks.length}>{inline(line, onCite)}</p>
      ),
    );
  });
  flush();
  return <div className="md">{blocks}</div>;
}

export function ChatPanel({ workspaceId, prompt, onClose, onOpenSettings, onOpenContext, screen, onNavigate, onFix }: Props) {
  const [entries, setEntries] = useState<Entry[]>([]);
  const [conversation, setConversation] = useState<string | null>(null);
  const [input, setInput] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [citation, setCitation] = useState<Citation | null>(null);
  const [trace, setTrace] = useState<Turn | null>(null);
  const [showLog, setShowLog] = useState(false);
  const [showHistory, setShowHistory] = useState(false);
  const [steps, setSteps] = useState<string[]>([]);
  const [hasContext, setHasContext] = useState(true);
  const [hintDismissed, setHintDismissed] = useState(() => contextHintDismissed(workspaceId));
  const bottom = useRef<HTMLDivElement>(null);
  const handled = useRef<number | null>(null);

  useEffect(() => {
    let cancelled = false;
    api.workspaceContext(workspaceId).then(
      (c) => !cancelled && setHasContext(c.revision > 0),
      () => undefined,
    );
    const unlisten = api.onAssistantProgress((p) => {
      if (p.workspace_id !== workspaceId || p.recheck_turn_id != null) return;
      const label = progressLabel(p);
      setSteps((s) => (s[s.length - 1] === label ? s : [...s, label]));
    });
    return () => {
      cancelled = true;
      void unlisten.then((f) => f());
    };
  }, [workspaceId]);

  const send = async (question: string) => {
    const q = question.trim();
    if (!q || busy) return;
    setEntries((e) => [...e, { role: "user", text: q }]);
    setInput("");
    setSteps([]);
    setBusy(true);
    setError(null);
    try {
      const turn = await api.ask(workspaceId, conversation, q, screen);
      setConversation(turn.conversation_id);
      setEntries((e) => [...e, { role: "assistant", text: turn.answer, turn }]);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  useEffect(() => {
    if (prompt && handled.current !== prompt.key) {
      handled.current = prompt.key;
      void send(prompt.text);
    }
    // Send each prepared prompt once.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [prompt]);

  useEffect(() => {
    bottom.current?.scrollIntoView({ behavior: "smooth" });
  }, [entries, busy]);

  const citations = new Map<number, Citation>();
  for (const e of entries) for (const c of e.turn?.citations ?? []) citations.set(c.n, c);

  return (
    <aside className="chat" aria-label="Assistant">
      <header>
        <h2>Assistant</h2>
        <div className="actions">
          <button className="ghost small" onClick={() => setShowHistory(true)} disabled={busy} title="Earlier conversations">
            History
          </button>
          <button className="ghost small" onClick={() => setShowLog(true)} title="Every request sent to the model">
            Show what was sent
          </button>
          <button
            className="ghost small"
            onClick={() => {
              setEntries([]);
              setConversation(null);
            }}
            disabled={busy}
          >
            New chat
          </button>
          <button className="ghost icon" onClick={onClose} aria-label="Close assistant">
            ×
          </button>
        </div>
      </header>

      <div className="chat-body">
        {entries.length === 0 && (
          <div className="chat-empty">
            {!hasContext && !hintDismissed && (
              <Callout
                tone="action"
                title="Teach the assistant your business"
                actions={
                  <button className="primary small" onClick={onOpenContext}>
                    Add workspace context
                  </button>
                }
                onDismiss={() => {
                  try {
                    localStorage.setItem(contextHintKey(workspaceId), "1");
                  } catch {
                    // Not remembered; the callout comes back next time.
                  }
                  setHintDismissed(true);
                }}
              >
                Say what each account is for, what your tags mean and what your apps are called. Answers will use
                your terms instead of guessing.
              </Callout>
            )}
            <p className="muted">
              Ask about your spend, resources and findings. The assistant answers with figures from Afinín&apos;s local data
              and cites them; it can&apos;t change anything in AWS.
            </p>
            <div className="suggestions">
              {SUGGESTIONS.map((s) => (
                <button key={s} className="ghost suggestion" onClick={() => void send(s)}>
                  {s}
                </button>
              ))}
            </div>
          </div>
        )}
        {entries.map((e, i) => (
          <div key={i} className={`msg ${e.role}`}>
            {e.role === "user" ? (
              <p>{e.text}</p>
            ) : (
              <>
                <Markdown text={e.text} onCite={(n) => setCitation(citations.get(n) ?? null)} />
                {e.turn && e.turn.missing_data.length > 0 && <MissingDataCallout missing={e.turn.missing_data} onFix={onFix} />}
                {e.turn && e.turn.unverified_figures.length > 0 && (
                  <Callout tone="warn" title="Some figures don’t match the data">
                    {e.turn.unverified_figures.join(", ")}{" "}
                    {e.turn.unverified_figures.length === 1 ? "isn’t" : "aren’t"} in any source the assistant used. It may
                    have calculated or misread {e.turn.unverified_figures.length === 1 ? "it" : "them"}; check the cited
                    sources before relying on {e.turn.unverified_figures.length === 1 ? "it" : "them"}.
                  </Callout>
                )}
                {e.turn && e.turn.assumptions.length > 0 && (
                  <div className="assumptions" aria-label="Assumptions">
                    {e.turn.assumptions.map((a) => (
                      <span key={a}>{a}</span>
                    ))}
                  </div>
                )}
                {e.turn && navLinks(e.turn).length > 0 && (
                  <div className="nav-links">
                    {navLinks(e.turn).map((l) => (
                      <button key={l.label} className="ghost small" onClick={() => onNavigate(l.target)}>
                        {l.label} →
                      </button>
                    ))}
                  </div>
                )}
                {e.turn?.turn_id != null && (
                  <AnswerFeedback
                    turnId={e.turn.turn_id}
                    rating={e.rating ?? null}
                    note={e.ratingNote ?? null}
                    reference={e.reference ?? false}
                    onRated={(rating, ratingNote) =>
                      setEntries((list) => list.map((x) => (x === e ? { ...x, rating, ratingNote } : x)))
                    }
                    onReference={(reference) =>
                      setEntries((list) => list.map((x) => (x === e ? { ...x, reference } : x)))
                    }
                  />
                )}
                {e.turn && (
                  <div className="msg-meta muted small">
                    {e.turn.model}
                    {e.turn.data_as_of && ` · data as of ${relativeTime(e.turn.data_as_of)}`}
                    {e.turn.context_revision > 0 && ` · workspace context r${e.turn.context_revision}`}
                    {e.turn.reference_examples > 0 &&
                      ` · guided by ${e.turn.reference_examples} reference answer${e.turn.reference_examples === 1 ? "" : "s"}`}
                    {e.turn.redacted ? ` · ${e.turn.masked_values} identifiers masked` : " · not redacted (local model)"}
                    {e.turn.citations.length > 0 && ` · ${e.turn.citations.length} data source(s)`}
                    {e.turn.rounds.length > 0 && ` · ${duration(e.turn.elapsed_ms)}`}
                    {e.turn.cost_usd != null && ` · ${usd(e.turn.cost_usd)}`}
                    {" · "}
                    <button className="link small" onClick={() => setTrace(e.turn ?? null)}>
                      Trace
                    </button>
                    {e.turn.flags.length > 0 && (
                      <span className="warn-text">
                        {" "}
                        ({e.turn.flags.length} cost warning{e.turn.flags.length === 1 ? "" : "s"})
                      </span>
                    )}
                  </div>
                )}
              </>
            )}
          </div>
        ))}
        {busy && (
          <div className="msg assistant muted" aria-live="polite">
            {steps.length === 0 ? (
              "Looking at your data…"
            ) : (
              <ul className="plain progress">
                {steps.map((s, i) => (
                  <li key={i} className={i === steps.length - 1 ? "now" : "done"}>
                    {s}
                  </li>
                ))}
              </ul>
            )}
          </div>
        )}
        {error &&
          (/configured|API key|Settings/i.test(error) ? (
            // A setup step, not a failure: point to where to fix it.
            <Callout
              tone="action"
              title="Set up the assistant"
              actions={
                <button className="primary small" onClick={onOpenSettings}>
                  Open settings
                </button>
              }
            >
              {error}
            </Callout>
          ) : (
            <Callout tone="danger" title="The assistant couldn’t answer" onDismiss={() => setError(null)} dismissLabel="Dismiss">
              {error}
            </Callout>
          ))}
        <div ref={bottom} />
      </div>

      <form
        className="chat-input"
        onSubmit={(e) => {
          e.preventDefault();
          void send(input);
        }}
      >
        <textarea
          rows={2}
          placeholder="Ask about costs, resources or findings…"
          value={input}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              void send(input);
            }
          }}
        />
        <button className="primary" disabled={busy || !input.trim()}>
          Send
        </button>
      </form>

      {citation && (
        <Modal title={`Source ${citation.n}: ${citation.tool}`} onClose={() => setCitation(null)}>
          <h4 className="label">Query</h4>
          <pre className="code">{JSON.stringify(citation.input, null, 2)}</pre>
          <h4 className="label">Result from Afinín&apos;s local data</h4>
          <pre className="code tall">{JSON.stringify(citation.result, null, 2)}</pre>
        </Modal>
      )}
      {showHistory && (
        <HistoryDialog
          workspaceId={workspaceId}
          current={conversation}
          onClose={() => setShowHistory(false)}
          onDeleted={(id) => {
            if (id === conversation) {
              setEntries([]);
              setConversation(null);
            }
          }}
          onOpen={(id) => {
            setShowHistory(false);
            setError(null);
            api.assistantTurns(id).then(
              (turns) => {
                setConversation(id);
                setEntries(
                  turns.flatMap((t): Entry[] => [
                    { role: "user", text: t.question },
                    {
                      role: "assistant",
                      text: t.turn.answer,
                      turn: { ...t.turn, turn_id: t.id },
                      rating: t.rating,
                      ratingNote: t.rating_note,
                      reference: t.reference,
                    },
                  ]),
                );
              },
              (e) => setError(errorMessage(e)),
            );
          }}
        />
      )}
      {trace && <TraceDialog turn={trace} workspaceId={workspaceId} onClose={() => setTrace(null)} />}
      {showLog && <LlmLogDialog workspaceId={workspaceId} onClose={() => setShowLog(false)} />}
    </aside>
  );
}

/** "Show what was sent" (FR-45): every request exactly as it left this machine. */
function LlmLogDialog({ workspaceId, onClose }: { workspaceId: string; onClose: () => void }) {
  const [entries, setEntries] = useState<LlmLogEntry[] | null>(null);
  const [open, setOpen] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api.llmLog(workspaceId).then(setEntries, (e) => setError(errorMessage(e)));
  }, [workspaceId]);

  const clear = async () => {
    try {
      await api.clearLlmLog(workspaceId);
      setEntries([]);
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  return (
    <Modal
      title="What was sent to the model"
      onClose={onClose}
      footer={
        <>
          <button className="ghost danger" onClick={clear} disabled={!entries?.length}>
            Clear log
          </button>
          <span className="spacer" />
          <button className="primary" onClick={onClose}>
            Close
          </button>
        </>
      }
    >
      {error && (
        <Callout tone="danger" title="Couldn’t load the request log">
          {error}
        </Callout>
      )}
      <p className="muted small">
        Stored only in Afinín&apos;s encrypted local database. API keys are never logged. Clearing keeps token counts
        for usage and the budget.
      </p>
      {entries?.length === 0 && <p className="muted">No requests yet.</p>}
      <ul className="log">
        {entries?.map((e) => (
          <li key={e.id}>
            <button className="ghost log-head" onClick={() => setOpen(open === e.id ? null : e.id)} aria-expanded={open === e.id}>
              <span>{new Date(e.created_at).toLocaleString()}</span>
              <span>
                {e.provider} · {e.model}
              </span>
              <span className={e.redacted ? "badge ok" : "badge warn"}>{e.redacted ? "redacted" : "not redacted"}</span>
              <span className="muted small">
                {e.round != null && `round ${e.round} · `}
                {tokens(e.input_tokens + e.cache_read_tokens + e.cache_write_tokens)} in
                {e.cache_read_tokens > 0 && ` (${tokens(e.cache_read_tokens)} cached)`} · {tokens(e.output_tokens)} out
                {e.latency_ms != null && ` · ${duration(e.latency_ms)}`}
              </span>
            </button>
            {open === e.id && (
              <>
                <h4 className="label">Request</h4>
                <pre className="code tall">{e.request}</pre>
                <h4 className="label">Response</h4>
                <pre className="code tall">{e.response}</pre>
              </>
            )}
          </li>
        ))}
      </ul>
    </Modal>
  );
}

function AnswerFeedback({
  turnId,
  rating,
  note,
  reference,
  onRated,
  onReference,
}: {
  turnId: number;
  rating: Rating | null;
  note: string | null;
  reference: boolean;
  onRated: (rating: Rating | null, note: string | null) => void;
  onReference: (reference: boolean) => void;
}) {
  const [draft, setDraft] = useState(note ?? "");
  const [editing, setEditing] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const rate = async (next: Rating | null, nextNote: string | null) => {
    try {
      await api.rateAssistantTurn(turnId, next, nextNote);
      onRated(next, nextNote);
      setError(null);
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  return (
    <div className="feedback">
      <div className="feedback-buttons" role="group" aria-label="Rate this answer">
        <button
          className={`ghost small${rating === "up" ? " active" : ""}`}
          aria-pressed={rating === "up"}
          onClick={() => void rate(rating === "up" ? null : "up", null)}
        >
          Useful
        </button>
        <button
          className={`ghost small${rating === "down" ? " active" : ""}`}
          aria-pressed={rating === "down"}
          onClick={() => {
            if (rating === "down") {
              setEditing(false);
              void rate(null, null);
            } else {
              setEditing(true);
              void rate("down", null);
            }
          }}
        >
          Not right
        </button>
        <button
          className={`ghost small${reference ? " active" : ""}`}
          aria-pressed={reference}
          title="Re-check this answer later against current data (History → Reference answers)"
          onClick={() =>
            api.setReferenceTurn(turnId, !reference).then(
              () => onReference(!reference),
              (e) => setError(errorMessage(e)),
            )
          }
        >
          {reference ? "Reference ✓" : "Use as reference"}
        </button>
        {rating === "down" && note && !editing && <span className="muted small">“{note}”</span>}
      </div>
      {rating === "down" && editing && (
        <form
          className="feedback-note"
          onSubmit={(e) => {
            e.preventDefault();
            setEditing(false);
            void rate("down", draft.trim() || null);
          }}
        >
          <input
            autoFocus
            aria-label="What was wrong"
            placeholder="What was wrong? (optional, kept locally)"
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
          />
          <button className="ghost small">Save</button>
        </form>
      )}
      {error && <span className="error small">{error}</span>}
    </div>
  );
}

function HistoryDialog({
  workspaceId,
  current,
  onOpen,
  onDeleted,
  onClose,
}: {
  workspaceId: string;
  current: string | null;
  onOpen: (id: string) => void;
  onDeleted: (id: string) => void;
  onClose: () => void;
}) {
  const [list, setList] = useState<ConversationSummary[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [deleting, setDeleting] = useState<ConversationSummary | null>(null);

  useEffect(() => {
    let cancelled = false;
    api.assistantConversations(workspaceId).then(
      (l) => !cancelled && setList(l),
      (e) => !cancelled && setError(errorMessage(e)),
    );
    return () => {
      cancelled = true;
    };
  }, [workspaceId]);

  const remove = async (c: ConversationSummary) => {
    setDeleting(null);
    try {
      await api.deleteAssistantConversation(c.id);
      setList((l) => l?.filter((x) => x.id !== c.id) ?? null);
      onDeleted(c.id);
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  return (
    <Modal title="Conversations" onClose={onClose}>
      {error && (
        <Callout tone="danger" title="Couldn’t load conversations">
          {error}
        </Callout>
      )}
      <ReferenceChecks workspaceId={workspaceId} />
      <GapList workspaceId={workspaceId} onOpen={onOpen} />
      <h3>Conversations</h3>
      <p className="muted small">Saved in Afinín&apos;s encrypted local database, per workspace.</p>
      {list?.length === 0 && <p className="muted">No conversations yet.</p>}
      {list && list.length > 0 && (
        <table className="breakdown history">
          <tbody>
            {list.map((c) => (
              <tr key={c.id}>
                <td>
                  <button className="link" onClick={() => onOpen(c.id)}>
                    {c.title}
                  </button>
                  {c.id === current && <span className="badge ok">open</span>}
                  <div className="muted small">
                    {c.turns} {c.turns === 1 ? "question" : "questions"} · {relativeTime(c.updated_at)}
                  </div>
                </td>
                <td className="num">
                  <button className="ghost small danger" onClick={() => setDeleting(c)}>
                    Delete
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {deleting && (
        <ConfirmDialog
          title="Delete conversation"
          message={`Delete “${deleting.title}” and its ${deleting.turns} saved answers and ratings? The request log is kept.`}
          confirmLabel="Delete"
          onCancel={() => setDeleting(null)}
          onConfirm={() => void remove(deleting)}
        />
      )}
    </Modal>
  );
}

/** Questions the assistant couldn't fully answer: what to unlock next. */
function GapList({ workspaceId, onOpen }: { workspaceId: string; onOpen: (conversationId: string) => void }) {
  const [gaps, setGaps] = useState<StoredTurn[]>([]);
  useEffect(() => {
    let cancelled = false;
    api.assistantGaps(workspaceId).then(
      (g) => !cancelled && setGaps(g),
      () => undefined,
    );
    return () => {
      cancelled = true;
    };
  }, [workspaceId]);
  if (gaps.length === 0) return null;
  return (
    <details className="gaps">
      <summary>
        <h3>Couldn’t fully answer ({gaps.length})</h3>
      </summary>
      <ul className="plain">
        {gaps.map((g) => (
          <li key={g.id}>
            <button className="link" onClick={() => onOpen(g.turn.conversation_id)}>
              {g.question}
            </button>
            <span className="muted small">
              {" "}
              · {[...new Set(g.turn.missing_data.map((m) => FIX[m.need].label))].join("; ")} · {relativeTime(g.created_at)}
            </span>
          </li>
        ))}
      </ul>
    </details>
  );
}
