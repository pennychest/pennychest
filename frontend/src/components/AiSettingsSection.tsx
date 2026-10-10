import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { AlertCircle, Check, ChevronDown, ChevronRight, RefreshCw } from "lucide-react";
import {
  aiApi,
  type AiConfig,
  type AiKind,
  type AiMode,
  type AiProvider,
  type AiTask,
  type ChatScope,
} from "../api/client";
import { cn } from "../lib/utils";
import { Badge } from "./ui/badge";
import { Button } from "./ui/button";
import { Card, CardContent } from "./ui/card";
import { Input } from "./ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "./ui/select";

const CONFIG_KEY = ["ai-config"];

const TASK_ORDER: AiTask[] = ["categorise", "taps", "matching", "insights", "rules", "chat", "check_actions"];

const KINDS: { kind: AiKind; title: string; label: string; help: string }[] = [
  {
    kind: "decision",
    title: "Decision model",
    label: "Decision model",
    help:
      "Picks from fixed answers and says how sure it is. Fast and cheap, so it suits jobs that run on every import and card tap. It can't chat or write rules.",
  },
  {
    kind: "llm",
    title: "Language model",
    label: "Language model",
    help: "Writes text, so it can do everything, including chat and suggesting rules. Slower, and costs more per request.",
  },
];

const KIND_LABEL: Record<AiKind, string> = { decision: "Decision model", llm: "Language model" };

const TASK_HELP: Record<AiTask, string> = {
  taps: "Suggests a category for each card tap. Your rules still come first.",
  categorise: "Picks a category for imported transactions that no rule or card tap covered.",
  rules: "Proposes categorisation rules from your transactions.",
  chat: "Answers questions about your finances and, if you allow it below, makes changes for you.",
  check_actions:
    "Before chat makes a change, checks that it's what you asked for. If it isn't, the change is held back and chat asks you first.",
  matching:
    "Links card taps and transfers between your accounts to the right statement lines, and flags transactions you already have.",
  insights: "Marks which regular payments are subscriptions or bills, and flags unusual charges. Chat uses both.",
};

// Where to run each task when it's on demand
const ON_DEMAND_HELP: Partial<Record<AiTask, string>> = {
  taps: "Ask for a category on each tap on the Card taps page.",
  categorise: "Run it from an import's review page.",
  matching: "Run it from an import's review page, or with Reconcile on the Card taps page.",
  insights: "Run it from an import's review page.",
};

const AUTOMATIC_HELP: Partial<Record<AiTask, string>> = {
  taps: "Runs on every card tap as it arrives.",
  categorise: "Runs straight after each import. Each run can be undone from Settings → Changes made by AI.",
  matching: "Runs on every import and card tap, and only acts when it's confident.",
  insights: "Runs after each import.",
};

export function AiSettingsSection() {
  const { data: config, isLoading, error } = useQuery({ queryKey: CONFIG_KEY, queryFn: aiApi.config });

  if (isLoading) return <div className="text-sm text-muted-foreground">Loading…</div>;
  if (error || !config) {
    return <p className="text-sm text-destructive">Couldn't load AI settings: {error?.message}</p>;
  }

  return (
    <div className="space-y-6">
      {KINDS.map(({ kind, title, help }) => (
        <section key={kind} className="space-y-3">
          <div>
            <h2 className="text-lg font-semibold">{title}</h2>
            <p className="text-sm text-muted-foreground">{help}</p>
          </div>
          <ModelCard kind={kind} config={config} />
          {config.providers
            .filter((p) => p.kind === kind)
            .map((provider) => (
              <ProviderCard key={provider.id} provider={provider} />
            ))}
        </section>
      ))}

      <section className="space-y-3">
        <div>
          <h2 className="text-lg font-semibold">Tasks</h2>
          <p className="text-sm text-muted-foreground">
            Turn each one on or off, choose which model it uses, and whether it runs by itself or when you ask.
          </p>
        </div>
        <LearnedCard />
        {TASK_ORDER.map((task) => (
          <TaskCard key={task} task={task} config={config} />
        ))}
      </section>
    </div>
  );
}

function useRefreshModels(providerId: string | null) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: () => aiApi.refreshModels(providerId!),
    // Either way: a rejected key changes the provider's status too
    onSettled: () => queryClient.invalidateQueries({ queryKey: CONFIG_KEY }),
  });
}

// The one model of a kind that every task using that kind shares
function ModelCard({ kind, config }: { kind: AiKind; config: AiConfig }) {
  const queryClient = useQueryClient();
  const current = config.models[kind];
  const eligible = config.providers.filter((p) => p.kind === kind);
  const [draftProviderId, setDraftProviderId] = useState<string | null>(current.provider);
  const providerId = draftProviderId ?? current.provider;
  const provider = eligible.find((p) => p.id === providerId) ?? null;
  const refresh = useRefreshModels(provider?.id ?? null);

  const choose = useMutation({
    mutationFn: ({ providerId, model }: { providerId: string | null; model: string | null }) =>
      aiApi.chooseModel(kind, providerId, model),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: CONFIG_KEY }),
  });

  // Keep the saved model selectable even if it isn't in the fetched list (e.g. an alias).
  const models = provider ? [...provider.models] : [];
  if (provider && provider.id === current.provider && current.model &&
      !models.some((m) => m.id === current.model)) {
    models.unshift({ id: current.model, label: current.model });
  }
  // Only providers that are set up, plus whichever is already chosen
  const choices = eligible.filter((p) => p.configured || p.id === provider?.id);
  const selectedModel = provider?.id === current.provider ? current.model ?? undefined : undefined;
  const error = choose.error?.message ?? refresh.error?.message;
  const label = KIND_LABEL[kind];

  return (
    <Card>
      <CardContent className="pt-4 space-y-3">
        <div className="grid gap-2 sm:grid-cols-2">
          <Select
            value={provider?.id ?? ""}
            onValueChange={(id) => {
              setDraftProviderId(id);
              refresh.reset();
            }}
            disabled={choices.length === 0}
          >
            <SelectTrigger aria-label={`${label} provider`}>
              <SelectValue
                placeholder={choices.length === 0 ? "Set up a provider below first" : "Choose a provider"}
              />
            </SelectTrigger>
            <SelectContent>
              {choices.map((p) => (
                <SelectItem key={p.id} value={p.id}>
                  {p.label}
                  {!p.configured ? " (not set up)" : p.problem ? " (not working)" : ""}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>

          <div className="flex gap-2">
            <Select
              value={selectedModel ?? ""}
              onValueChange={(model) => choose.mutate({ providerId: provider!.id, model })}
              disabled={!provider || models.length === 0 || choose.isPending}
            >
              <SelectTrigger aria-label={`${label} model`}>
                <SelectValue placeholder={provider && models.length === 0 ? "No models loaded" : "Choose a model"} />
              </SelectTrigger>
              <SelectContent>
                {models.map((m) => (
                  <SelectItem key={m.id} value={m.id}>
                    {m.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Button
              variant="outline"
              size="icon"
              className="shrink-0"
              onClick={() => refresh.mutate()}
              disabled={!provider?.configured || refresh.isPending}
              title="Refresh the model list"
              aria-label="Refresh the model list"
            >
              <RefreshCw className={cn("h-4 w-4", refresh.isPending && "animate-spin")} />
            </Button>
          </div>
        </div>

        {provider && !provider.configured && (
          <p className="text-xs text-muted-foreground">Set up {provider.label} below first.</p>
        )}
        {provider?.configured && models.length === 0 && !refresh.isPending && (
          <p className="text-xs text-muted-foreground">Refresh to load {provider.label}'s models.</p>
        )}
        {error && <p className="text-xs text-destructive">{error}</p>}

        <div className="flex items-center justify-between gap-2">
          {current.ready ? (
            <p className="text-xs text-muted-foreground flex items-center gap-1">
              <Check className="h-3 w-3" />
              Using {config.providers.find((p) => p.id === current.provider)?.label} · {current.model}
            </p>
          ) : current.provider ? (
            <p className="text-xs text-warning flex items-center gap-1">
              <AlertCircle className="h-3 w-3 shrink-0" />
              {current.problem}
            </p>
          ) : (
            <p className="text-xs text-muted-foreground">No {label.toLowerCase()} chosen.</p>
          )}
          {current.provider && (
            <Button
              variant="ghost"
              size="sm"
              onClick={() => {
                setDraftProviderId(null);
                choose.mutate({ providerId: null, model: null });
              }}
              disabled={choose.isPending}
            >
              Clear
            </Button>
          )}
        </div>
      </CardContent>
    </Card>
  );
}

function Segmented<T extends string>({
  label,
  value,
  options,
  onChange,
  disabled,
}: {
  label: string;
  value: T;
  options: { value: T; label: string }[];
  onChange: (value: T) => void;
  disabled?: boolean;
}) {
  return (
    <div className="flex flex-wrap items-center gap-2">
      <span className="text-xs text-muted-foreground w-12">{label}</span>
      <div role="radiogroup" aria-label={label} className="inline-flex rounded-md border p-0.5">
        {options.map((option) => (
          <button
            key={option.value}
            type="button"
            role="radio"
            aria-checked={value === option.value}
            disabled={disabled}
            onClick={() => value !== option.value && onChange(option.value)}
            className={cn(
              "rounded px-3 py-1 text-xs font-medium transition-colors",
              value === option.value ? "bg-primary text-primary-foreground" : "hover:bg-accent",
            )}
          >
            {option.label}
          </button>
        ))}
      </div>
    </div>
  );
}

function TaskCard({ task, config }: { task: AiTask; config: AiConfig }) {
  const queryClient = useQueryClient();
  const current = config.tasks[task];
  const update = useMutation({
    mutationFn: (settings: { enabled?: boolean; kind?: AiKind; mode?: AiMode }) =>
      aiApi.updateTask(task, settings),
    // Show the change straight away; roll back if saving fails.
    onMutate: async (settings) => {
      await queryClient.cancelQueries({ queryKey: CONFIG_KEY });
      const previous = queryClient.getQueryData<AiConfig>(CONFIG_KEY);
      if (previous) {
        queryClient.setQueryData<AiConfig>(CONFIG_KEY, {
          ...previous,
          tasks: { ...previous.tasks, [task]: { ...previous.tasks[task], ...settings } },
        });
      }
      return { previous };
    },
    onError: (_error, _settings, context) => {
      if (context?.previous) queryClient.setQueryData(CONFIG_KEY, context.previous);
    },
    onSettled: () => queryClient.invalidateQueries({ queryKey: CONFIG_KEY }),
  });

  const providerLabel = config.providers.find((p) => p.id === current.provider)?.label;
  const modeHelp =
    current.mode === "automatic" ? AUTOMATIC_HELP[task] : current.mode === "on_demand" ? ON_DEMAND_HELP[task] : null;

  return (
    <Card>
      <CardContent className="pt-4 space-y-3">
        <label className="flex items-start gap-3 cursor-pointer">
          <input
            type="checkbox"
            className="mt-0.5 h-4 w-4 accent-primary"
            checked={current.enabled}
            onChange={(e) => update.mutate({ enabled: e.target.checked })}
            aria-label={`Use AI for ${current.label.toLowerCase()}`}
          />
          <span>
            <span className="block text-sm font-medium">{current.label}</span>
            <span className="block text-xs text-muted-foreground">{TASK_HELP[task]}</span>
          </span>
        </label>

        {current.enabled && (
          <div className="space-y-2 pl-7">
            {current.kinds.length > 1 && (
              <Segmented
                label="Uses"
                value={current.kind}
                options={current.kinds.map((kind) => ({ value: kind, label: KIND_LABEL[kind] }))}
                onChange={(kind) => update.mutate({ kind })}
              />
            )}
            {current.mode && (
              <Segmented
                label="Runs"
                value={current.mode}
                options={[
                  { value: "automatic", label: "Automatically" },
                  { value: "on_demand", label: "When I ask" },
                ]}
                onChange={(mode) => update.mutate({ mode })}
              />
            )}
            {modeHelp && <p className="text-xs text-muted-foreground">{modeHelp}</p>}
            {current.mode === "automatic" && current.kind === "llm" && (
              <p className="text-xs text-muted-foreground">
                A language model on every {task === "taps" ? "tap" : "import"} can add up; a decision model is faster and
                cheaper.
              </p>
            )}

            {task === "chat" && <ChatPermissions scopes={current.scopes ?? []} />}

            {current.ready ? (
              <p className="text-xs text-muted-foreground flex items-center gap-1">
                <Check className="h-3 w-3" />
                Using {providerLabel} · {current.model}
              </p>
            ) : (
              <p className="text-xs text-warning flex items-center gap-1">
                <AlertCircle className="h-3 w-3 shrink-0" />
                {current.problem}
              </p>
            )}
          </div>
        )}
        {update.error && <p className="text-xs text-destructive">{update.error.message}</p>}
      </CardContent>
    </Card>
  );
}

const LEARNED_KEY = ["ai-learned"];

function percent(value: number) {
  return `${Math.round(value * 100)}%`;
}

// The categoriser that learns from the user's own history. It runs before any AI provider.
function LearnedCard() {
  const queryClient = useQueryClient();
  const { data: status } = useQuery({ queryKey: LEARNED_KEY, queryFn: aiApi.learnedStatus });
  const toggle = useMutation({
    mutationFn: aiApi.setLearned,
    onSettled: () => queryClient.invalidateQueries({ queryKey: LEARNED_KEY }),
  });
  if (!status) return null;

  return (
    <Card>
      <CardContent className="pt-4 space-y-3">
        <div>
          <p className="text-sm font-medium">Learning from your categories</p>
          <p className="text-xs text-muted-foreground">
            Categorises imports and card taps the way you have before, with no AI provider needed. It
            goes after your rules and before the AI, and only answers when it's at least{" "}
            {percent(status.confidence)} sure.
          </p>
        </div>
        <label className="flex items-start gap-2 text-sm cursor-pointer">
          <input
            type="checkbox"
            className="mt-0.5 h-4 w-4 accent-primary"
            checked={status.enabled}
            disabled={toggle.isPending}
            onChange={(e) => toggle.mutate(e.target.checked)}
          />
          <span>Use it</span>
        </label>
        {status.ready ? (
          <p className="text-xs text-muted-foreground">
            Learned from {status.examples} transactions in {status.categories} categories.
            {status.check && status.check.accuracy !== null && (
              <>
                {" "}
                On your past transactions it would have answered {percent(status.check.coverage)} of
                them, getting {percent(status.check.accuracy)} of those right.
              </>
            )}
          </p>
        ) : (
          <p className="text-xs text-muted-foreground">
            It starts once you've categorised or confirmed {status.min_examples} transactions (
            {status.examples} so far).
          </p>
        )}
      </CardContent>
    </Card>
  );
}

const CHAT_PERMISSIONS: { scope: ChatScope; label: string; help: string }[] = [
  {
    scope: "organise",
    label: "Can change categories, rules and budgets",
    help: "Create and rename categories, add rules and set budgets.",
  },
  {
    scope: "transactions",
    label: "Can change transactions",
    help: "Add, edit, recategorise, review and delete transactions.",
  },
];

function ChatPermissions({ scopes }: { scopes: ChatScope[] }) {
  const queryClient = useQueryClient();
  const save = useMutation({
    mutationFn: aiApi.setChatScopes,
    // Tick the box straight away; roll back if saving fails.
    onMutate: async (next: ChatScope[]) => {
      await queryClient.cancelQueries({ queryKey: CONFIG_KEY });
      const previous = queryClient.getQueryData<AiConfig>(CONFIG_KEY);
      if (previous) {
        queryClient.setQueryData<AiConfig>(CONFIG_KEY, {
          ...previous,
          tasks: { ...previous.tasks, chat: { ...previous.tasks.chat, scopes: next } },
        });
      }
      return { previous };
    },
    onError: (_error, _next, context) => {
      if (context?.previous) queryClient.setQueryData(CONFIG_KEY, context.previous);
    },
    onSettled: () => queryClient.invalidateQueries({ queryKey: CONFIG_KEY }),
  });

  function toggle(scope: ChatScope, on: boolean) {
    save.mutate(on ? [...scopes, scope] : scopes.filter((s) => s !== scope));
  }

  return (
    <fieldset className="space-y-2 rounded-md border p-3">
      <legend className="px-1 text-xs font-medium text-muted-foreground">What chat may change</legend>
      {CHAT_PERMISSIONS.map(({ scope, label, help }) => (
        <label key={scope} className="flex items-start gap-2 text-sm cursor-pointer">
          <input
            type="checkbox"
            className="mt-0.5 h-4 w-4 accent-primary"
            checked={scopes.includes(scope)}
            disabled={save.isPending}
            onChange={(e) => toggle(scope, e.target.checked)}
          />
          <span>
            {label}
            <span className="block text-xs text-muted-foreground">{help}</span>
          </span>
        </label>
      ))}
      <p className="text-xs text-muted-foreground">
        Changes happen without asking first. Each one can be undone from the chat or from Settings →
        Changes made by AI.
      </p>
      {save.error && <p className="text-xs text-destructive">{save.error.message}</p>}
    </fieldset>
  );
}

function ProviderCard({ provider }: { provider: AiProvider }) {
  const queryClient = useQueryClient();
  const [open, setOpen] = useState(false);
  const [values, setValues] = useState<Record<string, string>>({});
  const [replacing, setReplacing] = useState<Record<string, boolean>>({});
  const refresh = useRefreshModels(provider.id);

  const save = useMutation({
    mutationFn: (changes: Record<string, string | null>) => aiApi.saveProvider(provider.id, changes),
    onSuccess: async () => {
      setValues({});
      setReplacing({});
      const config = await queryClient.fetchQuery({ queryKey: CONFIG_KEY, queryFn: aiApi.config, staleTime: 0 });
      // Loading the model list straight away confirms the credentials work.
      if (config.providers.find((p) => p.id === provider.id)?.configured) refresh.mutate();
    },
  });

  const changed = Object.fromEntries(
    Object.entries(values).filter(([key, value]) => {
      const field = provider.fields.find((f) => f.key === key);
      return field && value.trim() !== (field.secret ? "" : field.value ?? "");
    }),
  );

  return (
    <Card>
      <CardContent className="py-3 space-y-3">
        <button
          type="button"
          className="flex w-full items-center gap-2 text-left"
          onClick={() => setOpen((o) => !o)}
          aria-expanded={open}
        >
          {open ? <ChevronDown className="h-4 w-4 shrink-0" /> : <ChevronRight className="h-4 w-4 shrink-0" />}
          <span className="font-medium">{provider.label}</span>
          <Badge
            variant={provider.problem ? "destructive" : provider.configured ? "success" : "outline"}
            className="ml-auto"
          >
            {provider.problem ? "Not working" : provider.configured ? "Set up" : "Not set up"}
          </Badge>
        </button>

        {open && (
          <form
            className="space-y-3 pl-6"
            onSubmit={(e) => {
              e.preventDefault();
              save.mutate(changed);
            }}
          >
            {provider.fields.map((field) => {
              const showSaved = field.secret && field.is_set && !replacing[field.key];
              return (
                <div key={field.key} className="space-y-1">
                  <label className="text-sm font-medium" htmlFor={`${provider.id}-${field.key}`}>
                    {field.label}
                    {!field.required && <span className="text-muted-foreground font-normal"> (optional)</span>}
                  </label>
                  {showSaved ? (
                    <div className="flex items-center gap-2">
                      <span className="text-sm text-muted-foreground flex items-center gap-1">
                        <Check className="h-3 w-3" /> Saved
                      </span>
                      <Button type="button" variant="outline" size="sm"
                        onClick={() => setReplacing((r) => ({ ...r, [field.key]: true }))}>
                        Replace
                      </Button>
                      <Button type="button" variant="ghost" size="sm"
                        onClick={() => save.mutate({ [field.key]: null })} disabled={save.isPending}>
                        Remove
                      </Button>
                    </div>
                  ) : (
                    <Input
                      id={`${provider.id}-${field.key}`}
                      type={field.secret ? "password" : "text"}
                      autoComplete="off"
                      placeholder={field.placeholder}
                      value={values[field.key] ?? (field.secret ? "" : field.value ?? "")}
                      onChange={(e) => setValues((v) => ({ ...v, [field.key]: e.target.value }))}
                    />
                  )}
                </div>
              );
            })}

            <div className="flex flex-wrap items-center gap-2">
              <Button type="submit" size="sm" disabled={Object.keys(changed).length === 0 || save.isPending}>
                Save
              </Button>
              {provider.configured && (
                <Button type="button" variant="outline" size="sm" onClick={() => refresh.mutate()}
                  disabled={refresh.isPending}>
                  <RefreshCw className={cn("h-4 w-4 mr-1", refresh.isPending && "animate-spin")} />
                  Refresh models
                </Button>
              )}
              {provider.models_updated_at && (
                <span className="text-xs text-muted-foreground">
                  {provider.models.length} models · updated{" "}
                  {new Date(provider.models_updated_at).toLocaleDateString()}
                </span>
              )}
            </div>
            {save.error && <p className="text-xs text-destructive">{save.error.message}</p>}
            {refresh.error ? (
              <p className="text-xs text-destructive">{refresh.error.message}</p>
            ) : (
              provider.problem && <p className="text-xs text-destructive">{provider.problem}</p>
            )}
            {refresh.isSuccess && (
              <p className="text-xs text-muted-foreground flex items-center gap-1">
                <Check className="h-3 w-3" /> Connected · {refresh.data.models.length} models available
              </p>
            )}
          </form>
        )}
      </CardContent>
    </Card>
  );
}
