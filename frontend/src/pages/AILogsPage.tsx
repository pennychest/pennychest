import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { ChevronDown, ChevronLeft, ChevronRight } from "lucide-react";
import { aiApi, type AIRequestLog } from "../api/client";
import { Badge } from "../components/ui/badge";
import { Card, CardContent } from "../components/ui/card";
import { Button } from "../components/ui/button";
import { cn, formatAccountPath } from "../lib/utils";

const PAGE_SIZE = 50;

type Json = Record<string, unknown>;
type Answer = { label: string; choice: string | null; confidence?: number };
type RuleSuggestion = {
  pattern: string;
  match_type: string;
  target_account_full_path: string;
  priority: number;
  description?: string;
};
type Usage = { input: number; output: number };
type Result =
  | { kind: "answers"; items: Answer[] }
  | { kind: "rules"; rules: RuleSuggestion[] }
  | { kind: "json"; value: unknown }
  | { kind: "text"; text: string }
  | { kind: "none" };

const OPERATIONS: Record<string, string> = {
  categorise: "Categorise",
  tap_categorise: "Card tap",
  suggest_rules: "Suggest rules",
};

// ── Reading the different providers' formats ────────────────────────────────
//
// Claude, OpenAI-compatible (OpenAI, Gemini), Vertex, Ollama and Jev each store their own request and
// response shapes. These pull out the prompt, the answer and token counts from any of them.

function asObject(value: unknown): Json | null {
  return value && typeof value === "object" && !Array.isArray(value) ? (value as Json) : null;
}

function jevCalls(log: AIRequestLog): { request: Json; response: Json }[] | null {
  const requests = log.request_data?.calls as Json[] | undefined;
  if (!Array.isArray(requests) || !requests.some((r) => "questions" in r)) return null;
  const responses = (log.response_data?.calls as Json[] | undefined) ?? [];
  return requests.map((request, i) => ({ request, response: responses[i] ?? {} }));
}

function userPrompt(req: Json | null): string {
  const contents = req?.contents as { role: string; parts?: { text?: string }[] }[] | undefined;
  if (Array.isArray(contents)) {
    // Vertex: the last user turn's text parts
    const user = contents.filter((c) => c.role === "user").at(-1);
    return (user?.parts ?? []).map((p) => p.text ?? "").join("\n");
  }
  const messages = req?.messages as { role: string; content: unknown }[] | undefined;
  const user = messages?.filter((m) => m.role === "user").at(-1);
  const content = user?.content;
  if (typeof content === "string") return content.replace(/^\/no_think\n/, "");
  if (Array.isArray(content)) {
    return content.map((b) => (asObject(b)?.text as string) ?? "").join("\n");
  }
  return "";
}

function responseText(resp: Json | null): string | null {
  if (!resp) return null;
  const candidates = resp.candidates as { content?: { parts?: { text?: string; thought?: boolean }[] } }[] | undefined;
  if (Array.isArray(candidates)) {
    // Vertex
    const parts = candidates[0]?.content?.parts ?? [];
    return parts.filter((p) => p.text && !p.thought).map((p) => p.text).join("") || null;
  }
  const choices = resp.choices as { message?: { content?: string } }[] | undefined;
  if (choices?.[0]?.message?.content) return choices[0].message.content; // OpenAI-compatible
  const content = resp.content as { type: string; text?: string; input?: unknown }[] | undefined;
  if (Array.isArray(content)) {
    const tool = content.find((b) => b.type === "tool_use");
    if (tool) return JSON.stringify(tool.input); // Claude, older tool-use logs
    const text = content.find((b) => b.type === "text")?.text;
    if (text) return text; // Claude
  }
  const message = asObject(resp.message);
  if (typeof message?.content === "string") return message.content; // Ollama
  return null;
}

function usageOf(log: AIRequestLog): Usage | null {
  const jev = jevCalls(log);
  const parts = jev
    ? jev.map((c) => asObject(c.response.usage))
    : [asObject(log.response_data?.usage) ?? asObject(log.response_data?.usageMetadata)];
  let input = 0;
  let output = 0;
  let found = false;
  for (const u of parts) {
    if (!u) continue;
    const i = (u.input_tokens ?? u.prompt_tokens ?? u.promptTokenCount) as number | undefined;
    const o = (u.output_tokens ?? u.completion_tokens ?? u.candidatesTokenCount) as number | undefined;
    if (i !== undefined || o !== undefined) {
      found = true;
      input += i ?? 0;
      output += o ?? 0;
    }
  }
  const resp = log.response_data;
  if (!found && typeof resp?.prompt_eval_count === "number") {
    return { input: resp.prompt_eval_count, output: (resp.eval_count as number) ?? 0 }; // Ollama
  }
  return found ? { input, output } : null;
}

function modelOf(log: AIRequestLog): string | null {
  const jev = jevCalls(log);
  const req = jev ? jev[0]?.request : log.request_data;
  return (req?.model as string) ?? null;
}

// Prompts list transactions as "- id=12: TESCO STORES"; used to name the LLM's answers.
function descriptionsFromPrompt(prompt: string): Map<number, string> {
  const found = new Map<number, string>();
  for (const match of prompt.matchAll(/^- id=(\d+): (.*)$/gm)) found.set(Number(match[1]), match[2]);
  return found;
}

function readLog(log: AIRequestLog): { prompt: string; result: Result } {
  const jev = jevCalls(log);
  if (jev) {
    const items: Answer[] = [];
    const lines: string[] = [];
    for (const { request, response } of jev) {
      const state = asObject(request.state) ?? {};
      const answers = asObject(response.answers) ?? {};
      for (const key of Object.keys(asObject(request.questions) ?? {})) {
        const label = String(state[key] ?? key);
        const answer = asObject(answers[key]);
        lines.push(`${key}: ${label}`);
        items.push({
          label,
          choice: (answer?.choice as string) ?? null,
          confidence: answer?.confidence as number | undefined,
        });
      }
    }
    return { prompt: lines.join("\n"), result: { kind: "answers", items } };
  }

  const prompt = userPrompt(log.request_data);
  const text = responseText(log.response_data);
  if (!text) return { prompt, result: { kind: "none" } };
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    return { prompt, result: { kind: "text", text } };
  }
  const obj = asObject(parsed);
  if (Array.isArray(obj?.categorisations)) {
    const names = descriptionsFromPrompt(prompt);
    const items = (obj.categorisations as { transaction_id: number; account_full_path: string }[]).map(
      (c) => ({ label: names.get(c.transaction_id) ?? `#${c.transaction_id}`, choice: c.account_full_path }),
    );
    return { prompt, result: { kind: "answers", items } };
  }
  if (Array.isArray(obj?.rules)) {
    return { prompt, result: { kind: "rules", rules: obj.rules as RuleSuggestion[] } };
  }
  return { prompt, result: { kind: "json", value: parsed } };
}

// ── Pieces ───────────────────────────────────────────────────────────────────

function SectionLabel({ children }: { children: React.ReactNode }) {
  return (
    <div className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground mb-1.5">
      {children}
    </div>
  );
}

function Pre({ children }: { children: React.ReactNode }) {
  return (
    <pre className="bg-muted/50 border rounded p-2 overflow-auto max-h-72 text-[11px] font-mono whitespace-pre-wrap break-words leading-relaxed">
      {children}
    </pre>
  );
}

function AnswerList({ items }: { items: Answer[] }) {
  if (items.length === 0) return <p className="text-xs text-muted-foreground">No answers returned.</p>;
  return (
    <ul className="divide-y rounded border">
      {items.map((a, i) => (
        <li key={i} className="px-2 py-1.5 text-xs">
          <p className="font-mono text-[11px] break-words">{a.label}</p>
          <p className="flex items-center gap-1.5 text-muted-foreground">
            <span aria-hidden>→</span>
            {a.choice ? (
              <span className="text-foreground">{formatAccountPath(a.choice)}</span>
            ) : (
              <span>no answer</span>
            )}
            {a.confidence !== undefined && (
              <span className={cn("ml-auto tabular-nums", a.confidence < 0.5 && "text-amber-600")}>
                {Math.round(a.confidence * 100)}%
              </span>
            )}
          </p>
        </li>
      ))}
    </ul>
  );
}

function RuleList({ rules }: { rules: RuleSuggestion[] }) {
  if (rules.length === 0) return <p className="text-xs text-muted-foreground">No rules suggested.</p>;
  return (
    <div className="space-y-2">
      {rules.map((r, i) => (
        <div key={i} className="border rounded p-2 text-xs space-y-0.5">
          <div className="flex items-center gap-2 flex-wrap">
            <code className="bg-muted px-1.5 py-0.5 rounded text-[11px] font-mono">{r.pattern}</code>
            <Badge variant="outline" className="text-[10px] px-1.5">{r.match_type}</Badge>
            <span className="text-muted-foreground">→</span>
            <span>{formatAccountPath(r.target_account_full_path)}</span>
          </div>
          {r.description && <div className="text-muted-foreground">{r.description}</div>}
        </div>
      ))}
    </div>
  );
}

function ResultView({ log, result }: { log: AIRequestLog; result: Result }) {
  if (log.error) {
    return (
      <div className="text-xs text-destructive bg-destructive/10 border border-destructive/20 rounded p-2 break-words">
        {log.error}
      </div>
    );
  }
  switch (result.kind) {
    case "answers":
      return <AnswerList items={result.items} />;
    case "rules":
      return <RuleList rules={result.rules} />;
    case "json":
      return <Pre>{JSON.stringify(result.value, null, 2)}</Pre>;
    case "text":
      return <Pre>{result.text}</Pre>;
    default:
      return <p className="text-xs text-muted-foreground">Nothing recorded.</p>;
  }
}

function ExpandedLog({ log }: { log: AIRequestLog }) {
  const [raw, setRaw] = useState(false);
  const { prompt, result } = readLog(log);

  return (
    <div className="space-y-3 text-xs">
      <div className="grid gap-4 lg:grid-cols-2">
        <div className="min-w-0">
          <SectionLabel>Sent</SectionLabel>
          {prompt ? <Pre>{prompt}</Pre> : <p className="text-muted-foreground">Nothing recorded.</p>}
        </div>
        <div className="min-w-0">
          <SectionLabel>{log.error ? "Error" : "Answer"}</SectionLabel>
          <ResultView log={log} result={result} />
        </div>
      </div>
      <button type="button" className="text-primary underline" onClick={() => setRaw((v) => !v)}>
        {raw ? "Hide raw request and response" : "Show raw request and response"}
      </button>
      {raw && (
        <div className="grid gap-4 lg:grid-cols-2">
          <div className="min-w-0">
            <SectionLabel>Request</SectionLabel>
            <Pre>{JSON.stringify(log.request_data, null, 2) ?? "null"}</Pre>
          </div>
          <div className="min-w-0">
            <SectionLabel>Response</SectionLabel>
            <Pre>{JSON.stringify(log.response_data, null, 2) ?? "null"}</Pre>
          </div>
        </div>
      )}
    </div>
  );
}

function LogRow({ log }: { log: AIRequestLog }) {
  const [expanded, setExpanded] = useState(false);
  const usage = usageOf(log);
  const model = modelOf(log);
  const provider = log.provider.split(":")[0];
  const when = new Date(log.created_at).toLocaleString("en-GB", {
    day: "numeric",
    month: "short",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });

  return (
    <li>
      <button
        type="button"
        className="w-full flex items-start gap-2 px-3 py-2.5 text-left hover:bg-muted/40 transition-colors"
        onClick={() => setExpanded((v) => !v)}
        aria-expanded={expanded}
      >
        {expanded ? (
          <ChevronDown className="h-4 w-4 mt-0.5 shrink-0 text-muted-foreground" />
        ) : (
          <ChevronRight className="h-4 w-4 mt-0.5 shrink-0 text-muted-foreground" />
        )}
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2 flex-wrap">
            <span className="font-medium text-sm">{OPERATIONS[log.operation] ?? log.operation}</span>
            {log.error ? (
              <Badge variant="outline" className="text-[10px] px-1.5 py-0 border-destructive/40 text-destructive">
                error
              </Badge>
            ) : (
              <Badge variant="outline" className="text-[10px] px-1.5 py-0 border-green-300 text-green-700">
                ok
              </Badge>
            )}
            <span className="ml-auto text-xs text-muted-foreground tabular-nums whitespace-nowrap">{when}</span>
          </div>
          <div className="flex items-center gap-x-2 gap-y-0.5 flex-wrap text-xs text-muted-foreground mt-0.5">
            <span className="font-mono truncate max-w-full">
              {provider}
              {model ? ` · ${model}` : ""}
            </span>
            {log.duration_ms !== null && <span>{(log.duration_ms / 1000).toFixed(1)} s</span>}
            {usage && (
              <span className="tabular-nums">
                ↑{usage.input.toLocaleString()} ↓{usage.output.toLocaleString()} tokens
              </span>
            )}
          </div>
          {log.error && !expanded && (
            <p className="text-xs text-destructive mt-0.5 line-clamp-1">{log.error}</p>
          )}
        </div>
      </button>
      {expanded && (
        <div className="px-3 pb-3 sm:pl-9">
          <ExpandedLog log={log} />
        </div>
      )}
    </li>
  );
}

// ── Page ─────────────────────────────────────────────────────────────────────

export function AILogsPage() {
  const [offset, setOffset] = useState(0);

  const { data, isLoading, isError } = useQuery({
    queryKey: ["ai-logs", offset],
    queryFn: () => aiApi.getLogs({ limit: PAGE_SIZE, offset }),
  });

  const logs = data?.logs ?? [];
  const total = data?.total ?? 0;
  const pageCount = Math.ceil(total / PAGE_SIZE);
  const currentPage = Math.floor(offset / PAGE_SIZE) + 1;

  return (
    <div className="space-y-4">
      <div className="flex items-center gap-2">
        <Button variant="ghost" size="icon" asChild>
          <Link to="/settings" aria-label="Back to Settings">
            <ChevronLeft className="h-5 w-5" />
          </Link>
        </Button>
        <div>
          <h1 className="text-xl font-semibold">AI Request Logs</h1>
          <p className="text-sm text-muted-foreground">{total} entries</p>
        </div>
      </div>

      <Card>
        <CardContent className="p-0">
          {isLoading && <div className="p-6 text-sm text-muted-foreground">Loading…</div>}
          {isError && <div className="p-6 text-sm text-destructive">Failed to load logs.</div>}
          {!isLoading && !isError && logs.length === 0 && (
            <div className="p-6 text-sm text-muted-foreground">No AI requests logged yet.</div>
          )}
          {logs.length > 0 && (
            <ul className="divide-y">
              {logs.map((log) => (
                <LogRow key={log.id} log={log} />
              ))}
            </ul>
          )}
        </CardContent>
      </Card>

      {pageCount > 1 && (
        <div className="flex items-center justify-end gap-2 text-sm">
          <Button variant="outline" size="sm" disabled={offset === 0}
            onClick={() => setOffset(Math.max(0, offset - PAGE_SIZE))}>
            Previous
          </Button>
          <span className="text-muted-foreground">{currentPage} / {pageCount}</span>
          <Button variant="outline" size="sm" disabled={offset + PAGE_SIZE >= total}
            onClick={() => setOffset(offset + PAGE_SIZE)}>
            Next
          </Button>
        </div>
      )}
    </div>
  );
}
