# AI agents (MCP)

PennyChest includes an [MCP](https://modelcontextprotocol.io) server, so AI assistants you already use can work with your ledger. These include Claude Code, Claude Desktop, claude.ai and ChatGPT. They run the same actions as the in-app [chat](ai-integration.md#chat). Answers come from database queries, so the numbers are never the model's own arithmetic.

The server is at `https://<your PennyChest>/mcp`, using Streamable HTTP. Set it up under **Settings → AI agents (MCP)**.

## What an agent can do

An agent can always look things up. It can change things only if you allow it, separately for each token or connector. Each action describes itself to the agent, including what its inputs mean, so you don't need to explain PennyChest to it.

### Look up your data (always allowed)

| Action | What it does |
|---|---|
| `list_accounts` | Your bank accounts, cards and other accounts |
| `list_categories` | Your spending and income categories |
| `list_transactions` | Find transactions, with their categories, accounts and review status |
| `list_rules` | Your categorisation rules |
| `list_card_taps` | [Card taps](card-taps.md), including spending that isn't on a statement yet |
| `get_budgets` | Budgets and how much is left of each |
| `spending_summary` | Spending or income over a period, by category, month or merchant |
| `compare_periods` | Two periods side by side, with what changed most |
| `cash_flow` | Income, spending and what was left over, month by month |
| `net_worth_history` | Net worth at the end of each month |
| `transactions_digest` | Every transaction in a period in one go, for spotting patterns |
| `recurring_payments` | Active subscriptions and bills, with what each costs a year |
| `unusual_charges` | Charges that look unusual, and bills that went up |
| `savings_opportunities` | An overview of where you could save money |

### Change categories, rules and budgets

| Action | What it does |
|---|---|
| `create_category` | Add a category under an existing one |
| `rename_category` | Rename a category, keeping everything attached to it |
| `create_rule` | Add a rule for future imports |
| `set_budget` | Set a monthly or annual budget for a category |
| `delete_budget` | Remove a budget |

### Change transactions

| Action | What it does |
|---|---|
| `add_transaction` | Record a payment or income, e.g. a cash purchase |
| `add_transfer` | Record money moved between your own accounts |
| `edit_transaction` | Change a transaction's description, date, amount, category or account |
| `recategorise_transactions` | Move transactions to another category |
| `mark_reviewed` | Mark imported transactions as reviewed |
| `delete_transactions` | Delete transactions |

`delete_transactions` and `delete_budget` are marked as destructive, so clients that support it ask you before running them. Every change an agent makes is listed under **Settings → Changes made by AI** and can be undone.

`recurring_payments` and `unusual_charges` use the labels and scores from the *Flagging subscriptions and unusual charges* [task](ai-integration.md#tasks). Without it, they have less to go on.

### Things to ask

- "How much did I spend on eating out last month, compared with the month before?"
- "What subscriptions am I paying for, and what do they cost a year?"
- "Anything unusual on my cards this week?"
- "Am I spending more than I earn?"
- "Put everything from COSTA in Eating out, and add a rule for it."

## Claude Code, Claude Desktop and other local clients

These sign in with a personal access token:

1. Under **Settings → AI agents (MCP) → New access token**, give the token a name, e.g. *Claude Code on my laptop*, and tick what it may change.
2. Copy the token straight away, because it isn't stored and won't be shown again. PennyChest also shows the setup for Claude Code and Claude Desktop with the token filled in:

    === "Claude Code"

        ```bash
        claude mcp add --transport http pennychest https://<your PennyChest>/mcp \
          --header "Authorization: Bearer pc_..."
        ```

    === "Claude Desktop"

        Add this to `claude_desktop_config.json`. It needs Node.js, for `mcp-remote`.

        ```json
        {
          "mcpServers": {
            "pennychest": {
              "command": "npx",
              "args": ["mcp-remote", "https://<your PennyChest>/mcp", "--header", "Authorization:${PENNYCHEST_AUTH}"],
              "env": { "PENNYCHEST_AUTH": "Bearer pc_..." }
            }
          }
        }
        ```

    === "Other clients"

        Use the server address with Streamable HTTP, and send the token as `Authorization: Bearer pc_...`.

## claude.ai, ChatGPT and other web apps

Web apps sign in with OAuth, so no token is needed:

1. In the app, add a custom connector with the address `https://<your PennyChest>/mcp`. PennyChest shows it under **Settings → AI agents (MCP)** to copy.
2. The app sends you to PennyChest to sign in. You then choose what the connector may change, if anything.
3. The connector then appears in the token list as a connected app.

For this to work, the web app's servers need to reach your PennyChest. That means it must be on the internet over HTTPS. If it's behind a proxy, the proxy must pass on `X-Forwarded-Proto`. Connectors' access lasts an hour at a time and is renewed automatically.

## Revoking access

Every token and connected app is listed under **Settings → AI agents (MCP) → Tokens**, along with when it was last used. **Revoke** cuts it off straight away.
