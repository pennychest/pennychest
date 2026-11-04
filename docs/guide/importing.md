# Importing statements

Drop statement files onto the **Import** page. PennyChest reads CSV files itself; for other formats, such as HSBC's PDF statements, install a [plugin](plugins.md). Each file is checked against the transactions you already have, categorised, and left for you to review.

## CSV files

Every bank lays out its CSV export differently. When you add one, the **Columns** card shows its first rows and which column holds each field:

- **Date** and **Description**
- **Amount**, when one column holds both money in and out, or **Money out** and **Money in** when they're separate. Money out counts as outgoing whether the bank writes it as `12.50` or `-12.50`.

Each column is chosen, in order:

1. **From a template** you saved for files laid out the same way.
2. **By name.** Common names such as *Date*, *Description*, *Amount* or *Paid out* are recognised.
3. **By AI**, if *Reading CSV columns* is on. A decision model says how sure it is, and anything under 70% is highlighted for you to check. If the task runs [when you ask](ai-integration.md#automatically-or-when-you-ask), press **Suggest with AI**.

You can change any column, and the file can't be imported until it has a date, a description and an amount.

### Templates

Once the columns are right, give them a name under **Save as a template**, e.g. *Monzo export*. Later files with the same headers use the template straight away. Header case and spacing don't matter.

## Duplicates

You can import a month's transactions a few at a time and still import the full statement at the end. A new transaction is flagged as a possible duplicate when one already on the account has the same date and amount, and the same description or one with all the same words in it, such as *COUNCIL TAX* and *COUNCIL TAX DD*. If *Matching taps, duplicates and transfers* is on, its model also looks for ones described differently, up to three days apart.

On the import's review page, delete or keep each one, or press **Delete Duplicates** to delete them all, leaving only what the statement added. The transactions they duplicate stay as they are. **Confirm All** skips flagged transactions, so they can't count twice unless you confirm one yourself.
