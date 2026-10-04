# Categorisation

Off-the-shelf categorisation typically gets 70–90% of transactions right, and never says when it's unsure. PennyChest works in layers instead. Each layer only answers when it's confident, and passes everything else down to the next one. Whatever is left at the end comes to you to review.

On seven months of real spending, this got **98.8% of transactions right while categorising 83% of them automatically**. See [how well it works](#how-well-it-works).

## The layers

When you import a statement, each transaction goes through these layers in order until one of them categorises it:

1. **Your rules.** Rules match a transaction's description by prefix, substring or regular expression, e.g. `COOP` → `Expenses:Food`. When more than one rule matches, the one with the highest priority wins. PennyChest comes with UK default rules, which you can edit under **Settings → Categorisation Rules**.
2. **Card taps.** If a [card tap](card-taps.md) from your phone matches the statement line, the line takes the tap's category.
3. **Learning from your categories.** This is a model trained on transactions you've already categorised. It runs inside PennyChest and needs no AI provider.
4. **AI.** If you've chosen a provider for *Categorising transactions*, the remaining lines go to it. This layer works best with [Jev](#jev), which reports how sure it is.
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

!!! tip "Its weak spot is shops it has never seen"
    It's right about 95% of the time on merchants you've used before, but only about half the time on new ones, because it has nothing to go on. That's the gap the AI layer fills.

## Jev

[Jev](https://typesafe.ai), from TypeSafe AI, is a decision model rather than a chatbot. It's given each transaction and your list of categories, and picks one category together with a confidence that is meant to be calibrated. In the testing below, its confidence held up: the higher the threshold, the more often it was right.

PennyChest uses a Jev answer only when its confidence is at least **0.9**. Anything less sure is left for you rather than guessed. Jev hasn't been trained on your data, but it knows what a shop is, which makes it the best layer for merchants you've never used before.

You can also use a general-purpose LLM for this task, e.g. Claude, GPT, Gemini or an Ollama model; see [AI integration](ai-integration.md). LLMs don't report a confidence, so every answer they give is used. They weren't part of the testing below.

## Card taps

[Card taps](card-taps.md) are payments your phone sends to PennyChest as you make them. They're categorised the moment they arrive, by your rules, then the learned model, then the AI provider chosen for *Categorising card taps*. When the statement arrives, each tap passes its category on to the line it matches.

## How well it works

All of these results come from the same dataset. It is **993 real transactions** from Jersey, categorised by hand: seven months (January–July 2026), five accounts and 24 categories. The categories are very uneven: Food (240) and Parking (219) hold almost half the lines, and many categories have only a handful. That's why rare categories and new shops are the hard part.

### Which learning method

Seven ways of learning from history were compared using 10-fold cross-validation. The lines were shuffled and split into tenths, and each tenth was predicted by a model trained on the other nine. A "new shop" is one whose name (its first two words) wasn't in that round's training data. That was 155 of the 993 lines, or 16%.

| Method | All lines | Shops seen before | New shops |
|---|---:|---:|---:|
| Merchant lookup | 80.3% | 95.1% | 0.0% |
| Naive Bayes | 85.4% | 91.8% | 51.0% |
| **Logistic regression** | **88.3%** | **94.7%** | **53.5%** |
| Linear SVM | 88.5% | 95.1% | 52.9% |
| Cosine similarity, nearest | 86.1% | 93.3% | 47.1% |
| Cosine similarity, 5 nearest | 85.4% | 93.1% | 43.9% |
| Cosine similarity, centroid | 79.5% | 85.8% | 45.2% |

Logistic regression and the linear SVM came out almost level. PennyChest uses logistic regression because it produces a probability, which is what decides whether the model answers or passes the transaction on.

### Learned model and Jev together

To test the layers the way they're used for real, the models were trained on the oldest 794 lines and tested on the newest 199. Of those 199, 37 were from shops not seen in training.

When made to answer every line, each layer is better at something different:

| | All lines | Shops seen before | New shops |
|---|---:|---:|---:|
| Learned model | 88.4% | 94.4% | 62.2% |
| Jev, no training | 71.4% | 72.8% | 64.9% |

The learned model knows your habits. Jev knows what a shop is. Each one covers the other's blind spot.

With confidence thresholds, the learned model answered first, at 70% sure or more. That was 157 of the 199 lines (79%), and it got **all of them** right. Jev then took the rest whenever its confidence passed a threshold:

| Jev threshold | Jev right | Jev wrong | Left for you | Right overall | Categorised automatically |
|---|---:|---:|---:|---:|---:|
| 0.5 | 14 | 9 | 19 | 95.0% | 90% |
| 0.7 | 10 | 5 | 27 | 97.1% | 86% |
| 0.9 (PennyChest's setting) | 7 | 2 | 33 | **98.8%** | **83%** |

Raising the threshold makes fewer mistakes, but leaves more lines for you. PennyChest uses 0.9, on the basis that reviewing a line you can see is uncategorised is easier than finding and correcting a wrong one. At that setting there were 2 mistakes in 166 automatic answers.

### Test it on your own data

`backend/tools/evaluate_categorisers.py` runs these comparisons on your own database, or on a CSV. It only reads the data and changes nothing. Copy the database out of your container, then run the script from a checkout of this repository in a throwaway container, so nothing is installed on the host:

```bash
docker cp <container>:/data/pennychest.db .
docker run --rm -v "$PWD/backend/tools:/w" -v "$PWD/pennychest.db:/data.db:ro" python:3.12-slim sh -c \
  "pip install -q scikit-learn && python /w/evaluate_categorisers.py --db /data.db"
```

Run it with `--help` to see the options, including which categories to trust and using a CSV instead of a database.
