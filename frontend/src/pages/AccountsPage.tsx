import { useState, useEffect } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { accountsApi, setupApi, type Account } from "../api/client";
import { formatCurrency, formatDate } from "../lib/utils";
import { Button } from "../components/ui/button";
import { Input } from "../components/ui/input";
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
  Folder,
  Plus,
  Trash2,
  ChevronRight,
  ChevronLeft,
  Pencil,
} from "lucide-react";
import { DynamicIcon } from "lucide-react/dynamic";

// ---------------------------------------------------------------------------
// Icon rendering + group config
// ---------------------------------------------------------------------------

const MY_ACCOUNTS_GROUPS = [
  { path: "Assets:Bank:Current", label: "Current Accounts" },
  { path: "Assets:Bank:Savings", label: "Savings Accounts" },
  { path: "Assets:Bank:ISA", label: "ISAs" },
  { path: "Assets:Cash", label: "Cash" },
  { path: "Liabilities:CreditCard", label: "Credit Cards" },
  { path: "Liabilities:Loan", label: "Loans" },
];

function resolveIconName(account: Account, accounts?: Account[]): string | null {
  if (account.icon) return account.icon;
  if (accounts && account.parent_id) {
    const parent = accounts.find((a) => a.id === account.parent_id);
    if (parent) return resolveIconName(parent, accounts);
  }
  return null;
}

function toKebabCase(name: string): string {
  return name
    .replace(/([A-Z])/g, (c, _, i) => (i === 0 ? c : `-${c}`).toLowerCase())
    .toLowerCase()
    .replace(/([a-z])(\d)/g, "$1-$2");
}

export function AccountIcon({ account, accounts, className }: { account: Account; accounts?: Account[]; className?: string }) {
  const name = resolveIconName(account, accounts);
  if (!name) return <Folder className={className} />;
  return <DynamicIcon name={toKebabCase(name) as Parameters<typeof DynamicIcon>[0]["name"]} className={className} />;
}

function slugify(name: string): string {
  return name
    .trim()
    .split(/\s+/)
    .map((w) => w.replace(/[^a-zA-Z0-9]/g, ""))
    .filter(Boolean)
    .join("");
}

function sanitizeIdentifier(id: string): string {
  return id
    .trim()
    .split(/\s+/)
    .map((p) => p.replace(/[^a-zA-Z0-9]/g, ""))
    .join("_");
}

type BankIdentifierContext = "sort_code" | "card_last4" | null;

function getBankIdentifierContext(parentFullPath: string): BankIdentifierContext {
  const sortCodePaths = ["Assets:Bank:Current", "Assets:Bank:Savings", "Assets:Bank:ISA"];
  const cardPaths = ["Liabilities:CreditCard"];
  if (sortCodePaths.some((p) => parentFullPath === p || parentFullPath.startsWith(p + ":")))
    return "sort_code";
  if (cardPaths.some((p) => parentFullPath === p || parentFullPath.startsWith(p + ":")))
    return "card_last4";
  return null;
}

// ---------------------------------------------------------------------------
// Main page
// ---------------------------------------------------------------------------

export function AccountsPage() {
  const queryClient = useQueryClient();

  const { data: accounts = [], isLoading } = useQuery({
    queryKey: ["accounts"],
    queryFn: accountsApi.list,
  });

  const seedMutation = useMutation({
    mutationFn: setupApi.seed,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["accounts"] });
      queryClient.invalidateQueries({ queryKey: ["rules"] });
      queryClient.invalidateQueries({ queryKey: ["setup-status"] });
    },
  });

  const childMap = buildChildMap(accounts);

  if (isLoading) {
    return <div className="py-12 text-center text-muted-foreground">Loading...</div>;
  }

  if (accounts.length === 0) {
    return (
      <Card>
        <CardContent className="py-12 text-center space-y-4">
          <p className="text-muted-foreground">No accounts yet.</p>
          <div className="flex flex-col items-center gap-2">
            <Button
              onClick={() => seedMutation.mutate()}
              disabled={seedMutation.isPending}
            >
              {seedMutation.isPending ? "Loading..." : "Load UK Defaults"}
            </Button>
            {seedMutation.isError && (
              <p className="text-sm text-destructive">{seedMutation.error.message}</p>
            )}
            <p className="text-xs text-muted-foreground">
              Loads the UK default chart of accounts and rules. You can customise these after.
            </p>
          </div>
        </CardContent>
      </Card>
    );
  }

  return (
    <div className="space-y-8">
      {/* My Accounts */}
      <section>
        <h2 className="sticky top-14 z-30 bg-background/95 backdrop-blur py-2 text-lg font-semibold border-b mb-4">
          My Accounts
        </h2>
        <MyAccountsSection accounts={accounts} childMap={childMap} />
      </section>

    </div>
  );
}

// ---------------------------------------------------------------------------
// My Accounts — icon grid with drill-down
// ---------------------------------------------------------------------------

function MyAccountsSection({
  accounts,
  childMap,
}: {
  accounts: Account[];
  childMap: Map<number, Account[]>;
}) {
  const [selectedBucket, setSelectedBucket] = useState<Account | null>(null);
  const [showTypePicker, setShowTypePicker] = useState(false);
  const [addParent, setAddParent] = useState<Account | null>(null);
  const [detailAccount, setDetailAccount] = useState<Account | null>(null);

  // All bucket accounts that exist in the seed
  const allBuckets = MY_ACCOUNTS_GROUPS
    .map((g) => accounts.find((a) => a.full_path === g.path))
    .filter((a): a is Account => a !== undefined);

  // Buckets that have at least one direct child (leaf account)
  const populatedBuckets = allBuckets.filter(
    (b) => (childMap.get(b.id) ?? []).length > 0
  );

  // Leaf accounts within the selected bucket
  const bucketChildren = selectedBucket
    ? childMap.get(selectedBucket.id) ?? []
    : [];

  return (
    <>
      {/* Breadcrumb when drilled in */}
      {selectedBucket && (
        <div className="flex items-center gap-1 mb-3">
          <button
            className="text-sm text-muted-foreground hover:text-foreground"
            onClick={() => setSelectedBucket(null)}
          >
            My Accounts
          </button>
          <ChevronRight className="h-3.5 w-3.5 text-muted-foreground" />
          <span className="text-sm font-medium">{selectedBucket.name}</span>
        </div>
      )}

      {/* Back button on mobile */}
      {selectedBucket && (
        <Button
          variant="ghost"
          size="sm"
          className="mb-3 -ml-2 sm:hidden"
          onClick={() => setSelectedBucket(null)}
        >
          <ChevronLeft className="h-4 w-4 mr-1" />
          Back
        </Button>
      )}

      {/* Root: bucket type tiles */}
      {!selectedBucket && (
        <div className="grid grid-cols-3 sm:grid-cols-4 gap-3">
          {populatedBuckets.map((bucket) => {
            const hasMultiple = (childMap.get(bucket.id) ?? []).length > 1;
            return (
              <CategoryTile
                key={bucket.id}
                account={bucket}
                accounts={accounts}
                hasChildren={hasMultiple}
                onClick={() => setSelectedBucket(bucket)}
              />
            );
          })}
          <button
            onClick={() => setShowTypePicker(true)}
            className="flex flex-col items-center justify-center gap-2 p-3 rounded-xl border border-dashed border-muted-foreground/30 text-muted-foreground hover:border-muted-foreground/60 hover:text-foreground transition-colors aspect-square"
          >
            <Plus className="h-6 w-6" />
            <span className="text-xs">Add</span>
          </button>
        </div>
      )}

      {/* Drilled in: leaf account tiles */}
      {selectedBucket && (
        <div className="grid grid-cols-3 sm:grid-cols-4 gap-3">
          {bucketChildren.map((acc) => (
            <AccountTile
              key={acc.id}
              account={acc}
              accounts={accounts}
              onClick={() => setDetailAccount(acc)}
            />
          ))}
          <button
            onClick={() => setAddParent(selectedBucket)}
            className="flex flex-col items-center justify-center gap-2 p-3 rounded-xl border border-dashed border-muted-foreground/30 text-muted-foreground hover:border-muted-foreground/60 hover:text-foreground transition-colors aspect-square"
          >
            <Plus className="h-6 w-6" />
            <span className="text-xs">Add</span>
          </button>
        </div>
      )}

      {/* Type picker dialog */}
      <TypePickerDialog
        open={showTypePicker}
        buckets={allBuckets}
        onPick={(bucket) => {
          setShowTypePicker(false);
          setAddParent(bucket);
        }}
        onOpenChange={(open) => { if (!open) setShowTypePicker(false); }}
      />

      <AddAccountDialog
        open={addParent !== null}
        parent={addParent}
        onOpenChange={(open) => {
          if (!open) setAddParent(null);
        }}
        onCreated={(account) => {
          // Navigate into the bucket that was just added to
          const bucket = allBuckets.find((b) => b.id === account.parent_id);
          if (bucket) setSelectedBucket(bucket);
        }}
      />
      <AccountDetailDialog
        account={detailAccount}
        accounts={accounts}
        onOpenChange={(open) => { if (!open) setDetailAccount(null); }}
      />
    </>
  );
}

// Tile for a real leaf account (shows name + identifier)
function AccountTile({
  account,
  accounts,
  onClick,
}: {
  account: Account;
  accounts: Account[];
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      className="relative flex flex-col items-center justify-center gap-2 p-3 rounded-xl border border-border bg-card hover:bg-accent transition-colors aspect-square"
    >
      <AccountIcon account={account} accounts={accounts} className="h-7 w-7 text-primary" />
      <span className="text-xs font-medium text-center leading-tight line-clamp-2">
        {account.name}
      </span>
      {account.bank_identifier && (
        <span className="text-[10px] text-muted-foreground truncate w-full text-center">
          {account.bank_identifier}
        </span>
      )}
    </button>
  );
}

// Type picker dialog — shows bucket type icons for the user to choose
function TypePickerDialog({
  open,
  buckets,
  onPick,
  onOpenChange,
}: {
  open: boolean;
  buckets: Account[];
  onPick: (bucket: Account) => void;
  onOpenChange: (open: boolean) => void;
}) {
  const groupLabel = (path: string) =>
    MY_ACCOUNTS_GROUPS.find((g) => g.path === path)?.label ?? path.split(":").pop() ?? path;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Add account</DialogTitle>
          <DialogDescription>What type of account would you like to add?</DialogDescription>
        </DialogHeader>
        <div className="grid grid-cols-3 gap-3 py-2">
          {buckets.map((bucket) => {
            return (
              <button
                key={bucket.id}
                onClick={() => onPick(bucket)}
                className="flex flex-col items-center justify-center gap-2 p-3 rounded-xl border border-border bg-card hover:bg-accent transition-colors aspect-square"
              >
                <AccountIcon account={bucket} accounts={buckets} className="h-7 w-7 text-primary" />
                <span className="text-xs font-medium text-center leading-tight">
                  {groupLabel(bucket.full_path)}
                </span>
              </button>
            );
          })}
        </div>
      </DialogContent>
    </Dialog>
  );
}

// ---------------------------------------------------------------------------
// Category grid (Spending / Income) — with drill-down
// ---------------------------------------------------------------------------

export function CategoryGrid({
  rootType,
  accounts,
  childMap,
}: {
  rootType: "expense" | "income";
  accounts: Account[];
  childMap: Map<number, Account[]>;
}) {
  const [navStack, setNavStack] = useState<Account[]>([]);
  const [addParent, setAddParent] = useState<Account | null>(null);
  const [detailAccount, setDetailAccount] = useState<Account | null>(null);

  const rootTypeName = rootType === "expense" ? "expense" : "income";
  const rootLabel = rootType === "expense" ? "Spending Categories" : "Income Sources";

  // Root accounts for this type (depth 0 — e.g. "Expenses" or "Income")
  const typeRoots = accounts.filter(
    (a) => a.type === rootTypeName && !a.parent_id
  );

  // Current node: if navStack is empty, show children of all type roots
  const currentChildren: Account[] =
    navStack.length === 0
      ? typeRoots.flatMap((r) => childMap.get(r.id) ?? [])
      : childMap.get(navStack[navStack.length - 1].id) ?? [];

  function drillInto(account: Account) {
    const hasChildren = (childMap.get(account.id) ?? []).length > 0;
    if (hasChildren) {
      setNavStack([...navStack, account]);
    } else {
      setDetailAccount(account);
    }
  }

  function navigateTo(index: number) {
    setNavStack(navStack.slice(0, index));
  }

  // Determine the parent for "Add" button
  const addTarget =
    navStack.length > 0
      ? navStack[navStack.length - 1]
      : typeRoots[0] ?? null;

  return (
    <>
      {/* Breadcrumb */}
      {navStack.length > 0 && (
        <div className="flex items-center gap-1 mb-3 flex-wrap">
          <button
            className="text-sm text-muted-foreground hover:text-foreground"
            onClick={() => navigateTo(0)}
          >
            {rootLabel}
          </button>
          {navStack.map((node, i) => (
            <span key={node.id} className="flex items-center gap-1">
              <ChevronRight className="h-3.5 w-3.5 text-muted-foreground" />
              {i < navStack.length - 1 ? (
                <button
                  className="text-sm text-muted-foreground hover:text-foreground"
                  onClick={() => navigateTo(i + 1)}
                >
                  {node.name}
                </button>
              ) : (
                <span className="text-sm font-medium">{node.name}</span>
              )}
            </span>
          ))}
        </div>
      )}

      {/* Back button on mobile when drilled in */}
      {navStack.length > 0 && (
        <Button
          variant="ghost"
          size="sm"
          className="mb-3 -ml-2 sm:hidden"
          onClick={() => setNavStack(navStack.slice(0, -1))}
        >
          <ChevronLeft className="h-4 w-4 mr-1" />
          Back
        </Button>
      )}

      {currentChildren.length === 0 ? (
        <p className="text-sm text-muted-foreground italic">
          Nothing here yet —{" "}
          <button className="underline" onClick={() => setAddParent(addTarget)}>
            add a category
          </button>
        </p>
      ) : (
        <div className="grid grid-cols-3 sm:grid-cols-4 gap-3">
          {currentChildren.map((account) => {
            const hasChildren = (childMap.get(account.id) ?? []).length > 0;
            return (
              <CategoryTile
                key={account.id}
                account={account}
                accounts={accounts}
                hasChildren={hasChildren}
                onClick={() => drillInto(account)}
              />
            );
          })}
          {/* Add tile */}
          <button
            onClick={() => setAddParent(addTarget)}
            className="flex flex-col items-center justify-center gap-2 p-3 rounded-xl border border-dashed border-muted-foreground/30 text-muted-foreground hover:border-muted-foreground/60 hover:text-foreground transition-colors aspect-square"
          >
            <Plus className="h-6 w-6" />
            <span className="text-xs">Add</span>
          </button>
        </div>
      )}

      <AddAccountDialog
        open={addParent !== null}
        parent={addParent}
        onOpenChange={(open) => { if (!open) setAddParent(null); }}
      />
      <AccountDetailDialog
        account={detailAccount}
        accounts={accounts}
        onOpenChange={(open) => { if (!open) setDetailAccount(null); }}
        onAddSubcategory={() => {
          const acc = detailAccount;
          setDetailAccount(null);
          setAddParent(acc);
        }}
      />
    </>
  );
}

function CategoryTile({
  account,
  accounts,
  hasChildren,
  onClick,
}: {
  account: Account;
  accounts: Account[];
  hasChildren: boolean;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      className={[
        "relative flex flex-col items-center justify-center gap-2 p-3 rounded-xl border transition-colors aspect-square",
        hasChildren
          ? "bg-primary/5 border-primary/30 hover:bg-primary/10"
          : "bg-card border-border hover:bg-accent",
      ].join(" ")}
    >
      <AccountIcon account={account} accounts={accounts} className="h-7 w-7 text-primary" />
      <span className="text-xs font-medium text-center leading-tight line-clamp-2">
        {account.name}
      </span>
      {hasChildren && (
        <ChevronRight className="absolute top-1.5 right-1.5 h-3 w-3 text-muted-foreground" />
      )}
    </button>
  );
}

// ---------------------------------------------------------------------------
// Account detail dialog (leaf)
// ---------------------------------------------------------------------------

function AccountDetailDialog({
  account,
  accounts: _accounts,
  onOpenChange,
  onAddSubcategory,
}: {
  account: Account | null;
  accounts?: Account[];
  onOpenChange: (open: boolean) => void;
  onAddSubcategory?: () => void;
}) {
  const queryClient = useQueryClient();
  const [editing, setEditing] = useState(false);
  const [name, setName] = useState("");
  const [bankIdentifier, setBankIdentifier] = useState("");
  const [selectedIcon, setSelectedIcon] = useState<string>("");

  const isAssetOrLiability =
    account?.type === "asset" || account?.type === "liability";

  const openingBalanceQuery = useQuery({
    queryKey: ["accounts", account?.id, "opening-balance"],
    queryFn: () =>
      account ? accountsApi.getOpeningBalance(account.id) : null,
    enabled: !!account && isAssetOrLiability,
  });

  useEffect(() => {
    if (account) {
      setName(account.name);
      setBankIdentifier(account.bank_identifier ?? "");
      setSelectedIcon(account.icon ?? "");
      setEditing(false);
    }
  }, [account]);

  const updateMutation = useMutation({
    mutationFn: ({ id, data }: { id: number; data: Partial<Account> }) =>
      accountsApi.update(id, data),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["accounts"] });
      setEditing(false);
    },
  });

  const deleteMutation = useMutation({
    mutationFn: accountsApi.delete,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["accounts"] });
      onOpenChange(false);
    },
  });

  if (!account) return null;

  return (
    <Dialog
      open={account !== null}
      onOpenChange={(o) => {
        if (!o) onOpenChange(false);
      }}
    >
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{account.name}</DialogTitle>
          <DialogDescription>
            {account.type.charAt(0).toUpperCase() + account.type.slice(1)}
            {(account.type === "asset" || account.type === "liability") && ` · ${account.currency}`}
          </DialogDescription>
        </DialogHeader>

        {editing ? (
          <div className="space-y-3">
            <div>
              <label className="text-sm font-medium">Name</label>
              <Input value={name} onChange={(e) => setName(e.target.value)} autoFocus />
            </div>
            {(account.type === "asset" || account.type === "liability") && (
              <div>
                <label className="text-sm font-medium">Bank identifier</label>
                <Input
                  value={bankIdentifier}
                  onChange={(e) => setBankIdentifier(e.target.value)}
                  placeholder="e.g. 12-34-56 12345678 or last 4 digits"
                />
              </div>
            )}
            <div>
              <label className="text-sm font-medium block mb-2">
                Icon{" "}
                <a
                  href="https://lucide.dev/icons/"
                  target="_blank"
                  rel="noopener noreferrer"
                  className="text-xs text-muted-foreground underline font-normal"
                >
                  Browse icons ↗
                </a>
              </label>
              <Input
                value={selectedIcon}
                onChange={(e) => setSelectedIcon(e.target.value)}
                placeholder="e.g. ShoppingBag"
              />
            </div>
          </div>
        ) : (
          <div className="space-y-2 text-sm">
            {account.bank_identifier && (
              <div className="flex justify-between">
                <span className="text-muted-foreground">Identifier</span>
                <span className="font-mono">{account.bank_identifier}</span>
              </div>
            )}
            {(account.type === "asset" || account.type === "liability") && (
              <div className="flex justify-between">
                <span className="text-muted-foreground">Opening balance</span>
                {openingBalanceQuery.isLoading ? (
                  <span className="text-muted-foreground italic">…</span>
                ) : openingBalanceQuery.data ? (
                  <span className="text-right">
                    <span className="font-mono">
                      {formatCurrency(
                        parseFloat(openingBalanceQuery.data.amount),
                        account.currency,
                      )}
                    </span>
                    <span className="text-muted-foreground text-xs ml-2">
                      {formatDate(openingBalanceQuery.data.date)}
                    </span>
                  </span>
                ) : (
                  <span className="text-muted-foreground italic">Not set</span>
                )}
              </div>
            )}
          </div>
        )}

        {updateMutation.isError && (
          <p className="text-sm text-destructive">{updateMutation.error.message}</p>
        )}

        <DialogFooter className="gap-2 flex-col sm:flex-row">
          <Button
            variant="destructive"
            size="sm"
            className="sm:mr-auto"
            onClick={() => deleteMutation.mutate(account.id)}
            disabled={deleteMutation.isPending}
          >
            <Trash2 className="h-3.5 w-3.5 mr-1" />
            Delete
          </Button>
          {onAddSubcategory && !editing && (
            <Button variant="outline" size="sm" onClick={onAddSubcategory}>
              <Plus className="h-3.5 w-3.5 mr-1" />
              Add subcategory
            </Button>
          )}
          {editing ? (
            <>
              <Button variant="outline" size="sm" onClick={() => setEditing(false)}>
                Cancel
              </Button>
              <Button
                size="sm"
                disabled={updateMutation.isPending}
                onClick={() =>
                  updateMutation.mutate({
                    id: account.id,
                    data: { name, bank_identifier: bankIdentifier || null, icon: selectedIcon || null },
                  })
                }
              >
                Save
              </Button>
            </>
          ) : (
            <Button
              variant="outline"
              size="sm"
              onClick={() => {
                setName(account.name);
                setBankIdentifier(account.bank_identifier ?? "");
                setEditing(true);
              }}
            >
              <Pencil className="h-3.5 w-3.5 mr-1" />
              Edit
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

// ---------------------------------------------------------------------------
// Add account dialog — context-aware
// ---------------------------------------------------------------------------

function AddAccountDialog({
  open,
  parent,
  onOpenChange,
  onCreated,
}: {
  open: boolean;
  parent: Account | null;
  onOpenChange: (open: boolean) => void;
  onCreated?: (account: Account) => void;
}) {
  const queryClient = useQueryClient();
  const [name, setName] = useState("");
  const [sortCode, setSortCode] = useState("");
  const [accountNumber, setAccountNumber] = useState("");
  const [cardLast4, setCardLast4] = useState("");

  const idContext = parent ? getBankIdentifierContext(parent.full_path) : null;

  const createMutation = useMutation({
    mutationFn: accountsApi.create,
    onSuccess: (account) => {
      queryClient.invalidateQueries({ queryKey: ["accounts"] });
      onOpenChange(false);
      resetForm();
      onCreated?.(account);
    },
  });

  function resetForm() {
    setName("");
    setSortCode("");
    setAccountNumber("");
    setCardLast4("");
  }

  function buildBankIdentifier(): string | null {
    if (idContext === "sort_code") {
      const raw = `${sortCode} ${accountNumber}`.trim();
      return raw || null;
    }
    if (idContext === "card_last4") {
      return cardLast4.trim() || null;
    }
    return null;
  }

  function buildFullPath(): string {
    if (!parent) return slugify(name);
    const bankId = buildBankIdentifier();
    const suffix = bankId ? sanitizeIdentifier(bankId) : slugify(name);
    return `${parent.full_path}:${suffix}`;
  }

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!name || !parent) return;
    const bankId = buildBankIdentifier();
    createMutation.mutate({
      name,
      full_path: buildFullPath(),
      parent_id: parent.id,
      type: parent.type,
      currency: "GBP",
      bank_identifier: bankId,
    });
  }

  return (
    <Dialog open={open} onOpenChange={(o) => { if (!o) { onOpenChange(false); resetForm(); } }}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Add account</DialogTitle>
          {parent && (
            <DialogDescription>
              Adding under <span className="font-medium">{parent.name}</span>
            </DialogDescription>
          )}
        </DialogHeader>

        <form onSubmit={handleSubmit} className="space-y-4">
          <div>
            <label className="text-sm font-medium">Name</label>
            <Input
              placeholder={
                idContext === "sort_code"
                  ? "e.g. HSBC Current"
                  : idContext === "card_last4"
                  ? "e.g. HSBC Credit Card"
                  : "e.g. Dining Out"
              }
              value={name}
              onChange={(e) => setName(e.target.value)}
              autoFocus
            />
          </div>

          {idContext === "sort_code" && (
            <div className="grid grid-cols-2 gap-3">
              <div>
                <label className="text-sm font-medium">Sort code</label>
                <Input
                  placeholder="12-34-56"
                  value={sortCode}
                  onChange={(e) => setSortCode(e.target.value)}
                  maxLength={8}
                />
              </div>
              <div>
                <label className="text-sm font-medium">Account number</label>
                <Input
                  placeholder="12345678"
                  value={accountNumber}
                  onChange={(e) => setAccountNumber(e.target.value)}
                  maxLength={8}
                />
              </div>
              <p className="col-span-2 text-xs text-muted-foreground">
                Used to identify this account in transfer descriptions. Optional but recommended.
              </p>
            </div>
          )}

          {idContext === "card_last4" && (
            <div>
              <label className="text-sm font-medium">Last 4 digits</label>
              <Input
                placeholder="1234"
                value={cardLast4}
                onChange={(e) => setCardLast4(e.target.value.replace(/\D/g, "").slice(0, 4))}
                maxLength={4}
                inputMode="numeric"
              />
              <p className="text-xs text-muted-foreground mt-1">Optional but recommended.</p>
            </div>
          )}

          {createMutation.isError && (
            <p className="text-sm text-destructive">{createMutation.error.message}</p>
          )}

          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              onClick={() => { onOpenChange(false); resetForm(); }}
            >
              Cancel
            </Button>
            <Button type="submit" disabled={!name || createMutation.isPending}>
              {createMutation.isPending ? "Creating…" : "Create"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

export function buildChildMap(accounts: Account[]): Map<number, Account[]> {
  const map = new Map<number, Account[]>();
  accounts.forEach((a) => {
    if (a.parent_id != null) {
      const arr = map.get(a.parent_id) ?? [];
      arr.push(a);
      map.set(a.parent_id, arr);
    }
  });
  return map;
}
