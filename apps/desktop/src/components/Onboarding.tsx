// SPDX-License-Identifier: MIT OR Apache-2.0

import { useState } from "react";
import { errorMessage } from "../api";

export function Onboarding({ onCreate }: { onCreate: (name: string) => Promise<void> }) {
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    try {
      await onCreate(name);
    } catch (err) {
      setError(errorMessage(err));
    }
  };

  return (
    <main className="onboarding">
      <img src="/afinin.svg" alt="" width={72} height={72} />
      <h1>Welcome to Afinín</h1>
      <p className="muted">
        Tune your AWS spend. Afinín connects <strong>read-only</strong> to your accounts and keeps
        everything on this machine.
      </p>
      <form onSubmit={submit} className="stack">
        <label>
          Name your first workspace
          <input
            autoFocus
            placeholder="e.g. Company A, Personal"
            value={name}
            onChange={(e) => setName(e.target.value)}
          />
        </label>
        {error && <p className="error">{error}</p>}
        <button className="primary" disabled={!name.trim()}>
          Create workspace
        </button>
      </form>
    </main>
  );
}
