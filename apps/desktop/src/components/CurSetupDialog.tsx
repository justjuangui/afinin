// SPDX-License-Identifier: MIT OR Apache-2.0

import { useEffect, useState } from "react";
import { api, errorMessage } from "../api";
import type { Account, CurConfig, CurValidation } from "../types";
import { Callout } from "./Callout";
import { DocsLink } from "./DocsLink";
import { Modal } from "./Modal";

const DOCS = "https://docs.aws.amazon.com/cur/latest/userguide/dataexports-create.html";
const ICON = { ok: "✓", warning: "!", error: "✕" } as const;
const LABEL = { ok: "OK", warning: "Warning", error: "Problem" } as const;

interface Props {
  account: Account;
  onClose: () => void;
  onSaved: () => void;
}

/** CUR 2.0 setup (FR-15). Afinín never creates the export: the user deploys the template. */
export function CurSetupDialog({ account, onClose, onSaved }: Props) {
  const [step, setStep] = useState<1 | 2>(1);
  const [config, setConfig] = useState<CurConfig>({
    account_id: account.id,
    bucket: "",
    prefix: "cur",
    export_name: "afinin",
    region: "us-east-1",
  });
  const [existing, setExisting] = useState(false);
  const [help, setHelp] = useState<{ deploy_command: string; read_policy: string } | null>(null);
  const [validation, setValidation] = useState<CurValidation | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api.getCurConfig(account.id).then(
      (c) => {
        if (c) {
          setConfig(c);
          setExisting(true);
          setStep(2);
        }
      },
      (e) => setError(errorMessage(e)),
    );
  }, [account.id]);

  useEffect(() => {
    api.curSetupHelp(config.bucket).then(setHelp, (e) => setError(errorMessage(e)));
  }, [config.bucket]);

  const update = (patch: Partial<CurConfig>) => {
    setConfig((c) => ({ ...c, ...patch }));
    setValidation(null);
  };

  const validate = async () => {
    setBusy(true);
    setError(null);
    try {
      setValidation(await api.validateCur(config));
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const save = async () => {
    try {
      await api.saveCurConfig(config);
      onSaved();
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  const remove = async () => {
    try {
      await api.deleteCurConfig(account.id);
      onSaved();
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  const hasErrors = validation?.checks.some((c) => c.status === "error") ?? true;
  const detected = validation?.detected;
  const differs =
    detected &&
    (detected.bucket !== config.bucket ||
      detected.prefix !== config.prefix.replace(/^\/+|\/+$/g, "") ||
      detected.region !== config.region);

  return (
    <Modal
      title={`CUR 2.0 for ${account.name}`}
      onClose={onClose}
      footer={
        step === 1 ? (
          <>
            <button className="ghost" onClick={onClose}>Cancel</button>
            <button className="primary" onClick={() => setStep(2)}>
              I have an export →
            </button>
          </>
        ) : (
          <>
            {existing && (
              <button className="ghost danger" onClick={remove}>
                Disconnect CUR
              </button>
            )}
            <span className="spacer" />
            <button className="ghost" onClick={() => setStep(1)}>
              ← Setup help
            </button>
            <button className="ghost" onClick={validate} disabled={busy || !config.bucket}>
              {busy ? "Checking…" : "Check"}
            </button>
            <button className="primary" onClick={save} disabled={hasErrors}>
              Save
            </button>
          </>
        )
      }
    >
      {error && (
        <Callout tone="danger" title="CUR setup failed">
          {error}
        </Callout>
      )}
      {step === 1 ? (
        <div className="stack">
          <p>
            CUR 2.0 adds each resource&apos;s own cost and your real rates (Savings Plans, RIs, discounts). Afinín only reads
            it; you create the export once in the <strong>management (payer) account</strong>.
          </p>
          <ol className="steps">
            <li>
              Save the CloudFormation template:{" "}
              <button className="ghost" onClick={() => void api.saveCurTemplate()}>
                Save afinin-cur-export.yaml
              </button>
            </li>
            <li>
              Choose a bucket name and deploy it in <code>us-east-1</code>:
              <input placeholder="unique-bucket-name" value={config.bucket} onChange={(e) => update({ bucket: e.target.value })} />
              {help && <pre className="code">{help.deploy_command}</pre>}
            </li>
            <li>
              Allow the role Afinín uses for this account to read the bucket:
              {help && <pre className="code">{help.read_policy}</pre>}
            </li>
            <li>AWS delivers the first files within 24 hours. You can connect now; Afinín picks them up on the next sync.</li>
          </ol>
          <p className="muted small">
            Prefer the console? <DocsLink href={DOCS}>Create a CUR 2.0 export</DocsLink> with Parquet format and
            &ldquo;Include resource IDs&rdquo;.
          </p>
        </div>
      ) : (
        <div className="stack">
          <div className="form-grid">
            <label>
              Bucket
              <input value={config.bucket} onChange={(e) => update({ bucket: e.target.value })} />
            </label>
            <label>
              Bucket region
              <input value={config.region} onChange={(e) => update({ region: e.target.value })} />
            </label>
            <label>
              Prefix
              <input value={config.prefix} onChange={(e) => update({ prefix: e.target.value })} />
            </label>
            <label>
              Export name
              <input value={config.export_name} onChange={(e) => update({ export_name: e.target.value })} />
            </label>
          </div>
          <p className="muted small">
            Afinín reads <code>s3://{config.bucket || "<bucket>"}/{config.prefix ? `${config.prefix.replace(/^\/+|\/+$/g, "")}/` : ""}{config.export_name}/</code>.
            Downloads cost about $0.09/GB of S3 transfer; files are loaded into the encrypted database and deleted.
          </p>

          {validation && (
            <ul className="checks">
              {validation.checks.map((c, i) => (
                <li key={i} className={`check-${c.status}`}>
                  <span className="check-icon" aria-hidden>
                    {ICON[c.status]}
                  </span>
                  <span className="visually-hidden">{LABEL[c.status]}: </span>
                  {c.message}
                </li>
              ))}
            </ul>
          )}
          {detected && differs && (
            <Callout
              tone="action"
              title="The export delivers somewhere else"
              actions={
                <button className="primary small" onClick={() => update(detected)}>
                  Use these settings
                </button>
              }
            >
              It writes to <code>s3://{detected.bucket}/{detected.prefix}</code> in <code>{detected.region}</code>.
            </Callout>
          )}
          {!validation && <p className="muted small">Check the settings before saving.</p>}
        </div>
      )}
    </Modal>
  );
}
