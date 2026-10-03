"""Categorising from the user's own history: a model trained on the transactions they've
categorised, used before any outside AI.

Logistic regression over character n-grams of the cleaned description. On a year's worth of
hand-categorised statements it was right on every line it was at least 70% sure about (about
four in five lines), and handled about half of merchants it had never seen
(see tools/evaluate_categorisers.py). It learns only from categories the user stands behind:
set by hand, by one of their rules, or confirmed. The model is retrained whenever that data
changes, which takes about a second, so there's nothing to store or migrate.
"""

import re
import threading
from dataclasses import dataclass

from sqlalchemy import func, or_, select
from sqlalchemy.orm import Session

from pennychest.accounts.models import Account, AccountType
from pennychest.ai.config import _get, _put
from pennychest.core.lookup_models import CategorisationSource
from pennychest.transactions.models import Posting, Transaction

ENABLED_KEY = "ai.learned.enabled"
UNCATEGORISED_PATH = "Expenses:Uncategorised"
# Below this it leaves the line for the AI (or the user)
CONFIDENCE = 0.7
MIN_EXAMPLES = 30
# Payment processors that prefix the real merchant's name
PREFIXES = {"SQ", "SUMUP", "ZETTLE", "IZ", "CRV", "PAYPAL", "PP", "CKO", "SP", "WWW"}


def clean(text: str) -> str:
    text = re.sub(r"[0-9]+", " ", (text or "").upper())
    text = re.sub(r"[^A-Z& ]+", " ", text)
    words = [w for w in text.split() if w not in PREFIXES]
    return " ".join(words)


def learned_enabled(db: Session) -> bool:
    return _get(db, ENABLED_KEY) != "off"


def set_learned_enabled(db: Session, enabled: bool) -> None:
    _put(db, ENABLED_KEY, "on" if enabled else "off")
    db.commit()


def _trusted_postings(db: Session):
    """Category postings the user stands behind, on transactions with exactly one category."""
    category_postings = (
        select(Posting.transaction_id)
        .join(Account, Account.id == Posting.account_id)
        .where(Account.type.in_([AccountType.EXPENSE, AccountType.INCOME]))
        .group_by(Posting.transaction_id)
        .having(func.count() == 1)
    )
    return (
        select(Posting.id, Posting.account_id, Transaction.description)
        .join(Transaction, Transaction.id == Posting.transaction_id)
        .join(Account, Account.id == Posting.account_id)
        .join(CategorisationSource, CategorisationSource.id == Posting.categorised_by_id)
        .where(
            Posting.transaction_id.in_(category_postings),
            Account.type.in_([AccountType.EXPENSE, AccountType.INCOME]),
            Account.full_path != UNCATEGORISED_PATH,
            or_(
                CategorisationSource.name.in_(["manual", "rule"]),
                Transaction.status == "confirmed",
            ),
        )
    )


@dataclass
class Model:
    signature: tuple
    vectoriser: object
    classifier: object
    examples: int
    categories: int

    def predict(self, descriptions: list[str]) -> list[tuple[int, float]]:
        probabilities = self.classifier.predict_proba(
            self.vectoriser.transform([clean(d) for d in descriptions])
        )
        best = probabilities.argmax(1)
        return [
            (int(self.classifier.classes_[i]), float(p[i])) for i, p in zip(best, probabilities)
        ]


def _make(strength: float = 20.0):
    # Imported here so the app starts without loading scikit-learn
    from sklearn.feature_extraction.text import TfidfVectorizer
    from sklearn.linear_model import LogisticRegression

    vectoriser = TfidfVectorizer(analyzer="char_wb", ngram_range=(2, 5), sublinear_tf=True)
    return vectoriser, LogisticRegression(C=strength, max_iter=5000)


_lock = threading.Lock()
_cache: dict[str, Model | None] = {}


def _signature(db: Session) -> tuple:
    # Changes whenever a trusted category is added, removed or changed
    rows = _trusted_postings(db).subquery()
    return tuple(
        db.execute(
            select(
                func.count(),
                func.coalesce(func.max(rows.c.id), 0),
                func.coalesce(func.sum(rows.c.id * 7 + rows.c.account_id), 0),
            )
        ).one()
    )


def get_model(db: Session) -> Model | None:
    """The model for the current data, retrained if anything changed; None until there's
    enough history to learn from."""
    signature = _signature(db)
    with _lock:
        if "model" in _cache and _cache.get("signature") == signature:
            return _cache["model"]
        rows = db.execute(_trusted_postings(db)).all()
        labels = [r.account_id for r in rows]
        model = None
        if len(rows) >= MIN_EXAMPLES and len(set(labels)) >= 2:
            vectoriser, classifier = _make()
            classifier.fit(vectoriser.fit_transform([clean(r.description) for r in rows]), labels)
            model = Model(signature, vectoriser, classifier, len(rows), len(set(labels)))
        _cache["model"] = model
        _cache["signature"] = signature
        return model


def suggest(db: Session, descriptions: list[str]) -> list[tuple[int, float] | None]:
    """For each description, (category account id, probability) when the model is confident
    enough to use, otherwise None."""
    if not descriptions or not learned_enabled(db):
        return [None] * len(descriptions)
    model = get_model(db)
    if model is None:
        return [None] * len(descriptions)
    return [
        (account_id, p) if p >= CONFIDENCE else None
        for account_id, p in model.predict(descriptions)
    ]


def status(db: Session) -> dict:
    """What the model has learned from, and how it does on that history (5-fold check)."""
    rows = db.execute(_trusted_postings(db)).all()
    labels = [r.account_id for r in rows]
    result = {
        "enabled": learned_enabled(db),
        "examples": len(rows),
        "categories": len(set(labels)),
        "min_examples": MIN_EXAMPLES,
        "confidence": CONFIDENCE,
        "ready": len(rows) >= MIN_EXAMPLES and len(set(labels)) >= 2,
        "check": None,
    }
    if result["ready"]:
        result["check"] = _check(db, rows, labels)
    return result


def _check(db: Session, rows, labels) -> dict:
    signature = _signature(db)
    with _lock:
        cached = _cache.get("check")
        if cached and cached[0] == signature:
            return cached[1]
    import numpy as np

    texts = np.array([clean(r.description) for r in rows], dtype=object)
    y = np.array(labels)
    order = np.random.default_rng(0).permutation(len(rows))
    sure = right = 0
    for fold in np.array_split(order, 5):
        train = np.setdiff1d(order, fold)
        if len(set(y[train])) < 2:
            continue
        vectoriser, classifier = _make()
        classifier.fit(vectoriser.fit_transform(texts[train]), y[train])
        probabilities = classifier.predict_proba(vectoriser.transform(texts[fold]))
        predicted = classifier.classes_[probabilities.argmax(1)]
        confident = probabilities.max(1) >= CONFIDENCE
        sure += int(confident.sum())
        right += int((confident & (predicted == y[fold])).sum())
    check = {
        # Share of past transactions it would have categorised itself...
        "coverage": sure / len(rows),
        # ...and how many of those it would have got right
        "accuracy": right / sure if sure else None,
    }
    with _lock:
        _cache["check"] = (signature, check)
    return check


def forget() -> None:
    """Drop cached models (tests, and anything that edits history outside the ORM)."""
    with _lock:
        _cache.clear()
