import { useState, useRef } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { rulesApi, accountsApi, lookupsApi, type Account, type Rule } from "../api/client";
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
import { Plus, Trash2, Pencil, Download, Upload, AlertCircle, ChevronLeft, ChevronRight, Bot } from "lucide-react";
import { formatAccountPath } from "../lib/utils";
import { useRouter } from "@tanstack/react-router";

export function RulesPage() {
  const queryClient = useQueryClient();
  const router = useRouter();
  const [showCreateDialog, setShowCreateDialog] = useState(false);
  const [editingRule, setEditingRule] = useState<Rule | null>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);

  const { data: rules = [], isLoading } = useQuery({
    queryKey: ["rules"],
    queryFn: rulesApi.list,
  });

  const { data: accounts = [] } = useQuery({
    queryKey: ["accounts"],
    queryFn: accountsApi.list,
  });

  const { data: matchTypes = [] } = useQuery({
    queryKey: ["match-types"],
    queryFn: lookupsApi.matchTypes,
  });

  const deleteMutation = useMutation({
    mutationFn: rulesApi.delete,
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["rules"] }),
  });

  const importMutation = useMutation({
    mutationFn: rulesApi.import,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["rules"] });
    },
  });

  function handleExport() {
    rulesApi.export().then((data) => {
      const blob = new Blob([JSON.stringify(data, null, 2)], { type: "application/json" });
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = "pennychest-rules.json";
      document.body.appendChild(a);
      a.click();
      document.body.removeChild(a);
      setTimeout(() => URL.revokeObjectURL(url), 100);
    });
  }

  function handleImportFile(e: React.ChangeEvent<HTMLInputElement>) {
    const file = e.target.files?.[0];
    if (!file) return;
    const reader = new FileReader();
    reader.onload = (ev) => {
      try {
        const data = JSON.parse(ev.target?.result as string);
        if (Array.isArray(data)) {
          importMutation.mutate(data);
        }
      } catch {
        // Invalid JSON
      }
    };
    reader.readAsText(file);
  }

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2">
          <Button variant="ghost" size="icon" onClick={() => router.history.back()}>
            <ChevronLeft className="h-5 w-5" />
          </Button>
          <h1 className="text-3xl font-bold">Rules</h1>
        </div>
        <div className="flex gap-2 shrink-0">
          <Button variant="outline" onClick={handleExport} title="Export rules as JSON" aria-label="Export rules">
            <Download className="h-4 w-4 sm:mr-1" />
            <span className="hidden sm:inline">Export</span>
          </Button>
          <Button
            variant="outline"
            onClick={() => fileInputRef.current?.click()}
            title="Import rules from JSON"
            aria-label="Import rules"
          >
            <Upload className="h-4 w-4 sm:mr-1" />
            <span className="hidden sm:inline">Import</span>
          </Button>
          <input
            ref={fileInputRef}
            type="file"
            accept=".json"
            onChange={handleImportFile}
            className="hidden"
          />
          <Button onClick={() => setShowCreateDialog(true)} aria-label="New rule">
            <Plus className="h-4 w-4 sm:mr-1" />
            <span className="hidden sm:inline">New Rule</span>
          </Button>
        </div>
      </div>

      <p className="text-muted-foreground">
        Rules auto-categorise transactions at import time. Higher priority rules take precedence.
      </p>

      {importMutation.isSuccess && (
        <div className="text-sm text-green-600 bg-green-50 rounded-md p-3">
          Imported {importMutation.data.imported} rules, skipped {importMutation.data.skipped}.
        </div>
      )}

      {importMutation.isError && (
        <div className="flex items-center gap-2 text-sm text-destructive bg-destructive/10 rounded-md p-3">
          <AlertCircle className="h-4 w-4" />
          {importMutation.error.message}
        </div>
      )}

      {isLoading ? (
        <div className="text-center py-12 text-muted-foreground">Loading...</div>
      ) : rules.length === 0 ? (
        <Card>
          <CardContent className="py-12 text-center text-muted-foreground">
            No rules defined yet. Create rules to auto-categorise your transactions.
          </CardContent>
        </Card>
      ) : (
        <div className="space-y-2">
          {rules.map((rule) => (
            <div
              key={rule.id}
              className="flex items-center gap-3 rounded-lg border p-3 hover:bg-accent/50 transition-colors"
            >
              <div className="flex-1 min-w-0">
                <div className="flex items-center gap-2 flex-wrap">
                  <span className="font-mono font-medium">{rule.pattern}</span>
                  <Badge variant="secondary">{rule.match_type_name ?? "?"}</Badge>
                  <Badge variant="outline">Priority: {rule.priority}</Badge>
                  {rule.source === "ai" && (
                    <Badge variant="outline" className="text-violet-600 border-violet-300 gap-1">
                      <Bot className="h-3 w-3" /> AI
                    </Badge>
                  )}
                  {rule.source === "import" && (
                    <Badge variant="outline" className="text-blue-600 border-blue-300">Imported</Badge>
                  )}
                </div>
                <div className="text-sm text-muted-foreground mt-0.5">
                  → {formatAccountPath(rule.target_account_path)}
                </div>
                {rule.description && (
                  <div className="text-xs text-muted-foreground italic mt-0.5">{rule.description}</div>
                )}
              </div>
              <div className="flex gap-1">
                <Button
                  variant="ghost"
                  size="icon"
                  onClick={() => setEditingRule(rule)}
                  title="Edit"
                >
                  <Pencil className="h-4 w-4" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  onClick={() => deleteMutation.mutate(rule.id)}
                  title="Delete"
                >
                  <Trash2 className="h-4 w-4 text-destructive" />
                </Button>
              </div>
            </div>
          ))}
        </div>
      )}

      <RuleDialog
        open={showCreateDialog}
        onOpenChange={setShowCreateDialog}
        accounts={accounts}
        matchTypes={matchTypes}
        rule={null}
      />

      {editingRule && (
        <RuleDialog
          open={true}
          onOpenChange={(open) => { if (!open) setEditingRule(null); }}
          accounts={accounts}
          matchTypes={matchTypes}
          rule={editingRule}
        />
      )}
    </div>
  );
}

function RuleDialog({
  open,
  onOpenChange,
  accounts,
  matchTypes,
  rule,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  accounts: Account[];
  matchTypes: { id: number; name: string }[];
  rule: Rule | null;
}) {
  const queryClient = useQueryClient();
  const [pattern, setPattern] = useState(rule?.pattern ?? "");
  const [chosenMatchTypeId, setMatchTypeId] = useState(String(rule?.match_type_id ?? ""));
  // The dialog can mount before the match types load, so default to the first once they have
  const matchTypeId = chosenMatchTypeId || String(matchTypes[0]?.id ?? "");
  const [targetAccountId, setTargetAccountId] = useState(String(rule?.target_account_id ?? ""));
  const [priority, setPriority] = useState(String(rule?.priority ?? 0));
  const [pickingCategory, setPickingCategory] = useState(false);

  const isCategory = (a: Account) => a.type === "expense" || a.type === "income";
  const target = accounts.find((a) => String(a.id) === targetAccountId);
  // Rules are usually about categorising, but transfer-style rules can still point at an account
  const [targetIsAccount, setTargetIsAccount] = useState(!!target && !isCategory(target));

  function switchTargetKind() {
    setTargetIsAccount(!targetIsAccount);
    setTargetAccountId("");
  }

  // Live preview
  const previewQuery = useQuery({
    queryKey: ["rule-preview", pattern, matchTypeId],
    queryFn: () =>
      rulesApi.preview({
        pattern,
        match_type_id: parseInt(matchTypeId),
        exclude_rule_id: rule?.id,
      }),
    enabled: open && pattern.length >= 2 && !!matchTypeId,
  });

  const createMutation = useMutation({
    mutationFn: (data: Omit<Rule, "id" | "match_type_name" | "target_account_path">) => rulesApi.create(data),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["rules"] });
      onOpenChange(false);
    },
  });

  const updateMutation = useMutation({
    mutationFn: (data: Partial<Rule>) => rulesApi.update(rule!.id, data),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["rules"] });
      onOpenChange(false);
    },
  });

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!pattern || !matchTypeId || !targetAccountId) return;

    const data = {
      pattern,
      match_type_id: parseInt(matchTypeId),
      target_account_id: parseInt(targetAccountId),
      priority: parseInt(priority) || 0,
      source: null,
      description: null,
    };

    if (rule) {
      updateMutation.mutate(data);
    } else {
      createMutation.mutate(data);
    }
  }

  const mutation = rule ? updateMutation : createMutation;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-lg">
        <DialogHeader>
          <DialogTitle>{rule ? "Edit Rule" : "New Rule"}</DialogTitle>
          <DialogDescription>
            {rule
              ? "Update the rule's pattern and category."
              : "Create a rule to auto-categorise matching transactions."}
          </DialogDescription>
        </DialogHeader>
        <form onSubmit={handleSubmit} className="space-y-4">
          <div>
            <label className="text-sm font-medium">Pattern</label>
            <Input
              placeholder="e.g. TESCO"
              value={pattern}
              onChange={(e) => setPattern(e.target.value)}
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
            <label className="text-sm font-medium">{targetIsAccount ? "Account" : "Category"}</label>
            {targetIsAccount ? (
              <Select value={targetAccountId} onValueChange={setTargetAccountId}>
                <SelectTrigger>
                  <SelectValue placeholder="Select account..." />
                </SelectTrigger>
                <SelectContent>
                  {accounts.filter((a) => !isCategory(a)).map((a) => (
                    <SelectItem key={a.id} value={String(a.id)}>
                      {formatAccountPath(a.full_path)}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            ) : (
              <button
                type="button"
                onClick={() => setPickingCategory(true)}
                className="flex h-9 w-full items-center gap-2 rounded-md border border-input bg-transparent px-3 text-sm shadow-sm hover:bg-accent focus:outline-none focus:ring-1 focus:ring-ring"
              >
                {target ? (
                  <>
                    <AccountIcon account={target} accounts={accounts} className="h-4 w-4 text-primary shrink-0" />
                    <span className="truncate">{formatAccountPath(target.full_path)}</span>
                  </>
                ) : (
                  <span className="text-muted-foreground">Choose a category...</span>
                )}
                <ChevronRight className="ml-auto h-4 w-4 text-muted-foreground shrink-0" />
              </button>
            )}
            <button
              type="button"
              onClick={switchTargetKind}
              className="mt-1 text-xs text-muted-foreground underline-offset-2 hover:underline"
            >
              {targetIsAccount ? "Pick a category instead" : "Pick an account instead"}
            </button>
          </div>

          {/* Live preview */}
          {previewQuery.data && pattern.length >= 2 && (
            <div className="rounded-md border p-3 space-y-2">
              <p className="text-sm font-medium">
                Preview: {previewQuery.data.match_count} matching transaction
                {previewQuery.data.match_count !== 1 ? "s" : ""}
              </p>
              {previewQuery.data.matching_transactions.length > 0 && (
                <div className="max-h-28 overflow-y-auto space-y-1">
                  {previewQuery.data.matching_transactions.slice(0, 8).map((t) => (
                    <p key={t.id} className="text-xs text-muted-foreground truncate">
                      {t.description}
                    </p>
                  ))}
                  {previewQuery.data.match_count > 8 && (
                    <p className="text-xs text-muted-foreground">
                      ...and {previewQuery.data.match_count - 8} more
                    </p>
                  )}
                </div>
              )}
              {previewQuery.data.conflicts.length > 0 && (
                <div className="border-t pt-2">
                  <p className="text-xs font-medium text-amber-600">
                    Overlaps with {previewQuery.data.conflicts.length} existing rule
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

          {mutation.isError && (
            <p className="text-sm text-destructive">{mutation.error.message}</p>
          )}
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={mutation.isPending}>
              {mutation.isPending ? "Saving..." : rule ? "Update" : "Create"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
      {pickingCategory && (
        <CategoryPicker
          open={true}
          onOpenChange={setPickingCategory}
          description={pattern ? `Transactions matching "${pattern}"` : undefined}
          accounts={accounts}
          moneyIn={target?.type === "income"}
          currentAccountId={target?.id}
          onPick={(id) => {
            setTargetAccountId(String(id));
            setPickingCategory(false);
          }}
        />
      )}
    </Dialog>
  );
}
