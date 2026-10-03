import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { authApi, SIGNED_OUT_EVENT } from "../api/client";
import { AUTH_QUERY_KEY, MIN_PASSWORD_LENGTH, setSignedIn } from "../lib/auth";
import { Button } from "./ui/button";
import { Card, CardContent } from "./ui/card";
import { Input } from "./ui/input";

export function AuthGate({ children }: { children: React.ReactNode }) {
  const queryClient = useQueryClient();
  const { data: status, isLoading, isError, refetch } = useQuery({
    queryKey: AUTH_QUERY_KEY,
    queryFn: authApi.status,
    staleTime: Infinity,
  });

  useEffect(() => {
    const onSignedOut = () => queryClient.invalidateQueries({ queryKey: AUTH_QUERY_KEY });
    window.addEventListener(SIGNED_OUT_EVENT, onSignedOut);
    return () => window.removeEventListener(SIGNED_OUT_EVENT, onSignedOut);
  }, [queryClient]);

  if (isLoading) {
    return <div className="text-muted-foreground text-sm p-4">Loading…</div>;
  }

  if (isError || !status) {
    return (
      <CenteredCard>
        <p className="text-sm text-muted-foreground">Couldn't reach PennyChest.</p>
        <Button onClick={() => refetch()} className="w-full">
          Try again
        </Button>
      </CenteredCard>
    );
  }

  if (status.setup_required) return <SignInScreen mode="setup" />;
  if (!status.authenticated) return <SignInScreen mode="login" />;
  return <>{children}</>;
}

function SignInScreen({ mode }: { mode: "setup" | "login" }) {
  const queryClient = useQueryClient();
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [formError, setFormError] = useState<string | null>(null);

  const mutation = useMutation({
    mutationFn: () => (mode === "setup" ? authApi.setup(password) : authApi.login(password)),
    onSuccess: () => setSignedIn(queryClient, true),
  });

  function submit(e: React.FormEvent) {
    e.preventDefault();
    setFormError(null);
    if (mode === "setup") {
      if (password.length < MIN_PASSWORD_LENGTH) {
        setFormError(`Use at least ${MIN_PASSWORD_LENGTH} characters.`);
        return;
      }
      if (password !== confirm) {
        setFormError("The passwords don't match.");
        return;
      }
    }
    mutation.mutate();
  }

  const error = formError ?? mutation.error?.message;

  return (
    <CenteredCard>
      <div className="space-y-1">
        <h1 className="text-xl font-semibold">
          {mode === "setup" ? "Create a password" : "Sign in"}
        </h1>
        <p className="text-sm text-muted-foreground">
          {mode === "setup"
            ? "Choose the password you'll use to open PennyChest on any device."
            : "Enter your PennyChest password."}
        </p>
      </div>
      <form onSubmit={submit} className="space-y-3">
        {/* Lets password managers save and fill the password against a fixed name */}
        <input
          type="text"
          name="username"
          autoComplete="username"
          value="pennychest"
          readOnly
          hidden
        />
        <Input
          type="password"
          placeholder="Password"
          autoComplete={mode === "setup" ? "new-password" : "current-password"}
          autoFocus
          value={password}
          onChange={(e) => setPassword(e.target.value)}
        />
        {mode === "setup" && (
          <Input
            type="password"
            placeholder="Confirm password"
            autoComplete="new-password"
            value={confirm}
            onChange={(e) => setConfirm(e.target.value)}
          />
        )}
        {error && <p className="text-sm text-destructive">{error}</p>}
        <Button type="submit" className="w-full" disabled={!password || mutation.isPending}>
          {mutation.isPending ? "…" : mode === "setup" ? "Create password" : "Sign in"}
        </Button>
      </form>
    </CenteredCard>
  );
}

function CenteredCard({ children }: { children: React.ReactNode }) {
  return (
    <div className="min-h-screen bg-background flex items-center justify-center px-4">
      <div className="w-full max-w-sm space-y-6">
        <div className="flex items-center justify-center gap-2 font-semibold text-2xl">
          <img src="/logo.png" alt="" className="h-10 w-10" />
          <span>
            <span style={{ color: "#D4A017" }}>Penny</span>
            <span style={{ color: "#2A6B2A" }}>Chest</span>
          </span>
        </div>
        <Card>
          <CardContent className="pt-6 space-y-4">{children}</CardContent>
        </Card>
      </div>
    </div>
  );
}
