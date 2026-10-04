# AI integration

AI in PennyChest is optional, and switched off until you choose a provider. Without one, PennyChest still categorises with your rules and with [the model it learns from your own categories](categorisation.md#learning-from-your-categories), which runs inside PennyChest and sends nothing anywhere.

With a provider, AI can:

- categorise what your rules and history don't cover
- match card taps, duplicates and transfers
- flag subscriptions and unusual charges
- suggest rules
- answer questions about your money in a chat

To let your own AI assistant work with your ledger, see [AI agents (MCP)](mcp.md).

## Providers

Add your credentials under **Settings → AI Integration → Providers**.

| Provider | What you need | Good for |
|---|---|---|
| **TypeSafe AI (Jev)** | an API key from [TypeSafe AI](https://typesafe.ai) | Categorising, matching, insights and checking chat actions: every task except rule suggestions and chat |
| **Anthropic (Claude)** | an API key from [console.anthropic.com](https://console.anthropic.com) | Every task |
| **OpenAI** | an API key; the base URL can point at any OpenAI-compatible API | Every task |
| **Google (Gemini)** | a Gemini API key | Every task |
| **Google Vertex AI** | an express mode API key | Every task |
| **Ollama (self-hosted)** | the URL of your Ollama server | Every task, with nothing leaving your network |

Jev is a decision model. It picks from the options it's given and says how confident it is, so PennyChest can leave an answer to you rather than act on a guess. The others are general-purpose LLMs that don't report a confidence, so every answer they give is used. A sensible setup is Jev for the automatic tasks and an LLM for rules and chat.

!!! note
    Provider APIs are billed by the provider, per token or per request. The Anthropic API is separate from a claude.ai subscription.

### Ollama

PennyChest doesn't run Ollama for you. Run it next to PennyChest, pull a model, then enter the server's URL as PennyChest sees it. For example, use `http://ollama:11434` for a Compose service called `ollama`, or `http://host.docker.internal:11434` for Ollama on the Docker host:

```bash
docker run -d --name ollama -p 11434:11434 -v ollama:/root/.ollama ollama/ollama
docker exec ollama ollama pull qwen3:4b
```

Small models are fine for categorising. Chat works better with a larger model that handles tool calling well.

## Tasks

Each task has its own provider and model, chosen under **Settings → AI Integration**, so you can use a cheap, fast model for the automatic tasks and a stronger one for chat. Use **Refresh the model list** after adding a provider. Any task without a provider is simply off.

### Automatic

These run by themselves whenever new data arrives.

| Task | What it does |
|---|---|
| **Categorising card taps** | Suggests a category for each card tap as it arrives. Your rules and your history come first. |
| **Categorising transactions** | Picks a category for imported transactions that no rule, card tap or learned category covered. Untick **Run straight after each import** to run it only from an import's review page instead. |
| **Checking chat actions** | Before chat makes a change, checks that it's what you asked for. If it isn't, the change is held back and chat asks you first. |
| **Matching taps, duplicates and transfers** | When a card tap could be one of several statement lines, picks the right one. On import, it also links transfers between your accounts and flags transactions you already have. It acts only when it's at least 80% sure. |
| **Flagging subscriptions and unusual charges** | After each import, marks which regular payments are subscriptions or bills, and flags charges that are unusual for the merchant or category. Chat uses both. |

An AI failure never breaks an import or a card tap. Whatever didn't happen is left for you, and the error is recorded in the [AI request logs](#ai-request-logs).

### On demand

These run only when you press a button or ask.

| Task | What it does |
|---|---|
| **Suggesting rules** | Proposes categorisation rules from your transactions, for you to accept or not. It needs a general-purpose model. |
| **Chat** | Answers questions about your finances and, if you allow it, makes changes for you. |

## Chat

The **Chat** page lets you ask about your money in plain language. For example, you can ask "how much did I spend eating out last month?" or "what are my subscriptions?".

The model doesn't do sums itself. It runs the same actions as the [MCP server](mcp.md), which query the database, and answers from their results. Each answer lists the actions it used to reach it.

By default chat can only look things up. Under **What chat may change** in **Settings → AI Integration**, you can let it:

- **Change categories, rules and budgets:** create and rename categories, add rules and set budgets
- **Change transactions:** add, edit, recategorise, review and delete transactions

Chat makes changes without stopping to ask first. If you've chosen a model for **Checking chat actions**, a second model reads the recent conversation before each change and judges whether you asked for it. If it judges you probably didn't, the change is held back, and chat tells you what it would do and asks you to confirm.

The check is a safety net, not a guarantee. If no model is chosen for it, or the check model can't be reached, the change goes ahead. Every change can be undone.

## Undoing AI changes

Every change made by the AI is listed under **Settings → Changes made by AI**, and each one can be undone. This includes:

- automatic categorisation from an import, including what the learned model did
- changes made in chat
- changes made by agents through MCP

## AI request logs

**Settings → Debug → AI request logs** shows every request sent to an AI provider, with what was sent, what came back, how long it took and any error.

## What's sent to the provider

Only what each task needs:

| Task | What's sent |
|---|---|
| Categorising | Transaction descriptions, or a card tap's merchant and amount, and the names of your accounts and categories |
| Matching | The pairs of records being compared |
| Insights | Merchants' recent charges, and the charges being scored |
| Chat | Your messages and the results of the actions chat runs, which can include any of your data that it looks up |

Statement files and the bank identifiers stored with your accounts (sort codes, account and card numbers) aren't sent. Account names are, so leave numbers out of them if that matters to you. If you'd rather nothing left your network, use Ollama, or rely on rules and the learned model alone.
