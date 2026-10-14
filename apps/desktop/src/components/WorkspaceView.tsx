// SPDX-License-Identifier: MIT OR Apache-2.0

import { useCallback, useEffect, useState } from "react";
import { api, errorMessage } from "../api";
import type { Account, Workspace } from "../types";
import { AccountRow } from "./AccountRow";
import { AddAccountDialog } from "./AddAccountDialog";
import { SsoLoginDialog } from "./SsoLoginDialog";
import { ConfirmDialog } from "./Modal";

type Confirm = { kind: "remove-account"; account: Account } | { kind: "delete-workspace" };

interface Props {
  workspace: Workspace;
  onChanged: () => Promise<void>;
}

export function WorkspaceView({ workspace, onChanged }: Props) {
  const [accounts, setAccounts] = useState<Account[]>([]);
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

  const test = async (account: Account) => {
    setTesting((s) => new Set(s).add(account.id));
    try {
      const updated = await api.testConnection(account.id);
      setAccounts((list) => list.map((a) => (a.id === updated.id ? updated : a)));
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
            {accounts.length} account{accounts.length === 1 ? "" : "s"} · read-only access
          </p>
        </div>
        <div className="actions">
          <button className="ghost" onClick={() => setRenaming(workspace.name)}>Rename</button>
          <button className="ghost danger" onClick={() => setConfirming({ kind: "delete-workspace" })}>
            Delete
          </button>
          <button className="primary" onClick={() => setAdding(true)}>Add account</button>
        </div>
      </header>

      {error && (
        <p className="error banner" onClick={() => setError(null)} title="Dismiss">
          {error}
        </p>
      )}

      {accounts.length === 0 ? (
        <section className="empty">
          <h2>Connect your first AWS account</h2>
          <p className="muted">
            Afinín uses the profiles in your <code>~/.aws/config</code>. IAM Identity Center (SSO)
            is recommended: credentials are short-lived and nothing secret is stored on disk.
          </p>
          <button className="primary" onClick={() => setAdding(true)}>Add account</button>
        </section>
      ) : (
        <table className="accounts">
          <thead>
            <tr>
              <th>Name</th>
              <th>Profile</th>
              <th>Account ID</th>
              <th>Status</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {accounts.map((a) => (
              <AccountRow
                key={a.id}
                account={a}
                testing={testing.has(a.id)}
                onTest={() => test(a)}
                onSignIn={() => setSsoAccount(a)}
                onRemove={() => setConfirming({ kind: "remove-account", account: a })}
              />
            ))}
          </tbody>
        </table>
      )}

      {adding && (
        <AddAccountDialog
          workspaceId={workspace.id}
          existingProfiles={accounts.map((a) => a.profile)}
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
              Delete “{workspace.name}” and its {accounts.length} account{accounts.length === 1 ? "" : "s"}?
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
