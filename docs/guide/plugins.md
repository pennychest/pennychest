# Plugins

Importers and exporters are plugins: Python packages that register themselves as entry points (`pennychest.importers`, `pennychest.exporters`). The core includes the CSV importer. Optional ones, such as the HSBC PDF statement importer and the Beancount exporter, live in [pennychest-plugins](https://github.com/pennychest/pennychest-plugins).

## Building them into the image

Pass plugins as pip requirements when building, pinned to a tag or commit:

```bash
docker build \
  --build-arg PENNYCHEST_PLUGINS="git+https://github.com/pennychest/pennychest-plugins@hsbc-v0.1.0#subdirectory=hsbc" \
  -t pennychest .
```

Separate several plugins with spaces.

## Outside Docker

`pip install` the same requirement into PennyChest's environment and restart it.

## Developing a plugin

Clone pennychest-plugins next to this repository. The dev Compose setup mounts it at `/plugins`, so you can run a plugin from your working copy:

```bash
docker compose -f docker-compose.dev.yml exec api pip install -e /plugins/<name>
```

Then restart `api`.
