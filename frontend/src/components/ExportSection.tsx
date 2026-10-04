import { useQuery } from "@tanstack/react-query";
import { BookOpenText, Database, Download, ShieldAlert } from "lucide-react";
import { exportApi } from "../api/client";
import { Card, CardContent } from "./ui/card";

function size(bytes: number) {
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

export function ExportSection() {
  const {
    data: info,
    isLoading,
    error,
  } = useQuery({ queryKey: ["export-info"], queryFn: exportApi.info });

  if (isLoading)
    return <p className="text-sm text-muted-foreground">Loading…</p>;
  if (error || !info)
    return (
      <p className="text-sm text-destructive">
        Couldn't check what can be exported.
      </p>
    );

  return (
    <div className="space-y-4">
      {info.exporters.length === 0 && (
        <p className="text-sm text-muted-foreground">
          Other formats, such as Beancount, come from exporter plugins. None are installed.
        </p>
      )}

      {info.exporters.map((exporter) => (
        <Card key={exporter.name}>
          <CardContent className="pt-4 space-y-4">
            <div className="flex items-start gap-3">
              <BookOpenText className="h-5 w-5 mt-0.5 text-muted-foreground shrink-0" />
              <div className="space-y-1">
                <p className="font-medium">{exporter.label}</p>
                {exporter.description && (
                  <p className="text-sm text-muted-foreground">{exporter.description}</p>
                )}
              </div>
            </div>
            <a
              href={exportApi.exporterUrl(exporter.name)}
              download
              className="inline-flex items-center gap-2 rounded-md border px-4 py-2 text-sm font-medium hover:bg-accent"
            >
              <Download className="h-4 w-4" />
              Download {exporter.label}
            </a>
          </CardContent>
        </Card>
      ))}

      <Card>
        <CardContent className="pt-4 space-y-4">
          <div className="flex items-start gap-3">
            <Database className="h-5 w-5 mt-0.5 text-muted-foreground shrink-0" />
            <div className="space-y-1">
              <p className="font-medium">Database backup</p>
              <p className="text-sm text-muted-foreground">
                A complete copy of PennyChest's SQLite database: every account,
                transaction, category, rule, budget and setting. Open it with
                any SQLite tool, or keep it to restore from.
              </p>
            </div>
          </div>

          {info.database_backup ? (
            <>
              <div className="flex items-start gap-2 rounded-md border border-amber-300 bg-amber-50/60 p-3 text-sm dark:bg-amber-950/30">
                <ShieldAlert className="h-4 w-4 mt-0.5 shrink-0 text-amber-600" />
                <p>
                  It includes your password's hash and any AI provider keys
                  you've saved, so keep it somewhere private.
                </p>
              </div>
              <a
                href={exportApi.databaseUrl}
                download
                className="inline-flex items-center gap-2 rounded-md bg-primary px-4 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90"
              >
                <Download className="h-4 w-4" />
                Download database backup
              </a>
              {info.size_bytes != null && (
                <p className="text-xs text-muted-foreground">
                  About {size(info.size_bytes)}.
                </p>
              )}
            </>
          ) : (
            <p className="text-sm text-muted-foreground">
              This PennyChest runs on {info.engine}, so the database can't be
              downloaded from here. Back it up with that database's own tools
              (for Postgres, pg_dump).
            </p>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
