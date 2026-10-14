// SPDX-License-Identifier: MIT OR Apache-2.0

import { useCallback, useEffect, useState } from "react";
import { api, errorMessage } from "./api";
import type { Settings, Workspace } from "./types";
import { Sidebar } from "./components/Sidebar";
import { WorkspaceView } from "./components/WorkspaceView";
import { Onboarding } from "./components/Onboarding";
import { SettingsView } from "./components/SettingsView";

export default function App() {
  const [startupError, setStartupError] = useState<string | null>(null);
  const [workspaces, setWorkspaces] = useState<Workspace[] | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [view, setView] = useState<"workspace" | "settings">("workspace");
  const [settings, setSettings] = useState<Settings | null>(null);
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(async () => {
    try {
      const [list, s] = await Promise.all([api.listWorkspaces(), api.getSettings()]);
      setSettings(s);
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
    setView("workspace");
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

  if (workspaces === null || settings === null) {
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
        selectedId={view === "workspace" ? selected.id : null}
        settingsActive={view === "settings"}
        onSelect={(id) => {
          setSelectedId(id);
          setView("workspace");
        }}
        onSettings={() => setView("settings")}
        onCreate={createWorkspace}
      />
      {view === "settings" ? (
        <SettingsView settings={settings} onSaved={setSettings} />
      ) : (
        <WorkspaceView
          key={selected.id}
          workspace={selected}
          settings={settings}
          onChanged={reload}
          onOpenSettings={() => setView("settings")}
        />
      )}
    </div>
  );
}
