# Quick start

To run it on your own computer, see [Installation](installation.md).

## In the cloud, in one click

[![Deploy to Render](https://render.com/images/deploy-to-render-button.svg)](https://render.com/deploy?repo=https://github.com/pennychest/pennychest)

Sign up to [Render](https://render.com), click **Deploy Blueprint**, then open the address Render gives you. It needs a paid plan for the disk your data is kept on; see [Render's pricing](https://render.com/pricing).

## In the cloud, from the command line

[Fly.io](https://fly.io) is usually cheaper, as it stops PennyChest when nobody's using it. With [flyctl](https://fly.io/docs/flyctl/install/) installed, create `fly.toml`, changing `app` to a name of your own:

```toml
app = "my-pennychest"

[build]
  image = "ghcr.io/pennychest/pennychest:latest"

[mounts]
  source = "pennychest_data"
  destination = "/data"

[http_service]
  internal_port = 8000
  force_https = true
  auto_stop_machines = "stop"
  auto_start_machines = true
  min_machines_running = 0

[[vm]]
  memory = "512mb"
```

Then:

```bash
fly auth login
fly apps create my-pennychest
fly deploy --ha=false
```

and open `https://my-pennychest.fly.dev`.

## Set a password straight away

!!! danger
    Until a password is set, anyone who reaches PennyChest can choose one. See [Signing in](signing-in.md).

## Next steps

- **Set up a decision model.** We strongly recommend a decision model such as Jev: it categorises each import quickly and cheaply, and only when it's sure. See [AI integration](ai-integration.md#models).
- **Set up your spending categories.** Choose a language model and ask the chat, or connect your own AI assistant through [MCP](mcp.md), to look at your transactions and set up categories that suit you. Let it change categories under **What chat may change**, or when you connect the assistant.

## Anywhere else

Anything that runs Docker can host PennyChest: keep `/data` on a persistent volume, or point it at [PostgreSQL](installation.md#postgresql).
