import { type ClassValue, clsx } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

export function formatCurrency(amount: number, currency = "GBP"): string {
  return new Intl.NumberFormat("en-GB", {
    style: "currency",
    currency,
  }).format(amount);
}

type NamedAccount = { id: number; name: string; full_path: string; parent_id: number | null; type: string };

/** A bank account or card by its name, with its group ("Alex HSBC (Credit Card)"); categories keep their path. */
export function accountLabel(account: NamedAccount, accounts: NamedAccount[]): string {
  if (account.type !== "asset" && account.type !== "liability") return formatAccountPath(account.full_path);
  const parent = accounts.find((a) => a.id === account.parent_id);
  return parent && parent.parent_id !== null ? `${account.name} (${parent.name})` : account.name;
}

const TOP_LEVEL_ACCOUNTS =new Set(["Expenses", "Income", "Assets", "Liabilities", "Equity"]);

export function formatAccountPath(path: string | null | undefined): string {
  if (!path) return "Unknown account";
  const parts = path.split(":");
  const display = TOP_LEVEL_ACCOUNTS.has(parts[0]) ? parts.slice(1) : parts;
  if (display.length === 0) return parts[0];
  return display
    .map((p) => p.replace(/([a-zA-Z])(\d)/g, "$1 $2"))
    .join(" → ");
}

export function formatDate(dateStr: string): string {
  return new Date(dateStr).toLocaleDateString("en-GB", {
    day: "numeric",
    month: "short",
    year: "numeric",
  });
}
