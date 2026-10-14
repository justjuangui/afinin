// SPDX-License-Identifier: MIT OR Apache-2.0

import { useEffect, useState } from "react";
import { api, errorMessage } from "../api";
import { relativeTime } from "../format";
import type { ContextEntry, ContextRevision, DimensionValue, StoredContext, WorkspaceContext } from "../types";
import { Modal } from "./Modal";
import { Callout } from "./Callout";

/** Mirror the limits in afinin-core `llm/context.rs`. */
const MAX_SECTION = 4000;
const MAX_NOTE = 500;
const MAX_ENTRIES = 50;
const MAX_KEY = 120;
const MAX_MEANING = 500;

type TextSection = "business" | "instructions";
type ListSection = "tags" | "glossary";

interface ListSpec {
  title: string;
  hint: string;
  keyLabel: string;
  keyPlaceholder: string;
  meaningPlaceholder: string;
  add: string;
}

const LISTS: Record<ListSection, ListSpec> = {
  tags: {
    title: "Tags",
    hint: 'What your tag keys (or key=value pairs) mean, so "what does the payments team spend" maps to the right tag.',
    keyLabel: "Tag",
    keyPlaceholder: "team  or  env=prod",
    meaningPlaceholder: "Owning squad: payments, growth, platform",
    add: "Add tag",
  },
  glossary: {
    title: "Glossary",
    hint: "Your own terms and how they map to AWS.",
    keyLabel: "Term",
    keyPlaceholder: "The data lake",
    meaningPlaceholder: "S3 buckets prefixed lake- plus Glue and Athena",
    add: "Add term",
  },
};

const SECTIONS: { key: TextSection; title: string; hint: string; placeholder: string }[] = [
  {
    key: "business",
    title: "Business",
    hint: "What the company does and which applications and environments run on these accounts.",
    placeholder:
      "Online furniture store. Main apps: storefront (web), checkout (payments API), catalog (search), data pipeline (nightly ETL).\nEnvironments: production, staging, dev.",
  },
  {
    key: "instructions",
    title: "Instructions for answers",
    hint: "How you want answers: metric, accounts to ignore, budgets to compare against.",
    placeholder: "Ignore the sandbox account unless asked.\nOur monthly budget is USD 5,000; mention it when talking about totals.\nAnswer in Spanish.",
  },
];

const EMPTY: WorkspaceContext = { business: "", accounts: {}, tags: [], glossary: [], instructions: "" };

/** Rows the backend keeps: both fields filled. */
const complete = (list: ContextEntry[]) =>
  list.map((e) => ({ key: e.key.trim(), meaning: e.meaning.trim() })).filter((e) => e.key && e.meaning);

function same(a: WorkspaceContext, b: WorkspaceContext): boolean {
  const norm = (c: WorkspaceContext) =>
    JSON.stringify({
      ...c,
      business: c.business.trim(),
      tags: complete(c.tags),
      glossary: complete(c.glossary),
      instructions: c.instructions.trim(),
      accounts: Object.fromEntries(
        Object.entries(c.accounts)
          .map(([k, v]) => [k, v.trim()])
          .filter(([, v]) => v)
          .sort(([x], [y]) => x.localeCompare(y)),
      ),
    });
  return norm(a) === norm(b);
}

interface Props {
  workspaceId: string;
  costTagKeys: string[];
}

export function ContextView({ workspaceId, costTagKeys }: Props) {
  const [stored, setStored] = useState<StoredContext | null>(null);
  const [draft, setDraft] = useState<WorkspaceContext>(EMPTY);
  const [accounts, setAccounts] = useState<DimensionValue[]>([]);
  const [saving, setSaving] = useState(false);
  const [status, setStatus] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [history, setHistory] = useState<ContextRevision[] | null>(null);
  const [tagKeys, setTagKeys] = useState<string[]>([]);

  useEffect(() => {
    let cancelled = false;
    Promise.all([api.workspaceContext(workspaceId), api.dimensionValues(workspaceId, { kind: "account" })]).then(
      ([s, a]) => {
        if (cancelled) return;
        setStored(s);
        setDraft(s.context);
        setAccounts(a);
      },
      (e) => !cancelled && setError(errorMessage(e)),
    );
    // Suggestions only; the editor works without inventory.
    api.inventoryFacets(workspaceId).then(
      (f) => !cancelled && setTagKeys(f.tag_keys.map((t) => t.key)),
      () => undefined,
    );
    return () => {
      cancelled = true;
    };
  }, [workspaceId]);

  if (!stored)
    return error ? (
      <Callout tone="danger" title="Couldn’t load the workspace context">
        {error}
      </Callout>
    ) : (
      <div className="loading">Loading…</div>
    );

  const dirty = !same(draft, stored.context);
  const lists: ListSection[] = ["tags", "glossary"];
  const tooLong =
    SECTIONS.some((s) => draft[s.key].length > MAX_SECTION) ||
    Object.values(draft.accounts).some((n) => n.length > MAX_NOTE) ||
    lists.some((k) => complete(draft[k]).length > MAX_ENTRIES);
  const duplicates = lists.flatMap((k) => {
    const keys = complete(draft[k]).map((e) => e.key.toLowerCase());
    return keys.filter((key, i) => keys.indexOf(key) !== i);
  });
  // Accounts seen in synced data, plus any noted account that is no longer there.
  const accountRows: DimensionValue[] = [
    ...accounts,
    ...Object.keys(draft.accounts)
      .filter((id) => !accounts.some((a) => a.key === id))
      .map((id) => ({ key: id, label: id })),
  ];

  const save = async () => {
    setSaving(true);
    setError(null);
    try {
      const saved = await api.saveWorkspaceContext(workspaceId, draft);
      setStored(saved);
      setDraft(saved.context);
      setStatus(saved.revision === stored.revision ? "No changes to save" : `Saved revision ${saved.revision}`);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setSaving(false);
    }
  };

  const openHistory = async () => {
    try {
      setHistory(await api.workspaceContextHistory(workspaceId));
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  return (
    <div className="context-grid">
      <form
        className="panel settings"
        onSubmit={(e) => {
          e.preventDefault();
          void save();
        }}
        onChange={() => setStatus(null)}
      >
        <div className="panel-header">
          <h2>Assistant context</h2>
          <span className="muted small">
            {stored.revision > 0 ? `Revision ${stored.revision} · saved ${relativeTime(stored.updated_at)}` : "Not saved yet"}
          </span>
        </div>
        <p className="muted small">
          Tell the assistant about the business behind these AWS accounts. It reads this with every question, so it can
          map your words (apps, teams, environments) to accounts, tags and services. Figures still come only from your
          data.
        </p>

        {SECTIONS.slice(0, 1).map((s) => (
          <Section key={s.key} section={s} value={draft[s.key]} onChange={(v) => setDraft({ ...draft, [s.key]: v })} />
        ))}

        <div className="context-section">
          <h3>AWS accounts</h3>
          <span className="hint">What each account is for. Accounts appear here after a cost sync.</span>
          {accountRows.length === 0 ? (
            <p className="muted small">No accounts synced yet.</p>
          ) : (
            <table className="breakdown context-accounts">
              <tbody>
                {accountRows.map((a) => {
                  const note = draft.accounts[a.key] ?? "";
                  return (
                    <tr key={a.key}>
                      <td className="context-account">
                        {a.label}
                        {!accounts.some((x) => x.key === a.key) && <span className="muted small"> · not in synced data</span>}
                      </td>
                      <td>
                        <input
                          aria-label={`Note for ${a.label}`}
                          placeholder="e.g. Production: checkout and catalog"
                          value={note}
                          maxLength={MAX_NOTE}
                          onChange={(e) => setDraft({ ...draft, accounts: { ...draft.accounts, [a.key]: e.target.value } })}
                        />
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          )}
        </div>

        {lists.map((k) => (
          <EntryList
            key={k}
            id={`ctx-${k}`}
            spec={LISTS[k]}
            entries={draft[k]}
            suggestions={k === "tags" ? [...new Set([...costTagKeys, ...tagKeys])].sort() : []}
            onChange={(list) => setDraft({ ...draft, [k]: list })}
          />
        ))}

        {SECTIONS.slice(1).map((s) => (
          <Section key={s.key} section={s} value={draft[s.key]} onChange={(v) => setDraft({ ...draft, [s.key]: v })} />
        ))}

        {duplicates.length > 0 && <p className="error small">&quot;{duplicates[0]}&quot; appears twice; keep one row per key.</p>}
        {error && (
          <Callout tone="danger" title="Couldn’t save the context">
            {error}
          </Callout>
        )}
        <div className="form-actions">
          {status && <span className="status ok">{status}</span>}
          <button type="button" className="ghost" disabled={!dirty || saving} onClick={() => setDraft(stored.context)}>
            Discard changes
          </button>
          <button className="primary" disabled={!dirty || saving || tooLong || duplicates.length > 0}>
            {saving ? "Saving…" : "Save"}
          </button>
        </div>
      </form>

      <aside className="settings-side">
        <section className="panel settings">
          <h2>How it is used</h2>
          <ul className="plain small context-notes">
            <li>Sent with every question as background, after the built-in notes on AWS billing.</li>
            <li>Instructions here are followed; text found inside AWS data (tags, names) never is.</li>
            <li>Each answer shows which revision it used, so you can explain a past answer after edits.</li>
          </ul>
        </section>
        <section className="panel settings">
          <h2>Privacy</h2>
          <p className="muted small">
            With masking on (the default for cloud models), account IDs, ARNs, IPs, account names and the tag values and
            resource names Afinín has discovered are replaced by placeholders before sending. Other words you write here
            are sent as written. Check exactly what was sent with <b>Show what was sent</b> in the assistant.
          </p>
        </section>
        <section className="panel settings">
          <h2>History</h2>
          <p className="muted small">Every saved revision is kept. Load an older one into the editor to restore it.</p>
          <div className="form-actions">
            <button type="button" className="ghost" disabled={stored.revision === 0} onClick={() => void openHistory()}>
              View history
            </button>
          </div>
        </section>
      </aside>

      {history && (
        <Modal title="Context history" onClose={() => setHistory(null)}>
          {history.length === 0 ? (
            <p className="muted">No saved revisions.</p>
          ) : (
            <table className="breakdown">
              <tbody>
                {history.map((h) => (
                  <tr key={h.revision}>
                    <td>
                      Revision {h.revision}
                      {h.revision === stored.revision && <span className="badge ok">current</span>}
                    </td>
                    <td className="muted">{relativeTime(h.created_at)}</td>
                    <td className="num">
                      <button
                        className="ghost small"
                        disabled={h.revision === stored.revision}
                        onClick={() => {
                          setDraft(h.context);
                          setStatus(`Loaded revision ${h.revision}; save to restore it`);
                          setHistory(null);
                        }}
                      >
                        Load into editor
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </Modal>
      )}
    </div>
  );
}

function EntryList({
  id,
  spec,
  entries,
  suggestions,
  onChange,
}: {
  id: string;
  spec: ListSpec;
  entries: ContextEntry[];
  suggestions: string[];
  onChange: (list: ContextEntry[]) => void;
}) {
  // Always show at least one empty row to type into.
  const rows = entries.length > 0 ? entries : [{ key: "", meaning: "" }];
  const set = (i: number, patch: Partial<ContextEntry>) => onChange(rows.map((e, j) => (j === i ? { ...e, ...patch } : e)));
  const incomplete = rows.some((e) => !e.key.trim() !== !e.meaning.trim());
  return (
    <div className="context-section">
      <span className="context-title">
        <h3>{spec.title}</h3>
        <span className="muted small">
          {complete(entries).length} / {MAX_ENTRIES}
        </span>
      </span>
      <span className="hint">{spec.hint}</span>
      {suggestions.length > 0 && (
        <datalist id={`${id}-keys`}>
          {suggestions.map((s) => (
            <option key={s} value={s} />
          ))}
        </datalist>
      )}
      <div className="kv-list">
        {rows.map((e, i) => (
          <div className="kv-row" key={i}>
            <input
              aria-label={`${spec.keyLabel} ${i + 1}`}
              placeholder={spec.keyPlaceholder}
              value={e.key}
              maxLength={MAX_KEY}
              list={suggestions.length > 0 ? `${id}-keys` : undefined}
              onChange={(ev) => set(i, { key: ev.target.value })}
            />
            <span className="kv-arrow" aria-hidden>
              →
            </span>
            <input
              aria-label={`Meaning of ${spec.keyLabel.toLowerCase()} ${i + 1}`}
              placeholder={spec.meaningPlaceholder}
              value={e.meaning}
              maxLength={MAX_MEANING}
              onChange={(ev) => set(i, { meaning: ev.target.value })}
            />
            <button
              type="button"
              className="ghost icon"
              aria-label={`Remove ${spec.keyLabel.toLowerCase()} ${i + 1}`}
              disabled={rows.length === 1 && !e.key && !e.meaning}
              onClick={() => onChange(rows.filter((_, j) => j !== i))}
            >
              ×
            </button>
          </div>
        ))}
      </div>
      <div className="kv-footer">
        <button
          type="button"
          className="ghost small"
          disabled={rows.length >= MAX_ENTRIES}
          onClick={() => onChange([...rows, { key: "", meaning: "" }])}
        >
          + {spec.add}
        </button>
        {incomplete && <span className="muted small">Rows need both fields to be saved.</span>}
      </div>
    </div>
  );
}

function Section({
  section,
  value,
  onChange,
}: {
  section: (typeof SECTIONS)[number];
  value: string;
  onChange: (v: string) => void;
}) {
  const over = value.length > MAX_SECTION;
  return (
    <label className="context-section">
      <span className="context-title">
        <h3>{section.title}</h3>
        <span className={over ? "error small" : "muted small"}>
          {value.length.toLocaleString()} / {MAX_SECTION.toLocaleString()}
        </span>
      </span>
      <span className="hint">{section.hint}</span>
      <textarea rows={5} placeholder={section.placeholder} value={value} onChange={(e) => onChange(e.target.value)} />
    </label>
  );
}
