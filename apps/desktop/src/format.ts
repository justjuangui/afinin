// SPDX-License-Identifier: MIT OR Apache-2.0

export function money(value: number, unit = "USD", compact = false): string {
  try {
    return new Intl.NumberFormat(undefined, {
      style: "currency",
      currency: unit,
      notation: compact ? "compact" : "standard",
      maximumFractionDigits: compact ? 1 : 2,
    }).format(value);
  } catch {
    return `${value.toFixed(2)} ${unit}`;
  }
}

export function percent(value: number | null): string {
  if (value === null) return "—";
  const sign = value > 0 ? "+" : "";
  return `${sign}${value.toFixed(1)}%`;
}

export function relativeTime(iso: string | null): string {
  if (!iso) return "never";
  const secs = (Date.now() - new Date(iso).getTime()) / 1000;
  if (secs < 60) return "just now";
  const rtf = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });
  if (secs < 3600) return rtf.format(-Math.round(secs / 60), "minute");
  if (secs < 86400) return rtf.format(-Math.round(secs / 3600), "hour");
  return rtf.format(-Math.round(secs / 86400), "day");
}

/** `YYYY-MM-DD` → short label for an axis. */
export function periodLabel(period: string, granularity: "daily" | "monthly"): string {
  const d = new Date(`${period}T00:00:00Z`);
  return new Intl.DateTimeFormat(undefined, {
    timeZone: "UTC",
    ...(granularity === "monthly" ? { month: "short", year: "2-digit" } : { month: "short", day: "numeric" }),
  }).format(d);
}
