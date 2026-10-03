import { useState } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { transactionsApi, accountsApi, importsApi, lookupsApi, type Transaction } from "../api/client";
import { CategoryPicker } from "../components/CategoryPicker";
import { AccountIcon } from "./AccountsPage";
import { Button } from "../components/ui/button";
import { Input } from "../components/ui/input";
import { Badge } from "../components/ui/badge";
import { Card, CardContent } from "../components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogFooter,
  DialogDescription,
} from "../components/ui/dialog";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "../components/ui/select";
import { cn, formatCurrency, formatDate, formatAccountPath } from "../lib/utils";
import { ArrowLeftRight, CalendarDays, ChevronLeft, ChevronRight, ChevronsLeft, ChevronsRight, Check, CheckCheck, CreditCard, Plus, Tag, Trash2, X } from "lucide-react";

type DatePreset = "all" | "this-month" | "last-month" | "last-3-months" | "this-year" | "custom";

const DATE_PRESETS: { value: DatePreset; label: string }[] = [
  { value: "all", label: "All time" },
  { value: "this-month", label: "This month" },
  { value: "last-month", label: "Last month" },
  { value: "last-3-months", label: "Last 3 months" },
  { value: "this-year", label: "This year" },
  { value: "custom", label: "Custom…" },
];

// yyyy-mm-dd in local time, as the API expects
function isoDate(d: Date) {
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}

function presetRange(preset: DatePreset): { from?: string; to?: string } {
  const now = new Date();
  const y = now.getFullYear();
  const m = now.getMonth();
  switch (preset) {
    case "this-month":
      return { from: isoDate(new Date(y, m, 1)) };
    case "last-month":
      return { from: isoDate(new Date(y, m - 1, 1)), to: isoDate(new Date(y, m, 0)) };
    case "last-3-months":
      return { from: isoDate(new Date(y, m - 2, 1)) };
    case "this-year":
      return { from: isoDate(new Date(y, 0, 1)) };
    default:
      return {};
  }
}

const PER_PAGE = 50;

export function TransactionsPage() {
  const queryClient = useQueryClient();
  const [statusFilter, setStatusFilter] = useState<string>("");
  const [search, setSearch] = useState("");
  const [categoryId, setCategoryId] = useState<number | null>(null);
  const [pickingFilterCategory, setPickingFilterCategory] = useState(false);
  const [datePreset, setDatePreset] = useState<DatePreset>("all");
  const [customFrom, setCustomFrom] = useState("");
  const [customTo, setCustomTo] = useState("");
  const [page, setPage] = useState(1);
  const [showCreateDialog, setShowCreateDialog] = useState(false);
  const [selectedIds, setSelectedIds] = useState<Set<number>>(new Set());
  const [categorising, setCategorising] = useState<Transaction | null>(null);
  const [deleting, setDeleting] = useState<Transaction | null>(null);

  const range = datePreset === "custom" ? { from: customFrom || undefined, to: customTo || undefined } : presetRange(datePreset);

  const filters = {
    status: statusFilter || undefined,
    search: search || undefined,
    category_id: categoryId ?? undefined,
    date_from: range.from,
    date_to: range.to,
  };

  const { data: transactions = [], isLoading } = useQuery({
    queryKey: ["transactions", filters, page],
    queryFn: () => transactionsApi.list({ ...filters, page, per_page: PER_PAGE }),
  });

  // Under the "transactions" key so it refreshes whenever the list does
  const { data: total } = useQuery({
    queryKey: ["transactions", "count", filters],
    queryFn: () => transactionsApi.count(filters).then((r) => r.count),
  });
  const pageCount = total === undefined ? undefined : Math.max(1, Math.ceil(total / PER_PAGE));

  // Deleting the last few transactions can leave you past the end
  if (pageCount !== undefined && page > pageCount) setPage(pageCount);

  const { data: accounts = [] } = useQuery({
    queryKey: ["accounts"],
    queryFn: accountsApi.list,
  });

  const { data: catSources = [] } = useQuery({
    queryKey: ["categorisation-sources"],
    queryFn: lookupsApi.categorisationSources,
  });

  const confirmMutation = useMutation({
    mutationFn: transactionsApi.confirm,
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["transactions"] }),
  });

  const bulkConfirmMutation = useMutation({
    mutationFn: transactionsApi.bulkConfirm,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["transactions"] });
      setSelectedIds(new Set());
    },
  });

  const deleteMutation = useMutation({
    mutationFn: transactionsApi.delete,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["transactions"] });
      setDeleting(null);
    },
  });

  const categoriseMutation = useMutation({
    mutationFn: ({ id, accountId }: { id: number; accountId: number }) =>
      importsApi.categorise(id, accountId),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["transactions"] });
      setCategorising(null);
    },
  });

  const toggleSelect = (id: number) => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const manualSourceId = catSources.find((c) => c.name === "manual")?.id;
  const filterCategory = accounts.find((a) => a.id === categoryId);

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <h1 className="text-3xl font-bold">Transactions</h1>
        <Button onClick={() => setShowCreateDialog(true)} aria-label="New transaction">
          <Plus className="h-4 w-4 sm:mr-1" />
          <span className="hidden sm:inline">New Transaction</span>
        </Button>
      </div>

      {/* On phones the filters stay on one line and scroll sideways; from sm up
          both groups flow into a single wrapping row via `sm:contents`. */}
      <div className="flex flex-col gap-2 sm:flex-row sm:flex-wrap sm:items-center">
        <div className="-mx-4 flex items-center gap-2 overflow-x-auto px-4 py-0.5 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden [&>*]:shrink-0 sm:contents">
          <Input
            placeholder="Search descriptions..."
            value={search}
            onChange={(e) => { setSearch(e.target.value); setPage(1); }}
            className="min-w-[9rem] !shrink sm:min-w-0 sm:flex-1 sm:max-w-xs"
          />
          <Select value={statusFilter} onValueChange={(v) => { setStatusFilter(v === "all" ? "" : v); setPage(1); }}>
            <SelectTrigger className="w-[130px]">
              <SelectValue placeholder="All statuses" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="all">All statuses</SelectItem>
              <SelectItem value="pending">Pending</SelectItem>
              <SelectItem value="confirmed">Confirmed</SelectItem>
            </SelectContent>
          </Select>
          <div className="flex items-center rounded-md border border-input shadow-sm h-9 text-sm">
            <button
              type="button"
              onClick={() => setPickingFilterCategory(true)}
              className="flex h-full items-center gap-2 px-3 rounded-md hover:bg-accent"
            >
              {filterCategory ? (
                <>
                  <AccountIcon account={filterCategory} accounts={accounts} className="h-4 w-4 text-primary shrink-0" />
                  <span className="truncate max-w-[10rem]">{formatAccountPath(filterCategory.full_path)}</span>
                </>
              ) : (
                <>
                  <Tag className="h-4 w-4 text-muted-foreground" />
                  <span className="text-muted-foreground">Any category</span>
                </>
              )}
            </button>
            {filterCategory && (
              <button
                type="button"
                onClick={() => { setCategoryId(null); setPage(1); }}
                className="flex h-full items-center px-2 border-l hover:bg-accent rounded-r-md"
                aria-label="Clear category filter"
              >
                <X className="h-3.5 w-3.5" />
              </button>
            )}
          </div>
          <Select value={datePreset} onValueChange={(v) => { setDatePreset(v as DatePreset); setPage(1); }}>
            <SelectTrigger className="w-[160px] justify-start [&>span]:flex-1 [&>span]:text-left">
              <CalendarDays className="h-4 w-4 text-muted-foreground shrink-0" />
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {DATE_PRESETS.map((p) => (
                <SelectItem key={p.value} value={p.value}>{p.label}</SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        {(datePreset === "custom" || selectedIds.size > 0) && (
          <div className="flex flex-wrap items-center gap-2 sm:contents">
            {datePreset === "custom" && (
              <div className="flex items-center gap-1.5 text-sm">
                <Input
                  type="date"
                  value={customFrom}
                  max={customTo || undefined}
                  onChange={(e) => { setCustomFrom(e.target.value); setPage(1); }}
                  className="w-[150px]"
                  aria-label="From date"
                />
                <span className="text-muted-foreground">to</span>
                <Input
                  type="date"
                  value={customTo}
                  min={customFrom || undefined}
                  onChange={(e) => { setCustomTo(e.target.value); setPage(1); }}
                  className="w-[150px]"
                  aria-label="To date"
                />
              </div>
            )}
            {selectedIds.size > 0 && (
              <Button
                variant="outline"
                size="sm"
                onClick={() => bulkConfirmMutation.mutate([...selectedIds])}
              >
                <CheckCheck className="h-4 w-4 mr-1" />
                Confirm {selectedIds.size} selected
              </Button>
            )}
          </div>
        )}
      </div>

      {isLoading ? (
        <div className="text-center py-12 text-muted-foreground">Loading...</div>
      ) : transactions.length === 0 ? (
        <Card>
          <CardContent className="py-12 text-center text-muted-foreground">
            {search || statusFilter || categoryId || range.from || range.to
              ? "No transactions match these filters."
              : "No transactions found. Create your first transaction or import a bank statement."}
          </CardContent>
        </Card>
      ) : (
        <div className="space-y-2">
          {transactions.map((txn) => (
            <TransactionRow
              key={txn.id}
              transaction={txn}
              selected={selectedIds.has(txn.id)}
              onToggleSelect={() => toggleSelect(txn.id)}
              onConfirm={() => confirmMutation.mutate(txn.id)}
              onDelete={() => setDeleting(txn)}
              onCategorise={() => setCategorising(txn)}
            />
          ))}
        </div>
      )}

      {(pageCount ?? 1) > 1 && (
        <div className="flex justify-center items-center gap-1">
          <Button variant="outline" size="icon" className="h-8 w-8" disabled={page <= 1} onClick={() => setPage(1)} aria-label="First page">
            <ChevronsLeft className="h-4 w-4" />
          </Button>
          <Button variant="outline" size="sm" disabled={page <= 1} onClick={() => setPage((p) => p - 1)}>
            <ChevronLeft className="h-4 w-4 sm:mr-1" />
            <span className="hidden sm:inline">Previous</span>
          </Button>
          <span className="px-2 text-sm text-muted-foreground tabular-nums">
            Page {page} of {pageCount}
          </span>
          <Button variant="outline" size="sm" disabled={page >= (pageCount ?? 1)} onClick={() => setPage((p) => p + 1)}>
            <span className="hidden sm:inline">Next</span>
            <ChevronRight className="h-4 w-4 sm:ml-1" />
          </Button>
          <Button
            variant="outline"
            size="icon"
            className="h-8 w-8"
            disabled={page >= (pageCount ?? 1)}
            onClick={() => setPage(pageCount ?? 1)}
            aria-label="Last page"
          >
            <ChevronsRight className="h-4 w-4" />
          </Button>
        </div>
      )}
      {total !== undefined && total > 0 && (
        <p className="text-center text-xs text-muted-foreground -mt-2">
          {total.toLocaleString("en-GB")} transaction{total === 1 ? "" : "s"}
        </p>
      )}

      {pickingFilterCategory && (
        <CategoryPicker
          open={true}
          onOpenChange={setPickingFilterCategory}
          title="Filter by category"
          accounts={accounts}
          moneyIn={filterCategory?.type === "income"}
          currentAccountId={categoryId}
          showUncategorised
          parentLabel={(name) => `All of ${name}`}
          onPick={(id) => {
            setCategoryId(id);
            setPage(1);
            setPickingFilterCategory(false);
          }}
        />
      )}

      {categorising && (
        <CategoryPicker
          open={true}
          onOpenChange={(open) => { if (!open) setCategorising(null); }}
          description={`${categorising.description} — ${formatCurrency(summarise(categorising).amount)}`}
          accounts={accounts}
          moneyIn={summarise(categorising).moneyIn}
          currentAccountId={(() => {
            const s = summarise(categorising);
            return s.kind === "category" && !s.uncategorised ? s.category.account_id : null;
          })()}
          onPick={(accountId) => categoriseMutation.mutate({ id: categorising.id, accountId })}
          isPending={categoriseMutation.isPending}
        />
      )}

      <Dialog open={deleting !== null} onOpenChange={(open) => { if (!open) setDeleting(null); }}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Delete this transaction?</DialogTitle>
            <DialogDescription>
              {deleting?.description} — {deleting ? formatCurrency(summarise(deleting).amount) : ""} on{" "}
              {deleting ? formatDate(deleting.date) : ""}. This can't be undone.
            </DialogDescription>
          </DialogHeader>
          {deleteMutation.error && <p className="text-sm text-destructive">{deleteMutation.error.message}</p>}
          <DialogFooter>
            <Button variant="outline" onClick={() => setDeleting(null)}>
              Cancel
            </Button>
            <Button
              variant="destructive"
              onClick={() => deleting && deleteMutation.mutate(deleting.id)}
              disabled={deleteMutation.isPending}
            >
              Delete
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <CreateTransactionDialog
        open={showCreateDialog}
        onOpenChange={setShowCreateDialog}
        accounts={accounts}
        manualSourceId={manualSourceId ?? 2}
      />
    </div>
  );
}

function isCategory(path: string | null) {
  return !!path && (path.startsWith("Expenses:") || path.startsWith("Income:"));
}

// What a transaction means at a glance: its category (or that it's a transfer), the account
// it was paid from or into, and how much went out (or came in).
function summarise(transaction: Transaction) {
  const categories = transaction.postings.filter((p) => isCategory(p.account_full_path));
  const accounts = transaction.postings.filter((p) => !isCategory(p.account_full_path));
  if (categories.length === 1) {
    const category = categories[0];
    const amount = parseFloat(category.amount);
    // Income is recorded negative, and so is a refund on a spending category
    const moneyIn = amount < 0;
    return {
      kind: "category" as const,
      category,
      label: formatAccountPath(category.account_full_path),
      uncategorised: category.account_full_path === "Expenses:Uncategorised",
      account: accounts[0]?.account_full_path ?? null,
      amount: Math.abs(amount),
      moneyIn,
    };
  }
  if (categories.length === 0 && accounts.length === 2) {
    const [from, to] = [...accounts].sort((a, b) => parseFloat(a.amount) - parseFloat(b.amount));
    return {
      kind: "transfer" as const,
      label: `${lastPart(from.account_full_path)} → ${lastPart(to.account_full_path)}`,
      account: null,
      amount: Math.abs(parseFloat(to.amount)),
      moneyIn: false,
    };
  }
  const total = transaction.postings
    .map((p) => parseFloat(p.amount))
    .filter((a) => a > 0)
    .reduce((a, b) => a + b, 0);
  return { kind: "split" as const, label: `Split across ${categories.length} categories`, account: null, amount: total, moneyIn: false };
}

function lastPart(path: string | null) {
  return path ? formatAccountPath(path).split(" → ").at(-1) : null;
}

function TransactionRow({
  transaction,
  selected,
  onToggleSelect,
  onConfirm,
  onDelete,
  onCategorise,
}: {
  transaction: Transaction;
  selected: boolean;
  onToggleSelect: () => void;
  onConfirm: () => void;
  onDelete: () => void;
  onCategorise: () => void;
}) {
  const summary = summarise(transaction);
  const pending = transaction.status === "pending";
  const uncategorised = summary.kind === "category" && summary.uncategorised;

  return (
    <div
      className={cn(
        "flex items-start gap-3 rounded-lg border p-3 transition-colors",
        uncategorised ? "border-amber-200 bg-amber-50/50" : "hover:bg-accent/50",
      )}
    >
      {pending && (
        <input
          type="checkbox"
          checked={selected}
          onChange={onToggleSelect}
          className="mt-1 h-4 w-4 shrink-0 rounded border-gray-300"
          aria-label={`Select ${transaction.description}`}
        />
      )}
      <div className="flex-1 min-w-0 space-y-1">
        <div className="flex items-start gap-3">
          <p className="flex-1 min-w-0 font-medium leading-snug line-clamp-2 break-words" title={transaction.description}>
            {transaction.description}
          </p>
          <span className={cn("font-semibold tabular-nums whitespace-nowrap", summary.moneyIn && "text-green-600")}>
            {summary.moneyIn ? "+" : ""}
            {formatCurrency(summary.amount)}
          </span>
        </div>
        <div className="flex items-center gap-x-2 gap-y-1 flex-wrap text-sm text-muted-foreground">
          <span className="whitespace-nowrap">{formatDate(transaction.date)}</span>
          <span aria-hidden>·</span>
          {summary.kind === "category" ? (
            <button
              type="button"
              onClick={onCategorise}
              className={cn(
                "inline-flex items-center gap-1 min-w-0 max-w-full hover:text-foreground",
                uncategorised && "text-amber-600 font-medium",
              )}
              title="Change category"
            >
              <Tag className="h-3 w-3 shrink-0" />
              <span className="truncate">{uncategorised ? "Uncategorised" : summary.label}</span>
            </button>
          ) : (
            <span className="inline-flex items-center gap-1 min-w-0">
              <ArrowLeftRight className="h-3 w-3 shrink-0" />
              <span className="truncate">{summary.label}</span>
            </span>
          )}
          {summary.account && (
            <span className="inline-flex items-center gap-1 min-w-0">
              <CreditCard className="h-3 w-3 shrink-0" />
              <span className="truncate">{lastPart(summary.account)}</span>
            </span>
          )}
          {pending && (
            <Badge variant="outline" className="text-[10px] px-1.5 py-0">
              pending
            </Badge>
          )}
          <span className="ml-auto flex items-center gap-0.5">
            {pending && (
              <Button variant="ghost" size="icon" className="h-8 w-8" onClick={onConfirm} title="Confirm" aria-label="Confirm">
                <Check className="h-4 w-4" />
              </Button>
            )}
            <Button variant="ghost" size="icon" className="h-8 w-8" onClick={onDelete} title="Delete" aria-label="Delete">
              <Trash2 className="h-4 w-4 text-muted-foreground" />
            </Button>
          </span>
        </div>
      </div>
    </div>
  );
}

function CreateTransactionDialog({
  open,
  onOpenChange,
  accounts,
  manualSourceId,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  accounts: { id: number; full_path: string }[];
  manualSourceId: number;
}) {
  const queryClient = useQueryClient();
  const [date, setDate] = useState(new Date().toISOString().split("T")[0]);
  const [description, setDescription] = useState("");
  const [debitAccountId, setDebitAccountId] = useState("");
  const [creditAccountId, setCreditAccountId] = useState("");
  const [amount, setAmount] = useState("");

  const createMutation = useMutation({
    mutationFn: transactionsApi.create,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["transactions"] });
      onOpenChange(false);
      resetForm();
    },
  });

  function resetForm() {
    setDate(new Date().toISOString().split("T")[0]);
    setDescription("");
    setDebitAccountId("");
    setCreditAccountId("");
    setAmount("");
  }

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!debitAccountId || !creditAccountId || !amount || !description) return;

    createMutation.mutate({
      date,
      description,
      status: "confirmed",
      postings: [
        { account_id: parseInt(debitAccountId), amount, categorised_by_id: manualSourceId },
        {
          account_id: parseInt(creditAccountId),
          amount: String(-parseFloat(amount)),
          categorised_by_id: manualSourceId,
        },
      ],
    });
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>New Transaction</DialogTitle>
          <DialogDescription>Create a new double-entry transaction.</DialogDescription>
        </DialogHeader>
        <form onSubmit={handleSubmit} className="space-y-4">
          <div className="grid grid-cols-2 gap-3">
            <div>
              <label className="text-sm font-medium">Date</label>
              <Input type="date" value={date} onChange={(e) => setDate(e.target.value)} />
            </div>
            <div>
              <label className="text-sm font-medium">Amount</label>
              <Input
                type="number"
                step="0.01"
                placeholder="0.00"
                value={amount}
                onChange={(e) => setAmount(e.target.value)}
              />
            </div>
          </div>
          <div>
            <label className="text-sm font-medium">Description</label>
            <Input
              placeholder="e.g. Tesco Groceries"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
            />
          </div>
          <div>
            <label className="text-sm font-medium">Debit Account (to)</label>
            <Select value={debitAccountId} onValueChange={setDebitAccountId}>
              <SelectTrigger>
                <SelectValue placeholder="Select account..." />
              </SelectTrigger>
              <SelectContent>
                {accounts.map((a) => (
                  <SelectItem key={a.id} value={String(a.id)}>
                    {formatAccountPath(a.full_path)}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <div>
            <label className="text-sm font-medium">Credit Account (from)</label>
            <Select value={creditAccountId} onValueChange={setCreditAccountId}>
              <SelectTrigger>
                <SelectValue placeholder="Select account..." />
              </SelectTrigger>
              <SelectContent>
                {accounts.map((a) => (
                  <SelectItem key={a.id} value={String(a.id)}>
                    {formatAccountPath(a.full_path)}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          {createMutation.isError && (
            <p className="text-sm text-destructive">{createMutation.error.message}</p>
          )}
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={createMutation.isPending}>
              {createMutation.isPending ? "Creating..." : "Create"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
