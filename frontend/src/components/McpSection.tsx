import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Check, Copy, KeyRound, Trash2 } from "lucide-react";
import { mcpApi, type McpToken, type TokenScope } from "../api/client";
import { Badge } from "./ui/badge";
import { Button } from "./ui/button";
import { Card, CardContent } from "./ui/card";
import { Input } from "./ui/input";

const TOKENS_KEY = ["mcp-tokens"];

const PERMISSIONS: { scope: TokenScope; label: string; help: string }[] = [
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

function serverUrl() {
  return `${window.location.origin}/mcp`;
}

function claudeCodeCommand(token: string) {
  return `claude mcp add --transport http pennychest ${serverUrl()} --header "Authorization: Bearer ${token}"`;
}

function claudeDesktopConfig(token: string) {
  return JSON.stringify(
    {
      mcpServers: {
        pennychest: {
          command: "npx",
          args: ["mcp-remote", serverUrl(), "--header", "Authorization:${PENNYCHEST_AUTH}"],
          env: { PENNYCHEST_AUTH: `Bearer ${token}` },
        },
      },
    },
    null,
    2,
  );
}

function when(iso: string | null) {
  if (!iso) return "never";
  return new Date(iso).toLocaleString("en-GB", { day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" });
}

function permissionSummary(scopes: TokenScope[]) {
  if (scopes.length === 0) return "Read only";
  const parts = scopes.map((s) => (s === "organise" ? "categories, rules, budgets" : "transactions"));
  return `Read, and change ${parts.join(" and ")}`;
}

function CopyBlock({ label, value }: { label: string; value: string }) {
  const [copied, setCopied] = useState(false);
  const [failed, setFailed] = useState(false);

  async function copy() {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(true);
      setFailed(false);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      setFailed(true);
    }
  }

  return (
    <div className="space-y-1">
      <div className="flex items-center justify-between gap-2">
        <p className="text-sm font-medium">{label}</p>
        <Button type="button" variant="outline" size="sm" onClick={copy}>
          {copied ? <Check className="h-3.5 w-3.5" /> : <Copy className="h-3.5 w-3.5" />}
          <span className="ml-1">{copied ? "Copied" : "Copy"}</span>
        </Button>
      </div>
      <pre className="bg-muted/50 border rounded p-2 text-[11px] font-mono whitespace-pre-wrap break-all select-all">
        {value}
      </pre>
      {failed && (
        <p className="text-xs text-muted-foreground">Couldn't copy automatically. Select the text and copy it instead.</p>
      )}
    </div>
  );
}

function NewTokenForm() {
  const queryClient = useQueryClient();
  const [name, setName] = useState("");
  const [scopes, setScopes] = useState<TokenScope[]>([]);
  const create = useMutation({
    mutationFn: () => mcpApi.createToken(name.trim(), scopes),
    onSuccess: () => {
      setName("");
      setScopes([]);
      queryClient.invalidateQueries({ queryKey: TOKENS_KEY });
    },
  });

  if (create.data) {
    const token = create.data.token;
    return (
      <Card className="border-green-300">
        <CardContent className="pt-4 space-y-4">
          <div>
            <p className="font-medium">Token created: {create.data.name}</p>
            <p className="text-sm text-muted-foreground">
              Copy it now. It isn't stored and won't be shown again.
            </p>
          </div>
          <CopyBlock label="Access token" value={token} />
          <CopyBlock label="Claude Code: run this in a terminal" value={claudeCodeCommand(token)} />
          <CopyBlock
            label="Claude Desktop: add to claude_desktop_config.json (needs Node.js)"
            value={claudeDesktopConfig(token)}
          />
          <p className="text-xs text-muted-foreground">
            Other MCP clients: use the server address {serverUrl()} (Streamable HTTP) and send the token as{" "}
            <code>Authorization: Bearer …</code>.
          </p>
          <Button variant="outline" onClick={() => create.reset()}>
            Done
          </Button>
        </CardContent>
      </Card>
    );
  }

  return (
    <Card>
      <CardContent className="pt-4 space-y-3">
        <p className="font-medium">New access token</p>
        <Input
          placeholder="Name, e.g. Claude Code on my laptop"
          value={name}
          onChange={(e) => setName(e.target.value)}
          maxLength={60}
          aria-label="Token name"
        />
        <fieldset className="space-y-2">
          <legend className="text-xs text-muted-foreground mb-1">
            It can always look up your data. Also allow it to:
          </legend>
          {PERMISSIONS.map(({ scope, label, help }) => (
            <label key={scope} className="flex items-start gap-2 text-sm cursor-pointer">
              <input
                type="checkbox"
                className="mt-0.5 h-4 w-4 accent-primary"
                checked={scopes.includes(scope)}
                onChange={(e) =>
                  setScopes((s) => (e.target.checked ? [...s, scope] : s.filter((x) => x !== scope)))
                }
              />
              <span>
                {label}
                <span className="block text-xs text-muted-foreground">{help}</span>
              </span>
            </label>
          ))}
        </fieldset>
        {create.error && <p className="text-xs text-destructive">{create.error.message}</p>}
        <Button onClick={() => create.mutate()} disabled={!name.trim() || create.isPending}>
          <KeyRound className="h-4 w-4 mr-1" />
          Create token
        </Button>
      </CardContent>
    </Card>
  );
}

function TokenRow({ token }: { token: McpToken }) {
  const queryClient = useQueryClient();
  const [confirming, setConfirming] = useState(false);
  const revoke = useMutation({
    mutationFn: () => mcpApi.revokeToken(token.id),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: TOKENS_KEY }),
  });

  return (
    <div className="flex items-start gap-3 px-4 py-3">
      <div className="flex-1 min-w-0">
        <p className="text-sm font-medium truncate">{token.name}</p>
        <p className="text-xs text-muted-foreground">
          <span className="font-mono">{token.prefix}…</span> · last used {when(token.last_used_at)}
        </p>
        <div className="flex gap-1 flex-wrap mt-1">
          {token.kind === "connector" && (
            <Badge variant="secondary" className="text-[10px]">
              Connected app
            </Badge>
          )}
          <Badge variant="outline" className="text-[10px]">
            {permissionSummary(token.scopes)}
          </Badge>
        </div>
      </div>
      {confirming ? (
        <div className="flex gap-1 shrink-0">
          <Button size="sm" variant="destructive" onClick={() => revoke.mutate()} disabled={revoke.isPending}>
            Revoke
          </Button>
          <Button size="sm" variant="ghost" onClick={() => setConfirming(false)}>
            Cancel
          </Button>
        </div>
      ) : (
        <Button size="icon" variant="ghost" onClick={() => setConfirming(true)} aria-label={`Revoke ${token.name}`}>
          <Trash2 className="h-4 w-4" />
        </Button>
      )}
    </div>
  );
}

export function McpSection() {
  const { data, isLoading } = useQuery({ queryKey: TOKENS_KEY, queryFn: mcpApi.listTokens });
  const tokens = data?.tokens ?? [];

  return (
    <div className="space-y-4">
      <p className="text-sm text-muted-foreground">
        Let AI agents such as Claude Code or Claude Desktop use PennyChest through MCP. Each agent gets
        its own token, limited to what you allow. Changes they make appear in Settings → Changes made by
        AI, where you can undo them.
      </p>
      <Card>
        <CardContent className="pt-4 space-y-2">
          <p className="font-medium">claude.ai, ChatGPT and other web apps</p>
          <p className="text-sm text-muted-foreground">
            Add a custom connector with this address. The app will send you here to sign in and choose
            what it may change; no token needed. It then appears in the list below.
          </p>
          <CopyBlock label="Connector address" value={serverUrl()} />
        </CardContent>
      </Card>
      <NewTokenForm />
      <div className="space-y-2">
        <h2 className="text-lg font-semibold">Tokens</h2>
        {isLoading ? (
          <p className="text-sm text-muted-foreground">Loading…</p>
        ) : tokens.length === 0 ? (
          <p className="text-sm text-muted-foreground">No tokens yet.</p>
        ) : (
          <Card>
            <CardContent className="divide-y p-0">
              {tokens.map((t) => (
                <TokenRow key={t.id} token={t} />
              ))}
            </CardContent>
          </Card>
        )}
      </div>
    </div>
  );
}
