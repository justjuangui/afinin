// SPDX-License-Identifier: MIT OR Apache-2.0
// UTC calendar helpers matching Cost Explorer's `YYYY-MM-DD` dates.

import type { Granularity } from "./types";

const iso = (d: Date) => d.toISOString().slice(0, 10);
const parse = (s: string) => new Date(`${s}T00:00:00Z`);

export const todayUtc = () => iso(new Date());

export function addDays(s: string, n: number): string {
  const d = parse(s);
  d.setUTCDate(d.getUTCDate() + n);
  return iso(d);
}

/** First day of the month `n` months from `s`'s month. */
export function addMonths(s: string, n: number): string {
  const d = parse(s);
  return iso(new Date(Date.UTC(d.getUTCFullYear(), d.getUTCMonth() + n, 1)));
}

export const firstOfMonth = (s: string) => addMonths(s, 0);

export function daysBetween(a: string, b: string): number {
  return Math.round((parse(b).getTime() - parse(a).getTime()) / 86_400_000);
}

export type PresetId = "mtd" | "last_30" | "last_month" | "last_3_months" | "last_12_months" | "custom";

export interface DateRange {
  preset: PresetId;
  start: string; // inclusive
  end: string; // exclusive
  granularity: Granularity;
}

export const PRESETS: { id: Exclude<PresetId, "custom">; label: string }[] = [
  { id: "mtd", label: "Month to date" },
  { id: "last_30", label: "Last 30 days" },
  { id: "last_month", label: "Last month" },
  { id: "last_3_months", label: "Last 3 months" },
  { id: "last_12_months", label: "Last 12 months" },
];

/** Data runs through yesterday, so ranges end (exclusive) today. */
export function presetRange(id: Exclude<PresetId, "custom">, today = todayUtc()): DateRange {
  const month = firstOfMonth(today);
  switch (id) {
    case "mtd":
      // On the 1st there is no data for this month yet; show yesterday.
      return { preset: id, start: month < today ? month : addDays(today, -1), end: today, granularity: "daily" };
    case "last_30":
      return { preset: id, start: addDays(today, -30), end: today, granularity: "daily" };
    case "last_month":
      return { preset: id, start: addMonths(today, -1), end: month, granularity: "daily" };
    case "last_3_months":
      return { preset: id, start: addMonths(today, -2), end: today, granularity: "monthly" };
    case "last_12_months":
      return { preset: id, start: addMonths(today, -11), end: today, granularity: "monthly" };
  }
}

/** The period just before `r`, used for comparisons (FR-12). */
export function previousRange(r: DateRange): { start: string; end: string } {
  if (r.preset === "mtd") {
    // Same days of the previous month.
    const start = addMonths(r.start, -1);
    const end = addDays(start, daysBetween(r.start, r.end));
    return { start, end: end < r.start ? end : r.start };
  }
  const monthAligned = r.start === firstOfMonth(r.start) && (r.end === firstOfMonth(r.end) || r.granularity === "monthly");
  if (monthAligned) {
    const months =
      (parse(r.end).getUTCFullYear() - parse(r.start).getUTCFullYear()) * 12 +
      parse(r.end).getUTCMonth() -
      parse(r.start).getUTCMonth() +
      (r.end === firstOfMonth(r.end) ? 0 : 1);
    return { start: addMonths(r.start, -months), end: r.start };
  }
  const len = daysBetween(r.start, r.end);
  return { start: addDays(r.start, -len), end: r.start };
}
