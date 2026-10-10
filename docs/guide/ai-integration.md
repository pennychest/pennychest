# AI integration

AI in PennyChest is optional, and does nothing until you choose a model. Without one, PennyChest still categorises with your rules and with [the model it learns from your own categories](categorisation.md#learning-from-your-categories), which runs inside PennyChest and sends nothing anywhere.

With a provider, AI can:

- categorise what your rules and history don't cover
- match card taps, duplicates and transfers
- flag subscriptions and unusual charges
- suggest rules
- answer questions about your money in a chat

To let your own AI assistant work with your ledger, see [AI agents (MCP)](mcp.md).

## Models

**Settings → AI Integration** has two kinds of model. Choose one of each, or just one; every task that uses that kind shares it.

- **Decision model.** Picks from the answers it's given and says how sure it is, so PennyChest can leave an answer to you rather than act on a guess. It's fast and cheap, which suits jobs that run on every import and card tap. It can't chat or write rules.
- **Language model.** A general-purpose LLM. It writes text, so it can do everything, including chat and suggesting rules, but it doesn't report a confidence, so every answer it gives is used. It's slower and costs more per request.

Add a provider's credentials under its section, then choose it and a model at the top. Use **Refresh the model list** after adding a provider.

| Provider | Kind | What you need |
|---|---|---|
| **TypeSafe AI (Jev)** | Decision model | an API key from [TypeSafe AI](https://typesafe.ai) |
| **OpenAI Decisions** | Decision model | an OpenAI API key; uses OpenAI's Decisions API |
| **Anthropic (Claude)** | Language model | an API key from [console.anthropic.com](https://console.anthropic.com) |
| **OpenAI** | Language model | an API key; the base URL can point at any OpenAI-compatible API |
| **Google (Gemini)** | Language model | a Gemini API key |
| **Google Vertex AI** | Language model | an express mode API key |
| **Ollama (self-hosted)** | Language model | the URL of your Ollama server, with nothing leaving your network |

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

Under **Tasks**, each task can be turned on or off. Tasks that can use either kind of model choose which one they use. If you've chosen a decision model, they use it unless you change them.

| Task | What it does | Can use |
|---|---|---|
| **Categorising transactions** | Picks a category for imported transactions that no rule, card tap or learned category covered. | Either |
| **Categorising card taps** | Suggests a category for each card tap. Your rules and your history come first. | Either |
| **Matching taps, duplicates and transfers** | Picks the right statement line for a card tap when several fit, links transfers between your accounts, and flags transactions you already have. It acts only when it's at least 80% sure. | Either |
| **Flagging subscriptions and unusual charges** | Marks which regular payments are subscriptions or bills, and flags charges that are unusual for the merchant or category. Chat uses both. | Either |
| **Suggesting rules** | Proposes categorisation rules from your transactions, for you to accept or not. | Language model |
| **Chat** | Answers questions about your finances and, if you allow it, makes changes for you. | Language model |
| **Checking chat actions** | Before chat makes a change, checks that it's what you asked for. | Either |

### Automatically or when you ask

The first four tasks can run **Automatically**, whenever new data arrives, or only **When I ask**:

| Task | When you ask, run it from |
|---|---|
| Categorising transactions | an import's review page, **Suggest with AI** |
| Categorising card taps | the **Card taps** page, **Suggest a category** on a tap |
| Matching | an import's review page, **Find matches with AI**, or **Reconcile** on the **Card taps** page |
| Subscriptions and unusual charges | an import's review page, **Check for subscriptions** |

Unless you choose, tasks run automatically with a decision model, and when you ask with a language model, as a language model on every import can add up. Suggesting rules only ever runs when you ask; chat and checking its actions run whenever you chat.

An AI failure never breaks an import or a card tap. Whatever didn't happen is left for you, and the error is recorded in the [AI request logs](#ai-request-logs).

## Chat

The **Chat** page lets you ask about your money in plain language. For example, you can ask "how much did I spend eating out last month?" or "what are my subscriptions?".

The model doesn't do sums itself. It runs the same actions as the [MCP server](mcp.md), which query the database, and answers from their results. Each answer lists the actions it used to reach it.

By default chat can only look things up. Under **What chat may change** in **Settings → AI Integration**, you can let it:

- **Change categories, rules and budgets:** create and rename categories, add rules and set budgets
- **Change transactions:** add, edit, recategorise, review and delete transactions

Chat makes changes without stopping to ask first. If **Checking chat actions** is on, a second model reads the recent conversation before each change and judges whether you asked for it. If it judges you probably didn't, the change is held back, and chat tells you what it would do and asks you to confirm.

The check is a safety net, not a guarantee. If it's off, or its model can't be reached, the change goes ahead. Every change can be undone.

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
