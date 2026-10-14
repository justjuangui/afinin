// SPDX-License-Identifier: MIT OR Apache-2.0
// Typed wrappers over the Tauri IPC commands defined in `src-tauri/src/lib.rs`.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  Account,
  ApiUsage,
  AwsProfile,
  CostComparisonRow,
  CostMetric,
  CostOverview,
  CostQuery,
  CostSeries,
  DimensionValue,
  GroupBy,
  Settings,
  SyncReport,
  SsoLoginPrompt,
  SsoLoginStatus,
  Workspace,
} from "./types";

export const api = {
  startupError: () => invoke<string | null>("startup_error"),
  listProfiles: () => invoke<AwsProfile[]>("list_profiles"),

  listWorkspaces: () => invoke<Workspace[]>("list_workspaces"),
  createWorkspace: (name: string) => invoke<Workspace>("create_workspace", { name }),
  renameWorkspace: (id: string, name: string) => invoke<void>("rename_workspace", { id, name }),
  deleteWorkspace: (id: string) => invoke<void>("delete_workspace", { id }),

  listAccounts: (workspaceId?: string) =>
    invoke<Account[]>("list_accounts", { workspaceId: workspaceId ?? null }),
  addAccount: (workspaceId: string, profile: string, name?: string) =>
    invoke<Account>("add_account", { workspaceId, profile, name: name ?? null }),
  removeAccount: (id: string) => invoke<void>("remove_account", { id }),
  testConnection: (id: string) => invoke<Account>("test_connection", { id }),

  startSsoLogin: (profile: string) => invoke<SsoLoginPrompt>("start_sso_login", { profile }),
  pollSsoLogin: (profile: string) => invoke<SsoLoginStatus>("poll_sso_login", { profile }),
  ssoLogout: (profile: string) => invoke<void>("sso_logout", { profile }),
  openDocs: (url: string) => invoke<void>("open_docs", { url }),

  getSettings: () => invoke<Settings>("get_settings"),
  saveSettings: (settings: Settings) => invoke<void>("save_settings", { settings }),

  syncAccount: (id: string) => invoke<SyncReport>("sync_account", { id }),
  syncWorkspace: (workspaceId: string) => invoke<void>("sync_workspace", { workspaceId }),
  apiUsage: () => invoke<ApiUsage[]>("api_usage"),
  /** Fires with an account id whenever its sync starts or ends. */
  onSyncChanged: (cb: (accountId: string) => void): Promise<UnlistenFn> =>
    listen<string>("sync-changed", (e) => cb(e.payload)),

  costOverview: (workspaceId: string, metric: CostMetric, linkedAccounts: string[]) =>
    invoke<CostOverview>("cost_overview", { workspaceId, metric, linkedAccounts }),
  costSeries: (query: CostQuery) => invoke<CostSeries>("cost_series", { query }),
  compareCosts: (query: CostQuery, prevStart: string, prevEnd: string) =>
    invoke<CostComparisonRow[]>("compare_costs", { query, prevStart, prevEnd }),
  dimensionValues: (workspaceId: string, groupBy: GroupBy) =>
    invoke<DimensionValue[]>("dimension_values", { workspaceId, groupBy }),
  exportCsv: (query: CostQuery, compare: [string, string] | null, fileName: string) =>
    invoke<boolean>("export_csv", { query, compare, fileName }),
};

/** IPC errors arrive as plain strings. */
export function errorMessage(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : String(e);
}
