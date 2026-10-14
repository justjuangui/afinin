// SPDX-License-Identifier: MIT OR Apache-2.0

import { useState } from "react";
import { errorMessage } from "../api";
import type { Workspace } from "../types";

interface Props {
  workspaces: Workspace[];
  selectedId: string;
  onSelect: (id: string) => void;
  onCreate: (name: string) => Promise<void>;
}

export function Sidebar({ workspaces, selectedId, onSelect, onCreate }: Props) {
  const [adding, setAdding] = useState(false);
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    try {
      await onCreate(name);
      setName("");
      setAdding(false);
      setError(null);
    } catch (err) {
      setError(errorMessage(err));
    }
  };

  return (
    <nav className="sidebar">
      <div className="brand">
        <img src="/afinin.svg" alt="" width={28} height={28} />
        <span>Afinín</span>
      </div>
      <h2>Workspaces</h2>
      <ul>
        {workspaces.map((w) => (
          <li key={w.id}>
            <button
              className={w.id === selectedId ? "nav-item active" : "nav-item"}
              onClick={() => onSelect(w.id)}
            >
              {w.name}
            </button>
          </li>
        ))}
      </ul>
      {adding ? (
        <form onSubmit={submit} className="stack">
          <input
            autoFocus
            placeholder="Workspace name"
            value={name}
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => e.key === "Escape" && setAdding(false)}
          />
          {error && <p className="error">{error}</p>}
        </form>
      ) : (
        <button className="ghost" onClick={() => setAdding(true)}>
          + New workspace
        </button>
      )}
    </nav>
  );
}
