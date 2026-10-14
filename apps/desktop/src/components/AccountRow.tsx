// SPDX-License-Identifier: MIT OR Apache-2.0

import { KIND_LABEL } from "../profileKind";
import type { Account } from "../types";

interface Props {
  account: Account;
  testing: boolean;
  onTest: () => void;
  onSignIn: () => void;
  onRemove: () => void;
}

export function AccountRow({ account, testing, onTest, onSignIn, onRemove }: Props) {
  const test = account.last_test;
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
        <button className="ghost danger" onClick={onRemove}>
          Remove
        </button>
      </td>
    </tr>
  );
}
