from datetime import date
from decimal import Decimal

import pytest

from pennychest.actions.catalog import ACTIONS, Scope
from pennychest.actions.undo import UNDO
from pennychest.chat import service
from pennychest.taps.models import CardTap
from tests.test_actions import (  # noqa: F401  (fixtures)
    _postings,
    _statement_row,
    act,
    imported,
    ledger,
    ok,
    spending,
)
from tests.test_chat import ScriptedModel, parse_sse


def changes(client):
    return client.get("/api/changes").json()["changes"]


def undo(client, change_id):
    return client.post(f"/api/changes/{change_id}/undo")


def undone(client, result):
    response = undo(client, result["change"]["id"])
    assert response.status_code == 200, response.text
    return response.json()


def category_paths(client):
    return {c["path"] for c in ok(client, "list_categories")["categories"]}


# Recording


def test_every_changing_action_can_be_undone():
    changing = {name for name, a in ACTIONS.items() if a.scope != Scope.READ}
    assert set(UNDO) == changing


def test_changes_are_recorded_with_a_summary(client, ledger):
    assert "change" not in ok(client, "list_categories")
    result = ok(client, "create_category", name="Amazon Prime", parent="Food")
    assert result["change"]["summary"] == 'Created category "Amazon Prime" under Expenses:Food'
    [logged] = changes(client)
    assert logged["id"] == result["change"]["id"]
    assert logged["action"] == "create_category" and logged["source"] == "api"
    assert logged["undone_at"] is None

    nothing = ok(client, "recategorise_transactions", transaction_ids=[999999], category="Food")
    assert "change" not in nothing
    assert len(changes(client)) == 1


def test_undo_endpoint_errors(client, ledger):
    assert undo(client, 12345).status_code == 404
    result = ok(client, "create_rule", pattern="DELIVEROO", category="Restaurants")
    undone(client, result)
    again = undo(client, result["change"]["id"])
    assert again.status_code == 400 and "already been undone" in again.json()["detail"]
    assert changes(client)[0]["undone_at"] is not None


def test_changes_require_session(anon_client):
    assert anon_client.get("/api/changes").status_code == 401
    assert anon_client.post("/api/changes/1/undo").status_code == 401


# Undoing each kind of change


def test_undo_create_category_unless_it_is_now_used(client, ledger):
    gym = ok(client, "create_category", name="Gym", parent="Expenses")
    undone(client, gym)
    assert "Expenses:Gym" not in category_paths(client)

    pets = ok(client, "create_category", name="Pets", parent="Expenses")
    ok(
        client,
        "add_transaction",
        description="Vet",
        amount="40",
        category="Pets",
        account="Current",
    )
    response = undo(client, pets["change"]["id"])
    assert response.status_code == 400 and "now has transactions" in response.json()["detail"]
    assert "Expenses:Pets" in category_paths(client)


def test_undo_rename_category(client, spending):
    renamed = ok(client, "rename_category", category="Food", new_name="Food & Drink")
    undone(client, renamed)
    assert {"Expenses:Food", "Expenses:Food:Groceries"} <= category_paths(client)
    assert "Expenses:FoodDrink" not in category_paths(client)

    first = ok(client, "rename_category", category="Transport", new_name="Travel")
    ok(client, "rename_category", category="Travel", new_name="Getting Around")
    response = undo(client, first["change"]["id"])
    assert response.status_code == 400 and "renamed again" in response.json()["detail"]


def test_undo_budgets(client, ledger):
    new = ok(client, "set_budget", category="Food", amount="200")
    undone(client, new)
    assert ok(client, "get_budgets")["budgets"] == []

    ok(client, "set_budget", category="Food", amount="200")
    raised = ok(client, "set_budget", category="Food", amount="350")
    undone(client, raised)
    assert ok(client, "get_budgets")["budgets"][0]["limit"] == "200.00"

    removed = ok(client, "delete_budget", category="Food")
    undone(client, removed)
    assert ok(client, "get_budgets")["budgets"][0]["limit"] == "200.00"


def test_undo_recategorise(client, spending):
    moved = ok(
        client,
        "recategorise_transactions",
        category="Transport",
        transaction_ids=[spending["tesco_aug"], spending["pizza"]],
    )
    undone(client, moved)
    by_id = {t["id"]: t for t in ok(client, "list_transactions")["transactions"]}
    assert by_id[spending["tesco_aug"]]["category"] == "Expenses:Food:Groceries"
    assert by_id[spending["pizza"]]["category"] == "Expenses:Food:Restaurants"


def test_undo_added_transactions_and_transfers(client, ledger):
    coffee = ok(
        client,
        "add_transaction",
        description="Coffee",
        amount="3",
        category="Restaurants",
        account="Current",
    )
    transfer = ok(client, "add_transfer", amount="50", from_account="Current", to_account="Savings")
    assert "Coffee" in coffee["change"]["summary"]
    undone(client, coffee)
    undone(client, transfer)
    assert ok(client, "list_transactions")["total"] == 0


def test_undo_edit_restores_every_field(client, ledger):
    txn = ok(
        client,
        "add_transaction",
        description="Coffee",
        amount="3",
        category="Restaurants",
        account="Current",
        date="2026-08-01",
    )["added"]
    edited = ok(
        client,
        "edit_transaction",
        transaction_id=txn["id"],
        description="Pay",
        date="2026-08-09",
        amount="1500",
        category="Salary",
        account="Visa",
    )
    assert edited["change"]["summary"] == (
        'Edited the description, date, amount, category, account of "Coffee" (2026-08-01)'
    )
    undone(client, edited)
    after = ok(client, "list_transactions")["transactions"][0]
    assert {k: after[k] for k in ("description", "date", "category", "amount", "account")} == {
        "description": "Coffee",
        "date": "2026-08-01",
        "category": "Expenses:Food:Restaurants",
        "amount": "3.00",
        "account": "Assets:Bank:Current",
    }


def test_undo_delete_puts_transactions_back_with_their_links(
    client, spending, imported, db_session
):
    tap = CardTap(
        tapped_on=date(2026, 8, 5), merchant="Tesco", amount=Decimal("20"), transaction_id=imported
    )
    db_session.add(tap)
    db_session.commit()
    before = {t["id"]: t for t in ok(client, "list_transactions")["transactions"]}
    postings_before = _postings(client, imported)

    deleted = ok(client, "delete_transactions", transaction_ids=[imported, spending["transfer"]])
    assert deleted["change"]["summary"] == "Deleted 2 transactions"
    db_session.expire_all()
    assert db_session.get(CardTap, tap.id).transaction_id is None

    undone(client, deleted)
    after = {t["id"]: t for t in ok(client, "list_transactions")["transactions"]}
    assert after[imported] == before[imported]
    assert after[spending["transfer"]] == before[spending["transfer"]]
    assert _postings(client, imported) == postings_before
    assert _statement_row(db_session).transaction_id == imported
    db_session.expire_all()
    assert db_session.get(CardTap, tap.id).transaction_id == imported


def test_undo_mark_reviewed(client, spending):
    reviewed = ok(client, "mark_reviewed", transaction_ids=[spending["pending"]])
    undone(client, reviewed)
    assert [t["id"] for t in ok(client, "list_transactions", status="pending")["transactions"]] == [
        spending["pending"]
    ]


# Chat


def test_chat_permissions_setting(client):
    assert client.get("/api/ai/config").json()["tasks"]["chat"]["scopes"] == []
    assert client.put("/api/ai/chat/scopes", json={"scopes": ["admin"]}).status_code == 400
    assert (
        client.put("/api/ai/chat/scopes", json={"scopes": ["transactions", "organise"]}).status_code
        == 204
    )
    assert client.get("/api/ai/config").json()["tasks"]["chat"]["scopes"] == [
        "organise",
        "transactions",
    ]
    assert client.put("/api/chat/scopes", json={"scopes": []}).status_code in (404, 405)


def test_system_prompt_describes_permissions():
    read_only = service.system_prompt(date(2026, 9, 30))
    assert "can't change anything" in read_only and "Settings > AI Integration > Chat" in read_only
    organise = service.system_prompt(date(2026, 9, 30), ["organise"])
    assert "create and rename categories" in organise
    assert "You can't change transactions" in organise
    both = service.system_prompt(date(2026, 9, 30), ["organise", "transactions"])
    assert "without asking for confirmation" in both and "can't" not in both


def test_chat_makes_changes_it_is_allowed_to_and_they_can_be_undone(client, ledger, monkeypatch):
    client.put("/api/ai/chat/scopes", json={"scopes": ["organise"]})
    scripted = ScriptedModel(
        (
            "",
            [
                ("create_category", {"name": "Amazon Prime", "parent": "Food"}),
                (
                    "add_transaction",
                    {"description": "x", "amount": "1", "category": "Food", "account": "Current"},
                ),
            ],
        ),
        ("Created Amazon Prime under Food. I can't add transactions yet.", []),
    )
    monkeypatch.setattr(service, "resolve_task", lambda db, task: (scripted, {}, "m1"))
    response = client.post("/api/chat/messages", json={"message": "Add Amazon Prime"})
    events = parse_sse(response.text)

    offered = set(scripted.calls[0]["tools"])
    assert "create_category" in offered and "add_transaction" not in offered
    change_events = [data for name, data in events if name == "change"]
    assert [c["summary"] for c in change_events] == [
        'Created category "Amazon Prime" under Expenses:Food',
    ]
    assert ("tool_result", {"name": "add_transaction", "ok": False}) in events
    assert changes(client)[0]["source"] == "chat"

    conversation_id = events[0][1]["id"]
    turn = client.get(f"/api/chat/conversations/{conversation_id}").json()["turns"][-1]
    assert turn["changes"] == [
        {"id": change_events[0]["id"], "summary": change_events[0]["summary"], "undone": False}
    ]

    assert undo(client, change_events[0]["id"]).status_code == 200
    turn = client.get(f"/api/chat/conversations/{conversation_id}").json()["turns"][-1]
    assert turn["changes"][0]["undone"] is True
    assert "Expenses:Food:AmazonPrime" not in category_paths(client)


def test_deleting_a_conversation_keeps_its_changes(client, ledger, monkeypatch):
    client.put("/api/ai/chat/scopes", json={"scopes": ["organise"]})
    scripted = ScriptedModel(
        ("", [("create_category", {"name": "Gym", "parent": "Expenses"})]), ("Done.", [])
    )
    monkeypatch.setattr(service, "resolve_task", lambda db, task: (scripted, {}, "m1"))
    events = parse_sse(client.post("/api/chat/messages", json={"message": "Gym"}).text)
    client.delete(f"/api/chat/conversations/{events[0][1]['id']}")
    [change] = changes(client)
    assert change["conversation_id"] is None
    assert undo(client, change["id"]).status_code == 200


@pytest.mark.parametrize("scopes", [[], ["transactions"]])
def test_chat_without_organise_cannot_create_categories(client, ledger, monkeypatch, scopes):
    client.put("/api/ai/chat/scopes", json={"scopes": scopes})
    scripted = ScriptedModel(
        ("", [("create_category", {"name": "Gym", "parent": "Expenses"})]), ("I can't do that.", [])
    )
    monkeypatch.setattr(service, "resolve_task", lambda db, task: (scripted, {}, "m1"))
    parse_sse(client.post("/api/chat/messages", json={"message": "Gym"}).text)
    assert "Expenses:Gym" not in category_paths(client)
    assert changes(client) == []
