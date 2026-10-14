// SPDX-License-Identifier: MIT OR Apache-2.0

import type { ProfileKind } from "./types";

export const KIND_LABEL: Record<ProfileKind, string> = {
  sso: "IAM Identity Center",
  assume_role: "Assume role",
  credential_process: "Credential process",
  web_identity: "Web identity",
  static_keys: "Access keys",
  unknown: "Unknown",
};

/** Lower is better; matches the recommendation order of FR-1. */
export const KIND_RANK: Record<ProfileKind, number> = {
  sso: 0,
  assume_role: 1,
  credential_process: 2,
  web_identity: 3,
  static_keys: 4,
  unknown: 5,
};
