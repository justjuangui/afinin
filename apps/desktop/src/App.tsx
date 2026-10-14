// SPDX-License-Identifier: MIT OR Apache-2.0

import { useCallback, useEffect, useState } from "react";
import { api, errorMessage } from "./api";
import type { Settings, Workspace } from "./types";
import { Sidebar } from "./components/Sidebar";
import { WorkspaceView } from "./components/WorkspaceView";
import { Onboarding } from "./components/Onboarding";
import { SettingsView } from "./components/SettingsView";
import { applyTheme } from "./theme";
import { Callout } from "./components/Callout";

export default function App() {
  const [startupError, setStartupError] = useState<string | null>(null);
  const [workspaces, setWorkspaces] = useState<Workspace[] | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [view, setView] = useState<"workspace" | "settings">("workspace");
  const [settings, setSettings] = useState<Settings | null>(null);
  const [update, setUpdate] = useState<{ version: string } | null>(null);
  const [installing, setInstalling] = useState(false);

  // Signed updates (release builds only); a failed check is silent.
  useEffect(() => {
    api.checkUpdate().then(setUpdate, () => setUpdate(null));
  }, []);
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(async () => {
    try {
      const [list, s] = await Promise.all([api.listWorkspaces(), api.getSettings()]);
      setSettings(s);
      applyTheme(s.theme);
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
        <Callout tone="danger" title={startupError}>
          Afinín stores its database key in the OS keychain. On Linux, make sure a Secret Service provider (GNOME
          Keyring or KWallet) is running and unlocked.
        </Callout>
      </main>
    );
  }

  if (workspaces === null || settings === null) {
    return error ? (
      <main className="fatal">
        <Callout tone="danger" title="Afinín couldn’t load your workspaces">
          {error}
        </Callout>
      </main>
    ) : (
      <main className="loading">Loading…</main>
    );
  }

  if (workspaces.length === 0) {
    return <Onboarding onCreate={createWorkspace} />;
  }

  const selected = workspaces.find((w) => w.id === selectedId) ?? workspaces[0];

  // App-level messages float above the layout. Errors after startup (reload,
  // update install) used to be stored but never shown.
  const toasts = (update || error) && (
    <div className="toasts">
      {error && (
        <Callout tone="danger" title="Something went wrong" onDismiss={() => setError(null)} dismissLabel="Dismiss">
          {error}
        </Callout>
      )}
      {update && (
        <Callout
          tone="action"
          title={`Afinín ${update.version} is available`}
          actions={
            <button
              className="primary small"
              disabled={installing}
              onClick={() => {
                setInstalling(true);
                api.installUpdate().catch((e) => {
                  setError(errorMessage(e));
                  setInstalling(false);
                });
              }}
            >
              {installing ? "Installing…" : "Install and restart"}
            </button>
          }
          onDismiss={() => setUpdate(null)}
          dismissLabel="Later"
        >
          Afinín restarts to finish the update.
        </Callout>
      )}
    </div>
  );

  return (
    <div className="layout">
      {toasts}
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
        <SettingsView
          settings={settings}
          onSaved={(s) => {
            setSettings(s);
            applyTheme(s.theme);
          }}
          onRestored={reload}
        />
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
