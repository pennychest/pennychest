import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ChevronLeft, ChevronRight, Pencil, PiggyBank, Plus, Trash2 } from "lucide-react";
import {
  accountsApi,
  budgetsApi,
  type Account,
  type Budget,
  type BudgetPeriod,
} from "../api/client";
import { Button } from "../components/ui/button";
import { Card, CardContent } from "../components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "../components/ui/dialog";
import { Input } from "../components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "../components/ui/select";
import { cn, formatAccountPath, formatCurrency } from "../lib/utils";
import { AccountIcon } from "./AccountsPage";

function isoDate(d: Date): string {
  const month = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${d.getFullYear()}-${month}-${day}`;
}

function monthLabel(d: Date): string {
  return d.toLocaleDateString("en-GB", { month: "long", year: "numeric" });
}

export function BudgetsPage() {
  const today = new Date();
  const [month, setMonth] = useState(() => new Date(today.getFullYear(), today.getMonth(), 1));
  const isCurrentMonth =
    month.getFullYear() === today.getFullYear() && month.getMonth() === today.getMonth();
  // Report on today within the current month, otherwise on the month shown.
  const on = isoDate(isCurrentMonth ? today : month);

  const [editing, setEditing] = useState<Budget | "new" | null>(null);
  const queryClient = useQueryClient();

  const { data: budgets = [], isLoading } = useQuery({
    queryKey: ["budgets", on],
    queryFn: () => budgetsApi.list(on),
  });
  const { data: accounts = [] } = useQuery({ queryKey: ["accounts"], queryFn: accountsApi.list });

  const deleteMutation = useMutation({
    mutationFn: budgetsApi.delete,
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["budgets"] }),
  });

  const monthly = budgets.filter((b) => b.period === "monthly");
  const annual = budgets.filter((b) => b.period === "annual");
  const accountById = new Map(accounts.map((a) => [a.id, a]));

  function shiftMonth(delta: number) {
    setMonth((m) => new Date(m.getFullYear(), m.getMonth() + delta, 1));
  }

  function remove(budget: Budget) {
    if (window.confirm(`Delete the budget for ${formatAccountPath(budget.account_full_path)}?`)) {
      deleteMutation.mutate(budget.id);
    }
  }

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between gap-2">
        <h1 className="text-3xl font-bold">Budgets</h1>
        <Button onClick={() => setEditing("new")}>
          <Plus className="h-4 w-4 mr-1" />
          New budget
        </Button>
      </div>

      <div className="flex items-center gap-1">
        <Button variant="ghost" size="icon" onClick={() => shiftMonth(-1)} aria-label="Previous month">
          <ChevronLeft className="h-5 w-5" />
        </Button>
        <span className="min-w-36 text-center font-medium">{monthLabel(month)}</span>
        <Button
          variant="ghost"
          size="icon"
          onClick={() => shiftMonth(1)}
          disabled={isCurrentMonth}
          aria-label="Next month"
        >
          <ChevronRight className="h-5 w-5" />
        </Button>
        {!isCurrentMonth && (
          <Button variant="link" size="sm" onClick={() => setMonth(new Date(today.getFullYear(), today.getMonth(), 1))}>
            This month
          </Button>
        )}
      </div>

      {isLoading ? (
        <div className="text-sm text-muted-foreground">Loading…</div>
      ) : budgets.length === 0 ? (
        <Card>
          <CardContent className="py-16 flex flex-col items-center gap-3 text-center">
            <PiggyBank className="h-10 w-10 text-muted-foreground opacity-40" />
            <p className="font-medium">No budgets yet</p>
            <p className="text-sm text-muted-foreground max-w-xs">
              Set a monthly or annual limit for a spending category and track how much is left.
            </p>
            <Button onClick={() => setEditing("new")}>
              <Plus className="h-4 w-4 mr-1" />
              New budget
            </Button>
          </CardContent>
        </Card>
      ) : (
        <>
          {monthly.length > 0 && <Totals budgets={monthly} label={`Spent in ${monthLabel(month)}`} />}
          <BudgetGroup title="Monthly" budgets={monthly} accountById={accountById} accounts={accounts}
            onEdit={setEditing} onDelete={remove} />
          <BudgetGroup title={`Annual ${month.getFullYear()}`} budgets={annual} accountById={accountById}
            accounts={accounts} onEdit={setEditing} onDelete={remove} />
        </>
      )}

      {editing && (
        <BudgetDialog
          budget={editing === "new" ? null : editing}
          budgets={budgets}
          accounts={accounts}
          onClose={() => setEditing(null)}
        />
      )}
    </div>
  );
}

function Totals({ budgets, label }: { budgets: Budget[]; label: string }) {
  const amount = budgets.reduce((sum, b) => sum + parseFloat(b.amount), 0);
  const spent = budgets.reduce((sum, b) => sum + parseFloat(b.spent), 0);
  const currency = budgets[0]?.currency ?? "GBP";
  return (
    <Card>
      <CardContent className="pt-4 space-y-2">
        <div className="flex items-baseline justify-between gap-2">
          <p className="text-sm text-muted-foreground">{label}</p>
          <p className="text-sm">
            <span className="font-semibold">{formatCurrency(spent, currency)}</span>
            <span className="text-muted-foreground"> of {formatCurrency(amount, currency)}</span>
          </p>
        </div>
        <ProgressBar spent={spent} amount={amount} />
      </CardContent>
    </Card>
  );
}

function ProgressBar({ spent, amount }: { spent: number; amount: number }) {
  const ratio = amount > 0 ? spent / amount : 0;
  const width = Math.min(Math.max(ratio, 0), 1) * 100;
  return (
    <div className="h-2 w-full rounded-full bg-muted overflow-hidden" role="progressbar"
      aria-valuenow={Math.round(ratio * 100)} aria-valuemin={0} aria-valuemax={100}>
      <div
        className={cn(
          "h-full rounded-full transition-all",
          ratio > 1 ? "bg-destructive" : ratio >= 0.8 ? "bg-warning" : "bg-success"
        )}
        style={{ width: `${width}%` }}
      />
    </div>
  );
}

function BudgetGroup({
  title,
  budgets,
  accountById,
  accounts,
  onEdit,
  onDelete,
}: {
  title: string;
  budgets: Budget[];
  accountById: Map<number, Account>;
  accounts: Account[];
  onEdit: (b: Budget) => void;
  onDelete: (b: Budget) => void;
}) {
  if (budgets.length === 0) return null;
  return (
    <section className="space-y-2">
      <h2 className="text-sm font-medium text-muted-foreground">{title}</h2>
      {budgets.map((budget) => (
        <BudgetRow key={budget.id} budget={budget} account={accountById.get(budget.account_id)}
          accounts={accounts} onEdit={() => onEdit(budget)} onDelete={() => onDelete(budget)} />
      ))}
    </section>
  );
}

function BudgetRow({
  budget,
  account,
  accounts,
  onEdit,
  onDelete,
}: {
  budget: Budget;
  account: Account | undefined;
  accounts: Account[];
  onEdit: () => void;
  onDelete: () => void;
}) {
  const amount = parseFloat(budget.amount);
  const spent = parseFloat(budget.spent);
  const remaining = parseFloat(budget.remaining);
  const unreviewed = parseFloat(budget.unreviewed);
  const name = formatAccountPath(budget.account_full_path);

  return (
    <Card>
      <CardContent className="py-4 space-y-2">
        <div className="flex items-center gap-3">
          {account ? (
            <AccountIcon account={account} accounts={accounts} className="h-5 w-5 text-primary shrink-0" />
          ) : (
            <PiggyBank className="h-5 w-5 text-primary shrink-0" />
          )}
          <span className="font-medium truncate">{name}</span>
          <div className="ml-auto flex shrink-0">
            <Button variant="ghost" size="icon" onClick={onEdit} aria-label={`Edit ${name} budget`}>
              <Pencil className="h-4 w-4" />
            </Button>
            <Button variant="ghost" size="icon" onClick={onDelete} aria-label={`Delete ${name} budget`}>
              <Trash2 className="h-4 w-4" />
            </Button>
          </div>
        </div>
        <ProgressBar spent={spent} amount={amount} />
        <div className="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-1 text-sm">
          <span>
            <span className="font-semibold">{formatCurrency(spent, budget.currency)}</span>
            <span className="text-muted-foreground"> of {formatCurrency(amount, budget.currency)}</span>
          </span>
          <span className={cn(remaining < 0 ? "text-destructive font-medium" : "text-muted-foreground")}>
            {remaining < 0
              ? `${formatCurrency(-remaining, budget.currency)} over`
              : `${formatCurrency(remaining, budget.currency)} left`}
          </span>
        </div>
        {unreviewed !== 0 && (
          <p className="text-xs text-muted-foreground">
            Includes {formatCurrency(unreviewed, budget.currency)} not yet reviewed.
          </p>
        )}
      </CardContent>
    </Card>
  );
}

function BudgetDialog({
  budget,
  budgets,
  accounts,
  onClose,
}: {
  budget: Budget | null;
  budgets: Budget[];
  accounts: Account[];
  onClose: () => void;
}) {
  const queryClient = useQueryClient();
  const [accountId, setAccountId] = useState<string>(budget ? String(budget.account_id) : "");
  const [period, setPeriod] = useState<BudgetPeriod>(budget?.period ?? "monthly");
  const [amount, setAmount] = useState(budget ? String(parseFloat(budget.amount)) : "");

  const categories = accounts
    .filter((a) => a.type === "expense" && a.full_path.includes(":"))
    .filter((a) => a.full_path !== "Expenses:Uncategorised")
    .sort((a, b) => a.full_path.localeCompare(b.full_path));
  const taken = new Set(
    budgets.filter((b) => b.period === period && b.id !== budget?.id).map((b) => b.account_id),
  );

  const { data: suggestion } = useQuery({
    queryKey: ["budget-suggestion", accountId, period],
    queryFn: () => budgetsApi.suggestion(parseInt(accountId), period),
    enabled: accountId !== "",
  });

  const save = useMutation({
    mutationFn: () =>
      budget
        ? budgetsApi.update(budget.id, { amount, period })
        : budgetsApi.create(parseInt(accountId), amount, period),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["budgets"] });
      onClose();
    },
  });

  const parsedAmount = parseFloat(amount);
  const canSave = accountId !== "" && parsedAmount > 0 && !taken.has(parseInt(accountId));
  const suggested = suggestion ? parseFloat(suggestion.suggested) : 0;

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>{budget ? "Edit budget" : "New budget"}</DialogTitle>
        </DialogHeader>
        <form
          className="space-y-4"
          onSubmit={(e) => {
            e.preventDefault();
            if (canSave) save.mutate();
          }}
        >
          <div className="space-y-1">
            <label className="text-sm font-medium">Category</label>
            <Select value={accountId} onValueChange={setAccountId} disabled={budget !== null}>
              <SelectTrigger aria-label="Category">
                <SelectValue placeholder="Choose a spending category" />
              </SelectTrigger>
              <SelectContent>
                {categories.map((a) => (
                  <SelectItem key={a.id} value={String(a.id)} disabled={taken.has(a.id)}>
                    {formatAccountPath(a.full_path)}
                    {taken.has(a.id) && " (already has a budget)"}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <p className="text-xs text-muted-foreground">Includes spending in its subcategories.</p>
          </div>

          <div className="space-y-1">
            <label className="text-sm font-medium">Period</label>
            <div className="flex gap-1 rounded-lg bg-muted p-1 w-fit">
              {(["monthly", "annual"] as const).map((p) => (
                <button
                  key={p}
                  type="button"
                  onClick={() => setPeriod(p)}
                  className={cn(
                    "rounded-md px-3 py-1.5 text-sm font-medium transition-colors",
                    period === p ? "bg-background text-foreground shadow-sm" : "text-muted-foreground hover:text-foreground"
                  )}
                >
                  {p === "monthly" ? "Monthly" : "Annual"}
                </button>
              ))}
            </div>
          </div>

          <div className="space-y-1">
            <label className="text-sm font-medium" htmlFor="budget-amount">Limit</label>
            <Input
              id="budget-amount"
              type="number"
              inputMode="decimal"
              min="0.01"
              step="0.01"
              placeholder="0.00"
              value={amount}
              onChange={(e) => setAmount(e.target.value)}
            />
            {suggestion && suggested > 0 && (
              <p className="text-xs text-muted-foreground">
                {period === "monthly"
                  ? `You've spent about ${formatCurrency(suggested)} a month on this over the last 3 months.`
                  : `You've spent ${formatCurrency(suggested)} on this over the last 12 months.`}{" "}
                <button type="button" className="text-primary underline"
                  onClick={() => setAmount(suggested.toFixed(2))}>
                  Use this
                </button>
              </p>
            )}
            {suggestion && suggested <= 0 && (
              <p className="text-xs text-muted-foreground">No recent spending in this category.</p>
            )}
          </div>

          {accountId !== "" && taken.has(parseInt(accountId)) && (
            <p className="text-xs text-destructive">This category already has a {period} budget.</p>
          )}
          {save.error && <p className="text-xs text-destructive">{save.error.message}</p>}

          <DialogFooter>
            <Button type="button" variant="outline" onClick={onClose}>Cancel</Button>
            <Button type="submit" disabled={!canSave || save.isPending}>
              {budget ? "Save" : "Create budget"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
