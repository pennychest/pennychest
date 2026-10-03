import { useState } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { Plug } from "lucide-react";
import { oauthApi, type OAuthRequest, type TokenScope } from "../api/client";
import { Button } from "../components/ui/button";
import { Card, CardContent } from "../components/ui/card";

const PERMISSIONS: { scope: TokenScope; label: string; help: string }[] = [
  {
    scope: "organise",
    label: "Change categories, rules and budgets",
    help: "Create and rename categories, add rules and set budgets.",
  },
  {
    scope: "transactions",
    label: "Change transactions",
    help: "Add, edit, recategorise, review and delete transactions.",
  },
];

function readRequest(): OAuthRequest | null {
  const params = new URLSearchParams(window.location.search);
  const clientId = params.get("client_id");
  const redirectUri = params.get("redirect_uri");
  if (!clientId || !redirectUri) return null;
  return {
    client_id: clientId,
    redirect_uri: redirectUri,
    state: params.get("state"),
    code_challenge: params.get("code_challenge"),
    code_challenge_method: params.get("code_challenge_method"),
    response_type: params.get("response_type") ?? "code",
  };
}

// Where a connector (claude.ai, ChatGPT…) sends the user to approve its access.
export function OAuthConsentPage() {
  const [req] = useState(readRequest);
  const [scopes, setScopes] = useState<TokenScope[]>([]);
  const info = useQuery({
    queryKey: ["oauth-info", req?.client_id, req?.redirect_uri],
    queryFn: () => oauthApi.info(req!.client_id, req!.redirect_uri),
    enabled: req !== null,
    retry: false,
  });
  const decide = useMutation({
    mutationFn: (allow: boolean) => (allow ? oauthApi.approve(req!, scopes) : oauthApi.deny(req!)),
    onSuccess: ({ redirect }) => window.location.assign(redirect),
  });

  if (!req || info.isError) {
    return (
      <Card className="max-w-md mx-auto mt-8">
        <CardContent className="pt-6 space-y-2">
          <p className="font-medium">This connection request isn't valid</p>
          <p className="text-sm text-muted-foreground">
            {info.error?.message ?? "It's missing details the app should have sent."} Try connecting again from
            the app.
          </p>
        </CardContent>
      </Card>
    );
  }
  if (info.isLoading || !info.data) {
    return <p className="text-sm text-muted-foreground">Loading…</p>;
  }

  return (
    <Card className="max-w-md mx-auto mt-4">
      <CardContent className="pt-6 space-y-5">
        <div className="flex items-start gap-3">
          <Plug className="h-6 w-6 mt-0.5 text-primary shrink-0" />
          <div>
            <h1 className="text-xl font-semibold">Connect {info.data.client_name} to PennyChest?</h1>
            <p className="text-sm text-muted-foreground mt-1">
              It will be able to look up your accounts, transactions, budgets and card taps. You'll be
              sent back to <span className="font-medium text-foreground">{info.data.redirect_host}</span>.
            </p>
          </div>
        </div>

        <fieldset className="space-y-2">
          <legend className="text-sm font-medium mb-1">Also allow it to:</legend>
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
          <p className="text-xs text-muted-foreground">
            Changes happen without asking first; each can be undone from Settings → Changes made by AI.
            You can disconnect it any time in Settings → AI agents (MCP).
          </p>
        </fieldset>

        {decide.error && <p className="text-sm text-destructive">{decide.error.message}</p>}
        <div className="flex gap-2">
          <Button className="flex-1" onClick={() => decide.mutate(true)} disabled={decide.isPending}>
            Allow
          </Button>
          <Button variant="outline" className="flex-1" onClick={() => decide.mutate(false)} disabled={decide.isPending}>
            Deny
          </Button>
        </div>
      </CardContent>
    </Card>
  );
}
