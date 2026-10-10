import { useState } from "react";
import { useMutation } from "@tanstack/react-query";
import { Check, Sparkles } from "lucide-react";
import { importsApi, type CsvColumns, type CsvField, type CsvPreview } from "../api/client";
import { columnsFromPreview, missingCsvFields } from "../lib/csv";
import { cn } from "../lib/utils";
import { Badge } from "./ui/badge";
import { Button } from "./ui/button";
import { Input } from "./ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "./ui/select";

const FIELDS: { field: CsvField; label: string }[] = [
  { field: "date", label: "Date" },
  { field: "description", label: "Description" },
  { field: "amount", label: "Amount" },
  { field: "money_out", label: "Money out" },
  { field: "money_in", label: "Money in" },
];

const NONE = "none";
// Below this a decision model's answer is worth checking
const UNSURE = 0.7;
const SAMPLE_ROWS = 3;

export function CsvMapping({
  file,
  preview,
  columns,
  onChange,
}: {
  file: File;
  preview: CsvPreview;
  columns: CsvColumns;
  onChange: (preview: CsvPreview, columns: CsvColumns) => void;
}) {
  const [templateName, setTemplateName] = useState("");

  const suggest = useMutation({
    mutationFn: () => importsApi.csvPreview(file, true),
    onSuccess: (next) => onChange(next, { ...columnsFromPreview(next), ...columns }),
  });
  const save = useMutation({
    mutationFn: () => importsApi.saveCsvTemplate(templateName.trim(), preview.headers, columns),
    onSuccess: (template) => onChange({ ...preview, template: { id: template.id, name: template.name } }, columns),
  });

  function choose(field: CsvField, value: string) {
    const next = { ...columns };
    if (value === NONE) delete next[field];
    else next[field] = Number(value);
    onChange(preview, next);
  }

  const missing = missingCsvFields(columns);
  const aiError = suggest.data?.ai_error ?? suggest.error?.message ?? preview.ai_error;
  const sample = preview.rows.slice(0, SAMPLE_ROWS);

  return (
    <div className="space-y-3 rounded-lg border p-3">
      <div className="flex flex-wrap items-center gap-2">
        <p className="text-sm font-medium">Columns</p>
        {preview.template && <Badge variant="secondary">Template: {preview.template.name}</Badge>}
        {preview.ai_available && !preview.template && (
          <Button
            variant="outline"
            size="sm"
            className="ml-auto"
            onClick={() => suggest.mutate()}
            disabled={suggest.isPending}
          >
            <Sparkles className="h-3.5 w-3.5 mr-1" />
            {suggest.isPending ? "Reading the file…" : "Suggest with AI"}
          </Button>
        )}
      </div>

      <div className="overflow-x-auto rounded border">
        <table className="w-full text-xs">
          <thead className="bg-muted/50">
            <tr>
              {preview.headers.map((header, i) => (
                <th key={i} className="px-2 py-1 text-left font-medium whitespace-nowrap">
                  {header || `Column ${i + 1}`}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {sample.map((row, r) => (
              <tr key={r} className="border-t">
                {preview.headers.map((_, i) => (
                  <td key={i} className="px-2 py-1 whitespace-nowrap text-muted-foreground">
                    {row[i] ?? ""}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      <div className="space-y-2">
        {FIELDS.map(({ field, label }) => {
          const suggestion = preview.columns[field];
          // Only show where a column came from until the user changes it
          const suggested = suggestion?.column != null && suggestion.column === columns[field];
          const unsure = suggested && suggestion.confidence != null && suggestion.confidence < UNSURE;
          return (
            <div key={field} className="grid grid-cols-[6.5rem_1fr] items-center gap-2 sm:grid-cols-[6.5rem_1fr_9rem]">
              <span className="text-sm">{label}</span>
              <Select value={columns[field] !== undefined ? String(columns[field]) : NONE} onValueChange={(v) => choose(field, v)}>
                <SelectTrigger aria-label={`${label} column`} className={cn(unsure && "border-amber-400")}>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value={NONE}>Not in this file</SelectItem>
                  {preview.headers.map((header, i) => (
                    <SelectItem key={i} value={String(i)}>
                      {header || `Column ${i + 1}`}
                      {sample[0]?.[i] ? ` · e.g. ${sample[0][i]}` : ""}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <span className={cn("col-start-2 text-xs sm:col-start-auto", unsure ? "text-amber-700" : "text-muted-foreground")}>
                {suggested &&
                  (suggestion.source === "template"
                    ? "From the template"
                    : suggestion.source === "name"
                    ? "Matched by name"
                    : suggestion.confidence != null
                    ? `AI · ${Math.round(suggestion.confidence * 100)}% sure`
                    : "Suggested by AI")}
              </span>
            </div>
          );
        })}
      </div>

      <p className="text-xs text-muted-foreground">
        Use Amount when one column holds both money in and out, or Money out and Money in when they're separate.
      </p>
      {missing.length > 0 && (
        <p className="text-xs text-warning">Choose the columns for the {missing.join(" and ")}.</p>
      )}
      {aiError && <p className="text-xs text-destructive">{aiError}</p>}

      {missing.length === 0 && !preview.template && (
        <form
          className="flex gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            save.mutate();
          }}
        >
          <Input
            placeholder="Save as a template, e.g. Monzo export"
            value={templateName}
            onChange={(e) => setTemplateName(e.target.value)}
            aria-label="Template name"
          />
          <Button type="submit" variant="outline" disabled={!templateName.trim() || save.isPending}>
            Save
          </Button>
        </form>
      )}
      {save.isSuccess && (
        <p className="text-xs text-muted-foreground flex items-center gap-1">
          <Check className="h-3 w-3" /> Saved. Files laid out like this will use these columns.
        </p>
      )}
      {save.error && <p className="text-xs text-destructive">{save.error.message}</p>}
    </div>
  );
}
