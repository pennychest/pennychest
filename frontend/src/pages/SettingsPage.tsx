import { useState } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { BookOpen, ShoppingBag, TrendingUp, ChevronRight, ChevronLeft, Bot, Check, KeyRound, Nfc, Copy, Bug, ScrollText, LogOut, History, Plug, Download, Puzzle } from "lucide-react";
import { accountsApi, authApi, tapsApi } from "../api/client";
import { AiSettingsSection } from "../components/AiSettingsSection";
import { ChangesSection } from "../components/ChangesSection";
import { McpSection } from "../components/McpSection";
import { ExportSection } from "../components/ExportSection";
import { PluginsSection } from "../components/PluginsSection";
import { useSignOut } from "../lib/auth";
import { Card, CardContent } from "../components/ui/card";
import { Button } from "../components/ui/button";
import { Input } from "../components/ui/input";
import { CategoryGrid, buildChildMap } from "./AccountsPage";

type Section = "spending" | "income" | "ai" | "changes" | "mcp" | "password" | "taps" | "plugins" | "export" | "debug" | null;

// iCloud link to the shared "PennyChest Tap" shortcut; its import questions ask for the
// address and token shown in Settings.
const TAP_SHORTCUT_URL = "https://www.icloud.com/shortcuts/bfedfdd6c6fc428dbba076faf590cad8";

export function SettingsPage() {
  const [activeSection, setActiveSection] = useState<Section>(null);
  const signOut = useSignOut();

  const { data: accounts = [], isLoading } = useQuery({
    queryKey: ["accounts"],
    queryFn: accountsApi.list,
  });

  const childMap = buildChildMap(accounts);

  if (isLoading) {
    return <div className="text-muted-foreground text-sm p-4">Loading…</div>;
  }

  if (activeSection === "spending") {
    return (
      <div className="space-y-4">
        <div className="flex items-center gap-2">
          <Button variant="ghost" size="icon" onClick={() => setActiveSection(null)}>
            <ChevronLeft className="h-5 w-5" />
          </Button>
          <h1 className="text-3xl font-bold">Spending Categories</h1>
        </div>
        <CategoryGrid rootType="expense" accounts={accounts} childMap={childMap} />
      </div>
    );
  }

  if (activeSection === "income") {
    return (
      <div className="space-y-4">
        <div className="flex items-center gap-2">
          <Button variant="ghost" size="icon" onClick={() => setActiveSection(null)}>
            <ChevronLeft className="h-5 w-5" />
          </Button>
          <h1 className="text-3xl font-bold">Income Sources</h1>
        </div>
        <CategoryGrid rootType="income" accounts={accounts} childMap={childMap} />
      </div>
    );
  }

  if (activeSection === "ai") {
    return (
      <div className="space-y-4">
        <div className="flex items-center gap-2">
          <Button variant="ghost" size="icon" onClick={() => setActiveSection(null)}>
            <ChevronLeft className="h-5 w-5" />
          </Button>
          <h1 className="text-3xl font-bold">AI Integration</h1>
        </div>
        <AiSettingsSection />
      </div>
    );
  }

  if (activeSection === "taps") {
    return (
      <div className="space-y-4">
        <div className="flex items-center gap-2">
          <Button variant="ghost" size="icon" onClick={() => setActiveSection(null)}>
            <ChevronLeft className="h-5 w-5" />
          </Button>
          <h1 className="text-3xl font-bold">Card taps</h1>
        </div>
        <TapSettingsSection />
      </div>
    );
  }

  if (activeSection === "changes") {
    return (
      <div className="space-y-4">
        <div className="flex items-center gap-2">
          <Button variant="ghost" size="icon" onClick={() => setActiveSection(null)}>
            <ChevronLeft className="h-5 w-5" />
          </Button>
          <h1 className="text-3xl font-bold">Changes made by AI</h1>
        </div>
        <ChangesSection />
      </div>
    );
  }

  if (activeSection === "mcp") {
    return (
      <div className="space-y-4">
        <div className="flex items-center gap-2">
          <Button variant="ghost" size="icon" onClick={() => setActiveSection(null)}>
            <ChevronLeft className="h-5 w-5" />
          </Button>
          <h1 className="text-3xl font-bold">AI agents (MCP)</h1>
        </div>
        <McpSection />
      </div>
    );
  }

  if (activeSection === "plugins") {
    return (
      <div className="space-y-4">
        <div className="flex items-center gap-2">
          <Button variant="ghost" size="icon" onClick={() => setActiveSection(null)}>
            <ChevronLeft className="h-5 w-5" />
          </Button>
          <h1 className="text-3xl font-bold">Plugins</h1>
        </div>
        <PluginsSection />
      </div>
    );
  }

  if (activeSection === "export") {
    return (
      <div className="space-y-4">
        <div className="flex items-center gap-2">
          <Button variant="ghost" size="icon" onClick={() => setActiveSection(null)}>
            <ChevronLeft className="h-5 w-5" />
          </Button>
          <h1 className="text-3xl font-bold">Export</h1>
        </div>
        <ExportSection />
      </div>
    );
  }

  if (activeSection === "debug") {
    return (
      <div className="space-y-4">
        <div className="flex items-center gap-2">
          <Button variant="ghost" size="icon" onClick={() => setActiveSection(null)}>
            <ChevronLeft className="h-5 w-5" />
          </Button>
          <h1 className="text-3xl font-bold">Debug</h1>
        </div>
        <Link to="/settings/ai-logs" className="block">
          <Card className="hover:bg-accent transition-colors cursor-pointer">
            <CardContent className="flex items-center gap-3 py-4">
              <ScrollText className="h-5 w-5 text-muted-foreground" />
              <div>
                <p className="font-medium">AI request logs</p>
                <p className="text-xs text-muted-foreground">Every request sent to an AI provider, with timings and errors.</p>
              </div>
              <ChevronRight className="h-4 w-4 text-muted-foreground ml-auto" />
            </CardContent>
          </Card>
        </Link>
      </div>
    );
  }

  if (activeSection === "password") {
    return (
      <div className="space-y-4">
        <div className="flex items-center gap-2">
          <Button variant="ghost" size="icon" onClick={() => setActiveSection(null)}>
            <ChevronLeft className="h-5 w-5" />
          </Button>
          <h1 className="text-3xl font-bold">Password</h1>
        </div>
        <PasswordSection />
      </div>
    );
  }

  return (
    <div className="space-y-4">
      <h1 className="text-3xl font-bold">Settings</h1>

      <Card className="hover:bg-accent transition-colors cursor-pointer" onClick={() => setActiveSection("spending")}>
        <CardContent className="flex items-center gap-3 py-4">
          <ShoppingBag className="h-5 w-5 text-muted-foreground" />
          <span className="font-medium">Spending Categories</span>
          <ChevronRight className="h-4 w-4 text-muted-foreground ml-auto" />
        </CardContent>
      </Card>

      <Card className="hover:bg-accent transition-colors cursor-pointer" onClick={() => setActiveSection("income")}>
        <CardContent className="flex items-center gap-3 py-4">
          <TrendingUp className="h-5 w-5 text-muted-foreground" />
          <span className="font-medium">Income Sources</span>
          <ChevronRight className="h-4 w-4 text-muted-foreground ml-auto" />
        </CardContent>
      </Card>

      <Link to="/rules" className="block">
        <Card className="hover:bg-accent transition-colors cursor-pointer">
          <CardContent className="flex items-center gap-3 py-4">
            <BookOpen className="h-5 w-5 text-muted-foreground" />
            <span className="font-medium">Categorisation Rules</span>
            <ChevronRight className="h-4 w-4 text-muted-foreground ml-auto" />
          </CardContent>
        </Card>
      </Link>

      <Card
        className="hover:bg-accent transition-colors cursor-pointer"
        onClick={() => setActiveSection("ai")}
      >
        <CardContent className="flex items-center gap-3 py-4">
          <Bot className="h-5 w-5 text-muted-foreground" />
          <span className="font-medium">AI Integration</span>
          <ChevronRight className="h-4 w-4 text-muted-foreground ml-auto" />
        </CardContent>
      </Card>

      <Card
        className="hover:bg-accent transition-colors cursor-pointer"
        onClick={() => setActiveSection("changes")}
      >
        <CardContent className="flex items-center gap-3 py-4">
          <History className="h-5 w-5 text-muted-foreground" />
          <span className="font-medium">Changes made by AI</span>
          <ChevronRight className="h-4 w-4 text-muted-foreground ml-auto" />
        </CardContent>
      </Card>

      <Card
        className="hover:bg-accent transition-colors cursor-pointer"
        onClick={() => setActiveSection("mcp")}
      >
        <CardContent className="flex items-center gap-3 py-4">
          <Plug className="h-5 w-5 text-muted-foreground" />
          <span className="font-medium">AI agents (MCP)</span>
          <ChevronRight className="h-4 w-4 text-muted-foreground ml-auto" />
        </CardContent>
      </Card>

      <Card
        className="hover:bg-accent transition-colors cursor-pointer"
        onClick={() => setActiveSection("taps")}
      >
        <CardContent className="flex items-center gap-3 py-4">
          <Nfc className="h-5 w-5 text-muted-foreground" />
          <span className="font-medium">Card taps</span>
          <ChevronRight className="h-4 w-4 text-muted-foreground ml-auto" />
        </CardContent>
      </Card>

      <Card
        className="hover:bg-accent transition-colors cursor-pointer"
        onClick={() => setActiveSection("password")}
      >
        <CardContent className="flex items-center gap-3 py-4">
          <KeyRound className="h-5 w-5 text-muted-foreground" />
          <span className="font-medium">Password</span>
          <ChevronRight className="h-4 w-4 text-muted-foreground ml-auto" />
        </CardContent>
      </Card>

      <Card
        className="hover:bg-accent transition-colors cursor-pointer"
        onClick={() => setActiveSection("plugins")}
      >
        <CardContent className="flex items-center gap-3 py-4">
          <Puzzle className="h-5 w-5 text-muted-foreground" />
          <span className="font-medium">Plugins</span>
          <ChevronRight className="h-4 w-4 text-muted-foreground ml-auto" />
        </CardContent>
      </Card>

      <Card
        className="hover:bg-accent transition-colors cursor-pointer"
        onClick={() => setActiveSection("export")}
      >
        <CardContent className="flex items-center gap-3 py-4">
          <Download className="h-5 w-5 text-muted-foreground" />
          <span className="font-medium">Export</span>
          <ChevronRight className="h-4 w-4 text-muted-foreground ml-auto" />
        </CardContent>
      </Card>

      <Card
        className="hover:bg-accent transition-colors cursor-pointer"
        onClick={() => setActiveSection("debug")}
      >
        <CardContent className="flex items-center gap-3 py-4">
          <Bug className="h-5 w-5 text-muted-foreground" />
          <span className="font-medium">Debug</span>
          <ChevronRight className="h-4 w-4 text-muted-foreground ml-auto" />
        </CardContent>
      </Card>

      <Card
        className="hover:bg-accent transition-colors cursor-pointer"
        onClick={() => !signOut.isPending && signOut.mutate()}
        role="button"
        aria-label="Sign out"
      >
        <CardContent className="flex items-center gap-3 py-4">
          <LogOut className="h-5 w-5 text-muted-foreground" />
          <span className="font-medium">Sign out</span>
        </CardContent>
      </Card>
    </div>
  );
}

function CopyField({ label, value, hidden = false }: { label: string; value: string; hidden?: boolean }) {
  const [copied, setCopied] = useState(false);
  const [failed, setFailed] = useState(false);

  async function copy() {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(true);
      setFailed(false);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      // The clipboard API needs HTTPS; fall back to selecting the text by hand.
      setFailed(true);
    }
  }

  return (
    <div className="space-y-1">
      <p className="text-sm font-medium">{label}</p>
      <div className="flex gap-2">
        <Input
          readOnly
          type={hidden ? "password" : "text"}
          value={value}
          onFocus={(e) => e.target.select()}
          className="font-mono text-xs"
        />
        <Button type="button" variant="outline" onClick={copy} className="shrink-0">
          {copied ? <Check className="h-4 w-4" /> : <Copy className="h-4 w-4" />}
          <span className="ml-1">{copied ? "Copied" : "Copy"}</span>
        </Button>
      </div>
      {failed && (
        <p className="text-xs text-muted-foreground">Couldn't copy automatically. Select the text and copy it instead.</p>
      )}
    </div>
  );
}

function TapSettingsSection() {
  const queryClient = useQueryClient();
  const [showToken, setShowToken] = useState(false);

  const { data, isLoading } = useQuery({
    queryKey: ["tap-token"],
    queryFn: tapsApi.getToken,
  });
  const token = data?.token ?? null;

  const regenerateMutation = useMutation({
    mutationFn: tapsApi.regenerateToken,
    onSuccess: (result) => queryClient.setQueryData(["tap-token"], result),
  });
  const clearMutation = useMutation({
    mutationFn: tapsApi.clearToken,
    onSuccess: () => queryClient.setQueryData(["tap-token"], { token: null }),
  });

  function replaceToken() {
    if (window.confirm("Replace the token? Phones using the current one will stop sending taps until you update them.")) {
      regenerateMutation.mutate();
    }
  }

  function turnOff() {
    if (window.confirm("Turn off card taps? Phones will stop sending taps until you create a new token.")) {
      clearMutation.mutate();
    }
  }

  if (isLoading) return <div className="text-sm text-muted-foreground">Loading…</div>;

  const error = regenerateMutation.error?.message ?? clearMutation.error?.message;

  return (
    <div className="space-y-4">
      <p className="text-sm text-muted-foreground">
        Record card and phone payments the moment you make them, using an iPhone shortcut. They're matched up
        automatically when you import the statement.
      </p>

      <Card>
        <CardContent className="pt-4 space-y-4">
          {token ? (
            <>
              <CopyField label="App address" value={window.location.origin} />
              <CopyField label="Tap token" value={token} hidden={!showToken} />
              <div className="flex flex-wrap gap-2">
                <Button variant="outline" size="sm" onClick={() => setShowToken((s) => !s)}>
                  {showToken ? "Hide token" : "Show token"}
                </Button>
                <Button variant="outline" size="sm" onClick={replaceToken} disabled={regenerateMutation.isPending}>
                  Replace token
                </Button>
                <Button variant="outline" size="sm" onClick={turnOff} disabled={clearMutation.isPending}>
                  Turn off
                </Button>
              </div>
            </>
          ) : (
            <div className="space-y-2">
              <p className="text-sm">Card taps are off. Create a token to connect your phone.</p>
              <Button onClick={() => regenerateMutation.mutate()} disabled={regenerateMutation.isPending}>
                Create token
              </Button>
            </div>
          )}
          {error && <p className="text-xs text-destructive">{error}</p>}
        </CardContent>
      </Card>

      {token && (
        <Card>
          <CardContent className="pt-4 space-y-3">
            <p className="text-sm font-medium">Set up an iPhone</p>
            <ol className="list-decimal pl-5 space-y-2 text-sm text-muted-foreground">
              <li>
                {TAP_SHORTCUT_URL ? (
                  <>
                    <a href={TAP_SHORTCUT_URL} className="text-primary underline" target="_blank" rel="noreferrer">
                      Install the PennyChest Tap shortcut
                    </a>
                    , tap Add Shortcut, and paste the app address and tap token above when it asks.
                  </>
                ) : (
                  <>Install the PennyChest Tap shortcut and paste the app address and tap token above when it asks.</>
                )}
              </li>
              <li>
                In the Shortcuts app, open <span className="text-foreground">Automation</span>, tap +, and choose{" "}
                <span className="text-foreground">Wallet</span>.
              </li>
              <li>Select the cards to track, choose Run Immediately, and pick the PennyChest Tap shortcut.</li>
            </ol>
          </CardContent>
        </Card>
      )}
    </div>
  );
}

function PasswordSection() {
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [confirm, setConfirm] = useState("");
  const [formError, setFormError] = useState<string | null>(null);

  const mutation = useMutation({
    mutationFn: () => authApi.changePassword(current, next),
    onSuccess: () => {
      setCurrent("");
      setNext("");
      setConfirm("");
    },
  });

  function submit(e: React.FormEvent) {
    e.preventDefault();
    setFormError(null);
    mutation.reset();
    if (next !== confirm) {
      setFormError("The new passwords don't match.");
      return;
    }
    mutation.mutate();
  }

  const error = formError ?? mutation.error?.message;

  return (
    <Card>
      <CardContent className="pt-4">
        <form onSubmit={submit} className="space-y-3">
          <p className="text-xs text-muted-foreground">
            Changing your password signs out every other device.
          </p>
          <input type="text" name="username" autoComplete="username" value="pennychest" readOnly hidden />
          <Input
            type="password"
            placeholder="Current password"
            autoComplete="current-password"
            value={current}
            onChange={(e) => setCurrent(e.target.value)}
          />
          <Input
            type="password"
            placeholder="New password (at least 8 characters)"
            autoComplete="new-password"
            value={next}
            onChange={(e) => setNext(e.target.value)}
          />
          <Input
            type="password"
            placeholder="Confirm new password"
            autoComplete="new-password"
            value={confirm}
            onChange={(e) => setConfirm(e.target.value)}
          />
          {error && <p className="text-xs text-destructive">{error}</p>}
          {mutation.isSuccess && (
            <p className="text-xs text-muted-foreground flex items-center gap-1">
              <Check className="h-3 w-3" /> Password changed
            </p>
          )}
          <Button type="submit" disabled={!current || !next || mutation.isPending}>
            Change password
          </Button>
        </form>
      </CardContent>
    </Card>
  );
}
