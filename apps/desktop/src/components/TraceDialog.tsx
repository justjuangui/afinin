// SPDX-License-Identifier: MIT OR Apache-2.0

import { useState } from "react";
import { api, errorMessage } from "../api";
import { duration, tokens, usd } from "../format";
import type { Citation, LlmLogEntry, Round, TraceFlag, Turn } from "../types";
import { Callout } from "./Callout";
import { Modal } from "./Modal";

/** Rough tokens of text sent to a model (as the core estimates). */
const estimate = (chars: number) => Math.ceil(chars / 4);

const contextTokens = (
  r: Pick<Round, "input_tokens" | "cache_read_tokens" | "cache_write_tokens">,
) => r.input_tokens + r.cache_read_tokens + r.cache_write_tokens;

/** What a flag means and what to do about it. */
function flagText(f: TraceFlag): { title: string; body: string } {
  switch (f.kind) {
    case "repeated_call":
      return {
        title: `Same query ${f.times} times`,
        body: `“${f.label}” was asked for ${f.times} times. Afinín ran it once and pointed the model at the first result, but each extra request still costs a round.`,
      };
    case "round_limit":
      return {
        title: `Stopped at the round limit (${f.rounds})`,
        body: "Every round resends the whole conversation, so this was the most expensive way to get no answer. Ask a narrower question (one period, one service), or add workspace context that names what you mean.",
      };
    case "tool_errors":
      return {
        title: `${f.count} failed tool call${f.count === 1 ? "" : "s"}`,
        body: "The model usually retries with a different input, paying a round each time. The error is in the source below; if it’s a missing filter value or tag, add it to the workspace context.",
      };
    case "large_result":
      return {
        title: `Source ${f.source} is large (~${tokens(f.tokens)} tokens)`,
        body: "It’s sent again on every later round of this conversation. Narrower periods, filters or groupings return smaller results; a new conversation stops resending it.",
      };
  }
}

function Stat({
  label,
  value,
  hint,
}: {
  label: string;
  value: string;
  hint?: string;
}) {
  return (
    <div className="trace-stat" title={hint}>
      <span className="muted small">{label}</span>
      <strong>{value}</strong>
    </div>
  );
}

function ToolLine({ c }: { c: Citation }) {
  return (
    <li className={c.ok ? undefined : "trace-error"}>
      <span className="cite">{c.n}</span> {c.label || c.tool}
      <span className="muted small">
        {c.duplicate_of != null
          ? ` · same as ${c.duplicate_of}, not run again`
          : ` · ${duration(c.ms)} · ~${tokens(estimate(c.sent_chars))} tokens sent`}
        {!c.ok && " · failed"}
      </span>
    </li>
  );
}

function RoundCard({
  round,
  sources,
  workspaceId,
}: {
  round: Round;
  sources: Citation[];
  workspaceId: string;
}) {
  const [entry, setEntry] = useState<LlmLogEntry | null>(null);
  const [error, setError] = useState<string | null>(null);
  const toggle = () => {
    if (entry) return setEntry(null);
    if (round.log_id == null) return;
    api
      .llmLogEntry(workspaceId, round.log_id)
      .then(setEntry, (e) => setError(errorMessage(e)));
  };
  const context = contextTokens(round);
  return (
    <li className="trace-round">
      <div className="trace-round-head">
        <strong>Round {round.n}</strong>
        <span className="muted small">
          {duration(round.ms)}
          {round.cost_usd != null && ` · ${usd(round.cost_usd)}`}
          {round.stop === "tool_use"
            ? ` · asked for ${round.sources.length} quer${round.sources.length === 1 ? "y" : "ies"}`
            : " · answered"}
        </span>
        {round.log_id != null && (
          <button
            className="ghost small"
            onClick={toggle}
            aria-expanded={entry != null}
          >
            {entry ? "Hide request" : "Show request"}
          </button>
        )}
      </div>
      <p className="muted small trace-tokens">
        {tokens(context)} in
        {round.cache_read_tokens > 0 &&
          ` (${tokens(round.cache_read_tokens)} from cache)`}
        {round.cache_write_tokens > 0 &&
          ` · ${tokens(round.cache_write_tokens)} cached for later`}{" "}
        · {tokens(round.output_tokens)} out
      </p>
      {sources.length > 0 && (
        <ul className="plain trace-tools">
          {sources.map((c) => (
            <ToolLine key={c.n} c={c} />
          ))}
        </ul>
      )}
      {error && (
        <Callout
          tone="danger"
          className="compact"
          title="Couldn’t load the request"
        >
          {error}
        </Callout>
      )}
      {entry && !entry.request && <p className="muted small">Cleared from the request log.</p>}
      {entry?.request && (
        <>
          <h4 className="label">Request</h4>
          <pre className="code tall">{entry.request}</pre>
          <h4 className="label">Response</h4>
          <pre className="code tall">{entry.response}</pre>
        </>
      )}
    </li>
  );
}

/** How an answer was reached and what it cost: rounds, queries, tokens, flags. */
export function TraceDialog({
  turn,
  workspaceId,
  onClose,
}: {
  turn: Turn;
  workspaceId: string;
  onClose: () => void;
}) {
  const byN = new Map(turn.citations.map((c) => [c.n, c]));
  const context = contextTokens(turn);
  const cacheShare = context > 0 ? turn.cache_read_tokens / context : 0;
  return (
    <Modal
      title="How this answer was made"
      onClose={onClose}
      footer={
        <button className="primary" onClick={onClose}>
          Close
        </button>
      }
    >
      <div className="trace-stats">
        <Stat
          label="Rounds"
          value={String(turn.rounds.length)}
          hint="Requests to the model; each resends the conversation"
        />
        <Stat
          label="Tokens in"
          value={tokens(context)}
          hint="Everything the model read, all rounds"
        />
        <Stat
          label="From cache"
          value={`${Math.round(cacheShare * 100)}%`}
          hint="Input read from the provider's prompt cache, billed at a fraction of the price"
        />
        <Stat label="Tokens out" value={tokens(turn.output_tokens)} />
        <Stat
          label="Cost"
          value={turn.cost_usd != null ? usd(turn.cost_usd) : "—"}
          hint={
            turn.cost_usd != null
              ? "Estimate from the price in Settings"
              : "Set the model's price in Settings → Assistant"
          }
        />
        <Stat label="Time" value={duration(turn.elapsed_ms)} />
      </div>
      {turn.flags.map((f, i) => {
        const { title, body } = flagText(f);
        return (
          <Callout key={i} tone="warn" className="compact" title={title}>
            {body}
          </Callout>
        );
      })}
      {turn.rounds.length === 0 ? (
        <p className="muted">
          This answer was saved before Afinín recorded traces.
        </p>
      ) : (
        <ol className="plain trace-rounds">
          {turn.rounds.map((r) => (
            <RoundCard
              key={r.n}
              round={r}
              workspaceId={workspaceId}
              sources={r.sources
                .map((n) => byN.get(n))
                .filter((c): c is Citation => c != null)}
            />
          ))}
        </ol>
      )}
    </Modal>
  );
}
