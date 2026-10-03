import { useState } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { Nfc, CreditCard, EyeOff, RefreshCw, RotateCcw, Link2, ChevronLeft, Sparkles, GraduationCap, Trash2 } from "lucide-react";
import {
  tapsApi,
  accountsApi,
  type Account,
  type CardTap,
  type TapStatus,
  type WalletCard,
} from "../api/client";
import { Button } from "../components/ui/button";
import { Badge } from "../components/ui/badge";
import { Card, CardContent, CardHeader, CardTitle, CardDescription } from "../components/ui/card";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "../components/ui/select";
import { cn, formatAccountPath, formatCurrency, formatDate } from "../lib/utils";

const TABS: { value: TapStatus; label: string }[] = [
  { value: "unmatched", label: "Awaiting statement" },
  { value: "reconciled", label: "Reconciled" },
  { value: "dismissed", label: "Dismissed" },
];

const UNPAIRED = "none";

export function TapsPage() {
  const queryClient = useQueryClient();
  const [tab, setTab] = useState<TapStatus>("unmatched");

  const { data: taps = [], isLoading } = useQuery({
    queryKey: ["taps", tab],
    queryFn: () => tapsApi.list(tab),
  });

  const { data: cards = [] } = useQuery({
    queryKey: ["tap-cards"],
    queryFn: tapsApi.cards,
  });

  const { data: accounts = [] } = useQuery({
    queryKey: ["accounts"],
    queryFn: accountsApi.list,
  });

  const payAccounts = accounts
    .filter((a) => a.type === "asset" || a.type === "liability")
    .sort((a, b) => a.full_path.localeCompare(b.full_path));

  function invalidate() {
    queryClient.invalidateQueries({ queryKey: ["taps"] });
    queryClient.invalidateQueries({ queryKey: ["tap-cards"] });
  }

  const dismissMutation = useMutation({ mutationFn: tapsApi.dismiss, onSuccess: invalidate });
  const restoreMutation = useMutation({ mutationFn: tapsApi.restore, onSuccess: invalidate });
  const deleteMutation = useMutation({ mutationFn: tapsApi.delete, onSuccess: invalidate });
  const reconcileMutation = useMutation({ mutationFn: tapsApi.reconcile, onSuccess: invalidate });
  const pairMutation = useMutation({
    mutationFn: ({ cardName, accountId }: { cardName: string; accountId: number | null }) =>
      tapsApi.pairCard(cardName, accountId),
    onSuccess: invalidate,
  });

  const unpairedCards = cards.filter((c) => c.account_id === null);

  // Total spend awaiting a statement, per currency
  const pendingTotals =
    tab === "unmatched"
      ? taps.reduce<Record<string, number>>((totals, t) => {
          totals[t.currency] = (totals[t.currency] ?? 0) + parseFloat(t.amount);
          return totals;
        }, {})
      : {};

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between gap-2">
        <div className="flex items-center gap-2">
          <Button variant="ghost" size="icon" asChild>
            <Link to="/imports" aria-label="Back to Import">
              <ChevronLeft className="h-5 w-5" />
            </Link>
          </Button>
          <h1 className="text-3xl font-bold">Card taps</h1>
        </div>
        <Button
          variant="outline"
          onClick={() => reconcileMutation.mutate()}
          disabled={reconcileMutation.isPending}
          title="Match taps against imported statements"
        >
          <RefreshCw className={cn("h-4 w-4 mr-1", reconcileMutation.isPending && "animate-spin")} />
          Reconcile
        </Button>
      </div>

      {reconcileMutation.data && (
        <p className="text-sm text-muted-foreground">
          {reconcileMutation.data.reconciled === 0
            ? "No new matches found."
            : `Matched ${reconcileMutation.data.reconciled} tap${reconcileMutation.data.reconciled === 1 ? "" : "s"} to statement transactions.`}
        </p>
      )}

      {unpairedCards.length > 0 && (
        <Card className="border-warning">
          <CardHeader>
            <CardTitle className="text-base">Pair your cards</CardTitle>
            <CardDescription>
              Choose the account each wallet card pays from so taps can be matched to statements.
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-3">
            {unpairedCards.map((card) => (
              <CardPairingRow
                key={card.card_name}
                card={card}
                accounts={payAccounts}
                onPair={(accountId) => pairMutation.mutate({ cardName: card.card_name, accountId })}
              />
            ))}
          </CardContent>
        </Card>
      )}

      <div className="flex gap-1 rounded-lg bg-muted p-1 w-fit">
        {TABS.map(({ value, label }) => (
          <button
            key={value}
            onClick={() => setTab(value)}
            className={cn(
              "rounded-md px-3 py-1.5 text-sm font-medium transition-colors",
              tab === value
                ? "bg-background text-foreground shadow-sm"
                : "text-muted-foreground hover:text-foreground"
            )}
          >
            {label}
          </button>
        ))}
      </div>

      {Object.keys(pendingTotals).length > 0 && (
        <p className="text-sm text-muted-foreground">
          Not yet on a statement:{" "}
          <span className="font-semibold text-foreground tabular-nums">
            {Object.entries(pendingTotals)
              .map(([currency, total]) => formatCurrency(total, currency))
              .join(" + ")}
          </span>
        </p>
      )}

      {isLoading ? (
        <div className="text-center py-12 text-muted-foreground">Loading...</div>
      ) : taps.length === 0 ? (
        <Card>
          <CardContent className="py-16 flex flex-col items-center gap-3 text-center text-muted-foreground">
            <Nfc className="h-10 w-10 opacity-30" />
            <p className="font-medium">
              {tab === "unmatched" ? "Nothing awaiting a statement" : `No ${tab} taps`}
            </p>
            {tab === "unmatched" && (
              <p className="text-sm max-w-xs">
                Card payments sent from your phone's wallet shortcut appear here until the statement
                that includes them is imported.
              </p>
            )}
          </CardContent>
        </Card>
      ) : (
        <div className="space-y-2">
          {taps.map((tap) => (
            <TapRow
              key={tap.id}
              tap={tap}
              onDismiss={() => dismissMutation.mutate(tap.id)}
              onRestore={() => restoreMutation.mutate(tap.id)}
              onDelete={() => {
                if (window.confirm(`Delete the ${tap.merchant} tap? This can't be undone.`)) {
                  deleteMutation.mutate(tap.id);
                }
              }}
            />
          ))}
        </div>
      )}

      {cards.some((c) => c.account_id !== null) && (
        <Card>
          <CardHeader>
            <CardTitle className="text-base">Wallet cards</CardTitle>
          </CardHeader>
          <CardContent className="space-y-3">
            {cards
              .filter((c) => c.account_id !== null)
              .map((card) => (
                <CardPairingRow
                  key={card.card_name}
                  card={card}
                  accounts={payAccounts}
                  onPair={(accountId) =>
                    pairMutation.mutate({ cardName: card.card_name, accountId })
                  }
                />
              ))}
          </CardContent>
        </Card>
      )}
    </div>
  );
}

function CardPairingRow({
  card,
  accounts,
  onPair,
}: {
  card: WalletCard;
  accounts: Account[];
  onPair: (accountId: number | null) => void;
}) {
  return (
    <div className="flex flex-col sm:flex-row sm:items-center gap-2">
      <div className="flex items-center gap-2 sm:w-64 min-w-0">
        <CreditCard className="h-4 w-4 shrink-0 text-muted-foreground" />
        <span className="font-medium truncate">{card.card_name}</span>
        <span className="text-xs text-muted-foreground shrink-0">
          {card.tap_count} tap{card.tap_count === 1 ? "" : "s"}
        </span>
      </div>
      <Select
        value={card.account_id !== null ? String(card.account_id) : UNPAIRED}
        onValueChange={(v) => onPair(v === UNPAIRED ? null : Number(v))}
      >
        <SelectTrigger className="sm:max-w-sm">
          <SelectValue placeholder="Select account..." />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value={UNPAIRED}>Not paired</SelectItem>
          {accounts.map((a) => (
            <SelectItem key={a.id} value={String(a.id)}>
              {formatAccountPath(a.full_path)}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </div>
  );
}

function TapRow({
  tap,
  onDismiss,
  onRestore,
  onDelete,
}: {
  tap: CardTap;
  onDismiss: () => void;
  onRestore: () => void;
  onDelete: () => void;
}) {
  const amount = parseFloat(tap.amount);

  return (
    <div className="flex items-center gap-3 rounded-lg border p-3 hover:bg-accent/50 transition-colors">
      <div className="flex-1 min-w-0">
        <div className="flex items-center gap-2">
          <span className="font-medium truncate">{tap.merchant}</span>
          {amount < 0 && (
            <Badge variant="secondary" className="shrink-0">
              refund
            </Badge>
          )}
        </div>
        {tap.status === "unmatched" && tap.suggested_account_full_path && (
          <div className="mt-1 flex min-w-0">
            <Badge
              variant="outline"
              className="max-w-full min-w-0 gap-1 font-medium"
              title={
                tap.suggestion_source === "learned"
                  ? `From your history (${Math.round((tap.suggestion_confidence ?? 0) * 100)}% sure)`
                  : tap.suggestion_source === "ai"
                  ? `Suggested by AI${
                      tap.suggestion_confidence != null
                        ? ` (${Math.round(tap.suggestion_confidence * 100)}% sure)`
                        : ""
                    }`
                  : "Suggested by your rules"
              }
            >
              {tap.suggestion_source === "ai" && <Sparkles className="h-3 w-3 shrink-0" aria-label="AI" />}
              {tap.suggestion_source === "learned" && (
                <GraduationCap className="h-3 w-3 shrink-0" aria-label="From your history" />
              )}
              <span className="truncate">{formatAccountPath(tap.suggested_account_full_path)}</span>
            </Badge>
          </div>
        )}
        <div className="text-sm text-muted-foreground flex flex-wrap gap-x-3 mt-0.5">
          <span>{formatDate(tap.tapped_on)}</span>
          {tap.card_name && (
            <span className="truncate">
              {tap.card_name}
              {tap.account_full_path && ` → ${formatAccountPath(tap.account_full_path)}`}
            </span>
          )}
        </div>
        {tap.status === "reconciled" && tap.transaction_description && (
          <div className="text-sm text-muted-foreground flex items-center gap-1 mt-0.5 min-w-0">
            <Link2 className="h-3.5 w-3.5 shrink-0" />
            <span className="truncate">
              {tap.transaction_description}
              {tap.transaction_date && ` · ${formatDate(tap.transaction_date)}`}
            </span>
          </div>
        )}
      </div>
      <div className="shrink-0 text-right font-semibold tabular-nums whitespace-nowrap">
        {formatCurrency(Math.abs(amount), tap.currency)}
      </div>
      <div className="flex shrink-0 flex-col gap-1 sm:flex-row">
        {tap.status === "dismissed" ? (
          <Button variant="ghost" size="icon" className="h-8 w-8 sm:h-9 sm:w-9" onClick={onRestore} title="Restore">
            <RotateCcw className="h-4 w-4" />
          </Button>
        ) : (
          <Button variant="ghost" size="icon" className="h-8 w-8 sm:h-9 sm:w-9" onClick={onDismiss} title="Dismiss">
            <EyeOff className="h-4 w-4" />
          </Button>
        )}
        <Button variant="ghost" size="icon" className="h-8 w-8 sm:h-9 sm:w-9" onClick={onDelete} title="Delete" aria-label="Delete">
          <Trash2 className="h-4 w-4" />
        </Button>
      </div>
    </div>
  );
}
