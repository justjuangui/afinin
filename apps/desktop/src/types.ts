// SPDX-License-Identifier: MIT OR Apache-2.0
// Mirrors the serialized types of `afinin-core`.

export type ProfileKind =
  | "sso"
  | "assume_role"
  | "credential_process"
  | "web_identity"
  | "static_keys"
  | "unknown";

export interface SsoSettings {
  session_name: string | null;
  start_url: string;
  region: string;
  account_id: string | null;
  role_name: string | null;
}

export interface AwsProfile {
  name: string;
  kind: ProfileKind;
  region: string | null;
  sso: SsoSettings | null;
  role_arn: string | null;
  source_profile: string | null;
}

export interface Workspace {
  id: string;
  name: string;
  created_at: string;
}

export interface ConnectionTest {
  ok: boolean;
  tested_at: string;
  aws_account_id: string | null;
  arn: string | null;
  error: string | null;
}

export interface Account {
  id: string;
  workspace_id: string;
  name: string;
  profile: string;
  kind: ProfileKind;
  aws_account_id: string | null;
  last_test: ConnectionTest | null;
  created_at: string;
}

export interface SsoLoginPrompt {
  verification_uri: string;
  verification_uri_complete: string | null;
  user_code: string;
  expires_in: number;
  interval: number;
}

export type SsoLoginStatus = "pending" | "complete" | "expired";
