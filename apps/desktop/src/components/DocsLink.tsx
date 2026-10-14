// SPDX-License-Identifier: MIT OR Apache-2.0

import type { ReactNode } from "react";
import { api } from "../api";

/** Opens an allowlisted documentation URL in the system browser. */
export function DocsLink({ href, children }: { href: string; children: ReactNode }) {
  return (
    <a
      href={href}
      onClick={(e) => {
        e.preventDefault();
        void api.openDocs(href);
      }}
    >
      {children}
    </a>
  );
}
