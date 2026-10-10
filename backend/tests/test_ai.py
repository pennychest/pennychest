import json
from types import SimpleNamespace

import httpx
import pytest

from pennychest.ai import providers
from pennychest.ai.config import set_provider_values
from tests.conftest import use_model


class FakeAPI:
    """Stands in for provider HTTP APIs: records requests, returns queued responses."""

    def __init__(self):
        self.requests: list[httpx.Request] = []
        self.responses: dict[tuple[str, str], list] = {}

    def on(self, method: str, url: str, *responses):
        self.responses.setdefault((method, url), []).extend(responses)

    def handler(self, request: httpx.Request) -> httpx.Response:
        self.requests.append(request)
        queue = self.responses.get((request.method, str(request.url)))
        if not queue:
            return httpx.Response(500, json={"error": f"unexpected {request.method} {request.url}"})
        status, body = queue.pop(0)
        if isinstance(body, str):  # a streamed body, sent as-is
            return httpx.Response(status, content=body.encode())
        return httpx.Response(status, json=body)

    def body(self, index: int = -1) -> dict:
        return json.loads(self.requests[index].content)


@pytest.fixture
def api(monkeypatch):
    fake = FakeAPI()
    monkeypatch.setattr(providers, "http_transport", httpx.MockTransport(fake.handler))
    return fake


def _provider(client, provider_id):
    providers = client.get("/api/ai/config").json()["providers"]
    return next(p for p in providers if p["id"] == provider_id)


def _source_id(client, name):
    sources = client.get("/api/lookup/categorisation-sources").json()
    return next(s["id"] for s in sources if s["name"] == name)


def _imported_transactions(client, *descriptions):
    """Transactions as an import leaves them: the expense side on Expenses:Uncategorised."""
    card = client.post("/api/accounts", json={
        "name": "Card", "full_path": "Liabilities:Card", "type": "liability",
    }).json()
    for path in ("Expenses:Uncategorised", "Expenses:Groceries", "Expenses:Transport"):
        client.post("/api/accounts", json={
            "name": path.split(":")[-1], "full_path": path, "type": "expense",
        })
    accounts = {a["full_path"]: a["id"] for a in client.get("/api/accounts").json()}
    import_default = _source_id(client, "import_default")
    ids = []
    for description in descriptions:
        txn = client.post("/api/transactions", json={
            "date": "2026-09-20",
            "description": description,
            "postings": [
                {"account_id": card["id"], "amount": "-5.00", "categorised_by_id": import_default},
                {"account_id": accounts["Expenses:Uncategorised"], "amount": "5.00",
                 "categorised_by_id": import_default},
            ],
        }).json()
        ids.append(txn["id"])
    return ids, accounts


def _expense_account(client, txn_id):
    txn = client.get(f"/api/transactions/{txn_id}").json()
    return next(
        p["account_full_path"] for p in txn["postings"]
        if p["account_full_path"].startswith("Expenses")
    )


# Config endpoints


def test_config_lists_providers_without_revealing_secrets(client, db_session):
    set_provider_values(db_session, "openai", {"api_key": "sk-secret"})
    config = client.get("/api/ai/config").json()

    by_id = {p["id"]: p for p in config["providers"]}
    assert set(by_id) == {
        "anthropic", "openai", "google", "vertex", "typesafe", "openai_decisions", "ollama"
    }
    assert {p["id"] for p in config["providers"] if p["kind"] == "decision"} == {
        "typesafe", "openai_decisions"
    }
    assert by_id["openai"]["configured"] is True
    assert by_id["anthropic"]["configured"] is False
    key_field = next(f for f in by_id["openai"]["fields"] if f["key"] == "api_key")
    assert key_field == {**key_field, "is_set": True, "value": None, "secret": True}
    assert "sk-secret" not in json.dumps(config)
    assert by_id["ollama"]["configured"] is False
    assert by_id["ollama"]["fields"][0]["value"] is None

    assert config["tasks"]["categorise"]["ready"] is False
    assert "Settings > AI" in config["tasks"]["categorise"]["problem"]


def test_save_and_clear_provider_fields(client):
    def save(values):
        return client.put("/api/ai/providers/typesafe", json={"values": values}).status_code

    assert save({"api_key": "ts-1"}) == 204
    assert save({"nope": "x"}) == 400
    assert client.put("/api/ai/providers/unknown", json={"values": {}}).status_code == 404

    def typesafe():
        return _provider(client, "typesafe")

    assert typesafe()["configured"] is True
    client.put("/api/ai/providers/typesafe", json={"values": {"api_key": None}})
    assert typesafe()["configured"] is False


def test_choose_model_checks_its_kind(client):
    def choose(kind, provider, model="m"):
        return client.put(f"/api/ai/models/{kind}", json={"provider": provider, "model": model})

    assert choose("llm", "typesafe").status_code == 400
    assert choose("decision", "openai").status_code == 400
    assert choose("decision", "typesafe", "").status_code == 400
    assert choose("nope", "openai").status_code == 404
    assert choose("decision", "typesafe", "jev-latest").status_code == 204

    config = client.get("/api/ai/config").json()
    assert config["models"]["decision"]["provider"] == "typesafe"
    assert config["models"]["decision"]["ready"] is False
    assert "API key" in config["models"]["decision"]["problem"]

    client.put("/api/ai/providers/typesafe", json={"values": {"api_key": "ts-1"}})
    assert client.get("/api/ai/config").json()["models"]["decision"]["ready"] is True

    assert client.put("/api/ai/models/decision", json={"provider": None}).status_code == 204
    assert client.get("/api/ai/config").json()["models"]["decision"]["provider"] is None


def test_tasks_share_the_model_of_their_kind(client, db_session):
    set_provider_values(db_session, "typesafe", {"api_key": "ts-1"})
    set_provider_values(db_session, "openai", {"api_key": "sk-1"})
    for task in ("categorise", "chat", "rules"):
        client.put(f"/api/ai/tasks/{task}", json={"enabled": True})
    client.put("/api/ai/models/decision", json={"provider": "typesafe", "model": "jev-latest"})
    client.put("/api/ai/models/llm", json={"provider": "openai", "model": "gpt-5-mini"})

    tasks = client.get("/api/ai/config").json()["tasks"]
    # Tasks that can use either prefer the decision model, and run by themselves with it
    assert tasks["categorise"] == {
        **tasks["categorise"],
        "kinds": ["decision", "llm"],
        "kind": "decision",
        "provider": "typesafe",
        "mode": "automatic",
        "ready": True,
    }
    assert tasks["chat"]["provider"] == "openai" and tasks["chat"]["mode"] is None
    assert tasks["rules"]["kinds"] == ["llm"]

    # With a language model, tasks only run when asked unless told otherwise
    client.put("/api/ai/tasks/categorise", json={"kind": "llm"})
    task = client.get("/api/ai/config").json()["tasks"]["categorise"]
    assert task["provider"] == "openai" and task["mode"] == "on_demand"
    client.put("/api/ai/tasks/categorise", json={"mode": "automatic"})
    assert client.get("/api/ai/config").json()["tasks"]["categorise"]["mode"] == "automatic"

    client.put("/api/ai/tasks/categorise", json={"enabled": False})
    task = client.get("/api/ai/config").json()["tasks"]["categorise"]
    assert task["enabled"] is False and task["provider"] is None
    assert "turned off" in task["problem"]


def test_task_settings_are_checked(client):
    def update(task, **body):
        return client.put(f"/api/ai/tasks/{task}", json=body).status_code

    assert update("rules", kind="decision") == 400
    assert update("chat", mode="automatic") == 400
    assert update("categorise", mode="sometimes") == 400
    assert update("nope", enabled=True) == 404
    assert update("categorise", kind="llm", mode="on_demand", enabled=True) == 204


def test_new_installs_have_every_task_on(client, db_session):
    from pennychest.ai.config import TASKS
    from pennychest.settings.models import AppSetting

    for task in TASKS:
        db_session.delete(db_session.get(AppSetting, f"ai.task.{task}.enabled"))
    db_session.commit()
    tasks = client.get("/api/ai/config").json()["tasks"]
    assert all(t["enabled"] for t in tasks.values())


def test_config_endpoints_require_session(anon_client):
    assert anon_client.get("/api/ai/config").status_code == 401
    assert anon_client.put("/api/ai/providers/openai", json={"values": {}}).status_code == 401
    assert anon_client.post("/api/ai/providers/openai/models").status_code == 401
    assert anon_client.put("/api/ai/tasks/categorise", json={"enabled": False}).status_code == 401
    assert anon_client.put("/api/ai/models/llm", json={"provider": None}).status_code == 401


# Model lists


def test_refresh_needs_credentials(client):
    response = client.post("/api/ai/providers/openai/models")
    assert response.status_code == 400
    assert "API key" in response.json()["detail"]


def test_refresh_openai_models_keeps_chat_models(client, api):
    client.put("/api/ai/providers/openai", json={"values": {"api_key": "sk-1"}})
    api.on("GET", "https://api.openai.com/v1/models", (200, {"data": [
        {"id": "gpt-5-mini"}, {"id": "o4-mini"}, {"id": "text-embedding-3-small"},
        {"id": "gpt-realtime"}, {"id": "whisper-1"}, {"id": "gpt-4o-mini-tts"},
    ]}))

    response = client.post("/api/ai/providers/openai/models")
    assert response.status_code == 200
    assert [m["id"] for m in response.json()["models"]] == ["gpt-5-mini", "o4-mini"]
    assert api.requests[0].headers["authorization"] == "Bearer sk-1"

    openai = _provider(client, "openai")
    assert [m["id"] for m in openai["models"]] == ["gpt-5-mini", "o4-mini"]
    assert openai["models_updated_at"]


def test_custom_openai_base_url_lists_every_model(client, api):
    client.put("/api/ai/providers/openai", json={
        "values": {"api_key": "k", "base_url": "https://proxy.example/v1/"},
    })
    api.on("GET", "https://proxy.example/v1/models", (200, {"data": [
        {"id": "anthropic/claude-sonnet-5"}, {"id": "meta/llama"},
    ]}))
    models = client.post("/api/ai/providers/openai/models").json()["models"]
    assert [m["id"] for m in models] == ["anthropic/claude-sonnet-5", "meta/llama"]


def test_refresh_google_models(client, api):
    client.put("/api/ai/providers/google", json={"values": {"api_key": "g-1"}})
    api.on("GET", "https://generativelanguage.googleapis.com/v1beta/openai/models", (200, {"data": [
        {"id": "models/gemini-3.8-flash"}, {"id": "models/gemini-embedding-001"},
        {"id": "models/imagen-4"}, {"id": "gemini-3.1-pro-preview"},
    ]}))
    models = client.post("/api/ai/providers/google/models").json()["models"]
    assert [m["id"] for m in models] == ["gemini-3.1-pro-preview", "gemini-3.8-flash"]


def test_refresh_typesafe_and_ollama_models(client, api):
    client.put("/api/ai/providers/typesafe", json={"values": {"api_key": "ts-1"}})
    api.on("GET", "https://api.typesafe.ai/v1/models", (200, {"models": [
        {"name": "jev-latest", "description": "Stable", "release_date": "2026-09-01"},
        {"name": "jev-preview", "description": "Preview", "release_date": "2026-09-01"},
    ]}))
    assert [m["id"] for m in client.post("/api/ai/providers/typesafe/models").json()["models"]] == [
        "jev-latest", "jev-preview",
    ]

    client.put("/api/ai/providers/ollama", json={"values": {"url": "http://ollama:11434/"}})
    api.on("GET", "http://ollama:11434/api/tags", (200, {"models": [
        {"name": "qwen3:4b"}, {"name": "llama3.2:3b"},
    ]}))
    assert [m["id"] for m in client.post("/api/ai/providers/ollama/models").json()["models"]] == [
        "llama3.2:3b", "qwen3:4b",
    ]


def test_provider_errors_are_readable_and_keep_keys_private(client, api):
    client.put("/api/ai/providers/openai", json={"values": {"api_key": "sk-very-secret"}})
    api.on("GET", "https://api.openai.com/v1/models",
           (401, {"error": {"message": "Incorrect key"}}))
    response = client.post("/api/ai/providers/openai/models")
    assert response.status_code == 400
    assert "rejected the credentials" in response.json()["detail"]
    assert "sk-very-secret" not in response.text


def test_refresh_anthropic_models_skips_models_without_structured_outputs(client, monkeypatch):
    seen = {}

    def fake_client(api_key):
        seen["key"] = api_key
        return SimpleNamespace(models=SimpleNamespace(list=lambda: iter([
            SimpleNamespace(id="claude-opus-5", display_name="Claude Opus 5",
                            capabilities={"structured_outputs": {"supported": True}}),
            SimpleNamespace(id="claude-old", display_name="Claude Old",
                            capabilities={"structured_outputs": {"supported": False}}),
        ])))

    monkeypatch.setattr(providers, "anthropic_client", fake_client)
    client.put("/api/ai/providers/anthropic", json={"values": {"api_key": "sk-ant-1"}})
    models = client.post("/api/ai/providers/anthropic/models").json()["models"]
    assert models == [{"id": "claude-opus-5", "label": "Claude Opus 5"}]
    assert seen["key"] == "sk-ant-1"


# Categorisation


def test_categorise_needs_a_configured_task(client):
    _imported_transactions(client, "TESCO STORES")
    response = client.post("/api/ai/categorise/all")
    assert response.status_code == 400
    assert "Settings > AI" in response.json()["detail"]


def test_categorise_with_jev_uses_choices_and_confidence(client, api, db_session):
    (tesco, mystery), _ = _imported_transactions(client, "TESCO STORES 2041", "SQ *MYSTERY")
    set_provider_values(db_session, "typesafe", {"api_key": "ts-1"})
    use_model(db_session, "categorise", "typesafe", "jev-latest")
    api.on("POST", "https://api.typesafe.ai/v1/systemone", (200, {
        "model": "jev-1.13.0",
        "answers": {
            f"t{tesco}": {"type": "choice", "choice": "Expenses:Groceries",
                          "probabilities": {"Expenses:Groceries": 0.93}, "confidence": 0.9},
            f"t{mystery}": {"type": "choice", "choice": "Expenses:Transport",
                            "probabilities": {"Expenses:Transport": 0.86}, "confidence": 0.85},
        },
        "usage": {"input_tokens": 500, "output_tokens": 40},
    }))

    response = client.post("/api/ai/categorise/all")
    assert response.status_code == 200
    assert response.json() == {"updated": 1}
    assert _expense_account(client, tesco) == "Expenses:Groceries"
    assert _expense_account(client, mystery) == "Expenses:Uncategorised"

    sent = api.body()
    assert sent["model"] == "jev-latest"
    assert sent["state"] == {f"t{tesco}": "TESCO STORES 2041", f"t{mystery}": "SQ *MYSTERY"}
    question = sent["questions"][f"t{tesco}"]
    assert question["type"] == "choice"
    assert "Expenses:Groceries" in question["criteria"]
    assert "Expenses:Uncategorised" not in question["criteria"]
    assert api.requests[-1].headers["authorization"] == "Bearer ts-1"

    log = client.get("/api/ai/logs").json()["logs"][0]
    assert log["provider"] == "typesafe:jev-latest" and log["error"] is None


def test_categorise_with_openai_structured_output(client, api, db_session):
    (tesco,), _ = _imported_transactions(client, "TESCO STORES")
    set_provider_values(db_session, "openai", {"api_key": "sk-1"})
    use_model(db_session, "categorise", "openai", "gpt-5-mini")
    content = json.dumps({"categorisations": [
        {"transaction_id": tesco, "account_full_path": "Expenses:Groceries"},
        {"transaction_id": 99999, "account_full_path": "Expenses:Groceries"},
    ]})
    api.on("POST", "https://api.openai.com/v1/chat/completions", (200, {
        "choices": [{"message": {"content": content}, "finish_reason": "stop"}],
    }))

    assert client.post("/api/ai/categorise/all").json() == {"updated": 1}
    assert _expense_account(client, tesco) == "Expenses:Groceries"
    sent = api.body()
    assert sent["model"] == "gpt-5-mini"
    schema = sent["response_format"]["json_schema"]
    assert schema["strict"] is True
    item = schema["schema"]["properties"]["categorisations"]["items"]
    assert "Expenses:Groceries" in item["properties"]["account_full_path"]["enum"]


def test_categorise_with_anthropic_structured_output(client, db_session, monkeypatch):
    (tesco,), _ = _imported_transactions(client, "TESCO STORES")
    calls = []

    def create(**request):
        calls.append(request)
        text = json.dumps({"categorisations": [
            {"transaction_id": tesco, "account_full_path": "Expenses:Groceries"},
        ]})
        return SimpleNamespace(
            stop_reason="end_turn",
            content=[SimpleNamespace(type="text", text=text)],
            model_dump=lambda mode=None: {"id": "msg_1"},
        )

    monkeypatch.setattr(providers, "anthropic_client",
                        lambda api_key: SimpleNamespace(messages=SimpleNamespace(create=create)))
    set_provider_values(db_session, "anthropic", {"api_key": "sk-ant-1"})
    use_model(db_session, "categorise", "anthropic", "claude-haiku-4-5")

    assert client.post("/api/ai/categorise/all").json() == {"updated": 1}
    request = calls[0]
    assert request["model"] == "claude-haiku-4-5"
    assert request["output_config"]["format"]["type"] == "json_schema"
    assert "tool_choice" not in request


def test_anthropic_refusal_is_reported(client, db_session, monkeypatch):
    _imported_transactions(client, "TESCO STORES")
    monkeypatch.setattr(providers, "anthropic_client", lambda api_key: SimpleNamespace(
        messages=SimpleNamespace(
            create=lambda **r: SimpleNamespace(stop_reason="refusal", content=[])
        )
    ))
    set_provider_values(db_session, "anthropic", {"api_key": "sk-ant-1"})
    use_model(db_session, "categorise", "anthropic", "claude-opus-5")

    response = client.post("/api/ai/categorise/all")
    assert response.status_code == 400
    assert "declined" in response.json()["detail"]
    assert client.get("/api/ai/logs").json()["logs"][0]["error"]


# Rule suggestions


def test_suggest_rules_with_ollama(client, api, db_session):
    _imported_transactions(client, "TESCO STORES")
    set_provider_values(db_session, "ollama", {"url": "http://ollama:11434"})
    use_model(db_session, "rules", "ollama", "qwen3:4b")
    content = json.dumps({"rules": [
        {"pattern": "TESCO", "match_type": "substring",
         "target_account_full_path": "Expenses:Groceries", "priority": 500,
         "description": "Supermarket"},
        {"pattern": "X", "match_type": "fuzzy", "target_account_full_path": "Expenses:Groceries",
         "priority": 10, "description": "bad match type"},
        {"pattern": "Y", "match_type": "prefix", "target_account_full_path": "Expenses:Invented",
         "priority": 10, "description": "unknown account"},
    ]})
    api.on("POST", "http://ollama:11434/api/chat", (200, {"message": {"content": content}}))

    response = client.post("/api/ai/suggest-rules")
    assert response.status_code == 200
    suggestions = response.json()["suggestions"]
    assert len(suggestions) == 1
    assert suggestions[0]["pattern"] == "TESCO" and suggestions[0]["priority"] == 100
    sent = api.body()
    assert sent["model"] == "qwen3:4b" and sent["stream"] is False
    assert sent["format"]["properties"]["rules"]["items"]["properties"]["match_type"]["enum"]


def test_rules_need_a_language_model(client, db_session):
    set_provider_values(db_session, "typesafe", {"api_key": "ts-1"})
    use_model(db_session, "rules", "typesafe", "jev-latest")
    response = client.post("/api/ai/suggest-rules")
    assert response.status_code == 400
    assert "Choose a language model" in response.json()["detail"]
