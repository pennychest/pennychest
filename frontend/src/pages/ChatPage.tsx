import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { Check, Hand, History, Loader2, MessageCircle, Plus, Send, Trash2, Undo2, X } from "lucide-react";
import { aiApi, changesApi, chatApi, type ChatChange, type ChatToolUse, type ChatTurn } from "../api/client";
import { Button } from "../components/ui/button";
import { Card, CardContent } from "../components/ui/card";
import { cn } from "../lib/utils";

const TOOL_LABELS: Record<string, string> = {
  spending_summary: "Totalling your spending",
  list_transactions: "Searching transactions",
  list_categories: "Checking your categories",
  list_accounts: "Checking your accounts",
  list_rules: "Checking your rules",
  get_budgets: "Checking your budgets",
  list_card_taps: "Checking card taps",
  savings_opportunities: "Looking for savings",
  recurring_payments: "Finding regular payments",
  compare_periods: "Comparing periods",
  transactions_digest: "Reading your transactions",
  unusual_charges: "Checking for unusual charges",
};

const CHANGE_LABELS: Record<string, string> = {
  create_category: "Creating a category",
  rename_category: "Renaming a category",
  create_rule: "Creating a rule",
  set_budget: "Setting a budget",
  delete_budget: "Removing a budget",
  recategorise_transactions: "Recategorising transactions",
  add_transaction: "Adding a transaction",
  add_transfer: "Adding a transfer",
  edit_transaction: "Editing a transaction",
  delete_transactions: "Deleting transactions",
  mark_reviewed: "Marking transactions reviewed",
};

// Data a chat change can touch, refreshed after an undo.
const CHANGEABLE_QUERIES = ["accounts", "transactions", "budgets", "rules", "taps", "changes"];

const SUGGESTIONS = [
  "Summarise my spending over the last 12 months",
  "How am I doing against my budgets this month?",
  "What were my biggest expenses last month?",
  "Which card payments haven't appeared on a statement yet?",
  "What subscriptions and bills am I paying?",
];

function toolLabel(name: string) {
  return TOOL_LABELS[name] ?? CHANGE_LABELS[name] ?? name.replaceAll("_", " ");
}

interface PendingTurn {
  steps: { name: string; done: boolean; ok: boolean; held?: boolean }[];
  text: string;
}

export function ChatPage() {
  const queryClient = useQueryClient();
  const [conversationId, setConversationId] = useState<number | null>(null);
  const [turns, setTurns] = useState<ChatTurn[]>([]);
  const [pending, setPending] = useState<PendingTurn | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [input, setInput] = useState("");
  const [showHistory, setShowHistory] = useState(false);
  const [undoErrors, setUndoErrors] = useState<Record<number, string>>({});
  const bottomRef = useRef<HTMLDivElement>(null);

  const { data: config } = useQuery({ queryKey: ["ai-config"], queryFn: aiApi.config });
  const { data: conversations = [] } = useQuery({
    queryKey: ["chat-conversations"],
    queryFn: chatApi.conversations,
  });
  const chatTask = config?.tasks.chat;

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth", block: "end" });
  }, [turns, pending, error]);

  async function openConversation(id: number) {
    setShowHistory(false);
    setError(null);
    const conversation = await chatApi.conversation(id);
    setConversationId(conversation.id);
    setTurns(conversation.turns);
  }

  function newConversation() {
    setShowHistory(false);
    setConversationId(null);
    setTurns([]);
    setError(null);
  }

  async function undoChange(changeId: number) {
    try {
      await changesApi.undo(changeId);
      setUndoErrors((errors) => {
        const rest = { ...errors };
        delete rest[changeId];
        return rest;
      });
      setTurns((all) =>
        all.map((turn) => ({
          ...turn,
          changes: turn.changes?.map((c) => (c.id === changeId ? { ...c, undone: true } : c)),
        })),
      );
      for (const key of CHANGEABLE_QUERIES) queryClient.invalidateQueries({ queryKey: [key] });
    } catch (e) {
      setUndoErrors((errors) => ({ ...errors, [changeId]: e instanceof Error ? e.message : "Couldn't undo" }));
    }
  }

  async function deleteConversation(id: number) {
    if (!window.confirm("Delete this conversation?")) return;
    await chatApi.delete(id);
    if (id === conversationId) newConversation();
    queryClient.invalidateQueries({ queryKey: ["chat-conversations"] });
  }

  async function send(text: string) {
    const message = text.trim();
    if (!message || pending) return;
    setInput("");
    setError(null);
    setTurns((t) => [...t, { role: "user", content: message }]);
    setPending({ steps: [], text: "" });
    let tools: ChatToolUse[] = [];
    let changes: ChatChange[] = [];
    try {
      await chatApi.send(message, conversationId, (event) => {
        switch (event.event) {
          case "conversation":
            setConversationId(event.id);
            queryClient.invalidateQueries({ queryKey: ["chat-conversations"] });
            break;
          case "text":
            setPending((p) => ({ steps: p?.steps ?? [], text: (p?.text ?? "") + event.delta }));
            break;
          case "tool":
            // Text written before a lookup is a preamble ("Let me check…"); the step replaces it.
            setPending((p) => ({
              steps: [...(p?.steps ?? []), { name: event.name, done: false, ok: true }],
              text: "",
            }));
            break;
          case "tool_result":
            tools = [...tools, { name: event.name, ok: event.ok, held: event.held }];
            setPending((p) => {
              const steps = [...(p?.steps ?? [])];
              const index = steps.findIndex((s) => s.name === event.name && !s.done);
              if (index !== -1) steps[index] = { ...steps[index], done: true, ok: event.ok, held: event.held };
              return { steps, text: p?.text ?? "" };
            });
            break;
          case "change":
            changes = [...changes, { id: event.id, summary: event.summary, undone: false }];
            break;
          case "message":
            setTurns((t) => [...t, { role: "assistant", content: event.content, tools, changes }]);
            break;
          case "error":
            setError(event.detail);
            break;
        }
      });
    } catch (e) {
      setError(e instanceof Error ? e.message : "Something went wrong");
    } finally {
      setPending(null);
      queryClient.invalidateQueries({ queryKey: ["chat-conversations"] });
      queryClient.invalidateQueries({ queryKey: ["ai-logs"] });
    }
  }

  const empty = turns.length === 0 && !pending;

  return (
    <div className="flex gap-4">
      {/* Conversation list: a sidebar on desktop, an overlay on mobile */}
      <aside
        className={cn(
          "w-64 shrink-0 space-y-2",
          showHistory ? "fixed inset-0 z-50 w-auto overflow-y-auto bg-background p-4" : "hidden md:block",
        )}
      >
        <div className="flex items-center gap-2">
          <Button className="flex-1" variant="outline" onClick={newConversation}>
            <Plus className="h-4 w-4 mr-1" /> New chat
          </Button>
          {showHistory && (
            <Button variant="ghost" size="icon" onClick={() => setShowHistory(false)} aria-label="Close history">
              <X className="h-5 w-5" />
            </Button>
          )}
        </div>
        {conversations.length === 0 && (
          <p className="text-xs text-muted-foreground px-1">No conversations yet.</p>
        )}
        {conversations.map((c) => (
          <div
            key={c.id}
            className={cn(
              "group flex items-center rounded-md text-sm hover:bg-accent",
              c.id === conversationId && "bg-accent",
            )}
          >
            <button type="button" className="flex-1 truncate px-2 py-2 text-left" onClick={() => openConversation(c.id)}>
              {c.title}
            </button>
            <button
              type="button"
              className="p-2 text-muted-foreground opacity-100 md:opacity-0 group-hover:opacity-100 hover:text-destructive"
              onClick={() => deleteConversation(c.id)}
              aria-label={`Delete conversation ${c.title}`}
            >
              <Trash2 className="h-4 w-4" />
            </button>
          </div>
        ))}
      </aside>

      <div className="flex-1 min-w-0 flex flex-col min-h-[calc(100dvh-10rem)]">
        <div className="flex items-center gap-2 mb-4">
          <h1 className="text-3xl font-bold">Chat</h1>
          <div className="ml-auto flex gap-1 md:hidden">
            <Button variant="ghost" size="icon" onClick={() => setShowHistory(true)} aria-label="Conversation history">
              <History className="h-5 w-5" />
            </Button>
            <Button variant="ghost" size="icon" onClick={newConversation} aria-label="New chat">
              <Plus className="h-5 w-5" />
            </Button>
          </div>
        </div>

        <div className="flex-1 space-y-4">
          {empty && !chatTask?.ready && (
            <Card>
              <CardContent className="py-12 flex flex-col items-center gap-3 text-center">
                <MessageCircle className="h-10 w-10 text-muted-foreground opacity-40" />
                <p className="font-medium">Choose a chat model first</p>
                <p className="text-sm text-muted-foreground max-w-sm">
                  Pick a provider and model for Chat in{" "}
                  <Link to="/settings" className="text-primary underline">Settings</Link> → AI Integration.
                </p>
              </CardContent>
            </Card>
          )}

          {empty && chatTask?.ready && (
            <div className="space-y-3 pt-4">
              <p className="text-muted-foreground">
                Ask about your spending, budgets or card payments. PennyChest looks the answers up in
                your data.
              </p>
              <div className="grid gap-2 sm:grid-cols-2">
                {SUGGESTIONS.map((s) => (
                  <button
                    key={s}
                    type="button"
                    onClick={() => send(s)}
                    className="rounded-lg border px-3 py-2 text-left text-sm hover:bg-accent transition-colors"
                  >
                    {s}
                  </button>
                ))}
              </div>
            </div>
          )}

          {turns.map((turn, i) =>
            turn.role === "user" ? (
              <div key={i} className="flex justify-end">
                <div className="max-w-[85%] rounded-2xl rounded-br-sm bg-primary px-4 py-2 text-primary-foreground whitespace-pre-wrap">
                  {turn.content}
                </div>
              </div>
            ) : (
              <div key={i} className="space-y-1">
                <div className="max-w-full rounded-2xl rounded-bl-sm bg-muted px-4 py-3">
                  <Markdown text={turn.content} />
                </div>
                {turn.changes && turn.changes.length > 0 && (
                  <ul className="px-2 space-y-1" aria-label="Changes made">
                    {turn.changes.map((c) => (
                      <li key={c.id} className="text-xs">
                        <div className="flex items-center gap-2">
                          {c.undone ? (
                            <Undo2 className="h-3 w-3 shrink-0 text-muted-foreground" />
                          ) : (
                            <Check className="h-3 w-3 shrink-0 text-success" />
                          )}
                          <span className={cn(c.undone && "line-through text-muted-foreground")}>{c.summary}</span>
                          {c.undone ? (
                            <span className="text-muted-foreground">Undone</span>
                          ) : (
                            <button
                              type="button"
                              className="text-primary underline"
                              onClick={() => undoChange(c.id)}
                              aria-label={`Undo: ${c.summary}`}
                            >
                              Undo
                            </button>
                          )}
                        </div>
                        {undoErrors[c.id] && <p className="pl-5 text-destructive">{undoErrors[c.id]}</p>}
                      </li>
                    ))}
                  </ul>
                )}
                {turn.tools?.some((t) => t.held) && (
                  <p className="flex items-center gap-1 px-2 text-xs text-muted-foreground">
                    <Hand className="h-3 w-3" />
                    Held back to ask you first:{" "}
                    {turn.tools
                      .filter((t) => t.held)
                      .map((t) => toolLabel(t.name).toLowerCase())
                      .join(", ")}
                  </p>
                )}
                {turn.tools && turn.tools.some((t) => t.name in TOOL_LABELS) && (
                  <p className="px-2 text-xs text-muted-foreground">
                    Looked up:{" "}
                    {turn.tools
                      .filter((t) => t.name in TOOL_LABELS)
                      .map((t, j) => (
                        <span key={j} className={cn(!t.ok && "text-destructive")}>
                          {j > 0 && ", "}
                          {toolLabel(t.name).toLowerCase()}
                        </span>
                      ))}
                  </p>
                )}
              </div>
            ),
          )}

          {pending && (pending.steps.length > 0 || !pending.text) && (
            <div className="rounded-2xl rounded-bl-sm bg-muted px-4 py-3 space-y-1 text-sm text-muted-foreground w-fit">
              {pending.steps.length === 0 && (
                <p className="flex items-center gap-2">
                  <Loader2 className="h-4 w-4 animate-spin" /> Thinking…
                </p>
              )}
              {pending.steps.map((step, i) => (
                <p key={i} className="flex items-center gap-2">
                  {step.done ? (
                    step.held ? (
                      <Hand className="h-4 w-4" />
                    ) : step.ok ? (
                      <Check className="h-4 w-4" />
                    ) : (
                      <X className="h-4 w-4 text-destructive" />
                    )
                  ) : (
                    <Loader2 className="h-4 w-4 animate-spin" />
                  )}
                  {toolLabel(step.name)}{step.held ? " — held back to ask you first" : "…"}
                </p>
              ))}
            </div>
          )}

          {pending?.text && (
            <div className="max-w-full rounded-2xl rounded-bl-sm bg-muted px-4 py-3" aria-live="polite">
              <Markdown text={pending.text} />
            </div>
          )}

          {error && <p className="text-sm text-destructive">{error}</p>}
          <div ref={bottomRef} />
        </div>

        <form
          className="sticky bottom-20 sm:bottom-0 mt-4 flex items-end gap-2 bg-background py-2"
          onSubmit={(e) => {
            e.preventDefault();
            send(input);
          }}
        >
          <textarea
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                send(input);
              }
            }}
            rows={Math.min(5, Math.max(1, input.split("\n").length))}
            placeholder={chatTask?.ready ? "Ask about your finances…" : "Choose a chat model in Settings first"}
            disabled={!chatTask?.ready}
            aria-label="Message"
            className="flex-1 resize-none rounded-md border border-input bg-transparent px-3 py-2 text-sm shadow-sm focus:outline-none focus:ring-1 focus:ring-ring disabled:opacity-50"
          />
          <Button type="submit" size="icon" disabled={!input.trim() || !!pending || !chatTask?.ready} aria-label="Send">
            <Send className="h-4 w-4" />
          </Button>
        </form>
        {chatTask?.ready && (
          <p className="text-xs text-muted-foreground">
            Answers come from your data but can still be wrong.
          </p>
        )}
      </div>
    </div>
  );
}

// react-markdown passes each renderer the syntax-tree node, which mustn't reach the DOM.
function withoutNode<T extends { node?: unknown }>(props: T): Omit<T, "node"> {
  const { node, ...rest } = props;
  void node;
  return rest;
}

function Markdown({ text }: { text: string }) {
  return (
    <div className="text-sm leading-relaxed space-y-2 break-words">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          ul: (props) => <ul className="list-disc pl-5 space-y-1" {...withoutNode(props)} />,
          ol: (props) => <ol className="list-decimal pl-5 space-y-1" {...withoutNode(props)} />,
          h1: (props) => <p className="font-semibold" {...withoutNode(props)} />,
          h2: (props) => <p className="font-semibold" {...withoutNode(props)} />,
          h3: (props) => <p className="font-semibold" {...withoutNode(props)} />,
          a: (props) => <a className="text-primary underline" target="_blank" rel="noreferrer" {...withoutNode(props)} />,
          code: (props) => <code className="rounded bg-background px-1 py-0.5 text-xs" {...withoutNode(props)} />,
          table: (props) => (
            <div className="overflow-x-auto">
              <table className="w-full border-collapse text-sm" {...withoutNode(props)} />
            </div>
          ),
          th: (props) => <th className="border-b px-2 py-1 text-left font-medium" {...withoutNode(props)} />,
          td: (props) => <td className="border-b border-border/50 px-2 py-1" {...withoutNode(props)} />,
        }}
      >
        {text}
      </ReactMarkdown>
    </div>
  );
}
