// SPDX-License-Identifier: MIT OR Apache-2.0

import type { ReactNode } from "react";

/**
 * A message that asks for the user's attention. Pick the tone by what the
 * user should do, not by how important the message feels:
 *
 * - `action`  — an improvement the user can make now (add context, enable CUR).
 *               Always has a primary action; may be dismissed ("Not now").
 * - `warn`    — results may be incomplete or wrong (permissions missing, stale data).
 * - `danger`  — something failed or is blocked; the user must act to continue.
 * - `success` — something the user started has finished.
 * - `info`    — background that helps but needs no action.
 *
 * Keep the title to one line that states the point; the body explains why.
 * Use at most one `action` callout per view.
 *
 * Callouts are for block-level messages: page and dialog errors, setup steps,
 * app notices (floating in `.toasts`). Two smaller patterns stay separate:
 * - a `badge` for a short status label inside a list or table row;
 * - inline `error small` text right under the field or row it refers to.
 */
export type CalloutTone = "action" | "warn" | "danger" | "success" | "info";

const ICON: Record<CalloutTone, string> = {
  action: "✦",
  warn: "!",
  danger: "×",
  success: "✓",
  info: "i",
};

interface Props {
  tone: CalloutTone;
  title: ReactNode;
  children?: ReactNode;
  /** Buttons; the first should be the primary action. */
  actions?: ReactNode;
  /** Shows a dismiss button ("Not now" unless `dismissLabel` says otherwise). */
  onDismiss?: () => void;
  dismissLabel?: string;
  className?: string;
}

export function Callout({ tone, title, children, actions, onDismiss, dismissLabel = "Not now", className }: Props) {
  return (
    <div className={`callout ${tone}${className ? ` ${className}` : ""}`} role={tone === "danger" ? "alert" : "status"}>
      <span className="callout-icon" aria-hidden>
        {ICON[tone]}
      </span>
      <div className="callout-body">
        <strong className="callout-title">{title}</strong>
        {children && <div className="callout-text">{children}</div>}
        {(actions || onDismiss) && (
          <div className="callout-actions">
            {actions}
            {onDismiss && (
              <button type="button" className="ghost small" onClick={onDismiss}>
                {dismissLabel}
              </button>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
