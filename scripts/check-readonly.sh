#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# NFR-1: Afinín must never call AWS APIs that mutate resources. This check fails
# if Rust sources call an SDK operation whose name starts with a write verb.
# Legitimate non-mutating exceptions (e.g. SSO OIDC token exchange) must carry a
# `// readonly-allow: <reason>` comment on the line directly above the call.
set -euo pipefail

cd "$(dirname "$0")/.."

verbs='create|delete|put|update|modify|terminate|start|stop|run|attach|detach|associate|disassociate|tag|untag|reboot|restore|copy|import|register|deregister|enable|disable|cancel|reset|replace|revoke|authorize|purchase|accept|reject|send|invoke|publish|execute|batch_write|batch_delete'
export READONLY_PATTERN="^[[:space:]]*[.](${verbs})_[a-z0-9_]*[(][)]"

status=0
while IFS= read -r file; do
  [[ -f "$file" ]] || continue  # deleted but not yet staged
  awk -v file="$file" '
    $0 ~ ENVIRON["READONLY_PATTERN"] && prev !~ /readonly-allow:/ {
      printf "%s:%d: possible AWS write call: %s\n", file, NR, $0
      found = 1
    }
    { prev = $0 }
    END { exit found }
  ' "$file" || status=1
done < <(git ls-files '*.rs')

if [[ $status -ne 0 ]]; then
  echo "error: write-like AWS calls found (see NFR-1)." >&2
else
  echo "read-only check passed"
fi
exit $status
