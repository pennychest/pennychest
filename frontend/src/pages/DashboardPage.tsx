import { useState } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { ArrowLeftRight, Check, LayoutGrid, Plus, RotateCcw } from "lucide-react";
import { dashboardApi, importsApi, setupApi, type Widget, type WidgetType } from "../api/client";
import { WidgetCard } from "../components/dashboard/WidgetCard";
import { WIDGETS } from "../components/dashboard/widgets";
import { Button } from "../components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "../components/ui/card";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "../components/ui/select";
import { formatCurrency } from "../lib/utils";

const LAYOUT_KEY = ["dashboard-layout"];

function newId(): string {
  return Math.random().toString(36).slice(2, 10);
}

export function DashboardPage() {
  const queryClient = useQueryClient();

  const { data: setupStatus } = useQuery({
    queryKey: ["setup-status"],
    queryFn: setupApi.status,
  });

  const { data: pendingTransfers } = useQuery({
    queryKey: ["pending-transfers-balance"],
    queryFn: importsApi.getPendingTransfersBalance,
  });

  const [editing, setEditing] = useState(false);
  const layout = useQuery({ queryKey: LAYOUT_KEY, queryFn: dashboardApi.layout });
  const widgets = layout.data?.widgets ?? [];
  const save = useMutation({
    mutationFn: dashboardApi.save,
    // Show the change straight away; the server's copy replaces it once saved.
    onMutate: (next) => queryClient.setQueryData(LAYOUT_KEY, next),
    onSuccess: (saved) => queryClient.setQueryData(LAYOUT_KEY, saved),
  });
  const reset = useMutation({
    mutationFn: dashboardApi.reset,
    onSuccess: (saved) => queryClient.setQueryData(LAYOUT_KEY, saved),
  });
  const update = (next: Widget[]) => save.mutate({ widgets: next });
  const add = (type: WidgetType) =>
    update([...widgets, { id: newId(), type, settings: { ...WIDGETS[type].defaults } }]);

  const seedMutation = useMutation({
    mutationFn: setupApi.seed,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["setup-status"] });
      queryClient.invalidateQueries({ queryKey: ["accounts"] });
      queryClient.invalidateQueries({ queryKey: ["rules"] });
    },
  });


  return (
    <div className="space-y-6">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h1 className="text-3xl font-bold">Dashboard</h1>
        <div className="flex gap-2">
          {editing && (
            <Button variant="ghost" size="sm" onClick={() => reset.mutate()} disabled={reset.isPending}>
              <RotateCcw className="h-4 w-4" /> Reset
            </Button>
          )}
          <Button variant={editing ? "default" : "outline"} size="sm" onClick={() => setEditing((e) => !e)}>
            {editing ? <Check className="h-4 w-4" /> : <LayoutGrid className="h-4 w-4" />}
            {editing ? "Done" : "Customise"}
          </Button>
        </div>
      </div>

      {setupStatus && !setupStatus.has_accounts && (
        <Card className="border-blue-200 bg-blue-50">
          <CardContent className="p-6">
            <h3 className="font-semibold mb-2">Welcome to PennyChest</h3>
            <p className="text-sm text-muted-foreground mb-4">
              Get started by loading the UK default chart of accounts and rules.
              You can customise these later.
            </p>
            <Button
              onClick={() => seedMutation.mutate()}
              disabled={seedMutation.isPending}
            >
              {seedMutation.isPending ? "Loading..." : "Load UK Defaults"}
            </Button>
            {seedMutation.isSuccess && (
              <p className="text-sm text-green-600 mt-2">
                Created {seedMutation.data.accounts_created} accounts and{" "}
                {seedMutation.data.rules_created} rules
              </p>
            )}
          </CardContent>
        </Card>
      )}

      {pendingTransfers && parseFloat(pendingTransfers.balance) !== 0 && (
        <Card className="border-blue-200 bg-blue-50/50">
          <CardHeader>
            <CardTitle className="text-base flex items-center gap-2">
              <ArrowLeftRight className="h-4 w-4 text-blue-600" />
              Unresolved Transfers
            </CardTitle>
          </CardHeader>
          <CardContent>
            <p className="text-sm text-muted-foreground">
              <span className="font-semibold text-foreground">
                {formatCurrency(Math.abs(parseFloat(pendingTransfers.balance)))}
              </span>{" "}
              is sitting in Transfers:Pending.{" "}
              {pendingTransfers.unlinked_count > 0 && (
                <>
                  <span className="font-semibold text-foreground">
                    {pendingTransfers.unlinked_count}
                  </span>{" "}
                  transaction{pendingTransfers.unlinked_count !== 1 ? "s" : ""} waiting for the other side to be imported.
                </>
              )}
            </p>
            <Button variant="outline" size="sm" className="mt-3" asChild>
              <Link to="/imports">Review Imports</Link>
            </Button>
          </CardContent>
        </Card>
      )}

      {save.isError && (
        <p className="text-sm text-destructive">Couldn't save the dashboard: {save.error.message}</p>
      )}

      {editing && <AddWidget onAdd={add} />}

      {layout.data ? (
        widgets.length ? (
          <div className="grid grid-cols-2 gap-3 sm:gap-4 lg:grid-cols-4">
            {widgets.map((widget, i) => (
              <WidgetCard
                key={widget.id}
                widget={widget}
                editing={editing}
                first={i === 0}
                last={i === widgets.length - 1}
                onChange={(settings) => update(widgets.map((w) => (w.id === widget.id ? { ...w, settings } : w)))}
                onMove={(direction) => {
                  const next = [...widgets];
                  [next[i], next[i + direction]] = [next[i + direction], next[i]];
                  update(next);
                }}
                onRemove={() => update(widgets.filter((w) => w.id !== widget.id))}
              />
            ))}
          </div>
        ) : (
          <Card>
            <CardContent className="p-6 text-center text-sm text-muted-foreground">
              Your dashboard is empty. Choose Customise to add widgets.
            </CardContent>
          </Card>
        )
      ) : (
        <p className="text-sm text-muted-foreground">
          {layout.isError ? `Couldn't load your dashboard: ${layout.error.message}` : "Loading…"}
        </p>
      )}
    </div>
  );
}

function AddWidget({ onAdd }: { onAdd: (type: WidgetType) => void }) {
  const [choice, setChoice] = useState<WidgetType | "">("");
  return (
    <div className="flex flex-wrap items-center gap-2">
      <Select value={choice} onValueChange={(v) => setChoice(v as WidgetType)}>
        <SelectTrigger className="h-9 w-64" aria-label="Widget to add">
          <SelectValue placeholder="Choose a widget to add" />
        </SelectTrigger>
        <SelectContent>
          {(Object.keys(WIDGETS) as WidgetType[]).map((type) => (
            <SelectItem key={type} value={type}>
              {WIDGETS[type].label}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
      <Button
        size="sm"
        disabled={!choice}
        onClick={() => {
          if (choice) onAdd(choice);
          setChoice("");
        }}
      >
        <Plus className="h-4 w-4" /> Add
      </Button>
    </div>
  );
}
