# Plugins

Plugins add importers, for more banks' statements, and exporters, to other formats. PennyChest includes the CSV importer; the official plugins, such as the HSBC PDF statement importer and the Beancount exporter, are in [pennychest-plugins](https://github.com/pennychest/pennychest-plugins).

## Installing a plugin

Go to **Settings → Plugins** and press **Install**. The plugin works straight away, without a restart. **Update** appears when there's a newer version, and **Remove** uninstalls it.

Plugins are installed into `/data/plugins`, next to the database, so they survive restarts and updates. If that folder is lost, PennyChest reinstalls them on startup.

## Other repositories

Settings lists the plugins in the official repository. To install plugins from somewhere else, such as your own fork, go to **Settings → Plugins → Add a repository** and paste its link.

!!! warning
    A plugin can run any code on your server and read all your data. Only add repositories you trust.

To offer only the official repository, set `PENNYCHEST_ALLOW_PLUGIN_REPOSITORIES=false`.

## Making your own

1. Fork [pennychest-plugins](https://github.com/pennychest/pennychest-plugins).
2. Add a directory for your plugin, following `hsbc/` (an importer) or `beancount/` (an exporter).
3. List it in the fork's `plugins.json`, and tag a release.
4. Add your fork under **Settings → Plugins → Add a repository**, and install your plugin.

The [pennychest-plugins README](https://github.com/pennychest/pennychest-plugins#making-your-own) has the details. To share a plugin with everyone, open a pull request there.

## Building plugins into the image

To bake plugins into your own image instead, pass them as pip requirements when building from a checkout of this repository, pinned to a tag:

```bash
docker build \
  --build-arg PENNYCHEST_PLUGINS="git+https://github.com/pennychest/pennychest-plugins@hsbc-v0.1.0#subdirectory=hsbc" \
  -t pennychest .
```

Separate several plugins with spaces. Plugins built in show as *Built in* in Settings and can't be removed there.

## Developing a plugin

Clone pennychest-plugins next to this repository. The dev Compose setup mounts it at `/plugins`, so you can run a plugin from your working copy:

```bash
docker compose -f docker-compose.dev.yml exec api pip install -e /plugins/<name>
```

Then restart `api`.
