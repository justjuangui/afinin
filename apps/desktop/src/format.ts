// SPDX-License-Identifier: MIT OR Apache-2.0

/**
 * Formatting follows the app's language, not the operating system's: an
 * English UI with "anteayer", "1 de oct" or "1,2 mil" in it reads as a bug.
 */
export const LOCALE = "en-US";

export function money(value: number, unit = "USD", compact = false): string {
  try {
    return new Intl.NumberFormat(LOCALE, {
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
  const rounded = value.toFixed(1);
  // Avoid "-0.0%" / "+0.0%" for changes that round to nothing.
  if (Number(rounded) === 0) return "0.0%";
  return `${value > 0 ? "+" : ""}${rounded}%`;
}

export function relativeTime(iso: string | null): string {
  if (!iso) return "never";
  const secs = (Date.now() - new Date(iso).getTime()) / 1000;
  if (secs < 60) return "just now";
  const rtf = new Intl.RelativeTimeFormat(LOCALE, { numeric: "auto" });
  if (secs < 3600) return rtf.format(-Math.round(secs / 60), "minute");
  if (secs < 86400) return rtf.format(-Math.round(secs / 3600), "hour");
  return rtf.format(-Math.round(secs / 86400), "day");
}

/** `YYYY-MM-DD` → short label for an axis. */
export function periodLabel(period: string, granularity: "daily" | "monthly"): string {
  const d = new Date(`${period}T00:00:00Z`);
  return new Intl.DateTimeFormat(LOCALE, {
    timeZone: "UTC",
    ...(granularity === "monthly" ? { month: "short", year: "2-digit" } : { month: "short", day: "numeric" }),
  }).format(d);
}

/** Token counts: 950, 12.3k, 1.2M. */
export function tokens(n: number): string {
  return new Intl.NumberFormat(LOCALE, { notation: "compact", maximumFractionDigits: 1 }).format(n);
}

/** Small USD amounts keep their cents' fractions: $0.0042, $0.12, $3.40. */
export function usd(value: number): string {
  const digits = value > 0 && value < 0.01 ? 4 : value < 1 ? 3 : 2;
  return new Intl.NumberFormat(LOCALE, { style: "currency", currency: "USD", maximumFractionDigits: digits }).format(
    value,
  );
}

/** Milliseconds as 850 ms or 4.2 s. */
export function duration(ms: number): string {
  return ms < 1000 ? `${ms} ms` : `${(ms / 1000).toFixed(1)} s`;
}
