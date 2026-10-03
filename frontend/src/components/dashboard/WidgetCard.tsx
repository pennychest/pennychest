import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { ArrowDown, ArrowUp, BarChart3, Table2, X } from "lucide-react";
import { accountsApi, type Period, type Widget, type WidgetSettings } from "../../api/client";
import { cn, formatAccountPath } from "../../lib/utils";
import { Button } from "../ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "../ui/card";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "../ui/select";
import { PERIOD_LABELS } from "./periods";
import { STAT_LABELS, WIDGETS, WidgetBody, widgetTitle } from "./widgets";

const MONTH_CHOICES = [3, 6, 12, 24, 36];
const CHART_LABELS = { bar: "Bars", donut: "Donut", line: "Lines" } as const;
export const MAX_SERIES = 8;

export function WidgetCard({
  widget,
  editing,
  first,
  last,
  onChange,
  onMove,
  onRemove,
}: {
  widget: Widget;
  editing: boolean;
  first: boolean;
  last: boolean;
  onChange: (settings: WidgetSettings) => void;
  onMove: (direction: -1 | 1) => void;
  onRemove: () => void;
}) {
  const info = WIDGETS[widget.type];
  const [table, setTable] = useState(false);
  const title = widgetTitle(widget.type, widget.settings);

  return (
    <Card className={cn(info.wide && "col-span-2", editing && "border-dashed")}>
      <CardHeader className="flex flex-row items-start justify-between gap-2 space-y-0 pb-2">
        <CardTitle className="text-sm font-medium leading-snug">{title}</CardTitle>
        <div className="-mr-2 -mt-1 flex shrink-0">
          {editing ? (
            <>
              <IconButton label="Move earlier" onClick={() => onMove(-1)} disabled={first}>
                <ArrowUp className="h-4 w-4" />
              </IconButton>
              <IconButton label="Move later" onClick={() => onMove(1)} disabled={last}>
                <ArrowDown className="h-4 w-4" />
              </IconButton>
              <IconButton label={`Remove ${title}`} onClick={onRemove}>
                <X className="h-4 w-4" />
              </IconButton>
            </>
          ) : (
            info.chart && (
              <IconButton
                label={table ? "Show as chart" : "Show as table"}
                onClick={() => setTable((t) => !t)}
                pressed={table}
              >
                {table ? <BarChart3 className="h-4 w-4" /> : <Table2 className="h-4 w-4" />}
              </IconButton>
            )
          )}
        </div>
      </CardHeader>
      <CardContent className="space-y-3">
        {editing && info.options.length > 0 && <SettingsEditor widget={widget} onChange={onChange} />}
        <WidgetBody type={widget.type} settings={widget.settings} table={table} />
      </CardContent>
    </Card>
  );
}

function IconButton({
  label,
  onClick,
  disabled,
  pressed,
  children,
}: {
  label: string;
  onClick: () => void;
  disabled?: boolean;
  pressed?: boolean;
  children: React.ReactNode;
}) {
  return (
    <Button
      variant="ghost"
      size="icon"
      className="h-8 w-8 text-muted-foreground"
      onClick={onClick}
      disabled={disabled}
      title={label}
      aria-label={label}
      aria-pressed={pressed}
    >
      {children}
    </Button>
  );
}

function SettingsEditor({ widget, onChange }: { widget: Widget; onChange: (s: WidgetSettings) => void }) {
  const info = WIDGETS[widget.type];
  const s = widget.settings;
  const set = (changes: WidgetSettings) => onChange({ ...s, ...changes });

  return (
    <div className="space-y-2 rounded-md bg-muted/60 p-2">
      <div className="flex flex-wrap gap-2">
        {info.options.includes("stat") && (
          <Choice
            label="Figure"
            value={s.stat ?? "net_worth"}
            options={Object.entries(STAT_LABELS) as [string, string][]}
            onChange={(stat) => set({ stat: stat as WidgetSettings["stat"] })}
          />
        )}
        {info.options.includes("period") && (
          <Choice
            label="Period"
            value={s.period ?? "this_month"}
            options={Object.entries(PERIOD_LABELS) as [string, string][]}
            onChange={(period) => set({ period: period as Period })}
          />
        )}
        {info.options.includes("months") && (
          <Choice
            label="Months"
            value={String(s.months ?? 12)}
            options={MONTH_CHOICES.map((m) => [String(m), `Last ${m} months`])}
            onChange={(months) => set({ months: Number(months) })}
          />
        )}
        {info.options.includes("chart") && info.charts && (
          <Choice
            label="Chart"
            value={s.chart ?? info.charts[0]}
            options={info.charts.map((c) => [c, CHART_LABELS[c]])}
            onChange={(chart) => set({ chart: chart as WidgetSettings["chart"] })}
          />
        )}
      </div>
      {info.options.includes("categories") && (
        <CategoryPicker selected={s.categories ?? []} onChange={(categories) => set({ categories })} />
      )}
    </div>
  );
}

function Choice({
  label,
  value,
  options,
  onChange,
}: {
  label: string;
  value: string;
  options: [string, string][];
  onChange: (value: string) => void;
}) {
  return (
    <Select value={value} onValueChange={onChange}>
      <SelectTrigger className="h-8 w-auto min-w-36 bg-background text-xs" aria-label={label}>
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        {options.map(([v, text]) => (
          <SelectItem key={v} value={v}>
            {text}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}

/** Up to eight spending categories, each its own series in the order picked. None means all
 * spending as one series. */
function CategoryPicker({ selected, onChange }: { selected: string[]; onChange: (c: string[]) => void }) {
  const { data: accounts } = useQuery({ queryKey: ["accounts"], queryFn: accountsApi.list });
  const categories = (accounts ?? [])
    .filter((a) => a.type === "expense" && a.full_path.includes(":"))
    .map((a) => a.full_path)
    .sort();
  const full = selected.length >= MAX_SERIES;

  return (
    <fieldset className="space-y-1">
      <legend className="text-xs text-muted-foreground">
        Categories to compare ({selected.length}/{MAX_SERIES}; none shows all spending)
      </legend>
      <div className="flex max-h-32 flex-wrap gap-1.5 overflow-y-auto">
        {categories.map((path) => {
          const on = selected.includes(path);
          return (
            <button
              key={path}
              type="button"
              aria-pressed={on}
              disabled={!on && full}
              onClick={() => onChange(on ? selected.filter((c) => c !== path) : [...selected, path])}
              className={cn(
                "rounded-full border px-2 py-0.5 text-xs transition-colors disabled:opacity-40",
                on ? "border-foreground bg-foreground text-background" : "bg-background hover:bg-accent",
              )}
            >
              {formatAccountPath(path)}
            </button>
          );
        })}
      </div>
    </fieldset>
  );
}
