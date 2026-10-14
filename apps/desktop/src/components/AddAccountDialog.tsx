// SPDX-License-Identifier: MIT OR Apache-2.0

import { useEffect, useMemo, useState } from "react";
import { api, errorMessage } from "../api";
import { KIND_LABEL, KIND_RANK } from "../profileKind";
import type { Account, AwsProfile } from "../types";
import { DocsLink } from "./DocsLink";
import { Modal } from "./Modal";

interface Props {
  workspaceId: string;
  existingProfiles: string[];
  onClose: () => void;
  onAdded: (account: Account) => void;
}

const SSO_DOCS = "https://docs.aws.amazon.com/cli/latest/userguide/cli-configure-sso.html";

export function AddAccountDialog({ workspaceId, existingProfiles, onClose, onAdded }: Props) {
  const [profiles, setProfiles] = useState<AwsProfile[] | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    api.listProfiles().then(setProfiles, (e) => setError(errorMessage(e)));
  }, []);

  const available = useMemo(
    () =>
      (profiles ?? [])
        .filter((p) => !existingProfiles.includes(p.name))
        .sort((a, b) => KIND_RANK[a.kind] - KIND_RANK[b.kind] || a.name.localeCompare(b.name)),
    [profiles, existingProfiles],
  );
  const profile = available.find((p) => p.name === selected);

  const add = async () => {
    if (!profile) return;
    setSaving(true);
    try {
      onAdded(await api.addAccount(workspaceId, profile.name, name.trim() || undefined));
    } catch (e) {
      setError(errorMessage(e));
      setSaving(false);
    }
  };

  return (
    <Modal
      title="Add AWS account"
      onClose={onClose}
      footer={
        <>
          <button className="ghost" onClick={onClose}>Cancel</button>
          <button className="primary" disabled={!profile || saving} onClick={add}>
            Add account
          </button>
        </>
      }
    >
      {error && <p className="error">{error}</p>}
      {profiles === null && !error && <p className="muted">Reading ~/.aws/config…</p>}
      {profiles !== null && available.length === 0 && (
        <div className="callout">
          <p>No unused AWS profiles found.</p>
          <p className="muted">
            Configure one with <code>aws configure sso</code>, then reopen this dialog.{" "}
            <DocsLink href={SSO_DOCS}>How to set up SSO</DocsLink>
          </p>
        </div>
      )}

      {available.length > 0 && (
        <>
          <p className="muted">Choose a profile from your AWS configuration.</p>
          <ul className="profile-list">
            {available.map((p) => (
              <li key={p.name}>
                <label className={p.name === selected ? "profile selected" : "profile"}>
                  <input
                    type="radio"
                    name="profile"
                    checked={p.name === selected}
                    onChange={() => setSelected(p.name)}
                  />
                  <div>
                    <div>
                      <code>{p.name}</code>
                      {p.kind === "sso" && <span className="badge ok">recommended</span>}
                      {p.kind === "static_keys" && <span className="badge warn">discouraged</span>}
                    </div>
                    <div className="muted small">
                      {KIND_LABEL[p.kind]}
                      {p.sso?.account_id && ` · ${p.sso.account_id}`}
                      {p.sso?.role_name && ` · ${p.sso.role_name}`}
                      {p.role_arn && ` · ${p.role_arn}`}
                      {p.region && ` · ${p.region}`}
                    </div>
                  </div>
                </label>
              </li>
            ))}
          </ul>
        </>
      )}

      {profile?.kind === "static_keys" && (
        <div className="callout warn">
          <strong>Long-lived access keys are discouraged.</strong> They never expire and are a
          common source of leaks. Prefer IAM Identity Center (SSO).{" "}
          <DocsLink href={SSO_DOCS}>Migrate to SSO</DocsLink>
        </div>
      )}

      {profile && (
        <label>
          Display name <span className="muted">(optional)</span>
          <input placeholder={profile.name} value={name} onChange={(e) => setName(e.target.value)} />
        </label>
      )}
    </Modal>
  );
}
