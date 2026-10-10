import json

import httpx
import pytest

from pennychest.ai import check_actions, providers
from pennychest.ai.config import set_provider_values
from pennychest.ai.providers import Call
from pennychest.chat import service
from tests.conftest import use_model
from tests.test_actions import ledger  # noqa: F401  (fixture)
from tests.test_chat import ScriptedModel, parse_sse


class FakeJev:
    """Answers every Noul question with `probability`, or fails with `status`."""

    def __init__(self):
        self.probability, self.status = 1.0, 200
        self.requests: list[dict] = []

    def handler(self, request: httpx.Request) -> httpx.Response:
        body = json.loads(request.content)
        self.requests.append(body)
        if self.status != 200:
            return httpx.Response(self.status, json={"error": "overloaded"})
        answers = {key: {"type": "noul", "noul": self.probability} for key in body["questions"]}
        return httpx.Response(200, json={"model": "jev-1", "answers": answers})


@pytest.fixture
def jev(db_session, monkeypatch):
    fake = FakeJev()
    monkeypatch.setattr(providers, "http_transport", httpx.MockTransport(fake.handler))
    set_provider_values(db_session, "typesafe", {"api_key": "ts-1"})
    use_model(db_session, "check_actions", "typesafe", "jev-latest")
    return fake


@pytest.fixture
def chat(client, ledger, monkeypatch):  # noqa: F811
    """Chat allowed to organise, with a scripted model that creates a Gym category."""
    client.put("/api/ai/chat/scopes", json={"scopes": ["organise"]})

    def use(*replies):
        scripted = ScriptedModel(
            *(replies or (("", [("create_category", {"name": "Gym", "parent": "Expenses"})]),
                          ("Done.", [])))
        )
        monkeypatch.setattr(service, "resolve_task", lambda db, task: (scripted, {}, "m1"))
        return scripted

    return use


def send(client, message, conversation_id=None):
    body = {"message": message}
    if conversation_id is not None:
        body["conversation_id"] = conversation_id
    return parse_sse(client.post("/api/chat/messages", json=body).text)


def category_paths(client):
    categories = client.post("/api/actions/list_categories", json={}).json()["categories"]
    return [c["path"] for c in categories]


def test_changes_the_user_asked_for_go_ahead(client, chat, jev):
    chat()
    jev.probability = 0.97
    events = send(client, "Add a Gym category")
    assert ("tool_result", {"name": "create_category", "ok": True}) in events
    assert "Expenses:Gym" in category_paths(client)

    [request] = jev.requests
    assert request["model"] == "jev-latest"
    assert request["questions"]["asked"]["type"] == "noul"
    assert request["state"]["conversation"] == [{"role": "user", "content": "Add a Gym category"}]
    proposed = request["state"]["proposed_action"]
    assert proposed["name"] == "create_category"
    assert proposed["arguments"] == {"name": "Gym", "parent": "Expenses"}
    logs = client.get("/api/ai/logs").json()["logs"]
    assert "check_action" in [log["operation"] for log in logs]


def test_unlikely_changes_are_held_back_and_the_model_asks(client, chat, jev):
    scripted = chat()
    jev.probability = 0.1
    events = send(client, "How much did I spend on food?")
    assert ("tool_result", {"name": "create_category", "ok": False, "held": True}) in events
    assert not [e for e, _ in events if e == "change"]
    assert "Expenses:Gym" not in category_paths(client)
    assert client.get("/api/changes").json()["changes"] == []

    result = scripted.calls[1]["history"][-1]
    assert result["is_error"] and "Don't try it again unless they confirm" in result["content"]
    turn = client.get(f"/api/chat/conversations/{events[0][1]['id']}").json()["turns"][-1]
    assert turn["tools"] == [{"name": "create_category", "ok": False, "held": True}]


def test_read_actions_are_never_checked(client, chat, jev):
    chat(("", [("list_categories", {})]), ("You have some.", []))
    send(client, "What categories do I have?")
    assert jev.requests == []


def test_actions_the_chat_cannot_run_are_not_checked(client, chat, jev):
    chat(("", [("add_transaction", {"description": "x"})]), ("I can't.", []))
    send(client, "Add a transaction")
    assert jev.requests == []


def test_nothing_is_checked_unless_turned_on(client, chat, monkeypatch):
    fake = FakeJev()
    monkeypatch.setattr(providers, "http_transport", httpx.MockTransport(fake.handler))
    chat()
    send(client, "Add a Gym category")
    assert fake.requests == []
    assert "Expenses:Gym" in category_paths(client)


def test_a_failing_check_lets_the_change_through(client, chat, jev):
    chat()
    jev.status = 503
    events = send(client, "Add a Gym category")
    assert ("tool_result", {"name": "create_category", "ok": True}) in events
    logs = client.get("/api/ai/logs").json()["logs"]
    assert any(log["operation"] == "check_action" and log["error"] for log in logs)


def test_llms_give_a_probability(db_session, monkeypatch):
    class FakeLLM(providers.Provider):
        id, label = "fake", "Fake"

        def complete_json(self, cfg, model, system, prompt, schema, name):
            self.prompt = prompt
            return Call({"probability": 1.4}, {"prompt": prompt}, {})

    fake = FakeLLM()
    monkeypatch.setattr(check_actions, "resolve_task", lambda db, task: (fake, {}, "m1"))
    conversation = [
        {"role": "user", "content": "Make a rule for Deliveroo"},
        {"role": "assistant", "content": "", "tool_calls": []},
        {"role": "tool", "content": "{}", "tool_call_id": "t1", "name": "x", "is_error": False},
    ]
    verdict = check_actions.check_action(
        db_session, conversation, "create_rule", "Create a rule.", {"pattern": "DELIVEROO"}
    )
    assert verdict.probability == 1.0 and verdict.allowed
    assert '"content": "Make a rule for Deliveroo"' in fake.prompt
    assert "DELIVEROO" in fake.prompt and '"role": "tool"' not in fake.prompt
