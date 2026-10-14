// SPDX-License-Identifier: MIT OR Apache-2.0

import { Fragment, useEffect, useRef, useState, type ReactNode } from "react";
import { api, errorMessage } from "../api";
import type { AssistantProgress, Citation, LlmLogEntry, Turn } from "../types";
import { Callout } from "./Callout";
import { Modal } from "./Modal";

interface Entry {
  role: "user" | "assistant";
  text: string;
  turn?: Turn;
}

interface Props {
  workspaceId: string;
  /** A prepared question ("Explain this"), sent as soon as it changes. */
  prompt: { text: string; key: number } | null;
  onClose: () => void;
  onOpenSettings: () => void;
  onOpenContext: () => void;
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

function Markdown({ text, onCite }: { text: string; onCite: (n: number) => void }) {
  const blocks: ReactNode[] = [];
  let list: string[] = [];
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
  };
  for (const raw of text.split("\n")) {
    const line = raw.trimEnd();
    const bullet = /^\s*(?:[-*]|\d+\.)\s+(.*)$/.exec(line);
    if (bullet) {
      list.push(bullet[1]);
      continue;
    }
    flush();
    if (!line.trim()) continue;
    const heading = /^#{1,4}\s+(.*)$/.exec(line);
    blocks.push(
      heading ? (
        <h4 key={blocks.length}>{inline(heading[1], onCite)}</h4>
      ) : (
        <p key={blocks.length}>{inline(line, onCite)}</p>
      ),
    );
  }
  flush();
  return <div className="md">{blocks}</div>;
}

export function ChatPanel({ workspaceId, prompt, onClose, onOpenSettings, onOpenContext }: Props) {
  const [entries, setEntries] = useState<Entry[]>([]);
  const [conversation, setConversation] = useState<string | null>(null);
  const [input, setInput] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [citation, setCitation] = useState<Citation | null>(null);
  const [showLog, setShowLog] = useState(false);
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
      if (p.workspace_id !== workspaceId) return;
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
      const turn = await api.ask(workspaceId, conversation, q);
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
                {e.turn && e.turn.assumptions.length > 0 && (
                  <div className="assumptions" aria-label="Assumptions">
                    {e.turn.assumptions.map((a) => (
                      <span key={a}>{a}</span>
                    ))}
                  </div>
                )}
                {e.turn && (
                  <div className="msg-meta muted small">
                    {e.turn.model}
                    {e.turn.context_revision > 0 && ` · workspace context r${e.turn.context_revision}`}
                    {e.turn.redacted ? ` · ${e.turn.masked_values} identifiers masked` : " · not redacted (local model)"}
                    {e.turn.citations.length > 0 && ` · ${e.turn.citations.length} data source(s)`}
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
        Stored only in Afinín&apos;s encrypted local database. API keys are never logged.
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
                {e.input_tokens}+{e.output_tokens} tokens
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
