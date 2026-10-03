import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { ChevronRight } from "lucide-react";
import { Cell, Pie, PieChart, ResponsiveContainer, Tooltip } from "recharts";
import {
  accountsApi,
  actionsApi,
  type Period,
  type SpendingSummary,
} from "../../api/client";
import { cn, formatCurrency, formatDate } from "../../lib/utils";
import { Button } from "../ui/button";
import { ChartTooltip, OTHER, SERIES, type TooltipRow } from "./chartParts";
import { periodRange } from "./periods";

const ROOT = "Expenses";
const PAGE = 50;

interface Slice {
  path: string;
  name: string;
  value: number;
  color: string;
  /** Spending recorded on the category we're looking at, not on any of its subcategories. */
  own: boolean;
  /** Has subcategories to drill into. */
  parent: boolean;
}

interface TransactionList {
  total: number;
  transactions: { id: number; date: string; description: string; amount: string }[];
}

function lastSegment(path: string): string {
  return path.split(":").pop()!.replace(/([a-z])([A-Z])/g, "$1 $2").replace(/([a-zA-Z])(\d)/g, "$1 $2");
}

/** Spending as a pie of top-level categories. Clicking a slice opens a pie of its
 * subcategories, and so on down; clicking a category with none lists its transactions. */
export function CategoryPie({ period }: { period: Period }) {
  // The categories drilled into, from the top: ["Expenses:Food", "Expenses:Food:Groceries"]
  const [trail, setTrail] = useState<string[]>([]);
  // Set when looking at a category's transactions rather than a pie
  const [listing, setListing] = useState<{ path: string; own: boolean } | null>(null);
  const { start, end } = periodRange(period);
  const current = trail[trail.length - 1] ?? ROOT;

  const goTo = (depth: number) => {
    setTrail(trail.slice(0, depth));
    setListing(null);
  };
  const crumbs = [
    { label: "All spending", onClick: () => goTo(0) },
    ...trail.map((path, i) => ({ label: lastSegment(path), onClick: () => goTo(i + 1) })),
    ...(listing && listing.path !== current
      ? [{ label: lastSegment(listing.path), onClick: () => undefined }]
      : listing?.own
        ? [{ label: "Not in a subcategory", onClick: () => undefined }]
        : []),
  ];

  return (
    <div className="space-y-3">
      <nav aria-label="Category" className="flex flex-wrap items-center gap-1 text-sm">
        {crumbs.map((crumb, i) => {
          const last = i === crumbs.length - 1;
          return (
            <span key={i} className="flex items-center gap-1">
              {i > 0 && <ChevronRight className="h-3.5 w-3.5 text-muted-foreground" />}
              {last ? (
                <span className="font-medium" aria-current="page">
                  {crumb.label}
                </span>
              ) : (
                <button type="button" onClick={crumb.onClick} className="text-muted-foreground hover:text-foreground hover:underline">
                  {crumb.label}
                </button>
              )}
            </span>
          );
        })}
      </nav>
      {listing ? (
        <Transactions path={listing.path} own={listing.own} start={start} end={end} />
      ) : (
        <Level
          current={current}
          start={start}
          end={end}
          onOpen={(slice) => {
            if (slice.parent && !slice.own) setTrail([...trail, slice.path]);
            else setListing({ path: slice.path, own: slice.own });
          }}
        />
      )}
    </div>
  );
}

function Level({
  current,
  start,
  end,
  onOpen,
}: {
  current: string;
  start: string;
  end: string;
  onOpen: (slice: Slice) => void;
}) {
  const depth = current.split(":").length + 1;
  const args = {
    start_date: start,
    end_date: end,
    group_by: "category",
    category_depth: depth,
    limit: 200,
    ...(current === ROOT ? {} : { category: current }),
  };
  const summary = useQuery({
    queryKey: ["action", "spending_summary", args],
    queryFn: () => actionsApi.call<SpendingSummary>("spending_summary", args),
  });
  const accounts = useQuery({ queryKey: ["accounts"], queryFn: accountsApi.list });

  if (!summary.data || !accounts.data) {
    return (
      <p className="py-6 text-center text-sm text-muted-foreground">
        {summary.isError ? `Couldn't load this: ${summary.error.message}` : "Loading…"}
      </p>
    );
  }

  const paths = accounts.data.map((a) => a.full_path);
  const groups = summary.data.groups
    .filter((g) => Number(g.total) > 0)
    .sort((a, b) => Number(b.total) - Number(a.total));
  const leftOut = summary.data.groups.filter((g) => Number(g.total) <= 0).length;
  // The palette has eight colours that stay distinguishable; past that, slices go grey and
  // the list beside the pie tells them apart.
  const slices: Slice[] = groups.map((g, i) => {
    const own = g.key === current;
    return {
      path: g.key,
      name: own ? `${lastSegment(g.key)} (not in a subcategory)` : lastSegment(g.key),
      value: Number(g.total),
      color: i < SERIES.length ? SERIES[i] : OTHER,
      own,
      parent: paths.some((p) => p.startsWith(`${g.key}:`)),
    };
  });

  if (!slices.length) {
    return <p className="py-6 text-center text-sm text-muted-foreground">No spending in this period.</p>;
  }
  const total = slices.reduce((sum, s) => sum + s.value, 0);

  return (
    <div className={cn("flex flex-col items-center gap-4 sm:flex-row sm:items-start", summary.isFetching && "opacity-60")}>
      <div className="h-52 w-52 shrink-0">
        <ResponsiveContainer>
          <PieChart>
            <Pie
              data={slices}
              dataKey="value"
              nameKey="name"
              outerRadius="100%"
              stroke="var(--color-card)"
              strokeWidth={slices.length > 1 ? 2 : 0}
              isAnimationActive={false}
              onClick={(_, index) => onOpen(slices[index])}
              className="cursor-pointer"
            >
              {slices.map((s) => (
                <Cell key={s.path + s.own} fill={s.color} />
              ))}
            </Pie>
            <Tooltip
              content={(props) => (
                <ChartTooltip active={props.active} payload={props.payload as unknown as TooltipRow[]} />
              )}
            />
          </PieChart>
        </ResponsiveContainer>
      </div>
      <div className="w-full min-w-0 space-y-1">
        <ul className="space-y-0.5 text-sm">
          {slices.map((s) => (
            <li key={s.path + s.own}>
              <button
                type="button"
                onClick={() => onOpen(s)}
                className="flex w-full items-center gap-2 rounded px-1 py-0.5 text-left hover:bg-accent"
                title={s.parent && !s.own ? `Open ${s.name}` : `${s.name} transactions`}
              >
                <span className="h-2.5 w-2.5 shrink-0 rounded-sm" style={{ background: s.color }} />
                <span className="flex-1 truncate">{s.name}</span>
                <span className="tabular-nums">{formatCurrency(s.value)}</span>
                <span className="w-9 text-right text-xs tabular-nums text-muted-foreground">
                  {Math.round((s.value / total) * 100)}%
                </span>
                <ChevronRight
                  className={cn("h-3.5 w-3.5 shrink-0 text-muted-foreground", !(s.parent && !s.own) && "invisible")}
                />
              </button>
            </li>
          ))}
        </ul>
        <p className="px-1 text-xs text-muted-foreground">
          {formatCurrency(total)} in total
          {Number(summary.data.unreviewed) > 0 && `, including ${formatCurrency(Number(summary.data.unreviewed))} not yet reviewed`}
          {leftOut > 0 && `. ${leftOut} with more refunds than spending left out`}.
        </p>
      </div>
    </div>
  );
}

function Transactions({ path, own, start, end }: { path: string; own: boolean; start: string; end: string }) {
  const [shown, setShown] = useState(PAGE);
  const args = {
    category: path,
    exact_category: own,
    start_date: start,
    end_date: end,
    limit: shown,
  };
  const query = useQuery({
    queryKey: ["action", "list_transactions", args],
    queryFn: () => actionsApi.call<TransactionList>("list_transactions", args),
    placeholderData: (previous) => previous,
  });
  if (!query.data) {
    return (
      <p className="py-6 text-center text-sm text-muted-foreground">
        {query.isError ? `Couldn't load this: ${query.error.message}` : "Loading…"}
      </p>
    );
  }
  const { total, transactions } = query.data;
  if (!transactions.length) {
    return <p className="py-6 text-center text-sm text-muted-foreground">No transactions in this period.</p>;
  }
  return (
    <div className="space-y-2">
      <ul className="max-h-80 divide-y overflow-y-auto text-sm">
        {transactions.map((t) => (
          <li key={t.id} className="flex items-center justify-between gap-3 py-1.5">
            <span className="min-w-0">
              <span className="block truncate">{t.description}</span>
              <span className="text-xs text-muted-foreground">{formatDate(t.date)}</span>
            </span>
            <span className="whitespace-nowrap tabular-nums">{formatCurrency(Number(t.amount))}</span>
          </li>
        ))}
      </ul>
      <div className="flex items-center justify-between gap-2 text-xs text-muted-foreground">
        <span>
          {transactions.length} of {total}
        </span>
        {transactions.length < total && (
          <Button variant="outline" size="sm" onClick={() => setShown((n) => n + PAGE)} disabled={query.isFetching}>
            Show more
          </Button>
        )}
      </div>
    </div>
  );
}
