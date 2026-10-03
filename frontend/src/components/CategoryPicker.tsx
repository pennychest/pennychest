import { useMemo, useState } from "react";
import { ChevronLeft, ChevronRight, CircleHelp, Search } from "lucide-react";
import type { Account } from "../api/client";
import { AccountIcon, buildChildMap } from "../pages/AccountsPage";
import { cn, formatAccountPath } from "../lib/utils";
import { Button } from "./ui/button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "./ui/dialog";
import { Input } from "./ui/input";

const UNCATEGORISED = "Expenses:Uncategorised";

type Side = "expense" | "income";

// Choose a spending or income category by icon, the way they're laid out in Settings.
// Picking one is the whole action: there's no separate confirm step.
export function CategoryPicker({
  open,
  onOpenChange,
  title = "Choose a category",
  description,
  accounts,
  moneyIn,
  currentAccountId,
  onPick,
  isPending,
  showUncategorised = false,
  parentLabel = (name) => `Just ${name}`,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title?: string;
  description?: string;
  accounts: Account[];
  // Money coming in opens on income, money going out on spending
  moneyIn: boolean;
  currentAccountId?: number | null;
  onPick: (accountId: number) => void;
  isPending?: boolean;
  // Offer Uncategorised as a choice, e.g. when filtering
  showUncategorised?: boolean;
  // Label for the tile that picks the category you've drilled into, rather than a child of it
  parentLabel?: (name: string) => string;
}) {
  const [side, setSide] = useState<Side>(moneyIn ? "income" : "expense");
  const [path, setPath] = useState<Account[]>([]);
  const [query, setQuery] = useState("");

  const categories = useMemo(
    () => accounts.filter((a) => (a.type === "expense" || a.type === "income") && a.full_path !== UNCATEGORISED),
    [accounts],
  );
  const uncategorised = accounts.find((a) => a.full_path === UNCATEGORISED);
  const childMap = useMemo(() => buildChildMap(categories), [categories]);
  const roots = categories.filter((a) => a.type === side && !a.parent_id);
  const here = path.at(-1);
  const tiles = here ? childMap.get(here.id) ?? [] : roots.flatMap((r) => childMap.get(r.id) ?? []);

  const matches = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return [];
    return categories
      .filter((a) => a.parent_id && a.full_path.toLowerCase().includes(q))
      .sort((a, b) => a.full_path.localeCompare(b.full_path))
      .slice(0, 30);
  }, [categories, query]);

  function switchSide(next: Side) {
    setSide(next);
    setPath([]);
  }

  function open_(account: Account) {
    if ((childMap.get(account.id) ?? []).length > 0) setPath([...path, account]);
    else onPick(account.id);
  }

  const tileClass =
    "relative flex flex-col items-center justify-center gap-1.5 p-2 rounded-xl border transition-colors aspect-square disabled:opacity-50";

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[90dvh] overflow-y-auto">
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          {description && <DialogDescription>{description}</DialogDescription>}
        </DialogHeader>

        <div className="relative">
          <Search className="absolute left-2.5 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground" />
          <Input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search categories"
            className="pl-8"
            aria-label="Search categories"
          />
        </div>

        {query.trim() ? (
          matches.length === 0 ? (
            <p className="text-sm text-muted-foreground py-4 text-center">No categories match.</p>
          ) : (
            <ul className="divide-y rounded-lg border">
              {matches.map((a) => (
                <li key={a.id}>
                  <button
                    type="button"
                    onClick={() => onPick(a.id)}
                    disabled={isPending}
                    className={cn(
                      "w-full flex items-center gap-3 px-3 py-2 text-left text-sm hover:bg-accent",
                      a.id === currentAccountId && "bg-primary/10",
                    )}
                  >
                    <AccountIcon account={a} accounts={categories} className="h-4 w-4 text-primary shrink-0" />
                    <span className="truncate">{formatAccountPath(a.full_path)}</span>
                  </button>
                </li>
              ))}
            </ul>
          )
        ) : (
          <>
            <div className="grid grid-cols-2 rounded-lg bg-muted p-1 text-sm" role="tablist">
              {(["expense", "income"] as const).map((s) => (
                <button
                  key={s}
                  type="button"
                  role="tab"
                  aria-selected={side === s}
                  onClick={() => switchSide(s)}
                  className={cn(
                    "rounded-md py-1.5 font-medium transition-colors",
                    side === s ? "bg-background shadow-sm" : "text-muted-foreground",
                  )}
                >
                  {s === "expense" ? "Spending" : "Income"}
                </button>
              ))}
            </div>

            {here && (
              <div className="flex items-center gap-1 text-sm">
                <Button variant="ghost" size="sm" className="-ml-2" onClick={() => setPath(path.slice(0, -1))}>
                  <ChevronLeft className="h-4 w-4 mr-0.5" />
                  Back
                </Button>
                <span className="text-muted-foreground truncate">{path.map((p) => p.name).join(" › ")}</span>
              </div>
            )}

            <div className="grid grid-cols-3 sm:grid-cols-4 gap-2">
              {here && (
                <button
                  type="button"
                  onClick={() => onPick(here.id)}
                  disabled={isPending}
                  className={cn(
                    tileClass,
                    "border-dashed hover:bg-accent",
                    here.id === currentAccountId && "ring-2 ring-primary",
                  )}
                >
                  <AccountIcon account={here} accounts={categories} className="h-6 w-6 text-primary" />
                  <span className="text-xs font-medium text-center leading-tight line-clamp-2">
                    {parentLabel(here.name)}
                  </span>
                </button>
              )}
              {showUncategorised && uncategorised && !here && side === "expense" && (
                <button
                  type="button"
                  onClick={() => onPick(uncategorised.id)}
                  disabled={isPending}
                  className={cn(
                    tileClass,
                    "border-dashed hover:bg-accent",
                    uncategorised.id === currentAccountId && "ring-2 ring-primary",
                  )}
                >
                  <CircleHelp className="h-6 w-6 text-muted-foreground" />
                  <span className="text-xs font-medium text-center leading-tight line-clamp-2">Uncategorised</span>
                </button>
              )}
              {tiles.map((a) => {
                const hasChildren = (childMap.get(a.id) ?? []).length > 0;
                return (
                  <button
                    key={a.id}
                    type="button"
                    onClick={() => open_(a)}
                    disabled={isPending}
                    className={cn(
                      tileClass,
                      hasChildren ? "bg-primary/5 border-primary/30 hover:bg-primary/10" : "bg-card hover:bg-accent",
                      a.id === currentAccountId && "ring-2 ring-primary",
                    )}
                  >
                    <AccountIcon account={a} accounts={categories} className="h-6 w-6 text-primary" />
                    <span className="text-xs font-medium text-center leading-tight line-clamp-2">{a.name}</span>
                    {hasChildren && (
                      <ChevronRight className="absolute top-1.5 right-1.5 h-3 w-3 text-muted-foreground" />
                    )}
                  </button>
                );
              })}
            </div>
            {tiles.length === 0 && !here && (
              <p className="text-sm text-muted-foreground text-center">
                No {side === "expense" ? "spending" : "income"} categories yet. Add some in Settings.
              </p>
            )}
          </>
        )}
      </DialogContent>
    </Dialog>
  );
}
