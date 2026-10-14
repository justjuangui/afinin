// SPDX-License-Identifier: MIT OR Apache-2.0

import { useEffect, useState } from "react";
import { api, errorMessage } from "../api";
import type { SsoLoginPrompt } from "../types";
import { Modal } from "./Modal";

interface Props {
  profile: string;
  onClose: () => void;
  onComplete: () => void;
}

export function SsoLoginDialog({ profile, onClose, onComplete }: Props) {
  const [prompt, setPrompt] = useState<SsoLoginPrompt | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;

    const poll = async (intervalSecs: number) => {
      if (cancelled) return;
      try {
        const status = await api.pollSsoLogin(profile);
        if (cancelled) return;
        if (status === "complete") onComplete();
        else if (status === "expired") setError("The sign-in request expired. Close this dialog and try again.");
        else timer = setTimeout(() => poll(intervalSecs), intervalSecs * 1000);
      } catch (e) {
        if (!cancelled) setError(errorMessage(e));
      }
    };

    api.startSsoLogin(profile).then(
      (p) => {
        if (cancelled) return;
        setPrompt(p);
        timer = setTimeout(() => poll(p.interval), p.interval * 1000);
      },
      (e) => !cancelled && setError(errorMessage(e)),
    );

    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
    // Start exactly one login per mounted dialog.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [profile]);

  return (
    <Modal title="Sign in with IAM Identity Center" onClose={onClose}>
      {error ? (
        <p className="error">{error}</p>
      ) : !prompt ? (
        <p className="muted">Contacting AWS…</p>
      ) : (
        <div className="stack center">
          <p>Approve the request in your browser. Confirm that it shows this code:</p>
          <div className="user-code">{prompt.user_code}</div>
          <p className="muted small">
            Browser didn't open? Go to <code>{prompt.verification_uri}</code> and enter the code.
          </p>
          <p className="status pending">Waiting for approval…</p>
        </div>
      )}
    </Modal>
  );
}
