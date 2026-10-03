import type { ReactNode } from "react";
import { useQueries, useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { AlertTriangle, Repeat } from "lucide-react";
import {
  Area,
  AreaChart,
  Bar,
  BarChart,
  CartesianGrid,
  Cell,
  Line,
  LineChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import {
  actionsApi,
  budgetsApi,
  tapsApi,
  transactionsApi,
  importsApi,
  type CashFlow,
  type NetWorthHistory,
  type RecurringPayments,
  type SpendingSummary,
  type UnusualCharges,
  type WidgetSettings,
  type WidgetType,
} from "../../api/client";
import { Button } from "../ui/button";
import { cn, formatAccountPath, formatCurrency, formatDate } from "../../lib/utils";
import {
  AXIS_PROPS,
  ChartTooltip,
  DataTable,
  GRID,
  Legend,
  SERIES,
  compactMoney,
  type TooltipRow,
} from "./chartParts";
import { CategoryPie } from "./CategoryPie";
import { PERIOD_LABELS, currentMonth, monthLabel, monthTitle, periodRange } from "./periods";

export interface WidgetInfo {
  label: string;
  /** Stat tiles take one column; everything else two. */
  wide: boolean;
  /** Chart widgets offer a table view. */
  chart: boolean;
  defaults: WidgetSettings;
  /** Which settings the widget offers. */
  options: ("stat" | "period" | "months" | "chart" | "categories")[];
  charts?: ("bar" | "donut" | "line")[];
}

export const WIDGETS: Record<WidgetType, WidgetInfo> = {
  stat: { label: "Stat tile", wide: false, chart: false, defaults: { stat: "net_worth" }, options: ["stat"] },
  spending_by_category: {
    label: "Spending by category",
    wide: true,
    chart: false,
    defaults: { period: "this_month" },
    options: ["period"],
  },
  spending_trend: {
    label: "Monthly spending",
    wide: true,
    chart: true,
    defaults: { months: 12, chart: "bar" },
    options: ["months", "chart", "categories"],
    charts: ["bar", "line"],
  },
  income_vs_expenses: {
    label: "Income vs spending",
    wide: true,
    chart: true,
    defaults: { months: 12 },
    options: ["months"],
  },
  net_worth: { label: "Net worth over time", wide: true, chart: true, defaults: { months: 12 }, options: ["months"] },
  budgets: { label: "Budgets", wide: true, chart: false, defaults: {}, options: [] },
  top_merchants: {
    label: "Top merchants",
    wide: true,
    chart: true,
    defaults: { period: "this_month" },
    options: ["period"],
  },
  pending: { label: "Transactions to review", wide: true, chart: false, defaults: {}, options: [] },
  unmatched_taps: { label: "Unmatched card taps", wide: true, chart: false, defaults: {}, options: [] },
  subscriptions: {
    label: "Subscriptions and unusual charges",
    wide: true,
    chart: false,
    defaults: { period: "this_month" },
    options: ["period"],
  },
};

export const STAT_LABELS = {
  net_worth: "Net worth",
  income: "Income this month",
  expenses: "Spending this month",
  savings_rate: "Savings rate",
} as const;

export function widgetTitle(type: WidgetType, settings: WidgetSettings): string {
  if (type === "stat") return STAT_LABELS[settings.stat ?? "net_worth"];
  const info = WIDGETS[type];
  if (settings.period) return `${info.label} · ${PERIOD_LABELS[settings.period].toLowerCase()}`;
  if (settings.months) return `${info.label} · last ${settings.months} months`;
  return info.label;
}

// Every query is an action call keyed by its arguments, so widgets asking the same question
// share one request.
function useAction<T>(name: string, args: Record<string, unknown>) {
  return useQuery({
    queryKey: ["action", name, args],
    queryFn: () => actionsApi.call<T>(name, args),
    placeholderData: (previous) => previous,
  });
}

function monthsAgoStart(months: number): string {
  const today = new Date();
  const d = new Date(today.getFullYear(), today.getMonth() - months + 1, 1);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-01`;
}

function todayIso(): string {
  return periodRange("this_month").end;
}

function State({ children }: { children: ReactNode }) {
  return <p className="py-6 text-center text-sm text-muted-foreground">{children}</p>;
}

function Loading({ isError, error }: { isError: boolean; error: Error | null }) {
  return <State>{isError ? `Couldn't load this: ${error?.message}` : "Loading…"}</State>;
}

/** Holds the previous render, faded, while new data loads. */
function Frame({ fetching, children }: { fetching: boolean; children: ReactNode }) {
  return <div className={cn("transition-opacity", fetching && "opacity-60")}>{children}</div>;
}

const tooltip = (labelFormatter?: (label: string) => string) => (props: {
  active?: boolean;
  payload?: unknown;
  label?: string | number;
}) => (
  <ChartTooltip
    active={props.active}
    payload={props.payload as TooltipRow[]}
    label={props.label}
    labelFormatter={labelFormatter}
  />
);

// Stat tiles

export function StatWidget({ settings }: { settings: WidgetSettings }) {
  const stat = settings.stat ?? "net_worth";
  const flow = useAction<CashFlow>("cash_flow", { months: 1 });
  const worth = useAction<NetWorthHistory>("net_worth_history", { months: 1 });

  if (stat === "net_worth") {
    if (!worth.data) return <Loading isError={worth.isError} error={worth.error} />;
    const entries = Object.entries(worth.data.currencies);
    if (!entries.length) return <p className="text-2xl font-semibold text-muted-foreground">--</p>;
    return (
      <div className="space-y-0.5">
        {entries.map(([currency, points]) => (
          <p key={currency} className="text-xl font-semibold sm:text-2xl">
            {formatCurrency(Number(points[points.length - 1].net_worth), currency)}
          </p>
        ))}
      </div>
    );
  }
  if (!flow.data) return <Loading isError={flow.isError} error={flow.error} />;
  const month = flow.data.months[flow.data.months.length - 1];
  const income = Number(month.income);
  const expenses = Number(month.expenses);
  const unreviewed = Number(flow.data.unreviewed_expenses);
  if (stat === "savings_rate") {
    return (
      <div>
        <p className="text-xl font-semibold sm:text-2xl">
          {income > 0 ? `${(((income - expenses) / income) * 100).toFixed(1)}%` : "--"}
        </p>
        <p className="text-xs text-muted-foreground">of this month's income kept</p>
      </div>
    );
  }
  return (
    <div>
      <p className="text-xl font-semibold sm:text-2xl">{formatCurrency(stat === "income" ? income : expenses)}</p>
      {stat === "expenses" && unreviewed > 0 && (
        <p className="text-xs text-muted-foreground">includes {formatCurrency(unreviewed)} not yet reviewed</p>
      )}
    </div>
  );
}

/** One series, so one colour; values at the bar tips. */
function HorizontalBars({ rows, fetching }: { rows: { name: string; value: number }[]; fetching: boolean }) {
  return (
    <Frame fetching={fetching}>
      <div style={{ height: rows.length * 32 + 8 }}>
        <ResponsiveContainer>
          <BarChart data={rows} layout="vertical" margin={{ left: 0, right: 64, top: 0, bottom: 0 }}>
            <XAxis type="number" hide />
            <YAxis type="category" dataKey="name" width={140} {...AXIS_PROPS} axisLine={false} />
            <Tooltip content={tooltip()} cursor={{ fill: GRID, opacity: 0.4 }} />
            <Bar
              dataKey="value"
              name="Spent"
              fill={SERIES[0]}
              barSize={16}
              radius={[0, 4, 4, 0]}
              isAnimationActive={false}
              label={{
                position: "right",
                fill: "var(--color-muted-foreground)",
                fontSize: 12,
                formatter: (v: unknown) => compactMoney(Number(v)),
              }}
            />
          </BarChart>
        </ResponsiveContainer>
      </div>
    </Frame>
  );
}

// Monthly spending, overall or by chosen categories

export function SpendingTrendWidget({ settings, table }: { settings: WidgetSettings; table: boolean }) {
  const months = settings.months ?? 12;
  const categories = settings.categories?.length ? settings.categories : [null];
  const start = monthsAgoStart(months);
  const end = todayIso();
  const queries = useQueries({
    queries: categories.map((category) => {
      const args = {
        start_date: start,
        end_date: end,
        group_by: "month",
        limit: 60,
        ...(category ? { category } : {}),
      };
      return {
        queryKey: ["action", "spending_summary", args],
        queryFn: () => actionsApi.call<SpendingSummary>("spending_summary", args),
        placeholderData: (previous: SpendingSummary | undefined) => previous,
      };
    }),
  });
  const failed = queries.find((q) => q.isError);
  if (failed || queries.some((q) => !q.data)) {
    return <Loading isError={Boolean(failed)} error={failed?.error ?? null} />;
  }

  const monthKeys: string[] = [];
  for (let i = months - 1; i >= 0; i--) {
    const d = new Date();
    const m = new Date(d.getFullYear(), d.getMonth() - i, 1);
    monthKeys.push(`${m.getFullYear()}-${String(m.getMonth() + 1).padStart(2, "0")}`);
  }
  // Colour follows the category's place in the user's own list, never its rank.
  const series = categories.map((category, i) => ({
    key: `s${i}`,
    name: category ? formatAccountPath(category) : "Spending",
    color: SERIES[i],
  }));
  const data = monthKeys.map((month) => {
    const row: Record<string, string | number> = { month };
    queries.forEach((q, i) => {
      const group = q.data!.groups.find((g) => g.key === month);
      row[`s${i}`] = group ? Number(group.total) : 0;
    });
    return row;
  });

  if (table) {
    return (
      <DataTable
        columns={["Month", ...series.map((s) => s.name)]}
        rows={data.map((row) => [monthLabel(String(row.month)), ...series.map((s) => Number(row[s.key]))])}
      />
    );
  }

  const fetching = queries.some((q) => q.isFetching);
  const common = (
    <>
      <CartesianGrid vertical={false} stroke={GRID} />
      <XAxis dataKey="month" tickFormatter={monthLabel} {...AXIS_PROPS} />
      <YAxis tickFormatter={compactMoney} width={56} {...AXIS_PROPS} axisLine={false} />
    </>
  );
  return (
    <Frame fetching={fetching}>
      <div className="space-y-2">
        <Legend items={series} mark={settings.chart === "line" ? "line" : "bar"} />
        <div className="h-56">
          <ResponsiveContainer>
            {settings.chart === "line" ? (
              <LineChart data={data} margin={{ top: 8, right: 8, left: 0, bottom: 0 }}>
                {common}
                <Tooltip content={tooltip(monthTitle)} cursor={{ stroke: GRID }} />
                {series.map((s) => (
                  <Line
                    key={s.key}
                    dataKey={s.key}
                    name={s.name}
                    stroke={s.color}
                    strokeWidth={2}
                    dot={false}
                    activeDot={{ r: 4, stroke: "var(--color-card)", strokeWidth: 2 }}
                    isAnimationActive={false}
                  />
                ))}
              </LineChart>
            ) : (
              <BarChart data={data} margin={{ top: 8, right: 8, left: 0, bottom: 0 }} barGap={2}>
                {common}
                <Tooltip content={tooltip(monthTitle)} cursor={{ fill: GRID, opacity: 0.4 }} />
                {series.map((s) => (
                  <Bar
                    key={s.key}
                    dataKey={s.key}
                    name={s.name}
                    fill={s.color}
                    maxBarSize={24}
                    radius={[4, 4, 0, 0]}
                    isAnimationActive={false}
                  >
                    {data.map((row) => (
                      <Cell key={String(row.month)} fillOpacity={row.month === currentMonth() ? 0.45 : 1} />
                    ))}
                  </Bar>
                ))}
              </BarChart>
            )}
          </ResponsiveContainer>
        </div>
        <PartialMonthNote />
      </div>
    </Frame>
  );
}

function PartialMonthNote() {
  return (
    <p className="text-xs text-muted-foreground">{monthLabel(currentMonth())} is the month so far.</p>
  );
}

// Income vs spending

export function IncomeVsExpensesWidget({ settings, table }: { settings: WidgetSettings; table: boolean }) {
  const query = useAction<CashFlow>("cash_flow", { months: settings.months ?? 12 });
  if (!query.data) return <Loading isError={query.isError} error={query.error} />;
  const data = query.data.months.map((m) => ({
    month: m.month,
    income: Number(m.income),
    expenses: Number(m.expenses),
    net: Number(m.net),
  }));
  if (table) {
    return (
      <DataTable
        columns={["Month", "Income", "Spending", "Left over"]}
        rows={data.map((m) => [monthLabel(m.month), m.income, m.expenses, m.net])}
      />
    );
  }
  const series = [
    { key: "income", name: "Income", color: SERIES[0] },
    { key: "expenses", name: "Spending", color: SERIES[1] },
  ];
  return (
    <Frame fetching={query.isFetching}>
      <div className="space-y-2">
        <Legend items={series} />
        <div className="h-56">
          <ResponsiveContainer>
            <BarChart data={data} margin={{ top: 8, right: 8, left: 0, bottom: 0 }} barGap={2}>
              <CartesianGrid vertical={false} stroke={GRID} />
              <XAxis dataKey="month" tickFormatter={monthLabel} {...AXIS_PROPS} />
              <YAxis tickFormatter={compactMoney} width={56} {...AXIS_PROPS} axisLine={false} />
              <Tooltip content={tooltip(monthTitle)} cursor={{ fill: GRID, opacity: 0.4 }} />
              {series.map((s) => (
                <Bar
                  key={s.key}
                  dataKey={s.key}
                  name={s.name}
                  fill={s.color}
                  maxBarSize={24}
                  radius={[4, 4, 0, 0]}
                  isAnimationActive={false}
                >
                  {data.map((row) => (
                    <Cell key={row.month} fillOpacity={row.month === currentMonth() ? 0.45 : 1} />
                  ))}
                </Bar>
              ))}
            </BarChart>
          </ResponsiveContainer>
        </div>
        <PartialMonthNote />
      </div>
    </Frame>
  );
}

// Net worth over time

export function NetWorthWidget({ settings, table }: { settings: WidgetSettings; table: boolean }) {
  const query = useAction<NetWorthHistory>("net_worth_history", { months: settings.months ?? 12 });
  if (!query.data) return <Loading isError={query.isError} error={query.error} />;
  const [currency, points] = Object.entries(query.data.currencies)[0] ?? ["GBP", []];
  if (!points.length) return <State>No accounts with balances yet.</State>;
  const data = points.map((p) => ({ date: p.date, value: Number(p.net_worth) }));
  const dateLabel = (d: string) => formatDate(d);
  if (table) {
    return <DataTable columns={["Month end", "Net worth"]} rows={data.map((p) => [dateLabel(p.date), p.value])} />;
  }
  const last = data[data.length - 1];
  return (
    <Frame fetching={query.isFetching}>
      <p className="mb-2 text-sm text-muted-foreground">
        {formatCurrency(last.value, currency)} on {dateLabel(last.date)}
      </p>
      <div className="h-56">
        <ResponsiveContainer>
          <AreaChart data={data} margin={{ top: 8, right: 8, left: 0, bottom: 0 }}>
            <CartesianGrid vertical={false} stroke={GRID} />
            <XAxis dataKey="date" tickFormatter={(d: string) => monthLabel(d.slice(0, 7))} {...AXIS_PROPS} />
            <YAxis tickFormatter={compactMoney} width={56} {...AXIS_PROPS} axisLine={false} />
            <Tooltip content={tooltip(dateLabel)} cursor={{ stroke: GRID }} />
            <Area
              dataKey="value"
              name="Net worth"
              stroke={SERIES[0]}
              strokeWidth={2}
              fill={SERIES[0]}
              fillOpacity={0.1}
              activeDot={{ r: 4, stroke: "var(--color-card)", strokeWidth: 2 }}
              isAnimationActive={false}
            />
          </AreaChart>
        </ResponsiveContainer>
      </div>
    </Frame>
  );
}

// Top merchants

export function TopMerchantsWidget({ settings, table }: { settings: WidgetSettings; table: boolean }) {
  const { start, end } = periodRange(settings.period ?? "this_month");
  const query = useAction<SpendingSummary>("spending_summary", {
    start_date: start,
    end_date: end,
    group_by: "merchant",
    limit: 8,
  });
  if (!query.data) return <Loading isError={query.isError} error={query.error} />;
  const rows = query.data.groups
    .map((g) => ({ name: g.key, value: Number(g.total) }))
    .filter((g) => g.value > 0);
  if (!rows.length) return <State>No spending in this period.</State>;
  if (table) return <DataTable columns={["Merchant", "Spent"]} rows={rows.map((r) => [r.name, r.value])} />;
  return <HorizontalBars rows={rows} fetching={query.isFetching} />;
}

// Budgets

export function BudgetsWidget() {
  const query = useQuery({ queryKey: ["budgets"], queryFn: () => budgetsApi.list() });
  if (!query.data) return <Loading isError={query.isError} error={query.error} />;
  if (!query.data.length) {
    return (
      <State>
        No budgets yet. <Link to="/budgets" className="underline">Set one up</Link>
      </State>
    );
  }
  return (
    <ul className="space-y-3">
      {query.data.map((b) => {
        const amount = Number(b.amount);
        const spent = Number(b.spent);
        const share = amount > 0 ? spent / amount : 0;
        const over = spent > amount;
        return (
          <li key={b.id} className="space-y-1">
            <div className="flex items-baseline justify-between gap-2 text-sm">
              <span className="truncate">{formatAccountPath(b.account_full_path)}</span>
              <span className="whitespace-nowrap tabular-nums text-muted-foreground">
                {formatCurrency(spent, b.currency)} of {formatCurrency(amount, b.currency)}
              </span>
            </div>
            <div className="h-2 rounded-full" style={{ background: GRID }}>
              <div
                className="h-2 rounded-full"
                style={{
                  width: `${Math.min(share, 1) * 100}%`,
                  background: over ? "var(--chart-critical)" : SERIES[0],
                }}
              />
            </div>
            {over && (
              <p className="flex items-center gap-1 text-xs text-destructive">
                <AlertTriangle className="h-3 w-3" /> Over by {formatCurrency(spent - amount, b.currency)}
              </p>
            )}
          </li>
        );
      })}
    </ul>
  );
}

// Transactions to review

export function PendingWidget() {
  const count = useQuery({ queryKey: ["pending-count"], queryFn: () => transactionsApi.count({ status: "pending" }) });
  const batches = useQuery({ queryKey: ["import-batches"], queryFn: importsApi.listBatches });
  if (!count.data) return <Loading isError={count.isError} error={count.error} />;
  const pendingBatches = batches.data?.filter((b) => b.pending_count > 0) ?? [];
  if (count.data.count === 0) {
    return (
      <div className="flex flex-wrap items-center justify-between gap-3">
        <p className="text-sm text-muted-foreground">Everything's reviewed.</p>
        <Button size="sm" asChild>
          <Link to="/imports">Import a statement</Link>
        </Button>
      </div>
    );
  }
  return (
    <div className="space-y-3">
      <p className="text-sm text-muted-foreground">
        <span className="font-semibold text-foreground">{count.data.count}</span> transaction
        {count.data.count !== 1 ? "s" : ""} to review.
      </p>
      <div className="flex flex-wrap gap-2">
        <Button variant="outline" size="sm" asChild>
          <Link to="/transactions" search={{ status: "pending" }}>
            Review transactions
          </Link>
        </Button>
        {pendingBatches.length > 0 && (
          <Button variant="outline" size="sm" asChild>
            <Link to="/imports">Review imports ({pendingBatches.length})</Link>
          </Button>
        )}
      </div>
    </div>
  );
}

// Unmatched card taps

export function UnmatchedTapsWidget() {
  const query = useQuery({ queryKey: ["taps", "unmatched"], queryFn: () => tapsApi.list("unmatched") });
  if (!query.data) return <Loading isError={query.isError} error={query.error} />;
  if (!query.data.length) return <State>Every card tap is matched to a statement.</State>;
  return (
    <div className="space-y-2">
      <ul className="divide-y text-sm">
        {query.data.slice(0, 5).map((tap) => (
          <li key={tap.id} className="flex items-center justify-between gap-2 py-1.5">
            <span className="truncate">{tap.merchant}</span>
            <span className="whitespace-nowrap text-muted-foreground">
              {formatDate(tap.tapped_on)} ·{" "}
              <span className="tabular-nums text-foreground">{formatCurrency(Number(tap.amount), tap.currency)}</span>
            </span>
          </li>
        ))}
      </ul>
      <Button variant="outline" size="sm" asChild>
        <Link to="/imports/taps">
          {query.data.length > 5 ? `See all ${query.data.length}` : "Open card taps"}
        </Link>
      </Button>
    </div>
  );
}

// Subscriptions and unusual charges

export function SubscriptionsWidget({ settings }: { settings: WidgetSettings }) {
  const { start, end } = periodRange(settings.period ?? "this_month");
  const subs = useAction<RecurringPayments>("recurring_payments", { kind: "subscription", limit: 10 });
  const unusual = useAction<UnusualCharges>("unusual_charges", { start_date: start, end_date: end, limit: 5 });
  if (!subs.data || !unusual.data) {
    const failed = subs.isError ? subs : unusual;
    return <Loading isError={failed.isError} error={failed.error} />;
  }
  const flagged = unusual.data.unusual_charges;
  const rises = unusual.data.price_rises;
  return (
    <div className="grid gap-4 sm:grid-cols-2">
      <section className="space-y-1.5">
        <h3 className="flex items-center gap-1.5 text-sm font-medium">
          <Repeat className="h-3.5 w-3.5" /> Subscriptions
        </h3>
        {subs.data.payments.length ? (
          <>
            <ul className="space-y-1 text-sm">
              {subs.data.payments.map((p) => (
                <li key={p.merchant} className="flex justify-between gap-2">
                  <span className="truncate">{p.merchant}</span>
                  <span className="whitespace-nowrap tabular-nums text-muted-foreground">
                    {formatCurrency(Number(p.yearly_cost))}/yr
                  </span>
                </li>
              ))}
            </ul>
            <p className="text-xs text-muted-foreground">
              {formatCurrency(Number(subs.data.yearly_total))} a year in total
            </p>
          </>
        ) : (
          <p className="text-sm text-muted-foreground">
            None found yet. They're labelled after each import once a model is set for
            subscriptions in Settings › AI.
          </p>
        )}
      </section>
      <section className="space-y-1.5">
        <h3 className="flex items-center gap-1.5 text-sm font-medium">
          <AlertTriangle className="h-3.5 w-3.5" /> Unusual · {PERIOD_LABELS[settings.period ?? "this_month"].toLowerCase()}
        </h3>
        {flagged.length || rises.length ? (
          <ul className="space-y-1 text-sm">
            {flagged.map((c) => (
              <li key={c.transaction_id} className="flex justify-between gap-2">
                <span className="truncate">{c.description}</span>
                <span className="whitespace-nowrap tabular-nums">{formatCurrency(Number(c.amount))}</span>
              </li>
            ))}
            {rises.map((r) => (
              <li key={r.merchant} className="flex justify-between gap-2">
                <span className="truncate">{r.merchant} went up</span>
                <span className="whitespace-nowrap tabular-nums text-muted-foreground">
                  {formatCurrency(Number(r.usual_amount))} → {formatCurrency(Number(r.latest_amount))}
                </span>
              </li>
            ))}
          </ul>
        ) : (
          <p className="text-sm text-muted-foreground">
            {unusual.data.unscored_charges > 0
              ? `Nothing flagged; ${unusual.data.unscored_charges} ${
                  unusual.data.unscored_charges === 1 ? "charge hasn't" : "charges haven't"
                } been checked.`
              : "Nothing unusual."}
          </p>
        )}
      </section>
    </div>
  );
}

export function WidgetBody({ type, settings, table }: { type: WidgetType; settings: WidgetSettings; table: boolean }) {
  switch (type) {
    case "stat":
      return <StatWidget settings={settings} />;
    case "spending_by_category": {
      const period = settings.period ?? "this_month";
      // A new period starts again from the top level.
      return <CategoryPie key={period} period={period} />;
    }
    case "spending_trend":
      return <SpendingTrendWidget settings={settings} table={table} />;
    case "income_vs_expenses":
      return <IncomeVsExpensesWidget settings={settings} table={table} />;
    case "net_worth":
      return <NetWorthWidget settings={settings} table={table} />;
    case "budgets":
      return <BudgetsWidget />;
    case "top_merchants":
      return <TopMerchantsWidget settings={settings} table={table} />;
    case "pending":
      return <PendingWidget />;
    case "unmatched_taps":
      return <UnmatchedTapsWidget />;
    case "subscriptions":
      return <SubscriptionsWidget settings={settings} />;
  }
}
