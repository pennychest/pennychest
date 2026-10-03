import { useState, useEffect } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { useNavigate, useParams } from "@tanstack/react-router";
import {
  importsApi,
  accountsApi,
  transactionsApi,
  rulesApi,
  lookupsApi,
  aiApi,
  type ImportTransaction,
  type RuleSuggestion,
} from "../api/client";
import { Button } from "../components/ui/button";
import { Card, CardContent } from "../components/ui/card";
import { Badge } from "../components/ui/badge";
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
import { Input } from "../components/ui/input";
import { ReviewTransactionRow } from "../components/ReviewTransactionRow";
import { CategoryPicker } from "../components/CategoryPicker";
import { formatCurrency, formatDate, formatAccountPath } from "../lib/utils";
import {
  Check,
  CheckCheck,
  ArrowLeft,
  ArrowLeftRight,
  RefreshCw,
  Bot,
  Lightbulb,
} from "lucide-react";

export function ImportReviewPage() {
  const { batchId } = useParams({ from: "/imports/$batchId" });
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const [showRuleDialog, setShowRuleDialog] = useState(false);
  const [showSuggestRulesDialog, setShowSuggestRulesDialog] = useState(false);
  const [selectedTxn, setSelectedTxn] = useState<ImportTransaction | null>(null);
  const [categoriseTarget, setCategoriseTarget] = useState<ImportTransaction | null>(null);
  const [transferTarget, setTransferTarget] = useState<ImportTransaction | null>(null);
  const [aiElapsed, setAiElapsed] = useState(0);

  const { data: review, isLoading } = useQuery({
    queryKey: ["import-review", batchId],
    queryFn: () => importsApi.getBatch(parseInt(batchId)),
  });

  const { data: aiConfig } = useQuery({
    queryKey: ["ai-config"],
    queryFn: aiApi.config,
  });

  const { data: accounts = [] } = useQuery({
    queryKey: ["accounts"],
    queryFn: accountsApi.list,
  });

  const { data: matchTypes = [] } = useQuery({
    queryKey: ["match-types"],
    queryFn: lookupsApi.matchTypes,
  });

  const confirmMutation = useMutation({
    mutationFn: transactionsApi.confirm,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["import-review", batchId] });
      queryClient.invalidateQueries({ queryKey: ["import-batches"] });
    },
  });

  const confirmAllMutation = useMutation({
    mutationFn: () => importsApi.confirmAll(parseInt(batchId)),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["import-review", batchId] });
      queryClient.invalidateQueries({ queryKey: ["import-batches"] });
      queryClient.invalidateQueries({ queryKey: ["transactions"] });
    },
  });

  const reapplyMutation = useMutation({
    mutationFn: () => importsApi.reapplyRules(parseInt(batchId)),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["import-review", batchId] });
    },
  });

  const aiCategoriseMutation = useMutation({
    mutationFn: () => aiApi.categoriseBatch(parseInt(batchId)),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["import-review", batchId] });
    },
  });

  useEffect(() => {
    if (!aiCategoriseMutation.isPending) { setAiElapsed(0); return; }
    const t = setInterval(() => setAiElapsed((s) => s + 1), 1000);
    return () => clearInterval(t);
  }, [aiCategoriseMutation.isPending]);

  const categoriseMutation = useMutation({
    mutationFn: ({
      transactionId,
      targetAccountId,
    }: {
      transactionId: number;
      targetAccountId: number;
    }) => importsApi.categorise(transactionId, targetAccountId),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["import-review", batchId] });
      setCategoriseTarget(null);
    },
  });

  const markTransferMutation = useMutation({
    mutationFn: ({ transactionId, peerTransactionId }: { transactionId: number; peerTransactionId: number | null }) =>
      importsApi.markAsTransfer(transactionId, peerTransactionId),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["import-review", batchId] });
      queryClient.invalidateQueries({ queryKey: ["pending-transfers-balance"] });
      setTransferTarget(null);
    },
  });

  if (isLoading) {
    return <div className="text-center py-12 text-muted-foreground">Loading import...</div>;
  }

  if (!review) {
    return (
      <div className="text-center py-12">
        <p className="text-muted-foreground">Import batch not found.</p>
        <Button variant="outline" className="mt-4" onClick={() => navigate({ to: "/imports" })}>
          Back to Imports
        </Button>
      </div>
    );
  }

  const { batch, transactions } = review;
  const pendingCount = transactions.filter((t) => t.status === "pending").length;
  const uncategorisedCount = transactions.filter(
    (t) => t.categorised_by === "import_default"
  ).length;
  const categorisedCount = transactions.filter(
    (t) => t.categorised_by !== "import_default" && t.categorised_by !== "transfer"
  ).length;
  const categoriseTask = aiConfig?.tasks.categorise;
  const rulesTask = aiConfig?.tasks.rules;

  return (
    <div className="space-y-4">
      {/* Header */}
      <div className="flex items-center gap-3">
        <Button variant="ghost" size="icon" onClick={() => navigate({ to: "/imports" })}>
          <ArrowLeft className="h-4 w-4" />
        </Button>
        <div className="flex-1">
          <h1 className="text-2xl font-bold">
            Review Import: {batch.file_name || `Batch #${batch.id}`}
          </h1>
          <p className="text-sm text-muted-foreground">
            {formatDate(batch.imported_at)} &middot; {batch.transaction_count} transactions
          </p>
        </div>
      </div>

      {/* Summary bar */}
      <div className="flex gap-3 flex-wrap">
        <Badge variant="secondary" className="text-sm py-1 px-3">
          {categorisedCount} categorised
        </Badge>
        {uncategorisedCount > 0 && (
          <Badge variant="outline" className="text-sm py-1 px-3 border-amber-300 text-amber-700">
            {uncategorisedCount} uncategorised
          </Badge>
        )}
        {pendingCount > 0 && (
          <Badge variant="outline" className="text-sm py-1 px-3">
            {pendingCount} pending
          </Badge>
        )}
      </div>

      {/* Action bar */}
      <div className="flex gap-2 flex-wrap">
        {pendingCount > 0 && (
          <Button
            onClick={() => confirmAllMutation.mutate()}
            disabled={confirmAllMutation.isPending}
          >
            <CheckCheck className="h-4 w-4 mr-1" />
            {confirmAllMutation.isPending
              ? "Confirming..."
              : `Confirm All (${pendingCount})`}
          </Button>
        )}
        <Button
          variant="outline"
          onClick={() => reapplyMutation.mutate()}
          disabled={reapplyMutation.isPending}
        >
          <RefreshCw className="h-4 w-4 mr-1" />
          {reapplyMutation.isPending ? "Applying..." : "Re-apply Rules"}
        </Button>
        {uncategorisedCount > 0 && (
          <Button
            variant="outline"
            onClick={() => aiCategoriseMutation.mutate()}
            disabled={aiCategoriseMutation.isPending || !categoriseTask?.ready}
            title={categoriseTask?.problem ?? undefined}
          >
            <Bot className="h-4 w-4 mr-1" />
            {aiCategoriseMutation.isPending
              ? `Categorising ${uncategorisedCount} transactions… ${aiElapsed}s`
              : `Suggest with AI (${uncategorisedCount})`}
          </Button>
        )}
        <Button
          variant="outline"
          onClick={() => setShowSuggestRulesDialog(true)}
          disabled={!rulesTask?.ready}
          title={rulesTask?.problem ?? undefined}
        >
          <Lightbulb className="h-4 w-4 mr-1" />
          Suggest Rules
        </Button>
        {aiCategoriseMutation.isPending && categoriseTask?.provider === "ollama" && (
          <span className="text-xs text-muted-foreground self-center">
            Local models can take a minute or two — hang tight.
          </span>
        )}
        {(reapplyMutation.isSuccess || aiCategoriseMutation.isSuccess) && (
          <span className="text-sm text-green-600 self-center">
            {aiCategoriseMutation.isSuccess
              ? `AI categorised ${aiCategoriseMutation.data.updated} transactions`
              : `Updated ${reapplyMutation.data?.updated} transactions`}
          </span>
        )}
        {aiCategoriseMutation.isError && (
          <span className="text-sm text-destructive self-center">
            {aiCategoriseMutation.error.message}
          </span>
        )}
      </div>

      {/* Transaction list */}
      {transactions.length === 0 ? (
        <Card>
          <CardContent className="py-8 text-center text-muted-foreground">
            No transactions in this import.
          </CardContent>
        </Card>
      ) : (
        <div className="space-y-1.5">
          {transactions.map((txn) => (
            <ReviewTransactionRow
              key={txn.id}
              transaction={txn}
              onConfirm={() => confirmMutation.mutate(txn.id)}
              onCategorise={() => setCategoriseTarget(txn)}
              onCreateRule={() => {
                setSelectedTxn(txn);
                setShowRuleDialog(true);
              }}
              onMarkTransfer={() => setTransferTarget(txn)}
            />
          ))}
        </div>
      )}

      {/* Transfer dialog */}
      {transferTarget && (
        <TransferDialog
          transaction={transferTarget}
          open={true}
          onOpenChange={(open) => { if (!open) setTransferTarget(null); }}
          onMark={(peerTransactionId) =>
            markTransferMutation.mutate({ transactionId: transferTarget.id, peerTransactionId })
          }
          isPending={markTransferMutation.isPending}
        />
      )}

      {/* Categorise dialog */}
      {categoriseTarget && (
        <CategoryPicker
          open={true}
          onOpenChange={(open) => { if (!open) setCategoriseTarget(null); }}
          description={`${categoriseTarget.description} — ${formatCurrency(Math.abs(parseFloat(categoriseTarget.amount)))}`}
          accounts={accounts}
          moneyIn={parseFloat(categoriseTarget.amount) > 0}
          currentAccountId={categoriseTarget.categorised_by === "import_default" ? null : accounts.find((a) => a.full_path === categoriseTarget.target_account_full_path)?.id}
          onPick={(targetAccountId) =>
            categoriseMutation.mutate({
              transactionId: categoriseTarget.id,
              targetAccountId,
            })
          }
          isPending={categoriseMutation.isPending}
        />
      )}

      {/* Suggest Rules dialog */}
      {showSuggestRulesDialog && (
        <SuggestRulesDialog
          open={true}
          onOpenChange={(open) => { if (!open) setShowSuggestRulesDialog(false); }}
        />
      )}

      {/* Create Rule dialog */}
      {showRuleDialog && selectedTxn && (
        <CreateRuleFromImportDialog
          transaction={selectedTxn}
          accounts={accounts}
          matchTypes={matchTypes}
          open={true}
          onOpenChange={(open) => {
            if (!open) {
              setShowRuleDialog(false);
              setSelectedTxn(null);
            }
          }}
          onRuleCreated={() => {
            setShowRuleDialog(false);
            setSelectedTxn(null);
            // Re-apply rules after creating a new one
            reapplyMutation.mutate();
          }}
        />
      )}
    </div>
  );
}

function TransferDialog({
  transaction,
  open,
  onOpenChange,
  onMark,
  isPending,
}: {
  transaction: ImportTransaction;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onMark: (peerTransactionId: number | null) => void;
  isPending: boolean;
}) {
  const suggestionsQuery = useQuery({
    queryKey: ["transfer-suggestions", transaction.id],
    queryFn: () => importsApi.getTransferSuggestions(transaction.id),
    enabled: open,
    staleTime: 0,
  });

  const suggestions = suggestionsQuery.data ?? [];
  const amount = parseFloat(transaction.amount);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-lg">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <ArrowLeftRight className="h-4 w-4" />
            Mark as Transfer
          </DialogTitle>
          <DialogDescription>
            {transaction.description} &mdash;{" "}
            {formatCurrency(Math.abs(amount))}{" "}
            {amount > 0 ? "in" : "out"}
          </DialogDescription>
        </DialogHeader>

        {suggestionsQuery.isLoading && (
          <p className="text-sm text-muted-foreground py-4 text-center">
            Looking for matching transactions…
          </p>
        )}

        {suggestionsQuery.isSuccess && suggestions.length > 0 && (
          <div className="space-y-2">
            <p className="text-sm font-medium">Possible matches:</p>
            {suggestions.map((s) => (
              <div
                key={s.transaction_id}
                className="flex items-center justify-between rounded-lg border p-3 gap-3"
              >
                <div className="min-w-0 space-y-0.5">
                  <p className="text-sm font-medium truncate">{s.description}</p>
                  <p className="text-xs text-muted-foreground">
                    {formatDate(s.date)}
                    {" · "}{formatAccountPath(s.account_full_path)}
                    {s.days_apart > 0 && ` · ${s.days_apart}d apart`}
                  </p>
                </div>
                <div className="flex items-center gap-2 shrink-0">
                  <span className="text-sm font-semibold tabular-nums">
                    {formatCurrency(Math.abs(parseFloat(s.amount)))}
                  </span>
                  <Button
                    size="sm"
                    onClick={() => onMark(s.transaction_id)}
                    disabled={isPending}
                  >
                    Link
                  </Button>
                </div>
              </div>
            ))}
          </div>
        )}

        {suggestionsQuery.isSuccess && suggestions.length === 0 && (
          <p className="text-sm text-muted-foreground">
            No transactions with the opposite amount found within ±3 days.
          </p>
        )}

        <div className="border-t pt-3 space-y-2">
          <p className="text-xs text-muted-foreground">
            Can't find the other side yet? Park it in <span className="font-medium">Transfers:Pending</span> — it will be auto-resolved when the other account's statement is imported.
          </p>
          <Button
            variant="outline"
            className="w-full"
            onClick={() => onMark(null)}
            disabled={isPending}
          >
            {isPending ? "Saving…" : "Park in Transfers:Pending"}
          </Button>
        </div>

        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function SuggestRulesDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const queryClient = useQueryClient();

  const suggestQuery = useQuery({
    queryKey: ["ai-suggest-rules"],
    queryFn: aiApi.suggestRules,
    staleTime: 0,
  });

  const [dismissed, setDismissed] = useState<Set<number>>(new Set());
  const [selected, setSelected] = useState<Set<number>>(new Set());

  const suggestions = suggestQuery.data?.suggestions ?? [];

  // Pre-select all valid suggestions when they load
  useEffect(() => {
    if (suggestQuery.data) {
      const valid = new Set(
        suggestQuery.data.suggestions
          .map((s, i) => (s.match_type_id && s.target_account_id ? i : -1))
          .filter((i) => i >= 0)
      );
      setSelected(valid);
      setDismissed(new Set());
    }
  }, [suggestQuery.data]);

  const createMutation = useMutation({
    mutationFn: (data: { pattern: string; match_type_id: number; target_account_id: number; priority: number; source: string; description: string | null }) =>
      rulesApi.create(data),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["rules"] });
    },
  });

  function acceptSuggestion(suggestion: RuleSuggestion, index: number) {
    if (!suggestion.match_type_id || !suggestion.target_account_id) return;
    createMutation.mutate({
      pattern: suggestion.pattern,
      match_type_id: suggestion.match_type_id,
      target_account_id: suggestion.target_account_id,
      priority: suggestion.priority,
      source: "ai",
      description: suggestion.description || null,
    });
    setDismissed((prev) => new Set(prev).add(index));
    setSelected((prev) => { const next = new Set(prev); next.delete(index); return next; });
  }

  function acceptSelected() {
    suggestions.forEach((s, i) => {
      if (selected.has(i) && !dismissed.has(i) && s.match_type_id && s.target_account_id) {
        createMutation.mutate({
          pattern: s.pattern,
          match_type_id: s.match_type_id!,
          target_account_id: s.target_account_id!,
          priority: s.priority,
          source: "ai",
          description: s.description || null,
        });
        setDismissed((prev) => new Set(prev).add(i));
      }
    });
    setSelected(new Set());
  }

  const visibleCount = suggestions.filter((_, i) => !dismissed.has(i)).length;
  const selectedVisibleCount = suggestions.filter((_, i) => selected.has(i) && !dismissed.has(i)).length;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-lg max-h-[80vh] overflow-y-auto">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <Lightbulb className="h-5 w-5 text-violet-600" />
            Suggested Rules
          </DialogTitle>
          <DialogDescription>
            Tick the rules you want to keep, then click Accept Selected.
          </DialogDescription>
        </DialogHeader>

        {suggestQuery.isLoading && (
          <p className="text-sm text-muted-foreground py-4 text-center">Asking AI for suggestions…</p>
        )}
        {suggestQuery.isError && (
          <p className="text-sm text-destructive py-4">{suggestQuery.error.message}</p>
        )}
        {suggestQuery.isSuccess && visibleCount === 0 && (
          <p className="text-sm text-muted-foreground py-4 text-center">
            No rule suggestions — all patterns are already covered.
          </p>
        )}

        <div className="space-y-3">
          {suggestions.map((s, i) => {
            if (dismissed.has(i)) return null;
            const canAccept = !!s.match_type_id && !!s.target_account_id;
            const isSelected = selected.has(i);
            return (
              <div
                key={i}
                className={`rounded-lg border p-3 space-y-1.5 transition-colors ${isSelected ? "border-violet-400 bg-violet-50/50" : "opacity-60"}`}
              >
                <div className="flex items-start gap-3">
                  <input
                    type="checkbox"
                    checked={isSelected}
                    disabled={!canAccept}
                    onChange={() => setSelected((prev) => {
                      const next = new Set(prev);
                      if (next.has(i)) next.delete(i); else next.add(i);
                      return next;
                    })}
                    className="mt-1 h-4 w-4 rounded border-gray-300 accent-violet-600 cursor-pointer"
                  />
                  <div className="flex-1 min-w-0 space-y-0.5">
                    <p className="font-mono text-sm font-medium truncate">"{s.pattern}"</p>
                    <p className="text-xs text-muted-foreground">
                      {s.match_type} · priority {s.priority} → {formatAccountPath(s.target_account_full_path)}
                    </p>
                    {s.description && (
                      <p className="text-xs text-muted-foreground italic">{s.description}</p>
                    )}
                  </div>
                  <div className="flex gap-1 shrink-0">
                    <Button
                      size="sm"
                      variant="ghost"
                      onClick={() => acceptSuggestion(s, i)}
                      disabled={!canAccept || createMutation.isPending}
                      title="Accept just this rule"
                    >
                      <Check className="h-3.5 w-3.5 mr-1" />
                      Accept
                    </Button>
                    <Button
                      size="sm"
                      variant="ghost"
                      onClick={() => {
                        setDismissed((prev) => new Set(prev).add(i));
                        setSelected((prev) => { const next = new Set(prev); next.delete(i); return next; });
                      }}
                    >
                      Dismiss
                    </Button>
                  </div>
                </div>
              </div>
            );
          })}
        </div>

        <DialogFooter className="gap-2">
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            Close
          </Button>
          {visibleCount > 0 && (
            <Button
              onClick={acceptSelected}
              disabled={selectedVisibleCount === 0 || createMutation.isPending}
            >
              <Check className="h-3.5 w-3.5 mr-1" />
              Accept Selected ({selectedVisibleCount})
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function CreateRuleFromImportDialog({
  transaction,
  accounts,
  matchTypes,
  open,
  onOpenChange,
  onRuleCreated,
}: {
  transaction: ImportTransaction;
  accounts: { id: number; full_path: string }[];
  matchTypes: { id: number; name: string }[];
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onRuleCreated: () => void;
}) {
  const queryClient = useQueryClient();

  // Pre-fill pattern by stripping numbers and normalising the description
  const suggestedPattern = transaction.description
    .replace(/\d{2,}/g, "")
    .replace(/\s{2,}/g, " ")
    .trim();

  const [pattern, setPattern] = useState(suggestedPattern || transaction.description);
  const [matchTypeId, setMatchTypeId] = useState(
    String(matchTypes.find((m) => m.name === "substring")?.id ?? matchTypes[0]?.id ?? "")
  );
  const [targetAccountId, setTargetAccountId] = useState("");
  const [priority, setPriority] = useState("10");

  const previewQuery = useQuery({
    queryKey: ["rule-preview", pattern, matchTypeId],
    queryFn: () =>
      rulesApi.preview({
        pattern,
        match_type_id: parseInt(matchTypeId),
      }),
    enabled: pattern.length >= 2 && !!matchTypeId,
  });

  const createMutation = useMutation({
    mutationFn: (data: { pattern: string; match_type_id: number; target_account_id: number; priority: number; source: string | null; description: string | null }) =>
      rulesApi.create(data),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["rules"] });
      onRuleCreated();
    },
  });

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!pattern || !matchTypeId || !targetAccountId) return;
    createMutation.mutate({
      pattern,
      match_type_id: parseInt(matchTypeId),
      target_account_id: parseInt(targetAccountId),
      priority: parseInt(priority) || 10,
      source: null,
      description: null,
    });
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-lg">
        <DialogHeader>
          <DialogTitle>Create Rule from Transaction</DialogTitle>
          <DialogDescription>
            Create a rule to auto-categorise similar transactions.
          </DialogDescription>
        </DialogHeader>
        <form onSubmit={handleSubmit} className="space-y-4">
          <div>
            <label className="text-sm font-medium">Pattern</label>
            <Input
              value={pattern}
              onChange={(e) => setPattern(e.target.value)}
              placeholder="e.g. TESCO"
            />
          </div>
          <div className="grid grid-cols-2 gap-3">
            <div>
              <label className="text-sm font-medium">Match Type</label>
              <Select value={matchTypeId} onValueChange={setMatchTypeId}>
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {matchTypes.map((m) => (
                    <SelectItem key={m.id} value={String(m.id)}>
                      {m.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            <div>
              <label className="text-sm font-medium">Priority</label>
              <Input
                type="number"
                value={priority}
                onChange={(e) => setPriority(e.target.value)}
              />
            </div>
          </div>
          <div>
            <label className="text-sm font-medium">Target Account</label>
            <Select value={targetAccountId} onValueChange={setTargetAccountId}>
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

          {/* Live preview */}
          {previewQuery.data && (
            <div className="rounded-md border p-3 space-y-2">
              <p className="text-sm font-medium">
                Live Preview: {previewQuery.data.match_count} matching transaction
                {previewQuery.data.match_count !== 1 ? "s" : ""}
              </p>
              {previewQuery.data.matching_transactions.length > 0 && (
                <div className="max-h-32 overflow-y-auto space-y-1">
                  {previewQuery.data.matching_transactions.slice(0, 10).map((t) => (
                    <p key={t.id} className="text-xs text-muted-foreground truncate">
                      {t.description}
                    </p>
                  ))}
                  {previewQuery.data.match_count > 10 && (
                    <p className="text-xs text-muted-foreground">
                      ...and {previewQuery.data.match_count - 10} more
                    </p>
                  )}
                </div>
              )}
              {previewQuery.data.conflicts.length > 0 && (
                <div className="border-t pt-2">
                  <p className="text-xs font-medium text-amber-600">
                    Conflicts with {previewQuery.data.conflicts.length} existing rule
                    {previewQuery.data.conflicts.length !== 1 ? "s" : ""}:
                  </p>
                  {previewQuery.data.conflicts.map((c) => (
                    <p key={c.rule_id} className="text-xs text-muted-foreground">
                      "{c.pattern}" ({c.match_type}, priority {c.priority})
                    </p>
                  ))}
                </div>
              )}
            </div>
          )}

          {createMutation.isError && (
            <p className="text-sm text-destructive">{createMutation.error.message}</p>
          )}

          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={createMutation.isPending}>
              {createMutation.isPending ? "Creating..." : "Create Rule"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
