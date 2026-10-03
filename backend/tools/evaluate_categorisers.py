"""Compare ways of learning categories from past transactions, on your own data.

Answers "would a model trained on what I've already categorised get new transactions right?",
and which method does best. Nothing is changed: it only reads the data it's given.

Data, either:
  --db PATH     a PennyChest SQLite database (e.g. a snapshot of /data/pennychest.db)
  --csv PATH    a CSV with columns description, category and optionally date

From a database, only transactions whose category you can trust are used. --trust picks them:
  manual      categorised by hand
  rule        categorised by one of your rules
  confirmed   any transaction you've confirmed (including ones the AI categorised)
  ai          AI-categorised, even if not confirmed (measures agreement with the AI, not accuracy)
Default: manual,rule,confirmed.

Two tests:
  time split     train on the oldest 80%, test on the newest 20% (how it would be used for real)
  cross-check    predict every transaction from the others: leave-one-out up to 300
                 transactions, otherwise 10-fold (each tenth predicted from the other nine)

Methods: merchant lookup (the baseline any "learned rules" would match), naive Bayes, logistic
regression, linear SVM, and cosine similarity (nearest neighbour, 5 nearest, category centroid),
all on character n-grams of the cleaned description.

Run it in a throwaway container so nothing is installed on the host:
  docker run --rm -v "$PWD:/w" python:3.12-slim sh -c \\
    "pip install -q scikit-learn && python /w/evaluate_categorisers.py --db /w/pennychest.db"
"""

import argparse
import csv
import re
import sqlite3
from collections import Counter, defaultdict

import numpy as np
from sklearn.feature_extraction.text import CountVectorizer, TfidfVectorizer
from sklearn.linear_model import LogisticRegression
from sklearn.naive_bayes import MultinomialNB
from sklearn.preprocessing import normalize
from sklearn.svm import LinearSVC

UNCATEGORISED = "Expenses:Uncategorised"
# Payment processors that prefix the real merchant's name
PREFIXES = {"SQ", "SUMUP", "ZETTLE", "IZ", "CRV", "PAYPAL", "PP", "CKO", "SP", "WWW"}


def clean(text: str) -> str:
    text = re.sub(r"[0-9]+", " ", text.upper())
    text = re.sub(r"[^A-Z& ]+", " ", text)
    return re.sub(r"\s+", " ", text).strip()


def merchant_key(text: str) -> str:
    words = [w for w in clean(text).split() if w not in PREFIXES and len(w) > 1]
    return " ".join(words[:2])


def load_db(path: str, trust: set[str]) -> list[tuple[str, str, str]]:
    db = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    rows = db.execute(
        """
        select t.id, t.date, t.description, a.full_path, cs.name, t.status
        from transactions t
        join postings p on p.transaction_id = t.id
        join accounts a on a.id = p.account_id
        join categorisation_sources cs on cs.id = p.categorised_by_id
        where lower(a.type) in ('expense', 'income')
        """
    ).fetchall()
    by_txn = defaultdict(list)
    for r in rows:
        by_txn[r[0]].append(r)
    single = [v[0] for v in by_txn.values() if len(v) == 1]
    print(f"Transactions with one category: {len(single)}, by source and status:")
    for (source, status), n in Counter((r[4], r[5]) for r in single).most_common():
        print(f"  {source:15} {status:10} {n}")
    kept = [
        (r[1], r[2], r[3])
        for r in single
        if r[3] != UNCATEGORISED
        and (r[4] in trust or ("confirmed" in trust and r[5] == "confirmed"))
    ]
    print(f"Using {len(kept)} (trusting: {', '.join(sorted(trust))})")
    return kept


def load_csv(path: str) -> list[tuple[str, str, str]]:
    with open(path, newline="") as f:
        rows = [
            (r.get("date") or "", r["description"], r["category"])
            for r in csv.DictReader(f)
            if r.get("description") and r.get("category")
        ]
    print(f"Using {len(rows)} rows from {path}")
    return rows


class Methods:
    """Each method is trained on (descriptions, categories) and predicts for new
    descriptions, returning (predictions, confidences); None means it declined to answer."""

    @staticmethod
    def lookup(train_x, train_y, test_x):
        votes = defaultdict(Counter)
        for x, y in zip(train_x, train_y):
            votes[merchant_key(x)][y] += 1
        preds, conf = [], []
        for x in test_x:
            v = votes.get(merchant_key(x))
            if v:
                cat, n = v.most_common(1)[0]
                preds.append(cat)
                conf.append(n / sum(v.values()))
            else:
                preds.append(None)
                conf.append(0.0)
        return preds, conf

    @staticmethod
    def _tfidf(train_x, test_x):
        vec = TfidfVectorizer(analyzer="char_wb", ngram_range=(2, 5), sublinear_tf=True)
        return vec.fit_transform(map(clean, train_x)), vec.transform(map(clean, test_x))

    @staticmethod
    def naive_bayes(train_x, train_y, test_x):
        vec = CountVectorizer(analyzer="char_wb", ngram_range=(2, 5))
        model = MultinomialNB(alpha=0.1).fit(vec.fit_transform(map(clean, train_x)), train_y)
        p = model.predict_proba(vec.transform(map(clean, test_x)))
        return list(model.classes_[p.argmax(1)]), list(p.max(1))

    @classmethod
    def logistic_regression(cls, train_x, train_y, test_x):
        a, b = cls._tfidf(train_x, test_x)
        model = LogisticRegression(C=20, max_iter=5000).fit(a, train_y)
        p = model.predict_proba(b)
        return list(model.classes_[p.argmax(1)]), list(p.max(1))

    @classmethod
    def linear_svm(cls, train_x, train_y, test_x):
        a, b = cls._tfidf(train_x, test_x)
        model = LinearSVC().fit(a, train_y)
        scores = model.decision_function(b)
        if scores.ndim == 1:  # two categories
            scores = np.column_stack([-scores, scores])
        top2 = np.sort(scores, axis=1)[:, -2:]
        return list(model.classes_[scores.argmax(1)]), list(top2[:, 1] - top2[:, 0])

    @classmethod
    def cosine_nearest(cls, train_x, train_y, test_x):
        a, b = cls._tfidf(train_x, test_x)
        sims = (b @ a.T).toarray()
        return list(np.array(train_y)[sims.argmax(1)]), list(sims.max(1))

    @classmethod
    def cosine_5_nearest(cls, train_x, train_y, test_x):
        a, b = cls._tfidf(train_x, test_x)
        sims = (b @ a.T).toarray()
        ys = np.array(train_y)
        preds, conf = [], []
        for row in sims:
            weights = defaultdict(float)
            for j in np.argsort(row)[-5:]:
                weights[ys[j]] += max(row[j], 0)
            cat = max(weights, key=weights.get)
            preds.append(cat)
            conf.append(weights[cat] / (sum(weights.values()) or 1))
        return preds, conf

    @classmethod
    def cosine_centroid(cls, train_x, train_y, test_x):
        a, b = cls._tfidf(train_x, test_x)
        ys = np.array(train_y)
        classes = np.unique(ys)
        centroids = normalize(np.vstack([np.asarray(a[ys == c].mean(0)) for c in classes]))
        sims = b @ centroids.T
        return list(classes[sims.argmax(1)]), list(np.asarray(sims.max(1)).ravel())


METHODS = {
    "Merchant lookup (baseline)": (Methods.lookup, (0.5, 0.7, 0.9)),
    "Naive Bayes": (Methods.naive_bayes, (0.5, 0.7, 0.9)),
    "Logistic regression": (Methods.logistic_regression, (0.5, 0.7, 0.9)),
    "Linear SVM": (Methods.linear_svm, (0.2, 0.5, 1.0)),
    "Cosine, nearest neighbour": (Methods.cosine_nearest, (0.5, 0.7, 0.9)),
    "Cosine, 5 nearest": (Methods.cosine_5_nearest, (0.5, 0.7, 0.9)),
    "Cosine, category centroid": (Methods.cosine_centroid, (0.3, 0.5, 0.7)),
}


def summary(correct, new, answered):
    parts = [f"all {correct.mean():6.1%}"]
    if (~new).any():
        parts.append(f"seen merchants {correct[~new].mean():6.1%}")
    if new.any():
        parts.append(f"new merchants {correct[new].mean():6.1%}")
    if not answered.all():
        parts.append(f"(answers {answered.mean():.0%})")
    return "   ".join(parts)


def time_split(data, fraction):
    data = sorted(data, key=lambda r: r[0])
    cut = int(len(data) * fraction)
    train, test = data[:cut], data[cut:]
    train_x, train_y = [r[1] for r in train], [r[2] for r in train]
    test_x, test_y = [r[1] for r in test], np.array([r[2] for r in test], dtype=object)
    seen = {merchant_key(x) for x in train_x}
    new = np.array([merchant_key(x) not in seen for x in test_x])
    print(
        f"\nTime split: train {len(train)}, test {len(test)} "
        f"({new.sum()} from merchants not seen in training)"
    )
    best, best_acc, best_preds = None, -1.0, None
    for name, (method, thresholds) in METHODS.items():
        preds, conf = method(train_x, train_y, test_x)
        preds = np.array(preds, dtype=object)
        answered = preds != None  # noqa: E711
        correct = answered & (preds == test_y)
        print(f"  {name:28} {summary(correct, new, answered)}")
        conf = np.array(conf, dtype=float)
        sure = [
            f"conf>={t:g}: {correct[conf >= t].mean():5.1%} right on {(conf >= t).mean():4.0%}"
            for t in thresholds
            if (conf >= t).any()
        ]
        print(" " * 32 + " | ".join(sure))
        if correct.mean() > best_acc:
            best, best_acc, best_preds = name, correct.mean(), preds
    wrong = [i for i in range(len(test_x)) if best_preds[i] != test_y[i]]
    if wrong:
        print(f"\n  Mistakes by the most accurate ({best}): description -> predicted | actual")
        for i in wrong[:15]:
            print(f"    {test_x[i][:40]:40} -> {str(best_preds[i])[:32]:32} | {test_y[i]}")


def cross_check(data, folds):
    """Predict every transaction from the rest: leave-one-out when folds is 0, otherwise k-fold
    (shuffled with a fixed seed, so runs are repeatable)."""
    xs, ys = [r[1] for r in data], [r[2] for r in data]
    n = len(xs)
    keys = [merchant_key(x) for x in xs]
    if folds:
        groups = np.array_split(np.random.default_rng(0).permutation(n), folds)
        label = f"{folds}-fold cross-check"
    else:
        groups = [[i] for i in range(n)]
        label = "Leave-one-out"
    hits = {name: np.zeros(n, dtype=bool) for name in METHODS}
    answered = {name: np.zeros(n, dtype=bool) for name in METHODS}
    new = np.zeros(n, dtype=bool)
    for group in groups:
        held = sorted(int(i) for i in group)
        rest = [i for i in range(n) if i not in set(held)]
        train_x, train_y = [xs[i] for i in rest], [ys[i] for i in rest]
        seen = {keys[i] for i in rest}
        for i in held:
            new[i] = keys[i] not in seen
        if len(set(train_y)) < 2:
            continue
        for name, (method, _) in METHODS.items():
            preds = method(train_x, train_y, [xs[i] for i in held])[0]
            for i, pred in zip(held, preds):
                answered[name][i] = pred is not None
                hits[name][i] = pred == ys[i]
    print(f"\n{label} over {n} ({new.sum()} from merchants missing from their training data)")
    for name in METHODS:
        print(f"  {name:28} {summary(hits[name], new, answered[name])}")


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--db", help="PennyChest SQLite database")
    source.add_argument("--csv", help="CSV with description, category and optional date")
    parser.add_argument(
        "--trust",
        default="manual,rule,confirmed",
        help="which categorisations to learn from (database only)",
    )
    parser.add_argument("--train-fraction", type=float, default=0.8)
    parser.add_argument(
        "--folds",
        type=int,
        default=None,
        help="parts for the cross-check: 0 for leave-one-out "
        "(default: leave-one-out up to 300 transactions, else 10)",
    )
    parser.add_argument("--skip-cross-check", action="store_true")
    args = parser.parse_args()

    data = load_db(args.db, set(args.trust.split(","))) if args.db else load_csv(args.csv)
    categories = Counter(r[2] for r in data)
    print(
        f"{len(categories)} categories; most used: "
        + ", ".join(f"{c} ({n})" for c, n in categories.most_common(5))
    )
    if len(data) < 20 or len(categories) < 2:
        raise SystemExit("Too little data to compare methods.")
    time_split(data, args.train_fraction)
    if not args.skip_cross_check:
        folds = args.folds if args.folds is not None else (0 if len(data) <= 300 else 10)
        cross_check(data, folds)


if __name__ == "__main__":
    main()
