#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# NFR-7: every source file carries the `MIT OR Apache-2.0` SPDX header.
set -euo pipefail

cd "$(dirname "$0")/.."

missing=0
while IFS= read -r file; do
  [[ -f "$file" ]] || continue
  if ! head -n 3 "$file" | grep -q "SPDX-License-Identifier: MIT OR Apache-2.0"; then
    echo "missing SPDX header: $file"
    missing=1
  fi
done < <(git ls-files '*.rs' '*.ts' '*.tsx' '*.js' '*.css' '*.sh' '*.py' '*.tf')

[[ $missing -eq 0 ]] && echo "SPDX check passed"
exit $missing
