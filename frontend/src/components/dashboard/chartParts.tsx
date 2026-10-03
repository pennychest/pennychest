import type { ReactNode } from "react";
import { cn, formatCurrency } from "../../lib/utils";

// Categorical slots in fixed order (see --chart-* in index.css). Never cycled: callers fold
// anything past the last slot into "Other".
export const SERIES = [1, 2, 3, 4, 5, 6, 7, 8].map((n) => `var(--chart-${n})`);
export const OTHER = "var(--chart-other)";
export const GRID = "var(--chart-grid)";
export const AXIS = "var(--chart-axis)";

/** Axis ticks: £0 / £500 / £1.2k / £12k. */
export function compactMoney(value: number): string {
  const abs = Math.abs(value);
  const sign = value < 0 ? "-" : "";
  if (abs >= 1000) return `${sign}£${Number((abs / 1000).toFixed(abs >= 10000 ? 0 : 1))}k`;
  return `${sign}£${Math.round(abs)}`;
}

export const AXIS_PROPS = {
  stroke: AXIS,
  tick: { fill: AXIS, fontSize: 12 },
  tickLine: false,
  axisLine: { stroke: GRID },
} as const;

export interface TooltipRow {
  name?: string | number;
  value?: number | string | (number | string)[];
  color?: string;
  payload?: Record<string, unknown>;
}

/** One tooltip for every series at the hovered point: values lead, names follow, each keyed
 * by a short stroke of its series colour. */
export function ChartTooltip({
  active,
  payload,
  label,
  labelFormatter,
}: {
  active?: boolean;
  payload?: TooltipRow[];
  label?: string | number;
  labelFormatter?: (label: string) => string;
}) {
  if (!active || !payload?.length) return null;
  const title = labelFormatter ? labelFormatter(String(label ?? "")) : String(label ?? "");
  return (
    <div className="rounded-md border bg-card px-3 py-2 text-xs shadow-sm">
      {title && <p className="mb-1 text-muted-foreground">{title}</p>}
      {payload.map((row, i) => (
        <p key={i} className="flex items-center gap-2">
          <span className="inline-block h-0.5 w-3 rounded" style={{ background: row.color }} />
          <span className="font-semibold tabular-nums">{formatCurrency(Number(row.value))}</span>
          <span className="text-muted-foreground">{row.name}</span>
        </p>
      ))}
    </div>
  );
}

/** Legend for two or more series: a swatch shaped like the mark (bar or line) beside text in
 * ordinary ink. */
export function Legend({
  items,
  mark = "bar",
}: {
  items: { name: string; color: string }[];
  mark?: "bar" | "line";
}) {
  if (items.length < 2) return null;
  return (
    <ul className="flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground">
      {items.map((item) => (
        <li key={item.name} className="flex items-center gap-1.5">
          <span
            className={cn("inline-block", mark === "bar" ? "h-2.5 w-2.5 rounded-sm" : "h-0.5 w-3 rounded")}
            style={{ background: item.color }}
          />
          {item.name}
        </li>
      ))}
    </ul>
  );
}

/** The chart's numbers as a table, so nothing depends on hovering or telling colours apart. */
export function DataTable({
  columns,
  rows,
}: {
  columns: string[];
  rows: (string | number | ReactNode)[][];
}) {
  return (
    <div className="max-h-72 overflow-auto">
      <table className="w-full text-sm">
        <thead>
          <tr className="border-b text-left text-xs text-muted-foreground">
            {columns.map((c, i) => (
              <th key={c} className={cn("py-1.5 font-medium", i > 0 && "text-right")}>
                {c}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((row, r) => (
            <tr key={r} className="border-b last:border-0">
              {row.map((cell, i) => (
                <td key={i} className={cn("py-1.5", i > 0 && "text-right tabular-nums")}>
                  {typeof cell === "number" ? formatCurrency(cell) : cell}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
