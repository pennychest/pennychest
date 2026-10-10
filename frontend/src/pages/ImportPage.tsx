import { useState, useRef, useEffect, useMemo } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import {
  importsApi,
  accountsApi,
  aiApi,
  tapsApi,
  type ImportBatch,
  type StatementDetection,
  type Account,
  type OpeningBalance,
  type CsvColumns,
  type CsvPreview,
} from "../api/client";
import { CsvMapping } from "../components/CsvMapping";
import { columnsFromPreview, missingCsvFields } from "../lib/csv";
import { Button } from "../components/ui/button";
import { Input } from "../components/ui/input";
import { Card, CardContent, CardHeader, CardTitle } from "../components/ui/card";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "../components/ui/select";
import { Badge } from "../components/ui/badge";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "../components/ui/dialog";
import { Popover, PopoverContent, PopoverTrigger } from "../components/ui/popover";
import { formatCurrency, formatDate, formatAccountPath } from "../lib/utils";
import {
  Upload,
  FileText,
  Eye,
  Trash2,
  AlertCircle,
  CheckCircle2,
  Plus,
  Bot,
  X,
  FileDown,
  Nfc,
  ChevronRight,
  Check,
  MoreHorizontal,
} from "lucide-react";

function slugify(name: string): string {
  return name.trim().split(/\s+/).map((w) => w.replace(/[^a-zA-Z0-9]/g, "")).filter(Boolean).join("");
}

function sanitizeIdentifier(id: string): string {
  return id.trim().split(/\s+/).map((p) => p.replace(/[^a-zA-Z0-9]/g, "")).join("_");
}

function accountLabel(a: Account): string {
  return a.bank_identifier ? `${a.name} (${a.bank_identifier})` : a.name;
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

type AccountGroup = { path: string; label: string; type: "asset" | "liability" };

// Where a new account for a detected statement goes in the chart of accounts
function statementParent(detection: StatementDetection | null): {
  path: string;
  type: "asset" | "liability";
} {
  switch (detection?.statement_type) {
    case "credit_card":
      return { path: "Liabilities:CreditCard", type: "liability" };
    case "savings":
      return { path: "Assets:Bank:Savings", type: "asset" };
    default:
      return { path: "Assets:Bank:Current", type: "asset" };
  }
}

const ACCOUNT_GROUPS: AccountGroup[] = [
  { path: "Assets:Bank:Current", label: "Current Account", type: "asset" },
  { path: "Assets:Bank:Savings", label: "Savings Account", type: "asset" },
  { path: "Assets:Bank:ISA", label: "ISA", type: "asset" },
  { path: "Assets:Cash", label: "Cash", type: "asset" },
  { path: "Liabilities:CreditCard", label: "Credit Card", type: "liability" },
  { path: "Liabilities:Loan", label: "Loan", type: "liability" },
];

const NOT_IMPORTABLE_PATHS = new Set([
  ...ACCOUNT_GROUPS.map((g) => g.path),
  "Assets:Transfers:Pending",
]);

async function ensureParentChain(
  groupPath: string,
  type: "asset" | "liability",
  existingAccounts: Account[],
): Promise<number> {
  const segments = groupPath.split(":");
  let parentId: number | null = null;
  for (let i = 0; i < segments.length; i++) {
    const currentPath = segments.slice(0, i + 1).join(":");
    const existing = existingAccounts.find((a) => a.full_path === currentPath);
    if (existing) {
      parentId = existing.id;
    } else {
      const created = await accountsApi.create({
        name: segments[i],
        full_path: currentPath,
        parent_id: parentId,
        type,
        currency: "GBP",
        bank_identifier: null,
      });
      parentId = created.id;
    }
  }
  return parentId!;
}

// ---------------------------------------------------------------------------
// File entry types
// ---------------------------------------------------------------------------

type FilePhase = "detecting" | "confirming" | "queued" | "uploading" | "done" | "error";

type FileEntry = {
  id: string;
  file: File;
  phase: FilePhase;
  // Set when the importer read the statement's account details (e.g. bank PDF statements)
  detection: StatementDetection | null;
  accountId: string;
  importerName: string;
  batchId: number | null;
  error: string | null;
  aiNote?: string | null;
  // CSV files: the file's columns, and which holds each field
  csv?: CsvPreview | null;
  csvColumns?: CsvColumns;
};

// ---------------------------------------------------------------------------
// Group state types (for statements grouped by bank_identifier)
// ---------------------------------------------------------------------------

type GroupPhase = "confirming" | "importing" | "done" | "error";

type GroupState = {
  phase: GroupPhase;
  accountId: string;
  importedAccountId: number | null;
  error: string | null;
};

// ---------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------

export function ImportPage() {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const fileInputRef = useRef<HTMLInputElement>(null);

  const [pendingFiles, setPendingFiles] = useState<FileEntry[]>([]);
  const [groupStates, setGroupStates] = useState<Record<string, GroupState>>({});
  const [dragOver, setDragOver] = useState(false);

  const { data: accounts = [] } = useQuery({
    queryKey: ["accounts"],
    queryFn: accountsApi.list,
  });

  const { data: batches = [], isLoading: batchesLoading } = useQuery({
    queryKey: ["import-batches"],
    queryFn: importsApi.listBatches,
  });

  const { data: importers = [] } = useQuery({
    queryKey: ["importers"],
    queryFn: importsApi.listImporters,
  });
  const acceptedTypes = [
    ...new Set(importers.flatMap((i) => i.file_types.map((t) => `.${t}`))),
  ].join(",");

  const { data: aiConfig } = useQuery({
    queryKey: ["ai-config"],
    queryFn: aiApi.config,
  });

  const { data: unmatchedTaps = [] } = useQuery({
    queryKey: ["taps", "unmatched"],
    queryFn: () => tapsApi.list("unmatched"),
  });

  const aiCategoriseAllMutation = useMutation({
    mutationFn: aiApi.categoriseAll,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["import-batches"] });
    },
  });

  const deleteMutation = useMutation({
    mutationFn: importsApi.deleteBatch,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["import-batches"] });
      queryClient.invalidateQueries({ queryKey: ["transactions"] });
    },
  });

  // Accounts a statement can be imported into: asset and liability leaves, other than the
  // group headings (Assets:Bank:Current, Liabilities:CreditCard, …) and the transfers account.
  // Credit cards sit a level higher than bank accounts (Liabilities:CreditCard:1234), so this
  // can't go by depth.
  const bankAccounts = accounts.filter((a) => {
    if (a.type !== "asset" && a.type !== "liability") return false;
    if (NOT_IMPORTABLE_PATHS.has(a.full_path)) return false;
    const hasChildren = accounts.some((b) => b.parent_id === a.id);
    const depth = a.full_path.split(":").length - 1;
    return !hasChildren && depth >= 2;
  });

  // Initialize group states when statements finish detection
  useEffect(() => {
    setGroupStates((prev) => {
      const next = { ...prev };
      for (const f of pendingFiles) {
        if (!f.detection) continue;
        const key = f.detection.bank_identifier;
        if (next[key]) continue; // already initialized; don't overwrite user changes
        next[key] = {
          phase: "confirming",
          accountId: f.detection.suggested_account_id
            ? String(f.detection.suggested_account_id)
            : "",
          importedAccountId: null,
          error: null,
        };
      }
      return next;
    });
  }, [pendingFiles]);

  // Derived: statements grouped by bank_identifier
  const statementGroups = useMemo(() => {
    const groups = new Map<string, FileEntry[]>();
    for (const f of pendingFiles) {
      if (!f.detection) continue;
      const key = f.detection.bank_identifier;
      if (!groups.has(key)) groups.set(key, []);
      groups.get(key)!.push(f);
    }
    return groups;
  }, [pendingFiles]);

  // Files still detecting, and files whose format doesn't identify an account (e.g. CSV)
  const detectingFiles = pendingFiles.filter((f) => f.phase === "detecting");
  const otherFiles = pendingFiles.filter((f) => !f.detection && f.phase !== "detecting");

  function updateFile(id: string, updates: Partial<FileEntry>) {
    setPendingFiles((prev) => prev.map((f) => (f.id === id ? { ...f, ...updates } : f)));
  }

  function updateGroup(bankId: string, updates: Partial<GroupState>) {
    setGroupStates((prev) => ({
      ...prev,
      [bankId]: { ...prev[bankId], ...updates },
    }));
  }

  async function processFile(id: string, file: File) {
    try {
      const { importer, statement } = await importsApi.detect(file);
      const accountId = statement?.suggested_account_id
        ? String(statement.suggested_account_id)
        : "";
      const csv = importer === "csv" ? await importsApi.csvPreview(file) : null;
      updateFile(id, {
        phase: accountId ? "queued" : "confirming",
        detection: statement,
        accountId,
        importerName: importer,
        csv,
        csvColumns: csv ? columnsFromPreview(csv) : undefined,
      });
    } catch (e) {
      updateFile(id, {
        phase: "error",
        error: e instanceof Error ? e.message : "Couldn't read this file",
      });
    }
  }

  function addFiles(files: File[]) {
    if (files.length === 0) return;
    const entries: FileEntry[] = files.map((file) => ({
      id: `${Date.now()}-${Math.random()}`,
      file,
      phase: "detecting" as FilePhase,
      detection: null,
      accountId: "",
      importerName: "",
      batchId: null,
      error: null,
    }));
    setPendingFiles((prev) => [...prev, ...entries]);
    entries.forEach((e) => processFile(e.id, e.file));
  }

  // Used for files imported one at a time (e.g. CSV)
  async function importEntry(entry: FileEntry) {
    if (!entry.accountId) return;
    updateFile(entry.id, { phase: "uploading", error: null });
    try {
      const result = await importsApi.upload(
        entry.file,
        parseInt(entry.accountId),
        entry.importerName,
        entry.csv ? entry.csvColumns : undefined,
      );
      updateFile(entry.id, {
        phase: "done",
        batchId: result.batch_id,
        aiNote: [
          result.ai_categorised_count > 0 &&
            `Categorised ${result.ai_categorised_count} transaction${result.ai_categorised_count === 1 ? "" : "s"}` +
              (result.learned_count > 0 ? ` (${result.learned_count} from your history)` : ""),
          result.ai_error && `AI couldn't categorise the rest: ${result.ai_error}`,
        ]
          .filter(Boolean)
          .join(". ") || null,
      });
      queryClient.invalidateQueries({ queryKey: ["import-batches"] });
      queryClient.invalidateQueries({ queryKey: ["changes"] });
      queryClient.invalidateQueries({ queryKey: ["transactions"] });
    } catch (e) {
      updateFile(entry.id, {
        phase: "error",
        error: e instanceof Error ? e.message : "Upload failed",
      });
    }
  }

  // Used for statement groups (all files in the group share one account)
  async function importGroup(bankId: string, entries: FileEntry[], accountId: string) {
    const accountIdNum = parseInt(accountId);
    updateGroup(bankId, { phase: "importing", error: null });
    for (const entry of entries) {
      updateFile(entry.id, { phase: "uploading", error: null });
      try {
        const result = await importsApi.upload(entry.file, accountIdNum, entry.importerName);
        updateFile(entry.id, { phase: "done", batchId: result.batch_id });
      } catch (e) {
        const errMsg = e instanceof Error ? e.message : "Upload failed";
        updateFile(entry.id, { phase: "error", error: errMsg });
        updateGroup(bankId, {
          phase: "error",
          error: `Failed to import ${entry.file.name}: ${errMsg}`,
        });
        queryClient.invalidateQueries({ queryKey: ["import-batches"] });
        return;
      }
    }
    queryClient.invalidateQueries({ queryKey: ["import-batches"] });
    queryClient.invalidateQueries({ queryKey: ["transactions"] });
    queryClient.invalidateQueries({ queryKey: ["changes"] });
    updateGroup(bankId, { phase: "done", importedAccountId: accountIdNum });
    navigate({
      to: "/imports/account/$accountId",
      params: { accountId: String(accountIdNum) },
    });
  }

  function handleDrop(e: React.DragEvent) {
    e.preventDefault();
    setDragOver(false);
    addFiles(Array.from(e.dataTransfer.files));
  }

  function handleFileSelect(e: React.ChangeEvent<HTMLInputElement>) {
    addFiles(Array.from(e.target.files ?? []));
    e.target.value = "";
  }

  // Group import history by account
  const batchesByAccount = useMemo(() => {
    const groups = new Map<number | null, ImportBatch[]>();
    for (const batch of batches) {
      const key = batch.account_id;
      if (!groups.has(key)) groups.set(key, []);
      groups.get(key)!.push(batch);
    }
    return groups;
  }, [batches]);

  return (
    <div className="space-y-6">
      <h1 className="text-3xl font-bold">Import</h1>

      <Link to="/imports/taps" className="block">
        <Card className="hover:bg-accent transition-colors">
          <CardContent className="flex items-center gap-3 py-4">
            <Nfc className="h-5 w-5 text-muted-foreground" />
            <span className="font-medium">Card taps</span>
            <span className="text-sm text-muted-foreground ml-auto">
              {unmatchedTaps.length === 0
                ? "All matched"
                : `${unmatchedTaps.length} awaiting a statement`}
            </span>
            <ChevronRight className="h-4 w-4 text-muted-foreground" />
          </CardContent>
        </Card>
      </Link>

      <Card>
        <CardHeader>
          <CardTitle>Upload Bank Statements</CardTitle>
        </CardHeader>
        <CardContent className="space-y-4">

          {/* Drop zone */}
          <div
            className={`border-2 border-dashed rounded-lg p-8 text-center transition-colors cursor-pointer ${
              dragOver
                ? "border-primary bg-primary/5"
                : "border-muted-foreground/25 hover:border-primary/50"
            }`}
            onDragOver={(e) => { e.preventDefault(); setDragOver(true); }}
            onDragLeave={() => setDragOver(false)}
            onDrop={handleDrop}
            onClick={() => fileInputRef.current?.click()}
          >
            <input
              ref={fileInputRef}
              type="file"
              accept={acceptedTypes || undefined}
              multiple
              onChange={handleFileSelect}
              className="hidden"
            />
            <div className="space-y-2">
              <Upload className="h-8 w-8 mx-auto text-muted-foreground" />
              <p className="text-sm text-muted-foreground">
                Drop one or more bank statements here, or click to browse
              </p>
              {importers.length > 0 && (
                <p className="text-xs text-muted-foreground">
                  Supports {importers.map((i) => i.label).join(", ")}
                </p>
              )}
            </div>
          </div>

          {/* Files still being analysed */}
          {detectingFiles.length > 0 && (
            <div className="space-y-1">
              {detectingFiles.map((f) => (
                <div
                  key={f.id}
                  className="flex items-center gap-2 rounded-md border px-3 py-2 text-sm"
                >
                  <FileText className="h-4 w-4 text-muted-foreground shrink-0" />
                  <span className="flex-1 truncate text-muted-foreground">{f.file.name}</span>
                  <span className="text-xs text-muted-foreground animate-pulse">Detecting…</span>
                </div>
              ))}
            </div>
          )}

          {/* Statement groups */}
          {Array.from(statementGroups.entries()).map(([bankId, entries]) => (
            <AccountGroupCard
              key={bankId}
              bankId={bankId}
              entries={entries}
              groupState={groupStates[bankId]}
              allAccounts={accounts}
              leafBankAccounts={bankAccounts}
              onImport={(accountId) => importGroup(bankId, entries, accountId)}
              onAccountChange={(accountId) => updateGroup(bankId, { accountId })}
              onAccountCreated={(account) => {
                queryClient.invalidateQueries({ queryKey: ["accounts"] });
                updateGroup(bankId, { accountId: String(account.id) });
              }}
              onRemoveEntry={(id) =>
                setPendingFiles((prev) => prev.filter((f) => f.id !== id))
              }
              onReview={(accountId) =>
                navigate({
                  to: "/imports/account/$accountId",
                  params: { accountId: String(accountId) },
                })
              }
            />
          ))}

          {/* Files that don't identify their account */}
          {otherFiles.map((entry) => (
            <FileCard
              key={entry.id}
              entry={entry}
              allAccounts={accounts}
              leafBankAccounts={bankAccounts}
              onAccountChange={(id) => updateFile(entry.id, { accountId: id })}
              onCsvChange={(csv, csvColumns) => updateFile(entry.id, { csv, csvColumns })}
              onImport={() => importEntry(entry)}
              onRemove={() =>
                setPendingFiles((prev) => prev.filter((f) => f.id !== entry.id))
              }
              onReview={() =>
                navigate({
                  to: "/imports/$batchId",
                  params: { batchId: String(entry.batchId) },
                })
              }
              onAccountCreated={(account) => {
                queryClient.invalidateQueries({ queryKey: ["accounts"] });
                updateFile(entry.id, { accountId: String(account.id), phase: "queued" });
              }}
            />
          ))}

        </CardContent>
      </Card>

      {/* Import history — grouped by account */}
      <div>
        <div className="flex items-center justify-between mb-3">
          <h2 className="text-xl font-semibold">Import History</h2>
          <div className="flex items-center gap-2">
            {aiCategoriseAllMutation.isSuccess && (
              <span className="text-sm text-green-600">
                AI categorised {aiCategoriseAllMutation.data.updated} transactions
              </span>
            )}
            {aiCategoriseAllMutation.isError && (
              <span className="text-sm text-destructive">
                {aiCategoriseAllMutation.error.message}
              </span>
            )}
            <Button
              variant="outline"
              size="sm"
              onClick={() => aiCategoriseAllMutation.mutate()}
              disabled={aiCategoriseAllMutation.isPending || !aiConfig?.tasks.categorise.ready}
              title={aiConfig?.tasks.categorise.problem ?? undefined}
            >
              <Bot className="h-4 w-4 mr-1" />
              {aiCategoriseAllMutation.isPending
                ? "Categorising..."
                : "Categorise All with AI"}
            </Button>
          </div>
        </div>
        {batchesLoading ? (
          <div className="text-center py-8 text-muted-foreground">Loading…</div>
        ) : batches.length === 0 ? (
          <Card>
            <CardContent className="py-8 text-center text-muted-foreground">
              No imports yet. Upload your first bank statement above.
            </CardContent>
          </Card>
        ) : (
          <div className="space-y-5">
            {Array.from(batchesByAccount.entries()).map(([accountId, accountBatches]) => (
              <div key={accountId ?? "unknown"} className="space-y-2">
                <div className="flex items-center gap-2 px-1">
                  {accountId ? (
                    <button
                      className="text-sm font-semibold hover:underline text-left"
                      onClick={() =>
                        navigate({
                          to: "/imports/account/$accountId",
                          params: { accountId: String(accountId) },
                        })
                      }
                    >
                      {accountBatches[0].account_name || accountBatches[0].account_path
                        ? accountHeading(accountBatches[0])
                        : `Account #${accountId}`}
                    </button>
                  ) : (
                    <span className="text-sm font-semibold text-muted-foreground">
                      Unknown account
                    </span>
                  )}
                  <span className="text-xs text-muted-foreground">
                    {accountBatches.length} file{accountBatches.length !== 1 ? "s" : ""}
                  </span>
                </div>
                {accountBatches.map((batch) => (
                  <BatchRow
                    key={batch.id}
                    batch={batch}
                    onReview={() =>
                      navigate({
                        to: "/imports/$batchId",
                        params: { batchId: String(batch.id) },
                      })
                    }
                    onDelete={() => deleteMutation.mutate(batch.id)}
                  />
                ))}
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// AccountGroupCard — groups statements sharing the same bank_identifier
// ---------------------------------------------------------------------------

function AccountGroupCard({
  bankId,
  entries,
  groupState,
  allAccounts,
  leafBankAccounts,
  onImport,
  onAccountChange,
  onAccountCreated,
  onRemoveEntry,
  onReview,
}: {
  bankId: string;
  entries: FileEntry[];
  groupState: GroupState | undefined;
  allAccounts: Account[];
  leafBankAccounts: Account[];
  onImport: (accountId: string) => void;
  onAccountChange: (accountId: string) => void;
  onAccountCreated: (account: Account) => void;
  onRemoveEntry: (id: string) => void;
  onReview: (accountId: number) => void;
}) {
  const queryClient = useQueryClient();
  const detection = entries.find((e) => e.detection)?.detection ?? null;
  const [showCreate, setShowCreate] = useState(leafBankAccounts.length === 0);
  const [newName, setNewName] = useState(detection?.name_hint ?? "");

  const { path: parentPath, type: accountType } = statementParent(detection);
  const parentAccount = allAccounts.find((a) => a.full_path === parentPath);

  const phase = groupState?.phase ?? "confirming";
  const selectedAccountId = groupState?.accountId ?? "";
  const isImporting = phase === "importing";
  const isDone = phase === "done";
  const hasError = phase === "error";

  const sortedEntries = useMemo(
    () =>
      [...entries].sort((a, b) => {
        const aDate = a.detection?.period_start ?? a.file.name;
        const bDate = b.detection?.period_start ?? b.file.name;
        return aDate.localeCompare(bDate);
      }),
    [entries],
  );

  const createMutation = useMutation({
    mutationFn: async () => {
      if (!detection) throw new Error("No detection data");
      const suffix = sanitizeIdentifier(detection.bank_identifier) || slugify(newName);
      const account = await accountsApi.create({
        name: newName.trim(),
        full_path: parentAccount
          ? `${parentAccount.full_path}:${suffix}`
          : slugify(newName),
        parent_id: parentAccount?.id ?? null,
        type: accountType,
        currency: "GBP",
        bank_identifier: detection.bank_identifier,
      });
      if (detection.opening_balance && detection.period_start) {
        await accountsApi.setOpeningBalance(account.id, {
          amount: detection.opening_balance,
          date: detection.period_start,
        });
      }
      return account;
    },
    onSuccess: (account) => {
      queryClient.invalidateQueries({ queryKey: ["accounts"] });
      onAccountCreated(account);
      setShowCreate(false);
    },
  });

  return (
    <div className="rounded-lg border p-4 space-y-3">
      {/* Group header */}
      <div className="flex items-start gap-3">
        <div className="flex-1 space-y-0.5">
          <div className="flex items-center gap-2">
            <CheckCircle2 className="h-4 w-4 text-green-600 shrink-0" />
            <span className="font-medium text-sm">{detection?.label ?? bankId}</span>
          </div>
          <p className="text-xs text-muted-foreground pl-6">
            Identifier: <code className="font-mono">{bankId}</code>
          </p>
        </div>
        <Badge variant="outline">
          {entries.length} statement{entries.length !== 1 ? "s" : ""}
        </Badge>
      </div>

      {/* Per-file list */}
      <div className="rounded-md bg-muted/30 px-3 py-2 space-y-1.5">
        {sortedEntries.map((e) => (
          <div key={e.id} className="flex items-center gap-2 text-xs">
            <FileText className="h-3.5 w-3.5 text-muted-foreground shrink-0" />
            <span className="flex-1 truncate text-muted-foreground">{e.file.name}</span>
            {e.detection?.period_start && e.detection?.period_end && (
              <span className="text-muted-foreground shrink-0">
                {formatDate(e.detection.period_start)} →{" "}
                {formatDate(e.detection.period_end)}
              </span>
            )}
            <PhaseIndicator phase={e.phase} />
            {!isImporting && !isDone && e.phase !== "uploading" && e.phase !== "done" && (
              <Button
                variant="ghost"
                size="icon"
                className="h-5 w-5 shrink-0"
                onClick={() => onRemoveEntry(e.id)}
              >
                <X className="h-3 w-3" />
              </Button>
            )}
          </div>
        ))}
      </div>

      {/* Done */}
      {isDone && groupState?.importedAccountId != null && (
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2 text-sm text-green-700">
            <CheckCircle2 className="h-4 w-4" />
            Imported successfully
          </div>
          <Button
            size="sm"
            variant="outline"
            onClick={() => onReview(groupState.importedAccountId!)}
          >
            <Eye className="h-3.5 w-3.5 mr-1" />
            Review
          </Button>
        </div>
      )}

      {/* Error */}
      {hasError && groupState?.error && <ErrorMessage message={groupState.error} />}

      {/* Importing */}
      {isImporting && (
        <p className="text-xs text-muted-foreground animate-pulse">
          Uploading statements…
        </p>
      )}

      {/* Account selection */}
      {!isDone && !isImporting && (
        <div className="space-y-3">
          <div>
            <label className="text-sm font-medium">Bank Account</label>
            {!selectedAccountId && (
              <p className="text-xs text-muted-foreground mb-1.5">
                {leafBankAccounts.length > 0
                  ? "Select the account these statements belong to, or create a new one."
                  : "No accounts yet — create one below."}
              </p>
            )}
            {leafBankAccounts.length > 0 && (
              <Select value={selectedAccountId} onValueChange={onAccountChange}>
                <SelectTrigger>
                  <SelectValue placeholder="Select bank account…" />
                </SelectTrigger>
                <SelectContent>
                  {leafBankAccounts.map((a) => (
                    <SelectItem key={a.id} value={String(a.id)}>
                      {accountLabel(a)}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            )}
          </div>

          {!showCreate && leafBankAccounts.length > 0 && (
            <button
              className="text-xs text-muted-foreground underline"
              onClick={() => setShowCreate(true)}
            >
              Or create a new account
            </button>
          )}

          {showCreate && (
            <div className="space-y-2 rounded-lg border p-3">
              <p className="text-sm font-medium">New account</p>
              <p className="text-xs text-muted-foreground">
                Will be created under{" "}
                <span className="font-medium">{parentAccount?.name ?? parentPath}</span>
              </p>
              <div>
                <label className="text-xs font-medium text-muted-foreground">Name</label>
                <Input
                  placeholder="e.g. HSBC Current"
                  value={newName}
                  onChange={(e) => setNewName(e.target.value)}
                />
              </div>
              {detection?.bank_identifier && (
                <p className="text-xs text-muted-foreground">
                  Identifier{" "}
                  <code className="font-mono">{detection.bank_identifier}</code>{" "}
                  will be saved with this account.
                </p>
              )}
              {createMutation.isError && (
                <ErrorMessage message={createMutation.error.message} />
              )}
              <div className="flex gap-2">
                {leafBankAccounts.length > 0 && (
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => setShowCreate(false)}
                  >
                    Cancel
                  </Button>
                )}
                <Button
                  size="sm"
                  onClick={() => createMutation.mutate()}
                  disabled={createMutation.isPending || !newName.trim()}
                >
                  <Plus className="h-3.5 w-3.5 mr-1" />
                  {createMutation.isPending ? "Creating…" : "Create & select"}
                </Button>
              </div>
            </div>
          )}

          <Button
            onClick={() => onImport(selectedAccountId)}
            disabled={!selectedAccountId}
            className="w-full"
          >
            <Upload className="h-4 w-4 mr-2" />
            Import {entries.length} statement{entries.length !== 1 ? "s" : ""}
          </Button>
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// FileCard — per-file upload card (CSV and undetected files)
// ---------------------------------------------------------------------------

function FileCard({
  entry,
  allAccounts,
  leafBankAccounts,
  onAccountChange,
  onCsvChange,
  onImport,
  onRemove,
  onReview,
  onAccountCreated,
}: {
  entry: FileEntry;
  allAccounts: Account[];
  leafBankAccounts: Account[];
  onAccountChange: (id: string) => void;
  onCsvChange: (csv: CsvPreview, columns: CsvColumns) => void;
  onImport: () => void;
  onRemove: () => void;
  onReview: () => void;
  onAccountCreated: (account: Account) => void;
}) {
  const queryClient = useQueryClient();
  const [showCreate, setShowCreate] = useState(leafBankAccounts.length === 0);
  const [newName, setNewName] = useState(entry.detection?.name_hint ?? "");
  const [newGroupPath, setNewGroupPath] = useState(ACCOUNT_GROUPS[0].path);
  const [sortCode, setSortCode] = useState("");
  const [accountNumber, setAccountNumber] = useState("");
  const [cardLast4, setCardLast4] = useState("");

  const idContext = getBankIdentifierContext(newGroupPath);

  // For statements: determine parent path from detection
  const { path: statementParentPath, type: accountType } = statementParent(entry.detection);
  const parentPath = entry.detection ? statementParentPath : null;
  const parentAccount = parentPath
    ? allAccounts.find((a) => a.full_path === parentPath)
    : null;

  // Opening balance (only relevant for a detected statement with an account selected)
  const selectedAccountIdNum = entry.accountId ? Number(entry.accountId) : null;
  const openingBalanceQuery = useQuery({
    queryKey: ["accounts", selectedAccountIdNum, "opening-balance"],
    queryFn: () =>
      selectedAccountIdNum ? accountsApi.getOpeningBalance(selectedAccountIdNum) : null,
    enabled: !!selectedAccountIdNum && entry.phase === "confirming" && !!entry.detection,
  });

  const setOpeningBalanceMutation = useMutation({
    mutationFn: ({ id, amount, date }: { id: number; amount: string; date: string }) =>
      accountsApi.setOpeningBalance(id, { amount, date }),
    onSuccess: (_data, vars) => {
      queryClient.invalidateQueries({
        queryKey: ["accounts", vars.id, "opening-balance"],
      });
    },
  });

  const createMutation = useMutation({
    mutationFn: async () => {
      if (entry.detection) {
        // Detected statement flow
        const suffix =
          sanitizeIdentifier(entry.detection.bank_identifier) || slugify(newName);
        const account = await accountsApi.create({
          name: newName.trim(),
          full_path: parentAccount
            ? `${parentAccount.full_path}:${suffix}`
            : slugify(newName),
          parent_id: parentAccount?.id ?? null,
          type: accountType,
          currency: "GBP",
          bank_identifier: entry.detection.bank_identifier,
        });
        if (entry.detection.opening_balance && entry.detection.period_start) {
          await accountsApi.setOpeningBalance(account.id, {
            amount: entry.detection.opening_balance,
            date: entry.detection.period_start,
          });
        }
        return account;
      } else {
        // Manual flow (e.g. CSV)
        const group = ACCOUNT_GROUPS.find((g) => g.path === newGroupPath)!;
        const parentId = await ensureParentChain(group.path, group.type, allAccounts);
        let bankIdentifier: string | null = null;
        if (idContext === "sort_code" && (sortCode || accountNumber)) {
          bankIdentifier = `${sortCode} ${accountNumber}`.trim();
        } else if (idContext === "card_last4" && cardLast4) {
          bankIdentifier = cardLast4;
        }
        const suffix = bankIdentifier
          ? sanitizeIdentifier(bankIdentifier)
          : slugify(newName);
        return accountsApi.create({
          name: newName.trim(),
          full_path: `${group.path}:${suffix}`,
          parent_id: parentId,
          type: group.type,
          currency: "GBP",
          bank_identifier: bankIdentifier,
        });
      }
    },
    onSuccess: (account) => {
      onAccountCreated(account);
      setShowCreate(false);
    },
  });

  // Opening balance recommendation
  const stored = openingBalanceQuery.data;
  const stmtPeriodStart = entry.detection?.period_start ?? null;
  const recommend = (() => {
    if (!entry.detection?.opening_balance || !stmtPeriodStart || !selectedAccountIdNum)
      return null;
    if (openingBalanceQuery.isLoading) return null;
    if (!stored) return { kind: "set" as const };
    if (stmtPeriodStart < stored.date) return { kind: "update" as const, current: stored };
    return null;
  })();

  const phaseLabel: Record<FilePhase, string> = {
    detecting: "Detecting…",
    confirming: "Needs account",
    queued: "Ready",
    uploading: "Uploading…",
    done: "Imported",
    error: "Error",
  };

  const phaseBadgeClass: Record<FilePhase, string> = {
    detecting: "",
    confirming: "",
    queued: "bg-green-100 text-green-800 border-green-200",
    uploading: "",
    done: "bg-green-100 text-green-800 border-green-200",
    error: "bg-destructive/10 text-destructive border-destructive/20",
  };

  const canRemove = entry.phase !== "uploading";
  // A CSV can't be imported until its columns are chosen
  const csvIncomplete = !!entry.csv && missingCsvFields(entry.csvColumns ?? {}).length > 0;

  return (
    <div className="rounded-lg border p-4 space-y-3">
      {/* Header row */}
      <div className="flex items-center gap-2">
        <FileText className="h-4 w-4 text-muted-foreground shrink-0" />
        <span className="text-sm font-medium truncate flex-1">{entry.file.name}</span>
        <span className="text-xs text-muted-foreground shrink-0">
          ({(entry.file.size / 1024).toFixed(1)} KB)
        </span>
        <Badge
          variant="outline"
          className={`shrink-0 ${phaseBadgeClass[entry.phase]}`}
        >
          {phaseLabel[entry.phase]}
        </Badge>
        {canRemove && (
          <Button
            variant="ghost"
            size="icon"
            className="shrink-0 h-7 w-7"
            onClick={onRemove}
          >
            <X className="h-3.5 w-3.5" />
          </Button>
        )}
      </div>

      {/* Detecting */}
      {entry.phase === "detecting" && (
        <p className="text-xs text-muted-foreground animate-pulse">
          Analysing statement…
        </p>
      )}

      {/* Statement detection banner */}
      {entry.detection && (entry.phase === "confirming" || entry.phase === "queued") && (
        <div className="flex items-center gap-2 rounded-md bg-green-50 border border-green-200 px-3 py-2">
          <CheckCircle2 className="h-4 w-4 text-green-600 shrink-0" />
          <div>
            <p className="text-xs font-semibold text-green-800">{entry.detection.label}</p>
            <p className="text-xs text-green-700">
              Identifier:{" "}
              <code className="font-mono">{entry.detection.bank_identifier}</code>
            </p>
          </div>
        </div>
      )}

      {/* Statement summary */}
      {entry.detection &&
        entry.phase !== "done" &&
        entry.phase !== "error" &&
        entry.phase !== "uploading" && (
          <StatementSummary detection={entry.detection} />
        )}

      {/* CSV: which column holds what */}
      {entry.csv && (entry.phase === "confirming" || entry.phase === "queued") && (
        <CsvMapping
          file={entry.file}
          preview={entry.csv}
          columns={entry.csvColumns ?? {}}
          onChange={onCsvChange}
        />
      )}

      {/* Queued: show matched account + Import button */}
      {entry.phase === "queued" && (
        <div className="flex items-center justify-between">
          <p className="text-xs text-muted-foreground">
            Account:{" "}
            <span className="font-medium text-foreground">
              {accountLabel(
                leafBankAccounts.find((a) => a.id === Number(entry.accountId)) ?? {
                  id: 0,
                  name: "Unknown",
                  full_path: "",
                  parent_id: null,
                  type: "asset",
                  currency: "GBP",
                },
              )}
            </span>
          </p>
          <Button size="sm" onClick={onImport} disabled={csvIncomplete}>
            <Upload className="h-3.5 w-3.5 mr-1" />
            Import
          </Button>
        </div>
      )}

      {/* Confirming: full account picker */}
      {entry.phase === "confirming" && (
        <div className="space-y-3">
          <div>
            <label className="text-sm font-medium">Bank Account</label>
            <p className="text-xs text-muted-foreground mb-1.5">
              {entry.detection
                ? "No account matched this statement. Select an existing account or create one."
                : "Which account is this statement for?"}
            </p>
            {leafBankAccounts.length > 0 ? (
              <Select value={entry.accountId} onValueChange={onAccountChange}>
                <SelectTrigger>
                  <SelectValue placeholder="Select bank account…" />
                </SelectTrigger>
                <SelectContent>
                  {leafBankAccounts.map((a) => (
                    <SelectItem key={a.id} value={String(a.id)}>
                      {accountLabel(a)}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            ) : (
              <p className="text-sm text-muted-foreground italic">
                No accounts yet — create one below.
              </p>
            )}
          </div>

          {/* Create account toggle */}
          {!showCreate && leafBankAccounts.length > 0 && (
            <button
              className="text-xs text-muted-foreground underline"
              onClick={() => setShowCreate(true)}
            >
              Or create a new account
            </button>
          )}

          {/* Create account form */}
          {showCreate && (
            <div className="space-y-2 rounded-lg border p-3">
              <p className="text-sm font-medium">New account</p>

              {/* CSV: account type selector */}
              {!entry.detection && (
                <div>
                  <label className="text-xs font-medium text-muted-foreground">
                    Account type
                  </label>
                  <Select value={newGroupPath} onValueChange={setNewGroupPath}>
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {ACCOUNT_GROUPS.map((g) => (
                        <SelectItem key={g.path} value={g.path}>
                          {g.label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
              )}

              {/* Detected statement: show parent path hint */}
              {entry.detection && (
                <p className="text-xs text-muted-foreground">
                  Will be created under{" "}
                  <span className="font-medium">
                    {parentAccount?.name ?? parentPath}
                  </span>
                </p>
              )}

              <div>
                <label className="text-xs font-medium text-muted-foreground">Name</label>
                <Input
                  placeholder="e.g. HSBC Current"
                  value={newName}
                  onChange={(e) => setNewName(e.target.value)}
                />
              </div>

              {!entry.detection && idContext === "sort_code" && (
                <div className="grid grid-cols-2 gap-2">
                  <div>
                    <label className="text-xs font-medium text-muted-foreground">
                      Sort code
                    </label>
                    <Input
                      placeholder="12-34-56"
                      value={sortCode}
                      onChange={(e) => setSortCode(e.target.value)}
                      maxLength={8}
                    />
                  </div>
                  <div>
                    <label className="text-xs font-medium text-muted-foreground">
                      Account number
                    </label>
                    <Input
                      placeholder="12345678"
                      value={accountNumber}
                      onChange={(e) => setAccountNumber(e.target.value)}
                      maxLength={8}
                    />
                  </div>
                  <p className="col-span-2 text-xs text-muted-foreground">
                    Used to identify this account in transfer descriptions. Optional.
                  </p>
                </div>
              )}

              {!entry.detection && idContext === "card_last4" && (
                <div>
                  <label className="text-xs font-medium text-muted-foreground">
                    Last 4 digits
                  </label>
                  <Input
                    placeholder="1234"
                    value={cardLast4}
                    onChange={(e) =>
                      setCardLast4(e.target.value.replace(/\D/g, "").slice(0, 4))
                    }
                    maxLength={4}
                    inputMode="numeric"
                  />
                </div>
              )}

              {entry.detection?.bank_identifier && (
                <p className="text-xs text-muted-foreground">
                  Identifier{" "}
                  <code className="font-mono">{entry.detection.bank_identifier}</code>{" "}
                  will be saved with this account.
                </p>
              )}

              {createMutation.isError && (
                <ErrorMessage message={createMutation.error.message} />
              )}

              <div className="flex gap-2">
                {leafBankAccounts.length > 0 && (
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => setShowCreate(false)}
                  >
                    Cancel
                  </Button>
                )}
                <Button
                  size="sm"
                  onClick={() => createMutation.mutate()}
                  disabled={createMutation.isPending || !newName.trim()}
                >
                  <Plus className="h-3.5 w-3.5 mr-1" />
                  {createMutation.isPending ? "Creating…" : "Create & select"}
                </Button>
              </div>
            </div>
          )}

          {/* Opening balance recommendation for existing accounts */}
          {recommend && selectedAccountIdNum && entry.detection?.opening_balance && stmtPeriodStart && (
            <OpeningBalanceRecommendation
              recommend={recommend}
              newAmount={entry.detection.opening_balance}
              newDate={stmtPeriodStart}
              accountCurrency={
                allAccounts.find((a) => a.id === selectedAccountIdNum)?.currency ?? "GBP"
              }
              onApply={() =>
                setOpeningBalanceMutation.mutate({
                  id: selectedAccountIdNum,
                  amount: entry.detection!.opening_balance!,
                  date: stmtPeriodStart,
                })
              }
              isPending={setOpeningBalanceMutation.isPending}
              isApplied={setOpeningBalanceMutation.isSuccess}
            />
          )}

          <Button
            onClick={onImport}
            disabled={!entry.accountId || csvIncomplete}
            className="w-full"
          >
            <Upload className="h-4 w-4 mr-2" />
            Import Transactions
          </Button>
        </div>
      )}

      {/* Uploading */}
      {entry.phase === "uploading" && (
        <p className="text-xs text-muted-foreground animate-pulse">Uploading…</p>
      )}

      {/* Done */}
      {entry.phase === "done" && (
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2 text-sm text-green-700">
            <CheckCircle2 className="h-4 w-4" />
            Imported successfully
          </div>
          <Button size="sm" variant="outline" onClick={onReview}>
            <Eye className="h-3.5 w-3.5 mr-1" />
            Review
          </Button>
        </div>
      )}
      {entry.phase === "done" && entry.aiNote && (
        <p className="text-xs text-muted-foreground">{entry.aiNote}</p>
      )}

      {/* Error */}
      {entry.phase === "error" && entry.error && (
        <ErrorMessage message={entry.error} />
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Statement supporting components
// ---------------------------------------------------------------------------

function StatementSummary({ detection }: { detection: StatementDetection }) {
  const hasAny =
    detection.period_start ||
    detection.period_end ||
    detection.opening_balance ||
    detection.closing_balance;
  if (!hasAny) return null;

  const opening = detection.opening_balance ? parseFloat(detection.opening_balance) : null;
  const closing = detection.closing_balance ? parseFloat(detection.closing_balance) : null;
  const balanceLabel =
    detection.statement_type === "credit_card" ? "Previous balance" : "Opening balance";
  const closingLabel =
    detection.statement_type === "credit_card" ? "New balance" : "Closing balance";

  return (
    <div className="rounded-md border bg-muted/30 px-4 py-3 text-sm">
      <p className="text-xs uppercase tracking-wide text-muted-foreground mb-2">
        Statement summary
      </p>
      <div className="grid grid-cols-2 gap-x-4 gap-y-1.5">
        {detection.period_start && detection.period_end && (
          <>
            <span className="text-muted-foreground">Period</span>
            <span className="text-right">
              {formatDate(detection.period_start)} → {formatDate(detection.period_end)}
            </span>
          </>
        )}
        {opening !== null && (
          <>
            <span className="text-muted-foreground">{balanceLabel}</span>
            <span className="text-right font-mono">{formatCurrency(opening)}</span>
          </>
        )}
        {closing !== null && (
          <>
            <span className="text-muted-foreground">{closingLabel}</span>
            <span className="text-right font-mono">{formatCurrency(closing)}</span>
          </>
        )}
      </div>
    </div>
  );
}

function OpeningBalanceRecommendation({
  recommend,
  newAmount,
  newDate,
  accountCurrency,
  onApply,
  isPending,
  isApplied,
}: {
  recommend: { kind: "set" } | { kind: "update"; current: OpeningBalance };
  newAmount: string;
  newDate: string;
  accountCurrency: string;
  onApply: () => void;
  isPending: boolean;
  isApplied: boolean;
}) {
  const newAmountNum = parseFloat(newAmount);

  if (isApplied) {
    return (
      <div className="flex items-center gap-2 rounded-md bg-green-50 border border-green-200 px-4 py-3 text-sm text-green-800">
        <CheckCircle2 className="h-4 w-4 shrink-0" />
        Opening balance set to {formatCurrency(newAmountNum, accountCurrency)} as of{" "}
        {formatDate(newDate)}.
      </div>
    );
  }

  return (
    <div className="rounded-md bg-amber-50 border border-amber-200 px-4 py-3 text-sm">
      <p className="font-medium text-amber-900">
        {recommend.kind === "set"
          ? "Set the account's opening balance from this statement?"
          : "This statement is older than the current opening balance"}
      </p>
      <p className="text-xs text-amber-800 mt-1">
        {recommend.kind === "set" ? (
          <>
            This account has no opening balance recorded. Use{" "}
            <span className="font-mono">
              {formatCurrency(newAmountNum, accountCurrency)}
            </span>{" "}
            as of {formatDate(newDate)}?
          </>
        ) : (
          <>
            Currently:{" "}
            <span className="font-mono">
              {formatCurrency(parseFloat(recommend.current.amount), accountCurrency)}
            </span>{" "}
            on {formatDate(recommend.current.date)}. Replace with{" "}
            <span className="font-mono">
              {formatCurrency(newAmountNum, accountCurrency)}
            </span>{" "}
            as of {formatDate(newDate)}?
          </>
        )}
      </p>
      <Button
        variant="outline"
        size="sm"
        className="mt-2"
        onClick={onApply}
        disabled={isPending}
      >
        {isPending
          ? "Applying…"
          : recommend.kind === "set"
            ? "Set opening balance"
            : "Update opening balance"}
      </Button>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

function PhaseIndicator({ phase }: { phase: FilePhase }) {
  if (phase === "uploading")
    return <span className="text-muted-foreground animate-pulse shrink-0">Uploading…</span>;
  if (phase === "done")
    return <CheckCircle2 className="h-3.5 w-3.5 text-green-600 shrink-0" />;
  if (phase === "error")
    return <AlertCircle className="h-3.5 w-3.5 text-destructive shrink-0" />;
  return null;
}

function ErrorMessage({ message }: { message: string }) {
  return (
    <div className="flex items-center gap-2 text-sm text-destructive bg-destructive/10 rounded-md p-3">
      <AlertCircle className="h-4 w-4 shrink-0" />
      {message}
    </div>
  );
}

function batchTitle(batch: ImportBatch): string {
  if (!batch.period_start || !batch.period_end) return batch.file_name || `Import #${batch.id}`;
  // Built from the parts, in local time: new Date("2026-07-31") would be UTC midnight.
  const local = (iso: string) => {
    const [y, m, d] = iso.split("-").map(Number);
    return new Date(y, m - 1, d);
  };
  const start = local(batch.period_start);
  const end = local(batch.period_end);
  const monthEnd = new Date(start.getFullYear(), start.getMonth() + 1, 0);
  if (start.getDate() === 1 && end.getTime() === monthEnd.getTime()) {
    return `${start.toLocaleDateString("en-GB", { month: "short", year: "numeric" })} statement`;
  }
  const from = start.toLocaleDateString("en-GB", {
    day: "numeric",
    month: "short",
    ...(start.getFullYear() !== end.getFullYear() ? { year: "numeric" } : {}),
  });
  return `${from} – ${formatDate(batch.period_end)}`;
}

function accountHeading(batch: ImportBatch): string {
  const name = batch.account_name ?? formatAccountPath(batch.account_path);
  const hint = batch.account_number_hint;
  return hint && !name.includes(hint) ? `${name} ··${hint}` : name;
}

function BatchRow({
  batch,
  onReview,
  onDelete,
}: {
  batch: ImportBatch;
  onReview: () => void;
  onDelete: () => void;
}) {
  const [menuOpen, setMenuOpen] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const title = batchTitle(batch);
  const showFileName = Boolean(batch.file_name) && title !== batch.file_name;

  return (
    <div className="flex items-start rounded-lg border transition-colors hover:bg-accent/50">
      {/* The whole row opens the review; the menu sits beside it rather than inside it. */}
      <button type="button" onClick={onReview} className="flex min-w-0 flex-1 items-start gap-3 p-3 text-left">
        <FileText className="mt-0.5 h-5 w-5 shrink-0 text-muted-foreground" />
        <span className="min-w-0 flex-1 space-y-0.5">
          <span className="block truncate font-medium" title={batch.file_name ?? undefined}>
            {title}
          </span>
          <span className="flex flex-wrap items-center gap-x-1.5 text-sm text-muted-foreground">
            {batch.pending_count > 0 ? (
              <span className="font-medium text-amber-700">{batch.pending_count} to review</span>
            ) : (
              <span className="inline-flex items-center gap-1">
                <Check className="h-3.5 w-3.5" /> All reviewed
              </span>
            )}
            <span aria-hidden>·</span>
            <span>
              {batch.transaction_count} transaction{batch.transaction_count !== 1 ? "s" : ""}
            </span>
          </span>
          <span className="flex gap-1 text-xs text-muted-foreground">
            {showFileName && <span className="min-w-0 truncate">{batch.file_name} ·</span>}
            <span className="shrink-0">Imported {formatDate(batch.imported_at)}</span>
          </span>
        </span>
        <ChevronRight className="mt-3 h-4 w-4 shrink-0 text-muted-foreground" />
      </button>
      <Popover open={menuOpen} onOpenChange={setMenuOpen}>
        <PopoverTrigger asChild>
          <Button variant="ghost" size="icon" className="m-1.5 shrink-0" aria-label={`More for ${title}`}>
            <MoreHorizontal className="h-4 w-4" />
          </Button>
        </PopoverTrigger>
        <PopoverContent align="end" className="w-52 p-1">
          <a
            href={importsApi.getBatchFileUrl(batch.id)}
            target="_blank"
            rel="noopener noreferrer"
            onClick={() => setMenuOpen(false)}
            className="flex items-center gap-2 rounded-sm px-2 py-1.5 text-sm hover:bg-accent"
          >
            <FileDown className="h-4 w-4" /> Download statement
          </a>
          <button
            type="button"
            onClick={() => {
              setMenuOpen(false);
              setConfirming(true);
            }}
            className="flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-sm text-destructive hover:bg-accent"
          >
            <Trash2 className="h-4 w-4" /> Delete import
          </button>
        </PopoverContent>
      </Popover>
      <Dialog open={confirming} onOpenChange={setConfirming}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Delete {title}?</DialogTitle>
            <DialogDescription>
              This removes the import and its {batch.transaction_count} transaction
              {batch.transaction_count !== 1 ? "s" : ""}. Rules and categories stay as they are.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" onClick={() => setConfirming(false)}>
              Cancel
            </Button>
            <Button
              variant="destructive"
              onClick={() => {
                setConfirming(false);
                onDelete();
              }}
            >
              Delete
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
