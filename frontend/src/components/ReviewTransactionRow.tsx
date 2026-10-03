import type { ReactNode } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { ArrowLeftRight, BookOpen, Check, Copy, FileDown, Plus, Tag } from "lucide-react";
import { importsApi, transactionsApi, type ImportTransaction } from "../api/client";
import { cn, formatAccountPath, formatCurrency, formatDate } from "../lib/utils";
import { Badge } from "./ui/badge";
import { Button } from "./ui/button";

// Matches the unusual_charges action: from here a charge is worth a second look.
const UNUSUAL_SCORE = 0.5;

// One imported transaction awaiting review. On phones the actions sit in their own bar
// under the details so the description and category get the full width.
export function ReviewTransactionRow({
  transaction,
  onConfirm,
  onCategorise,
  onCreateRule,
  onMarkTransfer,
}: {
  transaction: ImportTransaction;
  onConfirm: () => void;
  onCategorise: () => void;
  onCreateRule: () => void;
  onMarkTransfer: () => void;
}) {
  const amount = parseFloat(transaction.amount);
  const source = transaction.categorised_by;
  const isUncategorised = source === "import_default";
  const pending = transaction.status === "pending";

  return (
    <div
      className={cn(
        "rounded-lg border transition-colors sm:flex sm:items-center sm:gap-3",
        isUncategorised ? "border-amber-200 bg-amber-50/50" : "hover:bg-accent/50",
      )}
    >
      <div className="flex-1 min-w-0 p-3 sm:pr-0 space-y-1">
        <div className="flex items-start gap-3">
          <p className="flex-1 min-w-0 font-medium leading-snug line-clamp-2 break-words" title={transaction.description}>
            {transaction.description}
          </p>
          <span
            className={cn(
              "font-semibold tabular-nums whitespace-nowrap",
              amount > 0 && "text-green-600",
            )}
          >
            {amount > 0 ? "+" : ""}
            {formatCurrency(Math.abs(amount))}
          </span>
        </div>

        <div className="flex items-center gap-x-2 gap-y-1 flex-wrap text-sm text-muted-foreground">
          <span className="whitespace-nowrap">{formatDate(transaction.date)}</span>
          <span aria-hidden>·</span>
          <button
            type="button"
            onClick={onCategorise}
            className={cn(
              "inline-flex items-center gap-1 min-w-0 max-w-full rounded hover:text-foreground",
              isUncategorised && "text-amber-600 font-medium",
            )}
            title="Change category"
          >
            <Tag className="h-3 w-3 shrink-0" />
            <span className="truncate">
              {isUncategorised ? "Uncategorised" : formatAccountPath(transaction.target_account_full_path)}
            </span>
          </button>
          {source === "learned" && (
            <Badge
              variant="outline"
              className="text-[10px] px-1.5 py-0 border-emerald-300 text-emerald-700"
              title="Categorised from your own history"
            >
              Learned
            </Badge>
          )}
          {source === "ai" && (
            <Badge variant="outline" className="text-[10px] px-1.5 py-0 border-violet-300 text-violet-700">
              AI
            </Badge>
          )}
          {source === "manual" && (
            <Badge variant="outline" className="text-[10px] px-1.5 py-0">
              manual
            </Badge>
          )}
          {source === "transfer" && (
            <Badge variant="outline" className="text-[10px] px-1.5 py-0 border-blue-300 text-blue-700">
              {transaction.transfer_peer_id ? "transfer ✓" : "transfer (pending)"}
            </Badge>
          )}
          {(transaction.unusual_score ?? 0) >= UNUSUAL_SCORE && (
            <Badge
              variant="outline"
              className="text-[10px] px-1.5 py-0 border-red-300 text-red-700"
              title="Unusual for this merchant or category"
            >
              Unusual
            </Badge>
          )}
          {transaction.rule_pattern && (
            <span className="inline-flex items-center gap-1 min-w-0">
              <BookOpen className="h-3 w-3 shrink-0" />
              <span className="font-mono text-xs truncate">{transaction.rule_pattern}</span>
            </span>
          )}
          {!pending && (
            <Badge variant="secondary" className="text-[10px] px-1.5 py-0">
              confirmed
            </Badge>
          )}
        </div>
        {transaction.duplicate_of && <DuplicateNotice transaction={transaction} />}
      </div>

      <div className="flex border-t sm:border-t-0 sm:pr-2 shrink-0">
        {transaction.import_batch_id && (
          <Action
            label="Statement"
            href={importsApi.getBatchFileUrl(transaction.import_batch_id)}
            title={transaction.source_file_name ?? "Download source statement"}
          >
            <FileDown className="h-4 w-4" />
          </Action>
        )}
        <Action label="Transfer" onClick={onMarkTransfer} title="Mark as transfer">
          <ArrowLeftRight className="h-4 w-4" />
        </Action>
        <Action label="Category" onClick={onCategorise} title="Change category">
          <Tag className="h-4 w-4" />
        </Action>
        <Action label="Rule" onClick={onCreateRule} title="Create rule">
          <Plus className="h-4 w-4" />
        </Action>
        {pending && (
          <Action label="Confirm" onClick={onConfirm} title="Confirm" primary>
            <Check className="h-4 w-4" />
          </Action>
        )}
      </div>
    </div>
  );
}

// Data that changes when a duplicate is deleted or kept.
const REVIEW_QUERIES = ["import-review", "account-review", "import-batches", "transactions"];

function DuplicateNotice({ transaction }: { transaction: ImportTransaction }) {
  const queryClient = useQueryClient();
  const original = transaction.duplicate_of!;
  const refresh = () =>
    REVIEW_QUERIES.forEach((key) => queryClient.invalidateQueries({ queryKey: [key] }));
  const remove = useMutation({ mutationFn: () => transactionsApi.delete(transaction.id), onSuccess: refresh });
  const keep = useMutation({ mutationFn: () => importsApi.markNotDuplicate(transaction.id), onSuccess: refresh });
  const busy = remove.isPending || keep.isPending;
  const error = remove.error ?? keep.error;

  return (
    <div className="mt-2 rounded-md border border-amber-300 bg-amber-50 px-3 py-2 text-sm text-amber-900">
      <p className="flex items-start gap-1.5">
        <Copy className="h-4 w-4 mt-0.5 shrink-0" />
        <span>
          Possible duplicate of <span className="font-medium">{original.description}</span> on{" "}
          {formatDate(original.date)}, {original.source}.
        </span>
      </p>
      <div className="mt-2 flex flex-wrap gap-2">
        <Button size="sm" variant="destructive" disabled={busy} onClick={() => remove.mutate()}>
          Delete this one
        </Button>
        <Button size="sm" variant="outline" disabled={busy} onClick={() => keep.mutate()}>
          Not a duplicate
        </Button>
      </div>
      {error && <p className="mt-1 text-destructive">{error.message}</p>}
    </div>
  );
}

function Action({
  label,
  title,
  onClick,
  href,
  primary,
  children,
}: {
  label: string;
  title: string;
  onClick?: () => void;
  href?: string;
  primary?: boolean;
  children: ReactNode;
}) {
  const className = cn(
    "flex-1 sm:flex-none flex flex-col items-center justify-center gap-0.5 py-2 sm:h-9 sm:w-9 sm:py-0",
    "rounded-md text-muted-foreground hover:bg-accent hover:text-foreground transition-colors",
    primary && "text-primary",
  );
  const content = (
    <>
      {children}
      <span className="text-[11px] leading-none sm:sr-only">{label}</span>
    </>
  );
  return href ? (
    <a href={href} target="_blank" rel="noopener noreferrer" className={className} title={title}>
      {content}
    </a>
  ) : (
    <button type="button" onClick={onClick} className={className} title={title}>
      {content}
    </button>
  );
}
