// SPDX-License-Identifier: MIT OR Apache-2.0
// Typed wrappers over the Tauri IPC commands defined in `src-tauri/src/lib.rs`.

import { invoke } from "@tauri-apps/api/core";
import type {
  Account,
  AwsProfile,
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
};

/** IPC errors arrive as plain strings. */
export function errorMessage(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : String(e);
}
