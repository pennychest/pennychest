import type { CsvColumns, CsvField, CsvPreview } from "../api/client";

export function columnsFromPreview(preview: CsvPreview): CsvColumns {
  const columns: CsvColumns = {};
  for (const [field, suggestion] of Object.entries(preview.columns)) {
    if (suggestion?.column != null) columns[field as CsvField] = suggestion.column;
  }
  return columns;
}

// The fields a mapping still needs: a date, a description and some amount
export function missingCsvFields(columns: CsvColumns): string[] {
  const missing = [];
  if (columns.date === undefined) missing.push("date");
  if (columns.description === undefined) missing.push("description");
  if (columns.amount === undefined && columns.money_out === undefined && columns.money_in === undefined) {
    missing.push("amount");
  }
  return missing;
}
