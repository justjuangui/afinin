// SPDX-License-Identifier: MIT OR Apache-2.0

import { useCallback, useEffect, useState } from "react";
import { api, errorMessage } from "../api";
import { relativeTime } from "../format";
import type { Account, ContextEntry, CurStatus, NavTarget, Settings, Workspace } from "../types";
import { AccountRow } from "./AccountRow";
import { AddAccountDialog } from "./AddAccountDialog";
import { SsoLoginDialog } from "./SsoLoginDialog";
import { ConfirmDialog } from "./Modal";
import { CostDashboard } from "./CostDashboard";
import { InventoryView } from "./InventoryView";
import { FindingsView } from "./FindingsView";
import { ChatPanel } from "./ChatPanel";
import { Callout } from "./Callout";
import { CurSetupDialog } from "./CurSetupDialog";
import { ContextView } from "./ContextView";

type Confirm = { kind: "remove-account"; account: Account } | { kind: "delete-workspace" };

type Tab = "costs" | "findings" | "inventory" | "accounts" | "context";

const TAB_LABEL: Record<Tab, string> = {
  costs: "Costs",
  findings: "Findings",
  inventory: "Inventory",
  accounts: "Accounts",
  context: "Context",
};

interface Props {
  workspace: Workspace;
  settings: Settings;
  onChanged: () => Promise<void>;
  onOpenSettings: () => void;
}

export function WorkspaceView({ workspace, settings, onChanged, onOpenSettings }: Props) {
  const [accounts, setAccounts] = useState<Account[] | null>(null);
  const [tab, setTab] = useState<Tab>("costs");
  const [dataVersion, setDataVersion] = useState(0);
  const [cur, setCur] = useState<CurStatus | null>(null);
  const [curAccount, setCurAccount] = useState<Account | null>(null);
  const [chatOpen, setChatOpen] = useState(false);
  const [chatPrompt, setChatPrompt] = useState<{ text: string; key: number } | null>(null);
  // What each tab currently shows, for the assistant ("this view").
  const [screenDetails, setScreenDetails] = useState<Partial<Record<Tab, ContextEntry[]>>>({});
  const reportCosts = useCallback((d: ContextEntry[]) => setScreenDetails((s) => ({ ...s, costs: d })), []);
  const reportFindings = useCallback((d: ContextEntry[]) => setScreenDetails((s) => ({ ...s, findings: d })), []);
  const reportInventory = useCallback((d: ContextEntry[]) => setScreenDetails((s) => ({ ...s, inventory: d })), []);
  // Navigation requested by an assistant answer; `key` re-applies the same target.
  const [nav, setNav] = useState<{ target: NavTarget; key: number } | null>(null);
  const navigate = (target: NavTarget) => {
    setNav({ target, key: Date.now() });
    setTab(target.view);
  };
  const explain = (text: string) => {
    setChatPrompt({ text, key: Date.now() });
    setChatOpen(true);
  };
  const [error, setError] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const [ssoAccount, setSsoAccount] = useState<Account | null>(null);
  const [testing, setTesting] = useState<Set<string>>(new Set());
  const [confirming, setConfirming] = useState<Confirm | null>(null);
  const [renaming, setRenaming] = useState<string | null>(null);

  const reload = useCallback(async () => {
    try {
      setAccounts(await api.listAccounts(workspace.id));
    } catch (e) {
      setError(errorMessage(e));
    }
  }, [workspace.id]);

  useEffect(() => {
    let cancelled = false;
    api.listAccounts(workspace.id).then(
      (list) => !cancelled && setAccounts(list),
      (e) => !cancelled && setError(errorMessage(e)),
    );
    return () => {
      cancelled = true;
    };
  }, [workspace.id]);

  useEffect(() => {
    let cancelled = false;
    api.curStatus(workspace.id).then((s) => !cancelled && setCur(s), (e) => !cancelled && setError(errorMessage(e)));
    return () => {
      cancelled = true;
    };
  }, [workspace.id, dataVersion]);

  // Sync runs in the backend (manual or scheduled); refresh when it reports progress.
  useEffect(() => {
    const unlisten = api.onSyncChanged(() => {
      void reload();
      setDataVersion((v) => v + 1);
    });
    return () => {
      void unlisten.then((f) => f());
    };
  }, [reload]);

  const syncAll = async () => {
    try {
      await api.syncWorkspace(workspace.id);
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  const syncOne = async (account: Account) => {
    try {
      await api.syncAccount(account.id);
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  const test = async (account: Account) => {
    setTesting((s) => new Set(s).add(account.id));
    try {
      const updated = await api.testConnection(account.id);
      setAccounts((list) => list?.map((a) => (a.id === updated.id ? updated : a)) ?? null);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setTesting((s) => {
        const next = new Set(s);
        next.delete(account.id);
        return next;
      });
    }
  };

  const remove = async (account: Account) => {
    try {
      await api.removeAccount(account.id);
      await reload();
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  const rename = async (e: React.FormEvent) => {
    e.preventDefault();
    const name = renaming?.trim();
    setRenaming(null);
    if (!name || name === workspace.name) return;
    try {
      await api.renameWorkspace(workspace.id, name);
      await onChanged();
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  const deleteWorkspace = async () => {
    try {
      await api.deleteWorkspace(workspace.id);
      await onChanged();
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  const list = accounts ?? [];
  const syncing = list.some((a) => a.sync?.state === "running");
  const failedSyncs = list.filter((a) => a.sync?.state === "error").length;
  const lastSynced =
    list
      .map((a) => a.sync?.last_success_at)
      .filter((t): t is string => !!t)
      .sort()
      .at(-1) ?? null;

  return (
    <main className="content">
      <header className="page-header">
        <div>
          {renaming === null ? (
            <h1>{workspace.name}</h1>
          ) : (
            <form onSubmit={rename}>
              <input
                className="title-input"
                autoFocus
                value={renaming}
                onChange={(e) => setRenaming(e.target.value)}
                onBlur={rename}
                onKeyDown={(e) => e.key === "Escape" && setRenaming(null)}
              />
            </form>
          )}
          <p className="muted">
            {list.length} account{list.length === 1 ? "" : "s"} · read-only access
            {list.length > 0 && <> · last synced {relativeTime(lastSynced)}</>}
          </p>
        </div>
        <div className="actions">
          <button className="ghost" onClick={() => setRenaming(workspace.name)}>Rename</button>
          <button className="ghost danger" onClick={() => setConfirming({ kind: "delete-workspace" })}>
            Delete
          </button>
          {list.length > 0 && (
            <button className="ghost" onClick={syncAll} disabled={syncing}>
              {syncing ? "Syncing…" : "Sync now"}
            </button>
          )}
          <button className="ghost" onClick={() => setChatOpen((o) => !o)} aria-pressed={chatOpen}>
            Ask
          </button>
          <button className="primary" onClick={() => setAdding(true)}>Add account</button>
        </div>
      </header>

      {error && (
        <Callout tone="danger" title="Something went wrong" onDismiss={() => setError(null)} dismissLabel="Dismiss">
          {error}
        </Callout>
      )}

      {accounts === null ? null : list.length === 0 ? (
        <section className="empty">
          <h2>Connect your first AWS account</h2>
          <p className="muted">
            Afinín uses the profiles in your <code>~/.aws/config</code>. IAM Identity Center (SSO)
            is recommended: credentials are short-lived and nothing secret is stored on disk.
          </p>
          <button className="primary" onClick={() => setAdding(true)}>Add account</button>
        </section>
      ) : (
        <>
          <nav className="tabs" role="tablist">
            {(["costs", "findings", "inventory", "accounts", "context"] as Tab[]).map((t) => (
              <button key={t} role="tab" aria-selected={tab === t} className={tab === t ? "tab active" : "tab"} onClick={() => {
                  // A tab picked by hand never re-applies an earlier answer's view.
                  setNav(null);
                  setTab(t);
                }}>
                {TAB_LABEL[t]}
                {t === "accounts" && failedSyncs > 0 && <span className="badge warn">{failedSyncs} failed</span>}
              </button>
            ))}
          </nav>

          {tab === "costs" ? (
            <CostDashboard
              key={workspace.id}
              workspaceId={workspace.id}
              settings={settings}
              dataVersion={dataVersion}
              syncing={syncing}
              onSync={syncAll}
              curAvailable={(cur?.billing_periods.length ?? 0) > 0}
              onScreen={reportCosts}
              nav={nav?.target.view === "costs" ? nav : null}
            />
          ) : tab === "findings" ? (
            <FindingsView
              key={workspace.id}
              workspaceId={workspace.id}
              dataVersion={dataVersion}
              onExplain={explain}
              onScreen={reportFindings}
            />
          ) : tab === "inventory" ? (
            <InventoryView
              key={workspace.id}
              workspaceId={workspace.id}
              dataVersion={dataVersion}
              costTagKeys={settings.cost_tag_keys}
              onSync={syncAll}
              syncing={syncing}
              onExplain={explain}
              onScreen={reportInventory}
              nav={nav?.target.view === "inventory" ? nav : null}
            />
          ) : tab === "context" ? (
            <ContextView key={workspace.id} workspaceId={workspace.id} costTagKeys={settings.cost_tag_keys} />
          ) : (
            <table className="accounts">
              <thead>
                <tr>
                  <th>Name</th>
                  <th>Profile</th>
                  <th>Account ID</th>
                  <th>Connection</th>
                  <th>Cost data</th>
                  <th />
                </tr>
              </thead>
              <tbody>
                {list.map((a) => (
                  <AccountRow
                    key={a.id}
                    account={a}
                    testing={testing.has(a.id)}
                    onTest={() => test(a)}
                    onSync={() => syncOne(a)}
                    hasCur={cur?.configs.some((c) => c.account_id === a.id) ?? false}
                    onCur={() => setCurAccount(a)}
                    onSignIn={() => setSsoAccount(a)}
                    onRemove={() => setConfirming({ kind: "remove-account", account: a })}
                  />
                ))}
              </tbody>
            </table>
          )}
        </>
      )}

      {adding && (
        <AddAccountDialog
          workspaceId={workspace.id}
          existingProfiles={list.map((a) => a.profile)}
          onClose={() => setAdding(false)}
          onAdded={async (account) => {
            setAdding(false);
            await reload();
            void test(account);
          }}
        />
      )}

      {confirming?.kind === "remove-account" && (
        <ConfirmDialog
          title="Remove account"
          message={<>Remove “{confirming.account.name}” from this workspace? Your AWS profile is not changed.</>}
          confirmLabel="Remove"
          onCancel={() => setConfirming(null)}
          onConfirm={() => {
            setConfirming(null);
            void remove(confirming.account);
          }}
        />
      )}

      {confirming?.kind === "delete-workspace" && (
        <ConfirmDialog
          title="Delete workspace"
          message={
            <>
              Delete “{workspace.name}” and its {list.length} account{list.length === 1 ? "" : "s"}?
              Local data for these accounts is removed; your AWS profiles are not changed.
            </>
          }
          confirmLabel="Delete workspace"
          onCancel={() => setConfirming(null)}
          onConfirm={() => {
            setConfirming(null);
            void deleteWorkspace();
          }}
        />
      )}

      {chatOpen && (
        <ChatPanel
          workspaceId={workspace.id}
          prompt={chatPrompt}
          onClose={() => setChatOpen(false)}
          onOpenSettings={onOpenSettings}
          onOpenContext={() => setTab("context")}
          screen={{ view: TAB_LABEL[tab], details: screenDetails[tab] ?? [] }}
          onNavigate={navigate}
        />
      )}

      {curAccount && (
        <CurSetupDialog
          account={curAccount}
          onClose={() => setCurAccount(null)}
          onSaved={() => {
            setCurAccount(null);
            setDataVersion((v) => v + 1);
          }}
        />
      )}

      {ssoAccount && (
        <SsoLoginDialog
          profile={ssoAccount.profile}
          onClose={() => setSsoAccount(null)}
          onComplete={() => {
            const account = ssoAccount;
            setSsoAccount(null);
            void test(account);
          }}
        />
      )}
    </main>
  );
}
