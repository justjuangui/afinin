// SPDX-License-Identifier: MIT OR Apache-2.0

import { relativeTime } from "../format";
import { KIND_LABEL } from "../profileKind";
import type { Account } from "../types";

interface Props {
  account: Account;
  testing: boolean;
  onTest: () => void;
  onSync: () => void;
  hasCur: boolean;
  onCur: () => void;
  onSignIn: () => void;
  onRemove: () => void;
}

export function AccountRow({ account, testing, onTest, onSync, hasCur, onCur, onSignIn, onRemove }: Props) {
  const test = account.last_test;
  const sync = account.sync;
  return (
    <tr>
      <td>
        <strong>{account.name}</strong>
        {account.kind === "static_keys" && (
          <span className="badge warn" title="Long-lived access keys are discouraged. Migrate to IAM Identity Center.">
            access keys
          </span>
        )}
      </td>
      <td>
        <code>{account.profile}</code>
        <div className="muted small">{KIND_LABEL[account.kind]}</div>
      </td>
      <td>{account.aws_account_id ? <code>{account.aws_account_id}</code> : <span className="muted">—</span>}</td>
      <td>
        {testing ? (
          <span className="status pending">Testing…</span>
        ) : !test ? (
          <span className="status">Not tested</span>
        ) : test.ok ? (
          <span className="status ok" title={test.arn ?? undefined}>
            Connected
          </span>
        ) : (
          <span className="status fail" title={test.error ?? undefined}>
            Failed
          </span>
        )}
        {test && !test.ok && !testing && <div className="error small clamp">{test.error}</div>}
        {test?.ok && !testing && test.missing_permissions.length > 0 && (
          <div className="small warn-text" title={test.missing_permissions.join("\n")}>
            ⚠ Missing {test.missing_permissions.length} permission{test.missing_permissions.length === 1 ? "" : "s"}:{" "}
            {test.missing_permissions.slice(0, 2).join(", ")}
            {test.missing_permissions.length > 2 && "…"}
          </div>
        )}
      </td>
      <td>
        {!sync ? (
          <span className="status">Never synced</span>
        ) : sync.state === "running" ? (
          <span className="status pending">Syncing…</span>
        ) : sync.state === "ok" ? (
          <span className="status ok">Synced {relativeTime(sync.last_success_at)}</span>
        ) : (
          <>
            <span className="status fail" title={sync.error ?? undefined}>Sync failed</span>
            <div className="error small clamp">{sync.error}</div>
            {sync.last_success_at && <div className="muted small">Last good: {relativeTime(sync.last_success_at)}</div>}
          </>
        )}
      </td>
      <td className="row-actions">
        {account.kind === "sso" && (
          <button className="ghost" onClick={onSignIn}>
            Sign in
          </button>
        )}
        <button className="ghost" onClick={onTest} disabled={testing}>
          Test
        </button>
        <button className="ghost" onClick={onSync} disabled={sync?.state === "running"}>
          Sync
        </button>
        <button className="ghost" onClick={onCur} title="CUR 2.0: per-resource costs and real rates">
          {hasCur ? "CUR ✓" : "Set up CUR"}
        </button>
        <button className="ghost danger" onClick={onRemove}>
          Remove
        </button>
      </td>
    </tr>
  );
}
