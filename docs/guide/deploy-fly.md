# Deploying to Fly.io

[Fly.io](https://fly.io) runs the PennyChest image on a small machine with a persistent volume for the database. You get an HTTPS address, which [card taps](card-taps.md) and [web AI connectors](mcp.md#claudeai-chatgpt-and-other-web-apps) need. With the settings below, the machine stops when nobody's using it and starts again on the next request.

## 1. Install flyctl and sign in

Follow [Fly's install guide](https://fly.io/docs/flyctl/install/), then:

```bash
fly auth login
```

## 2. Write `fly.toml`

In an empty directory, create `fly.toml`. Change `app` to a name of your own: it becomes `https://<app>.fly.dev`. Change `primary_region` to [one near you](https://fly.io/docs/reference/regions/).

```toml
app = "my-pennychest"
primary_region = "lhr"

[build]
  image = "ghcr.io/pennychest/pennychest:latest"

[mounts]
  source = "pennychest_data"
  destination = "/data"
  initial_size = "1gb"

[http_service]
  internal_port = 8000
  force_https = true
  auto_stop_machines = "stop"
  auto_start_machines = true
  min_machines_running = 0

  [[http_service.checks]]
    interval = "30s"
    timeout = "5s"
    grace_period = "20s"
    method = "GET"
    path = "/api/health"

[[vm]]
  memory = "512mb"
  cpu_kind = "shared"
  cpus = 1
```

## 3. Create the app and deploy

```bash
fly apps create my-pennychest
fly deploy --ha=false
```

`--ha=false` keeps it to a single machine. SQLite lives on one volume, so a second machine would have its own, separate, empty database. Fly creates the volume on the first deploy.

## 4. Set a password straight away

Open `https://my-pennychest.fly.dev` and choose a password. Until you do, anyone who finds the address can choose one; see [Signing in](signing-in.md).

## Updating

`fly deploy` fetches the image again, so running it picks up the latest release. Migrations run on startup. To stay on a release series instead, use a version tag such as `ghcr.io/pennychest/pennychest:0.1`; see [the image tags](installation.md#the-image).

## Plugins

The published image has no [plugins](plugins.md). To include some, deploy from a checkout of the [pennychest repository](https://github.com/pennychest/pennychest) instead. Put `fly.toml` in the checkout, and replace its `[build]` section with:

```toml
[build]
  dockerfile = "Dockerfile"

  [build.args]
    PENNYCHEST_PLUGINS = "git+https://github.com/pennychest/pennychest-plugins@hsbc-v0.1.0#subdirectory=hsbc"
```

Fly then builds the image itself on each `fly deploy`.

## Good to know

- **Cost:** a 512 MB shared machine that stops when idle, plus a 1 GB volume, costs very little; see [Fly's pricing](https://fly.io/docs/about/pricing/).
- **Cold starts:** after it has stopped, the first request takes a few seconds while the machine starts. If card taps or connectors time out, set `min_machines_running = 1` to keep it running, which costs more.
- **Backups:** Fly snapshots volumes daily; see `fly volumes snapshots list`. You can also download the database under **Settings → Export**.
- **Your own domain:** `fly certs add finance.example.com`, then point the domain's DNS at the app as Fly tells you.
- **PostgreSQL:** set `PENNYCHEST_DATABASE_URL` with `fly secrets set` to use a Postgres database instead of SQLite; see [Installation](installation.md#postgresql).
