# Categorisation

PennyChest categorises in layers. Each layer only answers when it's confident, and passes everything else down to the next one. Whatever is left at the end comes to you to review.

## The layers

When you import a statement, each transaction goes through these layers in order until one of them categorises it:

1. **Your rules.** Rules match a transaction's description by prefix, substring or regular expression, e.g. `COOP` → `Expenses:Food`. When more than one rule matches, the one with the highest priority wins. PennyChest comes with UK default rules, which you can edit under **Settings → Categorisation Rules**.
2. **Card taps.** If a [card tap](card-taps.md) from your phone matches the statement line, the line takes the tap's category.
3. **Learning from your categories.** This is a model trained on transactions you've already categorised. It runs inside PennyChest and needs no AI provider.
4. **AI.** If *Categorising transactions* is on, the remaining lines go to its model, straight away or when you press **Suggest with AI** on the import's review page; see [AI integration](ai-integration.md#tasks). This layer works best with a decision model such as [Jev](#jev), which reports how sure it is.
5. **You.** Anything still uncategorised stays as `Expenses:Uncategorised` for you to review. These are mostly the genuinely ambiguous ones, such as PayPal, SumUp and Amazon.

An import's review page shows how each transaction was categorised, and confirming a transaction there tells PennyChest you agree with it. Everything the learned model and the AI categorised in an import is recorded as one change, so you can undo it in one go under **Settings → Changes made by AI**.

## Learning from your categories

PennyChest trains a [logistic regression](https://en.wikipedia.org/wiki/Logistic_regression) classifier on your own transaction history:

- **What it learns from:** only categories you stand behind. These are transactions you categorised by hand, ones your rules categorised, and any you've confirmed. It doesn't learn from unconfirmed AI guesses, so it can't reinforce the AI's mistakes.
- **What it looks at:** the description, cleaned up. Digits are removed, along with payment-processor prefixes like `SQ`, `SUMUP` and `PAYPAL` that hide the real merchant. It is then split into 2–5 letter chunks, weighted by TF-IDF. This lets it recognise a merchant whose descriptions vary slightly from one statement to the next.
- **When it answers:** only when it's at least **70%** sure. Below that, the transaction is passed on to the AI.
- **When it starts:** once you have at least 30 trusted transactions in at least two categories.
- **Training:** it retrains itself whenever your categories change. This takes about a second, so nothing needs to be stored or migrated.

It also categorises [card taps](card-taps.md) as they arrive.

You'll find it under **Settings → AI Integration → Learning from your categories**. Once it's ready, the card shows how many transactions it learned from. It also shows how it would have done on your own history, from a 5-fold check: the share of transactions it would have answered, and how many of those it would have got right. Untick **Use it** to turn it off.

## Jev

[Jev](https://typesafe.ai), from TypeSafe AI, is a decision model rather than a chatbot. It's given each transaction and your list of categories, and picks one category together with a confidence that is meant to be calibrated.

PennyChest uses a Jev answer only when its confidence is at least **0.9**. Anything less sure is left for you rather than guessed. Jev hasn't been trained on your data, but it knows what a shop is, which makes it the best layer for merchants you've never used before.

You can also use OpenAI's Decisions API, another decision model, or a language model such as Claude, GPT, Gemini or an Ollama model; see [AI integration](ai-integration.md). Language models don't report a confidence, so every answer they give is used.

## Card taps

[Card taps](card-taps.md) are payments your phone sends to PennyChest as you make them. They're categorised the moment they arrive, by your rules, then the learned model, then AI if *Categorising card taps* is on. When the statement arrives, each tap passes its category on to the line it matches.
