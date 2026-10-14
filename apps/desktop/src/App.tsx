// SPDX-License-Identifier: MIT OR Apache-2.0

import { useCallback, useEffect, useState } from "react";
import { api, errorMessage } from "./api";
import type { Workspace } from "./types";
import { Sidebar } from "./components/Sidebar";
import { WorkspaceView } from "./components/WorkspaceView";
import { Onboarding } from "./components/Onboarding";

export default function App() {
  const [startupError, setStartupError] = useState<string | null>(null);
  const [workspaces, setWorkspaces] = useState<Workspace[] | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(async () => {
    try {
      const list = await api.listWorkspaces();
      setWorkspaces(list);
      setSelectedId((cur) => (cur && list.some((w) => w.id === cur) ? cur : (list[0]?.id ?? null)));
    } catch (e) {
      setError(errorMessage(e));
    }
  }, []);

  useEffect(() => {
    api.startupError().then((err) => {
      if (err) setStartupError(err);
      else void reload();
    });
  }, [reload]);

  const createWorkspace = async (name: string) => {
    const ws = await api.createWorkspace(name);
    await reload();
    setSelectedId(ws.id);
  };

  if (startupError) {
    return (
      <main className="fatal">
        <h1>Afinín could not open its local data</h1>
        <p className="error">{startupError}</p>
        <p className="muted">
          Afinín stores its database key in the OS keychain. On Linux, make sure a Secret
          Service provider (GNOME Keyring or KWallet) is running and unlocked.
        </p>
      </main>
    );
  }

  if (workspaces === null) {
    return <main className="loading">{error ?? "Loading…"}</main>;
  }

  if (workspaces.length === 0) {
    return <Onboarding onCreate={createWorkspace} />;
  }

  const selected = workspaces.find((w) => w.id === selectedId) ?? workspaces[0];

  return (
    <div className="layout">
      <Sidebar
        workspaces={workspaces}
        selectedId={selected.id}
        onSelect={setSelectedId}
        onCreate={createWorkspace}
      />
      <WorkspaceView key={selected.id} workspace={selected} onChanged={reload} />
    </div>
  );
}
