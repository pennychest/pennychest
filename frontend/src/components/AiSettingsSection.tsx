import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { AlertCircle, Check, ChevronDown, ChevronRight, RefreshCw } from "lucide-react";
import { aiApi, type AiConfig, type AiProvider, type AiTask, type ChatScope } from "../api/client";
import { cn } from "../lib/utils";
import { Badge } from "./ui/badge";
import { Button } from "./ui/button";
import { Card, CardContent } from "./ui/card";
import { Input } from "./ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "./ui/select";

const CONFIG_KEY = ["ai-config"];

const AUTOMATIC_TASKS: AiTask[] = ["taps", "categorise", "check_actions", "matching", "insights"];
const ON_DEMAND_TASKS: AiTask[] = ["rules", "chat"];

const USED_FOR: Record<AiTask, string> = {
  taps: "card taps",
  categorise: "categorising",
  rules: "rule suggestions",
  chat: "chat",
  check_actions: "checking chat actions",
  matching: "matching",
  insights: "subscriptions and unusual charges",
};

const TASK_HELP: Record<AiTask, string> = {
  taps: "Suggests a category for each card tap as it arrives. Your rules still come first.",
  categorise:
    "Picks a category for imported transactions that no rule or card tap covered. You can also run it from an import's review page.",
  rules: "Proposes categorisation rules from your transactions. Needs a general-purpose model.",
  chat: "Answers questions about your finances and, if you allow it below, makes changes for you.",
  check_actions:
    "Before chat makes a change, checks that it's what you asked for. If it isn't, the change is held back and chat asks you first.",
  matching:
    "When a card tap could be one of several statement lines, picks the right one. On import, links transfers between your accounts and flags transactions you already have.",
  insights:
    "After each import, marks which regular payments are subscriptions or bills, and flags charges that are unusual for the merchant or category. Chat uses both.",
};

export function AiSettingsSection() {
  const { data: config, isLoading, error } = useQuery({ queryKey: CONFIG_KEY, queryFn: aiApi.config });

  if (isLoading) return <div className="text-sm text-muted-foreground">Loading…</div>;
  if (error || !config) {
    return <p className="text-sm text-destructive">Couldn't load AI settings: {error?.message}</p>;
  }

  return (
    <div className="space-y-6">
      <section className="space-y-3">
        <div>
          <h2 className="text-lg font-semibold">Automatic</h2>
          <p className="text-sm text-muted-foreground">
            Runs by itself whenever new data arrives. Choose a provider to turn it on.
          </p>
        </div>
        <LearnedCard />
        {AUTOMATIC_TASKS.map((task) => (
          <TaskCard key={task} task={task} config={config} />
        ))}
      </section>

      <section className="space-y-3">
        <div>
          <h2 className="text-lg font-semibold">On demand</h2>
          <p className="text-sm text-muted-foreground">Runs when you press a button or ask.</p>
        </div>
        {ON_DEMAND_TASKS.map((task) => (
          <TaskCard key={task} task={task} config={config} />
        ))}
      </section>

      <section className="space-y-3">
        <div>
          <h2 className="text-lg font-semibold">Providers</h2>
          <p className="text-sm text-muted-foreground">
            Connect the providers you want to use. Keys are stored in your PennyChest database and
            are never shown again after saving.
          </p>
        </div>
        {config.providers.map((provider) => (
          <ProviderCard key={provider.id} provider={provider} />
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

function TaskCard({ task, config }: { task: AiTask; config: AiConfig }) {
  const queryClient = useQueryClient();
  const current = config.tasks[task];
  const eligible = config.providers.filter((p) => p.tasks.includes(task));
  const [draftProviderId, setDraftProviderId] = useState<string | null>(current.provider);
  const providerId = draftProviderId ?? current.provider;
  const provider = eligible.find((p) => p.id === providerId) ?? null;
  const refresh = useRefreshModels(provider?.id ?? null);

  const choose = useMutation({
    mutationFn: ({ providerId, model }: { providerId: string | null; model: string | null }) =>
      aiApi.chooseModel(task, providerId, model),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: CONFIG_KEY }),
  });

  // Keep the saved model selectable even if it isn't in the fetched list (e.g. an alias).
  const models = provider ? [...provider.models] : [];
  if (provider && provider.id === current.provider && current.model &&
      !models.some((m) => m.id === current.model)) {
    models.unshift({ id: current.model, label: current.model });
  }
  // Only providers that are set up, plus whichever this task already uses
  const choices = eligible.filter((p) => p.configured || p.id === provider?.id);
  const selectedModel = provider?.id === current.provider ? current.model ?? undefined : undefined;
  const error = choose.error?.message ?? refresh.error?.message;

  return (
    <Card>
      <CardContent className="pt-4 space-y-3">
        <div>
          <div className="flex items-center gap-2">
            <p className="text-sm font-medium">{current.label}</p>
          </div>
          <p className="text-xs text-muted-foreground">{TASK_HELP[task]}</p>
          {task === "taps" && (
            <p className="text-xs text-muted-foreground mt-1">
              Runs on every tap, so a fast, cheap model such as Jev works best.
            </p>
          )}
          {task === "insights" && (
            <p className="text-xs text-muted-foreground mt-1">
              Runs on every imported charge, so a fast, cheap model such as Jev works best.
            </p>
          )}
          {task === "matching" && (
            <p className="text-xs text-muted-foreground mt-1">
              Only acts when it's confident. A fast, cheap model such as Jev works best.
            </p>
          )}
          {task === "check_actions" && (
            <p className="text-xs text-muted-foreground mt-1">
              Runs before every change, so a fast, cheap model such as Jev works best.
            </p>
          )}
        </div>

        <div className="grid gap-2 sm:grid-cols-2">
          <Select
            value={provider?.id ?? ""}
            onValueChange={(id) => {
              setDraftProviderId(id);
              refresh.reset();
            }}
            disabled={choices.length === 0}
          >
            <SelectTrigger aria-label={`${current.label} provider`}>
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
              <SelectTrigger aria-label={`${current.label} model`}>
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
          <p className="text-xs text-muted-foreground">Set up {provider.label} under Providers below first.</p>
        )}
        {provider?.configured && models.length === 0 && !refresh.isPending && (
          <p className="text-xs text-muted-foreground">Refresh to load {provider.label}'s models.</p>
        )}
        {error && <p className="text-xs text-destructive">{error}</p>}

        {task === "chat" && <ChatPermissions scopes={current.scopes ?? []} />}
        {task === "categorise" && <AutoCategoriseSwitch enabled={current.auto ?? true} />}

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
            <p className="text-xs text-muted-foreground">Not set up.</p>
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
              Turn off
            </Button>
          )}
        </div>
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

function AutoCategoriseSwitch({ enabled }: { enabled: boolean }) {
  const queryClient = useQueryClient();
  const save = useMutation({
    mutationFn: aiApi.setAutoCategorise,
    onMutate: async (next: boolean) => {
      await queryClient.cancelQueries({ queryKey: CONFIG_KEY });
      const previous = queryClient.getQueryData<AiConfig>(CONFIG_KEY);
      if (previous) {
        queryClient.setQueryData<AiConfig>(CONFIG_KEY, {
          ...previous,
          tasks: { ...previous.tasks, categorise: { ...previous.tasks.categorise, auto: next } },
        });
      }
      return { previous };
    },
    onError: (_error, _next, context) => {
      if (context?.previous) queryClient.setQueryData(CONFIG_KEY, context.previous);
    },
    onSettled: () => queryClient.invalidateQueries({ queryKey: CONFIG_KEY }),
  });

  return (
    <label className="flex items-start gap-2 text-sm cursor-pointer">
      <input
        type="checkbox"
        className="mt-0.5 h-4 w-4 accent-primary"
        checked={enabled}
        disabled={save.isPending}
        onChange={(e) => save.mutate(e.target.checked)}
      />
      <span>
        Run straight after each import
        <span className="block text-xs text-muted-foreground">
          Each run can be undone from Settings → Changes made by AI.
        </span>
      </span>
    </label>
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
  const usedFor = [...AUTOMATIC_TASKS, ...ON_DEMAND_TASKS]
    .filter((t) => provider.tasks.includes(t))
    .map((t) => USED_FOR[t])
    .join(", ");

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
          <span className="hidden sm:inline text-xs text-muted-foreground">Used for {usedFor}</span>
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
            <p className="sm:hidden text-xs text-muted-foreground">Used for {usedFor}</p>
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
