import type { Period } from "../../api/client";

export const PERIOD_LABELS: Record<Period, string> = {
  this_month: "This month",
  last_month: "Last month",
  last_3_months: "Last 3 months",
  last_12_months: "Last 12 months",
  this_year: "This year",
};

function iso(d: Date): string {
  const month = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${d.getFullYear()}-${month}-${day}`;
}

/** The first and last day of a period, as ISO dates. Periods end today unless they're over. */
export function periodRange(period: Period, today = new Date()): { start: string; end: string } {
  const y = today.getFullYear();
  const m = today.getMonth();
  switch (period) {
    case "this_month":
      return { start: iso(new Date(y, m, 1)), end: iso(today) };
    case "last_month":
      return { start: iso(new Date(y, m - 1, 1)), end: iso(new Date(y, m, 0)) };
    case "last_3_months":
      return { start: iso(new Date(y, m - 2, 1)), end: iso(today) };
    case "last_12_months":
      return { start: iso(new Date(y, m - 11, 1)), end: iso(today) };
    case "this_year":
      return { start: iso(new Date(y, 0, 1)), end: iso(today) };
  }
}

/** "2026-09" -> "Sep 26" */
export function monthLabel(month: string): string {
  const [y, m] = month.split("-").map(Number);
  return new Date(y, m - 1, 1).toLocaleDateString("en-GB", { month: "short", year: "2-digit" });
}

/** The current calendar month as "YYYY-MM". */
export function currentMonth(today = new Date()): string {
  return `${today.getFullYear()}-${String(today.getMonth() + 1).padStart(2, "0")}`;
}

/** Tooltip title for a month, flagging the one still in progress. */
export function monthTitle(month: string): string {
  return month === currentMonth() ? `${monthLabel(month)} (so far)` : monthLabel(month);
}
