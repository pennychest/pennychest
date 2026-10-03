import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Check, Undo2 } from "lucide-react";
import { changesApi, type ActionChange } from "../api/client";
import { cn } from "../lib/utils";
import { Badge } from "./ui/badge";
import { Card, CardContent } from "./ui/card";

const SOURCE_LABELS: Record<ActionChange["source"], string> = {
  chat: "Chat",
  api: "App",
  mcp: "Agent",
  import: "Import",
};

// Data a change can touch, refreshed after an undo.
const CHANGEABLE_QUERIES = ["accounts", "transactions", "budgets", "rules", "taps", "changes"];

function when(iso: string) {
  return new Date(iso).toLocaleString("en-GB", {
    day: "numeric",
    month: "short",
    hour: "2-digit",
    minute: "2-digit",
  });
}

export function ChangesSection() {
  const queryClient = useQueryClient();
  const [errors, setErrors] = useState<Record<number, string>>({});
  const { data, isLoading } = useQuery({ queryKey: ["changes"], queryFn: () => changesApi.list(100) });

  const undo = useMutation({
    mutationFn: changesApi.undo,
    onSuccess: (change) => {
      setErrors((all) => {
        const rest = { ...all };
        delete rest[change.id];
        return rest;
      });
      for (const key of CHANGEABLE_QUERIES) queryClient.invalidateQueries({ queryKey: [key] });
    },
    onError: (error, id) => setErrors((all) => ({ ...all, [id]: error.message })),
  });

  if (isLoading) return <div className="text-sm text-muted-foreground">Loading…</div>;
  const changes = data?.changes ?? [];

  return (
    <div className="space-y-3">
      <p className="text-sm text-muted-foreground">
        Everything AI has changed, from the chat or straight after an import, newest first.
        Undo puts things back as they were, as long as nothing has changed them since.
      </p>
      {changes.length === 0 ? (
        <Card>
          <CardContent className="py-10 text-center text-sm text-muted-foreground">
            No changes yet.
          </CardContent>
        </Card>
      ) : (
        <Card>
          <CardContent className="divide-y p-0">
            {changes.map((c) => (
              <div key={c.id} className="flex items-start gap-3 px-4 py-3">
                {c.undone_at ? (
                  <Undo2 className="mt-0.5 h-4 w-4 shrink-0 text-muted-foreground" />
                ) : (
                  <Check className="mt-0.5 h-4 w-4 shrink-0 text-success" />
                )}
                <div className="min-w-0 flex-1">
                  <p className={cn("text-sm", c.undone_at && "line-through text-muted-foreground")}>
                    {c.summary}
                  </p>
                  <p className="text-xs text-muted-foreground">
                    {when(c.created_at)}
                    {c.undone_at && ` · undone ${when(c.undone_at)}`}
                  </p>
                  {errors[c.id] && <p className="text-xs text-destructive">{errors[c.id]}</p>}
                </div>
                <Badge variant="outline" className="shrink-0">{SOURCE_LABELS[c.source] ?? c.source}</Badge>
                {!c.undone_at && (
                  <button
                    type="button"
                    className="shrink-0 text-sm text-primary underline disabled:opacity-50"
                    onClick={() => undo.mutate(c.id)}
                    disabled={undo.isPending}
                    aria-label={`Undo: ${c.summary}`}
                  >
                    Undo
                  </button>
                )}
              </div>
            ))}
          </CardContent>
        </Card>
      )}
    </div>
  );
}
