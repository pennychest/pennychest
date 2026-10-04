# Card taps

Statements arrive weeks after you spend. Card taps close that gap: each time you pay with Apple Pay, an iPhone shortcut sends the payment to PennyChest straight away. It shows up on the **Card taps** page, already categorised. When the statement is imported later, each tap is matched to its statement line and passes its category on.

## What you need

- An iPhone with your cards in Apple Wallet.
- A PennyChest your phone can reach wherever you are. In practice that means one on the internet over HTTPS; see [Deploying to Fly.io](deploy-fly.md).

## Setting it up

1. In PennyChest, go to **Settings → Card taps** and press **Create token**. This shows the **App address** and **Tap token** that the shortcut needs.
2. On your iPhone, [install the PennyChest Tap shortcut](https://www.icloud.com/shortcuts/bfedfdd6c6fc428dbba076faf590cad8) and tap **Add Shortcut**. Paste the app address and tap token when it asks.
3. In the Shortcuts app, open **Automation**, tap **+**, and choose **Wallet**.
4. Select the cards to track, choose **Run Immediately**, and pick the **PennyChest Tap** shortcut.
5. Make a payment. The first time a card is used, the **Card taps** page asks you to **pair** it with the account it pays from, e.g. your credit card or current account. This only happens once per card.

## What happens to a tap

- **It's recorded.** The shortcut sends the merchant, the amount, the currency, the date and the card's name. Taps are kept separately from your ledger, so nothing is added to your accounts twice.
- **It's categorised.** A category is suggested right away, by your [rules](categorisation.md#the-layers) first, then [your history](categorisation.md#learning-from-your-categories), then the AI provider chosen for *Categorising card taps*, if there is one. Each tap shows where its suggestion came from. The phone never waits for this: the shortcut gets its reply first.
- **It's matched.** When you import a statement for the paired account, each waiting tap is matched to a statement line with the same amount, dated from a day before to a week after the tap. If several lines fit, the one whose description shares a word with the merchant wins. If that doesn't settle it and you've chosen a model for *Matching taps, duplicates and transfers*, the model picks, but only when it's confident. Otherwise the closest date wins.
- **Its category is passed on.** If a matched statement line was left uncategorised, it takes the tap's category.

The **Card taps** page lists taps that are waiting for a statement, taps that have been matched, and any you've dismissed. Dismiss a tap you don't want matched, such as a payment that was cancelled. You can also run the matching yourself at any time.

## Managing the token

Under **Settings → Card taps**:

- **Replace token** issues a new one; the old token stops working, so paste the new one into the shortcut.
- **Turn off** deletes the token, and PennyChest stops accepting taps.

## Other phones and tools

Anything that can make an HTTP request can send taps, e.g. an Android automation app. Send a `POST` to `/api/taps` with the header `Authorization: Bearer <tap token>` and a JSON body:

```json
{"merchant": "COOP JERSEY", "amount": "£12.50", "date": "4 Oct 2026", "card": "Visa Debit"}
```

The amount can include a currency symbol, or you can send `currency` separately. A tap without a date is recorded for today.
