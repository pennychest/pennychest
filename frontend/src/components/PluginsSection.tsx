import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Loader2, ShieldAlert, Trash2 } from "lucide-react";
import { pluginsApi, type PluginRepository, type RepositoryPlugin } from "../api/client";
import { Badge } from "./ui/badge";
import { Button } from "./ui/button";
import { Card, CardContent } from "./ui/card";
import { Input } from "./ui/input";

const PLUGINS_KEY = ["plugins"];

export function PluginsSection() {
  const queryClient = useQueryClient();
  const [newUrl, setNewUrl] = useState("");

  const { data, isLoading, error } = useQuery({ queryKey: PLUGINS_KEY, queryFn: pluginsApi.list });

  const refresh = () => {
    queryClient.invalidateQueries({ queryKey: PLUGINS_KEY });
    // Importers and exporters come from plugins
    queryClient.invalidateQueries({ queryKey: ["export-info"] });
  };

  const install = useMutation({
    mutationFn: ({ repository, pkg }: { repository: string; pkg: string }) =>
      pluginsApi.install(repository, pkg),
    onSuccess: refresh,
  });
  const uninstall = useMutation({
    mutationFn: (pkg: string) => pluginsApi.uninstall(pkg),
    onSuccess: refresh,
  });
  const addRepository = useMutation({
    mutationFn: (url: string) => pluginsApi.addRepository(url),
    onSuccess: () => {
      setNewUrl("");
      refresh();
    },
  });
  const removeRepository = useMutation({
    mutationFn: (url: string) => pluginsApi.removeRepository(url),
    onSuccess: refresh,
  });

  if (isLoading) return <p className="text-sm text-muted-foreground">Loading…</p>;
  if (error || !data) return <p className="text-sm text-destructive">Couldn't load plugins.</p>;

  const busy = install.isPending || uninstall.isPending;
  const actionError = install.error?.message ?? uninstall.error?.message ?? removeRepository.error?.message;

  function remove(pkg: string, name: string) {
    if (window.confirm(`Remove ${name}?`)) uninstall.mutate(pkg);
  }

  function removeRepo(repo: PluginRepository) {
    if (window.confirm(`Remove ${repo.name}? Plugins installed from it stay installed.`)) {
      removeRepository.mutate(repo.url);
    }
  }

  function action(repo: PluginRepository, plugin: RepositoryPlugin) {
    if (plugin.built_in) return <Badge variant="secondary">Built in</Badge>;
    const installing = install.isPending && install.variables?.pkg === plugin.package;
    const removing = uninstall.isPending && uninstall.variables === plugin.package;
    if (installing || removing) {
      return (
        <span className="flex items-center gap-1 text-xs text-muted-foreground">
          <Loader2 className="h-3 w-3 animate-spin" />
          {installing ? "Installing… this can take a minute" : "Removing…"}
        </span>
      );
    }
    const doInstall = () => install.mutate({ repository: repo.url, pkg: plugin.package });
    if (!plugin.installed_version) {
      return (
        <Button size="sm" onClick={doInstall} disabled={busy}>
          Install
        </Button>
      );
    }
    return (
      <div className="flex gap-2">
        {plugin.installed_version !== plugin.version && (
          <Button size="sm" onClick={doInstall} disabled={busy}>
            Update to {plugin.version}
          </Button>
        )}
        <Button size="sm" variant="outline" onClick={() => remove(plugin.package, plugin.name)} disabled={busy}>
          Remove
        </Button>
      </div>
    );
  }

  return (
    <div className="space-y-4">
      <p className="text-sm text-muted-foreground">
        Plugins add importers, like HSBC PDF statements, and exporters, like Beancount.
      </p>

      {actionError && <p className="text-sm text-destructive whitespace-pre-line">{actionError}</p>}

      {data.repositories.map((repo) => (
        <Card key={repo.url}>
          <CardContent className="pt-4 space-y-4">
            <div className="flex items-start gap-2">
              <div className="min-w-0">
                <p className="font-medium">{repo.name}</p>
                <a href={repo.url} target="_blank" rel="noreferrer" className="text-xs text-muted-foreground underline break-all">
                  {repo.url}
                </a>
              </div>
              {repo.official ? (
                <Badge variant="secondary" className="ml-auto shrink-0">Official</Badge>
              ) : (
                <Button
                  variant="ghost"
                  size="icon"
                  className="ml-auto shrink-0"
                  aria-label={`Remove ${repo.name}`}
                  onClick={() => removeRepo(repo)}
                >
                  <Trash2 className="h-4 w-4" />
                </Button>
              )}
            </div>

            {repo.error && <p className="text-sm text-destructive">{repo.error}</p>}
            {!repo.error && repo.plugins.length === 0 && (
              <p className="text-sm text-muted-foreground">This repository doesn't list any plugins.</p>
            )}

            {repo.plugins.map((plugin) => (
              <div key={plugin.package} className="flex flex-wrap items-start gap-3 border-t pt-3">
                <div className="min-w-0 flex-1 space-y-1">
                  <p className="text-sm font-medium">
                    {plugin.name}{" "}
                    <span className="text-xs font-normal text-muted-foreground">
                      {plugin.installed_version ? `v${plugin.installed_version} installed` : `v${plugin.version}`}
                    </span>
                  </p>
                  {plugin.description && <p className="text-sm text-muted-foreground">{plugin.description}</p>}
                </div>
                {action(repo, plugin)}
              </div>
            ))}
          </CardContent>
        </Card>
      ))}

      {data.other_installed.length > 0 && (
        <Card>
          <CardContent className="pt-4 space-y-4">
            <p className="font-medium">Installed from elsewhere</p>
            {data.other_installed.map((plugin) => (
              <div key={plugin.package} className="flex items-center gap-3 border-t pt-3">
                <p className="text-sm flex-1">
                  {plugin.package} <span className="text-xs text-muted-foreground">v{plugin.version}</span>
                </p>
                <Button size="sm" variant="outline" onClick={() => remove(plugin.package, plugin.package)} disabled={busy}>
                  Remove
                </Button>
              </div>
            ))}
          </CardContent>
        </Card>
      )}

      {data.can_add_repositories && (
        <Card>
          <CardContent className="pt-4 space-y-3">
            <p className="font-medium">Add a repository</p>
            <div className="flex items-start gap-2 rounded-md border border-amber-300 bg-amber-50/60 p-3 text-sm dark:bg-amber-950/30">
              <ShieldAlert className="h-4 w-4 mt-0.5 shrink-0 text-amber-600" />
              <p>Plugins can run any code on your server and read all your data. Only add repositories you trust.</p>
            </div>
            <form
              className="flex gap-2"
              onSubmit={(e) => {
                e.preventDefault();
                addRepository.mutate(newUrl);
              }}
            >
              <Input
                placeholder="https://github.com/you/pennychest-plugins"
                value={newUrl}
                onChange={(e) => setNewUrl(e.target.value)}
              />
              <Button type="submit" disabled={!newUrl.trim() || addRepository.isPending}>
                Add
              </Button>
            </form>
            {addRepository.error && <p className="text-xs text-destructive">{addRepository.error.message}</p>}
          </CardContent>
        </Card>
      )}
    </div>
  );
}
